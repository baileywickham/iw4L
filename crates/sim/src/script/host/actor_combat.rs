//! Actor perception, shooting, damage, pain and death: the sentient registry,
//! enemy selection with last-known info, `shoot` through the bullet pipeline,
//! and dying into a corpse.

use super::actor_nav::switch_animscript;
use super::args::{arg, float, optional, string, vector};
use super::entities::EntityKind;
use super::entity_damage::EntityHit;
use super::natives::engine::{TraceIgnore, entity_id, entity_trace};
use super::sentients::{can_see, eye, muzzle};
use crate::actor::{ActorId, ActorPool, Dying, Known};
use crate::bullet_collision::TraceOutcome;
use crate::frame::FrameWorld;
use crate::script::runtime::{raise, thread_running};
use crate::script::{Arc, Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

/// Ticks between two looks of one actor (staggered by entity number).
const SENSE_TICKS: i64 = 2;
/// Sight traces all actors may spend in one tick.
const SIGHT_BUDGET: u32 = 24;
/// A known enemy not seen or reported for this long is forgotten.
const FORGET_MS: i64 = 20_000;
/// Squadmates this close hear about what an actor sees.
const SHARE_DIST: f32 = 768.0;
const RECENT_ATTACKER_MS: i64 = 4000;
/// The death animscript gets this long before the body becomes a corpse anyway.
const DEATH_TIMEOUT_MS: i64 = 8000;
const CORPSE_LIMIT: usize = 10;
const MASK_SHOT: u32 = 0x0280_0811;

fn now_ms(world: &World) -> i64 {
    i64::from(world.resource::<crate::step::StepRequest>().tick.0) * i64::from(crate::MATCH_TICK_MS)
}

fn origin(world: &mut World, object: u64) -> [f32; 3] {
    match super::players::entity_field(world, object, "origin") {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len > 0.0 {
        v.map(|c| c / len)
    } else {
        [0.0; 3]
    }
}

fn number_field(world: &mut World, object: u64, name: &str) -> Option<f32> {
    match super::players::entity_field(world, object, name) {
        Value::Int(v) => Some(v as f32),
        Value::Float(v) => Some(v),
        _ => None,
    }
}

fn health(world: &mut World, object: u64) -> i32 {
    number_field(world, object, "health").unwrap_or(0.0) as i32
}

fn label(world: &World, object: u64) -> String {
    let runtime = world.resource::<Runtime>();
    match runtime.player_client(object) {
        Some(client) => format!("player {client}"),
        None => runtime.entities.get(&object).map_or_else(
            || format!("object {object}"),
            |e| format!("entity {}", e.number),
        ),
    }
}

/// One sentient: a playing client, a live actor, or an entity made sentient.
#[derive(Clone, Debug)]
struct Sentient {
    object: u64,
    team: Arc<str>,
}

fn sentients(world: &mut World) -> Vec<Sentient> {
    let mut out = Vec::new();
    let players: Vec<(u32, u64)> = world
        .resource::<Runtime>()
        .players
        .iter()
        .map(|(client, slot)| (*client, slot.object))
        .collect();
    for (client, object) in players {
        let alive = FrameWorld::from_world(world)
            .client_meta(crate::ClientId(client))
            .is_some_and(|m| m.lifecycle == crate::ClientLifecycle::Alive);
        if !alive {
            continue;
        }
        let team = match super::players::load_field(world, client, "team") {
            Some(Value::String(team)) => team.to_string().into(),
            _ => "allies".into(),
        };
        out.push(Sentient { object, team });
    }
    let mut actors: Vec<(i32, Sentient)> = {
        let runtime = world.resource::<Runtime>();
        let pool = world.resource::<ActorPool>();
        let mut rows: Vec<(i32, Sentient)> = pool
            .actors
            .iter()
            .filter(|(_, a)| a.dying.is_none() && runtime.live(&a.object))
            .filter_map(|(_, a)| {
                Some((
                    runtime.entities.get(&a.object)?.number,
                    Sentient {
                        object: a.object,
                        team: a.team.clone(),
                    },
                ))
            })
            .collect();
        rows.extend(
            pool.sentients
                .iter()
                .filter(|(o, _)| runtime.live(o))
                .filter_map(|(object, team)| {
                    Some((
                        runtime.entities.get(object)?.number,
                        Sentient {
                            object: *object,
                            team: team.clone(),
                        },
                    ))
                }),
        );
        rows
    };
    actors.sort_by_key(|(number, _)| *number);
    out.extend(actors.into_iter().map(|(_, s)| s));
    out
}

pub(crate) fn hostile(a: &str, b: &str) -> bool {
    match (a, b) {
        ("axis", "allies") | ("allies", "axis") => true,
        ("team3", other) | (other, "team3") => matches!(other, "axis" | "allies"),
        _ => false,
    }
}

fn ignores(world: &mut World, actor: u64, target: u64) -> bool {
    number_field(world, target, "ignoreme").is_some_and(|v| v != 0.0)
        || super::natives::sp::threat_bias(world, actor, target) == i32::MIN
}

fn set_enemy(world: &mut World, id: ActorId, object: u64, enemy: Option<u64>) {
    let changed = world
        .resource_mut::<ActorPool>()
        .actors
        .get_mut(&id)
        .map(|a| std::mem::replace(&mut a.enemy, enemy) != enemy)
        .unwrap_or(false);
    if !changed {
        return;
    }
    if let Some(enemy) = enemy {
        let from = origin(world, object);
        let to = origin(world, enemy);
        diag::info!(
            Sim,
            "actor: {} acquired enemy {} dist={:.0}",
            label(world, object),
            label(world, enemy),
            length(sub(to, from))
        );
    }
    raise(world, Value::Object(object), "enemy", Vec::new());
}

/// Records that `id` knows where `target` is now; `seen` when by its own eyes.
fn learn(world: &mut World, id: ActorId, target: u64, seen: bool, now: i64) {
    let pos = origin(world, target);
    let sight = seen.then(|| eye(world, target));
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        let known = a.known.entry(target).or_insert(Known {
            time_ms: now,
            pos,
            sight_pos: None,
            seen_ms: None,
        });
        known.time_ms = now;
        known.pos = pos;
        if seen {
            known.sight_pos = sight;
            known.seen_ms = Some(now);
        }
    }
}

/// Squadmates near `id` that do not already know better hear about `target`.
fn share(world: &mut World, id: ActorId, object: u64, target: u64, now: i64) {
    let at = origin(world, object);
    let team = match world.resource::<ActorPool>().actors.get(&id) {
        Some(a) => a.team.clone(),
        None => return,
    };
    let mates: Vec<(ActorId, u64)> = world
        .resource::<ActorPool>()
        .actors
        .iter()
        .filter(|(other, a)| **other != id && a.team == team && a.dying.is_none())
        .filter(|(_, a)| a.known.get(&target).is_none_or(|k| k.time_ms < now))
        .map(|(other, a)| (*other, a.object))
        .collect();
    for (mate, mate_object) in mates {
        if length(sub(origin(world, mate_object), at)) <= SHARE_DIST {
            learn(world, mate, target, false, now);
        }
    }
}

/// One actor's look: sight checks against hostile sentients, then enemy choice
/// by distance, threat bias, recent attackers and `favoriteenemy`.
fn sense(
    world: &mut World,
    id: ActorId,
    object: u64,
    now: i64,
    everyone: &[Sentient],
    budget: &mut u32,
) {
    let Some((team, ignore_all, favorite, current, attacker)) =
        world.resource::<ActorPool>().actors.get(&id).map(|a| {
            (
                a.team.clone(),
                a.fields
                    .get("ignoreall")
                    .is_some_and(|v| *v == Value::Int(1)),
                match a.fields.get("favoriteenemy") {
                    Some(Value::Object(o)) => Some(*o),
                    _ => None,
                },
                a.enemy,
                a.last_attacker,
            )
        })
    else {
        return;
    };
    if ignore_all {
        set_enemy(world, id, object, None);
        return;
    }
    let targets: Vec<&Sentient> = everyone
        .iter()
        .filter(|s| s.object != object && hostile(&team, &s.team))
        .collect();
    let mut seen_any = Vec::new();
    for target in &targets {
        if ignores(world, object, target.object) {
            continue;
        }
        let cached = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .and_then(|a| a.sight.get(&target.object).copied())
            .is_some_and(|(at, _)| at == now);
        if !cached {
            if *budget == 0 {
                continue;
            }
            *budget -= 1;
        }
        if can_see(world, id, object, target.object, 0) {
            learn(world, id, target.object, true, now);
            seen_any.push(target.object);
        }
    }
    for target in seen_any {
        share(world, id, object, target, now);
    }
    let at = origin(world, object);
    let known: Vec<(u64, Known)> = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| a.known.iter().map(|(t, k)| (*t, *k)).collect())
        .unwrap_or_default();
    let mut best: Option<(f32, u64)> = None;
    let mut forget = Vec::new();
    for (target, info) in known {
        let still = targets.iter().any(|s| s.object == target);
        if !still || now - info.time_ms > FORGET_MS {
            forget.push(target);
            continue;
        }
        if ignores(world, object, target) {
            continue;
        }
        if favorite == Some(target) {
            best = Some((f32::INFINITY, target));
            break;
        }
        let mut score = -length(sub(info.pos, at));
        score += super::natives::sp::threat_bias(world, object, target) as f32;
        score += number_field(world, target, "threatbias").unwrap_or(0.0);
        if info.seen_ms.is_some_and(|t| now - t < 3000) {
            score += 300.0;
        }
        if attacker.is_some_and(|(o, t)| o == target && now - t < RECENT_ATTACKER_MS) {
            score += 1000.0;
        }
        if current == Some(target) {
            score += 200.0;
        }
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, target));
        }
    }
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        for target in &forget {
            a.known.remove(target);
        }
    }
    set_enemy(world, id, object, best.map(|(_, t)| t));
}

