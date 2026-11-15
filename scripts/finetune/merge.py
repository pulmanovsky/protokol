#!/usr/bin/env python3
"""Folds a LoRA adapter from train.py into the base model and saves one
model in Hugging Face format (fp16), ready for whisper.cpp's
models/convert-h5-to-ggml.py. The result has heard real meetings: keep it
on this Mac, never publish or commit it."""
import argparse
import json
import pathlib
import shutil

import torch
from peft import PeftModel
from transformers import WhisperForConditionalGeneration, WhisperProcessor


# whisper.cpp's converter reads the vocabulary from these files, which
# newer transformers no longer writes: copy them from the base model.
TOKENIZER_FILES = ("vocab.json", "added_tokens.json", "merges.txt", "normalizer.json")


def base_file(base, name):
    local = pathlib.Path(base) / name
    if local.exists():
        return local
    try:
        from huggingface_hub import hf_hub_download

        return pathlib.Path(hf_hub_download(base, name))
    except Exception:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("adapter", type=pathlib.Path)
    ap.add_argument("out", type=pathlib.Path)
    a = ap.parse_args()
    base = json.loads((a.adapter / "adapter_config.json").read_text())["base_model_name_or_path"]
    model = WhisperForConditionalGeneration.from_pretrained(base, dtype=torch.float32)
    model = PeftModel.from_pretrained(model, a.adapter).merge_and_unload()
    model = model.to(torch.float16)
    model.save_pretrained(a.out, safe_serialization=False)
    WhisperProcessor.from_pretrained(base).save_pretrained(a.out)
    for name in TOKENIZER_FILES:
        src = base_file(base, name)
        if src:
            shutil.copy(src, a.out / name)
    missing = [n for n in ("vocab.json", "added_tokens.json") if not (a.out / n).exists()]
    if missing:
        raise SystemExit(f"could not copy {missing} from {base}; whisper.cpp's converter needs them")
    print(f"merged {a.adapter} into {base} → {a.out}")


if __name__ == "__main__":
    main()
