#!/usr/bin/env python3
"""The trainer the app runs at night: fine-tunes Whisper on the
organisation's own checked meetings with MLX, on the Mac's GPU.

    protokol_train.py <job.json>

The app writes the job (`learning::job` in the app's code): the original
whisper.cpp model to start from, the pieces to train on and to watch the
loss on, the prompt recognition uses, and the settings of v5, the pilot
model that beat the original on 15.11 (LoRA rank 32 on the decoder, lr 2e-4).

On stdout one JSON object per line, as the app reads them:
  {"event":"started","pieces":N,"epochs":E}
  {"event":"step","epoch":e,"step":i,"of":N,"loss":x,"seconds_per_piece":s,"memory_gb":m}
  {"event":"epoch","epoch":e,"valid_loss":x}   (epoch 0: before training)
  {"event":"converting"}
  {"event":"done","minutes":x,"peak_memory_gb":m}   or   {"event":"failed","reason":"..."}
Anything else goes to stderr. The finished model goes to the job's
`ggml_out`, in the same format as the original: a copy of it with the
decoder's weights replaced by the trained ones.

The trainer stops by itself when its stdin closes: the app quit or crashed.
Nothing leaves the Mac.
"""
import json
import math
import os
import random
import sys
import threading
import time
import wave

import numpy as np

# The engine's own modules sit next to this file (also when run with -I)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ggml_whisper  # noqa: E402

# Settings of the job that are not in it: as in scripts/finetune/train.py
ACCUMULATE = 8
LORA_DROPOUT = 0.05
WEIGHT_DECAY = 0.01
CLIP = 1.0
BUCKET = 64
FREE_EVERY = 20
# MLX keeps freed buffers for reuse; more than this is given back at once
CACHE_LIMIT_GB = 2
SEED = 1
# Linear layers of each decoder block that get adapters
TARGETS = [("attn", "query"), ("attn", "key"), ("attn", "value"), ("attn", "out"), ("cross_attn", "query"), ("cross_attn", "key"), ("cross_attn", "value"), ("cross_attn", "out"), (None, "mlp1"), (None, "mlp2")]


def say(event, **fields):
    print(json.dumps({"event": event, **fields}, ensure_ascii=False), flush=True)


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def watch_stdin():
    """Exits when stdin closes (the app is gone); not when run by hand in a terminal"""
    if sys.stdin is None or sys.stdin.isatty():
        return

    def wait():
        try:
            while sys.stdin.buffer.read(4096):
                pass
        except Exception:  # noqa: BLE001 - the interpreter is shutting down
            return
        os._exit(3)

    threading.Thread(target=wait, daemon=True).start()


def read_wav(path):
    with wave.open(str(path), "rb") as w:
        if w.getframerate() != 16000 or w.getnchannels() != 1 or w.getsampwidth() != 2:
            raise ValueError("a piece is not 16 kHz mono 16-bit")
        data = w.readframes(w.getnframes())
    return np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0


