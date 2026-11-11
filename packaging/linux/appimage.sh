#!/usr/bin/env bash
# Builds a Linux AppImage of the release binary: one file that a user downloads, marks
# executable, and opens. Run `make appimage`, which builds the binary first.
#
# The AppImage tools are pinned to a release and checked with SHA-256 before use.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VERSION=${VERSION:?set VERSION to the application version}
DIST_DIR=${DIST_DIR:-$ROOT/dist}
if [[ $DIST_DIR != /* ]]; then
  DIST_DIR="$ROOT/$DIST_DIR"
fi
ARCH=$(uname -m)

TOOL_RELEASE=1.9.1
RUNTIME_RELEASE=20251108
case "$ARCH" in
x86_64)
  TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
  RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
  ;;
aarch64)
  TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
  RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
  ;;
*)
  printf 'error: no AppImage tools for %s\n' "$ARCH" >&2
  exit 2
  ;;
esac

WORK="${CARGO_TARGET_DIR:-$ROOT/target}/appimage"
TOOLS="$WORK/tools-$TOOL_RELEASE-$RUNTIME_RELEASE"
APPDIR="$WORK/Ronnie3000.AppDir"
OUT="$DIST_DIR/ronnie-3000-$VERSION-linux-$ARCH.AppImage"

if [[ $WORK != */appimage ]]; then
  printf 'error: unsafe AppImage workspace %s\n' "$WORK" >&2
  exit 2
fi

# Downloads a file once and checks its SHA-256. A file with another digest is never used.
fetch_checked() {
  local url=$1 dest=$2 sha=$3
  if [[ -f $dest ]] && printf '%s  %s\n' "$sha" "$dest" | sha256sum --check --status; then
    return 0
  fi
  curl -fsSL --proto '=https' --tlsv1.2 -o "$dest.part" "$url"
  if ! printf '%s  %s\n' "$sha" "$dest.part" | sha256sum --check --status; then
    rm -f "$dest.part"
    printf 'error: the SHA-256 of %s does not match\n' "$url" >&2
    exit 1
  fi
  mv "$dest.part" "$dest"
}

mkdir -p "$TOOLS" "$DIST_DIR"
fetch_checked \
  "https://github.com/AppImage/appimagetool/releases/download/$TOOL_RELEASE/appimagetool-$ARCH.AppImage" \
  "$TOOLS/appimagetool" "$TOOL_SHA256"
fetch_checked \
  "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_RELEASE/runtime-$ARCH" \
  "$TOOLS/runtime" "$RUNTIME_SHA256"
chmod +x "$TOOLS/appimagetool"

rm -rf "$APPDIR"
mkdir -p "$APPDIR"
make -s -C "$ROOT" install DESTDIR="$APPDIR" PREFIX=/usr >/dev/null
cp "$ROOT/packaging/ronnie-3000.desktop" "$APPDIR/ronnie-3000.desktop"
cp "$ROOT/assets/ronnie-3000-256.png" "$APPDIR/ronnie-3000.png"
ln -s ronnie-3000.png "$APPDIR/.DirIcon"
cat >"$APPDIR/AppRun" <<'EOF'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/ronie" "$@"
EOF
chmod +x "$APPDIR/AppRun"
"$APPDIR/usr/bin/ronie" --help >/dev/null

rm -f "$OUT"
# The extract-and-run mode needs no FUSE, so the tool also runs in a container or on CI.
ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOLS/appimagetool" --no-appstream \
  --runtime-file "$TOOLS/runtime" "$APPDIR" "$OUT" >/dev/null
chmod +x "$OUT"
(cd "$DIST_DIR" && sha256sum "$(basename "$OUT")" >"$(basename "$OUT").sha256")
printf 'wrote %s\n' "$OUT"
