//! Player-driven SP vehicles (the Spec Ops snowmobile): `mountvehicle`,
//! `dismountvehicle`, `vehphys_setspeed` and an arcade ground model over the
//! clip map. The driver is linked to the vehicle; its move input steers it.

use super::args::{arg, float};
use super::players::{LinkView, PlayerLink};
use crate::frame::FrameWorld;
use crate::script::Namespace::Method;
use crate::script::runtime::raise;
use crate::script::{NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;
use glam::Vec3;

const MPH: f32 = 17.6;
const TICK_S: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
const SUBSTEPS: usize = 4;
const GRAVITY: f32 = 800.0;
/// `veh_topspeed` when the script sets none (mph).
const TOP_MPH: f32 = 100.0;
const THROTTLE_MPH_S: f32 = 30.0;
const BRAKE_MPH_S: f32 = 60.0;
const COAST_MPH_S: f32 = 4.0;
const REVERSE_MPH: f32 = 15.0;
/// Share of the slope's pull that survives snow friction.
const SLOPE_GRIP: f32 = 0.8;
/// Gravity can carry the vehicle this far past `veh_topspeed`.
const OVERSPEED: f32 = 1.5;
/// Share of the overspeed lost per second.
const OVERSPEED_DRAG: f32 = 0.15;
/// Vehicles fall slower than players: jumps carry (physics-vehicle feel).
const AIR_GRAVITY: f32 = 600.0;
/// Steering rate (degrees/s) at full lock.
const STEER_DEG_S: f32 = 100.0;
const HALF_WIDTH: f32 = 22.0;
const HEIGHT: f32 = 36.0;
/// Bumps lower than this are driven over.
const STEP: f32 = 24.0;
/// The ground keeps a moving vehicle when it drops less than this per substep.
const SNAP: f32 = 20.0;
const WALKABLE: f32 = 0.45;
/// Ground this far below the free-fall path still holds a grounded vehicle.
const HOLD: f32 = 2.0;
/// Impact speed into a wall (u/s) that wrecks the vehicle (`veh_collision`).
const CRASH_SPEED: f32 = 900.0;
const SOLID: u32 = 0x0080_0211;
/// The autopilot's speed between gates (no race spline).
const GATE_MPH: f32 = 70.0;
/// Where the driver's feet sit relative to the vehicle origin.
const SEAT: [f32; 3] = [-16.0, 0.0, -8.0];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DriveInput {
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct Drive {
    pub driver: u32,
    pub input: DriveInput,
    vertical: f32,
    airborne: bool,
    yaw: f32,
    normal: [f32; 3],
}

fn autodrive() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("IW4L_VEH_AUTODRIVE").is_ok_and(|v| v != "0"))
}

fn driven(world: &World, client: u32) -> Option<u64> {
    world
        .resource::<Runtime>()
        .vehicles
        .iter()
        .find(|(_, v)| v.drive.as_ref().is_some_and(|d| d.driver == client))
        .map(|(id, _)| *id)
}

/// Takes the driver's move input before the link zeroes it: forward is the
/// throttle, back the brake (reverse once stopped), strafe steers.
pub(crate) fn drive_input(world: &mut World, client: u32, cmd: &playerstate_iw4::UserCmd) {
    let Some(id) = driven(world, client) else {
        return;
    };
    let frozen = FrameWorld::from_world(world)
        .client_meta(crate::ClientId(client))
        .is_some_and(|m| m.controls.frozen);
    let input = if frozen {
        DriveInput::default()
    } else {
        let forward = f32::from(cmd.forwardmove) / 127.0;
        DriveInput {
            throttle: forward.max(0.0),
            brake: (-forward).max(0.0),
            steer: (f32::from(cmd.rightmove) / 127.0).clamp(-1.0, 1.0),
        }
    };
    if let Some(drive) = world
        .resource_mut::<Runtime>()
        .vehicles
        .get_mut(&id)
        .and_then(|v| v.drive.as_mut())
    {
        drive.input = input;
    }
}

fn vector(value: Value) -> Option<[f32; 3]> {
    match value {
        Value::Vector(v) => Some(v),
        _ => None,
    }
}

