#!/usr/bin/env bash
# Proves the system route on a private PipeWire server with fake nodes.
#
# This starts its own pipewire and pipewire-pulse with script/private-pipewire. It has no ALSA, no
# session manager, and no device monitor, and its runtime directory is private, so that it cannot
# reach real hardware or the sound server of the desktop. With no session manager, this script
# makes each link itself.
#
# usage: script/check-system-route.sh [buffer frames ...]
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
repository="$PWD"

command -v pipewire >/dev/null && command -v pw-link >/dev/null && command -v pw-cat >/dev/null && command -v pw-metadata >/dev/null || {
  echo "This check needs pipewire, pw-link, pw-cat, and pw-metadata." >&2
  exit 1
}

# An optimized build, because a debug build of the sound server client runs too slowly for audio.
cargo test --release --locked -p ronie-app --test system_route --no-run
test_binary="$(cargo test --release --locked -p ronie-app --test system_route --no-run --message-format=json 2>/dev/null |
  jq -r 'select(.executable != null and .target.name == "system_route") | .executable' | tail -1)"

buffers=("$@")
[[ ${#buffers[@]} -gt 0 ]] || buffers=(64 128 256 512 1024)

# A socket path has a limit of 108 bytes, so that a short link points at the work directory.
scratch="$(mktemp -d)"
work="/tmp/ronnie-route.$$"
ln -s "$scratch" "$work"
server_pid=""
pulse_pid=""
linker_pid=""
feed_pid=""
record_pid=""

stop_all() {
  for pid in "$record_pid" "$feed_pid" "$linker_pid" "$pulse_pid" "$server_pid"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -f "$work"
  if [[ -z "${KEEP_WORK:-}" ]]; then
    rm -rf "$scratch"
  else
    echo "Work directory: $scratch"
  fi
}
trap stop_all EXIT

mkdir -p "$work/run" "$work/home" "$work/config"
chmod 700 "$work/run"
: >"$work/no-alsa.conf"
export XDG_RUNTIME_DIR="$work/run" HOME="$work/home" XDG_CONFIG_HOME="$work/config"
export PIPEWIRE_RUNTIME_DIR="$work/run"
# ALSA has no devices in this script, so that a fault cannot open real hardware.
export ALSA_CONFIG_PATH="$work/no-alsa.conf"
unset PIPEWIRE_REMOTE PULSE_SERVER PULSE_RUNTIME_PATH DBUS_SESSION_BUS_ADDRESS DISPLAY WAYLAND_DISPLAY

pipewire -c "$repository/script/private-pipewire/pipewire.conf" >"$work/server.log" 2>&1 &
server_pid=$!
sleep 0.5
pipewire -c "$repository/script/private-pipewire/pipewire-pulse.conf" >"$work/pulse.log" 2>&1 &
pulse_pid=$!
for _ in {1..100}; do
  [[ -S "$work/run/pulse/native" ]] && break
  sleep 0.05
done

node_id() {
  pw-cli ls Node | awk -v name="$1" '/^[ \t]*id [0-9]+/ { gsub(",", "", $2); id = $2 } index($0, "node.name = \"" name "\"") { print id }'
}

# With no session manager, a device node has no ports until it has this configuration.
expose_ports() { # node direction monitor positions channels
  pw-cli set-param "$(node_id "$1")" PortConfig "{ \"direction\": \"$2\", \"mode\": \"dsp\", \"monitor\": $3, \"format\": { \"mediaType\": \"audio\", \"mediaSubtype\": \"raw\", \"format\": \"F32P\", \"rate\": 48000, \"channels\": $5, \"position\": $4 } }" >/dev/null
}
expose_ports fake_headphones Input true '["FL","FR"]' 2
expose_ports fake_guitar_input Output false '["MONO"]' 1
expose_ports fake_guitar_feed Input false '["MONO"]' 1
expose_ports fake_webcam Output false '["FL","FR"]' 2
expose_ports fake_webcam_feed Input false '["FL","FR"]' 2

# The webcam is the system default, so that the automatic order must find the instrument input.
pw-metadata -n default 0 default.audio.sink '{ "name": "fake_headphones" }' >/dev/null
pw-metadata -n default 0 default.audio.source '{ "name": "fake_webcam" }' >/dev/null

# Does for each stream what a session manager does: gives it ports, and links them.
serve_streams() {
  declare -A ready=()
  while :; do
    # The server can use an id again, and a serial is new for each node.
    while read -r id serial class name; do
      [[ -n "${ready[$serial]:-}" ]] && continue
      case "$name" in fake_* | probe_*) continue ;; esac
      case "$class" in
        Stream/Output/Audio)
          pw-cli set-param "$id" PortConfig '{ "direction": "Output", "mode": "dsp", "format": { "mediaType": "audio", "mediaSubtype": "raw", "format": "F32P", "rate": 48000, "channels": 2, "position": ["FL","FR"] } }' >/dev/null 2>&1 || continue
          ;;
        Stream/Input/Audio)
          # The first stream of a round is on the mono input, and the next one on the stereo input.
          if [[ -e "$work/stream-ready" ]]; then
            pw-cli set-param "$id" PortConfig '{ "direction": "Input", "mode": "dsp", "format": { "mediaType": "audio", "mediaSubtype": "raw", "format": "F32P", "rate": 48000, "channels": 2, "position": ["FL","FR"] } }' >/dev/null 2>&1 || continue
          else
            pw-cli set-param "$id" PortConfig '{ "direction": "Input", "mode": "dsp", "format": { "mediaType": "audio", "mediaSubtype": "raw", "format": "F32P", "rate": 48000, "channels": 1, "position": ["MONO"] } }' >/dev/null 2>&1 || continue
          fi
          ;;
        *) continue ;;
      esac
      ready[$serial]=1
    done < <(pw-cli ls Node | awk '
      /^[ \t]*id [0-9]+/ { if (id != "" && class != "") print id, serial, class, name; gsub(",", "", $2); id = $2; class = ""; name = ""; serial = "" }
      /object.serial = / { serial = $3; gsub("\"", "", serial) }
      /node.name = / { name = $3; gsub("\"", "", name) }
      /media.class = / { class = $3; gsub("\"", "", class) }
      END { if (id != "" && class != "") print id, serial, class, name }')
    while read -r port; do
      case "$port" in
        *:output_FL) pw-link "$port" fake_headphones:playback_FL >/dev/null 2>&1 || true ;;
        *:output_FR) pw-link "$port" fake_headphones:playback_FR >/dev/null 2>&1 || true ;;
      esac
    done < <(pw-link -o | rg -v '^(fake_|probe_)' || true)
    while read -r port; do
      case "$port" in
        *:input_MONO) pw-link fake_guitar_input:capture_MONO "$port" >/dev/null 2>&1 || true ;;
        *:input_FL) pw-link fake_webcam:capture_FL "$port" >/dev/null 2>&1 || true ;;
        *:input_FR) pw-link fake_webcam:capture_FR "$port" >/dev/null 2>&1 || true ;;
      esac
    done < <(pw-link -i | rg -v '^(fake_|probe_)' || true)
    # Many short clients disturb the server, so that this loop stays slow.
    sleep 0.25
  done
}

