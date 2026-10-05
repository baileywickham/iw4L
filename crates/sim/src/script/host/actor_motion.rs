//! Actor root motion: the animtree's root delta moves the actor through the
//! clip map with a 15×72 hull (`movement_iw4::step_slide_move`), per anim mode,
//! cover arrival and negotiation (`animscripts/traverse/*`).
//!
//! `normal`/`none` (`AI_ANIM_MOVE_CODE`): the delta's length is the distance
//! walked along the path (`actor_nav::follow_path`). `gravity`/`zonly_physics`:
//! the delta turned by the actor's yaw, ground and gravity from physics.
//! `nogravity`: the full delta against the clip map. `noclip`/`nophysics`: the
//! full delta, no collision. Every anim-driven mode applies the anim's yaw.

use super::actor_nav::{ACTOR_MAXS, ACTOR_MINS, MASK_ACTOR_SOLID};
use super::actors::actor_of;
use super::anim::rotate_yaw;
use crate::actor::{ActorId, ActorPool, Negotiation, Orient, TraverseMode};
use crate::frame::FrameWorld;
use crate::script::runtime::thread_running;
use crate::script::{Arc, Runtime, Value};
use bevy_ecs::prelude::World;
use movement_iw4::{CollisionBackend, GroundTraceInput, Pml, step_slide_move};
use playerstate_iw4::ENTITYNUM_NONE;
use trace_iw4::Trace;

const TICK_SECONDS: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
/// `g_gravity`.
const GRAVITY: f32 = 800.0;
const ENTITYNUM_WORLD: i32 = 0x7FE;
/// Normal z of walkable ground (`MIN_WALK_NORMAL`).
const MIN_WALK_NORMAL: f32 = 0.7;
/// Airborne this long without landing: back to the last spot that stood.
const FALL_RESCUE_MS: i64 = 3000;
/// A traverse animscript that has not finished by then is ended.
const TRAVERSE_TIMEOUT_MS: i64 = 12_000;
/// Further than this from the end node when the traverse ends: placed on it.
const TRAVERSE_END_SLACK: f32 = 48.0;
/// A cover arrival that ends this close to its node finishes on it.
const ARRIVAL_SNAP: f32 = 24.0;
/// Lifts tried when the hull starts in solid.
const UNSTICK_LIFTS: [f32; 4] = [1.0, 4.0, 9.0, 18.0];

/// Anim modes where the code does not walk the path (anim deltas move the actor).
pub(crate) const ANIM_DRIVEN: [&str; 6] = [
    "zonly_physics",
    "gravity",
    "nogravity",
    "nophysics",
    "noclip",
    "angle deltas",
];

/// How a step meets the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Physics {
    /// Step-slide over the ground, gravity in the air; anim z is dropped.
    Ground,
    /// Slide against the clip map, no gravity, anim z kept.
    Fly,
    NoClip,
}

struct ActorClip<'a>(&'a FrameWorld<'a>);

