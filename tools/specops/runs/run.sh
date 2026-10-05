#!/bin/bash
# usage: run.sh <map> <cmdsfile> <logname> [timeout_s] [K=V ...]
# One local scripted run (waits for any other iw4l first); log -> context/runs/logs/<logname>.log,
# screenshots -> context/runs/logs/<logname>_shots/. IW4L_GSC_STUB_NATIVES=1 is always set.
MAP=$1; CMDS=$(realpath "$2"); NAME=$3; T=${4:-300}; shift 4 2>/dev/null || shift $#
cd "$(dirname "$0")/../.."
mkdir -p context/runs/logs
LOCK=context/runs/logs/.run.lock
until mkdir "$LOCK" 2>/dev/null; do sleep 5; done
trap 'rmdir "$LOCK"' EXIT
while pgrep -x iw4l >/dev/null; do sleep 0.5; done
set -a; . ./.env; set +a
rm -rf iw4l-artifacts/screenshots
env IW4L_GSC_STUB_NATIVES=1 IW4L_GSC_TRACE_LEVEL=1 "$@" timeout "$T" ./target/play/iw4l map "$MAP" --cmds "$(grep -v '^#' "$CMDS" | grep -v '^[[:space:]]*$' | paste -sd';' -)" >/dev/null 2>&1
echo "exit=$?"
cp iw4l-artifacts/logs/latest.log "context/runs/logs/$NAME.log"
rm -rf "context/runs/logs/${NAME}_shots"; [ -d iw4l-artifacts/screenshots ] && cp -r iw4l-artifacts/screenshots "context/runs/logs/${NAME}_shots"
grep -E "spec ops: mission|missionfailed|special_op_failed" "context/runs/logs/$NAME.log" | head -5
