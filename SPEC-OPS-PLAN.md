# IW4L Spec Ops plan

Goal: add MW2's **Spec Ops** to [IW4L](https://github.com/vladtrc/iw4L), the from-scratch Rust rewrite of MW2, *properly*: the original `so_*` missions running their own GSC scripts on a real single-player actor (AI) system. Not the shortcut of using IW4L's MP bots as enemies on MP maps with hand-written missions.

Status: implementation started 2026-10-02 on branch `spec-ops` of the fork `baileywickham/iw4L` (upstream `vladtrc/iw4L` is the `upstream` remote; no plan to merge back). Builds and runs on macOS arm64 (M4 Pro, Metal).

## Base to build on

- **Upstream `vladtrc/iw4L`**, not the mashup fork `chasmlol/2010-rust-rewrite-mashup` (that fork adds Skate 3 + a Minecraft map and nothing Spec Ops needs). Upstream is active: last commit 2026-10-02, 62 crates.
- Apache-2.0, written almost entirely by LLM agents; repo carries `AGENT.md`, `CONTEXT.md` (agent workflow) and short `docs/*.md` per area — start at `docs/INDEX.md`.
- What IW4L is: an engine reimplementation that loads your own MW2 install (fastfiles, D3D9 SM3 shaders → WGSL, collision, GSC scripts) on Bevy + wgpu. Not a decompilation; own architecture, MW2 data and script contract. MP only today (dm, war, dom, sd, sab, ctf, koth, dd). Linux, macOS, Windows.

## What already carries over

- **Fastfile loading.** `crates/fastfile_iw4/src/load/world.rs` already walks `GameWorldSp` (SP path nodes, path links, vehicle tracks) and `AddonMapEnts` (comment names `so_*` zones) — but only to advance the stream; the data is thrown away.
- **GSC compiler + VM** (`crates/sim/src/script/`): compiles GSC source from RawFiles, full language (threads, waits, notify/endon, arrays, structs). Realm-agnostic.
- Renderer, weapons (`weapon_iw4`), movement (`movement_iw4`), damage, FX, HUD, audio, killcam, demos, console.
- Netcode: one `TickInput → sim::step → Snapshot` funnel for host, prediction and replay (`docs/SIM-STEP.md`). Two-player co-op is well within it.
- Bots (`crates/bots`): own utility AI over a ClipMap-baked nav graph. Useful as reference for perception budgets and routing, **not** reusable as SP actors — they emit player `UserCmd`s, SP scripts drive actors through engine methods.

## What's missing

