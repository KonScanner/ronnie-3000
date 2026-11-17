#!/usr/bin/env bash
# Renders gear skins in Blender and writes the app's assets under crates/ui/assets/skins/<skin>/.
#
# Usage: tools/render/build_skins.sh [skin...]      (all skins when none are given)
# Env:   BLENDER   path to the Blender binary (default: the macOS app bundle)
#        SAMPLES   Cycles samples for the body; knob frames use half (default: 256)
#        WORK      scratch directory for the raw renders (default: a new temp directory)
# Needs: Blender 5.x, cwebp, ImageMagick (magick).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
ASSETS="$ROOT/crates/ui/assets/skins"
BLENDER="${BLENDER:-/Applications/Blender.app/Contents/MacOS/Blender}"
SAMPLES="${SAMPLES:-256}"
WORK="${WORK:-$(mktemp -d)}"

PEDALS=(od-green od-gold dist-black fuzz boost comp default-dark default-light)
AMPS=(highgain modern-us british chime clean bass default-clean default-crunch default-hi-gain)
CABS=(cab-412-modern cab-412-vintage cab-212-chime cab-112-clean cab-810-bass cab-default)

script_for() {
  local skin="$1" s
  for s in "${PEDALS[@]}"; do [[ "$s" == "$skin" ]] && { echo pedals.py; return; }; done
  for s in "${AMPS[@]}" "${CABS[@]}"; do [[ "$s" == "$skin" ]] && { echo gear.py; return; }; done
  echo "unknown skin: $skin" >&2
  return 1
}

if [[ $# -eq 0 ]]; then
  set -- "${PEDALS[@]}" "${AMPS[@]}" "${CABS[@]}"
fi

for skin in "$@"; do
  script="$(script_for "$skin")"
  raw="$WORK/$skin"
  out="$ASSETS/$skin"
  rm -rf "$raw" "$out"
  mkdir -p "$raw" "$out"
  echo "== $skin ($script)"
  "$BLENDER" -b -P "$HERE/$script" -- "$skin" "$raw" "$SAMPLES" > "$raw.log" 2>&1 || {
    tail -20 "$raw.log" >&2
    exit 1
  }
  cwebp -quiet -q 90 -alpha_q 100 -exact "$raw/body.png" -o "$out/body.webp"
  for control in gain output mix; do
    frames=("$raw"/knob-"$control"-*.png)
    [[ -e "${frames[0]}" ]] || continue
    rows=()
    for ((i = 0; i < ${#frames[@]}; i += 8)); do
      magick "${frames[@]:i:8}" -background none +append "$raw/row-$control-$i.png"
      rows+=("$raw/row-$control-$i.png")
    done
    magick "${rows[@]}" -background none -append "$raw/knob-$control.png"
    cwebp -quiet -q 92 -alpha_q 100 -exact "$raw/knob-$control.png" -o "$out/knob-$control.webp"
  done
  cp "$raw/skin.json" "$out/skin.json"
done

du -sh "$ASSETS"
