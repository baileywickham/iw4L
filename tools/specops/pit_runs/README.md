# The Pit (so_killspree_trainer) scripted full run

`pit_run.py` turns the map + mission entity dump (`ents.txt`) into a console
script (`pit_run.cmds`) that plays the whole course: all 24 enemy boards
(rails swept, the stage-14 board knifed on the stairs), the flag triggers the
mission waits on (`player_course_03a`, stairs, upstairs, jumping/jumped down),
then the end trigger. It ends in `special_op_succeeded` and the EOG summary.

```bash
python3 context/pit_runs/pit_run.py > context/pit_runs/pit_run.cmds   # plan → stderr
context/pit_runs/run_pit.sh [cmds] [logname]   # local run (≤240 s), log → context/pit_runs/<logname>.log
context/pit_runs/summary.sh <log>              # stages, flags, result
```

`run_pit_air.sh` does the same through `~/bin/iw4l-air` (spare Air).
`PIT_SHOTS=1` adds a screenshot per vantage. The run needs
`IW4L_GSC_STUB_NATIVES=1` (set by the run scripts); `god` is on.

Expected log tail (`pit_success_a.log`, `pit_success_b.log`):

```
spec ops: mission success map=so_killspree_trainer time=2:28.05 finished_time=148.05 star_count=1 targets_hit=24 friendlies_hit=0 menu=sp_eog_summary
spec ops: eog row 1: @SO_KILLSPREE_TRAINER_SCOREBOARD_FINISH_TIME | 2:28.05
spec ops: eog row 2: @SO_KILLSPREE_TRAINER_SCOREBOARD_ENEMIES_HIT | 24/24
spec ops: eog row 3: @SO_KILLSPREE_TRAINER_SCOREBOARD_CIVS_HIT | 0/5
```

One star: the run takes ~2:25 (2 stars need <45 s, 3 need <35 s and no civilian).

Notes for editing the script:
- `wait` takes seconds (`0.25s`); `ms` is not a unit (parses as 1 s).
- `hold +attack` must last ≥0.2 s to fire; a 0.25 s hold fires 3-4 rounds, so the
  script reloads (`give ammo; press +reload`) every 7 holds.
- Rail boards start at a random end (`cointoss`) and stop when the player is within
  128 units; a run can still miss a rail board now and then (2/2 recent runs passed).
- Older files here (`gen.py`, `stages.py`, `full*.log`, `s*.log`) are the earlier
  attempt from another worktree.
