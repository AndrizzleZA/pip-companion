#!/usr/bin/env bash
set -u
cd "/d/Claude Hosdala/pip"
S="/d/Claude Hosdala/pip/.tmp/shots"
mkdir -p "$S"
D="D:/Claude Hosdala/pip/.tmp/stage"
EXE="target/debug/pip.exe"
export PIP_DATA_DIR="$D"
WHO="${1:-sprout}"
for exp in 0 15 45; do
  rm -rf "$D"; mkdir -p "$D"
  printf '{"pet":{"character":"%s","size":"large"}}' "$WHO" > "$D/settings.json"
  printf '{"version":1,"created_at":1790000000,"affection":50,"experience":%d}' "$exp" > "$D/state.json"
  ( "$EXE" --portable >/dev/null 2>&1 & ); sleep 4
  "$EXE" --cmd "snap $D" >/dev/null 2>&1; sleep 1
  cp "$D/pet.png" "$S/stage-$WHO-$exp.png" 2>/dev/null && echo "$WHO exp=$exp captured"
  "$EXE" --cmd quit >/dev/null 2>&1; sleep 2
done