impl CollisionBackend for ActorClip<'_> {
    fn trace(&self, input: GroundTraceInput) -> Trace {
        self.0.trace_clip(
            input.start,
            input.end,
            input.mins,
            input.maxs,
            input.tracemask,
        )
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn flat_len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn entity_number(world: &World, object: u64) -> i32 {
    world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .map_or(-1, |e| e.number)
}

fn origin(world: &mut World, object: u64) -> [f32; 3] {
    match super::players::entity_field(world, object, "origin") {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

pub(crate) fn yaw(world: &mut World, object: u64) -> f32 {
    match super::players::entity_field(world, object, "angles") {
        Value::Vector(v) => v[1],
        _ => 0.0,
    }
}

fn with_actor<T>(
    world: &mut World,
    id: ActorId,
    f: impl FnOnce(&mut crate::actor::Actor) -> T,
) -> Option<T> {
    world.resource_mut::<ActorPool>().actors.get_mut(&id).map(f)
}

fn set_origin(world: &mut World, object: u64, pos: [f32; 3]) {
    world
        .resource_mut::<super::mechanics::Mechanics>()
        .stop(object, "origin");
    world
        .resource_mut::<Runtime>()
        .set_object_field(object, "origin", Value::Vector(pos));
}

/// Moves the actor's hull by `wish` (world units this tick). `None` when the
/// hull is stuck in solid even lifted a step; the caller falls back.
pub(crate) fn physics_move(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
    wish: [f32; 3],
    physics: Physics,
) -> Option<[f32; 3]> {
    if physics == Physics::NoClip {
        return Some(add(at, wish));
    }
    let now = super::players::now_ms(world);
    let fall = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map_or(0.0, |a| a.motion.fall);
    let frame = FrameWorld::from_world(world);
    let clip = ActorClip(&frame);
    let mut start = at;
    let mut ground = frame.trace_clip(
        start,
        [start[0], start[1], start[2] - 0.25],
        ACTOR_MINS,
        ACTOR_MAXS,
        MASK_ACTOR_SOLID,
    );
    if ground.startsolid != 0 {
        let lifted = UNSTICK_LIFTS.iter().find_map(|lift| {
            let up = [at[0], at[1], at[2] + lift];
            let t = frame.trace_clip(
                up,
                [up[0], up[1], up[2] - 0.25],
                ACTOR_MINS,
                ACTOR_MAXS,
                MASK_ACTOR_SOLID,
            );
            (t.startsolid == 0).then_some((up, t))
        });
        let (up, t) = lifted?;
        start = up;
        ground = t;
    }
    let on_ground =
        physics == Physics::Ground && ground.fraction < 1.0 && ground.normal[2] >= MIN_WALK_NORMAL;
    let mut ps = crate::world::blank_player_state();
    ps.origin = start;
    ps.velocity = [
        wish[0] / TICK_SECONDS,
        wish[1] / TICK_SECONDS,
        match physics {
            Physics::Ground if on_ground => 0.0,
            Physics::Ground => fall,
            _ => wish[2] / TICK_SECONDS,
        },
    ];
    ps.ground_entity_num = if on_ground {
        ENTITYNUM_WORLD
    } else {
        ENTITYNUM_NONE
    };
    let mut ground_trace = [0u32; 11];
    if on_ground {
        for (slot, n) in ground_trace[1..4].iter_mut().zip(ground.normal) {
            *slot = n.to_bits();
        }
    }
    let pml = Pml {
        forward: [0.0; 3],
        right: [0.0; 3],
        up: [0.0; 3],
        frametime: TICK_SECONDS,
        msec: crate::MATCH_TICK_MS as i32,
        walking: u32::from(on_ground),
        ground_plane: u32::from(on_ground),
        almost_ground_plane: 0,
        ground_trace,
        previous_origin: start,
        previous_velocity: ps.velocity,
        holdrand: 0,
        jump_animations: [None; 4],
        mantle_movetype: None,
        landing_animation: false,
    };
    let gravity = (physics == Physics::Ground && !on_ground).then_some(GRAVITY);
    if physics == Physics::Fly {
        movement_iw4::slide_move(
            &mut ps,
            &pml,
            &clip,
            ACTOR_MINS,
            ACTOR_MAXS,
            MASK_ACTOR_SOLID,
            None,
        );
    } else {
        step_slide_move(
            &mut ps,
            &pml,
            &clip,
            ACTOR_MINS,
            ACTOR_MAXS,
            MASK_ACTOR_SOLID,
            gravity,
        );
    }
    if physics != Physics::Ground {
        with_actor(world, id, |a| {
            a.motion.fall = 0.0;
            a.motion.air_ms = 0;
        });
        return Some(ps.origin);
    }
    let rescue = with_actor(world, id, |a| {
        if on_ground {
            a.motion.fall = 0.0;
            a.motion.air_ms = 0;
            a.motion.last_ground = Some(start);
            return None;
        }
        a.motion.fall = ps.velocity[2];
        a.motion.air_ms += crate::MATCH_TICK_MS as i64;
        if a.motion.air_ms < FALL_RESCUE_MS {
            return None;
        }
        a.motion.fall = 0.0;
        a.motion.air_ms = 0;
        a.motion.last_ground
    })
    .flatten();
    if let Some(back) = rescue {
        diag::warn!(
            Sim,
            "actor: entity {} fell {:.0} units without landing at {now} ms; back to {:.0} {:.0} {:.0}",
            entity_number(world, object),
            back[2] - ps.origin[2],
            back[0],
            back[1],
            back[2]
        );
        return Some(back);
    }
    Some(ps.origin)
}

/// Kinematic fallback: `to` with its height on the floor below.
pub(crate) fn kinematic(world: &mut World, at: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let frame = FrameWorld::from_world(world);
    let t = frame.trace_world(
        add(to, [0.0, 0.0, 18.0]),
        sub(to, [0.0, 0.0, 36.0]),
        [-4.0, -4.0, 0.0],
        [4.0, 4.0, 4.0],
        MASK_ACTOR_SOLID,
    );
    let mut pos = to;
    if t.fraction < 1.0 && t.startsolid == 0 {
        pos[2] = t.endpos[2];
    } else if (to[2] - at[2]).abs() < 1.0 {
        pos[2] = at[2];
    }
    pos
}

/// Turns the actor by the anim's yaw; a `face angle` target turns with it so
/// the anim's turn is not undone.
fn apply_anim_yaw(world: &mut World, id: ActorId, object: u64, turn: f32) {
    if turn.abs() < 1e-4 {
        return;
    }
    let yaw = yaw(world, object) + turn;
    world.resource_mut::<Runtime>().set_object_field(
        object,
        "angles",
        Value::Vector([0.0, (yaw + 180.0).rem_euclid(360.0) - 180.0, 0.0]),
    );
    with_actor(world, id, |a| {
        if let Orient::Angle(target) = &mut a.orient {
            *target += turn;
        }
    });
}

/// Root motion in an anim-driven mode: the delta turned by the actor's yaw.
/// Returns the new origin.
pub(crate) fn anim_step(world: &mut World, id: ActorId, object: u64, at: [f32; 3]) -> [f32; 3] {
    let Some((mode, delta)) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| (a.anim_mode.clone(), a.anim_delta.clone()))
    else {
        return at;
    };
    let physics = match &*mode {
        "zonly_physics" | "gravity" => Some(Physics::Ground),
        "nogravity" => Some(Physics::Fly),
        "noclip" | "nophysics" => Some(Physics::NoClip),
        _ => None,
    };
    let facing = yaw(world, object);
    apply_anim_yaw(world, id, object, delta.yaw);
    let Some(physics) = physics else {
        return at;
    };
    let mut wish = rotate_yaw(delta.trans, facing);
    if physics == Physics::Ground {
        wish[2] = 0.0;
        let airborne = world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .is_some_and(|a| a.motion.air_ms > 0 || a.motion.fall != 0.0);
        if flat_len(wish) < 0.01 && !airborne {
            return at;
        }
    } else if wish.iter().all(|c| c.abs() < 0.01) {
        return at;
    }
    let moved = physics_move(world, id, object, at, wish, physics)
        .unwrap_or_else(|| kinematic(world, at, add(at, wish)));
    record_sample(world, id, object, moved, at, &delta, &mode, false);
    moved
}

/// Per-actor speed log (`IW4L_ACTOR_MOTION_LOG=1`): once a second while moving.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_sample(
    world: &mut World,
    id: ActorId,
    object: u64,
    moved: [f32; 3],
    at: [f32; 3],
    delta: &crate::actor::AnimDelta,
    mode: &str,
    kinematic: bool,
) {
    static LOG: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("IW4L_ACTOR_MOTION_LOG").is_ok_and(|v| v == "1"));
    if !*LOG {
        return;
    }
    let step = flat_len(sub(moved, at));
    let sample = with_actor(world, id, |a| {
        a.motion.sample_dist += step;
        a.motion.sample_ticks += 1;
        a.motion.kinematic_ticks += u16::from(kinematic);
        if a.motion.sample_ticks < 20 {
            return None;
        }
        let out = (
            a.motion.sample_dist / (f32::from(a.motion.sample_ticks) * TICK_SECONDS),
            a.motion.kinematic_ticks,
            a.move_mode.name(),
            a.animscript.as_ref().map(|(n, _)| n.clone()),
        );
        a.motion.sample_dist = 0.0;
        a.motion.sample_ticks = 0;
        a.motion.kinematic_ticks = 0;
        Some(out)
    })
    .flatten();
    if let Some((speed, kin, movemode, script)) = sample {
        diag::info!(
            Sim,
            "actor: motion entity {} speed={speed:.0} anim={} animmode={mode} movemode={movemode} script={} kinematic_ticks={kin} at {:.0} {:.0} {:.0}",
            entity_number(world, object),
            delta.leaf.as_deref().unwrap_or("-"),
            script.as_deref().unwrap_or("-"),
            moved[0],
            moved[1],
            moved[2]
        );
    }
}

