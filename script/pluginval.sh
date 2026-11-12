#!/usr/bin/env bash
# Checks a VST3 bundle with pluginval (https://github.com/Tracktion/pluginval). pluginval is a
# GPL-3.0 test tool that this script downloads. It is not part of Ronnie. Runs on Linux, macOS,
# and Windows (Git Bash).
#
#   bash script/pluginval.sh "target/vst3-bundle/ronnie-3000-vst3-ci-linux-x86_64/Ronnie 3000.vst3"
#
# STRICTNESS sets the pluginval strictness level, 1 to 10. The default is 10, the strictest.
set -euo pipefail

VERSION=v1.0.4
BUNDLE=${1:?give the path of the .vst3 bundle}
STRICTNESS=${STRICTNESS:-10}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DIR="${CARGO_TARGET_DIR:-$ROOT/target}/pluginval-$VERSION"

case "$(uname -s)" in
  Linux)
    name=Linux
    sum=c01c49d8063965c4c2dea8324468336768f5c9139e0b1caebde14c2400b55352
    binary=pluginval
    ;;
  Darwin)
    name=macOS
    sum=3c4c533bda0c5059eea3ddaea752d757ee2025041f0f47e6bcb0e87f6082b29f
    binary=pluginval.app/Contents/MacOS/pluginval
    ;;
  MINGW* | MSYS* | CYGWIN*)
    name=Windows
    sum=c08e61ce3b96db41636f8ec7e76f4c7e2c13ebdac7fa1b5a1f52b4f32ec715ab
    binary=pluginval.exe
    ;;
  *)
    printf 'error: no pluginval for %s\n' "$(uname -s)" >&2
    exit 2
    ;;
esac

if [[ ! -d $BUNDLE ]]; then
  printf 'error: no VST3 bundle at %s\n' "$BUNDLE" >&2
  exit 2
fi

if [[ ! -f "$DIR/$binary" ]]; then
  mkdir -p "$DIR"
  zip="$DIR/pluginval_$name.zip"
  curl -fsSL --retry 3 -o "$zip" \
    "https://github.com/Tracktion/pluginval/releases/download/$VERSION/pluginval_$name.zip"
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$zip" | cut -d' ' -f1)
  else
    actual=$(shasum -a 256 "$zip" | cut -d' ' -f1)
  fi
  if [[ $actual != "$sum" ]]; then
    printf 'error: pluginval_%s.zip has the sha256 %s, not %s\n' "$name" "$actual" "$sum" >&2
    rm -f "$zip"
    exit 1
  fi
  if command -v unzip >/dev/null 2>&1; then
    unzip -q -o "$zip" -d "$DIR"
  else
    (cd "$DIR" && 7z x -y -bd "$zip" >/dev/null)
  fi
  chmod +x "$DIR/$binary"
fi

args=(--strictness-level "$STRICTNESS" --skip-gui-tests --timeout-ms 300000 --validate "$BUNDLE")
# pluginval is a JUCE app, and on Linux it needs an X display also with no window.
if [[ $name == Linux && -z ${DISPLAY:-} ]] && command -v xvfb-run >/dev/null 2>&1; then
  xvfb-run -a "$DIR/$binary" "${args[@]}"
else
  "$DIR/$binary" "${args[@]}"
fi
