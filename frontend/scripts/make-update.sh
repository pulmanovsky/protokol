#!/bin/bash
# Makes an update for «Протокол» from a build: the signed archive and the
# latest.json manifest, ready to be put where the apps look (BASE_URL).
#   scripts/make-update.sh <out-dir> <base-url> "<notes>"
# The build must be made with the signing key, e.g.
#   TAURI_SIGNING_PRIVATE_KEY_PATH=<path from PROTOKOL_UPDATER_KEY, default under your home folder> TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" pnpm exec tauri build …
set -euo pipefail
OUT=${1:?out dir}; BASE=${2:?base url}; NOTES=${3:-}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BUNDLE="$ROOT/../target/aarch64-apple-darwin/release/bundle/macos"
# Exactly one updater archive: another app left in the bundle folder (a test
# copy) must not lend its name or version
ARCHIVES=("$BUNDLE"/*.app.tar.gz)
if [ "${#ARCHIVES[@]}" -ne 1 ] || [ ! -f "${ARCHIVES[0]}" ]; then
  echo "Expected one .app.tar.gz in $BUNDLE, found: ${ARCHIVES[*]}"; exit 1
fi
ARCHIVE=${ARCHIVES[0]}
# The version of what was built (not of the sources, which may have moved on),
# read from the app the archive was made of
APP=${ARCHIVE%.tar.gz}
[ -d "$APP" ] || { echo "No $APP next to its archive"; exit 1; }
VERSION=$(defaults read "$APP/Contents/Info.plist" CFBundleShortVersionString)
SIG="$ARCHIVE.sig"
[ -f "$SIG" ] || { echo "No signature next to $ARCHIVE: build with the signing key"; exit 1; }
NAME="Protokol_${VERSION}_aarch64.app.tar.gz"
mkdir -p "$OUT"
cp "$ARCHIVE" "$OUT/$NAME"
python3 - "$OUT" "$BASE" "$VERSION" "$NAME" "$NOTES" "$SIG" <<'PY'
import json, sys, datetime
out, base, version, name, notes, sig = sys.argv[1:]
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "platforms": {"darwin-aarch64": {"signature": open(sig).read().strip(), "url": base.rstrip("/") + "/" + name}},
}
json.dump(manifest, open(f"{out}/latest.json", "w"), ensure_ascii=False, indent=2)
print(f"{out}/latest.json → {version}")
PY