fn number(value: Value) -> Option<f32> {
    match value {
        Value::Int(v) => Some(v as f32),
        Value::Float(v) => Some(v),
        _ => None,
    }
}

/// The race spline point ahead of the player (`_vehicle_spline`).
fn spline_aim(runtime: &mut Runtime, client: u32, origin: [f32; 3]) -> Option<[f32; 3]> {
    let player = runtime.players.get(&client)?.object;
    let Value::Object(targ) = runtime.object_field(player, "targ") else {
        return None;
    };
    let mut node = targ;
    let mut aim = None;
    for _ in 0..4 {
        let Value::Object(next) = runtime.object_field(node, "next_node") else {
            break;
        };
        let Some(point) = vector(runtime.object_field(next, "midpoint")) else {
            break;
        };
        aim = Some(point);
        let dx = point[0] - origin[0];
        let dy = point[1] - origin[1];
        if dx * dx + dy * dy > 700.0 * 700.0 || next == node {
            break;
        }
        node = next;
    }
    aim
}

/// Without a spline (`so_snowrace2`): the nearest gate (`flag_trigger`) in
/// front, else the finish line.
fn gate_aim(runtime: &mut Runtime, origin: [f32; 3], yaw: f32) -> Option<([f32; 3], bool)> {
    let forward = math_iw4::angle_vectors([0.0, yaw, 0.0]).0;
    let ids: Vec<u64> = runtime.entities.keys().copied().collect();
    let mut gates = Vec::new();
    let mut finish = None;
    for id in ids {
        let name = runtime.object_field(id, "targetname");
        let Value::String(name) = name else { continue };
        let Some(at) = vector(runtime.object_field(id, "origin")) else {
            continue;
        };
        match &*name {
            "flag_trigger" => gates.push(at),
            "finishline" => finish = Some(at),
            _ => {}
        }
    }
    gates
        .into_iter()
        .filter_map(|at| {
            let d = [at[0] - origin[0], at[1] - origin[1]];
            let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
            let ahead = (d[0] * forward[0] + d[1] * forward[1]) / dist.max(1.0);
            (ahead > 0.2).then_some((dist, at))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, at)| (at, true))
        .or(finish.map(|at| (at, false)))
}