/// Perception for every live actor, then the dying: death animscripts that have
/// ended (or run too long) leave corpses.
pub(crate) fn run(world: &mut World, actors: &[(i32, ActorId, u64)], now: i64) {
    let tick = now / i64::from(crate::MATCH_TICK_MS);
    let everyone = sentients(world);
    let mut budget = SIGHT_BUDGET;
    for (number, id, object) in actors {
        if (tick + i64::from(*number)) % SENSE_TICKS == 0 {
            sense(world, *id, *object, now, &everyone, &mut budget);
        }
    }
    let dying: Vec<(ActorId, u64, i64, Option<u64>)> = world
        .resource::<ActorPool>()
        .actors
        .iter()
        .filter_map(|(id, a)| {
            let since = a.dying?.since_ms;
            Some((*id, a.object, since, a.animscript.as_ref().map(|(_, s)| *s)))
        })
        .collect();
    for (id, object, since, serial) in dying {
        let running = serial.is_some_and(|serial| serial != 0 && thread_running(world, serial));
        if !running || now - since > DEATH_TIMEOUT_MS {
            become_corpse(world, id, object);
        }
    }
}

fn hitloc_of_tag(tag: &str) -> &'static str {
    let tag = tag.to_ascii_lowercase();
    let side =
        |left: &'static str, right: &'static str| if tag.ends_with("_le") { left } else { right };
    match tag.as_str() {
        "j_helmet" | "tag_helmet" => "helmet",
        "j_head" | "j_head_end" | "tag_eye" => "head",
        "j_neck" => "neck",
        "j_spine4" | "j_spineupper" | "j_clavicle_le" | "j_clavicle_ri" => "torso_upper",
        "j_mainroot" | "j_spinelower" | "pelvis" | "tag_origin" => "torso_lower",
        t if t.starts_with("j_shoulder") || t.starts_with("j_elbow_bulge") => {
            side("left_arm_upper", "right_arm_upper")
        }
        t if t.starts_with("j_elbow") => side("left_arm_lower", "right_arm_lower"),
        t if t.starts_with("j_wrist") || t.starts_with("j_gun") => side("left_hand", "right_hand"),
        t if t.starts_with("j_hip") => side("left_leg_upper", "right_leg_upper"),
        t if t.starts_with("j_knee") => side("left_leg_lower", "right_leg_lower"),
        t if t.starts_with("j_ankle") || t.starts_with("j_ball") => side("left_foot", "right_foot"),
        t if t.starts_with("tag_weapon") || t.starts_with("tag_flash") => "gun",
        _ => "torso_upper",
    }
}

