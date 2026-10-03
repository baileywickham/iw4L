//! Actor grenades: the toss solve behind `checkgrenadethrow*` (minimum energy,
//! fastest and lob arcs at the engine's 900 u/s cap, swept with traces),
//! `throwgrenade` and `magicgrenade*` as actor-owned projectiles, and the
//! response to a live grenade nearby: return throw, flee or cower.

use super::actor_nav::{path_now, receiver_actor, switch_animscript};
use super::args::{float, optional, string, vector};
use super::natives::engine::{TraceIgnore, collider_entity, entity_id, entity_trace};
use crate::actor::{ActorId, ActorPool, Detour, DetourKind};
use crate::bullet_collision::TraceOutcome;
use crate::frame::FrameWorld;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

const GRAVITY: f32 = 800.0;
/// `810000.0` in the toss solves: the square of the fastest throw.
const MAX_SPEED_SQ: f32 = 810_000.0;
const MASK_GRENADE_TOSS: u32 = 0x0281_0891;
/// A grenade this close (plus its blast radius) is noticed.
const NOTICE_MARGIN: f32 = 64.0;
/// Close enough to pick up and throw back without walking to it.
const RETURN_DIST: f32 = 96.0;
const RETURN_MIN_FUSE_MS: i32 = 1500;

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

fn number(world: &World, object: u64) -> i32 {
    world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .map_or(-1, |e| e.number)
}

fn grenade_weapon(world: &mut World, id: ActorId) -> Option<u32> {
    let name = match world
        .resource::<ActorPool>()
        .actors
        .get(&id)?
        .fields
        .get("grenadeweapon")
    {
        Some(Value::String(name)) => name.to_string(),
        _ => return None,
    };
    if name.is_empty() || name == "none" {
        return None;
    }
    crate::script_player::weapon_named(&FrameWorld::from_world(world), &name)
        .ok()
        .filter(|w| *w != 0)
}

fn grenade_ammo(world: &World, id: ActorId) -> i32 {
    world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map_or(0, |a| a.float_field("grenadeammo") as i32)
}

/// `Actor_Grenade_GetTossFromPosition`: the hand offset turned by the actor's yaw.
fn toss_from(world: &mut World, object: u64, offset: [f32; 3]) -> [f32; 3] {
    let at = origin(world, object);
    let yaw = match super::players::entity_field(world, object, "angles") {
        Value::Vector(v) => v[1],
        _ => 0.0,
    };
    let (forward, right, up) = math_iw4::angle_vectors([0.0, yaw, 0.0]);
    std::array::from_fn(|i| {
        at[i] + forward[i] * offset[0] - right[i] * offset[1] + up[i] * offset[2]
    })
}

/// `Actor_Grenade_CheckMinimumEnergyToss`.
fn min_energy(from: [f32; 3], land: [f32; 3]) -> Option<[f32; 3]> {
    let d = sub(land, from);
    let dist = length(d);
    if dist <= 0.0 || (d[2] + dist) * GRAVITY > MAX_SPEED_SQ {
        return None;
    }
    let t = (dist / GRAVITY * 2.0).sqrt();
    Some([d[0] / t, d[1] / t, GRAVITY * t * 0.5 + d[2] / t])
}

/// `Actor_Grenade_CheckMaximumEnergyToss`: the fastest (flat) or the lob arc at
/// full speed.
fn max_energy(from: [f32; 3], land: [f32; 3], lob: bool) -> Option<[f32; 3]> {
    let d = sub(land, from);
    let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dist_sq < 1.0 {
        return None;
    }
    let b = MAX_SPEED_SQ - GRAVITY * d[2];
    let disc = b * b - GRAVITY * GRAVITY * dist_sq;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let t_sq2 = if lob { b + root } else { b - root };
    let t = (t_sq2 * 2.0).sqrt() / GRAVITY;
    if t <= 0.0 {
        return None;
    }
    Some([d[0] / t, d[1] / t, GRAVITY * t * 0.5 + d[2] / t])
}

fn ballistic(from: [f32; 3], vel: [f32; 3], t: f32) -> [f32; 3] {
    [
        from[0] + vel[0] * t,
        from[1] + vel[1] * t,
        from[2] + vel[2] * t - 0.5 * GRAVITY * t * t,
    ]
}

