//! Actor cover and reacquire: cover node validity against the enemy, the engine's
//! cover score (distance, engagement, path visibility, node angle, target
//! direction, priority), the claimed node the code goal paths to, cover arrival,
//! `setruntopos`, reacquire steps and paths, and the path node stance queries.

use super::actor_nav::{node_index, node_value, path_now, point_at_goal, receiver_actor};
use super::args::{arg, float, optional, string, vector};
use super::natives::engine::{TraceIgnore, entity_trace};
use crate::actor::path;
use crate::actor::{
    ActorId, ActorPool, Detour, DetourKind, STANCE_CROUCH, STANCE_PRONE, STANCE_STAND,
};
use crate::bullet_collision::TraceOutcome;
use crate::frame::FrameWorld;
use crate::script::runtime::raise;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use crate::{SimPathGraph, SimPathNode};
use bevy_ecs::prelude::World;

const NODE_COVER_STAND: u32 = 2;
const NODE_COVER_CROUCH: u32 = 3;
const NODE_COVER_CROUCH_WINDOW: u32 = 4;
const NODE_COVER_PRONE: u32 = 5;
const NODE_COVER_RIGHT: u32 = 6;
const NODE_COVER_LEFT: u32 = 7;
const NODE_AMBUSH: u32 = 8;
const NODE_EXPOSED: u32 = 9;
const NODE_CONCEAL_STAND: u32 = 10;
const NODE_CONCEAL_CROUCH: u32 = 11;
const NODE_CONCEAL_PRONE: u32 = 12;
const NODE_TURRET: u32 = 18;
const NODE_GUARD: u32 = 19;
/// `Path_NodesInCylinder`'s cover type mask: cover, conceal, ambush, exposed, turret.
const COVER_TYPES: u32 = 0x41FFC;
const CONCEAL_TYPES: u32 = 0x1C00;
const PNF_PRIORITY: u16 = 0x40;
const PNF_NOT_STAND: u16 = 4;
const PNF_NOT_CROUCH: u16 = 8;
const PNF_NOT_PRONE: u16 = 0x10;
const MAX_CANDIDATES: usize = 256;
/// A full cover search runs at most this often per actor.
const COVER_SEARCH_MS: i64 = 1000;
const SENTIENT_NODE_MS: i64 = 500;
/// `Actor_PointNearNode`.
const NEAR_NODE_DIST: f32 = 15.0;
const MASK_AI_SIGHT: u32 = 0x0801;
const ARRIVAL_SPEED: f32 = 110.0;

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

fn dist2_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    dist2_sq(a, b) + (a[2] - b[2]).powi(2)
}

fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len > 0.0 {
        [v[0] / len, v[1] / len]
    } else {
        [0.0; 2]
    }
}

fn type_bit(node: &SimPathNode) -> u32 {
    1u32.checked_shl(node.node_type).unwrap_or(0)
}

/// `Actor_PointNearNode`.
pub(crate) fn near_node(at: [f32; 3], node: &SimPathNode) -> bool {
    (at[2] - node.origin[2]).powi(2) <= 80.0 * 80.0
        && dist2_sq(at, node.origin) <= NEAR_NODE_DIST * NEAR_NODE_DIST
}

/// The actor stands within `dist` of its claimed node (`Actor_NearCoverNode`).
pub(crate) fn near_claimed(world: &mut World, id: ActorId, object: u64, dist: f32) -> bool {
    let Some(claimed) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.claimed)
    else {
        return false;
    };
    let node = FrameWorld::from_world(world).path_graph().nodes[claimed as usize].origin;
    let at = origin(world, object);
    dist2_sq(at, node) <= dist * dist && (at[2] - node[2]).abs() <= 80.0
}

/// `Actor_Cover_GetAttackScript`: whether the node is usable for combat, and the
/// animscript an actor runs there.
fn attack_script(node: &SimPathNode, threatened: bool) -> (bool, Option<&'static str>) {
    match node.node_type {
        NODE_COVER_STAND | NODE_CONCEAL_STAND => (true, Some("cover_stand")),
        NODE_COVER_CROUCH | NODE_COVER_CROUCH_WINDOW | NODE_CONCEAL_CROUCH | NODE_TURRET => {
            (true, Some("cover_crouch"))
        }
        NODE_COVER_PRONE | NODE_CONCEAL_PRONE => (true, Some("cover_prone")),
        NODE_COVER_RIGHT => (true, Some("cover_right")),
        NODE_COVER_LEFT => (true, Some("cover_left")),
        NODE_EXPOSED | NODE_AMBUSH => (true, Some("combat")),
        NODE_GUARD if node.spawnflags & 0x80 == 0 || threatened => (true, Some("combat")),
        NODE_GUARD => (true, Some("stop")),
        _ => (node.spawnflags & 0x8000 != 0, None),
    }
}