/// Damage reaching an actor: damage fields, `"damage"`, then death, or pain when
/// `allowpain` and the hit was big enough.
pub(crate) fn damage(
    world: &mut World,
    id: ActorId,
    object: u64,
    hit: &EntityHit,
    attacker: Value,
    weapon: &str,
    tag: &str,
) {
    let now = now_ms(world);
    let Some((team, dying, allow_pain, allow_death, min_pain)) =
        world.resource::<ActorPool>().actors.get(&id).map(|a| {
            (
                a.team.clone(),
                a.dying.is_some(),
                a.float_field("allowpain") != 0.0,
                a.float_field("allowdeath") != 0.0
                    || !a
                        .animscript
                        .as_ref()
                        .is_some_and(|(name, _)| matches!(&**name, "scripted" | "custom")),
                a.float_field("minpaindamage"),
            )
        })
    else {
        return;
    };
    if dying {
        return;
    }
    let attacker_object = match attacker {
        Value::Object(o) => Some(o),
        _ => None,
    };
    if let Some(other) = attacker_object.and_then(|o| super::actors::actor_of(world, o))
        && other != id
        && world
            .resource::<ActorPool>()
            .actors
            .get(&other)
            .is_some_and(|a| a.team == team && a.float_field("dodamagetoall") == 0.0)
    {
        return;
    }
    let location = hitloc_of_tag(tag);
    let hitloc = weapon_iw4::HITLOC_NAMES
        .iter()
        .position(|&name| name == location)
        .unwrap_or(0) as u8;
    let scale = if hit.flags & crate::script_player::IDFLAGS_RADIUS == 0 && hit.means != "MOD_MELEE"
    {
        FrameWorld::from_world(world)
            .combat_facts_for(hit.weapon)
            .map_or(1.0, |facts| facts.location_scale(hitloc))
    } else {
        1.0
    };
    let amount = ((hit.amount as f32) * scale).max(1.0) as i32;
    let means = if location == "head" || location == "helmet" {
        if hit.means.ends_with("_BULLET") {
            "MOD_HEAD_SHOT"
        } else {
            hit.means
        }
    } else {
        hit.means
    };
    let dir = normalized(hit.dir);
    let yaw = super::players::entity_field(world, object, "angles");
    let facing = match yaw {
        Value::Vector(v) => v[1],
        _ => 0.0,
    };
    let damage_yaw =
        math_iw4::angle_normalize_360(dir[1].atan2(dir[0]).to_degrees() - facing + 180.0) - 180.0;
    let before = health(world, object);
    let mut after = before - amount;
    if !allow_death && after <= 0 {
        after = 1;
    }
    {
        let mut runtime = world.resource_mut::<Runtime>();
        runtime.set_object_field(object, "health", Value::Int(after));
    }
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.fields.insert("damagetaken", Value::Int(amount));
        a.fields.insert("damagedir", Value::Vector(dir));
        a.fields
            .insert("damageyaw", Value::Int(damage_yaw.round() as i32));
        a.fields.insert("damagelocation", Value::string(location));
        a.fields.insert("damageweapon", Value::string(weapon));
        a.fields.insert("damagemod", Value::string(means));
        a.hits_taken += 1;
        if let Some(o) = attacker_object {
            a.last_attacker = Some((o, now));
            a.fields.insert("lastattacker", Value::Object(o));
        }
    }
    if let Some(o) = attacker_object
        && world.resource::<Runtime>().live(&o)
        && o != object
    {
        learn(world, id, o, false, now);
    }
    let model = match world
        .resource_mut::<Runtime>()
        .object_field(object, "model")
    {
        Value::String(model) => model,
        _ => "".into(),
    };
    diag::info!(
        Sim,
        "actor: {} damaged {amount} ({}) at {location} health {before}->{after} by {} means={means}",
        label(world, object),
        hit.amount,
        attacker_object.map_or_else(|| "world".into(), |o| label(world, o)),
    );
    let receiver = Value::Object(object);
    raise(
        world,
        receiver.clone(),
        "damage",
        vec![
            Value::Int(amount),
            attacker.clone(),
            Value::Vector(dir),
            Value::Vector(hit.point),
            Value::string(means),
            Value::String(model),
            Value::string(tag),
            Value::string(location),
            Value::Int(hit.flags),
            Value::string(weapon),
        ],
    );
    if after <= 0 {
        kill_actor(world, id, object, attacker, means, weapon);
        return;
    }
    let busy = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.animscript.as_ref().map(|(name, _)| name.clone()))
        .is_some_and(|name| matches!(&*name, "pain" | "death" | "scripted" | "custom"));
    let linked = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .is_some_and(|e| e.linked_to.is_some());
    if allow_pain && !busy && !linked && amount as f32 >= min_pain {
        switch_animscript(world, id, object, "pain".into(), now);
    }
}