/// Test driver (`IW4L_VEH_AUTODRIVE=1`, `=2` also logs each decision): full
/// throttle, steering at the race spline point
/// `_vehicle_spline::track_player_progress` keeps on the player
/// (`player.targ.next_node.midpoint`) or at the next gate, swerving round what
/// a probe ahead hits.
fn autopilot(
    world: &mut World,
    client: u32,
    origin: [f32; 3],
    yaw: f32,
    speed: f32,
) -> Option<DriveInput> {
    // Gates sit off the racing line: slow down to turn for them.
    let (aim, cap) = match spline_aim(&mut world.resource_mut::<Runtime>(), client, origin) {
        Some(aim) => (aim, f32::MAX),
        None => match gate_aim(&mut world.resource_mut::<Runtime>(), origin, yaw)? {
            (aim, true) => (aim, GATE_MPH * MPH),
            (aim, false) => (aim, f32::MAX),
        },
    };
    let want = math_iw4::vect_to_angles([aim[0] - origin[0], aim[1] - origin[1], 0.0])[1];
    // Probe along the slope the vehicle stands on, not level into the hill.
    let pitch = {
        let mut runtime = world.resource_mut::<Runtime>();
        let id = runtime
            .vehicles
            .iter()
            .find(|(_, v)| v.drive.as_ref().is_some_and(|d| d.driver == client))
            .map(|(id, _)| *id)?;
        vector(runtime.object_field(id, "angles")).map_or(0.0, |a| a[0])
    };
    // A steep straight descent (the run-in to the final jump) needs its speed.
    let cap = if math_iw4::angle_subtract(pitch, 0.0) > 25.0 {
        f32::MAX
    } else {
        cap
    };
    let from = Vec3::from_array(origin) + Vec3::Z * STEP;
    let look = (speed.abs() * 1.1).clamp(400.0, 2000.0);
    // Prefer a clear line near the spline and near where the vehicle points
    // (no flip-flopping between sides of an obstacle).
    let mut best = (want, -1.0f32, f32::MIN);
    for offset in [
        0.0f32, 10.0, -10.0, 20.0, -20.0, 32.0, -32.0, 45.0, -45.0, 60.0, -60.0,
    ] {
        let heading = want + offset;
        let dir = Vec3::from_array(math_iw4::angle_vectors([pitch - 2.0, heading, 0.0]).0);
        let clear = clearance(world, from, dir, look);
        let turn = math_iw4::angle_subtract(heading, yaw).abs();
        let score = clear * 200.0 - offset.abs() * 0.6 - turn * 0.6;
        if score > best.2 {
            best = (heading, clear, score);
        }
    }
    let delta = math_iw4::angle_subtract(best.0, yaw);
    let ahead = {
        let dir = Vec3::from_array(math_iw4::angle_vectors([pitch - 2.0, yaw, 0.0]).0);
        clearance(world, from, dir, (speed.abs() * 0.5).max(150.0)) >= 1.0
    };
    let sharp = delta.abs() > 35.0 && speed > 50.0 * MPH;
    if std::env::var("IW4L_VEH_AUTODRIVE").is_ok_and(|v| v == "2") {
        diag::info!(
            Sim,
            "autodrive: at ({:.0} {:.0} {:.0}) yaw {yaw:.0} aim ({:.0} {:.0}) want {want:.0} pick {:.0} clear {:.2} ahead {ahead} pitch {pitch:.0}",
            origin[0],
            origin[1],
            origin[2],
            aim[0],
            aim[1],
            best.0,
            best.1
        );
    }
    Some(DriveInput {
        throttle: if best.1 < 0.5 || sharp || !ahead || speed > cap {
            0.0
        } else {
            1.0
        },
        brake: if (!ahead || best.1 < 0.3) && speed > 30.0 * MPH {
            1.0
        } else {
            0.0
        },
        steer: (-delta / if ahead { 12.0 } else { 4.0 }).clamp(-1.0, 1.0),
    })
}

/// How much of `dist` along `dir` the vehicle could drive (0..1): a probe
/// that meets drivable ground climbs it and goes on.
fn clearance(world: &mut World, from: Vec3, dir: Vec3, dist: f32) -> f32 {
    let mut at = from;
    let mut dir = dir;
    let mut left = dist;
    for _ in 0..8 {
        let hit = probe(world, at, at + dir * left);
        if hit.startsolid != 0 {
            break;
        }
        if hit.fraction >= 1.0 {
            return 1.0;
        }
        if hit.normal[2] < WALKABLE {
            left *= 1.0 - hit.fraction;
            break;
        }
        left *= 1.0 - hit.fraction;
        at = Vec3::from_array(hit.endpos) + Vec3::Z * STEP;
        let n = Vec3::from_array(hit.normal);
        dir = (dir - n * dir.dot(n)).normalize_or(dir);
    }
    1.0 - left / dist
}

/// The autopilot's look-ahead: a box wider than the vehicle for a margin.
fn probe(world: &mut World, start: Vec3, end: Vec3) -> trace_iw4::Trace {
    let half = HALF_WIDTH * 1.6;
    super::natives::engine::trace(
        world,
        start.to_array(),
        end.to_array(),
        [-half, -half, 0.0],
        [half, half, HEIGHT],
        SOLID,
    )
}

fn trace(world: &mut World, start: Vec3, end: Vec3) -> trace_iw4::Trace {
    super::natives::engine::trace(
        world,
        start.to_array(),
        end.to_array(),
        [-HALF_WIDTH, -HALF_WIDTH, 0.0],
        [HALF_WIDTH, HALF_WIDTH, HEIGHT],
        SOLID,
    )
}

