//! Actor spawners, actor spawning (`aitype/<name>::main`, `animscripts/init::main`),
//! the actor/sentient field branch, AI queries and the per-tick actor think.

use super::anim::EntityAnim;
use super::args::{arg, float, optional, string};
use super::arrays::new_array;
use super::entities::EntityKind;
use super::natives::engine::entity_id;
use crate::actor::fields::{self, TEAMS};
use crate::actor::{Actor, ActorId, ActorPool, MAX_ACTORS};
use crate::frame::FrameWorld;
use crate::script::runtime::{raise, run_now_thread, thread_running};
use crate::script::{Arc, Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

const ANIMTREE: &str = "generic_human";
/// How long a finished animscript waits before the think starts it again.
pub(crate) const ANIMSCRIPT_RETRY_MS: i64 = 500;

pub(crate) fn actor_of(world: &World, object: u64) -> Option<ActorId> {
    match world.resource::<Runtime>().entities.get(&object)?.kind {
        EntityKind::Actor(actor) => Some(actor),
        _ => None,
    }
}

fn aitype(classname: &str) -> String {
    let name = classname.to_ascii_lowercase();
    format!("aitype/{}", name.strip_prefix("actor_").unwrap_or(&name))
}

pub(crate) fn has_function(world: &World, name: &str) -> bool {
    world
        .resource::<Runtime>()
        .program
        .as_ref()
        .is_some_and(|program| program.names.contains_key(name))
}

fn classname_team(classname: &str) -> &'static str {
    let classname = classname.to_ascii_lowercase();
    if classname.starts_with("actor_ally_") {
        "allies"
    } else if classname.starts_with("actor_enemy_") {
        "axis"
    } else {
        "neutral"
    }
}

fn now_ms(world: &World) -> i64 {
    world
        .get_resource::<crate::step::StepRequest>()
        .map_or(0, |request| {
            i64::from(request.tick.0) * i64::from(crate::MATCH_TICK_MS)
        })
}

