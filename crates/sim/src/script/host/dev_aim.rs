//! Dev-only helpers for scripted Spec Ops runs (console `enemies`, `autoaim`):
//! the hostile sentients a player could shoot, and a path node to see one from.
//! Read-only queries; they never change the simulation.

use super::actor_combat::{hostile, sentients};
use super::natives::engine::{TraceIgnore, entity_trace};
use super::sentients::eye;
use crate::bullet_collision::TraceOutcome;
use crate::frame::FrameWorld;
use crate::script::{Runtime, Value};
use bevy_ecs::prelude::World;

/// What stops a bullet, bodies aside: a clear line here is a clear shot.
const MASK_SIGHT: u32 =
    crate::bullet_collision::MASK_SHOT & !crate::bullet_collision::CONTENTS_BODY;
/// Below the eye: the chest.
const CHEST_DROP: f32 = 16.0;
const HEAD_DROP: f32 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct DevAimTarget {
    pub entnum: i32,
    pub team: String,
    pub classname: String,
    pub origin: [f32; 3],
    /// Chest (or head) point to aim at.
    pub aim: [f32; 3],
    pub dist: f32,
    /// No world geometry between the player's eye and `aim`.
    pub visible: bool,
    /// Where the sight line stops when it is not clear.
    pub blocked_at: Option<[f32; 3]>,
    pub health: i32,
    pub(crate) object: u64,
}

fn player_object(world: &World, client: u32) -> Option<u64> {
    world
        .resource::<Runtime>()
        .players
        .get(&client)
        .map(|p| p.object)
}

/// Where the sight line to `target` stops, ignoring the player and the
/// target's own body; `None` when it is clear.
fn blocked_at(
    world: &mut World,
    from: [f32; 3],
    to: [f32; 3],
    client: u32,
    target: u64,
) -> Option<[f32; 3]> {
    let model = world
        .resource::<Runtime>()
        .entities
        .get(&target)
        .and_then(|e| e.presence);
    let ignore = TraceIgnore {
        client: Some(crate::ClientId(client)),
        model,
        ..TraceIgnore::default()
    };
    match entity_trace(world, from, to, MASK_SIGHT, ignore) {
        TraceOutcome::Miss { .. } => None,
        TraceOutcome::Hit { end, .. } | TraceOutcome::StartSolid { end, .. } => Some(end),
        TraceOutcome::Invalid { .. } => Some(from),
    }
}

