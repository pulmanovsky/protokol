# Whisper fine-tuning on checked meetings (local only)

Teaches Whisper large-v3 the words of one organisation's meetings from the
secretary's own checked text. Everything runs on one Mac; the audio, the text
and the resulting model never leave it. **The model is never published or
committed**: it has heard real meetings.

Input: the pieces written by the eval stage `export` (see
`frontend/src-tauri/src/eval_pipeline.rs`): `<out>/<meeting id>/NNNNN.wav`
(16 kHz mono) and `pieces.jsonl` with the checked text.

```sh
cd ~/Documents/Protokol/test/finetune     # scripts copied here; Python 3.10+
python3 -m venv venv
venv/bin/pip install torch transformers peft
# 1. Which pieces teach and which test (the most recent meetings test).
#    --replay <out>/replay-<tag>.jsonl leaves out pieces whose checked text
#    differs from Whisper's in over half the words (rewritten lines)
venv/bin/python prepare.py <export out> --test <id>,<id>,<id> --out work
# 2. LoRA on the decoder (--encoder adapts the encoder too); hours.
#    First a quick try for speed and memory: --limit 40 --epochs 1
#    --prompt-file <out>/prompt.txt (eval stage «prompt») teaches with the
#    app's own initial prompt in front, as whisper.cpp decodes in the app
PYTORCH_ENABLE_MPS_FALLBACK=1 venv/bin/python train.py work --epochs 3
#    Merge the adapter with the lowest test loss in train.log
# 3. One model in Hugging Face format
venv/bin/python merge.py work/adapter-N work/merged
# 4. whisper.cpp format: the converter and Whisper's mel filters only
mkdir -p tools/whisper/whisper/assets models
curl -L -o tools/convert-h5-to-ggml.py https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/models/convert-h5-to-ggml.py
curl -L -o tools/whisper/whisper/assets/mel_filters.npz https://raw.githubusercontent.com/openai/whisper/main/whisper/assets/mel_filters.npz
venv/bin/python tools/convert-h5-to-ggml.py work/merged tools/whisper models
mv models/ggml-model.bin models/ggml-large-v3.bin
```

Then measure on the held-out meetings with the eval stage `replay`,
`PROTOKOL_EVAL_MODELS_DIR=<folder>/models`, `PROTOKOL_EVAL_MEETINGS=<test ids>`
and its own `PROTOKOL_EVAL_TAG`, against the same meetings with the original
model (the per-meeting lines of an earlier replay). Before a long run, take
the adapter of the quick try through steps 3–4 and replay one meeting: a
barely trained model must score like the original, or the conversion is
wrong.
