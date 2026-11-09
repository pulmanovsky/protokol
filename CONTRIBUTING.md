# Contributing

Requires Xcode, Rust, Node.js and pnpm.

```bash
cd frontend
pnpm install
pnpm exec tauri build --target aarch64-apple-darwin --bundles app,dmg -- --features metal
```

The first build also builds FFmpeg from its official source (LGPL, a few
minutes, once; see `frontend/scripts/build-ffmpeg-macos.sh`).

Tests: `cargo test --lib --features metal` in `frontend/src-tauri`.

Never attach or commit real recordings, transcripts or names. Use invented examples.
