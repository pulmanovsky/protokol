# Releasing

1. Build with the update signing key in the environment (the private key is kept off the repository):

   ```bash
   cd frontend
   TAURI_SIGNING_PRIVATE_KEY=<path to the key> TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
     pnpm exec tauri build --target aarch64-apple-darwin --bundles app,dmg -- --features metal
   ```

2. Check that no test hooks are in the build: `strings <app>/Contents/MacOS/protokol | grep PROTOKOL_TEST` must print nothing. Check the FFmpeg inside: `<app>/Contents/MacOS/ffmpeg -version` starts with `ffmpeg version <version>-protokol-lgpl` and its configuration has no `--enable-gpl`.

3. Make the update package:

   ```bash
   scripts/make-update.sh <out-dir> https://github.com/pulmanovsky/protokol/releases/download/v<version> "<notes>"
   ```

4. Create a GitHub release `v<version>` and upload the `.dmg`, the `.app.tar.gz`, `latest.json` and the FFmpeg source archive the app's `ffmpeg` was built from (`ffmpeg-<version>.tar.xz` from https://ffmpeg.org/releases/ , the version and SHA-256 in `frontend/scripts/build-ffmpeg-macos.sh`; the LGPL asks for the source next to the binary). Apps read `releases/latest/download/latest.json`.

The public key is in `tauri.conf.json`. If the private key is lost, installed copies can no longer accept updates; if it leaks, publish a new version with a new key.