/// One tick of a driven vehicle. Returns the notifies to raise on it.
pub(crate) fn step(world: &mut World, id: u64) -> Vec<(&'static str, Vec<Value>)> {
    let mut notes = Vec::new();
    let (mut drive, mut speed) = {
        let runtime = world.resource::<Runtime>();
        let Some(vehicle) = runtime.vehicles.get(&id) else {
            return notes;
        };
        let Some(drive) = vehicle.drive.clone() else {
            return notes;
        };
        (drive, vehicle.speed)
    };
    let (origin, top) = {
        let mut runtime = world.resource_mut::<Runtime>();
        let origin = vector(runtime.object_field(id, "origin")).unwrap_or([0.0; 3]);
        let top = number(runtime.object_field(id, "veh_topspeed")).unwrap_or(TOP_MPH) * MPH;
        (origin, top)
    };
    let frozen = FrameWorld::from_world(world)
        .client_meta(crate::ClientId(drive.driver))
        .is_some_and(|m| m.controls.frozen);
    if autodrive()
        && !frozen
        && let Some(input) = autopilot(world, drive.driver, origin, drive.yaw, speed)
    {
        drive.input = input;
    }
    let input = drive.input;
    let dt = TICK_S / SUBSTEPS as f32;
    let mut pos = Vec3::from_array(origin);
    let mut normal = Vec3::from_array(drive.normal);
    for _ in 0..SUBSTEPS {
        let input = if drive.airborne {
            DriveInput::default()
        } else {
            input
        };
        let turn = STEER_DEG_S * input.steer * (speed.abs() / 300.0).clamp(0.25, 1.0) * dt;
        let reversing = if speed < 0.0 { -1.0 } else { 1.0 };
        drive.yaw = math_iw4::angle_normalize_360(drive.yaw - turn * reversing);
        let heading = Vec3::from_array(math_iw4::angle_vectors([0.0, drive.yaw, 0.0]).0);
        // Along the ground: forward projected onto the ground plane.
        let along = if drive.airborne {
            heading
        } else {
            (heading - normal * heading.dot(normal)).normalize_or(heading)
        };
        if input.throttle > 0.0 && speed >= 0.0 {
            if speed < top {
                speed = (speed + THROTTLE_MPH_S * MPH * input.throttle * dt).min(top);
            }
        } else if input.brake > 0.0 {
            speed -= BRAKE_MPH_S * MPH * input.brake * dt;
            speed = speed.max(-REVERSE_MPH * MPH);
        } else if !drive.airborne {
            let coast = COAST_MPH_S * MPH * dt;
            speed = if speed.abs() <= coast {
                0.0
            } else {
                speed - coast * speed.signum()
            };
        }
        // A stopped vehicle nobody drives holds on the slope.
        let parked = input.throttle == 0.0 && speed.abs() < 5.0 * MPH;
        if !drive.airborne && !parked {
            speed += -GRAVITY * along.z * SLOPE_GRIP * dt;
        }
        // Downhill runs past the engine's top speed; drag pulls it back.
        if speed > top {
            speed -= (speed - top) * OVERSPEED_DRAG * dt;
        }
        speed = speed.clamp(-REVERSE_MPH * MPH, top.max(0.0) * OVERSPEED);
        let mut velocity = if drive.airborne {
            drive.vertical -= AIR_GRAVITY * dt;
            heading * speed + Vec3::Z * drive.vertical
        } else {
            along * speed
        };
        // Horizontal sweep raised by the step height; walls slide and slow.
        let lift = Vec3::Z * STEP;
        let wanted = pos + velocity * dt;
        let sweep = trace(world, pos + lift, wanted + lift);
        let mut next = wanted;
        if sweep.startsolid == 0 && sweep.fraction < 1.0 {
            let n = Vec3::from_array(sweep.normal);
            if n.z < WALKABLE {
                let into = -velocity.dot(n);
                // Head-on only: a glancing hit slides along the wall.
                if into > CRASH_SPEED && into > velocity.length() * 0.7 && n.z.abs() < 0.35 {
                    diag::info!(
                        Sim,
                        "vehicle drive: {id} crashes at ({:.0} {:.0} {:.0}), {into:.0} u/s into a wall ({:.2} {:.2} {:.2})",
                        pos.x,
                        pos.y,
                        pos.z,
                        n.x,
                        n.y,
                        n.z
                    );
                    notes.push((
                        "veh_collision",
                        vec![
                            Value::Vector(velocity.to_array()),
                            Value::Vector(sweep.normal),
                        ],
                    ));
                }
                velocity -= n * velocity.dot(n).min(0.0);
                let flat = Vec3::new(velocity.x, velocity.y, 0.0);
                speed = flat.length().copysign(speed) * 0.9;
                next =
                    Vec3::from_array(sweep.endpos) - lift + velocity * dt * (1.0 - sweep.fraction);
            } else {
                next = Vec3::from_array(sweep.endpos) - lift;
            }
        }
        // Ground: from step height down past the snap distance. A grounded
        // vehicle keeps to ground no lower than a free fall from its slope
        // would take it, so crests and ramps launch it.
        let reach = if drive.airborne {
            (-velocity.z * dt).max(0.0) + STEP
        } else {
            SNAP
        };
        let ground = trace(world, next + lift, next - Vec3::Z * reach);
        let fall = 0.5 * AIR_GRAVITY * dt * dt + HOLD;
        let hit = ground.startsolid == 0
            && ground.fraction < 1.0
            && ground.normal[2] >= WALKABLE
            && (drive.airborne || ground.endpos[2] >= next.z - fall);
        if hit {
            next.z = ground.endpos[2];
            normal = Vec3::from_array(ground.normal);
            if drive.airborne {
                drive.airborne = false;
                drive.vertical = 0.0;
                notes.push(("veh_landed", Vec::new()));
            }
        } else if !drive.airborne {
            drive.airborne = true;
            drive.vertical = velocity.z;
            normal = Vec3::Z;
            notes.push(("veh_leftground", Vec::new()));
        }
        pos = next;
    }
    drive.normal = normal.to_array();
    let forward = Vec3::from_array(math_iw4::angle_vectors([0.0, drive.yaw, 0.0]).0);
    let pitch = if drive.airborne {
        0.0
    } else {
        let along = (forward - normal * forward.dot(normal)).normalize_or(forward);
        math_iw4::vect_to_angles(along.to_array())[0]
    };
    let roll = -input.steer * (speed / (top.max(1.0))).clamp(0.0, 1.0) * 15.0;
    let velocity = ((pos - Vec3::from_array(origin)) / TICK_S).to_array();
    if world
        .resource::<crate::step::StepRequest>()
        .tick
        .0
        .is_multiple_of(40)
    {
        diag::info!(
            Sim,
            "vehicle drive: {id} at ({:.0} {:.0} {:.0}) yaw {:.0} {:.0} mph{} input {:.1}/{:.1}/{:.2}",
            pos.x,
            pos.y,
            pos.z,
            drive.yaw,
            speed / MPH,
            if drive.airborne { " airborne" } else { "" },
            input.throttle,
            input.brake,
            input.steer
        );
    }
    let mut runtime = world.resource_mut::<Runtime>();
    if let Some(vehicle) = runtime.vehicles.get_mut(&id) {
        vehicle.speed = speed;
        vehicle.velocity = velocity;
        vehicle.heading = forward.to_array();
        vehicle.drive = Some(drive.clone());
    }
    runtime.set_object_field(id, "origin", Value::Vector(pos.to_array()));
    runtime.set_object_field(id, "angles", Value::Vector([pitch, drive.yaw, roll]));
    runtime.set_object_field(id, "veh_speed", Value::Float(speed / MPH));
    runtime.set_object_field(id, "veh_throttle", Value::Float(input.throttle));
    notes
}

