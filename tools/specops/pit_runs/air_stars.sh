#!/bin/bash
# Pit run on the Air with a fresh profile (new stars -> eog_notify_newstars). usage: air_stars.sh <logname>
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../.." && pwd)
sed 's/screenshot so_eog; wait 1s; finish_run/screenshot so_eog; wait 3s; screenshot so_eog2; wait 1s; finish_run/' "$D/pit_run.cmds" > "$D/pit_stars.cmds"
IW4L_AIR_TIMEOUT=${IW4L_AIR_TIMEOUT:-300} ~/bin/iw4l-air "$WT" --env IW4L_GSC_STUB_NATIVES=1 --env IW4L_GSC_TRACE_LEVEL=1 \
  --env IW4L_PROFILE_PATH=/tmp/iw4l-a5f7-$RANDOM/profile.cfg \
  -- map so_killspree_trainer --cmds "$(cat "$D/pit_stars.cmds")" | grep -v '^screenshot:'
cp "$WT/iw4l-artifacts/air/latest.log" "$D/$1.log"
rm -rf "$D/$1_shots"; cp -r "$WT/iw4l-artifacts/air/screenshots" "$D/$1_shots"
"$D/summary.sh" "$D/$1.log" | tail -5
