#!/bin/bash
# usage: run_pit_air.sh [cmdsfile] [logname] — one scripted The Pit run on the spare Air via ~/bin/iw4l-air
# (log copied to context/pit_runs/<logname>.log, screenshots in iw4l-artifacts/air/screenshots/).
D=$(cd "$(dirname "$0")" && pwd)
CMDS=$(realpath "${1:-$D/pit_run.cmds}")
WT=$(cd "$D/../.." && pwd)
IW4L_AIR_TIMEOUT=${IW4L_AIR_TIMEOUT:-280} ~/bin/iw4l-air "$WT" --env IW4L_GSC_STUB_NATIVES=1 --env IW4L_GSC_TRACE_LEVEL=1 \
  -- map so_killspree_trainer --cmds "$(cat "$CMDS")" | grep -v '^screenshot:'
L="$D/${2:-last}.log"
cp "$WT/iw4l-artifacts/air/latest.log" "$L"
"$D/summary.sh" "$L"
