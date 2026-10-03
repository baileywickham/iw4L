//! Sentient and actor perception queries: `cansee` (eye-to-eye trace inside the
//! actor's fov and sight distance), team checks, muzzle and stance queries.

use super::actors::actor_of;
use super::args::{arg, string};
use super::natives::engine::{TraceIgnore, entity_id, entity_trace};
use crate::actor::{ActorId, ActorPool, STANCE_CROUCH, STANCE_PRONE, STANCE_STAND};
use crate::bullet_collision::TraceOutcome;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

/// The mask `sightconetrace` and AI sight share.
const MASK_AI_SIGHT: u32 = 0x0801;
const INDOOR_CEILING: f32 = 512.0;

fn now_ms(world: &World) -> i64 {
    i64::from(world.resource::<crate::step::StepRequest>().tick.0) * i64::from(crate::MATCH_TICK_MS)
}

fn vector_field(world: &mut World, object: u64, name: &str) -> [f32; 3] {
    match super::players::entity_field(world, object, name) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn receiver_actor(world: &World, receiver: &Value) -> Result<(ActorId, u64), String> {
    match receiver {
        Value::Object(object) => actor_of(world, *object)
            .map(|id| (id, *object))
            .ok_or_else(|| "receiver is not an actor".into()),
        _ => Err("receiver is not an actor".into()),
    }
}

/// Where a sentient sees from: a player's view, an actor's `tag_eye` (or its
/// stance height), any other entity's origin.
pub(crate) fn eye(world: &mut World, object: u64) -> [f32; 3] {
    let origin = vector_field(world, object, "origin");
    if let Some(client) = world.resource::<Runtime>().player_client(object) {
        let height = crate::frame::FrameWorld::from_world(world)
            .player(crate::ClientId(client))
            .map_or(60.0, |ps| ps.view_height_current);
        return [origin[0], origin[1], origin[2] + height];
    }
    let Some(id) = actor_of(world, object) else {
        return origin;
    };
    if let Some((at, _)) = super::presence::tag_world(world, object, "tag_eye") {
        return at;
    }
    let pose = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.fields.get("anim_pose").cloned());
    let height = match pose {
        Some(Value::String(pose)) if &*pose == "crouch" => 40.0,
        Some(Value::String(pose)) if &*pose == "prone" => 11.0,
        _ => 64.0,
    };
    [origin[0], origin[1], origin[2] + height]
}

fn ignore(world: &World, actor: u64, target: u64) -> TraceIgnore {
    let runtime = world.resource::<Runtime>();
    let model = |id: u64| runtime.entities.get(&id).and_then(|e| e.presence);
    TraceIgnore {
        client: runtime.player_client(target).map(crate::ClientId),
        other_client: None,
        model: model(actor).or_else(|| model(target)),
    }
}

/// How long a sight result stays in the per-pair cache for `cansee`'s latency.
const SIGHT_CACHE_MS: i64 = 1000;

/// `Actor_CanSeeEntity`: within `maxsightdistsqrd`, inside the fov cone, and an
/// unobstructed line from eye to eye. A result younger than `latency_ms` (and any
/// from this tick) is reused.
pub(crate) fn can_see(
    world: &mut World,
    id: ActorId,
    object: u64,
    target: u64,
    latency_ms: i64,
) -> bool {
    let now = now_ms(world);
    let cached = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.sight.get(&target).copied())
        .filter(|(at, _)| now - *at <= latency_ms.max(0));
    if let Some((_, seen)) = cached {
        return seen;
    }
    let (fov, max_sq) = {
        let pool = world.resource::<ActorPool>();
        let a = &pool.actors[&id];
        (
            a.float_field("fovcosine"),
            a.float_field("maxsightdistsqrd"),
        )
    };
    let from = eye(world, object);
    let to = eye(world, target);
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let yaw = vector_field(world, object, "angles")[1].to_radians();
    let len = len_sq.sqrt().max(1e-3);
    let facing = (yaw.cos() * d[0] + yaw.sin() * d[1]) / len;
    let seen = len_sq <= max_sq
        && (len_sq < 1.0 || facing >= fov)
        && matches!(
            entity_trace(
                world,
                from,
                to,
                MASK_AI_SIGHT,
                ignore(world, object, target)
            ),
            TraceOutcome::Miss { .. }
        );
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.sight.retain(|_, (at, _)| now - *at < SIGHT_CACHE_MS);
        a.sight.insert(target, (now, seen));
    }
    seen
}

