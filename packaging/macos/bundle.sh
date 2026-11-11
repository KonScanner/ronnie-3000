#!/usr/bin/env bash
# Builds "Ronnie 3000.app" from a macOS binary and zips it for a release. Runs on macOS.
#
#   VERSION=0.1.0 BINARY=target/universal/ronie bash packaging/macos/bundle.sh
#
# The bundle has an ad-hoc signature only. macOS asks the user to confirm the first start.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VERSION=${VERSION:?set VERSION to the application version}
BINARY=${BINARY:?set BINARY to the macOS ronie binary}
DIST_DIR=${DIST_DIR:-$ROOT/dist}
if [[ $DIST_DIR != /* ]]; then
  DIST_DIR="$ROOT/$DIST_DIR"
fi
WORK="${CARGO_TARGET_DIR:-$ROOT/target}/macos-bundle"
APP="$WORK/Ronnie 3000.app"
ZIP="$DIST_DIR/ronnie-3000-$VERSION-macos-universal.zip"

if [[ $WORK != */macos-bundle ]]; then
  printf 'error: unsafe bundle workspace %s\n' "$WORK" >&2
  exit 2
fi
rm -rf "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DIST_DIR"

install -m 755 "$BINARY" "$APP/Contents/MacOS/ronie"
sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"

# iconutil takes a folder of PNGs with fixed names. Each @2x image has twice the pixels.
ICONSET="$WORK/Ronnie3000.iconset"
mkdir -p "$ICONSET"
for pair in 16:16 16@2x:32 32:32 32@2x:64 128:128 128@2x:256 256:256 256@2x:512 512:512; do
  name=${pair%%:*}
  pixels=${pair##*:}
  points=${name%@2x}
  suffix=${name#"$points"}
  cp "$ROOT/assets/ronnie-3000-$pixels.png" "$ICONSET/icon_${points}x${points}${suffix}.png"
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Ronnie3000.icns"

codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
"$APP/Contents/MacOS/ronie" --help >/dev/null

rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
(cd "$DIST_DIR" && shasum -a 256 "$(basename "$ZIP")" >"$(basename "$ZIP").sha256")
printf 'wrote %s\n' "$ZIP"
