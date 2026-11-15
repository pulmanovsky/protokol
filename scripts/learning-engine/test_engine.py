#!/usr/bin/env python3
"""Checks the trainer end to end on a tiny random model (runs on any
machine with MLX, CPU included; takes about a minute):

- it speaks the app's protocol, line by line;
- the new model file differs from the original only in the decoder's
  trained weights, and reads back as the trained model;
- it stops by itself when its stdin closes.

    python test_engine.py
"""
import json
import math
import os
import struct
import subprocess
import sys
import tempfile
import time
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import ggml_whisper  # noqa: E402

DIMS = dict(n_vocab=51866, n_audio_ctx=1500, n_audio_state=64, n_audio_head=2, n_audio_layer=1, n_text_ctx=448, n_text_state=64, n_text_head=2, n_text_layer=2, n_mels=128)


def openai_name(name):
    return name.replace(".mlp1.", ".mlp.0.").replace(".mlp2.", ".mlp.2.")


def write_tiny_model(path):
    """A whisper.cpp file with random weights and the large-v3 vocabulary, as convert-pt-to-ggml.py writes it"""
    import mlx.core as mx
    from mlx.utils import tree_flatten
    from whisper_mlx import model as whisper
    from whisper_mlx.audio import mel_filters

    mx.random.seed(7)
    model = whisper.Whisper(whisper.ModelDimensions(**DIMS), mx.float32)
    tensors = []
    for name, v in tree_flatten(model.parameters()):
        a = np.array(v).astype(np.float32)
        if name.endswith("positional_embedding") and name.startswith("decoder"):
            a = np.random.default_rng(1).normal(0, 0.02, a.shape).astype(np.float32)
        if name in ("encoder.conv1.weight", "encoder.conv2.weight"):
            a = a.transpose(0, 2, 1)
        if name in ("encoder.conv1.bias", "encoder.conv2.bias"):
            a = a.reshape(-1, 1)
        tensors.append((openai_name(name), a))
    tensors.append(("encoder.positional_embedding", np.array(whisper.sinusoids(DIMS["n_audio_ctx"], DIMS["n_audio_state"])).astype(np.float32)))
    filters = np.array(mel_filters(128)).astype(np.float32)
    with open(path, "wb") as f:
        f.write(struct.pack("<i", ggml_whisper.MAGIC))
        f.write(struct.pack("<11i", *[DIMS[k] for k in ggml_whisper.HPARAMS[:-1]], 1))
        f.write(struct.pack("<2i", *filters.shape))
        f.write(filters.tobytes())
        tokens = [bytes([b]) for b in range(256)]
        f.write(struct.pack("<i", len(tokens)))
        for t in tokens:
            f.write(struct.pack("<i", len(t)) + t)
        for name, a in tensors:
            small = a.ndim < 2 or name in ("encoder.conv1.bias", "encoder.conv2.bias", "encoder.positional_embedding", "decoder.positional_embedding")
            ftype = 0 if small else 1
            data = a.astype(np.float32 if small else np.float16)
            raw = name.encode()
            f.write(struct.pack("<3i", data.ndim, len(raw), ftype))
            f.write(struct.pack(f"<{data.ndim}i", *reversed(data.shape)))
            f.write(raw)
            f.write(data.tobytes())


def write_wav(path, seconds, freq):
    t = np.arange(int(seconds * 16000)) / 16000
    s = (0.3 * np.sin(2 * math.pi * freq * t) * 32767).astype(np.int16)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(s.tobytes())