/// `Actor_Grenade_IsValidTrajectory`: the arc swept in segments up to the apex and
/// down to the target; the last segment may stop on floor close to the target or
/// on an enemy.
fn valid_trajectory(
    world: &mut World,
    object: u64,
    team: &str,
    from: [f32; 3],
    vel: [f32; 3],
    goal: [f32; 3],
) -> bool {
    let disc = vel[2] * vel[2] + 2.0 * GRAVITY * (from[2] - goal[2]);
    if disc <= 0.0 {
        return false;
    }
    let t_land = (vel[2] + disc.sqrt()) / GRAVITY;
    let ignore = TraceIgnore {
        model: world
            .resource::<Runtime>()
            .entities
            .get(&object)
            .and_then(|e| e.presence),
        ..TraceIgnore::default()
    };
    const SEGMENTS: usize = 6;
    let mut at = from;
    for i in 1..=SEGMENTS {
        let to = if i == SEGMENTS {
            goal
        } else {
            ballistic(from, vel, t_land * i as f32 / SEGMENTS as f32)
        };
        match entity_trace(world, at, to, MASK_GRENADE_TOSS, ignore) {
            TraceOutcome::Miss { .. } => {}
            TraceOutcome::Hit {
                end,
                normal,
                collider,
                ..
            } if i == SEGMENTS => {
                let hit = collider_entity(world, collider);
                if let Value::Object(target) = hit {
                    return target_team(world, target)
                        .is_some_and(|t| super::actor_combat::hostile(team, &t));
                }
                let miss = length(sub(end, goal));
                diag::debug!(
                    Sim,
                    "actor: toss lands miss={miss:.0} normal_z={:.2} hit={hit:?}",
                    normal[2]
                );
                return miss < 4.0 || miss < 30.0 && normal[2] > 0.5;
            }
            other => {
                diag::debug!(
                    Sim,
                    "actor: toss blocked segment {i}/{SEGMENTS} at {:?} -> {:?}: {other:?}",
                    at,
                    to
                );
                return false;
            }
        }
        at = to;
    }
    true
}

fn target_team(world: &mut World, object: u64) -> Option<String> {
    if let Some(id) = super::actors::actor_of(world, object) {
        return world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map(|a| a.team.to_string());
    }
    match super::players::entity_field(world, object, "team") {
        Value::String(team) => Some(team.to_string()),
        _ => None,
    }
}

/// `Actor_Grenade_IsSafeTarget`: no sentient of the thrower's side (itself
/// included) inside the blast.
fn safe_target(world: &mut World, id: ActorId, target: [f32; 3], weapon: u32) -> bool {
    let Some(team) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| a.team.clone())
    else {
        return false;
    };
    let radius = FrameWorld::from_world(world)
        .equipment_facts_for(weapon)
        .map_or(256.0, |f| f.explosion_radius.max(0) as f32)
        * 1.1;
    let friends: Vec<u64> = world
        .resource::<ActorPool>()
        .actors
        .values()
        .filter(|a| a.dying.is_none() && !super::actor_combat::hostile(&team, &a.team))
        .map(|a| a.object)
        .collect();
    let players: Vec<u64> = world
        .resource::<Runtime>()
        .players
        .values()
        .map(|s| s.object)
        .collect();
    let friendly_players: Vec<u64> = players
        .into_iter()
        .filter(|p| {
            target_team(world, *p).is_some_and(|t| !super::actor_combat::hostile(&team, &t))
        })
        .collect();
    friends
        .into_iter()
        .chain(friendly_players)
        .all(|o| length(sub(origin(world, o), target)) > radius)
}

/// The toss toward `target`: the first method in order whose arc is clear.
fn check_toss(
    world: &mut World,
    id: ActorId,
    object: u64,
    offset: [f32; 3],
    target: [f32; 3],
    methods: &[String],
) -> Option<([f32; 3], [f32; 3])> {
    let weapon = grenade_weapon(world, id)?;
    if grenade_ammo(world, id) <= 0 || !safe_target(world, id, target, weapon) {
        return None;
    }
    let team = world
        .resource::<ActorPool>()
        .actors
        .get(&id)?
        .team
        .to_string();
    let from = toss_from(world, object, offset);
    for method in methods {
        let vel = match method.as_str() {
            "min energy" => min_energy(from, target),
            "min time" => max_energy(from, target, false),
            "max time" => max_energy(from, target, true),
            "infinite energy" => {
                let d = length(sub(target, from));
                let t = (d * 2.0 / GRAVITY).sqrt();
                (t > 0.0).then(|| {
                    let d = sub(target, from);
                    [d[0] / t, d[1] / t, GRAVITY * t * 0.5 + d[2] / t]
                })
            }
            other => {
                diag::warn!(Sim, "actor: grenade toss method '{other}' unknown");
                None
            }
        };
        if let Some(vel) = vel
            && valid_trajectory(world, object, &team, from, vel, target)
        {
            return Some((from, vel));
        }
    }
    None
}

