#!/usr/bin/env bash
# Builds learning-engine-macos-arm64.tar.gz: a standalone Python for macOS
# arm64 with mlx, numpy and tiktoken, the trainer and its manifest. Runs on
# any machine with Python 3 and curl: the wheels are fetched for macOS arm64.
#
#   PBS=<python-build-standalone install_only archive for aarch64-apple-darwin> \
#     scripts/learning-engine/package.sh [out-dir]
#
# PBS is a URL or a downloaded file: a release asset of
# github.com/astral-sh/python-build-standalone, named like
# cpython-3.13.<n>+<date>-aarch64-apple-darwin-install_only_stripped.tar.gz;
# PY must match its Python version (default 3.13).
#
# Then sign it with the key of the app's updates, as the release does:
#   npx tauri signer sign learning-engine-macos-arm64.tar.gz   → .tar.gz.sig
# and put both on the public release `learning-engine-1` (see learning::engine).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:-$PWD}"
PY="${PY:-3.13}"
: "${PBS:?set PBS to the python-build-standalone archive for aarch64-apple-darwin (URL or file)}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ENGINE="$WORK/engine"
mkdir -p "$ENGINE"

echo "Python $PY for macOS arm64"
if [ -f "$PBS" ]; then
  tar -xzf "$PBS" -C "$ENGINE"
else
  curl -fsSL "$PBS" | tar -xz -C "$ENGINE"
fi
test -x "$ENGINE/python/bin/python3" || { echo "the archive has no python/bin/python3" >&2; exit 1; }
SITE="$ENGINE/python/lib/python$PY/site-packages"
test -d "$SITE" || { echo "no $SITE: PY does not match the archive" >&2; exit 1; }

echo "mlx, numpy, tiktoken for macOS arm64"
# pip reads `platform_system == "Darwin"` from the machine it runs on, so
# mlx's Metal part is named here, not left to mlx's own requirements
MLX_VERSION="$(sed -n 's/^mlx==//p' "$HERE/requirements.txt")"
python3 -m pip install --quiet --no-compile --target "$SITE" \
  --platform macosx_14_0_arm64 --python-version "$PY" --implementation cp --only-binary=:all: \
  -r "$HERE/requirements.txt" "mlx-metal==$MLX_VERSION"
test -n "$(ls "$SITE"/mlx/lib/*.metallib 2>/dev/null)" || { echo "mlx without its Metal library" >&2; exit 1; }

echo "the trainer"
cp "$HERE/protokol_train.py" "$HERE/ggml_whisper.py" "$ENGINE/"
mkdir -p "$ENGINE/whisper_mlx/assets"
cp "$HERE"/whisper_mlx/*.py "$HERE/whisper_mlx/LICENSE" "$ENGINE/whisper_mlx/"
cp "$HERE"/whisper_mlx/assets/* "$ENGINE/whisper_mlx/assets/"
cat > "$ENGINE/manifest.json" <<'JSON'
{"program": "python/bin/python3", "args": ["-I", "{engine}/protokol_train.py"]}
JSON

# The archive is published: nothing in it may name the machine or the person
# who built it, so the files' owner is neutral (not the login name)
if tar --version 2>/dev/null | grep -q bsdtar; then
  NEUTRAL=(--uid 0 --gid 0 --uname root --gname wheel)
else
  NEUTRAL=(--owner=0 --group=0 --numeric-owner)
fi
if grep -rIl --exclude-dir=python "/Users/\|/home/" "$ENGINE" >/dev/null 2>&1; then
  echo "a local path is written into the trainer files" >&2; exit 1
fi

ARCHIVE="$OUT_DIR/learning-engine-macos-arm64.tar.gz"
# No extended attributes or resource forks from a Mac's tar
COPYFILE_DISABLE=1 tar "${NEUTRAL[@]}" -czf "$ARCHIVE" -C "$ENGINE" .
# What is published: the file list with owners, and no local paths anywhere
tar -tvzf "$ARCHIVE" | awk '{print $3, $4}' | sort | uniq -c
if tar -xzOf "$ARCHIVE" 2>/dev/null | LC_ALL=C grep -a -c "/Users/" | grep -qv '^0$'; then
  echo "warning: the archive contains the text /Users/ (check with: tar -xzOf … | grep -a /Users/)" >&2
fi
ls -lh "$ARCHIVE"
shasum -a 256 "$ARCHIVE" 2>/dev/null || sha256sum "$ARCHIVE"