/// The actor dies: `"death"` (attacker, means, weapon), the death animscript, and
/// a corpse once it ends.
pub(crate) fn kill_actor(
    world: &mut World,
    id: ActorId,
    object: u64,
    attacker: Value,
    means: &str,
    weapon: &str,
) {
    let now = now_ms(world);
    let Some(shots) = world
        .resource_mut::<ActorPool>()
        .actors
        .get_mut(&id)
        .and_then(|a| {
            if a.dying.is_some() {
                return None;
            }
            a.dying = Some(Dying { since_ms: now });
            a.enemy = None;
            Some(a.shots)
        })
    else {
        return;
    };
    super::actor_nav::stop(world, id);
    if health(world, object) > 0 {
        world
            .resource_mut::<Runtime>()
            .set_object_field(object, "health", Value::Int(0));
    }
    let by = match &attacker {
        Value::Object(o) => label(world, *o),
        _ => "world".into(),
    };
    diag::info!(
        Sim,
        "actor: {} died by {by} means={means} weapon={weapon} shots_fired={shots}",
        label(world, object)
    );
    raise(
        world,
        Value::Object(object),
        "death",
        vec![attacker, Value::string(means), Value::string(weapon)],
    );
    switch_animscript(world, id, object, "death".into(), now);
}

fn corpse_limit(world: &World) -> usize {
    world
        .resource::<Runtime>()
        .dvars
        .get("ai_corpsecount")
        .and_then(|text| text.trim().parse::<usize>().ok())
        .unwrap_or(CORPSE_LIMIT)
}

/// The dead actor becomes `ActorCorpse`: it keeps its model and last pose, stops
/// thinking and blocking, and joins the capped body queue.
fn become_corpse(world: &mut World, id: ActorId, object: u64) {
    super::actor_nav::release_all(world, id);
    world.resource_mut::<ActorPool>().actors.remove(&id);
    let live = {
        let mut runtime = world.resource_mut::<Runtime>();
        match runtime.entities.get_mut(&object) {
            Some(entity) => {
                entity.kind = EntityKind::ActorCorpse;
                entity.solid = false;
                entity.can_damage = false;
                true
            }
            None => false,
        }
    };
    if !live {
        return;
    }
    let limit = corpse_limit(world);
    let mut evicted = Vec::new();
    {
        let mut pool = world.resource_mut::<ActorPool>();
        pool.corpses.push_back(object);
        while pool.corpses.len() > limit {
            evicted.extend(pool.corpses.pop_front());
        }
    }
    {
        let mut runtime = world.resource_mut::<Runtime>();
        for old in &evicted {
            if runtime.live(old) && !runtime.pending_deletes.contains(old) {
                runtime.pending_deletes.push(*old);
            }
        }
    }
    let queued = world.resource::<ActorPool>().corpses.len();
    diag::info!(
        Sim,
        "actor: {} is a corpse (body queue {queued})",
        label(world, object)
    );
}

/// AI-vs-player accuracy by distance where the weapon's graph is not loaded.
fn accuracy_at(dist: f32) -> f32 {
    const GRAPH: [(f32, f32); 5] = [
        (0.0, 1.0),
        (300.0, 0.9),
        (1000.0, 0.55),
        (2000.0, 0.3),
        (4000.0, 0.1),
    ];
    for pair in GRAPH.windows(2) {
        let ((d0, a0), (d1, a1)) = (pair[0], pair[1]);
        if dist <= d1 {
            return a0 + (a1 - a0) * ((dist - d0) / (d1 - d0)).clamp(0.0, 1.0);
        }
    }
    GRAPH[GRAPH.len() - 1].1
}