/// `Actor_Cover_InitRange`: the yaw window (relative to the node) and minimum
/// distance an enemy must be in for the node to cover against it.
fn node_range(node: &SimPathNode) -> (f32, f32, f32) {
    match node.node_type {
        NODE_COVER_STAND | NODE_COVER_CROUCH | NODE_CONCEAL_STAND | NODE_CONCEAL_CROUCH
        | NODE_TURRET => (315.0, 45.0, 81225.0),
        NODE_COVER_CROUCH_WINDOW => (350.0, 10.0, 81225.0),
        NODE_COVER_PRONE => (330.0, 30.0, 640_000.0),
        NODE_COVER_RIGHT => (300.0, 14.0, 81225.0),
        NODE_COVER_LEFT => (348.0, 60.0, 81225.0),
        NODE_CONCEAL_PRONE => (330.0, 30.0, 81225.0),
        _ => (305.0, 55.0, 0.0),
    }
}

fn within_node_angle(pos: [f32; 3], node: &SimPathNode) -> bool {
    let (min, max, _) = node_range(node);
    let yaw = (pos[1] - node.origin[1])
        .atan2(pos[0] - node.origin[0])
        .to_degrees();
    let angle = math_iw4::angle_normalize_360(yaw - node.angle);
    if min <= max {
        (min..=max).contains(&angle)
    } else {
        angle >= min || angle <= max
    }
}

fn range_valid(pos: [f32; 3], node: &SimPathNode) -> bool {
    dist2_sq(pos, node.origin) >= node_range(node).2 && within_node_angle(pos, node)
}

/// What the actor knows of its enemy, as cover choice reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Threat {
    pub pos: [f32; 3],
    pub known: [f32; 3],
    pub visible: bool,
    pub node: Option<u16>,
}

fn sentient_node(world: &mut World, target: u64, now: i64) -> Option<u16> {
    let cached = world
        .resource::<ActorPool>()
        .sentient_nodes
        .get(&target)
        .copied()
        .filter(|(at, _)| now - at < SENTIENT_NODE_MS);
    if let Some((_, node)) = cached {
        return node;
    }
    let at = origin(world, target);
    let node = super::actor_nav::nearest_node(&FrameWorld::from_world(world), at);
    world
        .resource_mut::<ActorPool>()
        .sentient_nodes
        .insert(target, (now, node));
    node
}

pub(crate) fn threat(world: &mut World, id: ActorId) -> Option<Threat> {
    let now = now_ms(world);
    let (enemy, known) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| {
            let enemy = a.enemy?;
            Some((enemy, *a.known.get(&enemy)?))
        })?;
    if !world.resource::<Runtime>().live(&enemy) {
        return None;
    }
    let pos = origin(world, enemy);
    Some(Threat {
        pos,
        known: known.pos,
        visible: known.seen_ms.is_some_and(|t| now - t <= 500),
        node: sentient_node(world, enemy, now),
    })
}

/// `Actor_Cover_CheckWithEnemy`: the node covers against where the enemy is, or
/// failing that against where it was last known.
fn covers_against(node: &SimPathNode, threat: Option<&Threat>, check_range: bool) -> bool {
    let Some(threat) = threat else {
        return true;
    };
    if !(check_range || threat.visible) || range_valid(threat.pos, node) {
        return true;
    }
    range_valid(threat.known, node)
}

/// `Actor_Cover_PickAttackScript`: `None` when the node is no combat node;
/// `Some(None)` when it is but does not cover against the enemy.
pub(crate) fn pick_attack_script(
    node: &SimPathNode,
    threat: Option<&Threat>,
    check_range: bool,
) -> Option<Option<&'static str>> {
    let (usable, script) = attack_script(node, threat.is_some());
    if !usable {
        return None;
    }
    if covers_against(node, threat, check_range) {
        Some(script)
    } else {
        Some(None)
    }
}

