//! Dev-only helpers for scripted Spec Ops runs (listen host, cheats on):
//! `enemies` prints the live hostile sentients; `autoaim` points the local
//! player's view at the nearest one in sight, and can fire and move to a path
//! node that sees one. The mission scripts still run for real; this only
//! stands in for the player's hands.

use bevy::prelude::*;
use net::{ClientActionInbox, LocalPresentClient, LookState, PresentedSnapshot};
use sim::{ClientAction, ClientLifecycle};

use crate::{
    ConsoleCommand, ConsoleInputState, ConsoleLine, ConsoleRegistry, ConsoleSettings, ConsoleState,
};

#[derive(Resource, Default)]
pub(crate) struct DevAim {
    on: bool,
    fire: bool,
    head: bool,
    vehicles: bool,
    /// Move to a node that sees a hostile after this long with none in sight.
    hunt_after: Option<f32>,
    max_dist: f32,
    unseen: f32,
    fire_cooldown: f32,
    hunt_cooldown: f32,
    log_cooldown: f32,
    locked: Option<i32>,
    /// The locked target, its health and how long it has kept that health.
    stall: Option<(i32, i32, f32)>,
    /// Targets shot without effect (out of reach of the aim point): skipped for a while.
    skip: Vec<(i32, f32)>,
    /// Spots where a shot achieved nothing: hunting does not return to them.
    avoid: Vec<[f32; 3]>,
}

const FIRE_PULSE: f32 = 0.25;
const FIRE_GAP: f32 = 0.1;
const HUNT_COOLDOWN: f32 = 3.0;
const LOG_EVERY: f32 = 5.0;
const STALL_SECONDS: f32 = 6.0;
const SKIP_SECONDS: f32 = 8.0;
const AVOID_MAX: usize = 16;
const GRAVITY: f32 = 800.0;

/// Pitch (IW4 sign: negative is up) that lands a projectile launched at `speed`
/// on a point `x` away and `y` above; the flatter of the two arcs.
fn lob_pitch(x: f32, y: f32, speed: f32) -> Option<f32> {
    let v2 = speed * speed;
    let disc = v2 * v2 - GRAVITY * (GRAVITY * x * x + 2.0 * y * v2);
    if disc < 0.0 || x <= 1.0 {
        return None;
    }
    let tan = (v2 - disc.sqrt()) / (GRAVITY * x);
    Some(-tan.atan().to_degrees())
}

pub(crate) fn register(registry: &mut ConsoleRegistry) {
    if registry.resolve("enemies").is_none() {
        registry.register(
            crate::CommandSpec::new("enemies")
                .usage("enemies — print live hostile sentients, nearest first (dev)"),
        );
    }
    if registry.resolve("autoaim").is_none() {
        registry.register(crate::CommandSpec::new("autoaim").usage(
            "autoaim off | on [fire] [head] [vehicles] [hunt <s>] [range <u>] — aim at the nearest visible hostile (dev, cheats)",
        ));
    }
}

fn alive(presented: &PresentedSnapshot, id: sim::ClientId) -> bool {
    presented
        .snapshot()
        .and_then(|s| s.meta.for_client(id))
        .is_some_and(|m| m.lifecycle == ClientLifecycle::Alive)
}

