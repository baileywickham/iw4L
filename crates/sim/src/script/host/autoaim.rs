//! Test aimer (`IW4L_AUTOAIM=1`, `=2` also logs each new target): in a Spec Ops
//! load, a player's command is turned at the nearest hostile actor in clear
//! sight (head height, within `IW4L_AUTOAIM_RANGE`, default 2500) and fires on
//! every other tick, so semi-automatic weapons cycle. Authority only; the
//! shots go through the normal weapon and damage path. Scripted runs use it
//! where a mission needs kills (`download` waves) that fixed `look`s can't aim.

use super::actors::actor_of;
use super::natives::engine::{TraceIgnore, entity_trace};
use crate::actor::ActorPool;
use crate::bullet_collision::TraceOutcome;
use crate::frame::FrameWorld;
use crate::script::Runtime;
use bevy_ecs::prelude::World;
use movement_iw4::ANGLE2SHORT;
use playerstate_iw4::buttons;

const MASK_SHOT_SIGHT: u32 = 0x0801;

fn mode() -> u8 {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        std::env::var("IW4L_AUTOAIM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}

fn range() -> f32 {
    static RANGE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *RANGE.get_or_init(|| {
        std::env::var("IW4L_AUTOAIM_RANGE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2500.0)
    })
}

pub(crate) fn autoaim(world: &mut World, client: u32, cmd: &mut playerstate_iw4::UserCmd) {
    if mode() == 0
        || !world
            .get_resource::<crate::step::StepRequest>()
            .is_some_and(|r| r.reason.advances_authority_world())
        || world.get_resource::<Runtime>().is_none()
        || !super::players::single_player(world)
    {
        return;
    }
    let id = crate::ClientId(client);
    let (delta, frozen) = {
        let frame = FrameWorld::from_world(world);
        let Some(ps) = frame.player(id) else {
            return;
        };
        let frozen = frame
            .client_meta(id)
            .is_some_and(|m| m.lifecycle != crate::ClientLifecycle::Alive || m.controls.frozen);
        (ps.delta_angles, frozen)
    };
    if frozen {
        return;
    }
    let Some(player) = world
        .resource::<Runtime>()
        .players
        .get(&client)
        .map(|s| s.object)
    else {
        return;
    };
    let from = super::sentients::eye(world, player);
    let hostiles: Vec<u64> = world
        .resource::<ActorPool>()
        .actors
        .values()
        .filter(|a| a.dying.is_none() && matches!(&*a.team, "axis" | "team3"))
        .map(|a| a.object)
        .collect();
    let mut best: Option<(f32, u64, [f32; 3])> = None;
    let census = mode() >= 3 && world.resource::<crate::step::StepRequest>().tick.0 % 40 == 0;
    for object in hostiles {
        if actor_of(world, object).is_none() {
            continue;
        }
        let eye = super::sentients::eye(world, object);
        let to = [eye[0], eye[1], eye[2] - 4.0];
        let d = (0..3)
            .map(|i| (to[i] - from[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        if census && d < 600.0 {
            diag::info!(
                Sim,
                "autoaim: near entity {object} eye {eye:.0?} dist {d:.0}"
            );
        }
        if d > range() || best.is_some_and(|(b, _, _)| b <= d) {
            continue;
        }
        let ignore = TraceIgnore {
            client: Some(id),
            other_client: None,
            model: world
                .resource::<Runtime>()
                .entities
                .get(&object)
                .and_then(|e| e.presence),
        };
        if matches!(
            entity_trace(world, from, to, MASK_SHOT_SIGHT, ignore),
            TraceOutcome::Miss { .. }
        ) {
            best = Some((d, object, to));
        }
    }
    let Some((dist, object, to)) = best else {
        return;
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let yaw = d[1].atan2(d[0]).to_degrees();
    let pitch = -d[2].atan2(d[0].hypot(d[1])).to_degrees();
    cmd.angles[0] = ((pitch - delta[0]) * ANGLE2SHORT) as i32;
    cmd.angles[1] = ((yaw - delta[1]) * ANGLE2SHORT) as i32;
    let tick = world.resource::<crate::step::StepRequest>().tick.0;
    if tick % 2 == 0 {
        cmd.buttons |= buttons::ATTACK;
    } else {
        cmd.buttons &= !buttons::ATTACK;
    }
    static LAST: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
    let changed = LAST
        .lock()
        .map(|mut last| last.replace(object) != Some(object))
        .unwrap_or(false);
    if mode() >= 2 && changed {
        diag::info!(
            Sim,
            "autoaim: client {client} at entity {object} dist {dist:.0} yaw {yaw:.1} pitch {pitch:.1}"
        );
    }
}
