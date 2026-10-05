# Scripted Spec Ops runs (objective missions)

Modeled on `context/pit_runs/`: a `gen.py` per mission turns the map + mission entity dump into a
`--cmds` script (`run.cmds`). Routes are teleport hops (`tp … &`) over path nodes (`so_lib.route`,
Dijkstra over `node_*` entities); the mission's own triggers, use triggers, timers and destructibles
do the rest. `god` is on in every run.

```bash
python3 context/runs/ents.py context/runs/ents_<name>.txt <basemap> <mission zone>   # entity dump
python3 context/runs/<mission>/gen.py > context/runs/<mission>/run.cmds
context/runs/run.sh <map> <cmds> <logname> [timeout] [K=V ...]   # local; log -> context/runs/logs/
context/runs/air.sh <map> <cmds> <logname> [timeout] [K=V ...]   # same on the Air (~/bin/iw4l-air)
context/runs/regress.sh                                           # Pit, snowrace1, mp_boneyard on the Air (local runs: the watchdog kills them)
python3 context/runs/q.py <name> <regex>                          # grep an entity dump
```

- `so_crossing_so_bridge/` — start trigger, then hops north along the deck into `so_obj_crossing` (0:27.80).
- `so_escape_airport/` — hops through the terminal triggers in story order into `escaped_trigger` (0:49.25).
- `so_intel_boneyard/` — both `pmc_objective` laptops (the mission picks one at random), +activate 4.5 s, then the extraction trigger (1:08.15).
- `so_defuse_favela_escape/` — three briefcases: +activate held 6.5 s each (link, briefcase weapon, 4.5 s use bar) (1:01.90).
- `so_demo_so_bridge/` — two RPG rockets at every target car from ~380 u (`give ammo` between shots); cars explode through `_destructible` (5:05.70, 36/36).
- `so_download_arcadia/` — three laptop downloads with `IW4L_AUTOFIRE=1` (test aimer kills the defenders) and a patrol around each laptop, `give ammo` every ~10 s, then extraction at the Stryker (7:45.70, Air, 1200 s timeout). `stryker_probe.cmds` checks the Stryker path moves.
