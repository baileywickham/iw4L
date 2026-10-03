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
| so_ac130_co_hunted | co_hunted | yes (co-op) | hunters · acquire ground player | 4 + gameskill | — | ok; gunner thermal + HUD | **Completes in co-op** (Air, Regular, 2:59.60): client in the AC-130 (`tag_player` of `c130_zoomrig`, ~5200 AGL, thermal, 105/40/25 mm cycle on `weapnext`), 105/40 mm shells explode, 40 mm kill credited to the gunner; ground player reaches checkpoint B. Open: zoom (`cg_playerFovScale*`), gunner kills not in the EOG partner count |
| so_assault_oilrig | oilrig | yes (first breach) | 3 + 2 hostages (breach room) | 0 | 1.1 / 154 | ok, breach in slow-mo | Breach → `breaching_on` (timer) → `setslowmotion 1→0.25`, room cleared; next: play-through (deck 2 rappellers, heli) |
| so_chopper_invasion | invasion | yes (co-op) | waves on the route | 4 + gameskill | — | ok; door-gunner view | **Completes in co-op** (Air, Regular, 1:17.90): client on the Black Hawk door minigun (`useby`), chopper lifts off and orbits the ground player, driveby/parking lot/diner end path; minigun rounds kill (7–10 kills credited to the gunner per run); ground player on the roof ends it |
| so_crossing_so_bridge | so_bridge | yes | 8 (8) · 8 · 3 (after a player shot) | 0 | 1.1 / 60 | ok, rappellers on the deck | Rappellers land (actor `animscripted` root motion; they hung 385 u up at the anim start); a still, silent player beside them is outside their fov; play-through |
| so_defense_invasion | invasion | yes | 20 (20) · 20 · 10 | 10 | 3.5 / 178 | ok | Engages; play-through (waves, sentry turrets) |
| so_defuse_favela_escape | favela_escape | yes | 14 (0; allies + civilians) | 1 | 2.4 / 203 | ok | Enemies spawn on advance; play-through |
| so_demo_so_bridge | so_bridge | yes (first shot) | 0 | 3 | 0.9 / 58 | ok, TARGET markers | Car targets/waves: AI spawn as the player advances (same rappel fix as crossing); car `"exploded"` (destructibles) not yet checked |
| so_download_arcadia | arcadia | yes | 6 allies · 0; axis sentry | 0 | 2.0 / 218 | ok, sentry drawn | Map `misc_turret` sentry is a turret: `_sentry` runs without errors (11 → 0), the axis minigun tracks and shoots the player (`turret: … targets player 0`, hits by `misc_turret`); play-through |
| so_escape_airport | airport | yes (start trigger) | 5 (3) · 3 · 2 | 6 | 1.9 / 137 | ok | Engages after the start-trigger teleport (`tp 3040 3312 70`); play-through |
| so_forest_contingency | contingency | yes | 6 (5 + dog) on `first_patrol_cqb` | 117 | 1.6 / 172 | ok | Stealth: standing player spotted at 489 (`maxvisibledist` 500), squad alerts; dogs, play-through |
| so_hidden_so_ghillies | so_ghillies | yes | 4 (4) · stealth | 1 | 1.0 / 59 | ok, prone near unaware patrol | Stealth works (prone unseen at 157–600, standing spotted at 264–659, gunshot/death alert); ghillie enemies, corpse discovery not yet observed |
| so_intel_boneyard | boneyard | yes | 25 (25) · 2 · 1 | 0 | 1.6 / 121 | ok | Play-through (intel use triggers) |
| so_juggernauts_favela | favela | yes | 1 (1) · 1 · 1 | 10 | 1.6 / 152 | ok | One juggernaut alive at a time by design; remaining errors are `_hiding_door` scenes with no anim tree (`level.scr_animtree["hiding_door"]` unset) |
| so_killspree_favela | favela | yes | 15 (15) · 11 · 4 | 16 | 2.6 / 136 | ok, enemies in view | Play-through |
| so_killspree_invasion | invasion | yes | 26 (26) · 30 · 10 | 340 → 9 | 4.3 / 165 | ok | BTRs fire `btr80_turret` from the map's VehicleDef (player without god killed at 0:12); play-through |
| so_killspree_trainer | trainer | yes | targets | 271 | 0.7 / 128 | ok | **Completes** (scripted run 2:24, 24/24); radio music aliases missing |
| so_rooftop_contingency | contingency | yes | 15 (15) · 17 · 9 | 87 → 6 | 2.9 / 189 | ok | Play-through (waves; UAV/predator); `add_spawn_function` foreach on undefined once |
| so_sabotage_cliffhanger | cliffhanger | yes | 16 (16) · stealth | 17 | 2.3 / 290 | ok | Prone player unseen by patrols at 68–111 (stealth); the SO deletes the snowmobiles (no snowmobile ending); truck riders `tag_guy*` |
| so_showers_gulag | gulag | yes | 16 (16) · 16 · 2 | 0 | 4.0 / 474 | ok, breach in slow-mo | First breach (showers) runs in slow-mo; play-through; worst GSC frame 474 ms |
| so_snowrace1_cliffhanger | cliffhanger | yes (timer) | bikes + riders | 11575 → 4 | 3.0 / 333 | ok, timer + stars | **Completes** with `IW4L_VEH_AUTODRIVE=1`: 0:58.60 (3-star time), no god; drivable snowmobile, final jump |
| so_snowrace2_cliffhanger | cliffhanger | yes (timer) | 1 | 1 | 1.7 / 342 | ok, Mission Success | **Completes** with `IW4L_VEH_AUTODRIVE=1`: 1:25.30, 26/29 gates, deterministic |
| so_takeover_estate | estate | yes | 25 (25) · 7 · 1 | 0 | 1.6 / 95 | ok | Play-through (this Spec Op deletes the estate breaches: `so_delete_breach_ents`) |
| so_takeover_oilrig | oilrig | yes (start trigger) | 1 (1) · 0 in 85 s | 0 | 1.2 / 135 | ok, pickups on the floor | Players now start at `info_player_start_pmc` (were at the SP start, 21 k u away); after the `mission_start` trigger (`tp 1064 420 -1900`) the juggernaut paths to the player (one alive at a time by design) |