fn is_bad_guy(world: &mut World, object: u64) -> bool {
    let team = match actor_of(world, object) {
        Some(id) => world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map(|a| a.team.to_string()),
        None => match super::players::entity_field(world, object, "team") {
            Value::String(team) => Some(team.to_string()),
            _ => None,
        },
    };
    matches!(team.as_deref(), Some("axis" | "team3"))
}

pub(crate) fn muzzle(world: &mut World, object: u64) -> ([f32; 3], [f32; 3]) {
    if let Some((at, axis)) = super::presence::tag_world(world, object, "tag_flash") {
        let f = axis[0];
        let pitch = -f[2].clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = f[1].atan2(f[0]).to_degrees();
        return (at, [pitch, yaw, 0.0]);
    }
    let origin = vector_field(world, object, "origin");
    let angles = vector_field(world, object, "angles");
    let yaw = angles[1].to_radians();
    let at = [
        origin[0] + yaw.cos() * 20.0,
        origin[1] + yaw.sin() * 20.0,
        origin[2] + 48.0,
    ];
    (at, [0.0, angles[1], 0.0])
}

fn stance_bit(stance: &str) -> Result<u8, String> {
    match stance {
        "stand" => Ok(STANCE_STAND),
        "crouch" => Ok(STANCE_CROUCH),
        "prone" => Ok(STANCE_PRONE),
        other => Err(format!("unknown stance '{other}'")),
    }
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};

    registry.register(Method, "cansee", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        let latency = super::args::optional(args, 1, super::args::int)?.unwrap_or(0);
        Ok(Value::Int(
            can_see(world, id, object, target, i64::from(latency)).into(),
        ))
    });
    registry.register(Method, "isbadguy", |world, receiver, _| {
        let object = entity_id(world, receiver)?;
        Ok(Value::Int(is_bad_guy(world, object).into()))
    });
    registry.register(Method, "getmuzzlepos", |world, receiver, _| {
        let (_, object) = receiver_actor(world, receiver)?;
        Ok(Value::Vector(muzzle(world, object).0))
    });
    registry.register(Method, "getmuzzlesideoffsetpos", |world, receiver, _| {
        let (_, object) = receiver_actor(world, receiver)?;
        Ok(Value::Vector(muzzle(world, object).0))
    });
    registry.register(Method, "getmuzzleangle", |world, receiver, _| {
        let (_, object) = receiver_actor(world, receiver)?;
        Ok(Value::Vector(muzzle(world, object).1))
    });
    registry.register(Method, "isstanceallowed", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let bit = stance_bit(&string(args, 0)?.to_ascii_lowercase())?;
        let allowed = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .is_some_and(|a| a.allowed_stances & bit != 0);
        Ok(Value::Int(allowed.into()))
    });
    registry.register(Method, "allowedstances", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let mut stances = 0;
        for i in 0..args.len() {
            stances |= stance_bit(&string(args, i)?.to_ascii_lowercase())?;
        }
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            a.allowed_stances = stances;
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "setdefaultaimlimits", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            for (name, limit) in [
                ("upaimlimit", 45.0),
                ("downaimlimit", -45.0),
                ("rightaimlimit", 45.0),
                ("leftaimlimit", -45.0),
            ] {
                a.fields.insert(name, Value::Float(limit));
            }
        }
        Ok(Value::Undefined)
    });
    // Something solid overhead within reach counts as indoors.
    registry.register(Method, "isindoor", |world, receiver, _| {
        let object = entity_id(world, receiver)?;
        let from = eye(world, object);
        let up = [from[0], from[1], from[2] + INDOOR_CEILING];
        let t = super::presence::settled(world).trace_world(from, up, [0.0; 3], [0.0; 3], 1);
        Ok(Value::Int((t.fraction < 1.0 && t.startsolid == 0).into()))
    });
    // No enemy path knowledge exists yet: undefined is what the engine answers then.
    registry.register(
        Method,
        "getanglestolikelyenemypath",
        |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        },
    );
    for name in ["thermaldrawenable", "thermaldrawdisable"] {
        registry.register(Method, name, |world, receiver, _| {
            receiver_actor(world, receiver)?;
            Ok(Value::Undefined)
        });
    }
    // Weapon clip models are not loaded; "" is the engine's answer for a weapon without one.
    registry.register(Function, "getweaponclipmodel", |_, _, args| {
        string(args, 0)?;
        Ok(Value::string(""))
    });
}
