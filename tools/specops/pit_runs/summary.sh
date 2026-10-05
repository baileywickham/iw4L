#!/bin/bash
# usage: summary.sh <log> — course progress and the mission result of a Pit run
L=$1
echo "target_killed: $(grep -c 'level notify "target_killed"' $L)  civilian_killed: $(grep -c 'level notify "civilian_killed"' $L)"
grep -E 'label=stage|level notify "(course_[a-z_]*_dead|half_way|so_player_course_(jumped_down|end|completed)|challenge_done|missionfailed|missionsuccess|player_course_(upstairs|jumping_down))"|spec ops: mission|spec ops: eog|MOD_MELEE' $L \
  | sed -E 's/ t=[0-9]+//' | awk '!seen[$0]++' | sed -E 's/benchmark-mark: pid=[0-9]+ seq=[0-9]+ ns=[0-9]+ (label=[a-z0-9]+ tick=[0-9]+).*/\1/' | cut -c1-220 | head -60
