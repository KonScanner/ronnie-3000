#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

usage() {
  echo "usage: script/check-ui.sh --save-baseline | --suite ronnie-baseline | --suite reference-default" >&2
  exit 2
}

case "${1:-} ${2:-}" in
  "--save-baseline ") mode=save ;;
  "--suite ronnie-baseline") mode=baseline ;;
  "--suite reference-default") mode=reference ;;
  *) usage ;;
esac

baseline=.ui-baseline
work=target/ui-suite
testbed=target/debug/ronie-ui-testbed

# The size that each scenario needs at the 1024-point width of the suite.
window_of() {
  case "$1" in
    chrome-hints-off) echo 1024x578 ;;
    banner-* | settings-system-feedback-risk | settings-system-input-muted | settings-system-mic-denied | settings-system-rate96) echo 1024x658 ;;
    *) echo 1024x614 ;;
  esac
}

# A capture opens about one hundred windows, so they must not open on the desktop of the user.
# A separate home keeps the preferences of the user out of the images.
start_private_display() {
  private_home="$(mktemp -d)"
  local number=90
  while [[ -e "/tmp/.X11-unix/X$number" ]]; do
    number=$((number + 1))
  done
  private_display=":$number"
  Xwayland "$private_display" -noreset -geometry 2400x1600 >/dev/null 2>&1 &
  display_pid=$!
  trap 'kill "$display_pid" 2>/dev/null || true; rm -rf "$private_home"' EXIT
  for _ in {1..50}; do
    [[ -e "/tmp/.X11-unix/X$number" ]] && return 0
    sleep 0.1
  done
  echo "The private display did not start." >&2
  exit 1
}

in_private_display() {
  env -u WAYLAND_DISPLAY DISPLAY="$private_display" WINIT_X11_SCALE_FACTOR=2 \
    HOME="$private_home" XDG_CONFIG_HOME="$private_home/config" "$@"
}

capture_all() {
  rm -rf "$work/ronie"
  mkdir -p "$work/ronie"
  jq -r '.scenarios[].id' fixtures/ui-scenarios.json | while IFS= read -r id; do
    in_private_display "$testbed" \
      --native-scenario "$id" \
      --native-window "$(window_of "$id")" \
      --native-capture "$work/ronie/$id.png" \
      --native-report "$work/ronie/$id.json"
    jq -e '.passed == true and .capture_saved == true and .frame_count > 0 and .max_frame_ms >= 0 and .rss_kib > 0 and .accessibility.passed == true and .accessibility.unnamed_focusable == 0' "$work/ronie/$id.json" >/dev/null
  done
}

# Writes the report and lists each changed scenario, because a small change can pass the limits.
compare_with() {
  local status=0
  "$testbed" --compare-dir "$1" "$work/ronie" "$work/report.json" || status=$?
  jq --arg suite "$2" '.suite = $suite' "$work/report.json" >"$work/report.json.new"
  mv "$work/report.json.new" "$work/report.json"
  jq -r '.scenarios | map(select(.mismatched > 0)) | "Scenarios with a changed pixel: \(length)", (.[] | "  \(.id): \(.mismatch_ratio), worst tile \(.worst_tile_ratio), passed \(.passed)")' "$work/report.json"
  jq -r '.scenarios | map(select(.missing_reference == true)) | "Scenarios with no reference image: \(length)", (.[] | "  \(.id)")' "$work/report.json"
  echo "Report: $work/report.json"
  return "$status"
}

cargo build -p ronie-app --bin ronie-ui-testbed

case "$mode" in
  save)
    start_private_display
    capture_all
    rm -rf "$baseline"
    mkdir -p "$baseline"
    cp "$work"/ronie/*.png "$baseline"/
    git describe --always --dirty >"$baseline/commit.txt"
    echo "Saved $(jq '.scenarios | length' fixtures/ui-scenarios.json) images in $baseline from $(cat "$baseline/commit.txt")."
    ;;
  baseline)
    if [[ ! -d "$baseline" ]]; then
      echo "No baseline exists. Run: script/check-ui.sh --save-baseline" >&2
      exit 1
    fi
    start_private_display
    capture_all
    "$testbed" --selftest
    echo "Baseline from $(cat "$baseline/commit.txt")."
    compare_with "$baseline" ronnie-baseline
    ;;
  reference)
    reference_exe="${RONIE_REFERENCE_UI_EXE:-/tmp/ronie-reference-ui-build/UiTestbed_artefacts/Debug/UiTestbed}"
    if [[ ! -x "$reference_exe" ]]; then
      echo "Baseline UI testbed is unavailable: $reference_exe" >&2
      exit 1
    fi
    start_private_display
    mkdir -p "$work/reference"
    "$reference_exe" --capture "$work/reference"
    capture_all
    "$testbed" --selftest
    "$testbed" --bench
    cargo test -p ronie-app --test standalone_smoke -- --include-ignored --test-threads=1
    compare_with "$work/reference" reference-default
    ;;
esac