/// `cover_arrival`: the arrival anim's root motion carries the actor into its
/// node. What the anim has left to play is compared with the node each tick
/// and the miss is spread over the time left, so the arrival ends on the node.
pub(crate) fn arrival_step(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
    node: [f32; 3],
) -> Option<[f32; 3]> {
    let delta = world
        .resource::<ActorPool>()
        .actors
        .get(&id)?
        .anim_delta
        .clone();
    let remaining = super::anim::remaining_root(world, object);
    if flat_len(delta.trans) < 0.01 && delta.yaw.abs() < 1e-4 && remaining.is_none() {
        return None;
    }
    let facing = yaw(world, object);
    apply_anim_yaw(world, id, object, delta.yaw);
    let mut wish = rotate_yaw(delta.trans, facing);
    wish[2] = 0.0;
    if let Some((left, seconds)) = remaining {
        let end = add(add(at, wish), rotate_yaw(left, facing + delta.yaw));
        let miss = sub(node, end);
        let share = (TICK_SECONDS / seconds.max(TICK_SECONDS)).min(1.0);
        wish[0] += miss[0] * share;
        wish[1] += miss[1] * share;
    }
    let moved = physics_move(world, id, object, at, wish, Physics::Ground)
        .unwrap_or_else(|| kinematic(world, at, add(at, wish)));
    record_sample(world, id, object, moved, at, &delta, "cover_arrival", false);
    Some(moved)
}

