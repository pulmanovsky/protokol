# Releasing

0. Put the pinned whisper.cpp sources in place (once after cloning, and again when the pin in the script changes): `scripts/fetch-whisper-cpp.sh` from the repository root.

   Files kept out of git must be in place too, or the build quietly leaves them out (the bundle's resource list is a pattern): the word-alignment model `frontend/src-tauri/aligner-model/w2v-uk-int8.onnx` and `vocab.json` (see the README there), and the sidecars in `frontend/src-tauri/binaries`. A fresh clone or worktree has none of them.

1. Build with the update signing key in the environment (the private key is kept off the repository):

   ```bash
   cd frontend
   RUSTFLAGS="--remap-path-prefix=$HOME=~ -C link-arg=-Wl,-S" \
   CFLAGS="-ffile-prefix-map=$HOME=~" CXXFLAGS="-ffile-prefix-map=$HOME=~" \
   DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
   TAURI_SIGNING_PRIVATE_KEY=<path to the key> TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
     pnpm exec tauri build --target aarch64-apple-darwin --bundles app,dmg -- --features metal
   ```

   The prefix maps keep the build machine's home folder (and so the login name) out of the published binaries; without them every crate's source path is written in as `/Users/<name>/…`. `-Wl,-S` leaves the debug map out at link time, so the paths stay out even when the Rust toolchain's own strip fails (it did on 0.4.1: it could not find `libLLVM.dylib`). `DEVELOPER_DIR` uses Xcode's tools for this build when the Command Line Tools are the active developer folder.

2. Check that the bundle holds the same files as the previous release (`find <app> -type f | sort`, compare; the `.dmg` is about 280 MB, most of it the alignment model). Check that no test hooks are in the build: `strings <app>/Contents/MacOS/protokol | grep PROTOKOL_TEST` must print nothing. Check that no local paths are in it: `strings <app>/Contents/MacOS/* | grep -c "/Users/"` must print 0. Check the FFmpeg inside: `<app>/Contents/MacOS/ffmpeg -version` starts with `ffmpeg version <version>-protokol-lgpl` and its configuration has no `--enable-gpl`.

3. Make the update package:

   ```bash
   scripts/make-update.sh <out-dir> https://github.com/pulmanovsky/protokol/releases/download/v<version> "<notes>"
   ```

4. Create a GitHub release `v<version>` and upload the `.dmg`, the `.app.tar.gz`, `latest.json` and the FFmpeg source archive the app's `ffmpeg` was built from (`ffmpeg-<version>.tar.xz` from https://ffmpeg.org/releases/ , the version and SHA-256 in `frontend/scripts/build-ffmpeg-macos.sh`; the LGPL asks for the source next to the binary). Apps read `releases/latest/download/latest.json`.

The public key is in `tauri.conf.json`. If the private key is lost, installed copies can no longer accept updates; if it leaks, publish a new version with a new key.
