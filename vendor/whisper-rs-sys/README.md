# whisper-rs-sys 0.15.0 with whisper.cpp 1.9.5

A copy of [whisper-rs-sys](https://codeberg.org/tazz4843/whisper-rs) 0.15.0
(Unlicense), used through `[patch.crates-io]` in the root `Cargo.toml` so that
whisper-rs 0.16 builds against whisper.cpp 1.9.5 (the published crate carries
1.8.3).

Changes from the published crate:

- `build.rs` reads the whisper.cpp version from the `WHISPER_VERSION_*`
  lines used since 1.9, asks for the sources when they are missing, and
  replaces a build copy left from another pinned version.
- The `whisper.cpp/` folder is not kept in the repository. Run
  `scripts/fetch-whisper-cpp.sh` from the repository root once after cloning
  and whenever the pin in that script changes; it takes the tagged release from
  GitHub and checks its commit.