def make_job(work, base, pieces):
    os.makedirs(os.path.join(work, "train"), exist_ok=True)
    texts = ["Доповідає керівник першого відділу", "Завдання виконано в повному обсязі", "Три тисячі вісімнадцять"]
    items = []
    for k in range(pieces):
        f = os.path.join(work, "train", f"{k:05}.wav")
        write_wav(f, 1.0 + (k % 3), 200 + 50 * (k % 3))
        items.append({"file": f, "text": texts[k % 3]})
    with open(os.path.join(work, "train.jsonl"), "w", encoding="utf-8") as f:
        f.writelines(json.dumps(i, ensure_ascii=False) + "\n" for i in items)
    with open(os.path.join(work, "valid.jsonl"), "w", encoding="utf-8") as f:
        f.writelines(json.dumps(i, ensure_ascii=False) + "\n" for i in items[:2])
    job = dict(version=1, base_model=base, train=os.path.join(work, "train.jsonl"), valid=os.path.join(work, "valid.jsonl"), prompt="ЗСУ. Гончаренко. Протокол наради.", language="uk", lora_rank=4, learning_rate=1e-2, epochs=2, work_dir=work, ggml_out=os.path.join(work, "ggml-model.bin"))
    path = os.path.join(work, "job.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(job, f, ensure_ascii=False)
    return path, job


def run(job_path, close_stdin_after=None):
    p = subprocess.Popen([sys.executable, os.path.join(HERE, "protokol_train.py"), job_path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    events = []
    for line in p.stdout:
        events.append(json.loads(line))
        if close_stdin_after and events[-1]["event"] == close_stdin_after:
            p.stdin.close()
            break
    return p, events


def main():
    tmp = tempfile.mkdtemp(prefix="protokol-engine-")
    base = os.path.join(tmp, "ggml-large-v3.bin")
    write_tiny_model(base)
    hp, before = ggml_whisper.read_index(base)
    assert hp["n_text_layer"] == 2 and len(before) > 30

    # 1. A whole run
    job_path, job = make_job(os.path.join(tmp, "run"), base, 10)
    p, events = run(job_path)
    assert p.wait(timeout=600) == 0, events[-1:]
    kinds = [e["event"] for e in events]
    assert kinds[0] == "started" and events[0] == {"event": "started", "pieces": 10, "epochs": 2}, events[0]
    steps = [e for e in events if e["event"] == "step"]
    assert len(steps) == 20 and steps[-1]["step"] == 10 and steps[-1]["of"] == 10 and steps[-1]["epoch"] == 2
    assert all(isinstance(e["loss"], float) and e["seconds_per_piece"] > 0 for e in steps)
    assert [e["epoch"] for e in events if e["event"] == "epoch"] == [0, 1, 2]
    assert kinds[-2:] == ["converting", "done"], kinds[-3:]
    first, last = steps[0]["loss"], steps[-1]["loss"]
    print(f"loss {first:.3f} → {last:.3f}; valid {[e['valid_loss'] for e in events if e['event'] == 'epoch']}")
    assert last < first, "the loss should fall on the same three sentences"

    # 2. The new file: the same as the original but for the decoder's trained weights
    _, after = ggml_whisper.read_index(job["ggml_out"])
    assert list(after) == list(before)
    changed = [n for n in before if not np.array_equal(ggml_whisper.read_tensor(base, before[n]), ggml_whisper.read_tensor(job["ggml_out"], after[n]))]
    assert changed and all(n.startswith("decoder.blocks.") and n.endswith(".weight") and ("attn." in n or ".mlp." in n) for n in changed), changed
    assert len(changed) == 2 * 10, len(changed)
    assert os.path.getsize(job["ggml_out"]) == os.path.getsize(base)
    with open(base, "rb") as a, open(job["ggml_out"], "rb") as b:
        assert a.read(before["encoder.conv1.weight"].offset) == b.read(before["encoder.conv1.weight"].offset), "header and vocabulary kept"
    import mlx.core as mx

    model, _ = ggml_whisper.load_model(job["ggml_out"], mx.float32)
    assert model.decoder.blocks[0].mlp1.weight.shape == (256, 64)

    # 2b. Half precision (16 GB Macs): the same kind of run, the same kind of file
    job_path, job = make_job(os.path.join(tmp, "half"), base, 10)
    job["precision"] = "half"
    with open(job_path, "w", encoding="utf-8") as f:
        json.dump(job, f, ensure_ascii=False)
    p, events = run(job_path)
    assert p.wait(timeout=600) == 0, events[-1:]
    steps = [e for e in events if e["event"] == "step"]
    assert len(steps) == 20 and all(math.isfinite(e["loss"]) for e in steps), steps[-1:]
    assert steps[-1]["loss"] < steps[0]["loss"], (steps[0], steps[-1])
    _, half = ggml_whisper.read_index(job["ggml_out"])
    changed = [n for n in before if not np.array_equal(ggml_whisper.read_tensor(base, before[n]), ggml_whisper.read_tensor(job["ggml_out"], half[n]))]
    assert len(changed) == 2 * 10 and all(n.startswith("decoder.blocks.") for n in changed), changed
    print(f"half: loss {steps[0]['loss']:.3f} → {steps[-1]['loss']:.3f}")

    # 3. Stdin closed: it stops by itself
    job_path, _ = make_job(os.path.join(tmp, "quit"), base, 400)
    began = time.time()
    p, events = run(job_path, close_stdin_after="step")
    code = p.wait(timeout=30)
    assert code == 3 and time.time() - began < 30, code
    assert not os.path.exists(os.path.join(tmp, "quit", "ggml-model.bin"))

    # 4. A broken job fails with a reason
    bad = os.path.join(tmp, "bad.json")
    with open(bad, "w") as f:
        json.dump({"version": 9}, f)
    p, events = run(bad)
    assert p.wait(timeout=60) == 1 and events[-1]["event"] == "failed" and "version 9" in events[-1]["reason"], events
    print("ok")


if __name__ == "__main__":
    main()