fn dvar_float(world: &World, name: &str) -> Option<f32> {
    world
        .resource::<Runtime>()
        .dvars
        .get(name)
        .and_then(|text| text.trim().parse::<f32>().ok())
}

/// Where to aim at a sentient: a player's chest, an actor's eye height less a bit.
fn shoot_at_pos(world: &mut World, target: u64) -> [f32; 3] {
    let at = origin(world, target);
    let top = eye(world, target);
    [at[0], at[1], at[2] + (top[2] - at[2]) * 0.8]
}

/// `shoot( accuracyMod, shootOverride )`: one round from the muzzle. At the enemy
/// the hit is rolled from `accuracy` × mod × distance graph × the target's
/// `attackeraccuracy`; a miss passes beside it. An override position is shot as given.
fn shoot(
    world: &mut World,
    receiver: &Value,
    args: &[Value],
    blank: bool,
) -> Result<Value, String> {
    let (id, object) = receiver_actor(world, receiver)?;
    let accuracy_mod = optional(args, 0, float)?.unwrap_or(1.0);
    let override_pos = optional(args, 1, vector)?;
    let Some((weapon_name, accuracy, enemy, shoot_pos)) =
        world.resource::<ActorPool>().actors.get(&id).map(|a| {
            (
                match a.fields.get("weapon") {
                    Some(Value::String(w)) => w.to_string(),
                    _ => String::new(),
                },
                a.float_field("accuracy"),
                a.enemy,
                match a.fields.get("shootpos") {
                    Some(Value::Vector(v)) => Some(*v),
                    _ => None,
                },
            )
        })
    else {
        return Ok(Value::Undefined);
    };
    if weapon_name.is_empty() || weapon_name == "none" {
        return Ok(Value::Undefined);
    }
    let weapon = crate::script_player::weapon_named(&FrameWorld::from_world(world), &weapon_name)?;
    let (from, muzzle_angles) = muzzle(world, object);
    let target = override_pos
        .map(|pos| (pos, None))
        .or_else(|| {
            enemy
                .filter(|e| world.resource::<Runtime>().live(e))
                .map(|e| (shoot_at_pos(world, e), Some(e)))
        })
        .or_else(|| shoot_pos.map(|pos| (pos, None)));
    let aim = match target {
        Some((pos, Some(enemy))) => {
            let dist = length(sub(pos, from));
            let dist_scale = dvar_float(world, "ai_accuracydistscale").unwrap_or(1.0);
            let attacker_accuracy = match super::actors::actor_of(world, enemy) {
                Some(other) => world
                    .resource::<ActorPool>()
                    .actors
                    .get(&other)
                    .map_or(1.0, |a| a.float_field("attackeraccuracy")),
                None => number_field(world, enemy, "attackeraccuracy").unwrap_or(1.0),
            };
            let final_accuracy =
                (accuracy * accuracy_mod * accuracy_at(dist * dist_scale) * attacker_accuracy)
                    .clamp(0.0, 1.0);
            let (roll, side, lift) = {
                let mut pool = world.resource_mut::<ActorPool>();
                (pool.random(), pool.random(), pool.random())
            };
            if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
                a.fields
                    .insert("finalaccuracy", Value::Float(final_accuracy));
            }
            if roll < final_accuracy {
                pos
            } else {
                let dir = normalized(sub(pos, from));
                let right = normalized([-dir[1], dir[0], 0.0]);
                let spread = 24.0 + 40.0 * side;
                let sign = if side < 0.5 { -1.0 } else { 1.0 };
                let up = (lift - 0.3) * 48.0;
                [
                    pos[0] + right[0] * spread * sign,
                    pos[1] + right[1] * spread * sign,
                    pos[2] + up,
                ]
            }
        }
        Some((pos, None)) => pos,
        None => {
            let (forward, _, _) = math_iw4::angle_vectors(muzzle_angles);
            [
                from[0] + forward[0] * 1024.0,
                from[1] + forward[1] * 1024.0,
                from[2] + forward[2] * 1024.0,
            ]
        }
    };
    let shots = world
        .resource_mut::<ActorPool>()
        .actors
        .get_mut(&id)
        .map(|a| {
            a.shots += 1;
            a.shots
        });
    if shots == Some(1) {
        diag::info!(
            Sim,
            "actor: {} first shot weapon={weapon_name} at {}",
            label(world, object),
            enemy.map_or_else(|| "position".into(), |e| label(world, e))
        );
    }
    if blank {
        return Ok(Value::Undefined);
    }
    let Some(presence) = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .and_then(|e| e.presence)
    else {
        return Ok(Value::Undefined);
    };
    let seed = (world.resource_mut::<ActorPool>().random() * 16_777_216.0) as u32;
    let tick = world.resource::<crate::step::StepRequest>().tick;
    let velocity = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map_or([0.0; 3], |a| a.velocity);
    let shot = crate::AcceptedShot {
        shot_id: crate::ShotId(0),
        attacker: crate::Attacker::Entity(presence),
        attacker_life: crate::LifeSequence::default(),
        hand: 0,
        weapon,
        ammo_used: 1,
        origin: from,
        angles: math_iw4::vect_to_angles(sub(aim, from)),
        ads_frac: 0.0,
        view_height_current: 0.0,
        aim_spread_scale: 0.0,
        perks0: 0,
        combat_seed: seed,
        owner_velocity: velocity,
        spread_degrees: 0.0,
    };
    let mut frame = FrameWorld::from_world(world);
    if !frame.publishes_snapshot() {
        return Ok(Value::Undefined);
    }
    let emissions = crate::combat::phase_emit(&frame, core::slice::from_ref(&shot));
    crate::combat::phase_trace(&mut frame, tick, &emissions);
    Ok(Value::Undefined)
}

