//! AI events (`Actor_Broadcast*Event`): gunshots, bullets passing, explosions,
//! grenades, footsteps, pain and death reach actors within the `ai_eventDist*`
//! dvars. An actor that cares learns where the originator is (and may take it as
//! its enemy on its next look); listeners (`addaieventlistener`) also get
//! `"ai_event"` (name, originator, position).

use super::actor_combat::{hostile, label, learn, shooter_object};
use super::actors::actor_of;
use super::args::string;
use super::natives::engine::entity_id;
use crate::actor::{ActorId, ActorPool, AiEvent, EventSource, PendingEvent};
use crate::frame::FrameWorld;
use crate::script::runtime::raise;
use crate::script::{Arc, Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

/// An actor that heard about a sentient this recently does not care to hear again
/// (`Actor_CaresAboutInfo`).
const CARE_MS: i64 = 2000;
/// Line events reach actors whose middle is this close in height to the line.
const LINE_HALF_HEIGHT: f32 = 45.0;
const ACTOR_MID_HEIGHT: f32 = 36.0;
/// One footstep event per moving player this often.
const FOOTSTEP_MS: i64 = 400;
const FOOTSTEP_MIN_SPEED: f32 = 20.0;
const WALK_SPEED: f32 = 110.0;

impl AiEvent {
    const ALL: [AiEvent; 14] = [
        Self::Footstep,
        Self::FootstepWalk,
        Self::FootstepSprint,
        Self::NewEnemy,
        Self::Pain,
        Self::Death,
        Self::Explosion,
        Self::GrenadePing,
        Self::ProjectilePing,
        Self::Gunshot,
        Self::GunshotTeammate,
        Self::SilencedShot,
        Self::Bullet,
        Self::ProjectileImpact,
    ];

    /// The name `addaieventlistener` takes and `"ai_event"` passes.
    fn name(self) -> &'static str {
        match self {
            Self::Footstep => "footstep",
            Self::FootstepWalk => "footstep_walk",
            Self::FootstepSprint => "footstep_sprint",
            Self::NewEnemy => "new_enemy",
            Self::Pain => "pain",
            Self::Death => "death",
            Self::Explosion => "explode",
            Self::GrenadePing => "grenade danger",
            Self::ProjectilePing => "projectile_ping",
            Self::Gunshot => "gunshot",
            Self::GunshotTeammate => "gunshot_teammate",
            Self::SilencedShot => "silenced_shot",
            Self::Bullet => "bulletwhizby",
            Self::ProjectileImpact => "projectile_impact",
        }
    }

    /// The dvar holding the event's reach, and the engine default.
    fn reach(self) -> (&'static str, f32) {
        match self {
            Self::Footstep => ("ai_eventDistFootstep", 512.0),
            Self::FootstepWalk => ("ai_eventDistFootstepWalk", 256.0),
            Self::FootstepSprint => ("ai_eventDistFootstepSprint", 400.0),
            Self::NewEnemy => ("ai_eventDistNewEnemy", 1024.0),
            Self::Pain => ("ai_eventDistPain", 512.0),
            Self::Death => ("ai_eventDistDeath", 1024.0),
            Self::Explosion => ("ai_eventDistExplosion", 1500.0),
            Self::GrenadePing => ("ai_eventDistGrenadePing", 512.0),
            Self::ProjectilePing => ("ai_eventDistProjPing", 128.0),
            Self::Gunshot => ("ai_eventDistGunShot", 2048.0),
            Self::GunshotTeammate => ("ai_eventDistGunShotTeam", 2048.0),
            Self::SilencedShot => ("ai_eventDistSilencedShot", 128.0),
            Self::Bullet => ("ai_eventDistBullet", 96.0),
            Self::ProjectileImpact => ("ai_eventDistProjImpact", 256.0),
        }
    }

    /// The actor field that overrides the reach for the hearer (footsteps only).
    fn hearer_field(self) -> Option<&'static str> {
        match self {
            Self::Footstep => Some("footstepdetectdist"),
            Self::FootstepWalk => Some("footstepdetectdistwalk"),
            Self::FootstepSprint => Some("footstepdetectdistsprint"),
            _ => None,
        }
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|event| event.name().eq_ignore_ascii_case(name))
    }

    fn is_line(self) -> bool {
        matches!(self, Self::Bullet | Self::ProjectileImpact)
    }
}

