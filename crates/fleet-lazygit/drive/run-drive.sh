#!/bin/zsh
# run-drive.sh <repo-path> <drive-script> [timeout-seconds]
#
# Launches `target/release/fleet-lazygit` on <repo-path> with FLEET_LAZYGIT_DRIVE
# pointed at a scratch file, appends <drive-script> to it and waits for the
# driver to log `done quit`. Prints the drive log, then the app's stderr.
#
# $SCRATCH is where the drive file, its log and the app's stderr land.
set -u
BIN=${FLEET_LAZYGIT_BIN:-$(git rev-parse --show-toplevel)/target/release/fleet-lazygit}
SCRATCH=${SCRATCH:-${TMPDIR:-/tmp}/fleet-lazygit-drive}
REPO=$1
SCRIPT=$2
LIMIT=${3:-60}
mkdir -p "$SCRATCH"
DRIVE=$SCRATCH/drive.txt
rm -f "$DRIVE" "$DRIVE.log"
: > "$DRIVE"
RUST_LOG=${RUST_LOG:-warn} FLEET_LAZYGIT_DRIVE="$DRIVE" "$BIN" "$REPO" > "$SCRATCH/app.log" 2>&1 &
APP=$!
# The driver logs `start <path>` once it is armed; appending before that is lost.
for i in $(seq 1 100); do
  [[ -f "$DRIVE.log" ]] && grep -q ' start ' "$DRIVE.log" && break
  sleep 0.1
done
cat "$SCRIPT" >> "$DRIVE"
for i in $(seq 1 $((LIMIT * 10))); do
  grep -q 'done quit' "$DRIVE.log" 2>/dev/null && break
  kill -0 $APP 2>/dev/null || break
  sleep 0.1
done
sleep 0.4
kill $APP 2>/dev/null
wait $APP 2>/dev/null
echo "--- drive log ---"; cat "$DRIVE.log"
[[ -s "$SCRATCH/app.log" ]] && { echo "--- app stderr ---"; cat "$SCRATCH/app.log"; }