fn mount(world: &mut World, receiver: &Value, args: &[Value]) -> Result<Value, String> {
    let client = super::natives::player::player(world, receiver)?;
    let vehicle = super::natives::engine::entity_id(world, arg(args, 0)?)?;
    let yaw = {
        let mut runtime = world.resource_mut::<Runtime>();
        let angles = vector(runtime.object_field(vehicle, "angles")).unwrap_or([0.0; 3]);
        let state = runtime
            .vehicles
            .get_mut(&vehicle)
            .ok_or("parameter 1: not a vehicle")?;
        if state.drive.as_ref().is_some_and(|d| d.driver != client) {
            return Err("vehicle already has a driver".into());
        }
        state.goal = None;
        state.path_running = false;
        state.drive = Some(Drive {
            driver: client,
            input: DriveInput::default(),
            vertical: 0.0,
            airborne: false,
            yaw: angles[1],
            normal: [0.0, 0.0, 1.0],
        });
        runtime.set_object_field(vehicle, "veh_throttle", Value::Float(0.0));
        angles[1]
    };
    {
        let mut frame = FrameWorld::from_world(world);
        if let Some(ps) = frame.player_mut(crate::ClientId(client)) {
            let view = [0.0, yaw, 0.0];
            for ((delta, want), have) in ps.delta_angles.iter_mut().zip(view).zip(ps.viewangles) {
                *delta += math_iw4::angle_subtract(want, have);
            }
            ps.viewangles = view;
        }
    }
    super::players::link_player(
        world,
        client,
        PlayerLink {
            parent: vehicle,
            tag: None,
            origin: SEAT,
            angles: [0.0; 3],
            view: LinkView::Delta,
            clamp: None,
            parent_angles: [0.0, yaw, 0.0],
            restore_view: None,
        },
    );
    super::players::apply_player_links(world);
    diag::info!(Sim, "vehicle: client {client} mounts vehicle {vehicle}");
    raise(
        world,
        Value::Object(vehicle),
        "vehicle_mount",
        vec![receiver.clone()],
    );
    Ok(Value::Undefined)
}