/// Which teams hear an event relative to its originator's team.
#[derive(Clone, Copy, PartialEq)]
enum Audience {
    Enemies,
    Team,
    Everyone,
}

fn origin(world: &mut World, object: u64) -> [f32; 3] {
    match super::players::entity_field(world, object, "origin") {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn team_of(world: &mut World, object: u64) -> Option<Arc<str>> {
    if let Some(id) = actor_of(world, object) {
        return world
            .resource::<ActorPool>()
            .actors
            .get(&id)
            .map(|a| a.team.clone());
    }
    if let Some(client) = world.resource::<Runtime>().player_client(object) {
        return Some(match super::players::load_field(world, client, "team") {
            Some(Value::String(team)) => team.to_string().into(),
            _ => "allies".into(),
        });
    }
    world
        .resource::<ActorPool>()
        .sentients
        .get(&object)
        .cloned()
}

/// `footstepdetectdist*` before a script writes them: the matching event dvar.
pub(crate) fn footstep_detect_default(world: &World, field: &str) -> Option<f32> {
    let kind = [
        AiEvent::Footstep,
        AiEvent::FootstepWalk,
        AiEvent::FootstepSprint,
    ]
    .into_iter()
    .find(|kind| kind.hearer_field() == Some(field))?;
    let (dvar, default) = kind.reach();
    Some(super::actor_combat::dvar_float(world, dvar).unwrap_or(default))
}

/// Queues an event for the actors' next look; nothing listens without actors.
pub(crate) fn push(world: &mut World, kind: AiEvent, source: Option<EventSource>, at: [f32; 3]) {
    let mut pool = world.resource_mut::<ActorPool>();
    if pool.actors.is_empty() {
        return;
    }
    pool.events.push(PendingEvent {
        kind,
        source,
        attacker: None,
        at,
        end: None,
        weapon: 0,
    });
}

/// Pain or death of a sentient: its team hears about whoever hurt it.
pub(crate) fn push_casualty(
    world: &mut World,
    kind: AiEvent,
    casualty: u64,
    attacker: Option<u64>,
    at: [f32; 3],
) {
    let mut pool = world.resource_mut::<ActorPool>();
    if pool.actors.is_empty() {
        return;
    }
    pool.events.push(PendingEvent {
        kind,
        source: Some(EventSource::Object(casualty)),
        attacker,
        at,
        end: None,
        weapon: 0,
    });
}

/// Moving players make footstep events: sprinting, walking (crouched, aiming
/// down sights or slow) or running. Prone and airborne players make none.
fn footsteps(world: &mut World, now: i64) {
    let players: Vec<(u32, u64)> = world
        .resource::<Runtime>()
        .players
        .iter()
        .map(|(client, slot)| (*client, slot.object))
        .collect();
    for (client, object) in players {
        let kind = {
            let frame = FrameWorld::from_world(world);
            let alive = frame
                .client_meta(crate::ClientId(client))
                .is_some_and(|m| m.lifecycle == crate::ClientLifecycle::Alive);
            let Some(ps) = frame.player(crate::ClientId(client)) else {
                continue;
            };
            let speed = ps.velocity[0].hypot(ps.velocity[1]);
            let grounded = ps.ground_entity_num != playerstate_iw4::ENTITYNUM_NONE;
            let flags = ps.e_flags;
            if !alive
                || !grounded
                || speed < FOOTSTEP_MIN_SPEED
                || flags & playerstate_iw4::eflags::PRONE != 0
            {
                continue;
            }
            if ps.pm_flags & playerstate_iw4::pm_flags::SPRINTING != 0 {
                AiEvent::FootstepSprint
            } else if flags & playerstate_iw4::eflags::DUCK != 0
                || ps.f_weapon_pos_frac > 0.5
                || speed < WALK_SPEED
            {
                AiEvent::FootstepWalk
            } else {
                AiEvent::Footstep
            }
        };
        let due = world
            .resource::<ActorPool>()
            .footsteps
            .get(&client)
            .is_none_or(|last| now - last >= FOOTSTEP_MS);
        if !due {
            continue;
        }
        world
            .resource_mut::<ActorPool>()
            .footsteps
            .insert(client, now);
        let at = origin(world, object);
        push(world, kind, Some(EventSource::Object(object)), at);
    }
}

fn silenced(world: &mut World, weapon: u32) -> bool {
    crate::script_player::weapon_name(&FrameWorld::from_world(world), weapon).contains("silence")
}

/// The closest point of the line to `at`, if `at` projects onto it and lies
/// within `radius` in the plane and the half height of it.
fn line_point(start: [f32; 3], end: [f32; 3], at: [f32; 3], radius: f32) -> Option<[f32; 3]> {
    let d = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let len_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let t = (at[0] - start[0]) * d[0] + (at[1] - start[1]) * d[1] + (at[2] - start[2]) * d[2];
    if t < 0.0 || len_sq <= 0.0 {
        return None;
    }
    let point = if t < len_sq {
        let f = t / len_sq;
        [
            start[0] + d[0] * f,
            start[1] + d[1] * f,
            start[2] + d[2] * f,
        ]
    } else {
        end
    };
    let planar = (at[0] - point[0]).hypot(at[1] - point[1]);
    let height = (at[2] + ACTOR_MID_HEIGHT - point[2]).abs();
    (planar <= radius && height < LINE_HALF_HEIGHT).then_some(point)
}

fn cares(world: &World, id: ActorId, about: u64, now: i64) -> bool {
    world
        .resource::<ActorPool>()
        .actors
        .get(&id)
        .and_then(|a| a.known.get(&about))
        .is_none_or(|k| now - k.time_ms >= CARE_MS)
}

fn deliver(world: &mut World, actors: &[(i32, ActorId, u64)], event: PendingEvent, now: i64) {
    let originator = match event.source {
        Some(EventSource::Object(object)) => Some(object),
        Some(EventSource::Attacker(attacker)) => shooter_object(world, attacker),
        None => None,
    }
    .filter(|o| world.resource::<Runtime>().live(o));
    let team = originator.and_then(|o| team_of(world, o));
    let mut kinds = vec![event.kind];
    if event.kind == AiEvent::Gunshot {
        if silenced(world, event.weapon) {
            kinds[0] = AiEvent::SilencedShot;
        }
        kinds.push(AiEvent::GunshotTeammate);
    }
    let by_player =
        originator.is_some_and(|o| world.resource::<Runtime>().player_client(o).is_some());
    for kind in kinds {
        let audience = match kind {
            AiEvent::GunshotTeammate | AiEvent::Pain | AiEvent::Death | AiEvent::NewEnemy => {
                Audience::Team
            }
            AiEvent::Explosion | AiEvent::GrenadePing | AiEvent::ProjectilePing => {
                Audience::Everyone
            }
            _ => Audience::Enemies,
        };
        if audience != Audience::Everyone && team.is_none() {
            continue;
        }
        let (dvar, default) = kind.reach();
        let radius = super::actor_combat::dvar_float(world, dvar).unwrap_or(default);
        for &(_, id, object) in actors {
            if Some(object) == originator {
                continue;
            }
            let Some((actor_team, listening)) = world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .filter(|a| a.dying.is_none())
                .map(|a| (a.team.clone(), a.listeners & kind.bit() != 0))
            else {
                continue;
            };
            let enemy = team.as_deref().is_some_and(|t| hostile(&actor_team, t));
            let heard = match audience {
                Audience::Everyone => true,
                Audience::Team => team.as_deref() == Some(&*actor_team),
                // A player's bullets also pass its own side (`Bullet_Process`).
                Audience::Enemies => {
                    enemy
                        || (kind == AiEvent::Bullet
                            && by_player
                            && team.as_deref() == Some(&*actor_team))
                }
            };
            if !heard {
                continue;
            }
            let at = origin(world, object);
            let radius = kind
                .hearer_field()
                .and_then(|field| {
                    let pool = world.resource::<ActorPool>();
                    let a = pool.actors.get(&id)?;
                    a.fields.contains_key(field).then(|| a.float_field(field))
                })
                .unwrap_or(radius);
            let position = match event.end.filter(|_| kind.is_line()) {
                Some(end) => match line_point(event.at, end, at, radius) {
                    Some(point) => point,
                    None => continue,
                },
                None if dist(event.at, at) <= radius => event.at,
                None => continue,
            };
            let learned = react(
                world,
                id,
                object,
                kind,
                originator,
                enemy,
                event.attacker,
                now,
            );
            if kind == AiEvent::Explosion {
                raise(
                    world,
                    Value::Object(object),
                    "explode",
                    vec![
                        Value::Vector(event.at),
                        originator.map_or(Value::Undefined, Value::Object),
                    ],
                );
            }
            if listening {
                raise(
                    world,
                    Value::Object(object),
                    "ai_event",
                    vec![
                        Value::string(kind.name()),
                        originator.map_or(Value::Undefined, Value::Object),
                        Value::Vector(position),
                    ],
                );
            }
            if learned || listening {
                diag::info!(
                    Sim,
                    "actor: {} heard {} from {} dist={:.0}{}{}",
                    label(world, object),
                    kind.name(),
                    originator.map_or_else(|| "world".into(), |o| label(world, o)),
                    dist(event.at, at),
                    if learned { " learned" } else { "" },
                    if listening { " ai_event" } else { "" },
                );
            }
        }
    }
}

/// `Actor_ReceivePointEvent` / `Actor_ReceiveLineEvent`: what the actor learns.
#[allow(clippy::too_many_arguments)]
fn react(
    world: &mut World,
    id: ActorId,
    object: u64,
    kind: AiEvent,
    originator: Option<u64>,
    enemy: bool,
    attacker: Option<u64>,
    now: i64,
) -> bool {
    let about = match kind {
        AiEvent::Footstep
        | AiEvent::FootstepWalk
        | AiEvent::FootstepSprint
        | AiEvent::Gunshot
        | AiEvent::SilencedShot
        | AiEvent::Bullet
        | AiEvent::ProjectileImpact => originator.filter(|_| enemy),
        AiEvent::Pain | AiEvent::Death => attacker.filter(|a| {
            let actor_team = world
                .resource::<ActorPool>()
                .actors
                .get(&id)
                .map(|a| a.team.clone());
            let other = team_of(world, *a);
            matches!((actor_team, other), (Some(mine), Some(theirs)) if hostile(&mine, &theirs))
        }),
        _ => None,
    };
    let Some(about) = about.filter(|o| *o != object && world.resource::<Runtime>().live(o)) else {
        return false;
    };
    let line = matches!(kind, AiEvent::Bullet | AiEvent::ProjectileImpact);
    if !line && !cares(world, id, about, now) {
        return false;
    }
    if line && let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        a.last_attacker = Some((about, now));
    }
    learn(world, id, about, false, now);
    true
}

/// Delivers the queued events and footsteps to the live actors, in queue order.
pub(crate) fn run(world: &mut World, actors: &[(i32, ActorId, u64)], now: i64) {
    footsteps(world, now);
    let events = std::mem::take(&mut world.resource_mut::<ActorPool>().events);
    for event in events {
        deliver(world, actors, event, now);
    }
}

fn set_listener(
    world: &mut World,
    receiver: &Value,
    args: &[Value],
    add: bool,
) -> Result<Value, String> {
    let object = entity_id(world, receiver)?;
    let event = string(args, 0)?;
    let event =
        AiEvent::named(&event).ok_or_else(|| format!("Unable to find AI event for [{event}]"))?;
    let Some(id) = actor_of(world, object) else {
        return Ok(Value::Undefined);
    };
    if let Some(a) = world.resource_mut::<ActorPool>().actors.get_mut(&id) {
        if add {
            a.listeners |= event.bit();
        } else {
            a.listeners &= !event.bit();
        }
    }
    Ok(Value::Undefined)
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::Method;

    registry.register(Method, "addaieventlistener", |world, receiver, args| {
        set_listener(world, receiver, args, true)
    });
    registry.register(Method, "removeaieventlistener", |world, receiver, args| {
        set_listener(world, receiver, args, false)
    });
}