/// The arrival animscript ended: close enough to the node finishes on it.
pub(crate) fn finish_arrival(world: &mut World, id: ActorId, object: u64) {
    let Some(node) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.claimed)
    else {
        return;
    };
    let target = FrameWorld::from_world(world).path_graph().nodes[node as usize].origin;
    let at = origin(world, object);
    let miss = flat_len(sub(target, at));
    let number = entity_number(world, object);
    let snapped = miss > 0.5 && miss <= ARRIVAL_SNAP && (target[2] - at[2]).abs() < 24.0;
    if snapped {
        set_origin(world, object, [target[0], target[1], at[2]]);
    }
    diag::info!(
        Sim,
        "actor: entity {number} cover arrival ended {miss:.1} from node {node}{}",
        if snapped { " (placed on it)" } else { "" }
    );
}

/// The traverse animscript for a negotiation link's begin node, when the zones
/// carry it.
fn traverse_script(world: &mut World, start: u16) -> Result<Arc<str>, String> {
    let name = FrameWorld::from_world(world).path_graph().nodes[start as usize]
        .animscript
        .to_ascii_lowercase();
    if name.is_empty() {
        return Err("begin node has no animscript".into());
    }
    let script = format!("traverse/{name}");
    if !super::actors::has_function(world, &format!("animscripts/{script}::main")) {
        return Err(format!("animscripts/{script} is not loaded"));
    }
    Ok(script.into())
}

/// The actor stands at a negotiation link's begin node: play its traverse
/// animscript (`AIS_NEGOTIATION`). Links without a script are walked.
pub(crate) fn begin_traverse(
    world: &mut World,
    id: ActorId,
    object: u64,
    start: u16,
    end: u16,
    now: i64,
) {
    let number = entity_number(world, object);
    let script = match traverse_script(world, start) {
        Ok(script) => script,
        Err(why) => {
            diag::warn!(
                Sim,
                "actor: entity {number} negotiation {start}->{end}: {why}; walked kinematically"
            );
            with_actor(world, id, |a| a.motion.kinematic_link = Some(end));
            return;
        }
    };
    let node = FrameWorld::from_world(world).path_graph().nodes[start as usize].clone();
    let at = origin(world, object);
    let turn = (node.angle - yaw(world, object) + 540.0).rem_euclid(360.0) - 180.0;
    set_origin(world, object, [node.origin[0], node.origin[1], at[2]]);
    world.resource_mut::<Runtime>().set_object_field(
        object,
        "angles",
        Value::Vector([0.0, node.angle, 0.0]),
    );
    with_actor(world, id, |a| {
        a.traverse = Some(Negotiation {
            start,
            end,
            script: script.clone(),
            since_ms: now,
        });
        a.traverse_mode = TraverseMode::Gravity;
        a.motion.fall = 0.0;
        a.motion.air_ms = 0;
    });
    diag::info!(
        Sim,
        "actor: entity {number} traverse {script} node {start}->{end} at {:.0} {:.0} {:.0} turn={turn:.0}",
        at[0],
        at[1],
        at[2]
    );
    super::actor_nav::switch_animscript(world, id, object, script, now);
}