fn claimable(world: &World, id: ActorId, node: u16) -> bool {
    world
        .resource::<ActorPool>()
        .claims
        .get(&node)
        .is_none_or(|owner| *owner == id)
}

/// `Actor_Cover_IsValidCover`.
fn valid_cover(
    world: &World,
    id: ActorId,
    graph: &SimPathGraph,
    n: u16,
    threat: Option<&Threat>,
) -> bool {
    if !claimable(world, id, n) {
        return false;
    }
    let node = &graph.nodes[n as usize];
    if let Some(threat) = threat {
        let to = [
            threat.pos[0] - node.origin[0],
            threat.pos[1] - node.origin[1],
        ];
        if node.forward[0] * to[0] + node.forward[1] * to[1] < 0.0 {
            return false;
        }
    }
    !matches!(pick_attack_script(node, threat, true), Some(None))
}

/// `Path_NodesVisible`: the precomputed node-to-node visibility bit.
fn nodes_visible(graph: &SimPathGraph, a: u16, b: u16) -> bool {
    let (i, j) = (a.min(b) as usize, a.max(b) as usize);
    if i == j || graph.vis.is_empty() {
        return true;
    }
    let index = graph.nodes.len() * i + j;
    graph
        .vis
        .get(index >> 3)
        .is_some_and(|byte| byte & (1 << (index & 7)) != 0)
}

#[derive(Clone, Copy, Debug)]
struct Engagement {
    min: f32,
    min_falloff: f32,
    max: f32,
    max_falloff: f32,
}

impl Engagement {
    /// `Actor_Cover_ScoreOnEngagement`.
    fn score(&self, dist: f32) -> f32 {
        if dist < self.min_falloff || dist > self.max_falloff {
            0.0
        } else if dist < self.min {
            1.0 - (self.min - dist) / (self.min - self.min_falloff).max(1e-3)
        } else if dist <= self.max {
            1.0
        } else {
            (self.max_falloff - dist) / (self.max_falloff - self.max).max(1e-3)
        }
    }
}

fn dvar(world: &World, name: &str, default: f32) -> f32 {
    world
        .resource::<Runtime>()
        .dvars
        .get(name)
        .and_then(|text| text.trim().parse::<f32>().ok())
        .unwrap_or(default)
}

/// Weights of `ai_coverScore_*`.
struct Weights {
    distance: f32,
    engagement: f32,
    visibility: f32,
    cover_type: f32,
    node_angle: f32,
    target_dir: f32,
    player_los: f32,
    priority: f32,
}

impl Weights {
    fn read(world: &World) -> Self {
        Self {
            distance: dvar(world, "ai_coverscore_distance", 16.0),
            engagement: dvar(world, "ai_coverscore_engagement", 4.0),
            visibility: dvar(world, "ai_coverscore_visibility", 8.0),
            cover_type: dvar(world, "ai_coverscore_covertype", 2.0),
            node_angle: dvar(world, "ai_coverscore_nodeangle", 4.0),
            target_dir: dvar(world, "ai_coverscore_targetdir", 4.0),
            player_los: dvar(world, "ai_coverscore_playerlos", 8.0),
            priority: dvar(world, "ai_coverscore_priority", 8.0),
        }
    }
}

/// `Actor_Cover_GetNodeDistMetric` and `Actor_Cover_GetNodeMetric`.
fn node_metric(
    graph: &SimPathGraph,
    n: u16,
    at: [f32; 3],
    threat: Option<&Threat>,
    engagement: Engagement,
    w: &Weights,
) -> (f32, f32) {
    let node = &graph.nodes[n as usize];
    let dist_metric = (1.0 - dist_sq(at, node.origin).min(1_440_000.0) / 1_440_000.0) * w.distance;
    let mut metric = dist_metric + w.player_los;
    if let Some(threat) = threat {
        metric += engagement.score(dist_sq(threat.pos, node.origin).sqrt()) * w.engagement;
        if within_node_angle(threat.pos, node) {
            metric += w.node_angle;
        }
        metric += if near_node(at, node) {
            1.0
        } else {
            let to_target = normalize2([
                threat.pos[0] - node.origin[0],
                threat.pos[1] - node.origin[1],
            ]);
            let to_node = normalize2([node.origin[0] - at[0], node.origin[1] - at[1]]);
            (to_node[0] * to_target[0] + to_node[1] * to_target[1] + 1.0) * 0.5
        } * w.target_dir;
    }
    if threat
        .and_then(|t| t.node)
        .is_none_or(|enemy_node| nodes_visible(graph, n, enemy_node))
    {
        metric += w.visibility;
    }
    if type_bit(node) & CONCEAL_TYPES == 0 {
        metric += w.cover_type;
    }
    if node.spawnflags & PNF_PRIORITY != 0 {
        metric += w.priority;
    }
    (metric, dist_metric)
}

