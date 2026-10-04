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
| so_crossing_so_bridge | so_bridge | yes | 8 (8) · 8 · 3 (after a player shot) | 0 | 1.1 / 60 | ok, Mission Success | **Completes** (scripted, god + teleport hops over deck path nodes, `context/runs/so_crossing_so_bridge/`): 0:27.80. The 64 u start trigger must be touched (a hop across it never starts the timer) |
| so_defense_invasion | invasion | yes (timer) | waves 1–2 cleared (50 hunters), wave 3: 34/40 hunters + attack heli | 9 | — | ok | **Wave 3 of 5** (Air, Regular, god, `autoaim vehicles`, 84 kills in 23 min). Blockers: the mi28 takes no bullet damage (`bullet_armor` never drops; check bullet collision on SP helicopters), the BTRs of waves 4–5 need the map AT4s (bullet + grenade shielded) and the helper cannot pick up/switch to them; `_attack_heli` `Target_Set` now binds (was an error that ended `kill_heli_logic`); UAV hellfire `MagicBullet` has no owner (refused) |
| so_defuse_favela_escape | favela_escape | yes | 14 (0; allies + civilians) | 1 | 2.4 / 203 | ok, briefcase in hands | **Completes** (scripted, god + hops; Air): 1:01.90, 3/3 bombs. `+activate` at each `defuse_briefcase` → `playerlinkto`, `briefcase_bomb_defuse_sp` (`weapon_change`), 4.5 s `usebuttonpressed` bar |
| so_demo_so_bridge | so_bridge | yes (first shot) | 0 | 3 | 0.9 / 58 | ok, TARGET markers | **Completes** (scripted, god + hops, `give ammo` between RPG shots): 5:05.70, 36/36 cars `"exploded"` through `_destructible`. Fixed: SP rockets climbed (`projectileSpeedUp` 500 added to a linear rocket), blasts missed cars whose linked clip sits elsewhere (slide cars) |
| so_download_arcadia | arcadia | yes | 6 allies · 0; axis sentry | 0 | 2.0 / 218 | ok, DSM HUD | **Completes** (scripted, god + hops, `IW4L_AUTOAIM=1` test aimer, `give ammo`; Air): 7:45.70, 3/3 downloads, extraction at the Stryker. Fixed: SP `attachpath` places the vehicle on its node, a script speed 0 holds, `veh_pathdir` "reverse" drives the path backward |
| so_escape_airport | airport | yes (start trigger) | 5 (3) · 3 · 2 | 6 | 1.9 / 137 | ok, Mission Success | **Completes** (scripted, god + hops through the terminal's triggers in story order): 0:49.25 |
| so_forest_contingency | contingency | yes | 6 (5 + dog) on `first_patrol_cqb` | 117 | 1.6 / 172 | ok | Stealth: standing player spotted at 489 (`maxvisibledist` 500), squad alerts; dogs, play-through |
| so_hidden_so_ghillies | so_ghillies | yes | 4 (4) · stealth | 1 | 1.0 / 59 | ok, prone near unaware patrol | Stealth works (prone unseen at 157–600, standing spotted at 264–659, gunshot/death alert); ghillie enemies, corpse discovery not yet observed |
| so_intel_boneyard | boneyard | yes | 25 (25) · 2 · 1 | 0 | 1.6 / 121 | ok, Mission Success | **Completes** (scripted, god + hops): 1:08.15. `_pmc` picks one of the two laptops at random, the run uses both (3 s bar), then the extraction trigger |
| so_juggernauts_favela | favela | yes (`tp -1784 -840 660`) | 10 juggernauts (1–3 alive) | — | — | ok | **Completes** (Regular, god, `autoaim` lobbing the M79): 4:13.95, 10/10; needed M79 duds to strike AI (`MOD_IMPACT`); `context/runs/so_juggernauts_favela/run.cmds` |
| so_killspree_favela | favela | yes (`tp -1808 -1384 700`) | 30 kills · waves refill | — | — | ok, enemies in view | **Completes** (Regular, god, `autoaim`): 2:43.05, 30 kills, 0 civilians; `context/runs/so_killspree_favela/run.cmds` |
| so_killspree_invasion | invasion | yes (`tp 472 -4328 2312`, edge of the ring trigger) | 30 kills · BTRs fire | — | — | ok | **Completes** (Regular, god, `autoaim`): 1:19.65, 300 points; BTRs fire `btr80_turret`; `context/runs/so_killspree_invasion/run.cmds` |
| so_killspree_trainer | trainer | yes | targets | 271 | 0.7 / 128 | ok | **Completes** (scripted run 2:24, 24/24); radio music aliases missing |
| so_rooftop_contingency | contingency | yes (timer) | 61 kills over 3 waves | — | — | ok | **Completes** (Air, Regular, god, `autoaim` with the Barrett): 3:22.50, waves 1–3 wiped; UAV/predator pickup not used; `context/runs/so_rooftop_contingency/run.cmds` |
| so_sabotage_cliffhanger | cliffhanger | yes | 16 (16) · stealth | 17 | 2.3 / 290 | ok | Prone player unseen by patrols at 68–111 (stealth); the SO deletes the snowmobiles (no snowmobile ending); truck riders `tag_guy*` |
| so_showers_gulag | gulag | yes | 16 (16) · 16 · 2 | 0 | 4.0 / 474 | ok, breach in slow-mo | First breach (showers) runs in slow-mo; play-through; worst GSC frame 474 ms |
| so_snowrace1_cliffhanger | cliffhanger | yes (timer) | bikes + riders | 11575 → 4 | 3.0 / 333 | ok, timer + stars | **Completes** with `IW4L_VEH_AUTODRIVE=1`: 0:58.60 (3-star time), no god; drivable snowmobile, final jump |
| so_snowrace2_cliffhanger | cliffhanger | yes (timer) | 1 | 1 | 1.7 / 342 | ok, Mission Success | **Completes** with `IW4L_VEH_AUTODRIVE=1`: 1:25.30, 26/29 gates, deterministic |
| so_takeover_estate | estate | yes | 25 (25) · 7 · 1 | 0 | 1.6 / 95 | ok | Play-through (this Spec Op deletes the estate breaches: `so_delete_breach_ents`) |
| so_takeover_oilrig | oilrig | yes (`tp 1064 420 -1900`) | 15 juggernauts (1–3 alive) | — | — | ok | **Completes** (Air, Regular, god, `autoaim` with the M240): 3:08.50, 15/15; one earlier run stalled at 13/15 with a juggernaut frozen in `move` on a full path (not reproduced; the actor status line now prints `animmode`/`arrival`/`detour`); `context/runs/so_takeover_oilrig/run.cmds` |

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

**Objective missions (2026-10-03, scripted runs in `context/runs/<mission>/`):** crossing, escape_airport, intel_boneyard,
defuse_favela_escape, demo_so_bridge and download_arcadia complete with god and teleport hops; the mission logic (start/end
triggers, use triggers and bars, destructible `"exploded"`, download timers, Stryker extraction) runs for real. The default
(non-custom) EOG summary shows no rows for these (only `ui_eog_custom` missions such as demo/Pit print rows).

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

**Wave/defense group (2026-10-03, scripted runs, `context/runs/<mission>/run.cmds`):** 5 of 6 complete under `god` with
the dev-only `autoaim` console helper (`crates/console/src/dev_aim.rs`: aims at the nearest hostile with a clear shot,
lobs grenade-launcher rounds, pulses `+attack`, skips targets that take no damage, teleports to a path node that sees one;
`enemies` lists live hostiles): killspree_favela 2:43.05, killspree_invasion 1:19.65, juggernauts_favela 4:13.95,
takeover_oilrig 3:08.50, rooftop_contingency 3:22.50. defense_invasion reaches wave 3 (helicopter, BTRs, hellfire open).