fn receiver_actor(world: &World, receiver: &Value) -> Result<(ActorId, u64), String> {
    match receiver {
        Value::Object(object) => super::actors::actor_of(world, *object)
            .map(|id| (id, *object))
            .ok_or_else(|| "receiver is not an actor".into()),
        _ => Err("receiver is not an actor".into()),
    }
}

/// `canshoot( pos, offset )`: a clear bullet line from the muzzle (raised by the
/// offset) to the point, ignoring the shooter and whatever stands at the point.
fn can_shoot(
    world: &mut World,
    object: u64,
    to: [f32; 3],
    offset: [f32; 3],
    target: Option<u64>,
) -> bool {
    let (from, _) = muzzle(world, object);
    let from = [
        from[0] + offset[0],
        from[1] + offset[1],
        from[2] + offset[2],
    ];
    let runtime = world.resource::<Runtime>();
    let ignore = TraceIgnore {
        client: target
            .and_then(|t| runtime.player_client(t))
            .map(crate::ClientId),
        other_client: None,
        model: runtime.entities.get(&object).and_then(|e| e.presence),
    };
    match entity_trace(world, from, to, MASK_SHOT, ignore) {
        TraceOutcome::Miss { .. } => true,
        TraceOutcome::Hit { collider, .. } => target.is_some_and(|t| {
            super::natives::engine::collider_entity(world, collider) == Value::Object(t)
        }),
        _ => false,
    }
}

fn actor_or_corpse(world: &World, receiver: &Value) -> bool {
    world
        .resource::<Runtime>()
        .entity(receiver)
        .is_some_and(|(_, e)| matches!(e.kind, EntityKind::Actor(_) | EntityKind::ActorCorpse))
}

fn known_of(world: &World, id: ActorId, target: u64) -> Option<Known> {
    world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.known.get(&target).copied())
}

fn enemy_of(world: &World, id: ActorId) -> Option<u64> {
    world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.enemy)
}

/// Engine reads of the perception fields: `enemy`, `lastenemysightpos`, `lastattacker`.
pub(crate) fn load_field(world: &mut World, id: ActorId, name: &str) -> Option<Value> {
    let live = |world: &World, o: u64| world.resource::<Runtime>().live(&o);
    match name {
        "enemy" => Some(
            enemy_of(world, id)
                .filter(|e| live(world, *e))
                .map_or(Value::Undefined, Value::Object),
        ),
        "lastenemysightpos" => {
            let written = world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .and_then(|a| a.fields.get("lastenemysightpos").cloned());
            if let Some(value) = written {
                return Some(value);
            }
            Some(
                enemy_of(world, id)
                    .and_then(|e| known_of(world, id, e))
                    .and_then(|k| k.sight_pos)
                    .map_or(Value::Undefined, Value::Vector),
            )
        }
        "lastattacker" => Some(
            world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .and_then(|a| a.last_attacker)
                .filter(|(o, _)| live(world, *o))
                .map_or(Value::Undefined, |(o, _)| Value::Object(o)),
        ),
        _ => None,
    }
}

fn enemy_sq_dists(world: &mut World, id: ActorId, object: u64) -> Vec<f32> {
    let at = origin(world, object);
    let known: Vec<Known> = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| a.known.values().copied().collect())
        .unwrap_or_default();
    known
        .iter()
        .map(|k| {
            let d = sub(k.pos, at);
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        })
        .collect()
}

