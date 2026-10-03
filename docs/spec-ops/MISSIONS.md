# Spec Ops mission status

Measured 2026-10-03 on the M4 Pro: each mission ~90 s, `IW4L_GSC_STUB_NATIVES=1 IW4L_GSC_USAGE=1 IW4L_GSC_STATS=1 IW4L_GSC_TRACE_LEVEL=1`,
`--cmds 'wait world; spawn 0; god; [start]; wait 85s; screenshot; finish_run'`. `[start]` teleports into the mission's start
trigger (hull centre from `IW4L_TRIGGER_LOG=1`; the brush `origin` key is not inside L-shaped triggers) or fires once (demo).
Scripts and logs: `context/run.sh`, `context/batch.sh`, `context/summ.py`, `context/passd.txt` (worktree scratch, untracked).

- **Started** = the flag `enable_challenge_timer` waits on was notified.
- **AI** = actors spawned (axis) · axis that acquired the player · that fired at it. Player in god mode, not shooting.
- **Err** = GSC runtime errors in the run (all hits). **GSC** = script ms per tick, average / worst 200-tick window peak.
- All 23 load (base map + mission zone + SP common), spawn the player at `info_player_start_so`/`_pmc`, and get the mission loadout.

| Mission | Base | Started | AI | Err | GSC ms | Shot | Next blocker |
|---|---|---|---|---|---|---|---|
| so_ac130_co_hunted | co_hunted | yes | 6 (0) · 0 · 0 | 2 | 1.0 / 66 | checkerboard world | Co-op only (AC-130 gunner seat); base-map materials missing |
| so_assault_oilrig | oilrig | yes (first breach) | 3 + 2 hostages (breach room) | 0 | 1.1 / 154 | ok, breach in slow-mo | Breach → `breaching_on` (timer) → `setslowmotion 1→0.25`, room cleared; next: play-through (deck 2 rappellers, heli) |
| so_chopper_invasion | invasion | no | 0 | 10039 | 2.3 / 252 | ok | Co-op only: `level.chopperplayer` is player 2 (button natives on undefined spam errors solo) |
| so_crossing_so_bridge | so_bridge | yes | 8 (8) · 0 · 0 | 3 | 1.1 / 60 | ok | Axis stand ~385 u above their spawners and never acquire the player |
| so_defense_invasion | invasion | yes | 20 (20) · 20 · 10 | 10 | 3.5 / 178 | ok | Engages; play-through (waves, sentry turrets) |
| so_defuse_favela_escape | favela_escape | yes | 14 (0; allies + civilians) | 1 | 2.4 / 203 | ok | Enemies spawn on advance; play-through |
| so_demo_so_bridge | so_bridge | yes (first shot) | 0 | 3 | 0.9 / 58 | ok, TARGET markers | Car targets/waves: no AI in 85 s; check car damage → `vehicle_alive_think` |
| so_download_arcadia | arcadia | yes | 6 allies · 0 | 11 | 2.0 / 218 | ok | Stryker (SP vehicle) loads; the turret errors are `common_scripts/_sentry` on `misc_turret` sentries (`setconvergencetime`, `setmode`, … on real turrets) |
| so_escape_airport | airport | yes | 5 (3) · 3 · 0 | 0 | 1.9 / 137 | ok | Play-through |
| so_forest_contingency | contingency | yes | 6 (5 + dog) on `first_patrol_cqb` | 117 | 1.6 / 172 | ok | Stealth: standing player spotted at 489 (`maxvisibledist` 500), squad alerts; dogs, play-through |
| so_hidden_so_ghillies | so_ghillies | yes | 4 (4) · stealth | 1 | 1.0 / 59 | ok, prone near unaware patrol | Stealth works (prone unseen at 157–600, standing spotted at 264–659, gunshot/death alert); ghillie enemies, corpse discovery not yet observed |
| so_intel_boneyard | boneyard | yes | 25 (25) · 2 · 1 | 0 | 1.6 / 121 | ok | Play-through (intel use triggers) |
| so_juggernauts_favela | favela | yes | 1 (1) · 1 · 1 | 40 | 1.6 / 152 | ok | One juggernaut alive at a time by design; `_anim` scenes with missing anims |
| so_killspree_favela | favela | yes | 15 (15) · 11 · 4 | 16 | 2.6 / 136 | ok, enemies in view | Play-through |
| so_killspree_invasion | invasion | yes | 26 (26) · 30 · 10 | 340 → 9 | 4.3 / 165 | ok | BTRs fire `btr80_turret` from the map's VehicleDef (player without god killed at 0:12); play-through |
| so_killspree_trainer | trainer | yes | targets | 271 | 0.7 / 128 | ok | **Completes** (scripted run 2:24, 24/24); radio music aliases missing |
| so_rooftop_contingency | contingency | yes | 15 (15) · 19 · 6 | 87 | 2.9 / 189 | ok | Play-through (waves; UAV/predator) |
| so_sabotage_cliffhanger | cliffhanger | yes | 16 (16) · stealth | 17 | 2.3 / 290 | ok | Prone player unseen by patrols at 68–111 (stealth); the SO deletes the snowmobiles (no snowmobile ending); truck riders `tag_guy*` |
| so_showers_gulag | gulag | yes | 16 (16) · 16 · 2 | 0 | 4.0 / 474 | ok, breach in slow-mo | First breach (showers) runs in slow-mo; play-through; worst GSC frame 474 ms |
| so_snowrace1_cliffhanger | cliffhanger | yes (timer) | bikes + riders | 11575 → 17 | 3.0 / 333 | ok, timer + stars | **Completes** with `IW4L_VEH_AUTODRIVE=1`: 0:58.60 (3-star time), no god; drivable snowmobile, final jump |
| so_snowrace2_cliffhanger | cliffhanger | yes (timer) | 1 | 1 | 1.7 / 342 | ok | Autopilot drives the whole course and the jump, 20–24 gates, but the gate timer runs out at the jump |
| so_takeover_estate | estate | yes | 25 (25) · 7 · 1 | 0 | 1.6 / 95 | ok | Play-through (this Spec Op deletes the estate breaches: `so_delete_breach_ents`) |
| so_takeover_oilrig | oilrig | yes | 1 (1) · 0 | 0 | 1.2 / 135 | ok, pickups on the floor | One enemy alive at a time by design; play-through |