fn store_toss(world: &mut World, id: ActorId, toss: Option<([f32; 3], [f32; 3])>) -> Value {
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.toss = toss;
    }
    toss.map_or(Value::Undefined, |(_, vel)| Value::Vector(vel))
}

fn methods(args: &[Value], from: usize) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for i in from..args.len() {
        out.push(string(args, i)?);
    }
    if out.is_empty() {
        out.push("min energy".into());
    }
    Ok(out)
}

/// Launches an actor's grenade and gives it a script entity (`grenade` field,
/// `getmissileowner`-style ownership).
fn launch(
    world: &mut World,
    object: u64,
    weapon: u32,
    from: [f32; 3],
    vel: [f32; 3],
    fuse_ms: Option<i32>,
) -> Option<u64> {
    let presence = world
        .resource::<Runtime>()
        .entities
        .get(&object)?
        .presence?;
    let tick = world.resource::<crate::step::StepRequest>().tick;
    let projectile = crate::equipment::spawn_entity_grenade(
        &mut FrameWorld::from_world(world),
        presence,
        weapon,
        tick,
        from,
        vel,
        fuse_ms,
    )?;
    let grenade = super::weapons::adopt(world, &projectile, "grenade").ok()?;
    if let Some(entity) = world.resource_mut::<Runtime>().entities.get_mut(&grenade) {
        entity.missile_owner = Some(object);
    }
    Some(grenade)
}

/// `throwgrenade`: the stored toss, from the hand, with the actor's grenade
/// weapon (or the one it picked up).
fn throw(world: &mut World, id: ActorId, object: u64) -> Option<u64> {
    let (toss, picked) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| (a.toss, a.picked_up))?;
    let weapon = match picked {
        Some((weapon, _)) => weapon,
        None => grenade_weapon(world, id)?,
    };
    let toss = toss.or_else(|| {
        let enemy = world.resource::<ActorPool>().actors.get(&id)?.enemy?;
        let target = origin(world, enemy);
        let from = toss_from(world, object, [0.0, 0.0, 64.0]);
        Some((from, min_energy(from, target)?))
    });
    let (from, vel) = toss.unwrap_or_else(|| {
        let at = toss_from(world, object, [0.0, 0.0, 64.0]);
        let yaw = match super::players::entity_field(world, object, "angles") {
            Value::Vector(v) => v[1].to_radians(),
            _ => 0.0,
        };
        (at, [yaw.cos() * 400.0, yaw.sin() * 400.0, 200.0])
    });
    let fuse = picked.map(|(_, fuse)| fuse);
    let grenade = launch(world, object, weapon, from, vel, fuse)?;
    let thrown = {
        let mut pool = world.resource_mut::<ActorPool>();
        pool.grenades_thrown += 1;
        let thrown = pool.grenades_thrown;
        if let Some(a) = pool.actors.get_mut(&id) {
            a.toss = None;
            if a.picked_up.take().is_none() {
                let ammo = a.float_field("grenadeammo") as i32;
                a.fields
                    .insert("grenadeammo", Value::Int((ammo - 1).max(0)));
            }
        }
        thrown
    };
    let at = origin(world, object);
    let authored = FrameWorld::from_world(world)
        .equipment_facts_for(weapon)
        .map_or(-1, |f| f.fuse_time_ms);
    diag::info!(
        Sim,
        "actor: entity {} threw grenade {thrown} weapon={weapon} fuse={authored} from {:.0} {:.0} {:.0} speed={:.0}{}",
        number(world, object),
        at[0],
        at[1],
        at[2],
        length(vel),
        if fuse.is_some() { " (returned)" } else { "" }
    );
    Some(grenade)
}

