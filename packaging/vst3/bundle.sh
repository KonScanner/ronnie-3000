#!/usr/bin/env bash
# Builds "Ronnie 3000.vst3" from the plugin library and zips it for a release. Runs on Linux,
# macOS, and Windows (Git Bash).
#
#   VERSION=0.2.0 PLATFORM=linux-x86_64 LIBRARY=target/release/libronie_plugin_nice.so \
#     bash packaging/vst3/bundle.sh
#
# PLATFORM is linux-x86_64, linux-aarch64, macos-universal, macos-arm64, macos-x86_64, or
# windows-x86_64. LIBRARY is the library that `cargo build -p ronie-plugin-nice` writes:
# libronie_plugin_nice.so, libronie_plugin_nice.dylib (a lipo output for macos-universal), or
# ronie_plugin_nice.dll. On macOS the bundle has an ad-hoc signature only.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VERSION=${VERSION:?set VERSION to the application version}
PLATFORM=${PLATFORM:?set PLATFORM, for example linux-x86_64}
LIBRARY=${LIBRARY:?set LIBRARY to the plugin library}
DIST_DIR=${DIST_DIR:-$ROOT/dist}
if [[ $DIST_DIR != /* ]]; then
  DIST_DIR="$ROOT/$DIST_DIR"
fi
NAME="Ronnie 3000"
BASENAME="ronnie-3000-vst3-$VERSION-$PLATFORM"
WORK="${CARGO_TARGET_DIR:-$ROOT/target}/vst3-bundle"
STAGE="$WORK/$BASENAME"
BUNDLE="$STAGE/$NAME.vst3"
ZIP="$DIST_DIR/$BASENAME.zip"

if [[ $WORK != */vst3-bundle ]]; then
  printf 'error: unsafe bundle workspace %s\n' "$WORK" >&2
  exit 2
fi
if [[ ! -f $LIBRARY ]]; then
  printf 'error: no plugin library at %s\n' "$LIBRARY" >&2
  exit 2
fi
rm -rf "$WORK"
mkdir -p "$STAGE" "$DIST_DIR"

# A host finds the plugin through the VST3 entry point. A library without it is the wrong file.
has_entry() {
  nm "$@" | awk '$NF == "GetPluginFactory" || $NF == "_GetPluginFactory" { found = 1 } END { exit !found }'
}

case $PLATFORM in
  linux-x86_64 | linux-aarch64)
    folder="$BUNDLE/Contents/${PLATFORM#linux-}-linux"
    mkdir -p "$folder"
    cp "$LIBRARY" "$folder/$NAME.so"
    chmod 755 "$folder/$NAME.so"
    has_entry -D --defined-only "$folder/$NAME.so" || {
      printf 'error: %s has no VST3 entry point\n' "$LIBRARY" >&2
      exit 1
    }
    install_folder='~/.vst3 (for your user) or /usr/lib/vst3 (for all users)'
    ;;
  macos-universal | macos-arm64 | macos-x86_64)
    mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
    cp "$LIBRARY" "$BUNDLE/Contents/MacOS/$NAME"
    if [[ $PLATFORM == macos-universal ]]; then
      lipo "$BUNDLE/Contents/MacOS/$NAME" -verify_arch arm64 x86_64
    fi
    has_entry -gU "$BUNDLE/Contents/MacOS/$NAME" || {
      printf 'error: %s has no VST3 entry point\n' "$LIBRARY" >&2
      exit 1
    }
    sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/vst3/Info.plist" >"$BUNDLE/Contents/Info.plist"
    plutil -lint "$BUNDLE/Contents/Info.plist" >/dev/null
    printf 'BNDL????' >"$BUNDLE/Contents/PkgInfo"
    codesign --force --sign - "$BUNDLE"
    codesign --verify --strict "$BUNDLE"
    install_folder='~/Library/Audio/Plug-Ins/VST3 (for your user) or /Library/Audio/Plug-Ins/VST3 (for all users)'
    ;;
  windows-x86_64)
    folder="$BUNDLE/Contents/x86_64-win"
    mkdir -p "$folder"
    cp "$LIBRARY" "$folder/$NAME.vst3"
    install_folder='C:\Program Files\Common Files\VST3'
    ;;
  *)
    printf 'error: unknown PLATFORM %s\n' "$PLATFORM" >&2
    exit 2
    ;;
esac

cp "$ROOT/LICENSE" "$STAGE/LICENSE.txt"
cat >"$STAGE/INSTALL.txt" <<EOF
Ronnie 3000 $VERSION, VST3 plugin

1. Close your DAW.
2. Copy the folder "$NAME.vst3" to $install_folder.
3. Open your DAW, and let it scan for new plugins.
4. Add "$NAME" to a track as an effect.

The DAW project keeps the settings of each plugin.
EOF
if [[ $PLATFORM == macos-* ]]; then
  cat >>"$STAGE/INSTALL.txt" <<EOF

The plugin has no Apple signature yet. If macOS blocks it, run this command once after the copy:

  xattr -dr com.apple.quarantine ~/Library/Audio/Plug-Ins/VST3/"$NAME.vst3"
EOF
fi

rm -f "$ZIP"
if command -v ditto >/dev/null 2>&1; then
  ditto -c -k --sequesterRsrc --keepParent "$STAGE" "$ZIP"
elif command -v zip >/dev/null 2>&1; then
  (cd "$WORK" && zip -qr "$ZIP" "$BASENAME")
elif command -v 7z >/dev/null 2>&1; then
  (cd "$WORK" && 7z a -tzip -bd "$ZIP" "$BASENAME" >/dev/null)
elif command -v powershell.exe >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
  powershell.exe -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$STAGE")' -DestinationPath '$(cygpath -w "$ZIP")'"
else
  printf 'error: no zip tool (ditto, zip, or 7z)\n' >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$DIST_DIR" && sha256sum "$BASENAME.zip" >"$BASENAME.zip.sha256")
else
  (cd "$DIST_DIR" && shasum -a 256 "$BASENAME.zip" >"$BASENAME.zip.sha256")
fi
printf 'wrote %s\n' "$ZIP"
