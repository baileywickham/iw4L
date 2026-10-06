//! Actor goals, paths and movement: the script goal (`setgoal*`), A* over the
//! map's path nodes, kinematic path following, facing, node claims, `"goal"`,
//! and the move/stop animscript switch.

use super::actor_motion::Physics;
use super::actors::{actor_of, run_script};
use super::args::{arg, float, string, vector};
use super::natives::engine::{entity_id, path_node_object};
use crate::actor::path::{self, ActorPath, NEAREST_NODE_DIST, PathSearch, SearchStep};
use crate::actor::{Actor, ActorId, ActorPool, DetourKind, MoveMode, Orient};
use crate::frame::FrameWorld;
use crate::script::runtime::{raise, run_now_thread, thread_running};
use crate::script::{Arc, Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

/// A* expansions all actors share per tick; a search left over resumes next tick.
pub(crate) const EXPANSION_BUDGET: u32 = 4096;
/// A* expansions one script path request may spend.
const SCRIPT_PATH_BUDGET: u32 = 4096;
/// `pathWaitTime` after a failed search (`Actor_HandleInvalidPath`).
const BAD_PATH_WAIT_MS: i64 = 500;
const RUN_SPEED: f32 = 180.0;
const WALK_SPEED: f32 = 70.0;
/// `iPathEndTime - level.time < 200`: the engine names the last 200 ms `stop_soon`.
const STOP_SOON_SECONDS: f32 = 0.2;
const TICK_SECONDS: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
/// Degrees an actor turns per tick (`anglelerprate` 540/s).
const TURN_PER_TICK: f32 = 27.0;
/// `Actor_PointNearNode`.
const NODE_ARRIVE_DIST: f32 = 15.0;
const DIRECT_PATH_DIST: f32 = 256.0;
const SKIP_CHECK_DIST: f32 = 1024.0;
const DOOR_LOOKAHEAD: f32 = 256.0;
const STEP_HEIGHT: f32 = 18.0;
/// `MASK_ACTOR_SOLID` and the sight mask `Path_NearestNode` uses.
pub(crate) const MASK_ACTOR_SOLID: u32 = 0x0282_0011;
const MASK_NODE_SIGHT: u32 = 0x0082_0011;
pub(crate) const ACTOR_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
pub(crate) const ACTOR_MAXS: [f32; 3] = [15.0, 15.0, 72.0];
/// Root motion past this per tick is a time jump, not a step.
const MAX_ROOT_STEP: f32 = 40.0;
/// Ticks `move` may run with a path and no root delta before the kinematic
/// speed takes over.
const NO_DELTA_TICKS: u16 = 4;
/// Ticks the hull may stay blocked short of its step before it slides through
/// (each one plays the run anim in place; 10 read as half a second of treadmill).
const BLOCKED_TICKS: u16 = 4;
/// A traverse starts this close to the link's begin node.
const TRAVERSE_BEGIN_DIST: f32 = 16.0;

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn length2(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

fn yaw_of(v: [f32; 3]) -> f32 {
    v[1].atan2(v[0]).to_degrees()
}

fn angle_delta(to: f32, from: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

fn origin(world: &mut World, object: u64) -> [f32; 3] {
    match super::players::entity_field(world, object, "origin") {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn actor(world: &World, id: ActorId) -> Option<&Actor> {
    world.resource::<ActorPool>().actors.get(&id)
}

fn with_actor<T>(world: &mut World, id: ActorId, f: impl FnOnce(&mut Actor) -> T) -> Option<T> {
    world.resource_mut::<ActorPool>().actors.get_mut(&id).map(f)
}

pub(crate) fn receiver_actor(world: &World, receiver: &Value) -> Result<(ActorId, u64), String> {
    match receiver {
        Value::Object(object) => actor_of(world, *object)
            .map(|id| (id, *object))
            .ok_or_else(|| "receiver is not an actor".into()),
        _ => Err("receiver is not an actor".into()),
    }
}

pub(crate) fn node_index(world: &World, value: &Value) -> Result<u16, String> {
    let Value::Object(object) = value else {
        return Err(format!("{} is not a path node", super::args::kind(value)));
    };
    world
        .resource::<Runtime>()
        .path_nodes
        .iter()
        .position(|slot| *slot == Some(*object))
        .map(|i| i as u16)
        .ok_or_else(|| "not a path node".into())
}

pub(crate) fn node_value(world: &mut World, node: Option<u16>) -> Value {
    node.and_then(|n| path_node_object(world, n as usize).ok())
        .unwrap_or(Value::Undefined)
}

pub(crate) fn hull_clear(frame: &FrameWorld, from: [f32; 3], to: [f32; 3]) -> bool {
    let mins = [ACTOR_MINS[0], ACTOR_MINS[1], STEP_HEIGHT];
    let maxs = [ACTOR_MAXS[0], ACTOR_MAXS[1], 48.0];
    let t = frame.trace_world(from, to, mins, maxs, MASK_ACTOR_SOLID);
    t.fraction >= 1.0 && t.startsolid == 0
}

/// `Path_NearestNode`: the closest linked node within reach that the actor hull can see.
pub(crate) fn nearest_node(frame: &FrameWorld, at: [f32; 3]) -> Option<u16> {
    let graph = frame.path_graph();
    let mins = [ACTOR_MINS[0], ACTOR_MINS[1], ACTOR_MINS[2] + 17.0];
    for radius in [NEAREST_NODE_DIST, NEAREST_NODE_DIST * 3.0] {
        let lowered = [at[0], at[1], at[2] - 120.0];
        let candidates = path::nodes_in_cylinder(graph, lowered, radius, 184.0);
        let mut unlinked = Vec::new();
        for &n in candidates.iter().take(64) {
            let node = &graph.nodes[n as usize];
            if node.links.is_empty() {
                unlinked.push(n);
                continue;
            }
            let inside = (0..2).all(|i| (at[i] - node.origin[i]).abs() < 16.0)
                && at[2] > node.origin[2] - 1.0
                && at[2] < node.origin[2] + 73.0;
            if inside {
                return Some(n);
            }
            let t = frame.trace_world(at, node.origin, mins, ACTOR_MAXS, MASK_NODE_SIGHT);
            if t.fraction >= 1.0 && t.startsolid == 0 {
                return Some(n);
            }
        }
        for n in unlinked {
            let t = frame.trace_world(
                at,
                graph.nodes[n as usize].origin,
                mins,
                ACTOR_MAXS,
                MASK_NODE_SIGHT,
            );
            if t.fraction >= 1.0 && t.startsolid == 0 {
                return Some(n);
            }
        }
        if let Some(&n) = candidates.first().filter(|_| radius > NEAREST_NODE_DIST) {
            return Some(n);
        }
    }
    None
}

fn in_volume(world: &mut World, volume: Option<u64>, point: [f32; 3]) -> bool {
    volume.is_none_or(|v| {
        !world.resource::<Runtime>().live(&v) || super::triggers::contains_point(world, v, point)
    })
}

pub(crate) fn point_at_goal(world: &mut World, id: ActorId, point: [f32; 3]) -> bool {
    let Some((cylinder, volume)) =
        actor(world, id).map(|a| (a.point_in_goal_cylinder(point), a.goal.volume))
    else {
        return false;
    };
    cylinder && in_volume(world, volume, point)
}

fn release_claim(world: &mut World, id: ActorId) {
    let mut pool = world.resource_mut::<ActorPool>();
    let Some(node) = pool.actors.get_mut(&id).and_then(|a| a.claimed.take()) else {
        return;
    };
    if pool.claims.get(&node) == Some(&id) {
        pool.claims.remove(&node);
    }
    if let Some(a) = pool.actors.get_mut(&id) {
        a.prev_claimed = Some(node);
    }
}

pub(crate) fn claim(world: &mut World, id: ActorId, node: u16) {
    if actor(world, id).and_then(|a| a.claimed) == Some(node) {
        return;
    }
    if world
        .resource::<ActorPool>()
        .claims
        .get(&node)
        .is_some_and(|owner| *owner != id)
    {
        return;
    }
    release_claim(world, id);
    let mut pool = world.resource_mut::<ActorPool>();
    pool.claims.insert(node, id);
    if let Some(a) = pool.actors.get_mut(&id) {
        a.claimed = Some(node);
    }
}

pub(crate) fn release_all(world: &mut World, id: ActorId) {
    world
        .resource_mut::<ActorPool>()
        .claims
        .retain(|_, owner| *owner != id);
}

pub(crate) fn clear_path(world: &mut World, id: ActorId) {
    with_actor(world, id, |a| {
        a.path = None;
        a.search = None;
        a.velocity = [0.0; 3];
        a.move_mode = MoveMode::Stop;
    });
}

fn set_goal_pos(world: &mut World, id: ActorId, pos: [f32; 3], node: Option<u16>) {
    let volume = actor(world, id).and_then(|a| a.goal.volume);
    let keep_volume = volume.filter(|v| {
        world.resource::<Runtime>().live(v) && super::triggers::contains_point(world, *v, pos)
    });
    with_actor(world, id, |a| {
        a.goal.pos = pos;
        a.goal.node = node;
        a.goal.entity = None;
        a.goal.volume = keep_volume;
        a.path_wait_ms = 0;
        a.goal_reached = false;
    });
}

/// What the actor paths to this tick: a node to stand on, or the goal position
/// when it is outside the goal.
fn wanted_target(world: &mut World, id: ActorId, at: [f32; 3]) -> Option<([f32; 3], Option<u16>)> {
    let (goal, has_path, final_goal) = {
        let a = actor(world, id)?;
        (
            a.goal.clone(),
            a.path.is_some(),
            a.path.as_ref().map(|p| p.final_goal),
        )
    };
    if let Some(node) = goal.node {
        let near = (at[2] - goal.pos[2]).powi(2) <= 80.0 * 80.0
            && length2(sub(at, goal.pos)) <= NODE_ARRIVE_DIST;
        return (has_path || !near).then_some((goal.pos, Some(node)));
    }
    if let Some(end) = final_goal
        && point_at_goal(world, id, end)
    {
        return Some((end, None));
    }
    (!point_at_goal(world, id, at)).then_some((goal.pos, None))
}

fn begin_path(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
    target: [f32; 3],
    now: i64,
) {
    let frame = FrameWorld::from_world(world);
    let lifted = |p: [f32; 3]| add(p, [0.0, 0.0, 1.0]);
    if length(sub(target, at)) < DIRECT_PATH_DIST
        && (target[2] - at[2]).abs() < 48.0
        && hull_clear(&frame, lifted(at), lifted(target))
    {
        with_actor(world, id, |a| {
            a.path = Some(ActorPath::direct(target));
            a.search = None;
        });
        return;
    }
    let graph = frame.path_graph();
    if graph.is_empty() {
        bad_path(world, id, object, target, now, "no path nodes");
        return;
    }
    let cached = actor(world, id)
        .and_then(|a| a.nearest)
        .filter(|(_, from)| length(sub(*from, at)) < 32.0)
        .map(|(n, _)| n);
    let frame = FrameWorld::from_world(world);
    let from = cached.or_else(|| nearest_node(&frame, at));
    let to = nearest_node(&frame, target);
    let (Some(from), Some(to)) = (from, to) else {
        bad_path(world, id, object, target, now, "no nearest node");
        return;
    };
    let search = PathSearch::new(frame.path_graph(), from, to);
    with_actor(world, id, |a| {
        a.nearest = Some((from, at));
        a.search = Some((search, target));
    });
}

fn bad_path(world: &mut World, id: ActorId, object: u64, target: [f32; 3], now: i64, why: &str) {
    let first = with_actor(world, id, |a| {
        a.search = None;
        a.path = None;
        std::mem::replace(&mut a.path_wait_ms, now + BAD_PATH_WAIT_MS) == 0
    });
    if first == Some(true) {
        diag::info!(
            Sim,
            "actor: entity {object} bad_path to {:.0} {:.0} {:.0}: {why}",
            target[0],
            target[1],
            target[2]
        );
    }
    raise(
        world,
        Value::Object(object),
        "bad_path",
        vec![Value::Vector(target)],
    );
}

fn continue_search(world: &mut World, id: ActorId, object: u64, now: i64, budget: &mut u32) {
    let Some((mut search, target)) = with_actor(world, id, |a| a.search.take()).flatten() else {
        return;
    };
    let step = {
        let frame = FrameWorld::from_world(world);
        search.run(frame.path_graph(), budget)
    };
    match step {
        SearchStep::Pending => {
            with_actor(world, id, |a| a.search = Some((search, target)));
        }
        SearchStep::Failed => bad_path(world, id, object, target, now, "no route"),
        SearchStep::Found(nodes) => {
            let path =
                ActorPath::through(FrameWorld::from_world(world).path_graph(), &nodes, target);
            diag::info!(
                Sim,
                "actor: entity {object} path nodes={} expanded={} length={:.0} to {:.0} {:.0} {:.0}",
                nodes.len(),
                search.expanded,
                path.remaining(origin(world, object)),
                target[0],
                target[1],
                target[2]
            );
            with_actor(world, id, |a| a.path = Some(path));
        }
    }
}

/// A path found now, for script moves (reacquire, `usecovernode`): a direct line
/// when it is short and clear, else A* with its own expansion budget.
pub(crate) fn path_now(world: &mut World, id: ActorId, object: u64, target: [f32; 3]) -> bool {
    let at = origin(world, object);
    let content = FrameWorld::from_world(world).content();
    let graph = content.path_graph();
    let path = {
        let frame = FrameWorld::from_world(world);
        let lifted = |p: [f32; 3]| add(p, [0.0, 0.0, 1.0]);
        if length(sub(target, at)) < DIRECT_PATH_DIST
            && (target[2] - at[2]).abs() < 48.0
            && hull_clear(&frame, lifted(at), lifted(target))
        {
            Some(ActorPath::direct(target))
        } else {
            match (nearest_node(&frame, at), nearest_node(&frame, target)) {
                (Some(from), Some(to)) => {
                    let mut budget = SCRIPT_PATH_BUDGET;
                    match PathSearch::new(graph, from, to).run(graph, &mut budget) {
                        SearchStep::Found(nodes) => Some(ActorPath::through(graph, &nodes, target)),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
    };
    let found = path.is_some();
    if let Some(path) = path {
        with_actor(world, id, |a| {
            a.path = Some(path);
            a.search = None;
        });
    }
    found
}

/// A straight path to a point the caller already checked is clear.
pub(crate) fn set_direct_path(world: &mut World, id: ActorId, to: [f32; 3]) {
    with_actor(world, id, |a| {
        a.path = Some(ActorPath::direct(to));
        a.search = None;
    });
}

/// Path following (`AI_ANIM_MOVE_CODE`): the length of the anim's root delta
/// is the distance walked along the path this tick, moved through the hull
/// physics. `move` with a path and no root delta for `NO_DELTA_TICKS` walks at
/// the kinematic run/walk speed instead (logged once per anim). Also returns
/// the negotiation link to start when the actor stands at its begin node.
fn follow_path(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
) -> ([f32; 3], Option<(u16, u16)>) {
    let Some(mut path) = with_actor(world, id, |a| a.path.take()).flatten() else {
        if let Some((delta, mode)) =
            actor(world, id).map(|a| (a.anim_delta.clone(), a.anim_mode.clone()))
        {
            super::actor_motion::record_sample(world, id, object, at, at, &delta, &mode, false);
        }
        return (at, None);
    };
    let (walk_dist, rate, delta, in_move, kinematic_link, anim_mode) = {
        let a = actor(world, id).unwrap();
        (
            a.float_field("walkdist"),
            a.float_field("moveplaybackrate"),
            a.anim_delta.clone(),
            a.animscript
                .as_ref()
                .is_some_and(|(name, _)| &**name == "move"),
            a.motion.kinematic_link,
            a.anim_mode.clone(),
        )
    };
    let rate = match world
        .resource_mut::<Runtime>()
        .object_field(object, "moveplaybackrate")
    {
        Value::Float(r) if r > 0.0 => r,
        Value::Int(r) if r > 0 => r as f32,
        _ if rate > 0.0 => rate,
        _ => 1.0,
    };
    let mode = if length(sub(path.final_goal, at)) >= walk_dist {
        MoveMode::Run
    } else {
        MoveMode::Walk
    };
    let speed = match mode {
        MoveMode::Run => RUN_SPEED,
        _ => WALK_SPEED,
    } * rate;
    let mode = if path.remaining(at) < speed * STOP_SOON_SECONDS {
        MoveMode::StopSoon
    } else {
        mode
    };
    let anim_dist = length2(delta.trans);
    let (step_dist, kinematic) = if anim_dist > 0.01 {
        with_actor(world, id, |a| a.motion.no_delta_ticks = 0);
        (anim_dist.min(MAX_ROOT_STEP), false)
    } else {
        let (ticks, first) = with_actor(world, id, |a| {
            a.motion.no_delta_ticks = a.motion.no_delta_ticks.saturating_add(1);
            let first =
                a.motion.no_delta_ticks >= NO_DELTA_TICKS && a.motion.logged_leaf != delta.leaf;
            if first {
                a.motion.logged_leaf = delta.leaf.clone();
            }
            (a.motion.no_delta_ticks, first)
        })
        .unwrap_or((0, false));
        if in_move && ticks >= NO_DELTA_TICKS {
            if first {
                diag::warn!(
                    Sim,
                    "actor: entity {} moves without root motion: anim {} has no delta; kinematic {:.0} u/s",
                    world
                        .resource::<Runtime>()
                        .entities
                        .get(&object)
                        .map_or(-1, |e| e.number),
                    delta.leaf.as_deref().unwrap_or("(none)"),
                    speed
                );
            }
            (speed * TICK_SECONDS, true)
        } else {
            (0.0, false)
        }
    };
    {
        let frame = FrameWorld::from_world(world);
        if path.may_skip() {
            let after = path.points[path.next + 1].pos;
            if length(sub(after, at)) < SKIP_CHECK_DIST
                && (after[2] - at[2]).abs() < 64.0
                && hull_clear(
                    &frame,
                    add(at, [0.0, 0.0, 1.0]),
                    add(after, [0.0, 0.0, 1.0]),
                )
            {
                path.next += 1;
            }
        }
    }
    let start_next = path.next;
    let mut target = at;
    let mut step = step_dist;
    let mut traversing = false;
    while step > 0.0
        && let Some(point) = path.current().copied()
    {
        if point.traverse && kinematic_link != point.node {
            break;
        }
        traversing = point.traverse;
        let to = sub(point.pos, target);
        let d = length(to);
        if d <= step {
            target = point.pos;
            step -= d;
            path.next += 1;
        } else {
            target = add(target, to.map(|c| c * step / d));
            step = 0.0;
        }
    }
    let pos = if traversing {
        target
    } else if kinematic {
        super::actor_motion::kinematic(world, at, target)
    } else if step_dist > 0.0 {
        let wish = [target[0] - at[0], target[1] - at[1], 0.0];
        match super::actor_motion::physics_move(world, id, object, at, wish, Physics::Ground) {
            Some(pos) if length2(sub(target, pos)) <= 1.0 + 0.1 * step_dist => {
                with_actor(world, id, |a| a.motion.blocked_ticks = 0);
                pos
            }
            Some(pos) => {
                let blocked = with_actor(world, id, |a| {
                    a.motion.blocked_ticks = a.motion.blocked_ticks.saturating_add(1);
                    a.motion.blocked_ticks
                })
                .unwrap_or(0);
                if blocked == BLOCKED_TICKS {
                    diag::warn!(
                        Sim,
                        "actor: entity {} blocked at {:.0} {:.0} {:.0} short of its path; slides through",
                        world
                            .resource::<Runtime>()
                            .entities
                            .get(&object)
                            .map_or(-1, |e| e.number),
                        pos[0],
                        pos[1],
                        pos[2]
                    );
                }
                if blocked >= BLOCKED_TICKS {
                    super::actor_motion::kinematic(world, at, target)
                } else {
                    path.next = start_next;
                    pos
                }
            }
            None => super::actor_motion::kinematic(world, at, target),
        }
    } else {
        at
    };
    let begin = path
        .current()
        .filter(|p| p.traverse && kinematic_link != p.node)
        .and_then(|p| {
            let before = path.points.get(path.next.checked_sub(1)?)?;
            (length2(sub(before.pos, pos)) <= TRAVERSE_BEGIN_DIST)
                .then_some((before.node?, p.node?))
        });
    let done = path.current().is_none();
    let look = path
        .current()
        .map(|p| sub(p.pos, pos))
        .filter(|v| length2(*v) > 0.1)
        .unwrap_or_else(|| sub(pos, at));
    let look_len = length2(look);
    let moved = sub(pos, at);
    let link_walked = kinematic_link.is_some()
        && !path
            .current()
            .is_some_and(|p| p.traverse && p.node == kinematic_link);
    with_actor(world, id, |a| {
        a.velocity = moved.map(|c| c / TICK_SECONDS);
        a.distance_moved += length2(moved);
        if link_walked {
            a.motion.kinematic_link = None;
        }
        if look_len > 0.1 {
            a.lookahead_dir = [look[0] / look_len, look[1] / look_len, 0.0];
            a.lookahead_dist = look_len.min(path.remaining(pos));
        }
        if done {
            a.move_mode = MoveMode::Stop;
            a.velocity = [0.0; 3];
        } else {
            a.move_mode = mode;
            a.path = Some(path);
        }
    });
    super::actor_motion::record_sample(world, id, object, pos, at, &delta, &anim_mode, kinematic);
    (pos, begin)
}

/// In an anim-driven mode the path is not walked; points the anim carried the
/// actor to are passed.
fn pass_reached(world: &mut World, id: ActorId, at: [f32; 3]) {
    with_actor(world, id, |a| {
        let Some(path) = a.path.as_mut() else {
            return;
        };
        while let Some(point) = path.current()
            && !point.traverse
            && path.next + 1 < path.points.len()
            && length2(sub(point.pos, at)) <= NODE_ARRIVE_DIST
            && (point.pos[2] - at[2]).abs() < 64.0
        {
            path.next += 1;
        }
    });
}

fn face(world: &mut World, id: ActorId, object: u64, at: [f32; 3], anim_driven: bool) {
    let Some((orient, moving, look, claimed, enemy)) = actor(world, id).map(|a| {
        (
            a.orient,
            a.path.is_some(),
            a.lookahead_dir,
            a.claimed,
            a.enemy.and_then(|e| a.known.get(&e)).map(|k| k.pos),
        )
    }) else {
        return;
    };
    let wanted = match orient {
        Orient::Angle(yaw) => Some(yaw),
        Orient::Point(p) => (length2(sub(p, at)) > 1.0).then(|| yaw_of(sub(p, at))),
        Orient::Current => None,
        Orient::Motion | Orient::Enemy | Orient::Default if moving => {
            (length2(look) > 0.1).then(|| yaw_of(look))
        }
        Orient::Default => claimed.and_then(|n| {
            let node = FrameWorld::from_world(world)
                .path_graph()
                .nodes
                .get(n as usize)?
                .clone();
            (length2(sub(node.origin, at)) <= NODE_ARRIVE_DIST && node.node_type != 1)
                .then_some(node.angle)
        }),
        Orient::Enemy => enemy
            .filter(|p| length2(sub(*p, at)) > 1.0)
            .map(|p| yaw_of(sub(p, at))),
        Orient::Motion => None,
    };
    let Some(wanted) = wanted.filter(|_| !(anim_driven && orient == Orient::Default)) else {
        return;
    };
    let mut runtime = world.resource_mut::<Runtime>();
    let angles = match runtime.object_field(object, "angles") {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    };
    let delta = angle_delta(wanted, angles[1]);
    if delta.abs() < 0.01 {
        return;
    }
    let yaw = angles[1] + delta.clamp(-TURN_PER_TICK, TURN_PER_TICK);
    runtime.set_object_field(object, "angles", Value::Vector([0.0, yaw, 0.0]));
}

/// Animscripts that run to their end before the state picks again.
const STICKY_ANIMSCRIPTS: [&str; 6] = [
    "custom",
    "pain",
    "death",
    "cover_arrival",
    "grenade_cower",
    "grenade_return_throw",
];

/// The animscript the actor's state wants: `move` while it has a path, with an
/// enemy its claimed cover node's script or `combat`, else `stop`.
pub(crate) fn select_animscript(world: &mut World, id: ActorId, object: u64, now: i64) {
    let Some((current, started, moving, enemy)) = actor(world, id).map(|a| {
        (
            a.animscript.clone(),
            a.animscript_started_ms,
            a.path.is_some(),
            a.enemy.is_some(),
        )
    }) else {
        return;
    };
    if actor(world, id).is_some_and(|a| a.traverse.is_some()) {
        return;
    }
    let threatened = actor(world, id)
        .and_then(|a| a.grenade)
        .is_some_and(|g| world.resource::<Runtime>().live(&g));
    if let Some((name, serial)) = &current
        && &**name == "scripted"
        && thread_running(world, *serial)
        && actor(world, id).is_some_and(|a| now < a.scripted_until_ms)
    {
        return;
    }
    if let Some((name, serial)) = &current
        && STICKY_ANIMSCRIPTS.contains(&&**name)
        && thread_running(world, *serial)
        && (&**name != "grenade_cower" || threatened)
    {
        return;
    }
    if let Some((name, serial)) = &current
        && &**name == "combat"
        && thread_running(world, *serial)
        && dog_attacking(world, id)
    {
        return;
    }
    let bite = dog_enemy(world, id)
        .is_some_and(|(pos, reach)| in_reach(origin(world, object), pos, reach));
    let dog = actor(world, id).is_some_and(|a| &*a.species == "dog");
    let wanted: Arc<str> = if bite {
        "combat".into()
    } else if moving {
        "move".into()
    } else if dog {
        "stop".into()
    } else if enemy {
        super::actor_cover::combat_script(world, id, object).into()
    } else {
        "stop".into()
    };
    if let Some((name, serial)) = &current
        && *name == wanted
        && (thread_running(world, *serial) || now - started < super::actors::ANIMSCRIPT_RETRY_MS)
    {
        return;
    }
    if bite && current.as_ref().is_none_or(|(name, _)| &**name != "combat") {
        let number = world
            .resource::<Runtime>()
            .entities
            .get(&object)
            .map_or(0, |e| e.number);
        diag::info!(
            Sim,
            "actor: dog entity {number} in reach of its enemy, attacks"
        );
    }
    if wanted.starts_with("cover_") && current.as_ref().is_none_or(|(name, _)| *name != wanted) {
        let node = actor(world, id).and_then(|a| a.claimed);
        let number = world
            .resource::<Runtime>()
            .entities
            .get(&object)
            .map_or(0, |e| e.number);
        diag::info!(
            Sim,
            "actor: entity {number} at cover node {node:?} runs {wanted}"
        );
    }
    switch_animscript(world, id, object, wanted, now);
}

/// Ends the running animscript (`killanimscript`, `end_script`) and starts `wanted`.
pub(crate) fn switch_animscript(
    world: &mut World,
    id: ActorId,
    object: u64,
    wanted: Arc<str>,
    now: i64,
) {
    let Some(current) = actor(world, id).map(|a| a.animscript.clone()) else {
        return;
    };
    if let Some((name, serial)) = &current {
        let receiver = Value::Object(object);
        if thread_running(world, *serial) {
            crate::script::runtime::notify_now(world, receiver.clone(), "killanimscript", now);
        }
        if *name != wanted {
            let module = super::actors::animscript_module(world, id, name);
            run_script(
                world,
                &format!("animscripts/{module}::end_script"),
                receiver,
            );
        }
    }
    if !world.resource::<Runtime>().live(&object) {
        return;
    }
    with_actor(world, id, |a| {
        if let Some((name, _)) = &current
            && *name != wanted
        {
            a.prev_animscript = Some(name.clone());
        }
        a.anim_mode = "normal".into();
        a.orient = Orient::Default;
        a.animscript = Some((wanted.clone(), 0));
        a.animscript_started_ms = now;
    });
    let module = super::actors::animscript_module(world, id, &wanted);
    let main = format!("animscripts/{module}::main");
    let serial = run_script(world, &main, Value::Object(object));
    with_actor(world, id, |a| {
        a.animscript = Some((wanted, serial.unwrap_or(0)))
    });
}

/// A dying actor stops where it is.
pub(crate) fn stop(world: &mut World, id: ActorId) {
    with_actor(world, id, |a| a.traverse = None);
    clear_path(world, id);
    release_all(world, id);
}

/// `animscripted` on an actor: `animscripts/scripted::init` records the
/// arguments, then the `scripted` animscript plays them (`startscriptedanim`)
/// and holds the actor still for `duration_ms`. `None` when not an actor.
pub(crate) fn begin_scripted(
    world: &mut World,
    receiver: &Value,
    args: &[Value],
    duration_ms: i64,
) -> Option<()> {
    let (id, object) = receiver_actor(world, receiver).ok()?;
    let now = super::players::now_ms(world);
    clear_path(world, id);
    if let Err(fault) = run_now_thread(
        world,
        "animscripts/scripted::init",
        receiver.clone(),
        args.to_vec(),
        now,
    ) {
        diag::warn!(Sim, "actor: animscripts/scripted::init faulted: {fault}");
    }
    with_actor(world, id, |a| a.scripted_until_ms = now + duration_ms);
    switch_animscript(world, id, object, "scripted".into(), now);
    Some(())
}

/// `animcustom( func )`: the running animscript ends and `func` runs as the
/// actor's animscript (`custom`) until it returns.
fn begin_custom(world: &mut World, receiver: &Value, function: u32) -> Result<(), String> {
    let (id, object) = receiver_actor(world, receiver)?;
    let now = super::players::now_ms(world);
    if let Some((name, serial)) = actor(world, id).and_then(|a| a.animscript.clone()) {
        if thread_running(world, serial) {
            crate::script::runtime::notify_now(world, receiver.clone(), "killanimscript", now);
        }
        let module = super::actors::animscript_module(world, id, &name);
        run_script(
            world,
            &format!("animscripts/{module}::end_script"),
            receiver.clone(),
        );
    }
    clear_path(world, id);
    let serial =
        crate::script::runtime::spawn_function(world, function, receiver.clone(), Vec::new())?;
    with_actor(world, id, |a| {
        a.prev_animscript = a.animscript.take().map(|(name, _)| name);
        a.anim_mode = "normal".into();
        a.orient = Orient::Default;
        a.animscript = Some(("custom".into(), serial));
        a.animscript_started_ms = now;
    });
    Ok(())
}

/// `stopanimscripted`: the actor's state picks its animscript again.
pub(crate) fn end_scripted(world: &mut World, receiver: &Value) {
    if let Ok((id, object)) = receiver_actor(world, receiver) {
        with_actor(world, id, |a| a.scripted_until_ms = 0);
        world
            .resource_mut::<super::mechanics::Mechanics>()
            .stop(object, "origin");
    }
}

/// One actor's think: goal, cover or detour, path, movement, facing,
/// animscript, `"goal"`.
pub(crate) fn think(world: &mut World, id: ActorId, object: u64, now: i64, budget: &mut u32) {
    let linked = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .is_some_and(|e| e.linked_to.is_some());
    let scripted = actor(world, id).is_some_and(|a| now < a.scripted_until_ms);
    if linked || scripted {
        clear_path(world, id);
        select_animscript(world, id, object, now);
        return;
    }
    if actor(world, id).is_some_and(|a| a.traverse.is_some())
        && super::actor_motion::traverse_step(world, id, object, now)
    {
        let at = origin(world, object);
        face(world, id, object, at, true);
        return;
    }
    let at = origin(world, object);
    if let Some(goal_entity) = actor(world, id).and_then(|a| a.goal.entity) {
        if world.resource::<Runtime>().live(&goal_entity) {
            let pos = origin(world, goal_entity);
            with_actor(world, id, |a| a.goal.pos = pos);
        } else {
            with_actor(world, id, |a| a.goal.entity = None);
        }
    }
    if arriving(world, id, object) {
        let node = actor(world, id)
            .and_then(|a| a.claimed)
            .map(|n| FrameWorld::from_world(world).path_graph().nodes[n as usize].origin);
        let moved = node
            .and_then(|node| super::actor_motion::arrival_step(world, id, object, at, node))
            .unwrap_or_else(|| super::actor_cover::arrival_step(world, id, at));
        if moved != at {
            world
                .resource_mut::<super::mechanics::Mechanics>()
                .stop(object, "origin");
            world.resource_mut::<Runtime>().set_object_field(
                object,
                "origin",
                Value::Vector(moved),
            );
        }
        return;
    }
    let detour = !detour_done(world, id, object, now);
    if !detour {
        let target = code_target(world, id, object, at, now);
        follow_target(world, id, object, at, target, now, budget);
    }
    let anim_driven =
        actor(world, id).is_some_and(|a| super::actor_motion::ANIM_DRIVEN.contains(&&*a.anim_mode));
    let (moved, begin) = if anim_driven {
        let moved = super::actor_motion::anim_step(world, id, object, at);
        pass_reached(world, id, moved);
        (moved, None)
    } else {
        follow_path(world, id, object, at)
    };
    let moved = if moved == at {
        settle(world, id, object, at)
    } else {
        with_actor(world, id, |a| a.motion.settled = false);
        moved
    };
    if moved != at {
        world
            .resource_mut::<super::mechanics::Mechanics>()
            .stop(object, "origin");
        world
            .resource_mut::<Runtime>()
            .set_object_field(object, "origin", Value::Vector(moved));
    }
    if let Some((start, end)) = begin {
        super::actor_motion::begin_traverse(world, id, object, start, end, now);
        return;
    }
    super::actor_cover::approach_notify(world, id, object);
    face(world, id, object, moved, anim_driven);
    select_animscript(world, id, object, now);
    let at_goal = {
        let final_goal = actor(world, id).and_then(|a| a.path.as_ref().map(|p| p.final_goal));
        point_at_goal(world, id, moved)
            && final_goal.is_none_or(|end| point_at_goal(world, id, end))
    };
    if at_goal {
        raise(world, Value::Object(object), "goal", Vec::new());
        let first = with_actor(world, id, |a| {
            let first = !std::mem::replace(&mut a.goal_reached, true);
            (first, a.goal.node, a.distance_moved)
        });
        if let Some((true, node, distance)) = first {
            diag::info!(
                Sim,
                "actor: entity {object} notify \"goal\" at {:.0} {:.0} {:.0} node={node:?} moved={distance:.0}",
                moved[0],
                moved[1],
                moved[2]
            );
        }
    }
}

/// An actor that did not move this tick still stands on the floor: spawners
/// and cover nodes sit up to ~16 u above it, and only a moving hull met the
/// ground. Gravity applies until the hull lands, then it stays settled until
/// it moves again (no trace per idle tick).
fn settle(world: &mut World, id: ActorId, object: u64, at: [f32; 3]) -> [f32; 3] {
    let Some((settled, mode)) = actor(world, id).map(|a| (a.motion.settled, a.anim_mode.clone()))
    else {
        return at;
    };
    if settled || !matches!(&*mode, "normal" | "none" | "zonly_physics" | "gravity") {
        return at;
    }
    let Some(pos) =
        super::actor_motion::physics_move(world, id, object, at, [0.0; 3], Physics::Ground)
    else {
        // Stuck in solid: left where it stands, not retried every tick.
        with_actor(world, id, |a| a.motion.settled = true);
        return at;
    };
    let (landed, first) = actor(world, id).map_or((false, false), |a| {
        (
            a.motion.air_ms == 0 && a.motion.fall == 0.0,
            a.motion.air_ms == crate::MATCH_TICK_MS as i64,
        )
    });
    if landed && length(sub(pos, at)) < 0.01 {
        with_actor(world, id, |a| a.motion.settled = true);
        return at;
    }
    if first {
        diag::info!(
            Sim,
            "actor: entity {} idle above the floor at z={:.1}; drops to it",
            world
                .resource::<Runtime>()
                .entities
                .get(&object)
                .map_or(-1, |e| e.number),
            at[2]
        );
    }
    pos
}

/// `cover_arrival` is playing: its root motion carries the actor into the node.
fn arriving(world: &mut World, id: ActorId, object: u64) -> bool {
    let Some((arrival, script)) = actor(world, id).map(|a| (a.arrival, a.animscript.clone()))
    else {
        return false;
    };
    if arrival.is_none() {
        return false;
    }
    let running = script
        .is_some_and(|(name, serial)| &*name == "cover_arrival" && thread_running(world, serial));
    if !running {
        with_actor(world, id, |a| a.arrival = None);
        super::actor_motion::finish_arrival(world, id, object);
    }
    running
}

/// A detour (reacquire move, `setruntopos`) owns the path until it is walked;
/// a reacquire ends early once the enemy is in view.
fn detour_done(world: &mut World, id: ActorId, object: u64, now: i64) -> bool {
    let Some((detour, has_path, seen)) = actor(world, id).map(|a| {
        (
            a.detour,
            a.path.is_some(),
            a.enemy
                .and_then(|e| a.known.get(&e))
                .and_then(|k| k.seen_ms)
                .is_some_and(|t| now - t <= 300),
        )
    }) else {
        return true;
    };
    let Some(detour) = detour else {
        return true;
    };
    let reacquired = detour.kind == DetourKind::Reacquire && seen;
    if has_path && !reacquired {
        return false;
    }
    if reacquired {
        clear_path(world, id);
    }
    with_actor(world, id, |a| a.detour = None);
    if detour.kind == DetourKind::RunTo {
        raise(world, Value::Object(object), "runto_arrived", Vec::new());
    }
    true
}

/// A dog's enemy (`Actor_Dog_GetEnemyPos`): where it is, a quarter second ahead
/// while it moves, and how near counts as in reach (`meleeattackdist` + 15, + 15
/// more on a moving enemy).
fn dog_enemy(world: &mut World, id: ActorId) -> Option<([f32; 3], f32)> {
    let (enemy, mut reach) = actor(world, id)
        .filter(|a| &*a.species == "dog")
        .and_then(|a| Some((a.enemy?, a.float_field("meleeattackdist") + 15.0)))?;
    if !world.resource::<Runtime>().live(&enemy) {
        return None;
    }
    let mut pos = origin(world, enemy);
    let client = world
        .resource::<Runtime>()
        .player_client_of(&Value::Object(enemy));
    let velocity = client.and_then(|client| {
        FrameWorld::from_world(world)
            .player(crate::ClientId(client))
            .map(|ps| ps.velocity)
    });
    if let Some(v) = velocity.filter(|v| v[0] * v[0] + v[1] * v[1] > 1.0) {
        pos = add(pos, v.map(|c| c * 0.25));
        reach += 15.0;
    }
    Some((pos, reach))
}

fn in_reach(at: [f32; 3], pos: [f32; 3], reach: f32) -> bool {
    length2(sub(pos, at)) <= reach && (pos[2] - at[2]).abs() <= 80.0
}

/// `Actor_Dog_IsAttackScriptRunning`: a bite in progress keeps `combat` until the
/// script says it is safe to change.
fn dog_attacking(world: &World, id: ActorId) -> bool {
    actor(world, id).is_some_and(|a| {
        &*a.species == "dog"
            && !matches!(
                a.fields.get("safetochangescript"),
                None | Some(Value::Int(1..))
            )
    })
}

/// `Actor_SetMeleeAttackSpot`: `dist` from the enemy on the dog's side, else a
/// quarter turn either way or behind it, whichever the hull reaches the enemy from.
fn attack_spot(world: &mut World, at: [f32; 3], enemy: [f32; 3], dist: f32) -> [f32; 3] {
    let away = sub(at, enemy);
    let flat = length2(away).max(0.001);
    let (x, y) = (away[0] / flat, away[1] / flat);
    let frame = FrameWorld::from_world(world);
    let spots = [(x, y), (-y, x), (y, -x), (-x, -y)]
        .map(|(dx, dy)| [enemy[0] + dx * dist, enemy[1] + dy * dist, enemy[2]]);
    spots
        .iter()
        .copied()
        .find(|spot| {
            hull_clear(
                &frame,
                add(*spot, [0.0, 0.0, 1.0]),
                add(enemy, [0.0, 0.0, 1.0]),
            )
        })
        .unwrap_or(spots[0])
}

/// `Actor_FindPathToGoal`: with an enemy and no fixed node, the claimed cover node
/// in the goal; otherwise the script goal.
fn code_target(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
    now: i64,
) -> Option<([f32; 3], Option<u16>)> {
    let (fixed, keep, enemy, human, claimed) = actor(world, id).map(|a| {
        (
            a.float_field("fixednode") != 0.0,
            a.float_field("keepclaimednode") != 0.0,
            a.enemy.is_some(),
            &*a.species == "human",
            a.claimed,
        )
    })?;
    if let Some((pos, reach)) = dog_enemy(world, id)
        && point_at_goal(world, id, pos)
    {
        let dist = actor(world, id).map_or(0.0, |a| a.float_field("meleeattackdist"));
        return (!in_reach(at, pos, reach)).then(|| (attack_spot(world, at, pos, dist), None));
    }
    let cover = if keep {
        claimed
    } else if !fixed && enemy && human {
        super::actor_cover::claimed_cover(world, id, object, now)
    } else {
        None
    };
    if let Some(n) = cover {
        claim(world, id, n);
        let node = FrameWorld::from_world(world).path_graph().nodes[n as usize].clone();
        if super::actor_cover::near_node(at, &node) || enemy_in_fight_dist(world, id, at) {
            return None;
        }
        return Some((node.origin, Some(n)));
    }
    if holds_to_fight(world, id, at) {
        None
    } else {
        wanted_target(world, id, at)
    }
}

fn follow_target(
    world: &mut World,
    id: ActorId,
    object: u64,
    at: [f32; 3],
    target: Option<([f32; 3], Option<u16>)>,
    now: i64,
    budget: &mut u32,
) {
    let Some((target, node)) = target else {
        if actor(world, id).is_some_and(|a| a.path.is_some() || a.search.is_some()) {
            clear_path(world, id);
        }
        return;
    };
    if let Some(node) = node {
        claim(world, id, node);
    }
    let (current, searching, wait) = {
        let a = actor(world, id).unwrap();
        (
            a.path.as_ref().map(|p| p.final_goal),
            a.search.as_ref().map(|(_, t)| *t),
            a.path_wait_ms,
        )
    };
    let stale = |end: Option<[f32; 3]>| end.is_none_or(|e| length(sub(e, target)) > 1.0);
    let moving_goal = actor(world, id).is_some_and(|a| a.goal.entity.is_some());
    let replan = if moving_goal {
        current.is_some_and(|e| length(sub(e, target)) > 64.0)
    } else {
        stale(current)
    };
    if stale(current) && current.is_none() && stale(searching) && now >= wait {
        begin_path(world, id, object, at, target, now);
    } else if replan && current.is_some() && stale(searching) {
        with_actor(world, id, |a| a.path = None);
        if now >= wait {
            begin_path(world, id, object, at, target, now);
        }
    }
    continue_search(world, id, object, now, budget);
}

/// `Actor_CheckStop`: an enemy seen within `pathenemyfightdist` stops the move.
fn enemy_in_fight_dist(world: &mut World, id: ActorId, at: [f32; 3]) -> bool {
    let now = i64::from(world.resource::<crate::step::StepRequest>().tick.0)
        * i64::from(crate::MATCH_TICK_MS);
    actor(world, id).is_some_and(|a| {
        let Some(known) = a.enemy.and_then(|e| a.known.get(&e)) else {
            return false;
        };
        known.seen_ms.is_some_and(|t| now - t <= 500)
            && length(sub(known.pos, at)) <= a.float_field("pathenemyfightdist")
    })
}

/// With a known enemy an actor stops pathing once inside its goal (a goal
/// position, not a node), or within `pathenemyfightdist` of the enemy.
fn holds_to_fight(world: &mut World, id: ActorId, at: [f32; 3]) -> bool {
    let Some((enemy, fight_dist, node_goal)) = actor(world, id).and_then(|a| {
        let enemy = a.enemy?;
        let known = a.known.get(&enemy)?;
        Some((
            known.pos,
            a.float_field("pathenemyfightdist"),
            a.goal.node.is_some(),
        ))
    }) else {
        return false;
    };
    length(sub(enemy, at)) <= fight_dist || !node_goal && point_at_goal(world, id, at)
}

/// `Actor_PointAtGoal` for `isingoal`.
fn is_in_goal(world: &mut World, id: ActorId, point: [f32; 3]) -> bool {
    point_at_goal(world, id, point)
}

fn goal_volume_nodes(world: &mut World, volume: u64) -> Vec<u16> {
    let points: Vec<[f32; 3]> = FrameWorld::from_world(world)
        .path_graph()
        .nodes
        .iter()
        .map(|node| node.origin)
        .collect();
    super::triggers::contains_points(world, volume, &points)
        .into_iter()
        .enumerate()
        .filter(|(_, inside)| *inside)
        .map(|(n, _)| n as u16)
        .collect()
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};

    registry.register(Method, "setgoalpos", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let pos = vector(args, 0)?;
        set_goal_pos(world, id, pos, None);
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalnode", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let node = node_index(world, arg(args, 0)?)?;
        let (pos, linked) = {
            let frame = FrameWorld::from_world(world);
            let n = &frame.path_graph().nodes[node as usize];
            (n.origin, !n.links.is_empty() || n.spawnflags & 1 != 0)
        };
        if !linked {
            diag::warn!(
                Sim,
                "actor: entity {object} goal node at {:.0} {:.0} {:.0} has no path links",
                pos[0],
                pos[1],
                pos[2]
            );
        }
        set_goal_pos(world, id, pos, Some(node));
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalentity", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let target = entity_id(world, arg(args, 0)?)?;
        let pos = origin(world, target);
        with_actor(world, id, |a| {
            a.goal.entity = Some(target);
            a.goal_reached = false;
            a.goal.node = None;
            a.goal.volume = None;
            a.goal.pos = pos;
            a.path_wait_ms = 0;
        });
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalvolume", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let volume = entity_id(world, arg(args, 0)?)?;
        let goal = actor(world, id)
            .map(|a| a.goal.clone())
            .ok_or("receiver is not an actor")?;
        if goal.entity.is_some() {
            return Err("cannot set goal volume when a goal entity is set".into());
        }
        if !super::triggers::contains_point(world, volume, goal.pos) {
            return Err("cannot set goal volume which does not contain goal position".into());
        }
        with_actor(world, id, |a| a.goal.volume = Some(volume));
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalvolumeauto", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let volume = entity_id(world, arg(args, 0)?)?;
        let at = origin(world, object);
        let graph_nodes = goal_volume_nodes(world, volume);
        let best = {
            let frame = FrameWorld::from_world(world);
            let graph = frame.path_graph();
            graph_nodes.into_iter().min_by(|a, b| {
                let da = length(sub(graph.nodes[*a as usize].origin, at));
                let db = length(sub(graph.nodes[*b as usize].origin, at));
                da.total_cmp(&db).then(a.cmp(b))
            })
        };
        let pos = match best {
            Some(n) => FrameWorld::from_world(world).path_graph().nodes[n as usize].origin,
            None => origin(world, volume),
        };
        set_goal_pos(world, id, pos, None);
        with_actor(world, id, |a| a.goal.volume = Some(volume));
        Ok(Value::Undefined)
    });
    registry.register(Method, "getgoalvolume", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let volume = actor(world, id).and_then(|a| a.goal.volume);
        Ok(volume
            .filter(|v| world.resource::<Runtime>().live(v))
            .map_or(Value::Undefined, Value::Object))
    });
    registry.register(Method, "cleargoalvolume", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        with_actor(world, id, |a| a.goal.volume = None);
        Ok(Value::Undefined)
    });
    registry.register(Method, "isingoal", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        if args.len() != 1 {
            return Err("illegal call to isingoal()".into());
        }
        let point = vector(args, 0)?;
        Ok(Value::Int(is_in_goal(world, id, point).into()))
    });
    registry.register(Method, "orientmode", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let mode = string(args, 0)?.to_ascii_lowercase();
        let orient = match mode.as_str() {
            "face default" => Orient::Default,
            "face current" => Orient::Current,
            "face motion" => Orient::Motion,
            "face enemy" | "face enemy or motion" => Orient::Enemy,
            "face goal" => Orient::Default,
            "face angle" => Orient::Angle(float(args, 1)?),
            "face direction" => Orient::Angle(yaw_of(vector(args, 1)?)),
            "face point" => Orient::Point(vector(args, 1)?),
            other => return Err(format!("unknown orient mode '{other}'")),
        };
        with_actor(world, id, |a| a.orient = orient);
        Ok(Value::Undefined)
    });
    registry.register(Method, "animmode", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let mode = string(args, 0)?.to_ascii_lowercase();
        if !matches!(
            mode.as_str(),
            "normal"
                | "none"
                | "gravity"
                | "nogravity"
                | "zonly_physics"
                | "nophysics"
                | "noclip"
                | "angle deltas"
                | "point relative"
        ) {
            return Err(format!("unknown anim mode '{mode}'"));
        }
        with_actor(world, id, |a| a.anim_mode = mode.into());
        Ok(Value::Undefined)
    });
    registry.register(Method, "animcustom", |world, receiver, args| {
        match arg(args, 0)? {
            Value::Function(function) => begin_custom(world, receiver, *function)?,
            _ => return Err("animcustom expects a function".into()),
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "teleport", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let pos = vector(args, 0)?;
        teleport(world, id, object, pos, args.get(1));
        Ok(Value::Undefined)
    });
    // During a negotiation `forceteleport` only moves the actor (the traverse
    // scripts' height fix-ups); the path through the link is kept.
    registry.register(Method, "forceteleport", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let pos = vector(args, 0)?;
        if actor(world, id).is_some_and(|a| a.traverse.is_some()) {
            world
                .resource_mut::<super::mechanics::Mechanics>()
                .stop(object, "origin");
            let mut runtime = world.resource_mut::<Runtime>();
            runtime.set_object_field(object, "origin", Value::Vector(pos));
            if let Some(Value::Vector(angles)) = args.get(1) {
                runtime.set_object_field(object, "angles", Value::Vector([0.0, angles[1], 0.0]));
            }
            return Ok(Value::Undefined);
        }
        teleport(world, id, object, pos, args.get(1));
        Ok(Value::Undefined)
    });
    registry.register(Method, "getnegotiationstartnode", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let node = negotiation(world, id).map(|(start, _)| start);
        Ok(node_value(world, node))
    });
    registry.register(Method, "getnegotiationendnode", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let node = negotiation(world, id).map(|(_, end)| end);
        Ok(node_value(world, node))
    });
    registry.register(Method, "traversemode", |world, receiver, args| {
        let mode = string(args, 0)?.to_ascii_lowercase();
        super::actor_motion::set_traverse_mode(world, receiver, &mode)?;
        Ok(Value::Undefined)
    });
    registry.register(Method, "maymovetopoint", |world, receiver, args| {
        let (_, object) = receiver_actor(world, receiver)?;
        let to = vector(args, 0)?;
        let from = origin(world, object);
        let frame = super::presence::settled(world);
        Ok(Value::Int(
            hull_clear(&frame, add(from, [0.0, 0.0, 1.0]), add(to, [0.0, 0.0, 1.0])).into(),
        ))
    });
    registry.register(
        Method,
        "maymovefrompointtopoint",
        |world, receiver, args| {
            receiver_actor(world, receiver)?;
            let (from, to) = (vector(args, 0)?, vector(args, 1)?);
            let frame = super::presence::settled(world);
            Ok(Value::Int(
                hull_clear(&frame, add(from, [0.0, 0.0, 1.0]), add(to, [0.0, 0.0, 1.0])).into(),
            ))
        },
    );
    registry.register(Method, "getdoorpathnode", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let at = origin(world, object);
        let node = door_node(world, id, at);
        Ok(node_value(world, node))
    });
    registry.register(Method, "shouldfacemotion", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let facing =
            actor(world, id).is_some_and(|a| matches!(a.orient, Orient::Default | Orient::Motion));
        Ok(Value::Int(facing.into()))
    });
    registry.register(Function, "isnodeoccupied", |world, _, args| {
        let node = node_index(world, arg(args, 0)?)?;
        Ok(Value::Int(
            world
                .resource::<ActorPool>()
                .claims
                .contains_key(&node)
                .into(),
        ))
    });
    registry.register(Method, "nearnode", |world, receiver, args| {
        let (_, object) = receiver_actor(world, receiver)?;
        let node = node_index(world, arg(args, 0)?)?;
        let at = origin(world, object);
        let pos = FrameWorld::from_world(world).path_graph().nodes[node as usize].origin;
        Ok(Value::Int(
            ((at[2] - pos[2]).powi(2) <= 6400.0 && length2(sub(at, pos)) <= NODE_ARRIVE_DIST)
                .into(),
        ))
    });
}

