#!/bin/bash
# usage: run_pit.sh [cmdsfile] [logname]  — one scripted The Pit run (≤240 s), log copied to context/pit_runs/<logname>.log
CMDS=$(realpath "${1:-$(dirname "$0")/pit_run.cmds}")
cd "$(dirname "$0")/../.."

while pgrep -x iw4l >/dev/null; do sleep 5; done
set -a; . ./.env; set +a
IW4L_GSC_STUB_NATIVES=1 IW4L_GSC_TRACE_LEVEL=1 timeout 240 ./target/play/iw4l map so_killspree_trainer --cmds "$(cat "$CMDS")" >/dev/null 2>&1
echo "exit=$?"
cp iw4l-artifacts/logs/latest.log "context/pit_runs/${2:-last}.log"
L="context/pit_runs/${2:-last}.log"
context/pit_runs/summary.sh "context/pit_runs/${2:-last}.log"