fn weapon_row(world: &mut World, args: &[Value]) -> Result<weapon_iw4::WeaponCombatFacts, String> {
    let name = string(args, 0)?;
    if name == "none" || name.is_empty() {
        return Ok(Default::default());
    }
    let frame = FrameWorld::from_world(world);
    let index = crate::script_player::weapon_named(&frame, &name)?;
    Ok(frame.weapon_combat_row(index).unwrap_or_default())
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};

    registry.register(Function, "isexplosivedamagemod", |_, _, args| {
        let means = string(args, 0)?.to_ascii_uppercase();
        Ok(Value::Int(
            matches!(
                means.as_str(),
                "MOD_GRENADE"
                    | "MOD_GRENADE_SPLASH"
                    | "MOD_PROJECTILE"
                    | "MOD_PROJECTILE_SPLASH"
                    | "MOD_EXPLOSIVE"
            )
            .into(),
        ))
    });
    // There is no ragdoll yet: an actor or its corpse holds the death pose.
    registry.register(Method, "startragdoll", |world, receiver, _| {
        if !actor_or_corpse(world, receiver) {
            super::natives::player::corpse_anim(world, receiver)?;
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "isragdoll", |world, receiver, _| {
        if !actor_or_corpse(world, receiver) {
            super::natives::player::corpse_anim(world, receiver)?;
        }
        Ok(Value::Int(0))
    });
    // No throw solution is offered (grenades are a later stage): undefined.
    for name in [
        "checkgrenadethrow",
        "checkgrenadethrowpos",
        "checkgrenadelaunch",
        "checkgrenadelaunchpos",
    ] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    // No reacquire path is offered: the caller falls back to its other options.
    for name in ["findreacquiredirectpath", "findreacquireproximatepath"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Int(0))
        });
    }
    for name in ["reacquiremove", "trimpathtoattack"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    // No sidestep to reacquire: the caller tries its other options.
    registry.register(Method, "reacquirestep", |world, receiver, args| {
        receiver_actor(world, receiver)?;
        float(args, 0)?;
        Ok(Value::Int(0))
    });
    // `isgrenadepossafe( target, pos )`: no live squadmate within the blast radius.
    registry.register(Method, "isgrenadepossafe", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let pos = vector(args, 1)?;
        let team = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map(|a| a.team.clone());
        let mates: Vec<u64> = world
            .resource::<ActorPool>()
            .actors
            .values()
            .filter(|a| Some(&a.team) == team.as_ref() && a.dying.is_none() && a.object != object)
            .map(|a| a.object)
            .collect();
        let safe = mates
            .into_iter()
            .all(|mate| length(sub(origin(world, mate), pos)) > 256.0);
        Ok(Value::Int(safe.into()))
    });
    // `fire_type`: 0 full auto, 1 single shot, 2-4 bursts of that many rounds.
    registry.register(Function, "weaponisauto", |world, _, args| {
        Ok(Value::Int((weapon_row(world, args)?.fire_type == 0).into()))
    });
    registry.register(Function, "weaponburstcount", |world, _, args| {
        let fire_type = weapon_row(world, args)?.fire_type;
        Ok(Value::Int(if (2..=4).contains(&fire_type) {
            fire_type
        } else {
            0
        }))
    });
    registry.register(Function, "weaponisboltaction", |world, _, args| {
        Ok(Value::Int(weapon_row(world, args)?.bolt_action.into()))
    });
    // The motion direction relative to facing, in [-180, 180).
    registry.register(Method, "getmotionangle", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let motion = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map_or([0.0; 3], |a| {
                if length(a.velocity) > 1.0 {
                    a.velocity
                } else {
                    a.lookahead_dir
                }
            });
        if motion[0] == 0.0 && motion[1] == 0.0 {
            return Ok(Value::Float(0.0));
        }
        let facing = match super::players::entity_field(world, object, "angles") {
            Value::Vector(v) => v[1],
            _ => 0.0,
        };
        let yaw = motion[1].atan2(motion[0]).to_degrees() - facing;
        Ok(Value::Float(
            math_iw4::angle_normalize_360(yaw + 180.0) - 180.0,
        ))
    });
    // The floor under a point (default the actor's origin), or undefined over a drop.
    registry.register(Method, "getdroptofloorposition", |world, receiver, args| {
        let object = entity_id(world, receiver)?;
        let at = match optional(args, 0, vector)? {
            Some(at) => at,
            None => origin(world, object),
        };
        let from = [at[0], at[1], at[2] + 18.0];
        let to = [at[0], at[1], at[2] - 128.0];
        let trace = super::presence::settled(world).trace_world(
            from,
            to,
            [-15.0, -15.0, 0.0],
            [15.0, 15.0, 72.0],
            1,
        );
        if trace.startsolid != 0 || trace.fraction >= 1.0 {
            return Ok(Value::Undefined);
        }
        Ok(Value::Vector(std::array::from_fn(|i| {
            from[i] + (to[i] - from[i]) * trace.fraction
        })))
    });

    registry.register(Method, "shoot", |world, receiver, args| {
        shoot(world, receiver, args, false)
    });
    registry.register(Method, "shootblank", |world, receiver, args| {
        shoot(world, receiver, args, true)
    });
    registry.register(Method, "canshoot", |world, receiver, args| {
        let (_, object) = receiver_actor(world, receiver)?;
        let to = vector(args, 0)?;
        let offset = optional(args, 1, vector)?.unwrap_or([0.0; 3]);
        Ok(Value::Int(
            can_shoot(world, object, to, offset, None).into(),
        ))
    });
    registry.register(Method, "canshootenemy", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let Some(enemy) = enemy_of(world, id).filter(|e| world.resource::<Runtime>().live(e))
        else {
            return Ok(Value::Int(0));
        };
        let to = shoot_at_pos(world, enemy);
        Ok(Value::Int(
            can_shoot(world, object, to, [0.0; 3], Some(enemy)).into(),
        ))
    });
    registry.register(Method, "canattackenemynode", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let Some(enemy) = enemy_of(world, id).filter(|e| world.resource::<Runtime>().live(e))
        else {
            return Ok(Value::Int(0));
        };
        let to = shoot_at_pos(world, enemy);
        Ok(Value::Int(
            can_shoot(world, object, to, [0.0; 3], Some(enemy)).into(),
        ))
    });
    registry.register(Method, "seerecently", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        let window = (float(args, 1)? * 1000.0) as i64;
        let now = now_ms(world);
        let seen = known_of(world, id, target)
            .and_then(|k| k.seen_ms)
            .is_some_and(|t| now - t <= window);
        Ok(Value::Int(seen.into()))
    });
    registry.register(Method, "lastknowntime", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        Ok(known_of(world, id, target).map_or(Value::Int(0), |k| Value::Int(k.time_ms as i32)))
    });
    registry.register(Method, "lastknownpos", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        match known_of(world, id, target) {
            Some(k) => Ok(Value::Vector(k.pos)),
            None => Ok(Value::Vector(origin(world, target))),
        }
    });
    registry.register(Method, "getenemyinfo", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        let now = now_ms(world);
        learn(world, id, target, false, now);
        Ok(Value::Undefined)
    });
    registry.register(Method, "clearenemy", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        if let Some(enemy) = enemy_of(world, id)
            && let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id)
        {
            a.known.remove(&enemy);
        }
        set_enemy(world, id, object, None);
        Ok(Value::Undefined)
    });
    registry.register(Method, "getenemysqdist", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let Some(enemy) = enemy_of(world, id) else {
            return Ok(Value::Float(f32::MAX));
        };
        let d = sub(origin(world, enemy), origin(world, object));
        Ok(Value::Float(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]))
    });
    registry.register(Method, "getclosestenemysqdist", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let best = enemy_sq_dists(world, id, object)
            .into_iter()
            .fold(f32::MAX, f32::min);
        Ok(Value::Float(best))
    });
    registry.register(Method, "isknownenemyinradius", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let center = vector(args, 0)?;
        let radius = float(args, 1)?;
        let inside = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .is_some_and(|a| {
                a.known
                    .values()
                    .any(|k| length(sub(k.pos, center)) <= radius)
            });
        Ok(Value::Int(inside.into()))
    });
    registry.register(Method, "isknownenemyinvolume", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let volume = entity_id(world, arg(args, 0)?)?;
        let points: Vec<[f32; 3]> = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map(|a| a.known.values().map(|k| k.pos).collect())
            .unwrap_or_default();
        let inside = points
            .into_iter()
            .any(|p| super::triggers::contains_point(world, volume, p));
        Ok(Value::Int(inside.into()))
    });
    registry.register(Method, "setentitytarget", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        let now = now_ms(world);
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            a.entity_target = Some(target);
        }
        learn(world, id, target, false, now);
        set_enemy(world, id, object, Some(target));
        Ok(Value::Undefined)
    });
    registry.register(Method, "clearentitytarget", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let target = world
            .resource_mut::<ActorPool>()
            .actors
            .get_mut(&id)
            .and_then(|a| a.entity_target.take());
        if target.is_some() && enemy_of(world, id) == target {
            set_enemy(world, id, object, None);
        }
        Ok(Value::Undefined)
    });
    for name in ["issuppressed", "ismovesuppressed", "issuppressionwaiting"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Int(0))
        });
    }
    for name in [
        "updateplayersightaccuracy",
        "clearpotentialthreat",
        "flagenemyunattackable",
        "stoplookat",
        "setlookatyawlimits",
        "setlookatanimnodes",
        "settalktospecies",
    ] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    for name in ["setpotentialthreat", "setlookat", "setlookatentity"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    registry.register(Method, "makeentitysentient", |world, receiver, args| {
        let object = entity_id(world, receiver)?;
        let team = string(args, 0)?.to_ascii_lowercase();
        world
            .resource_mut::<ActorPool>()
            .sentients
            .insert(object, team.into());
        Ok(Value::Undefined)
    });
    registry.register(Method, "freeentitysentient", |world, receiver, _| {
        let object = entity_id(world, receiver)?;
        world.resource_mut::<ActorPool>().sentients.remove(&object);
        Ok(Value::Undefined)
    });
    registry.register(Method, "getshootatpos", |world, receiver, _| {
        let object = entity_id(world, receiver)?;
        Ok(Value::Vector(shoot_at_pos(world, object)))
    });
    registry.register(Method, "kill", |world, receiver, args| {
        let Ok((id, object)) = receiver_actor(world, receiver) else {
            return super::natives::sp_player::kill_player(world, receiver);
        };
        let attacker = args.get(1).cloned().unwrap_or(Value::Undefined);
        kill_actor(world, id, object, attacker, "MOD_SUICIDE", "none");
        Ok(Value::Undefined)
    });
    registry.register(Method, "dropweapon", |world, receiver, args| {
        receiver_actor(world, receiver)?;
        string(args, 0)?;
        Ok(Value::Undefined)
    });
    registry.register(Function, "getcorpsearray", |world, _, _| {
        let corpses: Vec<Value> = {
            let runtime = world.resource::<Runtime>();
            world
                .resource::<ActorPool>()
                .corpses
                .iter()
                .filter(|o| runtime.live(o))
                .map(|o| Value::Object(*o))
                .collect()
        };
        super::arrays::new_array(world, corpses)
    });
}