**Summary:** 23/23 load and spawn with the mission loadout; 21/23 start solo (assault_oilrig needs a breach, chopper a
second player); in 9 enemies acquire the player within 85 s and in 8 they shoot; 1 completes (The Pit); snowrace1 completes under the test autopilot (vehicles pass, arrows mark its re-measured error counts). No mission hangs
(snowrace used to stall at tick 4). Before these fixes (same runs, no start teleport): 2.5 M errors in arcadia,
528 k in sabotage, ~100–750 stub calls per mission; now 0–340 errors except chopper (co-op) and snowrace1.

**Blocker pass (2026-10-03, Air):** snowrace2 completes (1:25.30, 26 gates); arcadia's sentry tracks and fires (errors
11 → 0); crossing's rappellers land on the deck and engage; takeover_oilrig starts at its PMC start; airport engages
after its start trigger. Logs in `context/runs/air/` (worktree scratch). Start teleports: airport `tp 3040 3312 70`,
crossing `tp 10496 32144 10`, takeover_oilrig `tp 1064 420 -1900`, arcadia sentry view `tp 395 -2040 2460 259 0`.

**Stealth and breach (2026-10-03, later):** the hidden, sabotage, forest, oilrig and gulag rows come from targeted runs
(start-trigger teleport, then prone/stand/fire or `+activate` at a `trigger_use_breach`), not the 85 s sweep above.
oilrig now starts solo (22/23). Logs: `actor: … acquired enemy … maxvisibledist=… <stance>`, `actor: … alertlevel a -> b`,
`actor: … heard <event> from …`, `setslowmotion: from -> to over ms`. Evidence in `context/stealth/` (worktree scratch).

## Cross-mission blockers, ranked

1. ~~**Co-op-only missions**~~ (2026-10-03, both complete in a two-player run on the Air): base maps without
   createart fog now draw unfogged (co_hunted's checkerboard was every world/model draw refused for missing fog
   constants), bone-only XModels (`c130_zoomrig`) keep their tags, tagged `playerLinkTo*` places the player on the tag,
   `useby` seats a player on a vehicle turret (aim = view, attack → `turret_fire`), `weapon_fired`, `weapnext`/`weapprev`
   reach `notifyOnCommand`, `coop_start` (default `so_char_client`: the joining player is the gunner; the host can
   `set coop_start so_char_host`), decoded snapshots up to 8 MiB (invasion's bootstrap snapshot exceeded 256 KiB and the
   client never joined), party wait 240 s. Open: AC-130 zoom FOV, laser designator drawing, `IsSplitscreen` paths.
2. ~~**Player vehicles**~~ (2026-10-03, vehicles pass): `mountvehicle`/`dismountvehicle`, an arcade snowmobile over the
   clip map (`script/host/vehicle_drive.rs`), `vehphys_setspeed`; snowrace1 and snowrace2 complete. Open: the
   first-person snowmobile/hands models are not drawn; AI bikes are slower than the test autopilot and get wiped out
   as left behind (by design of `_vehicle_spline`).
3. **SP vehicle assets** (mostly done 2026-10-03): VehicleDefs from SP map/mission zones (turret weapon: BTR fires
   `btr80_turret`), tags on vehicles spawned this frame (`tag_driver`, riders link), bone-only skeletons of SP common and
   mission XModels for tag lookups, case-insensitive tag names, `setvehicleteam`; map `misc_turret`s are turrets
   (`_sentry` sentries fight by sentient team); `vehicledriveto` vehicles ride the ground between their spline goals.
   Open: so_bridge/forest rider runs not re-measured, car destructibles (`"exploded"`, so_demo).
4. ~~**Stealth** (`_stealth_*`, S5)~~: engine side done (sight limited by the target's `maxvisibledist`, AI events and
   `addaieventlistener`, `alertlevel`); remaining: ghillie enemies, corpse discovery in a real run, `ai_busyEvent*`.
5. ~~**Breach** (`_slowmo_breach`)~~: oilrig starts on its first breach, gulag showers breach runs; estate's Spec Op has none.
6. Missing SP sound aliases (`playsound: sound alias not found`), `useby`, `getturret` on real turrets.
