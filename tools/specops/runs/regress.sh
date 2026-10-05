#!/bin/bash
# usage: regress.sh — the three regression checks on the Air: The Pit, snowrace1, mp_boneyard spawn.
# Logs -> context/runs/logs/reg_{pit,snow,mp}.log
D=$(cd "$(dirname "$0")" && pwd); WT=$(cd "$D/../.." && pwd)
mkdir -p "$D/logs"
IW4L_AIR_TIMEOUT=280 ~/bin/iw4l-air "$WT" --env IW4L_GSC_STUB_NATIVES=1 -- map so_killspree_trainer \
  --cmds "$(cat "$D/../pit_runs/pit_run.cmds")" | grep -v '^screenshot:'
cp "$WT/iw4l-artifacts/air/latest.log" "$D/logs/reg_pit.log"
IW4L_AIR_TIMEOUT=200 ~/bin/iw4l-air "$WT" --env IW4L_GSC_STUB_NATIVES=1 --env IW4L_VEH_AUTODRIVE=1 -- map so_snowrace1_cliffhanger \
  --cmds 'wait world; spawn 0; wait 100s; finish_run' | grep -v '^screenshot:'
cp "$WT/iw4l-artifacts/air/latest.log" "$D/logs/reg_snow.log"
IW4L_AIR_TIMEOUT=150 ~/bin/iw4l-air "$WT" -- map mp_boneyard --cmds 'wait world; spawn 0; wait 5s; screenshot mp; finish_run' | grep -v '^screenshot:'
cp "$WT/iw4l-artifacts/air/latest.log" "$D/logs/reg_mp.log"
grep -h "spec ops: mission" "$D/logs/reg_pit.log" "$D/logs/reg_snow.log"
grep -c "InGame" "$D/logs/reg_mp.log"
