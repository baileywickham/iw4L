# Actor core design (milestone 4)

Written 2026-10-02 from a read of this repo, the SP GSC (trainer, `animscripts/*`, `maps/_*`, `aitype/*`) and, for C-side responsibilities only, KisakCOD's CoD4 SP actor code (`src/game/actor_*.cpp`, `sentient*.cpp`, `pathnode.cpp`).

## Findings that shape it

- The engine calls named scripts itself: `aitype/%s`, `animscripts/%s`, `animscripts/%s/%s`, `animscripts/traverse/%s`, `animscripts/scripted`, the `generic_human` animtree and the `killanimscript` notify. Entry animscripts: init, combat, move, stop, death, pain, flashed, grenade_cower, grenade_return_throw, cover_arrival, cover_*, scripted, reactions, turret, dog_*. Entity types `ET_ACTOR`, `ET_ACTOR_SPAWNER`, `ET_ACTOR_CORPSE` follow `ET_VEHICLE_SPAWNER`.
- Reusable: `runtime::run_now` (synchronous script thread, the equivalent of setting an animscript), `XAnimTreeRuntime` (goal weights, rates, `calc_delta_translation`), `DObjSemanticState`/`XAnimTreeSnapshot` already in `SnapshotMeta.entity_dobjs`, `movement_iw4::step_slide_move<C: CollisionBackend>`, `#using_animtree`/`%anim` → `Value::Animation`.
- The combat pipeline assumes a player attacker (`AcceptedShot.attacker`, `magic_bullet(owner: ClientId)`, `ColliderId::Player`); actors need an `Attacker` enum.
- Field use in `animscripts` + `_spawner` + `_utility`: `self.a` 1161 (script struct, not an engine field). Engine fields by count: `enemy` 297, `weapon` 151, `node` 103, `team` 101, `goalradius` 55, then `damageyaw`, `type`, `health`, `goodshootpos`, `grenade`, `lookaheaddir`, `groundtype`, `allowpain`, `fixednode`, `movemode`, `keepclaimednode`, `pathenemy*`, `ignoreme`. Dump the authoritative actor/sentient field tables from `iw4sp.exe` (extend `scripts/gen-iw4sp-catalog.py`).

## Layout

`crates/sim/src/actor/`: `mod.rs` (`ActorId`, `Actor`, `ActorPool` resource, cloned with `SimWorld`), `spawn.rs`, `state.rs` (AIS state stack + animscript dispatch), `anim.rs`, `goal.rs`, `path.rs` (A* over `SimPathGraph` + per-node runtime: claim owner, dangerous-until, disconnect count), `physics.rs`, `senses.rs` (sentients, vis cache, threat-bias groups), `combat.rs`, `grenade.rs`, `cover.rs`, `death.rs`. Natives in `script/host/actors.rs` and `script/host/sentients.rs`.

- `EntityKind::Actor(ActorId)`, `ActorSpawner`, `ActorCorpse` in `host/entities.rs`; the script entity is identity, `Actor` holds sim state.
- Fields: a fourth branch in `Op::LoadField`/`StoreField` after players; `ACTOR_FIELDS: &[ActorFieldDef { name, kind, get, set }]`, resolved once per program into a symbol-indexed table. `origin`/`angles`/`health`/`team`/`targetname` read/write the `Actor`; unknown names fall through to the generic map.
- Step: `run_actors_system` after `record_collision_state_system`, before `run_entity_types_system`, authority frames only, entnum order, own RNG stream (`actor_draws`). Per actor: state transitions (`killanimscript`, then `run_now("animscripts/<state>::main")`), state think, physics/anim delta, nearest node, look-at, touch triggers, notetrack/goal notifies. Clients don't predict actors.
- Network: `EntityState` with `ET_ACTOR` plus an `entity_dobjs` entry under `AuthorityModelOwner::Actor` (body + head + weapon, nonzero-weight anim nodes only, skip unchanged revisions). Hits via `ColliderId::EntityDObjBone`.
- Spawners: `actor_*` with `spawnflags & 1` → `ActorSpawner` (keys, `count`, `aitype/<name>::spawner()`); other `actor_*` spawn at load; `aitype/<name>::precache()` per classname. `dospawn` (refuses if blocked/visible) / `stalingradspawn` (forced): create, copy keys, `aitype/<name>::main`, `animscripts/init::main`, decrement `count`, notify `"spawned"`, later `"finished spawning"`. Bind `getaiarray`, `getaispeciesarray`, `getspawnerarray`, `getspawnerteamarray`, `isspawner`, `isai`, `issentient`, `isalive`, `setspawnerteam`.