/// Grenades in flight or on the ground near live actors: each actor judges a
/// grenade once (`grenadeawareness`), then picks it up and throws it back when it
/// is at its feet with time left, runs out of the blast, or cowers.
pub(crate) fn run(world: &mut World, actors: &[(i32, ActorId, u64)], now: i64) {
    let live: Vec<crate::ProjectileState> = crate::frame::collect_projectiles(world)
        .into_iter()
        .filter(|p| p.live && p.detonate_at_ms.is_some())
        .collect();
    for (_, id, _) in actors {
        let stale = world
            .resource::<ActorPool>()
            .actors
            .get(id)
            .and_then(|a| a.grenade);
        if let Some(object) = stale
            && !world.resource::<Runtime>().live(&object)
            && let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(id)
        {
            a.grenade = None;
        }
    }
    if live.is_empty() {
        return;
    }
    let time = now as i32;
    for projectile in live {
        let Some(facts) = FrameWorld::from_world(world).equipment_facts_for(projectile.weapon)
        else {
            continue;
        };
        let radius = facts.explosion_radius.max(0) as f32;
        if radius <= 0.0 {
            continue;
        }
        let pos = projectile.origin_at(time);
        let object = match world.resource::<Runtime>().missiles.get(&projectile.id) {
            Some(object) => *object,
            None => match super::weapons::adopt(world, &projectile, "grenade") {
                Ok(object) => object,
                Err(_) => continue,
            },
        };
        let fuse_left = projectile.detonate_at_ms.unwrap_or(time) - time;
        for (_, id, actor) in actors {
            let Some((busy, seen, awareness, dying)) =
                world.resource::<ActorPool>().actors.get(id).map(|a| {
                    (
                        a.grenade.is_some(),
                        a.grenades_seen.contains(&object),
                        a.fields
                            .get("grenadeawareness")
                            .map_or(0.9, |_| a.float_field("grenadeawareness")),
                        a.dying.is_some(),
                    )
                })
            else {
                continue;
            };
            if busy || seen || dying {
                continue;
            }
            let at = origin(world, *actor);
            let dist = length(sub(pos, at));
            if dist > radius + NOTICE_MARGIN {
                continue;
            }
            let roll = world.resource_mut::<ActorPool>().random();
            if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(id) {
                a.grenades_seen.push(object);
                if a.grenades_seen.len() > 16 {
                    a.grenades_seen.remove(0);
                }
            }
            if roll >= awareness
                || projectile.owner_entity
                    == world
                        .resource::<Runtime>()
                        .entities
                        .get(actor)
                        .and_then(|e| e.presence)
            {
                continue;
            }
            respond(
                world,
                *id,
                *actor,
                object,
                &projectile,
                pos,
                radius,
                fuse_left,
                now,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn respond(
    world: &mut World,
    id: ActorId,
    object: u64,
    grenade: u64,
    projectile: &crate::ProjectileState,
    pos: [f32; 3],
    radius: f32,
    fuse_left: i32,
    now: i64,
) {
    let at = origin(world, object);
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.grenade = Some(grenade);
        a.detour = None;
    }
    let grounded = projectile.grounded || length(projectile.velocity_at(now as i32)) < 50.0;
    let returns = grounded
        && length(sub(pos, at)) <= RETURN_DIST
        && fuse_left >= RETURN_MIN_FUSE_MS
        && world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .is_some_and(|a| a.enemy.is_some())
        && world.resource_mut::<ActorPool>().random() < 0.5;
    let n = number(world, object);
    if returns {
        diag::info!(
            Sim,
            "actor: entity {n} grenade response: return throw (fuse {fuse_left} ms)"
        );
        super::actor_nav::clear_path(world, id);
        switch_animscript(world, id, object, "grenade_return_throw".into(), now);
        return;
    }
    let away = {
        let d = [at[0] - pos[0], at[1] - pos[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let d = if len > 1.0 {
            [d[0] / len, d[1] / len]
        } else {
            [1.0, 0.0]
        };
        let reach = radius + NOTICE_MARGIN;
        [pos[0] + d[0] * reach, pos[1] + d[1] * reach, at[2]]
    };
    if fuse_left > 1000 && path_now(world, id, object, away) {
        diag::info!(
            Sim,
            "actor: entity {n} grenade response: flee ({:.0} from blast)",
            radius
        );
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            a.detour = Some(Detour {
                pos: away,
                kind: DetourKind::Flee,
            });
        }
        return;
    }
    diag::info!(Sim, "actor: entity {n} grenade response: cower");
    super::actor_nav::clear_path(world, id);
    switch_animscript(world, id, object, "grenade_cower".into(), now);
}

fn log_check(world: &mut World, object: u64, what: &str, target: [f32; 3], found: bool) {
    let at = origin(world, object);
    diag::debug!(
        Sim,
        "actor: entity {} {what} dist={:.0} -> {}",
        number(world, object),
        length(sub(target, at)),
        if found { "toss" } else { "none" }
    );
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::Method;

    // `checkgrenadethrowpos( handOffset, target, withBounce, method... )`.
    registry.register(Method, "checkgrenadethrowpos", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let offset = vector(args, 0)?;
        let target = vector(args, 1)?;
        let methods = methods(args, 3)?;
        let toss = check_toss(world, id, object, offset, target, &methods);
        log_check(
            world,
            object,
            "checkgrenadethrowpos",
            target,
            toss.is_some(),
        );
        Ok(store_toss(world, id, toss))
    });
    // `checkgrenadethrow( handOffset, randomRange, method... )`: at the enemy where
    // it will be in a second, scattered by up to `randomRange`.
    registry.register(Method, "checkgrenadethrow", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let offset = vector(args, 0)?;
        let range = optional(args, 1, float)?.unwrap_or(0.0);
        let methods = methods(args, 2)?;
        let Some(enemy) = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .and_then(|a| a.enemy)
            .filter(|e| world.resource::<Runtime>().live(e))
        else {
            return Ok(Value::Undefined);
        };
        let mut target = origin(world, enemy);
        if let Some(velocity) = world
            .resource::<Runtime>()
            .player_client(enemy)
            .and_then(|c| {
                FrameWorld::from_world(world)
                    .player(crate::ClientId(c))
                    .map(|ps| ps.velocity)
            })
        {
            target = [target[0] + velocity[0], target[1] + velocity[1], target[2]];
        }
        if range > 0.0 {
            let (a, b) = {
                let mut pool = world.resource_mut::<ActorPool>();
                (pool.random(), pool.random())
            };
            target[0] += (a * 2.0 - 1.0) * range;
            target[1] += (b * 2.0 - 1.0) * range;
        }
        let toss = check_toss(world, id, object, offset, target, &methods);
        log_check(world, object, "checkgrenadethrow", target, toss.is_some());
        Ok(store_toss(world, id, toss))
    });
    for name in ["checkgrenadelaunch", "checkgrenadelaunchpos"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    registry.register(Method, "throwgrenade", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        Ok(throw(world, id, object).map_or(Value::Undefined, Value::Object))
    });
    // `pickupgrenade`: the grenade the actor is reacting to leaves the world and
    // its remaining fuse goes with the actor's next `throwgrenade`.
    registry.register(Method, "pickupgrenade", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let Some(grenade) = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .and_then(|a| a.grenade)
        else {
            return Ok(Value::Undefined);
        };
        let Some(number) = world
            .resource::<Runtime>()
            .entities
            .get(&grenade)
            .map(|e| e.number)
        else {
            return Ok(Value::Undefined);
        };
        let now = now_ms(world) as i32;
        let taken = FrameWorld::from_world(world).remove_projectile_by_number(number);
        if let Some(projectile) = taken {
            let fuse = projectile
                .detonate_at_ms
                .map_or(1000, |at| (at - now).max(250));
            {
                let mut runtime = world.resource_mut::<Runtime>();
                if !runtime.pending_deletes.contains(&grenade) {
                    runtime.pending_deletes.push(grenade);
                }
            }
            if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
                a.picked_up = Some((projectile.weapon, fuse));
            }
        }
        Ok(Value::Undefined)
    });
    // `magicgrenademanual( origin, velocity, fuseSeconds )` from an entity.
    registry.register(Method, "magicgrenademanual", |world, receiver, args| {
        let object = entity_id(world, receiver)?;
        let (from, vel) = (vector(args, 0)?, vector(args, 1)?);
        let fuse = optional(args, 2, float)?.map(|s| (s * 1000.0) as i32);
        let weapon = match super::actors::actor_of(world, object) {
            Some(id) => grenade_weapon(world, id),
            None => {
                crate::script_player::weapon_named(&FrameWorld::from_world(world), "fraggrenade")
                    .ok()
            }
        };
        let Some(weapon) = weapon else {
            return Ok(Value::Undefined);
        };
        Ok(launch(world, object, weapon, from, vel, fuse).map_or(Value::Undefined, Value::Object))
    });
    // `magicgrenade( start, end, fuseSeconds )`: a minimum-energy arc between them.
    registry.register(Method, "magicgrenade", |world, receiver, args| {
        let object = entity_id(world, receiver)?;
        let (from, to) = (vector(args, 0)?, vector(args, 1)?);
        let fuse = optional(args, 2, float)?.map(|s| (s * 1000.0) as i32);
        let weapon = match super::actors::actor_of(world, object) {
            Some(id) => grenade_weapon(world, id),
            None => {
                crate::script_player::weapon_named(&FrameWorld::from_world(world), "fraggrenade")
                    .ok()
            }
        };
        let (Some(weapon), Some(vel)) = (weapon, min_energy(from, to)) else {
            return Ok(Value::Undefined);
        };
        Ok(launch(world, object, weapon, from, vel, fuse).map_or(Value::Undefined, Value::Object))
    });
}