fn vector_field(runtime: &mut Runtime, object: u64, name: &str) -> [f32; 3] {
    match runtime.object_field(object, name) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

pub(crate) fn run_script(world: &mut World, name: &str, receiver: Value) -> Option<u64> {
    if !has_function(world, name) {
        return None;
    }
    let now = now_ms(world);
    match run_now_thread(world, name, receiver, Vec::new(), now) {
        Ok(running) => running,
        Err(fault) => {
            diag::warn!(Sim, "actor: {name} faulted: {fault}");
            None
        }
    }
}

/// Map `actor_*` entities: spawners (`spawnflags & 1`) stay as `ActorSpawner`s,
/// the rest spawn on the first actor think. Every aitype present is precached.
pub(crate) fn install_spawners(world: &mut World) {
    let mut runtime = world.resource_mut::<Runtime>();
    let placed: Vec<(u64, Arc<str>)> = runtime
        .entities
        .iter()
        .filter(|(_, e)| {
            e.kind == EntityKind::Map && e.classname.to_ascii_lowercase().starts_with("actor_")
        })
        .map(|(id, e)| (*id, e.classname.clone()))
        .collect();
    if placed.is_empty() {
        return;
    }
    let mut spawners = Vec::new();
    let mut pending = Vec::new();
    for (id, _) in &placed {
        if runtime.object_field(*id, "count") == Value::Undefined {
            runtime.set_object_field(*id, "count", Value::Int(1));
        }
        let flags = match runtime.object_field(*id, "spawnflags") {
            Value::Int(flags) => flags,
            _ => 0,
        };
        runtime.entities.get_mut(id).unwrap().kind = EntityKind::ActorSpawner;
        if flags & 1 != 0 {
            spawners.push(*id);
        } else {
            pending.push(*id);
        }
    }
    let aitypes: std::collections::BTreeSet<String> = placed
        .iter()
        .map(|(_, classname)| aitype(classname))
        .collect();
    world.resource_mut::<ActorPool>().pending = pending.clone();
    for name in &aitypes {
        run_script(world, &format!("{name}::precache"), Value::level());
    }
    for (id, classname) in &placed {
        run_script(
            world,
            &format!("{}::spawner", aitype(classname)),
            Value::Object(*id),
        );
    }
    diag::info!(
        Sim,
        "actor: {} spawners, {} placed actors, {} aitypes",
        spawners.len(),
        pending.len(),
        aitypes.len()
    );
}

fn spawner_team(world: &World, spawner: u64) -> Arc<str> {
    if let Some(team) = world.resource::<ActorPool>().spawner_teams.get(&spawner) {
        return team.clone();
    }
    let runtime = world.resource::<Runtime>();
    runtime
        .entities
        .get(&spawner)
        .map_or("neutral", |e| classname_team(&e.classname))
        .into()
}

const UNCOPIED: [&str; 6] = [
    "classname",
    "code_classname",
    "spawnflags",
    "count",
    "origin",
    "angles",
];

/// `dospawn` / `stalingradspawn`: an actor at the spawner, set up by its aitype and
/// `animscripts/init`; the spawner gets `"spawned"`.
fn spawn_actor(world: &mut World, spawner: u64, notify: bool) -> Result<Value, String> {
    let (classname, count) = {
        let mut runtime = world.resource_mut::<Runtime>();
        let entity = runtime.entities.get(&spawner).ok_or("spawner is gone")?;
        if entity.kind != EntityKind::ActorSpawner {
            return Err("entity is not an actor spawner".into());
        }
        let classname = entity.classname.clone();
        let count = match runtime.object_field(spawner, "count") {
            Value::Int(n) => n,
            Value::Float(n) => n as i32,
            _ => 0,
        };
        (classname, count)
    };
    if count <= 0 || world.resource::<ActorPool>().actors.len() >= MAX_ACTORS {
        return Ok(Value::Undefined);
    }
    let team = spawner_team(world, spawner);
    let origin = vector_field(&mut world.resource_mut::<Runtime>(), spawner, "origin");
    let angles = vector_field(&mut world.resource_mut::<Runtime>(), spawner, "angles");
    let presence = super::presence::spawn_presence(world, origin)?;
    let actor = world.resource_mut::<ActorPool>().allocate();
    let object = {
        let mut runtime = world.resource_mut::<Runtime>();
        runtime.set_object_field(spawner, "count", Value::Int(count - 1));
        let object = runtime.create_entity(EntityKind::Actor(actor), &classname)?;
        let skipped: Vec<u32> = UNCOPIED.iter().map(|name| runtime.symbol(name)).collect();
        let copied: Vec<(u32, Value)> = runtime.objects[&spawner]
            .iter()
            .filter(|(field, value)| {
                !skipped.contains(field)
                    && matches!(
                        value,
                        Value::Int(_)
                            | Value::Float(_)
                            | Value::String(_)
                            | Value::LocalizedString(_)
                            | Value::Vector(_)
                    )
            })
            .map(|(field, value)| (*field, value.clone()))
            .collect();
        runtime.objects.get_mut(&object).unwrap().extend(copied);
        runtime.set_object_field(object, "origin", Value::Vector(origin));
        runtime.set_object_field(object, "angles", Value::Vector([0.0, angles[1], 0.0]));
        runtime.set_object_field(object, "health", Value::Int(100));
        let entity = runtime.entities.get_mut(&object).unwrap();
        entity.presence = Some(presence);
        entity.can_damage = true;
        object
    };
    world
        .resource_mut::<ActorPool>()
        .actors
        .insert(actor, Actor::new(object, team, origin));
    use_tree(world, object, ANIMTREE);
    let receiver = Value::Object(object);
    run_script(
        world,
        &format!("{}::main", aitype(&classname)),
        receiver.clone(),
    );
    // The aitype names its tree (`self.animTree = "dog.atr"`); humans keep the default.
    let tree = match world
        .resource_mut::<Runtime>()
        .object_field(object, "animtree")
    {
        Value::String(name) => name.trim_end_matches(".atr").to_ascii_lowercase(),
        _ => String::new(),
    };
    if !tree.is_empty() && tree != ANIMTREE {
        use_tree(world, object, &tree);
    }
    {
        let mut runtime = world.resource_mut::<Runtime>();
        let health = runtime.object_field(object, "health");
        runtime.set_object_field(object, "maxhealth", health);
    }
    let init = format!(
        "animscripts/{}::main",
        animscript_module(world, actor, "init")
    );
    if let Some(serial) = run_script(world, &init, receiver.clone()) {
        diag::warn!(
            Sim,
            "actor: animscripts/init::main waited (thread {serial})"
        );
    }
    // Spawned after this tick's think, the actor would reach the snapshot with
    // an empty tree (one tick in the bind pose); its state's animscript starts now.
    if world.resource::<Runtime>().live(&object) {
        let now = now_ms(world);
        super::actor_nav::select_animscript(world, actor, object, now);
    }
    if notify {
        raise(
            world,
            Value::Object(spawner),
            "spawned",
            vec![receiver.clone()],
        );
    }
    let team = world.resource::<ActorPool>().actors[&actor].team.clone();
    let alive = live_actors(world).len();
    let model = match world
        .resource_mut::<Runtime>()
        .object_field(object, "model")
    {
        Value::String(model) => model.to_string(),
        _ => String::new(),
    };
    let posed = FrameWorld::from_world(world)
        .model_capability(&model)
        .flatten()
        .is_some();
    diag::info!(
        Sim,
        "actor: spawned {classname} team={team} at {:.0} {:.0} {:.0} model={model} posed={posed} ai={alive}",
        origin[0],
        origin[1],
        origin[2]
    );
    Ok(receiver)
}

pub(crate) fn load_field(world: &mut World, actor: ActorId, name: &str) -> Option<Value> {
    let def = fields::actor_field(name)?;
    if let Some(value) = super::actor_combat::load_field(world, actor, def.name) {
        return Some(value);
    }
    let pool = world.resource::<ActorPool>();
    let state = pool.actors.get(&actor)?;
    let value = match def.name {
        "team" => Value::string(&state.team),
        "type" => Value::string(&state.species),
        "script" => Value::string(state.animscript.as_ref().map_or("init", |(name, _)| name)),
        "prevscript" => Value::string(state.prev_animscript.as_deref().unwrap_or("init")),
        "goalpos" => Value::Vector(state.goal.pos),
        "pathgoalpos" => state
            .path
            .as_ref()
            .map_or(Value::Undefined, |path| Value::Vector(path.final_goal)),
        "lookaheaddir" => Value::Vector(state.lookahead_dir),
        "lookaheaddist" => Value::Float(state.lookahead_dist),
        "velocity" => Value::Vector(state.velocity),
        // The move script never sees "stop" (it would pick no move anim set):
        // a path picked this tick has not moved the actor yet, and a finished
        // path ends the move script only at the next animscript selection.
        "movemode"
            if state.move_mode == crate::actor::MoveMode::Stop
                && (state.path.is_some()
                    || state
                        .animscript
                        .as_ref()
                        .is_some_and(|(name, _)| &**name == "move")) =>
        {
            Value::string(if state.path.is_some() { "run" } else { "walk" })
        }
        "movemode" => Value::string(state.move_mode.name()),
        "footstepdetectdist" | "footstepdetectdistwalk" | "footstepdetectdistsprint"
            if !state.fields.contains_key(def.name) =>
        {
            let name = def.name;
            return super::actor_events::footstep_detect_default(world, name).map(Value::Float);
        }
        "alertlevelint" => Value::Int(match state.fields.get("alertlevel") {
            Some(Value::String(level)) => alert_level_int(level).unwrap_or(0),
            _ => 0,
        }),
        "node" | "prevnode" => {
            let node = if def.name == "node" {
                state.claimed
            } else {
                state.prev_claimed
            };
            return Some(super::actor_nav::node_value(world, node));
        }
        "lookforward" | "lookright" | "lookup" => {
            let object = state.object;
            let angles = vector_field(&mut world.resource_mut::<Runtime>(), object, "angles");
            let (forward, right, up) = math_iw4::angle_vectors(angles);
            Value::Vector(match def.name {
                "lookforward" => forward,
                "lookright" => right,
                _ => up,
            })
        }
        _ => state
            .fields
            .get(def.name)
            .cloned()
            .unwrap_or_else(|| fields::default_value(def)),
    };
    Some(value)
}

pub(crate) fn store_field(
    world: &mut World,
    actor: ActorId,
    name: &str,
    value: &Value,
) -> Result<bool, String> {
    let Some(def) = fields::actor_field(name) else {
        if fields::read_only_entity_field(name) {
            return Err(format!("entity field {name} is read-only"));
        }
        return Ok(false);
    };
    let value = fields::coerce(def, value)?;
    let mut pool = world.resource_mut::<ActorPool>();
    let Some(state) = pool.actors.get_mut(&actor) else {
        return Ok(false);
    };
    let mut alert_change = None;
    match (def.name, &value) {
        ("team", Value::String(team)) => {
            let team = team.to_string();
            if !TEAMS.contains(&team.as_str()) {
                return Err(format!("unknown team '{team}'"));
            }
            state.team = team.into();
        }
        ("type", Value::String(species)) => state.species = species.to_string().into(),
        ("alertlevel", Value::String(level)) => {
            if alert_level_int(level).is_none() {
                return Err(format!("unknown alert level '{level}'"));
            }
            let was = match state.fields.insert(def.name, value.clone()) {
                Some(Value::String(old)) => old.to_string(),
                _ => "noncombat".into(),
            };
            if !was.eq_ignore_ascii_case(level) {
                alert_change = Some((state.object, was, level.to_string()));
            }
        }
        _ => {
            state.fields.insert(def.name, value);
        }
    }
    if let Some((object, was, level)) = alert_change {
        diag::info!(
            Sim,
            "actor: {} alertlevel {was} -> {level}",
            super::actor_combat::label(world, object)
        );
    }
    Ok(true)
}

/// `alertlevel` as the engine stores it (`alertlevelint`).
fn alert_level_int(level: &str) -> Option<i32> {
    ["noncombat", "aware", "alert", "combat"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(level))
        .map(|n| n as i32)
}

fn live_actors(world: &World) -> Vec<(i32, ActorId, u64)> {
    let runtime = world.resource::<Runtime>();
    let mut actors: Vec<_> = world
        .resource::<ActorPool>()
        .actors
        .iter()
        .filter(|(_, a)| a.dying.is_none() && runtime.live(&a.object))
        .filter_map(|(id, a)| Some((runtime.entities.get(&a.object)?.number, *id, a.object)))
        .collect();
    actors.sort_unstable();
    actors
}

fn team_matches(team: &str, wanted: &str) -> bool {
    match wanted {
        "all" => true,
        "bad_guys" => team == "axis" || team == "team3",
        wanted => team == wanted,
    }
}

fn use_tree(world: &mut World, object: u64, name: &str) {
    match FrameWorld::from_world(world)
        .content()
        .script_anims()
        .tree(name)
    {
        Ok(tree) => {
            world
                .resource_mut::<super::mechanics::Mechanics>()
                .anims
                .insert(object, EntityAnim::new(tree));
        }
        Err(error) => diag::warn!(Sim, "actor: {error}"),
    }
}

/// The module an engine animscript state runs for this actor's species: dogs
/// run `animscripts/dog/dog_<state>` (states they have no script for fight).
pub(crate) fn animscript_module(world: &World, actor: ActorId, state: &str) -> String {
    let dog = world
        .resource::<ActorPool>()
        .actors
        .get(&actor)
        .is_some_and(|a| &*a.species == "dog");
    if !dog || state.starts_with("traverse/") {
        return state.to_owned();
    }
    match state {
        "init" | "move" | "stop" | "combat" | "death" | "pain" | "flashed" | "scripted" => {
            format!("dog/dog_{state}")
        }
        _ => "dog/dog_combat".to_owned(),
    }
}

fn ai_array(world: &mut World, teams: &[String], species: Option<&str>) -> Result<Value, String> {
    let objects: Vec<Value> = {
        let pool = world.resource::<ActorPool>();
        live_actors(world)
            .into_iter()
            .filter(|(_, id, _)| {
                let actor = &pool.actors[id];
                (teams.is_empty() || teams.iter().any(|t| team_matches(&actor.team, t)))
                    && species.is_none_or(|s| s == "all" || *actor.species == *s)
            })
            .map(|(_, _, object)| Value::Object(object))
            .collect()
    };
    new_array(world, objects)
}

fn spawners(world: &World, team: Option<&str>) -> Vec<u64> {
    let runtime = world.resource::<Runtime>();
    runtime
        .entities
        .iter()
        .filter(|(id, e)| e.kind == EntityKind::ActorSpawner && runtime.live(id))
        .filter(|(id, _)| team.is_none_or(|team| team_matches(&spawner_team(world, **id), team)))
        .map(|(id, _)| *id)
        .collect()
}

fn objects(world: &mut World, ids: Vec<u64>) -> Result<Value, String> {
    new_array(world, ids.into_iter().map(Value::Object).collect())
}

fn kind_of(world: &World, value: &Value) -> Option<EntityKind> {
    world
        .resource::<Runtime>()
        .entity(value)
        .map(|(_, e)| e.kind.clone())
}

fn receiver_actor(world: &World, receiver: &Value) -> Result<ActorId, String> {
    match kind_of(world, receiver) {
        Some(EntityKind::Actor(actor)) => Ok(actor),
        _ => Err("receiver is not an actor".into()),
    }
}

fn set_actor_fields(world: &mut World, actor: ActorId, values: &[(&'static str, Value)]) {
    if let Some(state) = world.resource_mut::<ActorPool>().actors.get_mut(&actor) {
        for (name, value) in values {
            state.fields.insert(name, value.clone());
        }
    }
}

/// Keeps actors alive between script calls: spawns map-placed actors and keeps an
/// animscript running on every actor (`animscripts/stop` until goals and combat exist).
pub(crate) fn run_actors(world: &mut World) {
    let request = world.resource::<crate::step::StepRequest>();
    if !request.reason.advances_authority_world() || world.resource::<Runtime>().program.is_none() {
        return;
    }
    let pending = std::mem::take(&mut world.resource_mut::<ActorPool>().pending);
    for spawner in pending {
        if let Err(error) = spawn_actor(world, spawner, false) {
            diag::warn!(Sim, "actor: placed actor: {error}");
        }
    }
    if world.resource::<ActorPool>().actors.is_empty() {
        return;
    }
    let gone: Vec<ActorId> = {
        let runtime = world.resource::<Runtime>();
        world
            .resource::<ActorPool>()
            .actors
            .iter()
            .filter(|(_, a)| !runtime.live(&a.object))
            .map(|(id, _)| *id)
            .collect()
    };
    for id in gone {
        super::actor_nav::release_all(world, id);
        world.resource_mut::<ActorPool>().actors.remove(&id);
    }
    let now = now_ms(world);
    if now % 5000 == 0 {
        let live = live_actors(world);
        let serials: Vec<(Arc<str>, Option<u64>)> = {
            let pool = world.resource::<ActorPool>();
            live.iter()
                .map(|(_, id, _)| {
                    let actor = &pool.actors[id];
                    (
                        actor.team.clone(),
                        actor.animscript.as_ref().map(|(_, serial)| *serial),
                    )
                })
                .collect()
        };
        let axis = serials.iter().filter(|(team, _)| &**team == "axis").count();
        let running = serials
            .iter()
            .filter(|(_, serial)| serial.is_some_and(|serial| thread_running(world, serial)))
            .count();
        let presented = {
            let runtime = world.resource::<Runtime>();
            live.iter()
                .filter(|(_, _, object)| runtime.shown.contains_key(object))
                .count()
        };
        diag::info!(
            Sim,
            "actor: getaiarray(\"axis\").size={axis} ai={} animscripts_running={running} presented={presented}",
            live.len()
        );
        for (number, id, object) in &live {
            let origin = vector_field(&mut world.resource_mut::<Runtime>(), *object, "origin");
            let health = match world
                .resource_mut::<Runtime>()
                .object_field(*object, "health")
            {
                Value::Int(h) => h,
                _ => 0,
            };
            let pool = world.resource::<ActorPool>();
            let a = &pool.actors[id];
            let goal = a.goal.pos;
            let to_goal = ((goal[0] - origin[0]).powi(2) + (goal[1] - origin[1]).powi(2)).sqrt();
            diag::info!(
                Sim,
                "actor: entity {number} {} script={} movemode={} at {:.0} {:.0} {:.0} goal {:.0} {:.0} {:.0} node={:?} dist={to_goal:.0} radius={:.0} moved={:.0} path={}/{} enemy={:?} shots={} hits_taken={} health={} animmode={} arrival={} detour={:?} speed={:.0} traverse={}",
                a.team,
                a.animscript.as_ref().map_or("none", |(name, _)| name),
                a.move_mode.name(),
                origin[0],
                origin[1],
                origin[2],
                goal[0],
                goal[1],
                goal[2],
                a.goal.node,
                a.float_field("goalradius"),
                a.distance_moved,
                a.path
                    .as_ref()
                    .map_or(0, |p| p.points.len() - p.next.min(p.points.len())),
                a.path.as_ref().map_or(0, |p| p.nodes),
                a.enemy,
                a.shots,
                a.hits_taken,
                health,
                a.anim_mode,
                a.arrival.is_some(),
                a.detour.map(|d| d.kind),
                (a.velocity[0] * a.velocity[0] + a.velocity[1] * a.velocity[1]).sqrt(),
                a.traverse.as_ref().map_or("-", |n| &*n.script),
            );
        }
    }
    let mut budget = super::actor_nav::EXPANSION_BUDGET;
    super::presence::settle_collision(world);
    let live = live_actors(world);
    super::actor_events::run(world, &live, now);
    super::actor_combat::run(world, &live, now);
    super::actor_grenade::run(world, &live, now);
    for (_, id, object) in live_actors(world) {
        super::actor_nav::think(world, id, object, now, &mut budget);
    }
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};

    registry.register(Method, "dospawn", |world, receiver, _| {
        let spawner = entity_id(world, receiver)?;
        spawn_actor(world, spawner, true)
    });
    registry.register(Method, "stalingradspawn", |world, receiver, _| {
        let spawner = entity_id(world, receiver)?;
        spawn_actor(world, spawner, true)
    });
    registry.register(Method, "setspawnerteam", |world, receiver, args| {
        let spawner = entity_id(world, receiver)?;
        let team = string(args, 0)?.to_ascii_lowercase();
        if !TEAMS.contains(&team.as_str()) {
            return Err(format!("unknown team '{team}'"));
        }
        world
            .resource_mut::<ActorPool>()
            .spawner_teams
            .insert(spawner, team.into());
        Ok(Value::Undefined)
    });

    registry.register(Function, "getaiarray", |world, _, args| {
        let teams = (0..args.len())
            .map(|i| string(args, i).map(|t| t.to_ascii_lowercase()))
            .collect::<Result<Vec<_>, _>>()?;
        ai_array(world, &teams, None)
    });
    registry.register(Function, "getaispeciesarray", |world, _, args| {
        let team = optional(args, 0, string)?.map(|t| t.to_ascii_lowercase());
        let species = optional(args, 1, string)?.map(|s| s.to_ascii_lowercase());
        ai_array(world, team.as_slice(), species.as_deref())
    });
    registry.register(Function, "getaicount", |world, _, _| {
        Ok(Value::Int(live_actors(world).len() as i32))
    });
    registry.register(Function, "getspawnerarray", |world, _, _| {
        let ids = spawners(world, None);
        objects(world, ids)
    });
    registry.register(Function, "getspawnerteamarray", |world, _, args| {
        let team = string(args, 0)?.to_ascii_lowercase();
        let ids = spawners(world, Some(&team));
        objects(world, ids)
    });
    registry.register(Function, "isspawner", |world, _, args| {
        let value = arg(args, 0)?;
        let spawner = kind_of(world, value) == Some(EntityKind::ActorSpawner)
            || super::vehicles::is_sp_spawner(world, value);
        Ok(Value::Int(spawner.into()))
    });
    registry.register(Function, "isai", |world, _, args| {
        let ai = matches!(kind_of(world, arg(args, 0)?), Some(EntityKind::Actor(_)));
        Ok(Value::Int(ai.into()))
    });
    registry.register(Function, "issentient", |world, _, args| {
        let value = arg(args, 0)?;
        let sentient = world
            .resource::<Runtime>()
            .player_client_of(value)
            .is_some()
            || matches!(kind_of(world, value), Some(EntityKind::Actor(_)));
        Ok(Value::Int(sentient.into()))
    });
    registry.register(Method, "getentnum", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        Ok(Value::Int(world.resource::<Runtime>().entities[&id].number))
    });

    registry.register(Method, "setengagementmindist", |world, receiver, args| {
        let actor = receiver_actor(world, receiver)?;
        let near = float(args, 0)?;
        let falloff = float(args, 1)?;
        set_actor_fields(
            world,
            actor,
            &[
                ("engagemindist", Value::Float(near)),
                ("engageminfalloffdist", Value::Float(falloff)),
            ],
        );
        Ok(Value::Undefined)
    });
    registry.register(Method, "setengagementmaxdist", |world, receiver, args| {
        let actor = receiver_actor(world, receiver)?;
        let far = float(args, 0)?;
        let falloff = float(args, 1)?;
        set_actor_fields(
            world,
            actor,
            &[
                ("engagemaxdist", Value::Float(far)),
                ("engagemaxfalloffdist", Value::Float(falloff)),
            ],
        );
        Ok(Value::Undefined)
    });
}