/// `Actor_Cover_FindBestCoverList`: valid cover nodes in the goal, best first. The
/// claimed node loses its distance bonus unless it would still rank first, so
/// actors advance rather than stay put.
pub(crate) fn best_cover_list(
    world: &mut World,
    id: ActorId,
    object: u64,
    limit: usize,
) -> Vec<u16> {
    let Some((goal, radius, height, claimed, engagement)) =
        world.resource::<ActorPool>().actors.get(&id).map(|a| {
            (
                a.goal.clone(),
                a.float_field("goalradius"),
                a.float_field("goalheight"),
                a.claimed,
                Engagement {
                    min: a.float_field("engagemindist"),
                    min_falloff: a.float_field("engageminfalloffdist"),
                    max: a.float_field("engagemaxdist"),
                    max_falloff: a.float_field("engagemaxfalloffdist"),
                },
            )
        })
    else {
        return Vec::new();
    };
    let threat = threat(world, id);
    let at = origin(world, object);
    let content = FrameWorld::from_world(world).content();
    let graph = content.path_graph();
    let mut candidates: Vec<u16> = path::nodes_in_cylinder(graph, goal.pos, radius, height)
        .into_iter()
        .filter(|&n| type_bit(&graph.nodes[n as usize]) & COVER_TYPES != 0)
        .take(MAX_CANDIDATES)
        .collect();
    if let Some(volume) = goal.volume.filter(|v| world.resource::<Runtime>().live(v)) {
        candidates.retain(|&n| {
            super::triggers::contains_point(world, volume, graph.nodes[n as usize].origin)
        });
    }
    let weights = Weights::read(world);
    let mut scored: Vec<(f32, f32, u16)> = candidates
        .into_iter()
        .filter(|&n| valid_cover(world, id, graph, n, threat.as_ref()))
        .map(|n| {
            let (metric, dist) = node_metric(graph, n, at, threat.as_ref(), engagement, &weights);
            (metric - dist, dist, n)
        })
        .collect();
    let by_metric =
        |a: &(f32, f32, u16), b: &(f32, f32, u16)| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2));
    scored.sort_by(by_metric);
    let count = scored.len();
    for (i, row) in scored.iter_mut().enumerate() {
        if Some(row.2) != claimed || i == 0 && count == 1 {
            row.0 += row.1;
        }
    }
    scored.sort_by(by_metric);
    scored.into_iter().take(limit).map(|(_, _, n)| n).collect()
}

/// `Actor_FindClaimedNode`: the cover node the code goal paths to while the actor
/// has an enemy. A claimed node still in the goal and still covering is kept;
/// otherwise the best cover in the goal, searched at most once a second.
pub(crate) fn claimed_cover(world: &mut World, id: ActorId, object: u64, now: i64) -> Option<u16> {
    let (claimed, search_due) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .map(|a| (a.claimed, now >= a.cover_search_ms))?;
    let content = FrameWorld::from_world(world).content();
    let graph = content.path_graph();
    let threat = threat(world, id);
    let kept = claimed.filter(|&n| {
        let node = &graph.nodes[n as usize];
        type_bit(node) & COVER_TYPES != 0
            && point_at_goal(world, id, node.origin)
            && valid_cover(world, id, graph, n, threat.as_ref())
    });
    if kept.is_some() || !search_due {
        return kept;
    }
    let number = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .map_or(0, |e| e.number);
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.cover_search_ms = now + COVER_SEARCH_MS + i64::from(number % 8) * 50;
    }
    let best = best_cover_list(world, id, object, 1).first().copied();
    if let Some(n) = best
        && best != claimed
    {
        let node = &graph.nodes[n as usize];
        let at = origin(world, object);
        diag::info!(
            Sim,
            "actor: entity {number} cover node {n} \"{}\" at {:.0} {:.0} {:.0} dist={:.0} enemy_dist={:.0}",
            node.type_name(),
            node.origin[0],
            node.origin[1],
            node.origin[2],
            dist_sq(at, node.origin).sqrt(),
            threat.map_or(0.0, |t| dist_sq(t.pos, node.origin).sqrt())
        );
    }
    best
}

