//! Test aimer for scripted runs. `IW4L_AUTOAIM=1` (`=2` also logs): while a
//! player holds attack, its view turns to the nearest hostile actor it can see,
//! so a scripted run can fight AI whose positions are not known up front. The
//! shot is the player's ordinary weapon fire. `=3` (hunt) also logs, and when
//! no hostile is in sight moves the player (on whole seconds of level time) to a path
//! node that sees the nearest one, for missions that are won by kills.

use super::actor_nav::nearest_node;
use super::actors::actor_of;
use super::natives::engine::{TraceIgnore, entity_trace};
use super::sentients::eye;
use crate::actor::ActorPool;
use crate::bullet_collision::{MASK_SHOT, TraceOutcome};
use crate::frame::FrameWorld;
use crate::script::Runtime;
use bevy_ecs::prelude::{Resource, World};
use playerstate_iw4::UserCmd;

/// Farther than this an actor is not picked (beyond the SP sniper engagement range).
const RANGE: f32 = 4000.0;
/// In hunt mode a farther target is walked up to rather than shot at.
const HUNT_RANGE: f32 = 1500.0;
/// Below the eye: the upper chest, where a miss still lands on the body.
const CHEST_DROP: f32 = 10.0;
/// A hunt stop is this far from the actor: close enough to hit, not on top of it.
const HUNT_MIN: f32 = 150.0;
const HUNT_MAX: f32 = 900.0;
const HUNT_EYE: f32 = 60.0;
const HUNT_EVERY_MS: i64 = 1000;
/// A target aimed at this long without taking a hit (the line is clear for the
/// sight trace but not for bullets) is passed over for `SKIP_MS`.
const FUTILE_MS: i64 = 4000;
const SKIP_MS: i64 = 20000;

/// What the aimer is on: the target, since when, and its hit count then; and
/// targets it gave up on until a time.
#[derive(Resource, Default)]
struct TestAim {
    on: Option<(u64, i64, u32)>,
    skip: std::collections::BTreeMap<u64, i64>,
}

fn mode() -> u8 {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        std::env::var("IW4L_AUTOAIM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn clear(world: &mut World, client: u32, object: u64, from: [f32; 3], to: [f32; 3]) -> bool {
    let model = world
        .resource::<Runtime>()
        .entities
        .get(&object)
        .and_then(|e| e.presence);
    let ignore = TraceIgnore {
        client: Some(crate::ClientId(client)),
        other_client: None,
        model,
    };
    matches!(
        entity_trace(world, from, to, MASK_SHOT, ignore),
        TraceOutcome::Miss { .. }
    )
}

/// Live hostile actors, nearest first, with their aim point.
fn hostiles(world: &mut World, from: [f32; 3]) -> Vec<(f32, u64, [f32; 3])> {
    let objects: Vec<u64> = world
        .resource::<ActorPool>()
        .actors
        .values()
        .filter(|a| a.dying.is_none() && matches!(&*a.team, "axis" | "team3"))
        .map(|a| a.object)
        .collect();
    let mut out: Vec<(f32, u64, [f32; 3])> = objects
        .into_iter()
        .map(|object| {
            let at = eye(world, object);
            let at = [at[0], at[1], at[2] - CHEST_DROP];
            (dist(from, at), object, at)
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

/// A path node near `target` with a clear line to it, for the hunt.
fn vantage(world: &mut World, client: u32, object: u64, target: [f32; 3]) -> Option<[f32; 3]> {
    let start = nearest_node(&FrameWorld::from_world(world), target)?;
    let mut ring = vec![start];
    {
        let frame = FrameWorld::from_world(world);
        let graph = frame.path_graph();
        for _ in 0..2 {
            let next: Vec<u16> = ring
                .iter()
                .flat_map(|&n| graph.nodes[usize::from(n)].links.iter().map(|l| l.to))
                .collect();
            ring.extend(next);
        }
        ring.sort_unstable();
        ring.dedup();
    }
    let mut best: Option<(f32, [f32; 3])> = None;
    for node in ring {
        let at = FrameWorld::from_world(world).path_graph().nodes[usize::from(node)].origin;
        let d = dist(at, target);
        if !(HUNT_MIN..=HUNT_MAX).contains(&d) || best.is_some_and(|(b, _)| b <= d) {
            continue;
        }
        if clear(
            world,
            client,
            object,
            [at[0], at[1], at[2] + HUNT_EYE],
            target,
        ) {
            best = Some((d, at));
        }
    }
    best.map(|(_, at)| at)
}

pub(crate) fn auto_aim(world: &mut World, client: u32, cmd: &UserCmd) {
    if mode() == 0 || cmd.buttons & playerstate_iw4::buttons::ATTACK == 0 {
        return;
    }
    let Some(player) = world
        .resource::<Runtime>()
        .players
        .get(&client)
        .map(|slot| slot.object)
    else {
        return;
    };
    let from = eye(world, player);
    let now = super::players::now_ms(world);
    let skip = world
        .get_resource_or_insert_with(TestAim::default)
        .skip
        .clone();
    let targets: Vec<_> = hostiles(world, from)
        .into_iter()
        .filter(|(_, object, _)| skip.get(object).is_none_or(|until| *until <= now))
        .collect();
    let seen = targets
        .iter()
        .filter(|(d, _, _)| *d <= if mode() == 3 { HUNT_RANGE } else { RANGE })
        .find(|(_, object, at)| clear(world, client, *object, from, *at))
        .copied();
    let Some((d, object, at)) = seen else {
        world.resource_mut::<TestAim>().on = None;
        if mode() == 3 {
            hunt(world, client, &targets);
        }
        return;
    };
    let hits = actor_of(world, object)
        .and_then(|id| {
            world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .map(|a| a.hits_taken)
        })
        .unwrap_or(0);
    {
        let mut state = world.resource_mut::<TestAim>();
        match state.on {
            Some((on, since, before)) if on == object => {
                if hits == before && now - since > FUTILE_MS {
                    state.skip.insert(object, now + SKIP_MS);
                    state.on = None;
                } else if hits != before {
                    state.on = Some((object, now, hits));
                }
            }
            _ => state.on = Some((object, now, hits)),
        }
    }
    let want = math_iw4::vect_to_angles([at[0] - from[0], at[1] - from[1], at[2] - from[2]]);
    if mode() >= 2 {
        diag::info!(
            Sim,
            "autoaim: client {client} -> actor {:?} ent {object} at {d:.0}",
            actor_of(world, object)
        );
    }
    let mut frame = FrameWorld::from_world(world);
    if let Some(ps) = frame.player_mut(crate::ClientId(client)) {
        for (axis, want) in want.iter().take(2).enumerate() {
            let commanded = cmd.angles[axis] as f32 * movement_iw4::SHORT2ANGLE;
            ps.delta_angles[axis] = math_iw4::angle_subtract(*want, commanded);
        }
    }
}

fn hunt(world: &mut World, client: u32, targets: &[(f32, u64, [f32; 3])]) {
    if super::players::now_ms(world) % HUNT_EVERY_MS != 0 {
        return;
    }
    for &(_, object, at) in targets {
        let Some(stop) = vantage(world, client, object, at) else {
            continue;
        };
        diag::info!(
            Sim,
            "autoaim: hunt client {client} -> ({:.0} {:.0} {:.0}) for ent {object}",
            stop[0],
            stop[1],
            stop[2]
        );
        FrameWorld::from_world(world).set_origin(crate::ClientId(client), stop);
        return;
    }
}