def load_pieces(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def main(job_path):
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim
    from mlx.utils import tree_flatten, tree_map
    from whisper_mlx.audio import N_FRAMES, N_SAMPLES, log_mel_spectrogram
    from whisper_mlx.tokenizer import get_tokenizer

    began = time.time()
    with open(job_path, encoding="utf-8") as f:
        job = json.load(f)
    if job.get("version") != 1:
        raise ValueError(f"job version {job.get('version')} is not supported")
    random.seed(SEED)
    mx.random.seed(SEED)

    class LoRALinear(nn.Module):
        """A frozen linear layer plus a trained low-rank addition (as PEFT's LoRA)"""

        def __init__(self, linear, rank, alpha):
            super().__init__()
            out_dims, in_dims = linear.weight.shape
            self.linear = linear
            bound = 1.0 / math.sqrt(in_dims)
            self.lora_a = mx.random.uniform(-bound, bound, (in_dims, rank))
            self.lora_b = mx.zeros((rank, out_dims))
            self.scale = alpha / rank
            self.dropout = nn.Dropout(LORA_DROPOUT)

        def __call__(self, x):
            y = self.linear(x)
            # The addition is computed in full precision, whatever the layer's
            return y + (self.scale * ((self.dropout(x) @ self.lora_a) @ self.lora_b)).astype(y.dtype)

        def merged(self):
            """The linear layer's weight with the addition put in, float32"""
            return self.linear.weight.astype(mx.float32) + self.scale * (self.lora_a @ self.lora_b).T

    base = job["base_model"]
    log(f"loading {os.path.basename(base)}")
    mx.set_cache_limit(CACHE_LIMIT_GB * 2**30)
    # "full": the decoder, where the gradients flow, in full precision
    # (about 12 GB on large-v3); "half": everything in half precision as the
    # file stores it, the added weights still in full (for 16 GB Macs)
    precision = job.get("precision", "full")
    if precision not in ("full", "half"):
        raise ValueError(f"unknown precision {precision}")
    dtype = mx.float16 if precision == "half" else mx.float32
    model, hp = ggml_whisper.load_model(base, dtype)
    # The encoder is frozen and only reads the audio: half precision, as
    # whisper.cpp and mlx-whisper run it, saves 1.3 GB on large-v3
    model.encoder.update(tree_map(lambda p: p.astype(mx.float16), model.encoder.parameters()))
    model.encoder._positional_embedding = model.encoder._positional_embedding.astype(mx.float16)
    model.freeze()
    rank = int(job["lora_rank"])
    adapters = {}
    for b, block in enumerate(model.decoder.blocks):
        for part, name in TARGETS:
            owner = getattr(block, part) if part else block
            lora = LoRALinear(getattr(owner, name), rank, 2 * rank)
            setattr(owner, name, lora)
            ggml_name = f"decoder.blocks.{b}." + (f"{part}.{name}" if part else {"mlp1": "mlp.0", "mlp2": "mlp.2"}[name]) + ".weight"
            adapters[ggml_name] = lora
    model.freeze()
    for lora in adapters.values():
        lora.unfreeze(keys=["lora_a", "lora_b"], recurse=False)
    trainable = sum(v.size for _, v in tree_flatten(model.trainable_parameters()))
    log(f"trainable parameters: {trainable / 1e6:.1f} M")

    tok = get_tokenizer(True, num_languages=model.num_languages, language=job.get("language", "uk"), task="transcribe")
    prefix = list(tok.sot_sequence_including_notimestamps)
    prompt_ids = tok.encode(job.get("prompt", "").strip()) if job.get("prompt", "").strip() else []
    limit = hp["n_text_ctx"]

    def padded(n):
        return min(limit, -(-n // BUCKET) * BUCKET)

    def example(item):
        """Audio features, the decoder's input and what it should predict
        (-100: not learned). The decoder reads <|startofprev|> prompt
        <|startoftranscript|><|uk|><|transcribe|><|notimestamps|> text, as
        whisper.cpp in the app, and learns only what follows the start token."""
        audio = read_wav(item["file"])[:N_SAMPLES]
        audio = np.pad(audio, (0, N_SAMPLES - len(audio)))
        mel = log_mel_spectrogram(audio, n_mels=hp["n_mels"])[:N_FRAMES]
        text = tok.encode(item["text"]) + [tok.eot]
        room = max(0, limit - 1 - len(prefix) - len(text))
        prompt = prompt_ids[-room:] if room and prompt_ids else []
        seq = ([tok.sot_prev] + prompt if prompt else []) + prefix + text
        if len(seq) - 1 > limit:
            return None
        start = len(prompt) + 1 if prompt else 0  # where <|startoftranscript|> is
        # Position t predicts token t + 1; from the start token on it is learned
        inputs = seq[:-1]
        labels = [-100] * start + seq[start + 1:]
        extra = padded(len(inputs)) - len(inputs)
        inputs, labels = inputs + [tok.eot] * extra, labels + [-100] * extra
        return mel[None], mx.array([inputs]), mx.array([labels])

    def loss_fn(model, features, inputs, labels):
        logits = model.decoder(inputs, features)[0].astype(mx.float32)
        mask = labels != -100
        ce = nn.losses.cross_entropy(logits, mx.where(mask, labels, 0), reduction="none")
        return (ce * mask).sum() / mx.maximum(mask.sum(), 1)

    def features_of(mel):
        # The encoder is frozen: no gradient goes through it
        return mx.stop_gradient(model.encoder(mel.astype(mx.float16))).astype(dtype)

    def memory_gb():
        return (mx.get_active_memory() + mx.get_cache_memory()) / 2**30

    train = load_pieces(job["train"])
    valid = load_pieces(job["valid"])
    epochs = int(job["epochs"])
    if not train:
        raise ValueError("nothing to train on")

    def valid_loss():
        if not valid:
            return None
        model.eval()
        total, n = 0.0, 0
        for item in valid:
            ex = example(item)
            if ex is None:
                continue
            mel, inputs, labels = ex
            loss = loss_fn(model, features_of(mel), inputs, labels)
            mx.eval(loss)
            total += loss.item()
            n += 1
        model.train()
        mx.clear_cache()
        return total / max(n, 1)

    say("started", pieces=len(train), epochs=epochs)
    lr = float(job["learning_rate"])
    # As PyTorch's AdamW: MLX leaves bias correction off by default, which
    # makes the first few hundred steps several times larger (in effect a
    # higher learning rate; on the pilot's data 17.3% instead of 15.8%)
    opt = optim.AdamW(learning_rate=lr, weight_decay=WEIGHT_DECAY, bias_correction=True)
    steps = math.ceil(len(train) / ACCUMULATE) * epochs
    warmup = max(1, steps // 20)

    def lr_at(s):
        # Linear warm-up, then linear decay to zero
        return lr * ((s + 1) / warmup if s < warmup else max(0.0, (steps - s) / max(1, steps - warmup)))

    loss_and_grad = nn.value_and_grad(model, loss_fn)
    # The loss before training: the same as the original model's (a check of the whole setup)
    v = valid_loss()
    say("epoch", epoch=0, valid_loss=round(v, 4) if v is not None else None)
    model.train()
    peak = 0.0
    times = []
    best = (math.inf, None)
    opt_step = 0
    grads = None
    loss_value = 0.0
    for epoch in range(1, epochs + 1):
        random.shuffle(train)
        for i, item in enumerate(train, 1):
            t0 = time.time()
            ex = example(item)
            if ex is not None:
                mel, inputs, labels = ex
                loss, g = loss_and_grad(model, features_of(mel), inputs, labels)
                g = tree_map(lambda x: x / ACCUMULATE, g)
                grads = g if grads is None else tree_map(lambda a, b: a + b, grads, g)
                mx.eval(loss, grads)
                loss_value = loss.item()
            if grads is not None and (i % ACCUMULATE == 0 or i == len(train)):
                grads, _ = optim.clip_grad_norm(grads, CLIP)
                opt.learning_rate = lr_at(opt_step)
                opt.update(model, grads)
                mx.eval(model.trainable_parameters(), opt.state)
                grads = None
                opt_step += 1
            if i % FREE_EVERY == 0:
                mx.clear_cache()
            times.append(time.time() - t0)
            times = times[-8:]
            mem = memory_gb()
            peak = max(peak, mem)
            # The median of the last pieces: one slow piece (a pause for a recording) does not count
            say("step", epoch=epoch, step=i, of=len(train), loss=round(loss_value, 4), seconds_per_piece=round(float(np.median(times)), 3), memory_gb=round(mem, 2))
        v = valid_loss()
        say("epoch", epoch=epoch, valid_loss=round(v, 4) if v is not None else None)
        # The adapters of the epoch with the lowest loss on the watched pieces
        if v is None or v < best[0]:
            best = (v if v is not None else -math.inf, {k: (np.array(l.lora_a), np.array(l.lora_b)) for k, l in adapters.items()})

    say("converting")
    replaced = {}
    for name, lora in adapters.items():
        a, b = best[1][name]
        lora.lora_a, lora.lora_b = mx.array(a), mx.array(b)
        replaced[name] = np.array(lora.merged())
    ggml_whisper.write_patched(base, job["ggml_out"], replaced)
    peak = max(peak, mx.get_peak_memory() / 2**30)
    say("done", minutes=round((time.time() - began) / 60, 2), peak_memory_gb=round(peak, 2))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    watch_stdin()
    try:
        main(sys.argv[1])
        code = 0
    except Exception as e:  # noqa: BLE001 - the app needs a reason, whatever it is
        say("failed", reason=f"{type(e).__name__}: {e}"[:300])
        code = 1
    # Not waiting for the stdin watcher
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(code)
