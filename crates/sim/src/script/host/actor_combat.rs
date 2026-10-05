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
/// A known enemy not seen or reported for this long is forgotten, unless it is
/// the player the actor is fighting (IW4 has no forget time; the actor keeps
/// hunting the player's last known position).
const FORGET_MS: i64 = 20_000;
/// Squadmates this close hear about the enemy an actor sees (`ai_eventDistNewEnemy`).
const NEW_ENEMY_DIST: f32 = 1024.0;
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

pub(crate) fn label(world: &World, object: u64) -> String {
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
pub(crate) struct Sentient {
    pub(crate) object: u64,
    pub(crate) team: Arc<str>,
}

/// Live players, actors and script sentients, players first then entnum order.
pub(crate) fn sentients(world: &mut World) -> Vec<Sentient> {
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
        let visible = super::sentients::max_visible_dist(world, enemy);
        let client = world.resource::<Runtime>().player_client(enemy);
        let stance = client.map_or("", |client| {
            crate::script_player::stance(&FrameWorld::from_world(world), crate::ClientId(client))
        });
        diag::info!(
            Sim,
            "actor: {} acquired enemy {} dist={:.0} maxvisibledist={visible:.0} {stance}",
            label(world, object),
            label(world, enemy),
            length(sub(to, from))
        );
    } else {
        diag::info!(Sim, "actor: {} has no enemy", label(world, object));
    }
    raise(world, Value::Object(object), "enemy", Vec::new());
}

/// Records that `id` knows where `target` is now; `seen` when by its own eyes.
pub(crate) fn learn(world: &mut World, id: ActorId, target: u64, seen: bool, now: i64) {
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

/// `AI_EV_NEW_ENEMY`: squadmates within `ai_eventDistNewEnemy` of `id` that do not
/// already know better hear about the enemy it sees.
fn share(world: &mut World, id: ActorId, object: u64, target: u64, now: i64) {
    let at = origin(world, object);
    let dist = dvar_float(world, "ai_eventDistNewEnemy").unwrap_or(NEW_ENEMY_DIST);
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
        if length(sub(origin(world, mate_object), at)) <= dist {
            learn(world, mate, target, false, now);
        }
    }
}

/// Spawner flag `ENEMYINFO` (8): the actor spawns knowing where every hostile
/// sentient is, so it turns to and engages them instead of idling until it
/// happens to see one.
pub(crate) const SPAWNFLAG_ENEMYINFO: i32 = 8;