/// The animscript an actor without a path runs: its claimed node's attack script
/// when it stands at that node and the node covers against the enemy, else `combat`.
pub(crate) fn combat_script(world: &mut World, id: ActorId, object: u64) -> &'static str {
    let Some(claimed) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.claimed)
    else {
        return "combat";
    };
    let content = FrameWorld::from_world(world).content();
    let node = &content.path_graph().nodes[claimed as usize];
    let at = origin(world, object);
    if !near_node(at, node) {
        return "combat";
    }
    let threat = threat(world, id);
    match pick_attack_script(node, threat.as_ref(), false) {
        Some(Some(script)) => script,
        _ => "combat",
    }
}

/// The node a cover arrival is heading into and the yaw it ends facing.
fn arrival_node(world: &World, id: ActorId) -> Option<u16> {
    world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.claimed)
}

/// Movement during `cover_arrival`: anim deltas would carry the actor from the
/// arrival start into the node; without root motion it slides there.
pub(crate) fn arrival_step(world: &mut World, id: ActorId, at: [f32; 3]) -> [f32; 3] {
    let Some(node) = arrival_node(world, id) else {
        return at;
    };
    let to = FrameWorld::from_world(world).path_graph().nodes[node as usize].origin;
    let d = sub(to, at);
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let step = ARRIVAL_SPEED * crate::MATCH_TICK_MS as f32 / 1000.0;
    if len > 256.0 {
        return at;
    }
    if len <= step {
        return to;
    }
    [
        at[0] + d[0] * step / len,
        at[1] + d[1] * step / len,
        at[2] + d[2] * step / len,
    ]
}

/// `"cover_approach"` once per path into a claimed cover node, when the move
/// script asked for it (`requestarrivalnotify`); the direction the actor comes in.
pub(crate) fn approach_notify(world: &mut World, id: ActorId, object: u64) {
    let Some((wanted, claimed, points)) = world.resource::<ActorPool>().actors.get(&id).map(|a| {
        (
            a.float_field("requestarrivalnotify") != 0.0,
            a.claimed,
            a.path
                .as_ref()
                .map(|p| (p.final_goal, p.points.len(), p.points.clone())),
        )
    }) else {
        return;
    };
    let (Some(claimed), Some((goal, _, points))) = (claimed, points) else {
        return;
    };
    if !wanted {
        return;
    }
    let node = FrameWorld::from_world(world).path_graph().nodes[claimed as usize].clone();
    if dist_sq(goal, node.origin) > 1.0 {
        return;
    }
    let at = origin(world, object);
    let from = points
        .iter()
        .rev()
        .map(|p| p.pos)
        .find(|p| dist2_sq(*p, node.origin) > 32.0 * 32.0)
        .unwrap_or(at);
    let dir = normalize2([node.origin[0] - from[0], node.origin[1] - from[1]]);
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.fields.insert("requestarrivalnotify", Value::Int(0));
    }
    raise(
        world,
        Value::Object(object),
        "cover_approach",
        vec![Value::Vector([dir[0], dir[1], 0.0])],
    );
}

fn sees_point(world: &mut World, object: u64, from: [f32; 3], to: [f32; 3], target: u64) -> bool {
    let ignore = {
        let runtime = world.resource::<Runtime>();
        TraceIgnore {
            client: runtime.player_client(target).map(crate::ClientId),
            other_client: None,
            model: runtime.entities.get(&object).and_then(|e| e.presence),
        }
    };
    matches!(
        entity_trace(world, from, to, MASK_AI_SIGHT, ignore),
        TraceOutcome::Miss { .. }
    )
}