/// Live sentients hostile to `client`'s team (and, with `vehicles`, live
/// vehicles of a hostile `script_team`), nearest first.
pub(crate) fn targets(
    world: &mut World,
    client: u32,
    head: bool,
    vehicles: bool,
) -> Vec<DevAimTarget> {
    let Some(player) = player_object(world, client) else {
        return Vec::new();
    };
    let team = match super::players::load_field(world, client, "team") {
        Some(Value::String(team)) => team.to_string(),
        _ => "allies".into(),
    };
    let from = eye(world, player);
    let mut out = Vec::new();
    for s in sentients(world) {
        if s.object == player || !hostile(&team, &s.team) {
            continue;
        }
        let tag = if head { "j_head" } else { "j_spineupper" };
        // A dog has no `j_spineupper`: the middle of its body.
        let aim = match super::presence::tag_world(world, s.object, tag)
            .map(|(at, _)| at)
            .or_else(|| model_centre(world, s.object))
        {
            Some(at) => at,
            None => {
                let at = eye(world, s.object);
                [
                    at[0],
                    at[1],
                    at[2] - if head { HEAD_DROP } else { CHEST_DROP },
                ]
            }
        };
        let origin = match super::players::entity_field(world, s.object, "origin") {
            Value::Vector(v) => v,
            _ => aim,
        };
        let health = match super::players::entity_field(world, s.object, "health") {
            Value::Int(v) => v,
            Value::Float(v) => v as i32,
            _ => 0,
        };
        let classname = match super::players::entity_field(world, s.object, "classname") {
            Value::String(name) => name.to_string(),
            _ => String::new(),
        };
        let entnum = world
            .resource::<Runtime>()
            .entities
            .get(&s.object)
            .map_or(-1, |e| e.number);
        let d = [aim[0] - from[0], aim[1] - from[1], aim[2] - from[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let blocked_at = blocked_at(world, from, aim, client, s.object);
        let visible = blocked_at.is_none();
        out.push(DevAimTarget {
            entnum,
            team: s.team.to_string(),
            classname,
            origin,
            aim,
            dist,
            visible,
            blocked_at,
            health,
            object: s.object,
        });
    }
    if vehicles {
        let rows: Vec<(u64, Option<String>)> = {
            let runtime = world.resource::<Runtime>();
            runtime
                .vehicles
                .iter()
                .filter(|(id, _)| runtime.live(id))
                .map(|(id, v)| (*id, v.team.clone()))
                .collect()
        };
        for (object, team_override) in rows {
            let vehicle_team = team_override.or_else(|| {
                match super::players::entity_field(world, object, "script_team") {
                    Value::String(t) => Some(t.to_string()),
                    _ => None,
                }
            });
            if !vehicle_team.as_deref().is_some_and(|t| hostile(&team, t)) {
                continue;
            }
            let health = match super::players::entity_field(world, object, "health") {
                Value::Int(v) => v,
                Value::Float(v) => v as i32,
                _ => 0,
            };
            // `_vehicle` gives bullet damage back while an MP-style helicopter's
            // `bullet_armor` lasts: count it, so a hit shows as lost health.
            let health = health
                + match super::players::entity_field(world, object, "bullet_armor") {
                    Value::Int(v) => v.max(0),
                    Value::Float(v) => (v as i32).max(0),
                    _ => 0,
                };
            // `_vehicle` gives a `godmode` vehicle (invasion's UAV) its health back.
            let god = matches!(
                super::players::entity_field(world, object, "godmode"),
                Value::Int(1..)
            );
            if health <= 0 || god {
                continue;
            }
            let origin = match super::players::entity_field(world, object, "origin") {
                Value::Vector(v) => v,
                _ => continue,
            };
            let aim =
                model_centre(world, object).unwrap_or([origin[0], origin[1], origin[2] + 40.0]);
            let classname = match super::players::entity_field(world, object, "classname") {
                Value::String(name) => name.to_string(),
                _ => String::new(),
            };
            let entnum = world
                .resource::<Runtime>()
                .entities
                .get(&object)
                .map_or(-1, |e| e.number);
            let d = [aim[0] - from[0], aim[1] - from[1], aim[2] - from[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let blocked_at = blocked_at(world, from, aim, client, object);
            out.push(DevAimTarget {
                entnum,
                team: vehicle_team.unwrap_or_default(),
                classname,
                origin,
                aim,
                dist,
                visible: blocked_at.is_none(),
                blocked_at,
                health,
                object,
            });
        }
    }
    out.sort_by(|a, b| a.dist.total_cmp(&b.dist).then(a.entnum.cmp(&b.entnum)));
    out
}

/// The middle of the entity's model bounds where it stands now (a helicopter's
/// body hangs below its origin).
fn model_centre(world: &mut World, object: u64) -> Option<[f32; 3]> {
    let presence = world
        .resource::<Runtime>()
        .entities
        .get(&object)?
        .presence?;
    let mut frame = FrameWorld::from_world(world);
    let dobj = frame.collision_owner_mut(presence)?.dobj.as_ref()?;
    let (mid, half) = dobj.capability.as_ref()?.bounds?;
    if half.iter().all(|h| *h <= 0.0) {
        return None;
    }
    Some(
        dobj.world_from_model
            .transform_point3(glam::Vec3::from_array(mid))
            .to_array(),
    )
}

/// A path node from which the player would see the nearest hostile: between
/// `min`..`max` units of it, line of sight from standing eye height, nearest
/// to the player, none within 64 units of `avoid`. Feet origin.
pub(crate) fn hunt_spot(
    world: &mut World,
    client: u32,
    min: f32,
    max: f32,
    avoid: &[[f32; 3]],
    vehicles: bool,
) -> Option<[f32; 3]> {
    let player = player_object(world, client)?;
    let here = match super::players::entity_field(world, player, "origin") {
        Value::Vector(v) => v,
        _ => return None,
    };
    let targets = targets(world, client, false, vehicles);
    for target in &targets {
        let mut nodes: Vec<(f32, [f32; 3])> = FrameWorld::from_world(world)
            .path_graph()
            .nodes
            .iter()
            .filter_map(|n| {
                let d = [
                    n.origin[0] - target.origin[0],
                    n.origin[1] - target.origin[1],
                ];
                let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
                let dz = (n.origin[2] - target.origin[2]).abs();
                if flat < min || flat > max || dz > 160.0 {
                    return None;
                }
                let avoided = avoid.iter().any(|a| {
                    let d = [n.origin[0] - a[0], n.origin[1] - a[1], n.origin[2] - a[2]];
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 64.0 * 64.0
                });
                if avoided {
                    return None;
                }
                let p = [
                    n.origin[0] - here[0],
                    n.origin[1] - here[1],
                    n.origin[2] - here[2],
                ];
                Some(((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt(), n.origin))
            })
            .collect();
        nodes.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, node) in nodes.into_iter().take(64) {
            let from = [node[0], node[1], node[2] + 60.0];
            if blocked_at(world, from, target.aim, client, target.object).is_none() {
                return Some(node);
            }
        }
    }
    // Nothing sees one: close in on the nearest, at the node nearest to it.
    let target = targets.first()?;
    let far = |a: &[f32; 3], b: &[f32; 3], r: f32| {
        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= r * r
    };
    FrameWorld::from_world(world)
        .path_graph()
        .nodes
        .iter()
        .map(|n| n.origin)
        .filter(|o| far(o, &target.origin, 64.0) && avoid.iter().all(|a| far(o, a, 64.0)))
        .min_by(|a, b| {
            let da = [
                a[0] - target.origin[0],
                a[1] - target.origin[1],
                a[2] - target.origin[2],
            ];
            let db = [
                b[0] - target.origin[0],
                b[1] - target.origin[1],
                b[2] - target.origin[2],
            ];
            (da[0] * da[0] + da[1] * da[1] + da[2] * da[2])
                .total_cmp(&(db[0] * db[0] + db[1] * db[1] + db[2] * db[2]))
        })
}
