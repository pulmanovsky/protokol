#!/bin/bash
# Builds the FFmpeg that ships inside Protokol on macOS: from the official
# source, LGPL (no --enable-gpl), no external libraries, only what the app
# uses: recording to .mp4/.m4a, joining recording parts, reading imported
# audio and video files.
#
#   build-ffmpeg-macos.sh <output binary> [aarch64-apple-darwin | x86_64-apple-darwin]
#
# build.rs runs it when binaries/ffmpeg-<target> is missing or is not this
# build. FFMPEG_TARBALL=<path> uses an already downloaded source archive.
# Each release carries the same source archive next to the app (LGPL).
set -euo pipefail

VERSION=8.1.3
SHA256=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3
URL="https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"

OUT=${1:?output binary path}
TARGET=${2:-aarch64-apple-darwin}
case "$TARGET" in
  aarch64-apple-darwin) ARCH=arm64 ;;
  x86_64-apple-darwin) ARCH=x86_64 ;;
  *) echo "Unsupported target: $TARGET" >&2; exit 1 ;;
esac
MIN_MACOS=${MACOSX_DEPLOYMENT_TARGET:-13.0}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/protokol-ffmpeg.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

TARBALL=${FFMPEG_TARBALL:-}
if [ -z "$TARBALL" ]; then
  TARBALL="$WORK/ffmpeg-$VERSION.tar.xz"
  echo "Downloading $URL"
  curl -fsSL --retry 3 -o "$TARBALL" "$URL"
fi
echo "$SHA256  $TARBALL" | shasum -a 256 -c -
tar -xf "$TARBALL" -C "$WORK"
cd "$WORK/ffmpeg-$VERSION"

CROSS=()
if [ "$(uname -m)" != "$ARCH" ]; then
  CROSS=(--enable-cross-compile)
fi
if [ "$ARCH" = x86_64 ]; then
  CROSS+=(--disable-x86asm) # no nasm needed
fi

./configure \
  --prefix="$WORK/out" \
  --extra-version=protokol-lgpl \
  --arch="$ARCH" --target-os=darwin --cc="clang -arch $ARCH" ${CROSS[@]+"${CROSS[@]}"} \
  --extra-cflags="-mmacosx-version-min=$MIN_MACOS" \
  --extra-ldflags="-mmacosx-version-min=$MIN_MACOS" \
  --enable-static --disable-shared \
  --disable-autodetect --enable-zlib \
  --disable-doc --disable-debug --disable-ffplay --disable-ffprobe --disable-network \
  --disable-everything \
  --enable-protocol=file,pipe \
  --enable-demuxer=mov,mp3,wav,aiff,caf,ogg,matroska,flac,aac,asf,amr,w64,concat,pcm_s16le,pcm_f32le \
  --enable-decoder=aac,aac_latm,mp3,mp3float,mp2,mp2float,opus,vorbis,flac,alac,wmav1,wmav2,wmapro,wmavoice,amrnb,amrwb,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_f32le,pcm_f32be,pcm_f64le,pcm_u8,pcm_alaw,pcm_mulaw,adpcm_ima_qt,adpcm_ms,gsm_ms \
  --enable-parser=aac,aac_latm,mpegaudio,opus,vorbis,flac \
  --enable-bsf=aac_adtstoasc \
  --enable-encoder=aac,pcm_s16le \
  --enable-muxer=mp4,ipod,mov,wav \
  --enable-filter=aresample,aformat,anull,atrim,asetpts,anoisesrc \
  --enable-indev=lavfi

if grep -q 'CONFIG_GPL 1' config.h; then
  echo "The build came out GPL; stopping" >&2
  exit 1
fi

make -j"$(sysctl -n hw.ncpu)" ffmpeg
BIN="$PWD/ffmpeg"

# What the app runs (see audio/encode.rs, audio/incremental_saver.rs,
# audio/channel_track.rs, audio/decoder.rs)
if [ "$(uname -m)" = "$ARCH" ]; then
  T="$WORK/check"
  mkdir -p "$T"
  "$BIN" -v error -y -f lavfi -i "anoisesrc=d=2:c=pink:r=48000:a=0.2" -ac 1 -c:a aac -b:a 192k -profile:a aac_low -movflags +faststart -f mp4 "$T/a.mp4"
  "$BIN" -v error -y -i "$T/a.mp4" -vn -acodec pcm_s16le "$T/a.wav"
  printf "file '%s'\nfile '%s'\n" "$T/a.mp4" "$T/a.mp4" > "$T/list.txt"
  "$BIN" -v error -y -f concat -safe 0 -i "$T/list.txt" -vn -af asetpts=N/SR/TB -c:a aac -b:a 192k -profile:a aac_low -movflags +faststart "$T/joined.mp4"
  "$BIN" -v error -y -f concat -safe 0 -i "$T/list.txt" -c copy "$T/copied.mp4"
  head -c 64000 /dev/zero > "$T/a.pcm"
  "$BIN" -v error -y -f s16le -ar 16000 -ac 1 -i "$T/a.pcm" -c:a aac -b:a 48k "$T/system.m4a"
  echo "Checks passed"
fi

mkdir -p "$(dirname "$OUT")"
cp "$BIN" "$OUT"
chmod +x "$OUT"
echo "Linked libraries:"
otool -L "$OUT" | tail -n +2
echo "Built $OUT ($(du -h "$OUT" | cut -f1)), FFmpeg $VERSION, LGPL"