## Responsibilities, in order

1. **Anim API** (blocks the rest): capture `animtrees/*.atr` RawFiles, parse to a tree definition with name → node ids; `setanim*` family (knob zeroes siblings, knoball the tree, limited skips parent weight, restart resets time), `clearanim`, anim time/length/notetrack queries, move/angle delta, `animscripted`, `animcustom`, `animmode`, `orientmode`, `useanimtree`; `setflaggedanim*` delivers notetracks and `"end"`; additive aim nodes (a typed gap in `xmodel_runtime` today).
2. **State stack**: exposed, turret, grenade, badplace, cover-arrival, death, pain, scripted, custom, negotiation.
3. **Fields** (above).
4. **Goals**: goal pos/node/entity/volume, radius/height, fixednode, `isingoal`, `"goal"`, node claims.
5. **Paths**: A* with link disconnects and badplaces, nearest node via node tree + sight, budgeted resumable search (like `bots/nav.rs`), negotiation links run `animscripts/traverse/<animscript>`, `connectpaths`/`disconnectpaths`.
6. **Movement** (done, `host/actor_motion.rs`): root motion from the weighted animtree delta (`EntityAnim::motion_delta`, blend weights normalized per node, additive layers skipped) through `step_slide_move` with a 15×72 hull. `normal`/`none` walk the path by the delta's length; `gravity`/`zonly_physics` apply the delta turned by yaw with ground and gravity; `nogravity` slides; `noclip`/`nophysics` apply it raw; anim-driven modes add the anim's yaw (a `face angle` target turns with it). Cover arrivals steer the remaining delta onto the node; negotiation links run `animscripts/traverse/<node animscript>` under `traversemode`. Kinematic fallbacks (logged): no root delta for 4 ticks in `move`, hull blocked 10 ticks, hull stuck in solid, link without a script.
7. **Perception**: sentient registry, per-pair vis cache with fov/max distance and a per-tick trace budget, last-known info, enemy selection by threat bias/distance/recent attacker, `ignoreall`/`ignoreme`/`favoriteenemy`, team sharing, `"enemy"`.
8. **Shooting/damage**: `Attacker::{Client, Entity}`; `shoot`/`shootblank`/`canshoot` from the muzzle tag; spread from accuracy × difficulty; damage fields + `"damage"` → pain/death.
9. **Grenades**: throw, throw-position solve, launcher, `grenade` field, cower/return-throw, pickup.
10. **Cover**: find/use/validate cover nodes scored by type, facing and path-vis exposure; arrival/exit.
11. **Death**: death animscript, corpse queue, weapon drop, `startragdoll` holds the last frame until there is ragdoll.

## Stages

| Stage | Done when | First mission |
|---|---|---|
| S0 measure | per-mission histogram of unbound natives and actor field use under `IW4L_GSC_STUB_NATIVES=1` | sizing |
| S1 spawn and stand | actors spawn, aitype + init run without runtime errors, idle loops, co-op client sees the idle | The Pit if ally AI blocks it |
| S2 path to goals | goal nodes reached with `"goal"`, move/stop scripts, one traverse | friendly chains |
| S3 see, shoot, die | spawner waves acquire and shoot the player, die into corpses | `so_killspree_favela`, `so_killspree_invasion` |
| S4 cover, grenades, pain | cover use, grenade throw/react | `so_bridge`, `so_juggernauts_favela` |
| S5 special | stealth awareness (done: `maxvisibledist`, AI events/listeners, `alertlevel`), dogs, turrets, vehicle riders | `so_ghillies`, `so_hidden_so_ghillies`, `so_ac130_co_hunted` |

## Risks

Catalog/field-table accuracy (diff all SO callsites against the catalog); animtree semantics (a flagged anim that never ends stalls a script — log per-tick weights in a single-actor harness); VM cost with 20+ AI; re-entrancy of `run_now` with `killanimscript`/`endon`; `entity_dobjs` bytes per actor; determinism (entnum order, BTreeMaps, own RNG, replay check); the `Attacker` refactor touching `combat.rs`, `missile.rs`, `damage.rs`.