**Summary:** 23/23 load and spawn with the mission loadout; 21/23 start solo (assault_oilrig needs a breach, chopper a
second player); in 9 enemies acquire the player within 85 s and in 8 they shoot; 1 completes (The Pit); snowrace1 completes under the test autopilot (vehicles pass, arrows mark its re-measured error counts). No mission hangs
(snowrace used to stall at tick 4). Before these fixes (same runs, no start teleport): 2.5 M errors in arcadia,
528 k in sabotage, ~100–750 stub calls per mission; now 0–340 errors except chopper (co-op) and snowrace1.

**Stealth and breach (2026-10-03, later):** the hidden, sabotage, forest, oilrig and gulag rows come from targeted runs
(start-trigger teleport, then prone/stand/fire or `+activate` at a `trigger_use_breach`), not the 85 s sweep above.
oilrig now starts solo (22/23). Logs: `actor: … acquired enemy … maxvisibledist=… <stance>`, `actor: … alertlevel a -> b`,
`actor: … heard <event> from …`, `setslowmotion: from -> to over ms`. Evidence in `context/stealth/` (worktree scratch).

## Cross-mission blockers, ranked

1. **Co-op-only missions** (ac130, chopper): need a two-player run; the AC-130/chopper gunner views.
2. ~~**Player vehicles**~~ (2026-10-03, vehicles pass): `mountvehicle`/`dismountvehicle`, an arcade snowmobile over the
   clip map (`script/host/vehicle_drive.rs`), `vehphys_setspeed`; snowrace1 completes. Open: snowrace2 gate timing, the
   first-person snowmobile/hands models are not drawn, enemy bikes still follow `vehicledriveto` in straight lines.
3. **SP vehicle assets** (mostly done 2026-10-03): VehicleDefs from SP map/mission zones (turret weapon: BTR fires
   `btr80_turret`), tags on vehicles spawned this frame (`tag_driver`, riders link), bone-only skeletons of SP common and
   mission XModels for tag lookups, case-insensitive tag names, `setvehicleteam`. Open: `misc_turret` sentries
   (`_sentry`: `setconvergencetime`, `setmode`, …), so_bridge/forest rider runs not re-measured.
4. ~~**Stealth** (`_stealth_*`, S5)~~: engine side done (sight limited by the target's `maxvisibledist`, AI events and
   `addaieventlistener`, `alertlevel`); remaining: ghillie enemies, corpse discovery in a real run, `ai_busyEvent*`.
5. ~~**Breach** (`_slowmo_breach`)~~: oilrig starts on its first breach, gulag showers breach runs; estate's Spec Op has none.
6. Missing SP sound aliases (`playsound: sound alias not found`), `useby`, `getturret` on real turrets.
