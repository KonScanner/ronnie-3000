#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VERSION=${VERSION:?set VERSION to the application version}
DIST_DIR=${DIST_DIR:-$ROOT/dist}
if [[ $DIST_DIR != /* ]]; then
  DIST_DIR="$ROOT/$DIST_DIR"
fi
ARCH=$(uname -m)
BASENAME="ronnie-3000-$VERSION-linux-$ARCH"
WORK="${CARGO_TARGET_DIR:-$ROOT/target}/linux-package"
STAGE="$WORK/root"

if [[ $WORK != */linux-package ]]; then
  printf 'error: unsafe package workspace %s\n' "$WORK" >&2
  exit 2
fi
rm -rf "$WORK"
mkdir -p "$STAGE" "$DIST_DIR"
make -s -C "$ROOT" install DESTDIR="$STAGE" PREFIX=/usr

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$STAGE/usr/share/applications/ronnie-3000.desktop"
fi
"$STAGE/usr/bin/ronie" --help >/dev/null

mkdir -p "$WORK/tar/$BASENAME"
cp -R "$STAGE/usr" "$WORK/tar/$BASENAME/"
tar -C "$WORK/tar" -czf "$DIST_DIR/$BASENAME.tar.gz" "$BASENAME"
tar -tzf "$DIST_DIR/$BASENAME.tar.gz" >/dev/null
sha256sum "$DIST_DIR/$BASENAME.tar.gz" >"$DIST_DIR/$BASENAME.tar.gz.sha256"
printf 'wrote %s\n' "$DIST_DIR/$BASENAME.tar.gz"