pub(crate) fn route(
    mut events: MessageReader<ConsoleCommand>,
    mut console: ResMut<ConsoleState>,
    settings: Res<ConsoleSettings>,
    mut line: ResMut<ConsoleLine>,
    local: Res<LocalPresentClient>,
    mut authority: Option<ResMut<net::AuthorityWorld>>,
    mut aim: ResMut<DevAim>,
    mut inputs: ResMut<ConsoleInputState>,
) {
    let capacity = settings.log_capacity;
    let mut echo = |msg: String| {
        diag::info!(Console, "{msg}");
        line.0 = msg.clone();
        console.echo(msg, capacity);
    };
    for cmd in events.read() {
        match cmd.name.as_str() {
            "enemies" => {
                let Some(world) = authority.as_deref_mut() else {
                    echo("enemies: no authority world (not a listen host)".into());
                    continue;
                };
                let targets = world.0.dev_aim_targets(local.0, false, true);
                echo(format!("enemies: {} hostile", targets.len()));
                for t in targets {
                    echo(format!(
                        "enemies: ent {} {} {} at ({:.0} {:.0} {:.0}) dist {:.0} health {}{}",
                        t.entnum,
                        t.team,
                        t.classname,
                        t.origin[0],
                        t.origin[1],
                        t.origin[2],
                        t.dist,
                        t.health,
                        if t.visible { " visible" } else { "" }
                    ));
                }
            }
            "autoaim" => {
                if authority.as_ref().is_some_and(|a| !a.0.cheats_enabled()) {
                    echo("autoaim: cheats are off".into());
                    continue;
                }
                let mut args = cmd.args.iter().map(String::as_str);
                match args.next() {
                    Some("off") | Some("0") => {
                        if aim.fire {
                            inputs.release("+attack");
                        }
                        *aim = DevAim::default();
                        echo("autoaim: off".into());
                    }
                    Some("on") | Some("1") => {
                        let mut next = DevAim {
                            on: true,
                            max_dist: 6000.0,
                            ..DevAim::default()
                        };
                        let mut bad = None;
                        while let Some(word) = args.next() {
                            match word {
                                "fire" => next.fire = true,
                                "head" => next.head = true,
                                "vehicles" => next.vehicles = true,
                                "hunt" => match args
                                    .next()
                                    .and_then(|s| s.trim_end_matches('s').parse::<f32>().ok())
                                {
                                    Some(s) => next.hunt_after = Some(s.max(0.5)),
                                    None => bad = Some("hunt <seconds>"),
                                },
                                "range" => match args.next().and_then(|s| s.parse::<f32>().ok()) {
                                    Some(r) => next.max_dist = r,
                                    None => bad = Some("range <units>"),
                                },
                                _ => bad = Some("unknown word"),
                            }
                        }
                        if let Some(bad) = bad {
                            echo(format!("autoaim: usage ({bad})"));
                            continue;
                        }
                        echo(format!(
                            "autoaim: on fire={} head={} vehicles={} hunt={:?} range={:.0}",
                            next.fire, next.head, next.vehicles, next.hunt_after, next.max_dist
                        ));
                        *aim = next;
                    }
                    _ => echo(
                        "usage: autoaim off | on [fire] [head] [vehicles] [hunt <s>] [range <u>]"
                            .into(),
                    ),
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive(
    time: Res<Time>,
    local: Res<LocalPresentClient>,
    presented: Res<PresentedSnapshot>,
    mut authority: Option<ResMut<net::AuthorityWorld>>,
    mut inbox: Option<ResMut<ClientActionInbox>>,
    mut seq: ResMut<net::ActionRequestIds>,
    mut look: ResMut<LookState>,
    mut aim: ResMut<DevAim>,
    mut inputs: ResMut<ConsoleInputState>,
) {
    if !aim.on {
        return;
    }
    let dt = time.delta_secs();
    aim.fire_cooldown -= dt;
    aim.hunt_cooldown -= dt;
    aim.log_cooldown -= dt;
    aim.skip.retain_mut(|(_, left)| {
        *left -= dt;
        *left > 0.0
    });
    let (Some(world), Some(ps)) = (authority.as_deref_mut(), presented.alive_player(local.0))
    else {
        return;
    };
    if !alive(&presented, local.0) {
        return;
    }
    let ps = *ps;
    let eye = [
        ps.origin[0],
        ps.origin[1],
        ps.origin[2] + ps.view_height_current,
    ];
    let targets = world.0.dev_aim_targets(local.0, aim.head, aim.vehicles);
    let target = targets
        .iter()
        .find(|t| {
            t.visible && t.dist <= aim.max_dist && !aim.skip.iter().any(|(e, _)| *e == t.entnum)
        })
        .cloned();
    let angles_to = |from: [f32; 3], to: [f32; 3]| {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let yaw = d[1].atan2(d[0]).to_degrees();
        let pitch = -d[2].atan2((d[0] * d[0] + d[1] * d[1]).sqrt()).to_degrees();
        [pitch, yaw, 0.0]
    };
    if aim.log_cooldown <= 0.0 {
        aim.log_cooldown = LOG_EVERY;
        let near = targets.first();
        diag::info!(
            Console,
            "autoaim: weapon={} state={} hostiles={} visible={} target={} nearest={}",
            ps.weapon,
            ps.weaponstate_primary,
            targets.len(),
            targets.iter().filter(|t| t.visible).count(),
            target.as_ref().map_or("none".into(), |t| format!(
                "ent {} dist {:.0}",
                t.entnum, t.dist
            )),
            near.map_or("none".into(), |t| format!(
                "ent {} ({:.0} {:.0} {:.0}) dist {:.0}{}",
                t.entnum,
                t.origin[0],
                t.origin[1],
                t.origin[2],
                t.dist,
                t.blocked_at.map_or(String::new(), |b| format!(
                    " sight blocked at ({:.0} {:.0} {:.0})",
                    b[0], b[1], b[2]
                ))
            )),
        );
    }
    match target {
        Some(t) => {
            aim.unseen = 0.0;
            if aim.locked != Some(t.entnum) {
                aim.locked = Some(t.entnum);
                diag::info!(
                    Console,
                    "autoaim: target ent {} {} dist {:.0} health {}",
                    t.entnum,
                    t.classname,
                    t.dist,
                    t.health
                );
            }
            let mut want = angles_to(eye, t.aim);
            if let Some((speed, _)) = world.0.dev_projectile_lob(local.0) {
                let d = [t.aim[0] - eye[0], t.aim[1] - eye[1], t.aim[2] - eye[2]];
                if let Some(pitch) = lob_pitch((d[0] * d[0] + d[1] * d[1]).sqrt(), d[2], speed) {
                    want[0] = pitch;
                }
            }
            let view = [
                want[0] - ps.delta_angles[0],
                want[1] - ps.delta_angles[1],
                -ps.delta_angles[2],
            ];
            look.angles = net::look_angles_from_degrees(view);
            aim.stall = match aim.stall {
                Some((e, h, since)) if e == t.entnum && h == t.health => Some((e, h, since + dt)),
                _ => Some((t.entnum, t.health, 0.0)),
            };
            if aim.fire && aim.stall.is_some_and(|(_, _, since)| since > STALL_SECONDS) {
                diag::info!(
                    Console,
                    "autoaim: ent {} takes no damage, skipping it",
                    t.entnum
                );
                aim.skip.push((t.entnum, SKIP_SECONDS));
                aim.stall = None;
                if aim.avoid.len() >= AVOID_MAX {
                    aim.avoid.remove(0);
                }
                aim.avoid.push(ps.origin);
            }
            if aim.fire && aim.fire_cooldown <= 0.0 {
                inputs.press("+attack", FIRE_PULSE);
                aim.fire_cooldown = FIRE_PULSE + FIRE_GAP;
            }
        }
        None => {
            aim.locked = None;
            aim.unseen += dt;
            let Some(after) = aim.hunt_after else {
                return;
            };
            if aim.unseen < after || aim.hunt_cooldown > 0.0 || targets.is_empty() {
                return;
            }
            aim.hunt_cooldown = HUNT_COOLDOWN;
            let Some(spot) = world.0.dev_hunt_spot(local.0, 150.0, 900.0, &aim.avoid) else {
                diag::info!(Console, "autoaim: hunt found no node");
                return;
            };
            // Already standing there and still nothing in sight: try elsewhere next time.
            let here = [
                spot[0] - ps.origin[0],
                spot[1] - ps.origin[1],
                spot[2] - ps.origin[2],
            ];
            if here[0] * here[0] + here[1] * here[1] + here[2] * here[2] < 48.0 * 48.0 {
                if aim.avoid.len() >= AVOID_MAX {
                    aim.avoid.remove(0);
                }
                aim.avoid.push(spot);
            }
            let Some(inbox) = inbox.as_deref_mut() else {
                return;
            };
            let face = angles_to([spot[0], spot[1], spot[2] + 60.0], targets[0].aim);
            let request_id = seq.allocate();
            if inbox
                .push(
                    local.0,
                    ClientAction::Move {
                        request_id,
                        origin: spot,
                        angles: face,
                    },
                )
                .is_ok()
            {
                look.angles = net::look_angles_from_degrees(face);
                diag::info!(
                    Console,
                    "autoaim: hunt to ({:.0} {:.0} {:.0}) for ent {}",
                    spot[0],
                    spot[1],
                    spot[2],
                    targets[0].entnum
                );
            }
        }
    }
}