# Starts a probe of this script: gives it ports, and waits for them.
probe_ports() { # node direction positions channels pattern
  for _ in {1..200}; do
    [[ -n "$(node_id "$1")" ]] && break
    sleep 0.01
  done
  expose_ports "$1" "$2" false "$3" "$4"
  for _ in {1..200}; do
    if [[ "$2" == Output ]]; then
      pw-link -o | rg -q "^$1:" && return 0
    else
      pw-link -i | rg -q "^$1:" && return 0
    fi
    sleep 0.01
  done
  echo "The probe $1 has no ports." >&2
  return 1
}

write_sine() { # path seconds
  python3 -I - "$1" "$2" <<'PY'
import math, struct, sys, wave
path, seconds = sys.argv[1], float(sys.argv[2])
rate = 48000
with wave.open(path, "wb") as out:
    out.setnchannels(1)
    out.setsampwidth(2)
    out.setframerate(rate)
    level = 10 ** (-12 / 20)
    out.writeframes(b"".join(
        struct.pack("<h", int(32767 * level * math.sin(2 * math.pi * 1000 * n / rate)))
        for n in range(int(seconds * rate))
    ))
PY
}

analyze() { # path
  python3 -I - "$1" <<'PY'
import math, struct, sys, wave
with wave.open(sys.argv[1], "rb") as source:
    rate = source.getframerate()
    width = source.getsampwidth()
    channels = source.getnchannels()
    raw = source.readframes(source.getnframes())
values = [v / 32768 for (v,) in struct.iter_unpack("<h", raw)]
at_input = values[0::channels]
at_output = values[1::channels]
def onset(samples, level):
    for index, sample in enumerate(samples):
        if abs(sample) > level:
            return index
    return None
start_in = onset(at_input, 0.05)
start_out = onset(at_output, 0.05)
if start_in is None or start_out is None:
    print("no onset in the recording")
    sys.exit(1)
delay = start_out - start_in
print(f"delay from the fake input to the fake output: {delay} frames, {delay * 1000 / rate:.2f} ms")
# The test holds its first stream for two seconds, and this window stays inside that time.
def faults(samples, start):
    body = samples[start + rate // 5 : start + rate * 3 // 2]
    gaps = run = 0
    for sample in body:
        run = run + 1 if sample == 0.0 else 0
        if run == 8:
            gaps += 1
    rising = [i for i in range(1, len(body)) if body[i - 1] <= 0 < body[i]]
    steps = sum(1 for a, b in zip(rising, rising[1:]) if not 47 <= b - a <= 49)
    return len(body) / rate, gaps, steps
seconds, gaps, steps = faults(at_input, start_in)
print(f"fake input in {seconds:.2f} s: {gaps} gaps, {steps} broken periods of the sine")
seconds, gaps, steps = faults(at_output, start_out)
print(f"fake output in {seconds:.2f} s: {gaps} gaps, {steps} broken periods of the sine")
PY
}

measure_tone() { # path
  python3 -I - "$1" <<'PY'
import math, struct, sys, wave
with wave.open(sys.argv[1], "rb") as source:
    rate = source.getframerate()
    width = source.getsampwidth()
    channels = source.getnchannels()
    raw = source.readframes(source.getnframes())
values = [v / 32768 for (v,) in struct.iter_unpack("<h", raw)]
left = values[0::channels]
right = values[1::channels]
loud = [i for i, s in enumerate(left) if abs(s) > 0.02]
if not loud:
    print("no tone in the recording")
    sys.exit(1)
first, last = loud[0], loud[-1]
body = left[first + rate // 10 : last - rate // 10]
peak = max(abs(s) for s in body)
rising = [i for i in range(1, len(body)) if body[i - 1] <= 0 < body[i]]
hertz = (len(rising) - 1) * rate / (rising[-1] - rising[0])
tail = left[last + rate // 100 :]
print(f"tone at the fake sink: {hertz:.1f} Hz, peak {20 * math.log10(peak):.2f} dBFS, length {(last - first) / rate:.3f} s")
print(f"after the tone: peak {max((abs(s) for s in tail), default=0.0):.6f} over {len(tail) / rate:.2f} s")
print(f"both channels equal: {max(abs(a - b) for a, b in zip(left, right)) < 1e-4}")
PY
}

wait_for() { # file
  for _ in {1..2000}; do
    [[ -e "$1" ]] && return 0
    kill -0 "$test_pid" 2>/dev/null || return 1
    sleep 0.01
  done
  return 1
}

write_sine "$work/sine.wav" 30
serve_streams &
linker_pid=$!

status=0
for frames in "${buffers[@]}"; do
  echo "=== buffer $frames"
  round="$work/round-$frames"
  mkdir -p "$round"
  rm -f "$work"/stream-ready "$work"/second-stream-ready "$work"/feed-can-stop "$work"/tone-starts "$work"/tone-ended "$work"/done
  RONNIE_PRIVATE_SERVER="$work" RONNIE_BUFFER="$frames" "$test_binary" --nocapture >"$round/test.log" 2>&1 &
  test_pid=$!
  wait_for "$work/stream-ready" || true

  # Each frame in the recording holds the input of the program and the output of the program.
  pw-cat --record -P '{ node.name = probe_record }' --format s16 --rate 48000 --channels 2 "$round/through.wav" >"$round/record.log" 2>&1 &
  record_pid=$!
  probe_ports probe_record Input '["FL","FR"]' 2
  pw-link fake_guitar_input:capture_MONO probe_record:input_FL >/dev/null 2>&1 || true
  pw-link fake_headphones:monitor_FL probe_record:input_FR >/dev/null 2>&1 || true
  sleep 0.3

  pw-cat --playback -P '{ node.name = probe_feed }' "$work/sine.wav" >"$round/feed.log" 2>&1 &
  feed_pid=$!
  probe_ports probe_feed Output '["MONO"]' 1
  pw-link probe_feed:output_MONO fake_guitar_feed:playback_MONO >/dev/null 2>&1 || true
  pw-link probe_feed:output_MONO fake_webcam_feed:playback_FR >/dev/null 2>&1 || true

  wait_for "$work/feed-can-stop" || true
  kill "$feed_pid" 2>/dev/null || true
  feed_pid=""
  kill "$record_pid" 2>/dev/null || true
  wait "$record_pid" 2>/dev/null || true
  record_pid=""

  wait_for "$work/tone-starts" || true
  pw-cat --record -P '{ node.name = probe_tone }' --format s16 --rate 48000 --channels 2 "$round/tone.wav" >"$round/tone-record.log" 2>&1 &
  record_pid=$!
  probe_ports probe_tone Input '["FL","FR"]' 2
  pw-link fake_headphones:monitor_FL probe_tone:input_FL >/dev/null 2>&1 || true
  pw-link fake_headphones:monitor_FR probe_tone:input_FR >/dev/null 2>&1 || true

  test_status=0
  wait "$test_pid" || test_status=$?
  kill "$record_pid" 2>/dev/null || true
  wait "$record_pid" 2>/dev/null || true
  record_pid=""

  cat "$round/test.log"
  [[ "$test_status" -eq 0 ]] || status=$test_status
  [[ -e "$work/done" ]] || status=1
  [[ -s "$round/through.wav" ]] && analyze "$round/through.wav" || status=1
  [[ -s "$round/tone.wav" ]] && measure_tone "$round/tone.wav" || status=1
  echo "capture overruns in the server log so far: $(rg -c 'overrun recover' "$work/pulse.log" || true)"
done
exit "$status"
