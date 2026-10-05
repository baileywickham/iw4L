#!/bin/bash
# One-command vibe capture: record a scripted run on the Air, replay the demo there with
# IW4L_FRAME_DUMP (fixed timestep, every frame to PNG + metrics), pull it back, then build
# metrics/sheets/video/report locally.
#
# usage: capture.sh <mission> <cmds-file> <out-dir> [fps] [K=V ...]
#   cmds-file: one command group per line (# comments ok). `record` is inserted after the
#   first `spawn`, `stoprecord` at the end (a trailing finish_run/quit is dropped), unless
#   the file already says `record`.
#   env: VIBE_SKIP_RECORD=1 reuses the last demo of the same name; VIBE_WIDTH (960);
#        VIBE_MAX_FRAMES; IW4L_AIR_TIMEOUT (default 900).
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../../.." && pwd)
MISSION=$1; CMDS_FILE=$(realpath "$2"); OUT=$(mkdir -p "$3" && cd "$3" && pwd); FPS=${4:-30}
shift 3; [ $# -gt 0 ] && shift
EXTRA=(); for kv in "$@"; do EXTRA+=(--env "$kv"); done
AIR="${IW4L_AIR_BIN:-$HOME/bin/iw4l-air}"
PULL="$WT/tools/specops/bin/iw4l-air-pull"
NAME="vibe_$(basename "$OUT" | tr -c 'A-Za-z0-9_\n' '_')"
export IW4L_AIR_TIMEOUT=${IW4L_AIR_TIMEOUT:-900}

CMDS=$(grep -v '^[[:space:]]*#' "$CMDS_FILE" | grep -v '^[[:space:]]*$' | paste -sd';' -)
if ! grep -q '\brecord\b' <<<"$CMDS"; then
  CMDS=$(sed -E 's/;[[:space:]]*(finish_run|quit)[[:space:]]*$//' <<<"$CMDS")
  CMDS=$(perl -pe "s/(spawn\s+\d+)/\$1; record $NAME/" <<<"$CMDS")
  CMDS="$CMDS; stoprecord; finish_run"
fi

if [ "${VIBE_SKIP_RECORD:-0}" != 1 ]; then
  echo "== record $NAME ($MISSION) on the Air"
  rm -f "$WT/iw4l-artifacts/air/latest.log"
  "$AIR" "$WT" --env IW4L_GSC_STUB_NATIVES=1 ${EXTRA[@]+"${EXTRA[@]}"} -- map "$MISSION" --cmds "$CMDS" | grep -v '^screenshot:' || true
  cp "$WT/iw4l-artifacts/air/latest.log" "$OUT/record.log" 2>/dev/null || true
  grep -E "record: writing|stoprecord:" "$OUT/record.log" || { echo "record failed (see $OUT/record.log)"; exit 1; }
fi

echo "== replay $NAME with frame dump on the Air"
REMOTE_DUMP="iw4l-artifacts/vibe/$NAME"
DUMP_ENV=(--env "IW4L_FRAME_DUMP=$REMOTE_DUMP,$FPS" --env "IW4L_FRAME_DUMP_WIDTH=${VIBE_WIDTH:-960}" --env IW4L_GSC_STUB_NATIVES=1)
[ -n "${VIBE_MAX_FRAMES:-}" ] && DUMP_ENV+=(--env "IW4L_FRAME_DUMP_MAX=$VIBE_MAX_FRAMES")
rm -f "$WT/iw4l-artifacts/air/latest.log"
ssh -i "$HOME/.ssh/macnode" -o BatchMode=yes baileywickham@192.168.0.50 "rm -rf runs/$(basename "$WT")/$REMOTE_DUMP"
"$AIR" "$WT" "${DUMP_ENV[@]}" ${EXTRA[@]+"${EXTRA[@]}"} -- play "$NAME" | grep -v '^screenshot:' || true
cp "$WT/iw4l-artifacts/air/latest.log" "$OUT/replay.log" 2>/dev/null || true
grep -E "frame dump:" "$OUT/replay.log" | tail -3 || true

echo "== pull"
"$PULL" "$WT" "$REMOTE_DUMP" "$OUT"

echo "== analyse"
python3 "$D/vibe.py" all "$OUT"
echo "report: $OUT/report.md"