pub(crate) fn spawn_enemy_info(world: &mut World, id: ActorId, object: u64) {
    let Some(team) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| a.team.clone())
    else {
        return;
    };
    let now = now_ms(world);
    let targets: Vec<u64> = sentients(world)
        .into_iter()
        .filter(|s| s.object != object && hostile(&team, &s.team))
        .map(|s| s.object)
        .collect();
    for target in targets {
        if !ignores(world, object, target) {
            learn(world, id, target, false, now);
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
    let was_visible = current.is_some_and(|enemy| {
        world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .and_then(|a| a.sight.get(&enemy))
            .is_some_and(|(_, seen)| *seen)
    });
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
        let tracked =
            current == Some(target) && world.resource::<Runtime>().player_client(target).is_some();
        if !still || (!tracked && now - info.time_ms > FORGET_MS) {
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
    let enemy = best.map(|(_, t)| t);
    set_enemy(world, id, object, enemy);
    if let Some(enemy) = enemy
        && seen_any.contains(&enemy)
    {
        share(world, id, object, enemy, now);
        if current != Some(enemy) || !was_visible {
            diag::info!(
                Sim,
                "actor: {} sees its enemy {}",
                label(world, object),
                label(world, enemy)
            );
            raise(world, Value::Object(object), "enemy_visible", Vec::new());
        }
    }
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
    suppression(world, actors, now);
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

/// A bullet line this close to an actor's chest passes it.
const WHIZ_DIST: f32 = 96.0;
const SUPPRESSION_WAIT_MS: i64 = 2000;
const SUPPRESSION_DURATION_MS: i64 = 5000;

fn segment_dist(point: [f32; 3], start: [f32; 3], end: [f32; 3]) -> (f32, f32) {
    let d = sub(end, start);
    let len_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let t = if len_sq > 0.0 {
        ((point[0] - start[0]) * d[0] + (point[1] - start[1]) * d[1] + (point[2] - start[2]) * d[2])
            / len_sq
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);
    let closest = [
        start[0] + d[0] * t,
        start[1] + d[1] * t,
        start[2] + d[2] * t,
    ];
    (length(sub(point, closest)), t)
}

pub(crate) fn shooter_object(world: &World, attacker: crate::Attacker) -> Option<u64> {
    let runtime = world.resource::<Runtime>();
    match attacker {
        crate::Attacker::Client(client) => runtime.players.get(&client.0).map(|s| s.object),
        crate::Attacker::Entity(id) => runtime.presented_by(id),
    }
}

/// `Actor_AddSuppressionLine` / `Actor_DecaySuppressionLines`: hostile bullets
/// passing close feed `suppressionmeter`; at a cover node they suppress
/// (`"suppression"`, `suppressionstarttime`) for `suppressionduration`, elsewhere
/// they only whiz by (`"bulletwhizby"`).
fn suppression(world: &mut World, actors: &[(i32, ActorId, u64)], now: i64) {
    let lines = std::mem::take(&mut world.resource_mut::<ActorPool>().whizzes);
    for (start, end, attacker) in lines {
        let Some(shooter) = shooter_object(world, attacker) else {
            continue;
        };
        let shooter_team = match super::actors::actor_of(world, shooter) {
            Some(other) => world
                .resource::<ActorPool>()
                .actors
                .get(&other)
                .map(|a| a.team.to_string()),
            None => match super::players::entity_field(world, shooter, "team") {
                Value::String(team) => Some(team.to_string()),
                _ => Some("allies".into()),
            },
        };
        for (_, id, object) in actors {
            if *object == shooter {
                continue;
            }
            let Some((team, ignore)) = world
                .resource::<ActorPool>()
                .actors
                .get(id)
                .map(|a| (a.team.clone(), a.float_field("ignoresuppression") != 0.0))
            else {
                continue;
            };
            if !shooter_team.as_deref().is_some_and(|t| hostile(&team, t)) {
                continue;
            }
            let chest = {
                let at = origin(world, *object);
                [at[0], at[1], at[2] + 48.0]
            };
            let (dist, t) = segment_dist(chest, start, end);
            if dist > WHIZ_DIST || t >= 1.0 {
                continue;
            }
            let at_cover = super::actor_cover::near_claimed(world, *id, *object, 32.0);
            let started = world
                .resource_mut::<ActorPool>()
                .actors
                .get_mut(id)
                .map(|a| {
                    a.suppression = (a.suppression + 0.15).min(1.0);
                    if at_cover && !ignore {
                        a.suppressed_ms = now;
                        if a.suppressed_since == 0 {
                            a.suppressed_since = now;
                            return true;
                        }
                    }
                    false
                });
            let note = if at_cover && !ignore {
                "suppression"
            } else {
                "bulletwhizby"
            };
            if started == Some(true) {
                diag::info!(
                    Sim,
                    "actor: {} suppressed by {} at cover",
                    label(world, *object),
                    label(world, shooter)
                );
            }
            raise(
                world,
                Value::Object(*object),
                note,
                vec![Value::Object(shooter)],
            );
        }
    }
    for (_, id, object) in actors {
        let ended = world
            .resource_mut::<ActorPool>()
            .actors
            .get_mut(id)
            .is_some_and(|a| {
                a.suppression = (a.suppression - 0.01).max(0.0);
                let duration = a
                    .fields
                    .get("suppressionduration")
                    .map_or(SUPPRESSION_DURATION_MS, |_| {
                        a.float_field("suppressionduration") as i64
                    });
                if a.suppressed_since > 0 && now - a.suppressed_ms >= duration {
                    a.suppressed_since = 0;
                    return true;
                }
                false
            });
        if ended {
            raise(world, Value::Object(*object), "suppression_end", Vec::new());
        }
    }
}

fn suppression_state(world: &World, id: ActorId) -> Option<(i64, i64, f32, bool, i64)> {
    world.resource::<ActorPool>().actors.get(&id).map(|a| {
        (
            a.suppressed_since,
            a.suppressed_ms,
            a.suppression,
            a.float_field("ignoresuppression") != 0.0,
            a.fields
                .get("suppressionwait")
                .map_or(SUPPRESSION_WAIT_MS, |_| {
                    a.float_field("suppressionwait") as i64
                }),
        )
    })
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
    let at = origin(world, object);
    super::actor_events::push_casualty(
        world,
        crate::actor::AiEvent::Pain,
        object,
        attacker_object,
        at,
    );
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
        let pains = world
            .resource_mut::<ActorPool>()
            .actors
            .get_mut(&id)
            .map_or(0, |a| {
                a.pains += 1;
                a.pains
            });
        diag::info!(
            Sim,
            "actor: {} pain {pains} ({amount} at {location}, yaw {damage_yaw:.0})",
            label(world, object)
        );
        switch_animscript(world, id, object, "pain".into(), now);
    } else if !allow_pain || busy {
        diag::debug!(
            Sim,
            "actor: {} no pain (allowpain={allow_pain} busy={busy} linked={linked})",
            label(world, object)
        );
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
    let at = origin(world, object);
    let attacker_object = match attacker {
        Value::Object(o) => Some(o),
        _ => None,
    };
    super::actor_events::push_casualty(
        world,
        crate::actor::AiEvent::Death,
        object,
        attacker_object,
        at,
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

/// AI accuracy by distance for a weapon without accuracy graphs (MP stand-ins).
fn fallback_accuracy_at(dist: f32) -> f32 {
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

/// `IW4L_AI_ACCURACY_LOG=1`: one line per actor shot at a sentient with every
/// accuracy term, the distance and whether the roll hit.
fn accuracy_log() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var("IW4L_AI_ACCURACY_LOG").is_ok_and(|v| v == "1"))
}

/// Field the last `updateplayersightaccuracy` result is kept under (1 until then).
const SIGHT_ACCURACY_FIELD: &str = "playersightaccuracy";

/// The terms `Actor_GetFinalAccuracy` multiplies.
struct AccuracyTerms {
    dist: f32,
    vs_player: bool,
    graphed: bool,
    weapon: f32,
    attacker: f32,
    stance: f32,
    movement: f32,
    sight: f32,
    attacker_count: f32,
}

/// Hostile actors whose enemy is `player` and that see it now: the player's
/// attacker count (each actor's sight result may be up to 250 ms old).
pub(crate) fn player_attacker_count(world: &mut World, player: u64) -> usize {
    let attackers: Vec<(i32, ActorId, u64)> = {
        let runtime = world.resource::<Runtime>();
        let mut rows: Vec<(i32, ActorId, u64)> = world
            .resource::<ActorPool>()
            .actors
            .iter()
            .filter(|(_, a)| a.dying.is_none() && a.enemy == Some(player))
            .filter_map(|(id, a)| Some((runtime.entities.get(&a.object)?.number, *id, a.object)))
            .collect();
        rows.sort_by_key(|(number, _, _)| *number);
        rows
    };
    attackers
        .into_iter()
        .filter(|&(_, id, object)| can_see(world, id, object, player, 250))
        .count()
}

/// `ai_accuracy_attackerCountDecrease` per attacker past the first, up to
/// `ai_accuracy_attackerCountMax` attackers.
fn attacker_count_accuracy(world: &mut World, player: u64) -> f32 {
    let count = player_attacker_count(world, player);
    if count <= 1 {
        return 1.0;
    }
    let decrease = dvar_float(world, "ai_accuracy_attackercountdecrease").unwrap_or(0.75);
    let max = dvar_float(world, "ai_accuracy_attackercountmax").map_or(4, |v| v.max(0.0) as usize);
    decrease.powi(count.min(max) as i32 - 1)
}

/// `Actor_GetFinalAccuracy`: the actor's `accuracy` × the target's
/// `attackeraccuracy` × the script's mod × the weapon graph at the sentients'
/// distance / 4000 (AI-vs-player distance scaled by `ai_accuracyDistScale`).
/// Against a player also: stance (prone 0.5, crouch 0.75), lateral movement
/// (1 − speed/250, at least 0.3), the last `updateplayersightaccuracy` and,
/// unless `noattackeraccuracymod`, the attacker-count decrease. Clamped to 0..1.
fn final_accuracy(
    world: &mut World,
    id: ActorId,
    object: u64,
    enemy: u64,
    weapon: u32,
    accuracy: f32,
    accuracy_mod: f32,
) -> (f32, AccuracyTerms) {
    let from = origin(world, object);
    let to = origin(world, enemy);
    let offset = sub(to, from);
    let dist = length(offset);
    let player = world
        .resource::<Runtime>()
        .player_client(enemy)
        .map(crate::ClientId);
    let graph_dist = match player {
        Some(_) => dist * dvar_float(world, "ai_accuracydistscale").unwrap_or(1.0),
        None => dist,
    };
    let graph = FrameWorld::from_world(world)
        .weapon_ai_accuracy(weapon)
        .and_then(|row| row.graph(player.is_some()).copied());
    let weapon_accuracy = graph.map_or_else(
        || fallback_accuracy_at(graph_dist),
        |graph| graph.value_at_distance(graph_dist),
    );
    let attacker = match super::actors::actor_of(world, enemy) {
        Some(other) => world
            .resource::<ActorPool>()
            .actors
            .get(&other)
            .map_or(1.0, |a| a.float_field("attackeraccuracy")),
        None => number_field(world, enemy, "attackeraccuracy").unwrap_or(1.0),
    };
    let mut terms = AccuracyTerms {
        dist,
        vs_player: player.is_some(),
        graphed: graph.is_some(),
        weapon: weapon_accuracy,
        attacker,
        stance: 1.0,
        movement: 1.0,
        sight: 1.0,
        attacker_count: 1.0,
    };
    if let Some(client) = player {
        let (pm_flags, velocity) = FrameWorld::from_world(world)
            .player(client)
            .map_or((0, [0.0; 3]), |ps| (ps.pm_flags, ps.velocity));
        terms.stance = if pm_flags & playerstate_iw4::pm_flags::PRONE != 0 {
            0.5
        } else if pm_flags & playerstate_iw4::pm_flags::CROUCH != 0 {
            0.75
        } else {
            1.0
        };
        let dir = normalized(offset);
        let lateral = (dir[1] * velocity[0] - dir[0] * velocity[1] + dir[2] * velocity[2]).abs();
        terms.movement = (1.0 - lateral.min(250.0) / 250.0).max(0.3);
        let (sight, no_attacker_mod) =
            world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .map_or((1.0, false), |a| {
                    (
                        match a.fields.get(SIGHT_ACCURACY_FIELD) {
                            Some(Value::Float(v)) => *v,
                            _ => 1.0,
                        },
                        a.float_field("noattackeraccuracymod") != 0.0,
                    )
                });
        terms.sight = sight;
        if !no_attacker_mod {
            terms.attacker_count = attacker_count_accuracy(world, enemy);
        }
    }
    let total = accuracy
        * terms.attacker
        * accuracy_mod
        * terms.weapon
        * terms.stance
        * terms.movement
        * terms.sight
        * terms.attacker_count;
    (total.clamp(0.0, 1.0), terms)
}

/// `updatePlayerSightAccuracy`: how much of its player enemy the actor sees,
/// from its eye to the enemy's eye and to 75/50/25% of the way from the
/// enemy's feet to its eye (10 + 30 + 30 + 30 points), as 0.5 + 0.5 × points/100.
/// 1 when the enemy is not a player.
fn update_player_sight_accuracy(world: &mut World, id: ActorId, object: u64) {
    let enemy = enemy_of(world, id).filter(|e| world.resource::<Runtime>().live(e));
    let accuracy = match enemy {
        Some(enemy) if world.resource::<Runtime>().player_client(enemy).is_some() => {
            let from = eye(world, object);
            let feet = origin(world, enemy);
            let top = eye(world, enemy);
            let ignore = {
                let runtime = world.resource::<Runtime>();
                TraceIgnore {
                    client: runtime.player_client(enemy).map(crate::ClientId),
                    other_client: None,
                    model: runtime.entities.get(&object).and_then(|e| e.presence),
                }
            };
            let mut points = 0.0;
            for (frac, worth) in [(1.0, 10.0), (0.75, 30.0), (0.5, 30.0), (0.25, 30.0)] {
                let to = [
                    feet[0] + (top[0] - feet[0]) * frac,
                    feet[1] + (top[1] - feet[1]) * frac,
                    feet[2] + (top[2] - feet[2]) * frac,
                ];
                if matches!(
                    entity_trace(world, from, to, MASK_AI_SIGHT, ignore),
                    TraceOutcome::Miss { .. }
                ) {
                    points += worth;
                }
            }
            0.5 + 0.5 * (points * 0.01)
        }
        _ => 1.0,
    };
    if accuracy_log() {
        diag::info!(
            Sim,
            "actor sight accuracy: {} {accuracy:.2}",
            label(world, object)
        );
    }
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.fields
            .insert(SIGHT_ACCURACY_FIELD, Value::Float(accuracy));
    }
}

/// Line of sight for AI (`Actor_CanSeeEntityPoint`).
const MASK_AI_SIGHT: u32 = 0x0801;

pub(crate) fn dvar_float(world: &World, name: &str) -> Option<f32> {
    world
        .resource::<Runtime>()
        .dvars
        .get(&name.to_ascii_lowercase())
        .and_then(|text| text.trim().parse::<f32>().ok())
}

/// Where to aim at a sentient: a player's chest, an actor's eye height less a bit.
fn shoot_at_pos(world: &mut World, target: u64) -> [f32; 3] {
    let at = origin(world, target);
    let top = eye(world, target);
    [at[0], at[1], at[2] + (top[2] - at[2]) * 0.8]
}

/// `shoot( accuracyMod, shootOverride )`: one round from the muzzle. At the enemy
/// the hit is rolled from `final_accuracy`; a miss passes beside it. An override
/// position is shot as given.
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
            let (final_accuracy, terms) =
                final_accuracy(world, id, object, enemy, weapon, accuracy, accuracy_mod);
            let (roll, side, lift) = {
                let mut pool = world.resource_mut::<ActorPool>();
                (pool.random(), pool.random(), pool.random())
            };
            if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
                a.fields
                    .insert("finalaccuracy", Value::Float(final_accuracy));
            }
            if accuracy_log() {
                diag::info!(
                    Sim,
                    "actor shot: {} weapon={weapon_name} at {} dist={:.0} muzzle_dist={:.0} vs={} graph={} \
                     accuracy={accuracy:.3} mod={accuracy_mod:.3} target={:.3} weapon_acc={:.3} \
                     stance={:.2} move={:.2} sight={:.2} attackers={:.3} final={final_accuracy:.3} \
                     hit={}",
                    label(world, object),
                    label(world, enemy),
                    terms.dist,
                    length(sub(pos, from)),
                    if terms.vs_player { "player" } else { "ai" },
                    terms.graphed as u8,
                    terms.attacker,
                    terms.weapon,
                    terms.stance,
                    terms.movement,
                    terms.sight,
                    terms.attacker_count,
                    (roll < final_accuracy) as u8,
                );
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
        Some((pos, None)) => {
            if accuracy_log() {
                diag::info!(
                    Sim,
                    "actor shot: {} weapon={weapon_name} at position (no accuracy roll)",
                    label(world, object)
                );
            }
            pos
        }
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
    if shots == Some(1) && accuracy_log() {
        let row = FrameWorld::from_world(world).weapon_ai_accuracy(weapon);
        diag::info!(
            Sim,
            "actor accuracy graphs: weapon={weapon_name} fightdist={:?} maxdist={:?} ai_vs_ai={:?} ai_vs_player={:?}",
            row.map(|r| r.fight_dist),
            row.map(|r| r.max_dist),
            row.and_then(|r| r.ai_vs_ai).map(|g| g.knots().to_vec()),
            row.and_then(|r| r.ai_vs_player).map(|g| g.knots().to_vec()),
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
    let number = world.resource::<Runtime>().entities[&object].number;
    let shot_id = FrameWorld::from_world(world).alloc_shot_id();
    let angles = math_iw4::vect_to_angles(sub(aim, from));
    let shot = crate::AcceptedShot {
        shot_id,
        attacker: crate::Attacker::Entity(presence),
        attacker_life: crate::LifeSequence::default(),
        hand: 0,
        weapon,
        ammo_used: 1,
        origin: from,
        angles,
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
    frame.push_entity_event(
        tick,
        crate::EventAudience::All,
        entity_iw4::predicted_weapon_fire_event(0, false),
        crate::EntityEventPayload {
            number,
            weapon,
            correlation: shot_id.0,
            origin: from,
            direction: angles,
            ..Default::default()
        },
    );
    Ok(Value::Undefined)
}

/// Reach of an actor's melee strike (`Actor_Melee` → `Weapon_Melee`).
const MELEE_RANGE: f32 = 64.0;

/// `melee( [direction] )` (`Actor_Melee`): a strike from the eye toward the
/// enemy's eye (turned to `direction` in the plane when given), or along
/// `direction` without an enemy. What it hits takes the weapon's melee damage
/// plus 0–4 (`MOD_MELEE`) and is returned; nothing, or something that takes
/// no damage, returns undefined.
fn melee(world: &mut World, receiver: &Value, args: &[Value]) -> Result<Value, String> {
    let (id, object) = receiver_actor(world, receiver)?;
    let direction = optional(args, 0, vector)?;
    let Some((weapon_name, enemy)) = world.resource::<ActorPool>().actors.get(&id).map(|a| {
        (
            match a.fields.get("weapon") {
                Some(Value::String(w)) => w.to_string(),
                _ => String::new(),
            },
            a.enemy.filter(|e| world.resource::<Runtime>().live(e)),
        )
    }) else {
        return Ok(Value::Undefined);
    };
    let from = eye(world, object);
    let forward = match (enemy, direction) {
        (Some(enemy), direction) => {
            let to = eye(world, enemy);
            let mut d = sub(to, from);
            if let Some(dir) = direction {
                let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
                let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt().max(f32::EPSILON);
                d = [dir[0] * flat / len, dir[1] * flat / len, d[2]];
            }
            normalized(d)
        }
        (None, Some(dir)) => normalized(dir),
        (None, None) => return Ok(Value::Undefined),
    };
    let end: [f32; 3] = std::array::from_fn(|i| from[i] + forward[i] * MELEE_RANGE);
    let presence = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .and_then(|e| e.presence);
    let ignore = TraceIgnore {
        model: presence,
        ..TraceIgnore::default()
    };
    let (collider, point) = match entity_trace(world, from, end, MASK_SHOT, ignore) {
        TraceOutcome::Hit { collider, end, .. } => (collider, end),
        _ => return Ok(Value::Undefined),
    };
    use crate::bullet_collision::ColliderId;
    let (target, hit) = {
        let runtime = world.resource::<Runtime>();
        match collider {
            ColliderId::Player { client, .. } => (
                crate::script::HitTarget::Player(client),
                runtime.players.get(&client.0).map(|slot| slot.object),
            ),
            ColliderId::EntityDObjBone { owner, .. }
            | ColliderId::EntityLinkedBrush { owner, .. } => match owner.script_model() {
                Some(model) => (
                    crate::script::HitTarget::Entity(model),
                    runtime.presented_by(model),
                ),
                None => return Ok(Value::Undefined),
            },
            ColliderId::World { .. } => return Ok(Value::Undefined),
        }
    };
    let Some(hit) = hit.filter(|hit| {
        world
            .resource::<Runtime>()
            .entities
            .get(hit)
            .is_some_and(|e| e.can_damage)
    }) else {
        return Ok(Value::Undefined);
    };
    if let Some(team) = super::actors::actor_of(world, hit)
        .and_then(|other| world.resource::<ActorPool>().actors.get(&other))
        .map(|other| other.team.clone())
        && world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .is_some_and(|a| a.team == team)
    {
        return Ok(Value::Undefined);
    }
    let frame = FrameWorld::from_world(world);
    let weapon = crate::script_player::weapon_named(&frame, &weapon_name).ok();
    let base = weapon
        .and_then(|w| frame.combat_facts_for(w))
        .map_or(0, |facts| facts.melee_damage);
    let amount = base + (world.resource_mut::<ActorPool>().random() * 5.0) as i32;
    diag::info!(
        Sim,
        "actor: {} melee hits {} for {amount} ({weapon_name})",
        label(world, object),
        label(world, hit)
    );
    // Applied now, as `G_Damage` is: the bite's knock-down makes the player
    // undamageable in the same frame.
    let tick = world.resource::<crate::step::StepRequest>().tick;
    let script_hit = crate::script::ScriptHit {
        piece: None,
        target,
        amount,
        origin: point,
        attacker: presence.map(crate::Attacker::Entity),
        inflictor: presence,
        means: "MOD_MELEE",
        weapon: weapon.unwrap_or(0),
        flags: 0,
        hitloc: 0,
    };
    match target {
        crate::script::HitTarget::Player(_) => {
            crate::damage::apply_script_hit(&mut FrameWorld::from_world(world), tick, &script_hit);
        }
        crate::script::HitTarget::Entity(model) => {
            let at = eye(world, hit);
            super::entity_damage::damage_entity(
                world,
                &EntityHit {
                    target: model,
                    amount,
                    attacker: script_hit.attacker,
                    means: "MOD_MELEE",
                    weapon: script_hit.weapon,
                    point,
                    dir: sub(at, point),
                    bone: None,
                    flags: 0,
                },
            );
        }
    }
    Ok(Value::Object(hit))
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
        "grenade" => Some(
            world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .and_then(|a| a.grenade)
                .filter(|o| live(world, *o))
                .map_or(Value::Undefined, Value::Object),
        ),
        "suppressionmeter" => Some(Value::Float(
            suppression_state(world, id).map_or(0.0, |s| s.2),
        )),
        "suppressionstarttime" => Some(Value::Int(
            suppression_state(world, id).map_or(0, |s| s.0 as i32),
        )),
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

    // `bulletspread( start, end, degrees )`: the end point turned by a random
    // angle up to `degrees` about the line, at the same distance.
    registry.register(Function, "bulletspread", |world, _, args| {
        let (start, end, spread) = (vector(args, 0)?, vector(args, 1)?, float(args, 2)?);
        let line = sub(end, start);
        let dist = length(line);
        if dist <= 0.0 {
            return Ok(Value::Vector(end));
        }
        let dir = normalized(line);
        let (cone, spin) = {
            let mut pool = world.resource_mut::<ActorPool>();
            (pool.random(), pool.random())
        };
        let angles = math_iw4::vect_to_angles(dir);
        let (_, right, up) = math_iw4::angle_vectors(angles);
        let off = (spread * cone).to_radians();
        let theta = spin * std::f32::consts::TAU;
        let turned: [f32; 3] = std::array::from_fn(|i| {
            dir[i] * off.cos() + (right[i] * theta.cos() + up[i] * theta.sin()) * off.sin()
        });
        Ok(Value::Vector(std::array::from_fn(|i| {
            start[i] + turned[i] * dist
        })))
    });
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

    registry.register(Method, "melee", melee);
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
    registry.register(Method, "issuppressed", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let since = suppression_state(world, id).map_or(0, |s| s.0);
        Ok(Value::Int((since > 0).into()))
    });
    registry.register(Method, "issuppressionwaiting", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let now = now_ms(world);
        let waiting = suppression_state(world, id).is_some_and(|(since, last, _, ignore, wait)| {
            !ignore && since > 0 && now - last < wait
        });
        Ok(Value::Int(waiting.into()))
    });
    // Friendly fire never blocks movement here.
    registry.register(Method, "ismovesuppressed", |world, receiver, _| {
        receiver_actor(world, receiver)?;
        Ok(Value::Int(0))
    });
    registry.register(Method, "updateplayersightaccuracy", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        update_player_sight_accuracy(world, id, object);
        Ok(Value::Undefined)
    });
    for name in [
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
    // `dropweapon( weapon, position, chance )`: a pickup item falls from the tag
    // the weapon hangs on.
    registry.register(Method, "dropweapon", |world, receiver, args| {
        let (_, object) = receiver_actor(world, receiver)?;
        let name = string(args, 0)?;
        let position = optional(args, 1, string)?.unwrap_or_else(|| "right".into());
        if name == "none" || name.is_empty() {
            return Ok(Value::Undefined);
        }
        let weapon = crate::script_player::weapon_named(&FrameWorld::from_world(world), &name)?;
        let tag = match position.as_str() {
            "left" => "tag_weapon_left",
            "chest" => "tag_weapon_chest",
            "back" => "tag_stowed_back",
            _ => "tag_weapon_right",
        };
        let at = match super::presence::tag_world(world, object, tag) {
            Some((at, _)) => at,
            None => {
                let o = origin(world, object);
                [o[0], o[1], o[2] + 40.0]
            }
        };
        let yaw = match super::players::entity_field(world, object, "angles") {
            Value::Vector(v) => v[1],
            _ => 0.0,
        };
        let tick = world.resource::<crate::step::StepRequest>().tick;
        let owner = world.resource::<Runtime>().entities[&object].number;
        let number = crate::item::spawn_weapon_item(
            &mut FrameWorld::from_world(world),
            tick,
            weapon,
            at,
            yaw,
            owner,
            true,
        );
        if number == playerstate_iw4::ENTITYNUM_NONE {
            return Ok(Value::Undefined);
        }
        diag::info!(
            Sim,
            "actor: {} dropped {name} as item {number}",
            label(world, object)
        );
        super::natives::player::new_item_entity(world, number, &format!("weapon_{name}"))
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