fn dismount(world: &mut World, receiver: &Value) -> Result<Value, String> {
    let client = super::natives::player::player(world, receiver)?;
    let Some(vehicle) = driven(world, client) else {
        return Ok(Value::Undefined);
    };
    if let Some(state) = world.resource_mut::<Runtime>().vehicles.get_mut(&vehicle) {
        state.drive = None;
    }
    super::players::unlink_player(world, client);
    diag::info!(Sim, "vehicle: client {client} dismounts vehicle {vehicle}");
    raise(
        world,
        Value::Object(vehicle),
        "vehicle_dismount",
        vec![receiver.clone()],
    );
    Ok(Value::Undefined)
}

fn input_field(
    world: &World,
    receiver: &Value,
    read: fn(&DriveInput) -> f32,
) -> Result<Value, String> {
    let Value::Object(id) = receiver else {
        return Err("receiver is not a vehicle".into());
    };
    let vehicle = world
        .resource::<Runtime>()
        .vehicles
        .get(id)
        .ok_or("receiver is not a vehicle")?;
    Ok(Value::Float(
        vehicle.drive.as_ref().map_or(0.0, |d| read(&d.input)),
    ))
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    registry.register(Method, "mountvehicle", mount);
    registry.register(Method, "dismountvehicle", |world, receiver, _| {
        dismount(world, receiver)
    });
    registry.register(Method, "vehphys_setspeed", |world, receiver, args| {
        let speed = float(args, 0)? * MPH;
        let Value::Object(id) = *receiver else {
            return Err("receiver is not a vehicle".into());
        };
        let mut runtime = world.resource_mut::<Runtime>();
        let Some(vehicle) = runtime.vehicles.get_mut(&id) else {
            return Ok(Value::Undefined);
        };
        vehicle.speed = speed;
        if vehicle.drive.is_none() {
            vehicle.max_speed = vehicle.max_speed.max(speed);
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "setvehicleteam", |world, receiver, args| {
        let team = super::args::string(args, 0)?;
        let Value::Object(id) = *receiver else {
            return Err("receiver is not a vehicle".into());
        };
        let mut runtime = world.resource_mut::<Runtime>();
        let vehicle = runtime
            .vehicles
            .get_mut(&id)
            .ok_or("receiver is not a vehicle")?;
        vehicle.team = Some(team);
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicle_getthrottle", |world, receiver, _| {
        input_field(world, receiver, |input| input.throttle)
    });
    registry.register(Method, "vehicle_getsteering", |world, receiver, _| {
        input_field(world, receiver, |input| input.steer)
    });
}