/// The next door node (`Door`, `Door Interior`) on the path within reach.
fn door_node(world: &mut World, id: ActorId, at: [f32; 3]) -> Option<u16> {
    let points = {
        let path = actor(world, id)?.path.as_ref()?;
        path.points[path.next.min(path.points.len())..].to_vec()
    };
    let frame = FrameWorld::from_world(world);
    let graph = frame.path_graph();
    let mut from = at;
    let mut ahead = 0.0;
    for point in points {
        ahead += length(sub(point.pos, from));
        if ahead > DOOR_LOOKAHEAD {
            return None;
        }
        from = point.pos;
        if let Some(node) = point.node
            && matches!(graph.nodes[node as usize].node_type, 13 | 14)
        {
            return Some(node);
        }
    }
    None
}

/// The negotiation link being played, else the one ahead on the path: its
/// begin and end nodes.
fn negotiation(world: &World, id: ActorId) -> Option<(u16, u16)> {
    if let Some(n) = &actor(world, id)?.traverse {
        return Some((n.start, n.end));
    }
    let path = actor(world, id)?.path.as_ref()?;
    let i = (path.next..path.points.len()).find(|&i| path.points[i].traverse)?;
    Some((
        path.points.get(i.checked_sub(1)?)?.node?,
        path.points[i].node?,
    ))
}

pub(crate) fn teleport(
    world: &mut World,
    id: ActorId,
    object: u64,
    pos: [f32; 3],
    angles: Option<&Value>,
) {
    world
        .resource_mut::<super::mechanics::Mechanics>()
        .stop(object, "origin");
    let mut runtime = world.resource_mut::<Runtime>();
    runtime.set_object_field(object, "origin", Value::Vector(pos));
    if let Some(Value::Vector(angles)) = angles {
        runtime.set_object_field(object, "angles", Value::Vector([0.0, angles[1], 0.0]));
    }
    with_actor(world, id, |a| {
        a.path = None;
        a.search = None;
        a.nearest = None;
        a.velocity = [0.0; 3];
    });
}
