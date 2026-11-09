#!/bin/zsh
# Quality and speed benchmark for one recording, with the app's own pipeline.
# Outputs (never committed) go to $PROTOKOL_TEST_ROOT/bench/<name>/ (a folder with copies).
#
#   scripts/bench/run.sh <audio file> <name>
#
# Steps: ASR (forced uk), speakers (diarization + Whisper per turn), the
# «Щоденна нарада» and «Загальна зустріч» documents from the speaker
# transcript, 3 one-minute excerpts for a human check. Each step records
# wall time and peak memory. Then scripts/bench/report.py writes report.md.
set -euo pipefail
AUDIO="$1"; NAME="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ROOT="${PROTOKOL_TEST_ROOT:-$HOME/protokol-copies}"
OUT="$ROOT/bench/$NAME"
MODELS="${PROTOKOL_SPEAKER_MODELS:-$HOME/Library/Application Support/ua.protokol.app/models/speakers}"
HELPER="${PROTOKOL_LLAMA_HELPER:-$ROOT/llama-helper-current}"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
mkdir -p "$OUT"
cd "$ROOT/frontend/src-tauri"

step() { # step <name> <env...> — runs the pipeline test with timing and peak RSS
  local name="$1"; shift
  echo "== $name"
  env "$@" PROTOKOL_EVAL_AUDIO="$AUDIO" PROTOKOL_LLAMA_HELPER="$HELPER" \
    /usr/bin/time -l caffeinate -dimsu cargo test --release --features coreml --lib eval_real_recording -- --ignored --nocapture \
    > "$OUT/$name.log" 2> "$OUT/$name.time" || { echo "step $name failed, see $OUT/$name.log"; exit 1; }
}

cargo build --release --features coreml --lib >/dev/null 2>&1 || true
step asr PROTOKOL_EVAL_STAGE=asr PROTOKOL_EVAL_OUT="$OUT/asr"
# Speakers as the app attaches them: word times of the import + diarization
# (PROTOKOL_BENCH_PER_TURN=1: the older per-turn Whisper pass instead)
if [[ -n "${PROTOKOL_BENCH_PER_TURN:-}" ]]; then SPK=speakers; else SPK=words; fi
step speakers PROTOKOL_EVAL_STAGE=$SPK PROTOKOL_EVAL_OUT="$OUT/speakers" \
  PROTOKOL_SEG_MODEL="$MODELS/segmentation-3.0.onnx" PROTOKOL_EMB_MODEL="$MODELS/voice-embedding.onnx"
# Documents from the speaker transcript
python3 "$ROOT/scripts/bench/report.py" segments "$OUT/speakers/transcript.txt" "$OUT/doc_daily/segments.json"
cp "$OUT/doc_daily/segments.json" "$OUT/doc_general.segments.json" 2>/dev/null || true
mkdir -p "$OUT/doc_general" && cp "$OUT/doc_daily/segments.json" "$OUT/doc_general/segments.json"
step doc_daily PROTOKOL_EVAL_STAGE=protocol PROTOKOL_EVAL_TEMPLATE=daily_reports_uk PROTOKOL_EVAL_OUT="$OUT/doc_daily"
step doc_general PROTOKOL_EVAL_STAGE=protocol PROTOKOL_EVAL_TEMPLATE=meeting_materials_uk PROTOKOL_EVAL_OUT="$OUT/doc_general"
# Three one-minute excerpts (early, middle, late) for a human to correct once
python3 "$ROOT/scripts/bench/report.py" excerpts "$AUDIO" "$OUT"
python3 "$ROOT/scripts/bench/report.py" report "$OUT"
echo "Report: $OUT/report.md"