/// `Actor_Exposed_ReacquireStepMove`: a sidestep of `dist` either way that brings
/// the enemy into view and stays in the goal.
fn reacquire_step(world: &mut World, id: ActorId, object: u64, dist: f32) -> bool {
    let Some(enemy) = world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.enemy)
        .filter(|e| world.resource::<Runtime>().live(e))
    else {
        return false;
    };
    let at = origin(world, object);
    let target = origin(world, enemy);
    let forward = normalize2([target[0] - at[0], target[1] - at[1]]);
    if forward == [0.0; 2] {
        return false;
    }
    let side = [forward[1], -forward[0]];
    let look = super::sentients::eye(world, enemy);
    let eye = super::sentients::eye(world, object);
    let first = if world.resource_mut::<ActorPool>().random() < 0.5 {
        1.0
    } else {
        -1.0
    };
    for sign in [first, -first] {
        let s = sign * dist;
        let check = [eye[0] + side[0] * s, eye[1] + side[1] * s, eye[2]];
        if !sees_point(world, object, check, look, enemy) {
            continue;
        }
        let to = [at[0] + side[0] * s, at[1] + side[1] * s, at[2]];
        let clear = {
            let frame = FrameWorld::from_world(world);
            super::actor_nav::hull_clear(
                &frame,
                [at[0], at[1], at[2] + 1.0],
                [to[0], to[1], to[2] + 1.0],
            )
        };
        if !clear || !point_at_goal(world, id, to) {
            continue;
        }
        set_detour(world, id, to, DetourKind::Reacquire);
        super::actor_nav::set_direct_path(world, id, to);
        let number = world.resource::<Runtime>().entities[&object].number;
        diag::info!(Sim, "actor: entity {number} reacquire step {s:.0}");
        return true;
    }
    false
}

fn set_detour(world: &mut World, id: ActorId, pos: [f32; 3], kind: DetourKind) {
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.detour = Some(Detour { pos, kind });
        a.path_wait_ms = 0;
    }
}

/// `findreacquiredirectpath` / `findreacquireproximatepath`: a path toward the
/// enemy's last known position (the proximate one stops halfway), left for
/// `reacquiremove` to commit to.
fn reacquire_path(world: &mut World, id: ActorId, object: u64, fraction: f32) {
    let Some(known) = world.resource::<ActorPool>().actors.get(&id).and_then(|a| {
        let enemy = a.enemy?;
        Some(a.known.get(&enemy)?.pos)
    }) else {
        super::actor_nav::clear_path(world, id);
        return;
    };
    let at = origin(world, object);
    let to = [
        at[0] + (known[0] - at[0]) * fraction,
        at[1] + (known[1] - at[1]) * fraction,
        known[2],
    ];
    let to = if point_at_goal(world, id, to) {
        to
    } else {
        let goal = world.resource::<ActorPool>().actors[&id].goal.pos;
        let toward = sub(to, goal);
        let radius = world.resource::<ActorPool>().actors[&id].float_field("goalradius");
        let len = (toward[0] * toward[0] + toward[1] * toward[1])
            .sqrt()
            .max(1.0);
        let keep = (radius - 16.0).max(0.0) / len;
        if keep >= 1.0 {
            to
        } else {
            [
                goal[0] + toward[0] * keep,
                goal[1] + toward[1] * keep,
                to[2],
            ]
        }
    };
    if path_now(world, id, object, to) {
        set_detour(world, id, to, DetourKind::Reacquire);
    }
}

fn node_stances(node: &SimPathNode) -> u8 {
    let mut stances = STANCE_STAND | STANCE_CROUCH | STANCE_PRONE;
    if node.spawnflags & PNF_NOT_STAND != 0 {
        stances &= !STANCE_STAND;
    }
    if node.spawnflags & PNF_NOT_CROUCH != 0 {
        stances &= !STANCE_CROUCH;
    }
    if node.spawnflags & PNF_NOT_PRONE != 0 {
        stances &= !STANCE_PRONE;
    }
    stances
}

fn node_of(world: &mut World, value: &Value) -> Result<(u16, SimPathNode), String> {
    let n = node_index(world, value)?;
    let node = FrameWorld::from_world(world).path_graph().nodes[n as usize].clone();
    Ok((n, node))
}

