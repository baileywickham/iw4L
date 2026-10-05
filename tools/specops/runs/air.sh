#!/bin/bash
# usage: air.sh <map> <cmdsfile> <logname> [timeout_s] [K=V ...]
# Same as run.sh but on the spare Air through ~/bin/iw4l-air; log -> context/runs/logs/<logname>.log.
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../.." && pwd)
MAP=$1; CMDS=$(realpath "$2"); NAME=$3; T=${4:-300}; shift 4 2>/dev/null || shift $#
ENVS=(--env IW4L_GSC_STUB_NATIVES=1 --env IW4L_GSC_TRACE_LEVEL=1)
for kv in "$@"; do ENVS+=(--env "$kv"); done
mkdir -p "$D/logs"
IW4L_AIR_TIMEOUT=$T ~/bin/iw4l-air "$WT" "${ENVS[@]}" -- map "$MAP" --cmds "$(grep -v '^#' "$CMDS" | grep -v '^[[:space:]]*$' | paste -sd';' -)" | grep -v '^screenshot:'
cp "$WT/iw4l-artifacts/air/latest.log" "$D/logs/$NAME.log"
rm -rf "$D/logs/${NAME}_shots"; cp -r "$WT/iw4l-artifacts/air/screenshots" "$D/logs/${NAME}_shots" 2>/dev/null
grep -E "spec ops: mission (success|fail)" "$D/logs/$NAME.log" | head -3
