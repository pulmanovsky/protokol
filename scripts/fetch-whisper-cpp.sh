#!/bin/sh
# Puts the pinned whisper.cpp release into vendor/whisper-rs-sys/whisper.cpp
# (the files whisper-rs-sys builds from). The tag's commit is checked, so a
# moved tag is refused. Run from the repository root.
set -eu
TAG=v1.9.5
COMMIT=d1be6fde11ac6e0407606b4e42fe72d34add8037
DEST=vendor/whisper-rs-sys/whisper.cpp

if [ -f "$DEST/PROTOKOL_PIN" ] && [ "$(cat "$DEST/PROTOKOL_PIN")" = "$TAG $COMMIT" ]; then
  echo "whisper.cpp $TAG already in place"
  exit 0
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git -c advice.detachedHead=false clone -q --depth 1 --branch "$TAG" https://github.com/ggml-org/whisper.cpp "$tmp/src"
got=$(git -C "$tmp/src" rev-parse HEAD)
if [ "$got" != "$COMMIT" ]; then
  echo "whisper.cpp $TAG is at $got, expected $COMMIT: refused" >&2
  exit 1
fi
rm -rf "$DEST"
mkdir -p "$DEST/bindings/javascript"
cp -R "$tmp/src/CMakeLists.txt" "$tmp/src/LICENSE" "$tmp/src/cmake" "$tmp/src/include" "$tmp/src/src" "$tmp/src/ggml" "$DEST/"
cp "$tmp/src/bindings/javascript/package-tmpl.json" "$DEST/bindings/javascript/"
echo "$TAG $COMMIT" > "$DEST/PROTOKOL_PIN"
echo "whisper.cpp $TAG ($COMMIT) in $DEST"
