#!/usr/bin/env python3
"""LoRA fine-tuning of Whisper on the pieces listed by prepare.py.

Low-rank adapters on the decoder's attention and feed-forward layers (and
the encoder's with --encoder); the base model stays frozen. Saves the
adapter after each epoch (adapter-1, …, adapter-final) and logs the loss on
the held-out pieces, so a run stopped midway still leaves something to test.
"""
import argparse
import json
import math
import pathlib
import random
import time
import wave

import numpy as np
import torch
from peft import LoraConfig, get_peft_model
from transformers import WhisperForConditionalGeneration, WhisperProcessor


def read_wav(path):
    with wave.open(path, "rb") as w:
        assert w.getframerate() == 16000 and w.getnchannels() == 1 and w.getsampwidth() == 2, path
        data = w.readframes(w.getnframes())
    return np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0


def load(path):
    return [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("work", type=pathlib.Path, help="folder with train.jsonl and test.jsonl from prepare.py")
    ap.add_argument("--base", default="openai/whisper-large-v3")
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--lr", type=float, default=5e-4)
    ap.add_argument("--accumulate", type=int, default=8)
    ap.add_argument("--rank", type=int, default=32)
    ap.add_argument("--encoder", action="store_true", help="adapt the encoder too (slower, more memory)")
    ap.add_argument("--limit", type=int, default=0, help="use only this many training pieces (a quick try)")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--bucket", type=int, default=64, help="pad the decoder's tokens to a multiple of this, so the GPU sees few shapes (its cache grows with every new one)")
    ap.add_argument("--free-every", type=int, default=20, help="return cached GPU memory every this many pieces")
    ap.add_argument("--prompt-file", type=pathlib.Path, help="the app's initial prompt (eval stage «prompt»): teach with it in front, as the app decodes")
    a = ap.parse_args()
    random.seed(a.seed)
    torch.manual_seed(a.seed)
    device = "mps" if torch.backends.mps.is_available() else ("cuda" if torch.cuda.is_available() else "cpu")

    proc = WhisperProcessor.from_pretrained(a.base, language="uk", task="transcribe")
    model = WhisperForConditionalGeneration.from_pretrained(a.base, dtype=torch.float32)
    model.config.forced_decoder_ids = None
    model.config.suppress_tokens = []
    targets = ["q_proj", "k_proj", "v_proj", "out_proj", "fc1", "fc2"]
    if a.encoder:
        modules = targets
    else:
        # Decoder only: its layers are named model.decoder.layers.N.…
        modules = [n for n, m in model.named_modules() if ".decoder." in n and n.split(".")[-1] in targets]
    model = get_peft_model(model, LoraConfig(r=a.rank, lora_alpha=2 * a.rank, lora_dropout=0.05, target_modules=modules, bias="none"))
    model.print_trainable_parameters()
    model.to(device)

    train = load(a.work / "train.jsonl")
    test = load(a.work / "test.jsonl")
    if a.limit:
        random.shuffle(train)
        train = train[: a.limit]
    start_id = model.config.decoder_start_token_id

    tok = proc.tokenizer
    prompt_ids = tok(a.prompt_file.read_text(encoding="utf-8").strip(), add_special_tokens=False).input_ids if a.prompt_file else []
    prev_id = tok.convert_tokens_to_ids("<|startofprev|>")
    limit = model.config.max_target_positions

    def batch_of(item):
        """Audio features, the decoder's input (None: the model builds it
        from the labels) and the labels. With a prompt the decoder reads
        <|startofprev|> prompt <|startoftranscript|><|uk|><|transcribe|>
        <|notimestamps|> text, as whisper.cpp in the app, and learns only
        what follows the start token."""
        audio = read_wav(item["audio"])
        feats = proc.feature_extractor(audio, sampling_rate=16000, return_tensors="pt").input_features.to(device)
        if not prompt_ids:
            ids = tok(item["text"]).input_ids
            if ids and ids[0] == start_id:
                ids = ids[1:]  # the model puts the start token in front itself
            labels = ids + [-100] * (padded(len(ids)) - len(ids))  # the model reads -100 as padding
            return feats, None, torch.tensor([labels], device=device)
        text = tok(item["text"], add_special_tokens=False).input_ids + [tok.eos_token_id]
        prefix = tok.prefix_tokens  # start, language, task, no timestamps
        room = max(0, limit - 1 - len(prefix) - len(text))
        prompt = prompt_ids[-room:] if room else []
        seq = [prev_id] + prompt + prefix + text
        inputs, labels = seq[:-1], [-100] * (len(prompt) + 1) + seq[len(prompt) + 2:]
        # Padding at the end changes nothing before it (each token sees only
        # earlier ones) and is not learned (-100)
        extra = padded(len(inputs)) - len(inputs)
        inputs, labels = inputs + [tok.eos_token_id] * extra, labels + [-100] * extra
        return feats, torch.tensor([inputs], device=device), torch.tensor([labels], device=device)

    def padded(n):
        return min(limit, -(-n // a.bucket) * a.bucket) if a.bucket > 1 else n

    def free():
        if device == "mps":
            torch.mps.empty_cache()
        elif device == "cuda":
            torch.cuda.empty_cache()

    def loss_of(item):
        f, d, l = batch_of(item)
        return model(input_features=f, decoder_input_ids=d, labels=l).loss

    def test_loss():
        model.eval()
        total, n = 0.0, 0
        with torch.no_grad():
            for item in test:
                total += loss_of(item).item()
                n += 1
                if n % a.free_every == 0:
                    free()
        model.train()
        return total / max(n, 1)

    log = open(a.work / "train.log", "a", encoding="utf-8")

    def memory():
        if device == "mps":
            return f"; memory {torch.mps.driver_allocated_memory() / 2**30:.1f} GB (tensors {torch.mps.current_allocated_memory() / 2**30:.1f})"
        if device == "cuda":
            return f"; memory {torch.cuda.max_memory_allocated() / 2**30:.1f} GB"
        return ""

    def say(msg):
        line = f"{time.strftime('%H:%M:%S')} {msg}"
        print(line, flush=True)
        log.write(line + "\n")
        log.flush()

    say(f"device {device}; train {len(train)} pieces, test {len(test)}; epochs {a.epochs}; lr {a.lr}; encoder {'yes' if a.encoder else 'no'}; prompt {len(prompt_ids)} tokens")
    say(f"test loss before: {test_loss():.4f}")
    opt = torch.optim.AdamW([p for p in model.parameters() if p.requires_grad], lr=a.lr, weight_decay=0.01)
    steps = math.ceil(len(train) / a.accumulate) * a.epochs
    warmup = max(1, steps // 20)
    # Linear warm-up, then linear decay to zero
    sched = torch.optim.lr_scheduler.LambdaLR(opt, lambda s: (s + 1) / warmup if s < warmup else max(0.0, (steps - s) / max(1, steps - warmup)))
    model.train()
    began = time.time()
    done = 0
    for epoch in range(1, a.epochs + 1):
        random.shuffle(train)
        running, k = 0.0, 0
        for i, item in enumerate(train, 1):
            loss = loss_of(item) / a.accumulate
            loss.backward()
            running += loss.item() * a.accumulate
            k += 1
            if i % a.accumulate == 0 or i == len(train):
                torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
                opt.step()
                sched.step()
                opt.zero_grad()
            done += 1
            if done % a.free_every == 0:
                free()
            if done % 50 == 0:
                left = (time.time() - began) / done * (len(train) * a.epochs - done)
                say(f"epoch {epoch} piece {i}/{len(train)} train loss {running / k:.4f}; {(time.time() - began) / done:.2f} s a piece, about {left / 60:.0f} min left{memory()}")
                running, k = 0.0, 0
        out = a.work / ("adapter-final" if epoch == a.epochs else f"adapter-{epoch}")
        model.save_pretrained(out)
        say(f"epoch {epoch} done in {(time.time() - began) / 60:.1f} min ({(time.time() - began) / done:.2f} s a piece{memory()}); saved {out.name}")
        say(f"epoch {epoch} test loss {test_loss():.4f}")


if __name__ == "__main__":
    main()
