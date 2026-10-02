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