1. **SP data and zone discovery.** Discovery assumes MP: `mp_` prefixes and `common_mp` in `crates/asset_transport/src/discover.rs`. SP needs its own common/code zones and the `so_*` mission zones. SP data is a separate Steam app (SP is 10180 — verify; IW4L's docs only install MP, 10190).
2. **SP builtin catalog.** `crates/sim/src/script/profile/iw4_catalog.rs` is generated from the MP binary: 719 builtins, owners `Script/Player/Entity/HudElem/ScriptMover/PlayerCommand/Helicopter/Vehicle` — **no Actor**. About 490 natives are bound today. An unbound native fails the map load, so every native a mission reaches must exist before it runs at all.
3. **Actor system — the bulk of the work.** In the IW engine much of AI lives in C: pathfinding over path nodes, goal/cover selection, sight and awareness, accuracy, grenade reactions, the AI animation tree. GSC (`animscripts/*`, `maps/_spawner`, `maps/_utility`) drives it through actor methods and fields. Comparable to or larger than `crates/sim` (~46k lines).
4. **SP script library at load.** `maps/_load`, `_spawner`, `_specialops`, `_vehicle`, `animscripts/*` — much bigger and more tangled than MP's `maps/mp/gametypes/*`. Startup entry is MP-specific (`crates/sim/src/script/profile/iw4_startup.rs`).
5. **Spec Ops mode itself.** Two-player co-op, last stand / revive, mission timer, star ratings, difficulty, the menu to pick missions; snowmobile and vehicle missions.
6. **No script mod folder.** Scripts come only from fastfiles (`crates/assets/src/script_sources.rs`); `FileSources` exists in `crates/sim/src/script/source.rs` but is unused. An override folder layered in `crates/session/src/match_apply.rs:935` would help development (stub or patch SP scripts while natives are missing). Game modes are a fixed Rust enum (`crates/gamemode_iw4/src/kind.rs:67`).

## Plan

| # | Milestone | Done when | Size |
|---|---|---|---|
| 0 | Setup | Upstream builds and runs `make map mp_boneyard`; SP app installed under `IW4L_GAMES` | small |
| 1 | SP zones load as walk-around levels | An `so_*` map renders and you can walk it, no scripts | small–medium |
| 2 | SP catalog + script load | Catalog generated for SP; a mission's GSC compiles and installs; inventory of natives each mission reaches | medium |
| 3 | First mission with no actors | One AI-free mission (The Pit's pop-up targets are script models) plays start to finish | medium |
| 4 | Actor core | Path-node graph kept from `GameWorldSp`; actor entity, goals, nav, perception, shooting, animtree; `animscripts` run | **very large** |
| 5 | Spec Ops co-op | Two players, last stand/revive, timer, stars, difficulty, mission select | medium |
| 6 | Breadth | Vehicles/snowmobile, remaining missions, mission-by-mission fixes | large |

Order is chosen so each step is playable and testable on its own; step 4 is where most of the time goes, so get 1–3 solid first to find out what the actor API really has to cover.

## First concrete steps

- Clone upstream, build, run an MP map.
- Install MW2 SP data; dump the asset list of one `so_*` zone with IW4L's own fastfile loader.
- Grep the SP GSC (from the zones) for every builtin call; diff against the MP catalog to size steps 2 and 4.
- Add the script override folder (small, useful from day one).

## Open questions

- ~~Fork or upstream?~~ Fork; not merging back (decided 2026-10-02). Upstream's `CONTEXT.md` rules (no `#[test]` outside `approved_tests`, minimal comments) are followed where cheap, not enforced.
- How the MP catalog was generated (it says `@generated`; the generator is not in the tree) — needed for an SP catalog.
- Can SP and MP content mix in one process, or is SP its own runtime role?

## References

- IW4L docs: `docs/ENTITIES.md`, `docs/SIM-STEP.md`, `docs/GSC-RUNTIME.md`, `docs/BOTS.md`, `docs/MAP-LOAD.md`.
- Projects IW4L credits as references: OpenAssetTools (asset layouts, incl. `GameWorldSp`), IW4x (MW2 client), KisakCOD (CoD4 reimplementation; check whether it covers SP actors — CoD4 is the same engine family one generation back).

## Implementation plan (detailed)

Written 2026-10-02 after a code read of the three areas the early milestones touch. File references are to upstream `dc1ea70`.

### Track A — data-free work (doable now, verified by `cargo build`/`clippy` only)

**A1. Script override folder** (milestone 0–2 enabler)
- `crates/session/src/match_apply.rs` ~979: the local `Sources: SourceResolver` wraps `ScriptSources`; give it an `Option<FileSources>` built from `IW4L_SCRIPT_OVERRIDE`. Folder wins on `read`/`read_bytes`; `origin` reports `External` for overridden modules (keeps the existing purity / player-data guards).
- Route the gametype probe (~1000) and `Iw4Startup::new` through that resolver, not `sources.0`. Hook here, not in `match_walk.rs`, so the resident-map cache doesn't serve stale files.

**A2. Stub-natives dev mode** (`IW4L_GSC_STUB_NATIVES=1`)
- `crates/sim/src/script/runtime/lifecycle.rs:75-98`: instead of refusing with "N unbound natives", bind missing slots to one shared stub that returns `Err("unbound native (stub)")`; the VM already prefixes the name and logs once per call site. Report `gsc: stubbed n=… names=…` at install. Count hits in `Runtime.unsupported`.
- Flag threaded from `match_apply.rs` through `SimWorld::install_gsc_program` (`carrier.rs:152`).
- Optional: under the same flag, unknown non-catalog names compile to the stub (`compiler.rs:490`) so SP-only builtins don't stop compilation before the SP catalog exists.

**A3. SP realm, catalog scaffold, startup, SpecOps mode**
- `catalog.rs`: `Owner::Actor`, `Realm::Iw4Sp`, `Catalog::iw4sp()`. Catalog content starts as the MP list; the real SP list is generated in B2.
- `profile/iw4sp_startup.rs`: roots `codescripts/delete`, `codescripts/struct`, `maps/<map>`; entry `maps/<map>::main` (SP `main` calls `maps/_load::main` itself; no gametype step).
- `gamemode_iw4/src/kind.rs`: `SpecOps = 8` (`parse "so"|"specops"`, `script_tokens &[]`, team/co-op), `HostGameModeSelection::ALL` → 9 (`sim/src/spawn.rs:90`), the limit match (`match_apply.rs:1634`), HUD matches (`hud/{scorebar,targetmap,killcam_skip,menus/host}.rs`), wire tag 8 (`net/transport/meta_wire.rs`).
- `match_apply.rs:1035-1056`: SpecOps (or any `so_*` zone) selects `Catalog::iw4sp()` + `Iw4SpStartup`, skips the `maps/mp/gametypes/` probe and MP-only setup (`mp/stats_init.cfg`, objective visuals ~900).

**A4. SP zone discovery and a scriptless so_ load** (milestone 1)
- `asset_transport/src/discover.rs:405-477`: don't alias `so_*`/SP common zones to `mp_*`; `list_so_maps` beside `list_mp_map_packs` (628); realm-aware runtime-common lookup, falling back across all game trees when the map's own tree has no `common_mp` (SP may be a separate Steam folder).
- Feed so_ maps into console completion (`console/src/plugin/mod.rs:754`) and the menu list (`menu_load.rs:179`).
- Spawns: SP maps use `info_player_start`; fall back to it in `dm_spawn_points` (`asset_world/src/map_entities.rs:340`).
- First cut keeps MP `common_mp`/`patch_mp` as runtime common (techsets, weapons); SP `common`/`code_post_gfx` are added once real zone names are confirmed from an install.

**A5. Keep GameWorldSp path data** (milestone 4 groundwork)
- **Fix suspected x86 offsets first** in `fastfile_iw4/src/load/world.rs:229-361`: x86 `PathData` appears to skip `chainNodeCount` (expected x86: chainNodeForNode 16, nodeForChainNode 20, pathVis 28, nodeTree 36) and the SP glass pointer should be at 52, not 48. Verify against OpenAssetTools `IW4_Assets.h` (clone to `context/externals/`).
- Record `PathDataGeometry` / `VehicleTrackGeometry` in `ZoneStream` (`zone.rs`), plus the `AddonMapEnts` entity string and SP glass (both thrown away today).
- Owned `asset_world::path_data::{PathData, PathNode, PathLink}` built in `assets/src/lane/iw4.rs` beside `build_clip_collision` (~296) before arenas are freed (~409); carried on `PreparedWorld`.
- `sim`: `SimPathGraph` via `SimContentBuilder::set_path_graph`; converted in `match_apply.rs` near `install_clip_and_player` (~1456).
- GSC: real `getnode` / `getnodearray` / `getallnodes` (stubs today at `natives/t5.rs:787`).

### Track B — needs MW2 data (blocked)

Install with `steamcmd +@sSteamCmdForcePlatformType windows +force_install_dir ~/Games/MW2 +login <user> +app_update 10190 +app_update 10180 +quit` (verify 10180 is SP), `IW4L_GAMES=~/Games`. ~29 GB free on the dev Mac — tight.

- **B1.** `make map mp_boneyard` on the Mac (milestone 0). Then `make map so_…`; fix whatever stops the walk: asset types without an x86 loader (`SndDriverGlobals`, `UiMap`, `AiType`, `Character`, `XModelAlias` → `NoAssetLoader`, `fastfile_iw4/src/load/mod.rs:123-159`), SP techsets, nothing drawable (`match_apply.rs:1131`). Confirm path data counts with a load-report line.
- **B2.** Dump every `.gsc` from SP zones; grep builtin calls; generate `iw4sp_catalog.rs` (no generator exists upstream — write one in `xtask`), diff vs MP → sizing for milestones 2 and 4.
- **B3.** First AI-free mission (The Pit) with `IW4L_GSC_STUB_NATIVES=1` + overrides; bind natives until it plays (milestone 3).
- **B4.** Actor core (milestone 4): `EntityKind::Actor`, `host/actors.rs` field branch before the generic map (`runtime/mod.rs:911-980`), path-graph A*, goal/cover, perception, shooting, animtree; run `animscripts`.
- **B5/B6.** Co-op mode, then breadth (milestones 5–6).

## Progress log

**2026-10-02**
- Game data: Windows depots fetched through the Steam client console (`download_depot 10180 10181/10182/10183`, `10190 10184`) and merged into `~/Games/MW2` (12 GB). Depots 10186/10196 report "no license" and are not needed: 10183 (`zone/english`) carries MP, SP and all 23 `so_*` zones. Not `steamcmd`: the Homebrew cask is broken on this macOS; Valve's standalone tarball works but needs its own login.
- Milestone 0 done: `map mp_boneyard` spawns and renders on the Mac.
- macOS gotcha: an occluded window or locked screen gets no drawable, so world spawn's GPU gate waits forever (looked like a hang; upstream too). Now logged as `window surface: no frame to draw`; `IW4L_WINDOW_ON_TOP=1` keeps the window presented during scripted runs.
- SP base maps walk with the existing IW4 lane. Bare names alias to `mp_` first (`favela` → `mp_favela`); `iw4:favela` now means the exact SP zone. Path data verified on real zones: trainer 94 nodes / 534 links, favela 2270, cliffhanger 2866 (+1 vehicle track, 41 sectors), estate 3041, so_bridge 720.
- **Spec Ops zones are add-ons.** `so_<mission>_<basemap>` holds no world: `so_killspree_trainer` (7 MB) = Sound 388, Localize 33, TechniqueSet 25, MenuList 5, Material 4, RawFile 3, XModel 2, AddonMapEnts 1, on top of `trainer.ff` (117 MB). A mission load is base map + mission overlay.
- Known gaps: SP `estate` produces no GfxWorld draw product; running MP gametype scripts on an SP map never spawns the player (not the target path — SO uses its own startup and spawn).
- SP builtin catalog generated from `iw4sp.exe` by `scripts/gen-iw4sp-catalog.py` (965 builtins; 125 Actor + 9 Sentient methods); validated against upstream's MP catalog (578 shared names, no namespace or developer-flag disagreements).
- All 15 SO base maps walk headlessly with world + path nodes (estate needed inverted smodel bounds normalized).
- **Milestone 1 done:** `map so_killspree_trainer` loads The Pit (base `trainer` + mission zone + SP `common.ff`; script order common_mp < SP common < base map < mission), forces SpecOps mode, spawns the player at `info_player_start_so` with an M4. Under `IW4L_GSC_STUB_NATIVES=1`: 238 stubbed natives, 280 runtime errors (level.player missing before main, `_vehicle`, `setsaveddvar`, anim natives).
- Actor core design: `docs/spec-ops/ACTOR-DESIGN.md`.
- In progress: GSC animation API; milestone 3 (The Pit playable).
- **SP weapons from SP zones.** A Spec Ops load captures `Weapon` defs from SP `common.ff` (15), the base map (trainer 43, so_bridge 53) and the mission zone (so_demo_so_bridge 15), with their world models, view models/clips (map walk, or the script-zone walk for common/mission) and mission materials. `WeaponBuild::absorb_overriding` folds them in common < map < mission over the MP table; same-name rows (`usp`, `claymore`, `rpg`) replace the MP row in place, and SP rows are named in script without `_mp`. Referenced names (`,viewmodel_m4`) lose the comma. The Pit's loadout (`m4_grunt`, `usp`) and so_demo's (`rpg_player`, `c4`, `claymore`, …) come from real defs; no SP def refused combat facts. The SpecOps engine default loadout is now given only when no SP weapons were loaded; the MP family stand-in remains for missing names and logs `standing in`. Gaps: SP common materials not merged; combat/projectile FX of common/mission weapons not stamped (map weapons are); map `weapon_*` pickup entities are not spawned.

**2026-10-02 (evening, user away)**
- Live runs moved to the spare M2 Air via `~/bin/iw4l-air` (sync → build → run → log/screenshots back). Its SSH auth goes through 1Password, so it's unreachable while 1Password is locked; runs then happen on the main Mac.
- Animation: SP clips bind (all leaves present in SP common + base map + mission); actor idle anims send "end". Script anim trees reach snapshots, client pose and bullet collision.
- **Actors S1–S2:** spawners/aitypes/animscripts run; field tables extracted from iw4sp.exe (`scripts/gen-iw4sp-fields.py`); goals, node claims, budgeted A* over path nodes, kinematic movement, move/stop animscript switch, `cansee`. On `so_killspree_favela` enemies path 12–20 nodes toward the player and close in. SP entity cap 4096 (3 missions that failed install now run).
- **Rendering:** Pit target boards draw (per-pose constant cache key), actors draw (empty-tag head attach merges skeletons). Open: SP first-person weapons render dark (eye lighting falls back to a spot light region).
- **SP weapons** load from SP common/base map/mission zones; mission scripts give the loadout (The Pit: m4_grunt + usp; magazine empties on fire).
- **Co-op (milestone 5 core):** two players via lobby + local master, both SO starts, `level.players.size == 2`, loadouts, seeing each other, down → revive, both down → mission failed. Dev envs `IW4L_WINDOW_POS`, `DUO_*`.
- Commits after 58866cb are staged, waiting for the user's signing approval.
- In progress: actors S3 (see/shoot/die, `../iw4l-s3`), full successful Pit run (`../iw4l-pit`).

**Actors S4 (cover, grenades, pain, suppression)** — `so_killspree_favela`, local runs
- Cover: `script/host/actor_cover.rs`. The code goal of an actor with an enemy is its claimed cover node (`Actor_FindClaimedNode`: keep a valid claimed node in the goal, else the best cover in the goal cylinder scored like the engine — distance, engagement, path-vis bit to the enemy's nearest node, node angle, target direction, priority; `ai_coverScore_*` dvars). Near the node the state runs the node's script (`cover_left/right/stand/crouch/prone`, conceal nodes map to cover_*). `"cover_approach"` → `startcoverarrival` → `cover_arrival` slides into the node (no root motion). Bound: `findbestcovernode`, `findcovernode`/`getcovernode`, `usecovernode`, `iscovervalidagainstenemy`, `nearclaimnode(andangle)`, `gethighestnodestance`, `doesnodeallowstance`, `getvalidcoverpeekouts` (empty), `checkcoverexitposwithpath`, `setruntopos`/`"runto_arrived"`, `safeteleport`, `comparenodedirtopathdir`, `reacquirestep`, `findreacquiredirect/proximatepath` + `reacquiremove`, `bulletspread`.
- Grenades: `script/host/actor_grenade.rs`. `checkgrenadethrow(pos)` solves min-energy / min-time / max-time arcs (900 u/s cap, gravity 800) and sweeps them with traces; `throwgrenade`, `magicgrenade(manual)`, `pickupgrenade`. Projectiles carry `owner_entity` (actor) with `owner = NO_OWNER`; their blast goes through `ScriptBlast` with `Attacker::Entity` (SP player damage path, actors). Live grenades near an actor: return throw / flee / `grenade_cower`. Spec Ops never leaves warmup, so entity thinks (missiles, movers) now also run in SpecOps warmup (`step.rs`) — before this no projectile in SO ever moved.
- Pain was never observed only because the player never hurt an actor without killing it; with damage it runs (`actor: … pain N` log). Suppression: bullet lines from `phase_trace` feed `suppressionmeter`; at cover `"suppression"`/`suppression_end`, elsewhere `"bulletwhizby"`; `issuppressed`, `issuppressionwaiting`.
- Actor shots now allocate a shot id and push the fire event (sound); muzzle flash needs client bolts for actor bodies (render side, not done). `dropweapon` and `spawn("weapon_<name>")` create pickup items.
- Evidence (60 s, god): 6–8 `startcoverarrival`, cover_left/right/crouch scripts, 3 grenades thrown → 3 explosions → player `MOD_GRENADE_SPLASH` 149–178 (no god: health 100→53), flee response, 10+ pains, suppression at cover, reacquire step, weapons dropped on death. Stubbed natives 130→111, stub calls 307→93 (`bulletspread` 214 gone), `tryrunningtoenemy` cast error gone. MP `mp_boneyard` + 3 bots unchanged.
- Gaps: AI accuracy graphs are not loaded (curve kept); no grenade return throw observed yet; `findshufflecovernode` returns none; `ismovesuppressed` always 0; `moveAnim` set missing once per run in `run.gsc`.

**Mission select, difficulty, stars (milestone 5)**
- Main menu → SPECIAL OPS (`crates/ui/menus/specops.json`, `console/src/specops_menu.rs`): tiers and order from SP `common.ff` `sp/specOpsTable.csv` (profile index, tier, unlocks 0/4/8/20/40, "picks difficulty" column) hard-coded in `ui/src/frontend/specops.rs`; names and descriptions from SP `common.ff` localize (loaded off-frame on first open, ~450 ms). Rows show stars and best time; locked tiers are marked (GSC `can_save_to_profile` refuses stars there). No preview: the `levelshot_so_*` images are streamed and not in any installed IWD.
- Difficulty popup → `g_gameskill` 1/2/3 as a host rule dvar (`is_rule`), read by `getplayersetting("gameskill")`. Pit evidence: Veteran → player `attackeraccuracy=1.15 damagemultiplier=0.49 deathinvulnerabletime=800`; Regular → `1 / 0.29 / 3000` (log `spec ops: player … gameskill=`). Solo → `request_zone` + mode `so`; Host co-op → the normal lobby with `ui_mapname=iw4:so_…`, `ui_gametype=so`.
- Profile natives now write `sim::SpProfile`, persisted by the console beside `profile.cfg` as `specops.cfg`. Two menu launches of the scripted Pit: run 1 (Veteran) wrote `s22=147950`, `missionsohighestdifficulty` digit 22 = 2 (1 star), saved; run 2 loaded 3 fields and the menu showed `*--  2:27.95`.
- SP `playername` player field (was undefined → `coop_eog_summary` died at `_endmission:855`); the summary now runs through (`player.stats` was defined in these runs).
**Mission sweep (2026-10-03)** — status table in `docs/spec-ops/MISSIONS.md`
- All 23 missions load, spawn with their loadout and run 90 s without hanging; 21 start solo, enemies engage in 9, The Pit completes.
- SP vehicles: map `script_vehicle_*` with spawnflags 2 are spawners (`isspawner`), `vehicle_dospawn()` copies the spawner's keys (not its targetname); placed vehicles are live at load; ground paths run node to node without stopping, take node `speed`, notify `reached_end_node`; SP vehicle damage is applied by the engine. `script_vehicle_collmap` is no vehicle.
- Scripts: `animscripted`/`startscriptedanim` (actors via `animscripts/scripted`), `animcustom`, `getstartorigin`/`angles`, SP `playsound( alias, notify )` notifies after 2.5 s, ~80 presentation/save/badplace natives bound as no-ops, map `weapon_*` become pickups, `player.playername`.
- VM: a failed `waittill` kills its thread, and a thread is killed after 1000 errors in one run (snowrace stalled at tick 4 on error loops).
- `IW4L_TRIGGER_LOG=1` logs named triggers' bounds and hull centres (start triggers are often L-shaped lines; the `origin` key is not inside them).

**Stealth (S5) and slow-mo breach (2026-10-03)**
- Sight: `cansee`/perception clamp to the target sentient's `maxvisibledist` (the `_stealth` scripts write it per stance, movement and shadow every frame), `fovcosinebusy` while a scripted anim holds the actor, at least a 90° cone for the current/favorite enemy, corpses seen at their middle. Logs `actor: … acquired enemy … maxvisibledist=… <stance>` and `alertlevel a -> b` (`alertlevelint` derived).
- AI events (`script/host/actor_events.rs`, after KisakCOD `actor_events.cpp`): gunshot / silenced shot (weapon name contains "silence") / gunshot_teammate, bullet lines, explosions (projectiles and `radiusdamage`), grenade pings, player footsteps (run/walk/sprint, none prone), pain and death reach actors within the `ai_eventDist*` dvars (per-actor `footstepdetectdist*`); hostile ones teach the actor where the originator is, so the next look picks it as enemy (`"enemy"`). `addaieventlistener`/`removeaieventlistener` deliver `"ai_event"` (name, originator, position); explosions also notify `"explode"`. New-enemy sharing uses `ai_eventDistNewEnemy`. `dvar_float` now reads the lowercased dvar keys (stealth `setsaveddvar` values were ignored before).
- so_hidden: prone player unseen by patrols passing at 157–600 u; standing spotted at 264–659 (`maxvisibledist` 1500), squad goes alert → combat; an unsilenced shot while prone is heard by all four (`heard gunshot … learned ai_event`) and they attack; a silenced kill alerts only the squadmates within `ai_eventDistDeath` 512.
- Breach: `sethintstring` on a `trigger_use` no longer disables it (every hinted use trigger was dead), `playerlinktoblend` (snaps to the tag, no ease), `allowmelee` (new `melee_disabled` control, on the wire), `enable/disablebreaching` as presentation no-ops. so_assault_oilrig: use → charge → `breaching_on` (challenge timer) → `setslowmotion 1.00 -> 0.25 over 500 ms`, kills in slow-mo, `0.25 -> 1.00 over 750 ms`; hostages executed if the player does not fire (mission failed, as in the game). Slow-mo is the existing virtual-time scale (sim keeps its 50 ms tick): the same scripted run takes 36.5 s wall with the breach vs 26.4 s without (3.5 s sim at 0.25×). Gulag showers breach runs; estate's Spec Op deletes its breaches.
- Regressions: The Pit completes (2:24.80, 24/24); `mp_boneyard` + 3 bots InGame, same 2 known "unavailable" errors.
- Open: ghillie enemies and corpse discovery not observed in a run; `ai_busyEvent*` ignored; `playerlinktoblend` does not ease; animmode errors in sabotage come from vehicle riders (`tag_guy*`).

**Vehicles (2026-10-03)** — blockers 2 and 3 in `docs/spec-ops/MISSIONS.md`
- Player vehicles: `mountvehicle` links the player (delta view) and notifies `vehicle_mount`; `dismountvehicle` → `vehicle_dismount`. The driver's move input (forward = throttle, back = brake/reverse, strafe = steer; frozen controls give none) is taken in `step.rs` before the link zeroes it. `script/host/vehicle_drive.rs` drives it: 4 substeps of slope gravity (overspeed to 1.5× `veh_topspeed` downhill), steering, box sweeps that slide on walls, ground snap only down to the free-fall path (crests and ramps launch it), `veh_leftground`/`veh_landed`, head-on hits ≥ 900 u/s → `veh_collision` (the script's crash/death). `veh_speed`/`veh_throttle` fields, `vehphys_setspeed`, `vehicle_getthrottle`/`getsteering`.
- `IW4L_VEH_AUTODRIVE=1` (test driver; `=2` logs decisions) steers at `player.targ.next_node.midpoint` (or the next `flag_trigger` gate) with probe avoidance. **so_snowrace1 completes, 0:58.60 (3-star time), with and without god**, deterministic over 4 runs; snowrace2 runs the course and the final jump but its gate timer runs out.
- SP vehicle data: map and mission zones now capture VehicleDefs (BTR fires `btr80_turret` in killspree_invasion; `fireweapon: vehicle has no weapon` ×280 gone); a vehicle spawned this frame gets its model before a tag query (`tag_driver` riders link; snowrace rider errors 125 → 0); bone-only skeletons of SP common + mission XModels the scripts name (36 in snowrace) feed the sim's model library (`tread` tag errors 1230 → 0); tag names match case-insensitively (`j_SpineUpper`); `setvehicleteam`. Snowrace1 errors 11 575 → 17.
- Open: first-person snowmobile/hands models are not drawn; AI bikes follow `vehicledriveto` in straight lines; `misc_turret` sentry natives (`_sentry`); rider runs in so_bridge/forest not re-measured. `mp_boneyard` spawn 0 → InGame unchanged.

**Single-player blocker pass (2026-10-03, Air runs)** — rows in `docs/spec-ops/MISSIONS.md`
- Snowrace2 gate timing was three things, none in the timer: thin gate triggers (18 u) stepped over at ~60 u/tick (player touch triggers are now swept from the last pass's position, ≤ 512 u, `triggers.rs`); the autopilot aimed at the finish trigger's centre (off the track; now the nearest point of it) and capped speed at 70 mph for a gate dead ahead at the final ramp; and a one-substep flat at the ramp lip cancelled the launch (driven vehicles remember a ramp climb ≥ 400 u/s, decaying 0.6 a substep). **so_snowrace2 completes 1:25.30, 26/29 gates**, deterministic; snowrace1 still 0:58.85.
- Sentries: map `misc_turret`s become turrets at load (weapon from `weaponinfo`, a presence so they draw and can be shot); SP acquisition goes by sentient team over players, actors and sentients; SP shots go through the combat pipeline as an entity attacker; `maketurretinoperable` no longer stops a `sentry`-mode turret (it only blocks player use). Arcadia: `_sentry` errors 11 → 0, the axis minigun targets and hits the player.
- Actors follow `animscripted` root motion (`MotionPath::Anim`, stopped by `stopanimscripted`): so_bridge rappellers hung 385 u up at the anim's first frame; now they land, `over_solid_ground` fires and they engage (8/8 after a shot).
- `vehicledriveto` vehicles ride the ground between goals (trace down, pitch from the slope). The enemy bikes already follow `_vehicle_spline` (a goal 200 u ahead every 0.1 s); they fall behind the 150 mph autopilot and get wiped out as "left behind", as scripted.
- `movemode` never reads `stop` to the move script (it picked an undefined move anim set: `moveanim`/`setmovenonforwardanims` errors in every mission with movers). `info_player_start_pmc` is a Spec Ops start (takeover_oilrig spawned at the SP start 21 k u away).
- Regressions: The Pit completes (2:28.15, 24/24), snowrace1 completes, `mp_boneyard` spawn 0 → InGame.

**Objective missions scripted to success (2026-10-03)** — rows in `docs/spec-ops/MISSIONS.md`, scripts in `context/runs/<mission>/` (worktree scratch)
- `gen.py` per mission turns the base map + mission entity strings (`context/runs/ents.py`) into a `--cmds` run: teleport hops over path nodes (Dijkstra over `node_*`), the mission's own triggers, use triggers, timers and destructibles; `god` on.
- Complete: crossing 0:27.80, escape_airport 0:49.25, intel_boneyard 1:08.15 (both laptops tried; `_pmc` picks one at random), defuse_favela_escape 1:01.90 (briefcase use → link, defuse weapon, 4.5 s use bar), demo_so_bridge 5:05.70 (36/36 cars, `give ammo` between RPG shots).
- SP rockets flew 23° up: `fire_missile` added the def's `projectileSpeedUp` (rpg_player 500) to a linear rocket; IW4 `G_FireRocket` launches along the aim only, so SP ignores it (MP/T5 unchanged).
- Radius damage reaches an entity at its own origin when that is nearer than its linked collision brush (so_bridge slide cars carry their slide clip ~500 u away; rockets on them did nothing).
- `IW4L_AUTOFIRE=1` test aimer (`script/host/autoaim.rs`, `=2` logs targets, `=3` a census of hostiles within 600 u): turns the player's command at the nearest hostile actor in clear sight and fires every other tick; shots go through the normal weapon path. `IW4L_GSC_TRACE_NOTIFY=a,b` logs those notifies on any entity/struct (download progress lives on structs).
- SP path vehicles (arcadia's Stryker): `attachpath` puts the vehicle on the node (it spawned 13 k u away and spent 70 s driving to its path), a script speed of 0 holds at the next node (it used to restart at the 20 mph default), and `veh_pathdir = "reverse"` follows the path back through the nodes that target the current one (the extraction backs down the street to `vnode_house1`).
- so_download_arcadia: with `IW4L_AUTOFIRE=1` and a patrol around each laptop, the mission completes (Air, 7:45.70: three downloads, then the Stryker backs up to the extraction point); the defenders must all die or stay > 256 u away for 60 s, and the interior guards are behind walls, so standing at the laptop is not enough.
- `~/bin/iw4l-air` expanded `IW4L_AIR_TIMEOUT` on the Air (always 300 s); it now expands locally.

**Wave and defense missions (2026-10-03)** — rows in `docs/spec-ops/MISSIONS.md`, scripts in `context/runs/` (worktree scratch)
- Dev-only `autoaim on [fire] [head] [vehicles] [hunt <s>] [range <u>]` and `enemies` console verbs (`console/src/dev_aim.rs`, queries `SimWorld::dev_aim_targets`/`dev_hunt_spot`/`dev_projectile_lob` in `sim/src/script/host/dev_aim.rs`): the local player's view tracks the nearest hostile sentient with a clear shot (bullet mask, its own body ignored, chest tag), grenade launchers lob, `+attack` pulses, a target that keeps its health 6 s is skipped, and with nothing in sight the player teleports to a path node that sees one. Missions themselves run unchanged.
- **Complete (god):** killspree_favela 2:43.05 (30 kills), killspree_invasion 1:19.65 (300 points), juggernauts_favela 4:13.95 (10), takeover_oilrig 3:08.50 (15), rooftop_contingency 3:22.50 (3 waves, 61 kills). defense_invasion reaches wave 3.
- Engine: a grenade-launcher dud that hits an AI deals its impact damage (`MOD_IMPACT`; it vanished before, so point-blank M79 shots did nothing); `Target_Set`/`Target_Remove`/`Target_IsTarget`/`Target_GetArray` keep a lock-target set (`_attack_heli` died at `Target_Set`); the actor status line prints `animmode`/`arrival`/`detour`.
- Open: SP helicopters take no bullet damage in defense_invasion; ownerless `MagicBullet` (UAV hellfire) is refused; one takeover run had a juggernaut frozen in `move` with a full path (not reproduced).
- Regressions (Air): The Pit completes (2:28.10, 24/24), snowrace1 completes (0:58.85), `mp_boneyard` spawn 0 → InGame.


**Stealth + breach group to success (2026-10-03)** — rows in `docs/spec-ops/MISSIONS.md`, scripts in `context/runs/`
- All six complete from scripted runs (god + teleports between the missions' own triggers; Regular): so_showers_gulag
  0:40.85, so_sabotage_cliffhanger 0:55.25, so_forest_contingency 1:55.55, so_hidden_so_ghillies 2:03.75,
  so_assault_oilrig 2:55.15 (both breaches, deck 2 rappellers and heli, `barracks_cleared`), so_takeover_estate 2:16.15
  (40/40 PMC kills, ADS). Air repeats: showers 0:41.85, sabotage 0:55.55, forest 1:56.25, hidden 2:04.45, oilrig 2:56.65
  (teleport waits are wall-clock, so times drift by ~1 s). Regressions on the Air: Pit 2:28.25, snowrace1 0:58.85, mp_boneyard InGame.
- Test aimer `IW4L_AUTOAIM` (`script/host/test_aim.rs`, one call in `step.rs` after `constrain_cmd`): while attack is held
  the view turns (through `delta_angles`) to the nearest hostile actor with a clear shot line; `=2` logs, `=3` also moves
  the player once a second to a path node that sees the nearest hostile when none is within 1500 in sight. A target aimed at 4 s
  without a hit is skipped for 20 s.
- Run findings: large `trigger_multiple_flag_set` volumes are rotated slabs — teleport to the logged hull centre, not
  into the bounds box (hidden's church/houses/barn/valley triggers never fired before); estate's enemies populate only
  after the `mission_start` ring round the PMC start is touched; `makeusable` script models (sabotage C4) work.
- Actor `tag_eye` when the body model has none (it is on the head, which the sim does not pose): `gettagangles` /
  `gettagorigin` answer with the actor's eye and facing (stealth corpse discovery errored on `body_opforce_arctic_*`;
  `gettagorigin` returned the feet for any missing tag).
- Dogs: an aitype's `animTree` (`dog.atr`) replaces `generic_human` after `aitype::main`; species `dog` runs
  `animscripts/dog/dog_<state>` (init/move/stop/combat/death/pain/flashed/scripted; other states → dog_combat), and the
  dog animscripts are startup roots when the zones carry them. Dog melee on the player is not done.
- Open: heli `mgturret` `startfiring` errors once in oilrig; dog melee (`meleebiteattackplayer` scenes).

**Defense invasion to success, dog melee (2026-10-04)** — rows in `docs/spec-ops/MISSIONS.md`, scripts in `context/runs/`
- **so_defense_invasion completes** (Air, Regular, god, 10:08.75 with the final helper; 23:57.80 before it preferred soldiers). The mi28 never took bullets because
  SP vehicles were created undamageable (`can_damage` false until a script opts in, as MP helicopter scripts do); SP vehicles
  now take damage from spawn (`G_VehSpawner`), so bullets wear down `bullet_armor` and RPGs kill the BTRs and helis.
- Map weapon pickups in Spec Ops: touch and `+activate` item phases ran only in `Playing` (SO stays in warmup). The run
  picks up the RPG beside the start; `autoaim … vehicles` now prefers soldiers, `weapnext`s to a carried rocket launcher
  for vehicles and back, aims at a model's bounds centre (heli bodies hang below the origin; dogs have no `j_spineupper`),
  skips `godmode` vehicles (the UAV) and hunts nodes that see vehicles. `enemies` prints each target's collision.
- Ownerless SP `MagicBullet` fires from the world (`ScriptModelId::WORLD`, attacker undefined): UAV hellfires kill hunters.
  MP still refuses it. `IW4L_HELI_LOG=1` adds each flying vehicle's collision summary.
- Dogs (`Actor_Dog_Exposed_Think`): with the enemy in its goal a dog paths to a melee spot `meleeattackdist` away and
  runs `dog_combat` once within reach (+15, +15 and a 0.25 s lead on a moving enemy), which stays while
  `safetochangescript` is false; dogs in `zonly_physics`/`nophysics` move by their anim's root motion (the lunge).
  Bound `melee` (`Actor_Melee`: 64 u strike, weapon melee damage + 0–4, `MOD_MELEE`, applied at once), `getnormalhealth`;
  `clearpitchorient`, HUD/viewmodel hide/show, `allowlean`, `freevehicle` as presentation no-ops. `face enemy` turns a
  standing actor to its enemy. An SP player with `setcandamage( false )` takes no damage.
- Forest evidence (un-godded): `actor: dog entity 612 in reach of its enemy, attacks`, `melee hits player 0 for 54
  (dog_bite)`, `client=0 damaged 15 (raw 54) means=MOD_MELEE … by actor_enemy_dog 612`, knock-down view with the
  `+melee` hint (`context/dogs/dog_knockdown.png`, charge in `dog_charge.png`), then the player dies; shooting first,
  `entity 612 died by player 0 means=MOD_HEAD_SHOT`.
- Open: knock-down viewhands not drawn (`hideviewmodel`/`showonclient` are no-ops), neck snap not exercised, root motion
  only for dogs.
- Regressions (Air): The Pit 2:28.50 (24/24), snowrace1 0:58.85, killspree_invasion 0:50.60, forest 1:56.30, `mp_boneyard`
  spawn 0 → InGame. MP vehicle damage untouched (SP-only spawn damage, SP-only ownerless `MagicBullet`).

**Presentation loose ends (2026-10-04)**
- EOG "Stars Earned!": the mission menus' images and tables now come from SP `common.ff` (`sp/specopstable.csv`,
  death icons) and SP `ui.ff` (`difficulty_star_*_hi_res`, IWD stubs decoded from `iw_*.iwd`), walked beside the
  script walk; the HUD adds them to its image set when it merges the mission menus. SP-only menu ops 167
  (`getprofiledata`-style: last argument names the profile field) and 173 (character at index) read the local
  `SpProfile`. Pit (Air, fresh profile): one gold star + two grey empties. No crosshair under `*eog_*` menus.
- AC-130 zoom: SP `setsaveddvar( "cg_playerFovScale<N>" | "cg_fovScale", f )` publishes client dvars
  `cg_playerfovscale` / `cg_fovscale` to player N (all players); the client lens multiplies them into its FOV
  (`view_kick.rs`). SP `cg_fov` is 65 on the server. Co-op (Air): 105/40/25 mm → scale 0.846/0.385/0.200.
- Laser designator: `laserForceOn/Off` set `eflags::LASER` (`0x4000_0000`) on the player; FPV and remote bodies lase
  from `tag_laser`, else the flash tag; SP common's `gfx_laser_light` is merged into the material pool. **Not visible
  yet** in the gunner's thermal view or first person (A/B screenshots identical) — the post-light path is unverified.
- Lobby / `getmapname`: Spec Ops zones show the mission name (`BODY COUNT`), not the zone stem.
- Solo `player.laststand` reads 0 in SP; the co-op per-frame `spec ops: player … gameskill=` log alternation is gone
  (last line kept per player). Scripted one-shot presses (`weapnext`, `+actionslot N`) in an unfocused window fire once.
- Tooling (worktree scratch): `context/coop/air_duo.sh` (both clients + local master on the Air),
  `context/coop/air_lobby.sh`, `context/pit_runs/air_stars.sh` (fresh profile).

**First-person vehicle and rig models (2026-10-04)**
- Snowrace: `_snowmobile_drive` sets the bike to `vehicle_snowmobile_co_op` and attaches `viewhands_player_arctic_wind_coop`
  at `tag_player`; both live in the mission zone with LODs that name the base map's surfaces (`,vehicle_snowmobile_lod310`,
  `,viewhands_arctic_wind10`). SP common/mission XModels now capture full meshes (bones-only fallback); cross-zone LODs are
  deferred (`PendingSharedLod`) and spliced from the map walk's surfaces; the models the scripts name join the render
  catalog with their materials (mission absorb map, SP common materials those models use).
- Driver: feet on the vehicle's `tag_player` (eye = rig camera, 60 u up), `LINK_FLAGS_VEHICLE_SEAT`, viewmodel hidden
  (`weap_flags::VIEWMODEL_HIDDEN`), links re-applied after the drive step (the view lagged a tick, 132 u at 150 mph); the
  client draws the driven bike from the player's origin and `link_weapon_angles` (the seat pose). A player's link parent
  (`viewlocked_ent_num`, `viewlocked` 0) draws at LOD 0 (`viewhands_player_*` LOD 0 ends at 60 u). Bars and hands steer.
- SP `hideviewmodel`/`showviewmodel` set that bit; `showonclient`/`hideonclient` are per-client visibility.
- Dog knock-down: the rig, visibility, LOD and hidden weapon are wired but not yet seen in a screenshot (Air unreachable).

**Actor root motion (2026-10-04)** — `script/host/actor_motion.rs`, `actor_nav.rs`, `anim.rs`
- Soldiers and dogs move by their animtree's root delta (`motion_delta`: every weighted leaf's span, weights
  normalized per blend node, additive layers skipped) through `step_slide_move` against the clip map (15×72,
  gravity 800, 18 step, fall rescue after 3 s). `normal`/`none`: the delta's length walks the path; anim-driven modes
  (`gravity`, `zonly_physics`, `nogravity`, `noclip`, `nophysics`, `angle deltas`) apply the delta turned by yaw plus the
  anim's yaw. The dog-only root motion in `advance_anims` is gone (deltas are stored per actor and applied in think).
- Cover arrival: the arrival anim's delta, with the miss between what it has left and the node spread over the time left
  (arrivals end 0–6 u from the node; a pain interrupts some). Exits/turns (`zonly_physics`) carry the actor; passed path points are skipped.
- Negotiation: at a link's begin node the actor runs `animscripts/traverse/<animscript>::main` (sticky until it ends;
  `traversemode`, `getnegotiation*node` from the live link, `forceteleport` keeps the path). Traverse scripts are startup
  roots; ones the zones ship unparsable (`stairs_up`, `stairs_down`) are dropped at load.
- Fallbacks, logged: `move` with no root delta for 4 ticks walks at 180/70 (once: `pistol_stand_switch`), hull blocked
  10 ticks slides through (≤5 per run, one favela choke at -2773 -131), stuck in solid lifts ≤18.
- Evidence (Air, `IW4L_ACTOR_MOTION_LOG=1` prints per-actor speed/anim once a second): `run_lowready_F` steady 181–183 u/s,
  `sprint1_loop` ~186, `run_n_gun_L/R` ~175, `Juggernaut_walkF` ~20; traversals stepup_52, step_up_12, jumpdown_40,
  wall_hop, window_2 completed by their anims (end-node miss 7–27 u; jumpdown_40 59 u → placed); no falls.
  Screens `context/rootmotion/favela_run_{a,b}.png`.
- Runs (Air): killspree_favela 1:19.90, crossing 0:28.00, rooftop 2:21.00, killspree_invasion 0:59.20, juggernauts 5:53.00
  (slower: juggernauts walk at their anim's ~20 u/s, not 70); Pit 2:28.10, snowrace1 0:58.85, mp_boneyard InGame.
- Open: juggernaut walk speed vs the real game unverified; jumpdown_40 overshoots/undershoots its end node; no
  `starttraversearrival`; dying actors' death anims do not move.

**Hitch pass (2026-10-04, `so-hitch`)**
- Measuring: `IW4L_GSC_STATS=1` logs a `gsc hitch:` line for every script tick over `IW4L_GSC_HITCH_MS` (default 50; wall
  or thread CPU): phases, slowest natives, resumed functions, instructions per script function. `IW4L_SIM_STATS=1` logs each
  step system's mean/worst, the authority tick's p50/p99 (wall and CPU) and named hot paths (settles, traces, anim, heap,
  materialize, shots, FX world traces). Thread CPU tells real work from a contended Air descheduling the game (many "hitches"
  in loaded runs were 100–400 ms wall at <25 ms CPU). Scripts: `context/hitch/` (worktree scratch).
- Causes: (1) the level-entry tick is legit script work, 1.9–7.8 M instructions (`_load`, createfx, destructibles; showers'
  `_global_fx::global_fx_create` walks `level.struct` per effect, 5.7 M), made 2–5× slower by `isdefined( array )` deep-copying
  its argument, objects/arrays in ordered maps, three entity lookups per field access, a `foreach` key list copied twice, and
  a resource lookup pair per instruction; (2) the first `useanimtree` of `generic_human` built the 3,630-node tree and decoded its
  clips in the spawn tick (`dospawn`/`stalingradspawn` waves: 80–300 ms); (3) actor/player shots re-tested every collision row
  per pellet and penetration step (55–70 ms per shotgun/sniper `shoot` on a loaded Air); (4) per tick: a full heap mark, anim
  advance and pose publishing over every node of every actor's tree, every native trace re-reading every presence entity's
  fields and re-sorting them, a thread-entity scan and a wait-list scan per resumption, `setgoalvolumeauto` resolving the
  volume per path node, posing a DObj rebuilt per pose, FX world traces re-posing every model's movement brushes and fully
  testing every brush model.
- Fixes (all deterministic; the schedule counts ticks/allocations, never time): animtrees built on a background thread at
  load (`ScriptAnimLibrary::prewarm`, levels with `generic_human` only; a request in flight waits); heap collection every 10th
  tick or 8192 allocations; `EntityAnim` keeps the ascending list of non-default nodes and advances/frees/publishes only those
  (`XAnimTreeRuntime::update_inherited_rate_listed`, same result as the full walk); thread index (serial → entity) and a
  "doom epoch" so resumptions skip the wait-list scan unless an entity was deleted or a wait registered on one; objects and
  arrays keyed by a hashed id, arrays as a dense vector for keys 0.. plus an ordered map (`script/array.rs`, iteration order
  unchanged, differential-checked against `BTreeMap`); one entity lookup per field access; no argument copy for
  `isdefined`/`isarray`; `foreach` stores its fresh key list without a copy; the instruction budget kept in a local; field
  writes counted (`Objects`) so a settle reuses poses read since the last write, and keeps its sorted order while the
  presence set is unchanged; `collect_wanted` reads field ids once; `contains_points` for goal volumes; cached single-model
  DObj and a bone-name map for track binding (`xmodel_runtime`); shots keep only the rows the emission ray can reach; cached
  posed movement brushes per row; `clipmap_iw4::transformed_capsule_trace` rejects a brush model whose bounds the swept
  capsule misses (exactly the open trace `trace_capsule` would return).
- Air, before → after (thread CPU unless noted; tick = authority sim tick, median of 200-tick windows; frame = bench wall
  over the run, noisy with other agents' builds on the Air; level-entry tick = the first script tick after the player
  spawns; the sabotage baseline ran on a busier Air):

| mission | level-entry tick CPU ms | worst later script tick CPU ms (wall) | sim tick CPU p50 / p99 ms | frame p50 / p99 ms |
|---|---|---|---|---|
| so_killspree_favela (mid-fight) | 186 → 92 | 112 (122) → none ≥ 50 | 7.6 / 10.8 → 3.7 / 7.0 | 11.3 / 24 → 10.8 / 19 |
| so_showers_gulag | 644 → 204 | 101 (102) → none | 10.4 / 22.2 → 6.4 / 14.7 | 32.0 / 64 → 28.8 / 53 |
| so_snowrace1_cliffhanger | 467 → 130 | none → none | 9.0 / 12.8 → 4.3 / 7.8 | 5.9 / 47 → 5.4 / 17 |
| so_hidden_so_ghillies | < 50 → < 50 | 82 (82) → none | 7.5 / 12.8 → 4.0 / 9.1 | 6.7 / 24 → 6.4 / 19 |
| so_sabotage_cliffhanger | 768 → 150 | 241 (324) → none | 11.6 / 17.1 → 3.7 / 7.5 | 14.9 / 78 → 12.3 / 20 |

- Regressions (Air): The Pit 2:27.90 (24/24), favela, showers (`IW4L_AUTOAIM=1`), snowrace1, hidden and sabotage
  complete with their run scripts; `mp_boneyard` `spawn 0` → InGame.
- Interpreter alone (local, a `global_fx_create`-shaped loop: 300 × `foreach` over 1,000 structs + field writes): 171 → 60 ms.
- Remaining: the level-entry tick is still 90–200 ms CPU on favela/showers/cliffhanger (pure script instructions at
  ~10–20 ns each); hiding it needs the loading overlay held until the SP entry has run (session/UI), or a faster interpreter.
  FX particle world traces (~1,000–1,600 per tick on favela/hidden, ~6 µs each) barely moved: the bounds reject and cached
  movement brushes are small next to walking every collision row per trace (a per-row index of brush models would be next).
  `actors` (perception/nav) and snowrace's `vehicles::advance` and spline scripts are the largest per-tick costs left.
