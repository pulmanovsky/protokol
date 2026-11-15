# Learning engine

The program the app runs at night to fine-tune Whisper on the organisation's
own checked meetings (`learning` in the app's code). It runs on the Mac's
GPU through MLX and needs Apple Silicon with 16 GB or more.

- `protokol_train.py <job.json>`: one run, as the app asks for it.
- `ggml_whisper.py`: reads the app's own `ggml-large-v3.bin` into MLX and
  writes the new model as a copy of it with the trained weights replaced.
- `whisper_mlx/`: the model, log-Mel spectrogram and tokenizer from
  mlx-whisper 0.4.3 (MIT, Apple), so the engine needs only `mlx`, `numpy`
  and `tiktoken` (mlx-whisper itself pulls in torch, scipy and numba).
- `test_engine.py`: an end-to-end check on a tiny random model; runs on any
  machine with MLX, CPU included, in under a minute.

Nothing leaves the Mac: the engine reads the pieces the app wrote into the
run's work folder and writes one model file there.

## What the app asks

`job.json` (version 1):

| field | |
|---|---|
| `base_model` | the app's original `ggml-large-v3.bin`; every run starts from it |
| `train`, `valid` | JSON lines `{"file": "<16 kHz mono WAV>", "text": "<checked text>"}` |
| `prompt` | the prompt recognition uses (dictionary words), put in front as whisper.cpp does |
| `language` | `uk` |
| `lora_rank`, `learning_rate`, `epochs` | 32, 2e-4, 2: the settings of v5, the pilot model that beat the original on 15.11 |
| `precision` | `full` (default): the decoder in full precision, up to 11 GB; `half`: all in half precision as the file stores it, up to 7 GB, for Macs under 24 GB |
| `work_dir`, `ggml_out` | the run's folder and where the new model goes |

The rest is fixed as in `scripts/finetune/train.py`: adapters on the
decoder's attention (self and cross) and feed-forward layers, alpha 2×rank,
dropout 0.05, AdamW with weight decay 0.01, gradient clipped at 1.0,
8 pieces per step, 5% warm-up then linear decay. After each epoch the loss
on the `valid` pieces is measured; the epoch with the lowest one is kept.
The frozen encoder runs in half precision, as whisper.cpp runs it (1.3 GB
less); the decoder, where the gradients flow, in full precision.

## What it answers

One JSON object per line on stdout; anything else goes to stderr.

```
{"event":"started","pieces":601,"epochs":2}
{"event":"epoch","epoch":0,"valid_loss":0.77}      (before training)
{"event":"step","epoch":1,"step":1,"of":601,"loss":0.71,"seconds_per_piece":2.07,"memory_gb":12.1}
{"event":"epoch","epoch":1,"valid_loss":0.40}
{"event":"converting"}
{"event":"done","minutes":48.5,"peak_memory_gb":12.3}
{"event":"failed","reason":"..."}
```

`seconds_per_piece` is the median of the last 8 pieces, so a pause for a
recording (the app stops the process with SIGSTOP) does not look like a
slow Mac. The engine exits by itself when its stdin closes: the app quit or
crashed.

## How it is installed

In the app's data folder, `learning/engine/`:

```
manifest.json     {"program": "python/bin/python3", "args": ["-I", "{engine}/protokol_train.py"]}
python/           a standalone CPython 3.13 for macOS arm64 with requirements.txt installed
protokol_train.py, ggml_whisper.py, whisper_mlx/
```

`package.sh` builds it as `learning-engine-macos-arm64.tar.gz` (about 70 MB
to download, 250 MB unpacked, most of it MLX's Metal library); the release
signs it with the key of the app's updates. The app installs it only when
the user asks: downloaded from the public release, or, on a closed network,
from the archive and its `.sig` copied by hand. Nothing unsigned is
unpacked (`learning::engine`).

## Checked on a real Mac

15.11, a MacBook with 24 GB, the pilot's split (601 pieces, its 176 test
pieces as `valid`), the app's prompt, 2 epochs; mistakes on the three
meetings of 14.11 (the original model: 17.5%, v5: 15.8%):

| engine | precision | loss before / after | s a piece | peak | minutes | mistakes |
|---|---|---|---|---|---|---|
| 6a6e611 | full, encoder too | 0.7688 / 0.3820 | 1.57 | 16.9 GB | 43.8 | 15.2% |
| 3cc9c76 | `full` | 0.7686 / 0.3819 | 1.46 | 10.8 GB | 35.7 | 15.3% |
| 3cc9c76 | `half` | 0.7686 / 0.4056 | 1.06 | 7.0 GB | 26.6 | 15.5% |

`half` is used below 24 GB only: on a 16 GB Mac macOS and the app need
their share too, and `full` is a little more exact.