/// `UseCoverNode`: claim a valid cover node and path to it.
fn use_cover_node(world: &mut World, id: ActorId, object: u64, n: u16) -> bool {
    let content = FrameWorld::from_world(world).content();
    let graph = content.path_graph();
    let threat = threat(world, id);
    if !valid_cover(world, id, graph, n, threat.as_ref()) {
        return false;
    }
    let pos = graph.nodes[n as usize].origin;
    if !near_node(origin(world, object), &graph.nodes[n as usize])
        && !path_now(world, id, object, pos)
    {
        return false;
    }
    super::actor_nav::claim(world, id, n);
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.cover_list.clear();
        a.detour = None;
    }
    true
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::Method;

    registry.register(Method, "findbestcovernode", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let best = best_cover_list(world, id, object, 1).first().copied();
        Ok(node_value(world, best))
    });
    registry.register(Method, "findcovernode", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let has_enemy = world.resource::<ActorPool>().actors[&id].enemy.is_some();
        let mut list = if has_enemy {
            best_cover_list(world, id, object, 1000)
        } else {
            Vec::new()
        };
        list.reverse();
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            a.cover_list = list;
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "getcovernode", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let node = world
            .resource_mut::<ActorPool>()
            .actors
            .get_mut(&id)
            .and_then(|a| {
                if a.enemy.is_some() {
                    a.cover_list.pop()
                } else {
                    None
                }
            });
        Ok(node_value(world, node))
    });
    registry.register(Method, "usecovernode", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let (fixed, keep) = {
            let a = &world.resource::<ActorPool>().actors[&id];
            (
                a.float_field("fixednode") != 0.0,
                a.float_field("keepclaimednode") != 0.0,
            )
        };
        if fixed {
            return Err("cannot change node when using fixedNode mode".into());
        }
        if keep {
            return Err("cannot change node when keepclaimednode is set".into());
        }
        let n = node_index(world, arg(args, 0)?)?;
        Ok(Value::Int(use_cover_node(world, id, object, n).into()))
    });
    // The claimed node still covers against the enemy (`Actor_Cover_CheckWithEnemy`).
    registry.register(Method, "iscovervalidagainstenemy", |world, receiver, _| {
        let (id, _) = receiver_actor(world, receiver)?;
        let Some(claimed) = world.resource::<ActorPool>().actors[&id].claimed else {
            return Ok(Value::Int(0));
        };
        let node = FrameWorld::from_world(world).path_graph().nodes[claimed as usize].clone();
        let threat = threat(world, id);
        Ok(Value::Int(
            covers_against(&node, threat.as_ref(), false).into(),
        ))
    });
    // No other cover node is offered for a sideways shuffle.
    registry.register(Method, "findshufflecovernode", |world, receiver, _| {
        receiver_actor(world, receiver)?;
        Ok(Value::Undefined)
    });
    registry.register(Method, "nearclaimnode", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let Some(claimed) = world.resource::<ActorPool>().actors[&id].claimed else {
            return Ok(Value::Int(0));
        };
        let node = FrameWorld::from_world(world).path_graph().nodes[claimed as usize].clone();
        Ok(Value::Int(near_node(origin(world, object), &node).into()))
    });
    registry.register(Method, "nearclaimnodeandangle", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let Some(claimed) = world.resource::<ActorPool>().actors[&id].claimed else {
            return Ok(Value::Int(0));
        };
        let node = FrameWorld::from_world(world).path_graph().nodes[claimed as usize].clone();
        let yaw = match super::players::entity_field(world, object, "angles") {
            Value::Vector(v) => v[1],
            _ => 0.0,
        };
        let facing =
            (math_iw4::angle_normalize_360(yaw - node.angle + 180.0) - 180.0).abs() <= 45.0;
        Ok(Value::Int(
            (near_node(origin(world, object), &node) && facing).into(),
        ))
    });
    registry.register(Method, "atdangerousnode", |world, receiver, _| {
        receiver_actor(world, receiver)?;
        Ok(Value::Int(0))
    });
    registry.register(Method, "startcoverarrival", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        vector(args, 0)?;
        let yaw = float(args, 1)?;
        let now = now_ms(world);
        let node = arrival_node(world, id);
        if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
            a.arrival = Some(yaw);
            a.detour = None;
        }
        super::actor_nav::clear_path(world, id);
        world.resource_mut::<Runtime>().set_object_field(
            object,
            "angles",
            Value::Vector([0.0, yaw, 0.0]),
        );
        let number = world.resource::<Runtime>().entities[&object].number;
        diag::info!(
            Sim,
            "actor: entity {number} startcoverarrival node={node:?} yaw={yaw:.0}"
        );
        super::actor_nav::switch_animscript(world, id, object, "cover_arrival".into(), now);
        Ok(Value::Undefined)
    });
    // `Actor_CheckCoverLeave`: the exit leads along the path rather than back.
    registry.register(
        Method,
        "checkcoverexitposwithpath",
        |world, receiver, args| {
            let (id, object) = receiver_actor(world, receiver)?;
            let exit = vector(args, 0)?;
            let at = origin(world, object);
            let next = world.resource::<ActorPool>().actors[&id]
                .path
                .as_ref()
                .and_then(|p| {
                    p.points[p.next.min(p.points.len())..]
                        .iter()
                        .find(|pt| dist2_sq(pt.pos, at) > 16.0 * 16.0)
                        .map(|pt| pt.pos)
                });
            let Some(next) = next else {
                return Ok(Value::Int(0));
            };
            let along =
                (next[0] - at[0]) * (exit[0] - at[0]) + (next[1] - at[1]) * (exit[1] - at[1]);
            Ok(Value::Int((along >= 0.0).into()))
        },
    );
    registry.register(Method, "setruntopos", |world, receiver, args| {
        let (id, _) = receiver_actor(world, receiver)?;
        let pos = vector(args, 0)?;
        set_detour(world, id, pos, DetourKind::RunTo);
        Ok(Value::Undefined)
    });
    registry.register(Method, "reacquirestep", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let dist = float(args, 0)?;
        Ok(Value::Int(reacquire_step(world, id, object, dist).into()))
    });
    registry.register(
        Method,
        "findreacquiredirectpath",
        |world, receiver, args| {
            let (id, object) = receiver_actor(world, receiver)?;
            optional(args, 0, super::args::int)?;
            reacquire_path(world, id, object, 1.0);
            Ok(Value::Undefined)
        },
    );
    registry.register(
        Method,
        "findreacquireproximatepath",
        |world, receiver, args| {
            let (id, object) = receiver_actor(world, receiver)?;
            optional(args, 0, super::args::int)?;
            reacquire_path(world, id, object, 0.5);
            Ok(Value::Undefined)
        },
    );
    // `Actor_Exposed_StartReacquireMove`: commit to the path the find left.
    registry.register(Method, "reacquiremove", |world, receiver, _| {
        let (id, object) = receiver_actor(world, receiver)?;
        let moving = world.resource::<ActorPool>().actors[&id].path.is_some()
            && world.resource::<ActorPool>().actors[&id]
                .detour
                .is_some_and(|d| d.kind == DetourKind::Reacquire);
        if moving {
            let number = world.resource::<Runtime>().entities[&object].number;
            diag::info!(Sim, "actor: entity {number} reacquire move");
        }
        Ok(Value::Int(moving.into()))
    });
    registry.register(Method, "trimpathtoattack", |world, receiver, _| {
        receiver_actor(world, receiver)?;
        Ok(Value::Int(0))
    });
    registry.register(Method, "safeteleport", |world, receiver, args| {
        let (id, object) = receiver_actor(world, receiver)?;
        let pos = vector(args, 0)?;
        let at = origin(world, object);
        if dist_sq(at, pos) > 64.0 * 64.0 {
            return Ok(Value::Undefined);
        }
        super::actor_nav::teleport(world, id, object, pos, args.get(1));
        Ok(Value::Undefined)
    });
    // The node faces the way the path runs through it.
    registry.register(
        Method,
        "comparenodedirtopathdir",
        |world, receiver, args| {
            let (_, object) = receiver_actor(world, receiver)?;
            let (_, node) = node_of(world, arg(args, 0)?)?;
            let at = origin(world, object);
            let along = node.forward[0] * (node.origin[0] - at[0])
                + node.forward[1] * (node.origin[1] - at[1]);
            Ok(Value::Int((along >= 0.0).into()))
        },
    );
    registry.register(Method, "doesnodeallowstance", |world, receiver, args| {
        let (_, node) = node_of(world, receiver)?;
        let stance = match string(args, 0)?.as_str() {
            "stand" => STANCE_STAND,
            "crouch" => STANCE_CROUCH,
            "prone" => STANCE_PRONE,
            other => return Err(format!("unknown stance '{other}'")),
        };
        Ok(Value::Int((node_stances(&node) & stance != 0).into()))
    });
    registry.register(Method, "gethighestnodestance", |world, receiver, _| {
        let (_, node) = node_of(world, receiver)?;
        let stances = node_stances(&node);
        Ok(Value::string(if stances & STANCE_STAND != 0 {
            "stand"
        } else if stances & STANCE_CROUCH != 0 {
            "crouch"
        } else {
            "prone"
        }))
    });
    // Cover nodes offer no extra peek-outs beyond the cover type's own.
    registry.register(Method, "getvalidcoverpeekouts", |world, receiver, _| {
        node_of(world, receiver)?;
        super::arrays::new_array(world, Vec::new())
    });
}