/// One tick of a negotiation: the traverse anim's full delta under the
/// `traversemode`; when the script ends the path resumes past the end node.
/// `false` once the negotiation is over.
pub(crate) fn traverse_step(world: &mut World, id: ActorId, object: u64, now: i64) -> bool {
    let Some((negotiation, script, mode, delta)) =
        world.resource::<ActorPool>().actors.get(&id).and_then(|a| {
            Some((
                a.traverse.clone()?,
                a.animscript.clone(),
                a.traverse_mode,
                a.anim_delta.clone(),
            ))
        })
    else {
        return false;
    };
    let running = script
        .is_some_and(|(name, serial)| name == negotiation.script && thread_running(world, serial));
    let at = origin(world, object);
    if running && now - negotiation.since_ms < TRAVERSE_TIMEOUT_MS {
        let facing = yaw(world, object);
        apply_anim_yaw(world, id, object, delta.yaw);
        let wish = rotate_yaw(delta.trans, facing);
        let physics = match mode {
            TraverseMode::Gravity => Physics::Ground,
            TraverseMode::NoGravity => Physics::Fly,
            TraverseMode::NoClip => Physics::NoClip,
        };
        let moved =
            physics_move(world, id, object, at, wish, physics).unwrap_or_else(|| add(at, wish));
        if moved != at {
            set_origin(world, object, moved);
        }
        let label = format!("traverse:{mode:?}");
        record_sample(world, id, object, moved, at, &delta, &label, false);
        return true;
    }
    let end = FrameWorld::from_world(world).path_graph().nodes[negotiation.end as usize].origin;
    let miss = sub(end, at);
    let far = flat_len(miss) > TRAVERSE_END_SLACK || miss[2].abs() > TRAVERSE_END_SLACK;
    if far {
        set_origin(world, object, end);
    }
    diag::info!(
        Sim,
        "actor: entity {} traverse {} done in {} ms at {:.0} {:.0} {:.0} end node {} miss {:.0}/{:.0}{}{}",
        entity_number(world, object),
        negotiation.script,
        now - negotiation.since_ms,
        at[0],
        at[1],
        at[2],
        negotiation.end,
        flat_len(miss),
        miss[2],
        if running { " (timed out)" } else { "" },
        if far { " (placed on end node)" } else { "" }
    );
    with_actor(world, id, |a| {
        a.traverse = None;
        a.traverse_mode = TraverseMode::Gravity;
        a.motion.fall = 0.0;
        a.motion.air_ms = 0;
        if let Some(path) = a.path.as_mut()
            && path
                .current()
                .is_some_and(|p| p.traverse && p.node == Some(negotiation.end))
        {
            path.next += 1;
            if path.current().is_none() {
                a.path = None;
            }
        }
    });
    false
}

/// `traversemode( mode )`.
pub(crate) fn set_traverse_mode(
    world: &mut World,
    receiver: &Value,
    mode: &str,
) -> Result<(), String> {
    let object = super::natives::engine::entity_id(world, receiver)?;
    let id = actor_of(world, object).ok_or("receiver is not an actor")?;
    let mode = match mode {
        "gravity" => TraverseMode::Gravity,
        "nogravity" => TraverseMode::NoGravity,
        "noclip" => TraverseMode::NoClip,
        other => return Err(format!("unknown traverse mode '{other}'")),
    };
    with_actor(world, id, |a| a.traverse_mode = mode);
    Ok(())
}
