#!/usr/bin/env bash
# Both builds, at the same time, in the same five minutes.
#
# Sequential runs are not comparable: the companion stops idling altogether behind a fullscreen
# window, Windows drops its timer when the machine is busy, and the evening moves it towards
# bedtime -- so two runs an hour apart are two different experiments. Run side by side, both
# instances see the identical desktop, and the only difference left is the code.
#
# They cannot collide: Pip keys its instance on PIP_DATA_DIR (window title and mutex), so these are
# two separate companions that cannot see or talk to each other.
set -u
S="C:/Users/kruse/AppData/Local/Temp/claude/D--Claude-Hosdala/03e36f1d-083b-4d61-8de7-e8561c6fdd95/scratchpad"
WINDOW=${1:-300}
A="$S/dev-target/release/pip.exe"      # the blink unhooked from liveliness
B="$S/dev-target-30/release/pip.exe"   # as it was

start() {
  local dir="$1" exe="$2"
  rm -rf "$dir"; mkdir -p "$dir"
  printf '%s' '{ "pet": { "liveliness": "lively" }, "notify": { "sound": false } }' > "$dir/settings.json"
  PIP_DATA_DIR="$dir" PIP_MEASURE_AWAKE=1 nohup "$exe" --portable >/dev/null 2>&1 &
}

frames() { PIP_DATA_DIR="$1" "$2" --cmd stats | sed 's/.*"frames":\([0-9]*\).*/\1/'; }

start "$S/pair-new" "$A"
start "$S/pair-old" "$B"
sleep 60

a0=$(frames "$S/pair-new" "$A"); b0=$(frames "$S/pair-old" "$B")
sleep "$WINDOW"
a1=$(frames "$S/pair-new" "$A"); b1=$(frames "$S/pair-old" "$B")

PIP_DATA_DIR="$S/pair-new" "$A" --cmd quit >/dev/null
PIP_DATA_DIR="$S/pair-old" "$B" --cmd quit >/dev/null
rm -rf "$S/pair-new" "$S/pair-old"

echo "same ${WINDOW}s, same desktop:"
echo "  blink unhooked : $((a1 - a0)) drawings"
echo "  as it was      : $((b1 - b0)) drawings"
