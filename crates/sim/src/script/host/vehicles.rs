use super::args::{arg, float, int, optional, string, vector};
use super::entities::EntityKind;
use crate::script::Namespace::{Function, Method};
use crate::script::runtime::{raise, run_now};
use crate::script::{NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;

pub(crate) const DAMAGE: &str = "maps/mp/gametypes/_callbacksetup::codecallback_vehicledamage";

const VEHICLE_SLOTS: u8 = 8;
const MPH: f32 = 17.6;
const TICK_S: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
const ARRIVED: f32 = 4.0;
const GUNNER_RANGE: f32 = 8192.0;
/// A turret seat's view arcs about `tag_player` (pitch down/up, yaw right/left): out of
/// the door, not into the cabin.
const SEAT_ARCS: [f32; 4] = [-20.0, 75.0, -85.0, 85.0];
/// SP vehicles take no HUD/compass slot (`VEHICLE_SLOTS` is the MP pool).
const SP_SLOT: u8 = u8::MAX;
/// Path speed of an SP ground vehicle whose script and nodes set none.
const SP_DEFAULT_MPH: f32 = 20.0;

#[derive(Clone, Debug)]
pub(crate) struct Plane {
    owner: u32,
    origin: [f32; 3],
    velocity: [f32; 3],
    compass: Option<([String; 2], [i32; 2])>,
}

#[derive(Clone, Debug)]
pub(crate) struct Heli {
    slot: u8,
    pub(super) goal: Option<[f32; 3]>,
    path_node: Option<u64>,
    pub(super) path_running: bool,
    pub(super) velocity: [f32; 3],
    turning: f32,
    hover: Option<Hover>,
    stop_at_goal: bool,
    arrived: bool,
    near_goal: f32,
    near_notified: bool,
    pub(super) speed: f32,
    pub(super) max_speed: f32,
    accel: f32,
    decel: f32,
    pub(super) heading: [f32; 3],
    yaw_speed: f32,
    target_yaw: Option<f32>,
    goal_yaw: Option<f32>,
    look_at: Option<u64>,
    max_pitch: f32,
    max_roll: f32,
    owner: Option<u32>,
    compass: Option<([String; 2], [i32; 2])>,
    weapon: Option<u32>,
    turret: Option<TurretAim>,
    on_target: bool,
    gunner: Option<u32>,
    next_fire_ms: i32,
    /// A player drives it (`mountvehicle`).
    pub(crate) drive: Option<super::vehicle_drive::Drive>,
    /// SP `setvehicleteam`.
    pub(crate) team: Option<String>,
    fired: bool,
    /// Driven by `vehicledriveto` (an SP ground vehicle): it keeps to the
    /// ground and pitches with it.
    ground: bool,
    /// SP `veh_pathdir` "reverse": the path is followed back through the
    /// nodes that target the current one.
    reverse: bool,
    /// The script set a speed (`vehicle_setspeed*`); path nodes without a
    /// `speed` key then keep it, a stop included.
    scripted_speed: bool,
}

#[derive(Clone, Debug)]
struct Hover {
    radius: f32,
    speed: f32,
    accel: f32,
    center: Option<[f32; 3]>,
    velocity: [f32; 3],
    phase: f32,
}

impl Hover {
    fn advance(&mut self, origin: [f32; 3]) -> [f32; 3] {
        let center = *self.center.get_or_insert(origin);
        self.phase = (self.phase + self.speed / self.radius * TICK_S) % std::f32::consts::TAU;
        let target = [
            center[0] + self.radius * self.phase.cos(),
            center[1] + self.radius * self.phase.sin(),
            center[2],
        ];
        let delta = glam::Vec3::from_array(target) - glam::Vec3::from_array(origin);
        let wanted = delta.normalize_or_zero() * self.speed.min(delta.length() / TICK_S);
        let velocity = glam::Vec3::from_array(self.velocity);
        let velocity = velocity + (wanted - velocity).clamp_length_max(self.accel * TICK_S);
        let candidate = glam::Vec3::from_array(origin) + velocity * TICK_S;
        let next = glam::Vec3::from_array(center)
            + (candidate - glam::Vec3::from_array(center)).clamp_length_max(self.radius);
        self.velocity = ((next - glam::Vec3::from_array(origin)) / TICK_S).to_array();
        next.to_array()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TurretAim {
    Entity(u64, [f32; 3]),
    Point([f32; 3]),
}

impl Default for Heli {
    fn default() -> Self {
        Self {
            slot: 0,
            goal: None,
            path_node: None,
            path_running: false,
            velocity: [0.0; 3],
            turning: 1.0,
            hover: None,
            stop_at_goal: true,
            arrived: false,
            near_goal: 0.0,
            near_notified: false,
            speed: 0.0,
            max_speed: 0.0,
            accel: 0.0,
            decel: 0.0,
            heading: [1.0, 0.0, 0.0],
            yaw_speed: 90.0,
            target_yaw: None,
            goal_yaw: None,
            look_at: None,
            max_pitch: 20.0,
            max_roll: 30.0,
            owner: None,
            compass: None,
            weapon: None,
            turret: None,
            on_target: false,
            gunner: None,
            next_fire_ms: 0,
            drive: None,
            team: None,
            fired: false,
            ground: false,
            reverse: false,
            scripted_speed: false,
        }
    }
}

fn heli<'a>(world: &'a mut World, receiver: &Value) -> Result<&'a mut Heli, String> {
    let Value::Object(id) = receiver else {
        return Err("receiver is not a vehicle".into());
    };
    world
        .resource_mut::<Runtime>()
        .into_inner()
        .vehicles
        .get_mut(id)
        .ok_or_else(|| "receiver is not a vehicle".into())
}

fn parked_vehicle(world: &World, receiver: &Value) -> bool {
    matches!(receiver, Value::Object(id) if world
        .resource::<Runtime>()
        .entities
        .get(id)
        .is_some_and(|entity| entity.classname.starts_with("script_vehicle")))
}

pub(crate) fn is_heli(world: &World, receiver: &Value) -> bool {
    matches!(receiver, Value::Object(id) if world.resource::<Runtime>().vehicles.contains_key(id))
}

pub(crate) fn aim_turret(
    world: &mut World,
    receiver: &Value,
    aim: Option<TurretAim>,
) -> Result<Value, String> {
    let heli = heli(world, receiver)?;
    heli.turret = aim;
    heli.on_target = false;
    Ok(Value::Undefined)
}

fn aim_point(world: &mut World, aim: TurretAim) -> Option<[f32; 3]> {
    match aim {
        TurretAim::Point(point) => Some(point),
        TurretAim::Entity(object, offset) => {
            if !world.resource::<Runtime>().live(&object) {
                return None;
            }
            match super::players::entity_field(world, object, "origin") {
                Value::Vector(o) => Some(std::array::from_fn(|i| o[i] + offset[i])),
                _ => None,
            }
        }
    }
}

/// The gunner's eye, view direction and attack button.
fn gunner_input(world: &mut World, client: u32) -> Option<([f32; 3], [f32; 3], bool)> {
    let mut frame = crate::frame::FrameWorld::from_world(world);
    let ps = frame.player(crate::ClientId(client))?;
    let (angles, origin, height) = (ps.viewangles, ps.origin, ps.view_height_current);
    let held = world
        .resource::<Runtime>()
        .players
        .get(&client)?
        .held_buttons;
    Some((
        [origin[0], origin[1], origin[2] + height],
        math_iw4::angle_vectors(angles).0,
        held & playerstate_iw4::buttons::ATTACK != 0,
    ))
}

/// SP `vehicle useBy( player )`: the player takes the vehicle's turret seat
/// (linked at `tag_player`, the view aims the turret, attack raises
/// `turret_fire`); a second call by the same player leaves it.
fn use_by(world: &mut World, receiver: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Object(id) = *receiver else {
        return Err("receiver is not a vehicle".into());
    };
    let client = world
        .resource::<Runtime>()
        .player_client_of(arg(args, 0)?)
        .ok_or("parameter 1: not a player")?;
    let leaving = heli(world, receiver)?.gunner == Some(client);
    {
        let heli = heli(world, receiver)?;
        heli.gunner = (!leaving).then_some(client);
        heli.owner = (!leaving).then_some(client);
        heli.turret = None;
        heli.on_target = false;
    }
    let weapons = if leaving {
        "enableweapons"
    } else {
        "disableweapons"
    };
    let native = world.resource::<NativeRegistry>().get(Method, weapons);
    if let Some(native) = native {
        native(world, &arg(args, 0)?.clone(), &[])?;
    }
    if leaving {
        super::players::unlink_player(world, client);
        diag::info!(
            Sim,
            "vehicle: client {client} leaves the turret of vehicle {id}"
        );
        return Ok(Value::Undefined);
    }
    let tag: Option<std::sync::Arc<str>> = Some("tag_player".into());
    let (_, axis) = super::players::link_parent_pose(world, id, tag.as_deref());
    super::players::link_player(
        world,
        client,
        super::players::PlayerLink {
            parent: id,
            tag,
            origin: [0.0; 3],
            angles: [0.0; 3],
            view: super::players::LinkView::Delta,
            clamp: Some(SEAT_ARCS),
            parent_angles: math_iw4::axis_to_angles(axis),
            restore_view: None,
        },
    );
    super::players::apply_player_links(world);
    let weapon = heli(world, receiver)?.weapon.map(|weapon| {
        crate::frame::FrameWorld::from_world(world)
            .weapon_script_name(weapon)
            .to_owned()
    });
    let tag_angles = |world: &mut World, tag: &str| {
        super::presence::tag_world(world, id, tag).map(|(at, axis)| {
            (
                at.map(|v| v as i32),
                math_iw4::axis_to_angles(axis).map(|v| v as i32),
            )
        })
    };
    let seat = tag_angles(world, "tag_player");
    let turret = tag_angles(world, "tag_turret");
    let flash = tag_angles(world, "tag_flash");
    let angles = vec_field(&mut world.resource_mut::<Runtime>(), id, "angles");
    diag::info!(
        Sim,
        "vehicle: client {client} uses the turret of vehicle {id} (weapon {}) angles={angles:?} tag_player={seat:?} tag_turret={turret:?} tag_flash={flash:?}",
        weapon.as_deref().unwrap_or("none")
    );
    Ok(Value::Undefined)
}

fn fire_weapon(world: &mut World, receiver: &Value, args: &[Value]) -> Result<Value, String> {
    let Value::Object(id) = *receiver else {
        return Err("receiver is not a vehicle".into());
    };
    let (weapon, owner, turret, heading) = {
        let heli = heli(world, receiver)?;
        (heli.weapon, heli.owner, heli.turret, heli.heading)
    };
    let weapon = weapon.ok_or("vehicle has no weapon")?;
    if !std::mem::replace(&mut heli(world, receiver)?.fired, true) {
        let name = crate::frame::FrameWorld::from_world(world)
            .weapon_script_name(weapon)
            .to_owned();
        diag::info!(Sim, "vehicle: {id} fires its turret weapon {name}");
    }
    let tag = optional(args, 0, string)?.unwrap_or_default();
    let from = super::presence::tag_world(world, id, &tag)
        .map(|(origin, _)| origin)
        .unwrap_or_else(|| super::turrets::muzzle(world, id));
    let aim = match args.get(1) {
        Some(target) if *target != Value::Undefined => {
            let target = super::natives::engine::entity_id(world, target)?;
            let offset = optional(args, 2, vector)?.unwrap_or([0.0; 3]);
            Some(TurretAim::Entity(target, offset))
        }
        _ => turret,
    };
    let point = aim.and_then(|aim| aim_point(world, aim));
    let dir = point
        .and_then(|p| {
            glam::Vec3::from_array(std::array::from_fn(|i| p[i] - from[i])).try_normalize()
        })
        .map_or(heading, |d| d.to_array());
    let kind = crate::frame::FrameWorld::from_world(world)
        .combat_facts_for(weapon)
        .and_then(|f| weapon_iw4::fire_weapon_kind(f.weap_type, f.weap_class));
    if matches!(kind, Some(weapon_iw4::FireWeaponKind::Bullet) | None) {
        super::turrets::fire_bullet(world, id, from, dir, weapon, owner.map(crate::ClientId));
        return Ok(Value::Undefined);
    }
    let owner = owner
        .filter(|client| world.resource::<Runtime>().players.contains_key(client))
        .ok_or("vehicle owner is not connected")?;
    let end = std::array::from_fn(|i| from[i] + dir[i] * 1000.0);
    super::weapons::launch(
        world,
        crate::Attacker::Client(crate::ClientId(owner)),
        weapon,
        from,
        end,
    )
}

fn vec_field(runtime: &mut Runtime, object: u64, name: &str) -> [f32; 3] {
    match runtime.object_field(object, name) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn spawn_vehicle(
    world: &mut World,
    classname: &str,
    origin: [f32; 3],
    angles: [f32; 3],
    model: &str,
    flight: Option<Heli>,
) -> Result<Value, String> {
    let slot = if flight.as_ref().is_some_and(|f| f.slot == SP_SLOT) {
        Some(SP_SLOT)
    } else if flight.is_some() {
        let runtime = world.resource::<Runtime>();
        Some(
            (0..VEHICLE_SLOTS)
                .find(|slot| {
                    !runtime
                        .vehicles
                        .iter()
                        .any(|(id, vehicle)| runtime.live(id) && vehicle.slot == *slot)
                })
                .ok_or("vehicle pool exhausted")?,
        )
    } else {
        None
    };
    let presence = super::presence::spawn_presence(world, origin)?;
    let mut runtime = world.resource_mut::<Runtime>();
    let id = runtime.create_entity(EntityKind::Vehicle, classname)?;
    runtime.set_object_field(id, "origin", Value::Vector(origin));
    runtime.set_object_field(id, "angles", Value::Vector(angles));
    runtime.set_object_field(id, "model", Value::string(model));
    runtime.entities.get_mut(&id).unwrap().presence = Some(presence);
    if let Some(mut flight) = flight {
        flight.slot = slot.unwrap();
        flight.heading = math_iw4::angle_vectors(angles).0;
        runtime.set_object_field(id, "veh_speed", Value::Float(0.0));
        runtime.vehicles.insert(id, flight);
    }
    Ok(Value::Object(id))
}

fn set_goal(heli: &mut Heli, goal: [f32; 3], stop: bool) {
    if let Some(hover) = &mut heli.hover {
        hover.center = None;
        hover.velocity = [0.0; 3];
    }
    heli.goal = Some(goal);
    heli.stop_at_goal = stop;
    heli.arrived = false;
    heli.near_notified = false;
}

fn node(runtime: &mut Runtime, object: u64) -> Result<[f32; 3], String> {
    if !runtime
        .entities
        .get(&object)
        .is_some_and(|entity| entity.classname.starts_with("info_vehicle_node"))
    {
        return Err("parameter is not a vehicle node".into());
    }
    match runtime.object_field(object, "origin") {
        Value::Vector(origin) => Ok(origin),
        _ => Err("vehicle node has no origin".into()),
    }
}

fn next_node(runtime: &mut Runtime, object: u64) -> Result<Option<u64>, String> {
    node(runtime, object)?;
    let target = match runtime.object_field(object, "target") {
        Value::Undefined => return Ok(None),
        Value::String(name) if name.is_empty() => return Ok(None),
        Value::String(name) => name,
        _ => return Err("vehicle node target must be a name".into()),
    };
    let candidates: Vec<u64> = runtime
        .entities
        .iter()
        .filter(|(_, entity)| entity.classname.starts_with("info_vehicle_node"))
        .map(|(id, _)| *id)
        .collect();
    let matches: Vec<u64> = candidates
        .into_iter()
        .filter(|id| runtime.object_field(*id, "targetname") == Value::String(target.clone()))
        .collect();
    match matches.as_slice() {
        [object] => Ok(Some(*object)),
        _ => Err(format!(
            "vehicle path target '{target}' does not identify one node"
        )),
    }
}

/// An SP vehicle spawner: a map `script_vehicle_*` with spawnflags bit 2.
/// The node whose `target` is `object` (the path read backward); the first by
/// entity order when several merge into it.
fn prev_node(runtime: &mut Runtime, object: u64) -> Result<Option<u64>, String> {
    node(runtime, object)?;
    let name = match runtime.object_field(object, "targetname") {
        Value::String(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };
    let candidates: Vec<u64> = runtime
        .entities
        .iter()
        .filter(|(_, entity)| entity.classname.starts_with("info_vehicle_node"))
        .map(|(id, _)| *id)
        .collect();
    Ok(candidates
        .into_iter()
        .find(|id| runtime.object_field(*id, "target") == Value::String(name.clone())))
}

fn path_successor(
    runtime: &mut Runtime,
    object: u64,
    reverse: bool,
) -> Result<Option<u64>, String> {
    if reverse {
        prev_node(runtime, object)
    } else {
        next_node(runtime, object)
    }
}

/// SP scripts turn a path vehicle around by writing `veh_pathdir` (arcadia's
/// Stryker backs down the street to the extraction node); the vehicle then
/// heads for the neighbouring node in the new direction.
fn sync_path_direction(runtime: &mut Runtime, id: u64) {
    let Some(heli) = runtime.vehicles.get(&id) else {
        return;
    };
    let (Some(current), was) = (heli.path_node, heli.reverse) else {
        return;
    };
    if heli.slot != SP_SLOT {
        return;
    }
    let reverse = match runtime.object_field(id, "veh_pathdir") {
        Value::String(dir) => dir.eq_ignore_ascii_case("reverse"),
        _ => was,
    };
    if reverse == was {
        return;
    }
    runtime.vehicles.get_mut(&id).unwrap().reverse = reverse;
    let target = match path_successor(runtime, current, reverse) {
        Ok(Some(target)) => target,
        _ => return,
    };
    if let Ok(origin) = node(runtime, target) {
        runtime.vehicles.get_mut(&id).unwrap().path_running = true;
        path_goal(runtime, id, target, origin);
    }
}

pub(crate) fn is_sp_spawner(world: &World, value: &Value) -> bool {
    let Value::Object(id) = value else {
        return false;
    };
    let runtime = world.resource::<Runtime>();
    if runtime.vehicles.contains_key(id) {
        return false;
    }
    let Some(entity) = runtime.entities.get(id) else {
        return false;
    };
    if super::entities::code_classname(&entity.classname) != "script_vehicle"
        || entity.kind == EntityKind::Vehicle
    {
        return false;
    }
    let symbol = runtime
        .program
        .as_ref()
        .and_then(|program| program.symbol_ids.get("spawnflags").copied())
        .or_else(|| runtime.dynamic_symbols.get("spawnflags").copied());
    let flags = symbol
        .and_then(|symbol| runtime.objects.get(id)?.get(&symbol).cloned())
        .unwrap_or(Value::Undefined);
    matches!(flags, Value::Int(flags) if flags & 2 != 0)
}

/// An SP vehicle's state; its turret weapon comes from the `vehicletype` def.
fn sp_vehicle(world: &mut World, object: u64, angles: [f32; 3]) -> Heli {
    let weapon = match world
        .resource_mut::<Runtime>()
        .object_field(object, "vehicletype")
    {
        Value::String(vehicletype) => {
            crate::frame::FrameWorld::from_world(world).vehicle_turret_weapon(&vehicletype)
        }
        _ => None,
    };
    Heli {
        slot: SP_SLOT,
        heading: math_iw4::angle_vectors(angles).0,
        yaw_speed: 180.0,
        weapon,
        ..Default::default()
    }
}

/// SP `vehicle_dospawn()`: a vehicle with the spawner's keys, model and place.
fn sp_spawn(world: &mut World, spawner: u64) -> Result<Value, String> {
    let (classname, fields, origin, angles, model) = {
        let mut runtime = world.resource_mut::<Runtime>();
        let classname = runtime.entities[&spawner].classname.to_string();
        let fields = runtime
            .objects
            .get(&spawner)
            .cloned()
            .unwrap_or_else(Default::default);
        let origin = vec_field(&mut runtime, spawner, "origin");
        let angles = vec_field(&mut runtime, spawner, "angles");
        let model = match runtime.object_field(spawner, "model") {
            Value::String(model) => model.to_string(),
            _ => String::new(),
        };
        (classname, fields, origin, angles, model)
    };
    let state = sp_vehicle(world, spawner, angles);
    let vehicle = spawn_vehicle(world, &classname, origin, angles, &model, Some(state))?;
    let Value::Object(id) = vehicle else {
        return Ok(vehicle);
    };
    // The spawner keeps its targetname to itself: `so_snowrace` re-finds its one
    // spawner with `getent( name, "targetname" )` while earlier bikes still live.
    let mut runtime = world.resource_mut::<Runtime>();
    let skipped = [runtime.symbol("spawnflags"), runtime.symbol("targetname")];
    if let Some(target) = runtime.objects.get_mut(&id) {
        for (field, value) in fields {
            if !skipped.contains(&field) {
                target.entry(field).or_insert(value);
            }
        }
    }
    Ok(vehicle)
}

/// SP map vehicles that are not spawners are live vehicles from the start.
pub(crate) fn install_placed(world: &mut World) {
    if !super::players::single_player(world) {
        return;
    }
    let candidates: Vec<u64> = world
        .resource::<Runtime>()
        .entities
        .iter()
        .filter(|(_, e)| {
            e.kind == EntityKind::Map
                && super::entities::code_classname(&e.classname) == "script_vehicle"
        })
        .map(|(id, _)| *id)
        .collect();
    let mut placed = 0;
    for id in candidates {
        if is_sp_spawner(world, &Value::Object(id)) {
            continue;
        }
        let angles = vec_field(&mut world.resource_mut::<Runtime>(), id, "angles");
        let vehicle = sp_vehicle(world, id, angles);
        let mut runtime = world.resource_mut::<Runtime>();
        runtime.set_object_field(id, "veh_speed", Value::Float(0.0));
        runtime.vehicles.insert(id, vehicle);
        placed += 1;
    }
    if placed > 0 {
        diag::info!(Sim, "vehicle: {placed} placed SP vehicles");
    }
}

/// SP path nodes carry the speed (mph) the vehicle takes from them.
fn node_speed(runtime: &mut Runtime, node: u64) -> Option<f32> {
    match runtime.object_field(node, "speed") {
        Value::Int(speed) => Some(speed as f32),
        Value::Float(speed) => Some(speed),
        _ => None,
    }
}

fn path_goal(runtime: &mut Runtime, vehicle: u64, node: u64, origin: [f32; 3]) {
    let reverse = runtime.vehicles.get(&vehicle).is_some_and(|v| v.reverse);
    let last = !matches!(path_successor(runtime, node, reverse), Ok(Some(_)));
    let speed = node_speed(runtime, node);
    let heli = runtime.vehicles.get_mut(&vehicle).unwrap();
    if heli.slot == SP_SLOT {
        if let Some(speed) = speed.filter(|s| *s > 0.0) {
            heli.max_speed = speed * MPH;
        }
        if heli.max_speed <= 0.0 && !heli.scripted_speed {
            heli.max_speed = SP_DEFAULT_MPH * MPH;
        }
        heli.accel = heli.accel.max(10.0 * MPH);
        heli.decel = heli.decel.max(10.0 * MPH);
        heli.path_node = Some(node);
        set_goal(heli, origin, last);
    } else {
        heli.path_node = Some(node);
        set_goal(heli, origin, true);
    }
}

fn vehicle_array(world: &mut World, prefix: Option<&str>) -> Result<Value, String> {
    let runtime = world.resource::<Runtime>();
    let ids = match prefix {
        None => {
            let mut vehicles: Vec<_> = runtime
                .vehicles
                .iter()
                .filter(|(id, _)| runtime.live(id))
                .map(|(id, vehicle)| (vehicle.slot, *id))
                .collect();
            vehicles.sort_by_key(|(slot, _)| *slot);
            vehicles
                .into_iter()
                .map(|(_, id)| Value::Object(id))
                .collect()
        }
        Some(prefix) => runtime
            .entities
            .iter()
            .filter(|(id, entity)| {
                entity.classname.starts_with(prefix)
                    && (prefix != "script_vehicle"
                        || (entity.kind != EntityKind::Vehicle
                            && !runtime.vehicles.contains_key(id)
                            && super::entities::code_classname(&entity.classname) == prefix))
            })
            .map(|(id, _)| Value::Object(*id))
            .collect(),
    };
    super::arrays::new_array(world, ids)
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    registry.register(Function, "getallvehiclenodes", |world, _, _| {
        vehicle_array(world, Some("info_vehicle_node"))
    });
    registry.register(Function, "vehicle_getarray", |world, _, _| {
        vehicle_array(world, None)
    });
    registry.register(Function, "getnumvehicles", |world, _, _| {
        let runtime = world.resource::<Runtime>();
        Ok(Value::Int(
            runtime
                .vehicles
                .keys()
                .filter(|id| runtime.live(id))
                .count() as i32,
        ))
    });
    registry.register(Function, "vehicle_getspawnerarray", |world, _, _| {
        vehicle_array(world, Some("script_vehicle"))
    });
    registry.register(Method, "getvehicleowner", |world, receiver, _| {
        let object = super::natives::engine::entity_id(world, receiver)?;
        let owner = match world.resource::<Runtime>().planes.get(&object) {
            Some(plane) => Some(plane.owner),
            None => heli(world, receiver)?.owner,
        };
        Ok(owner.map_or(Value::Undefined, |client| {
            super::players::player_object(world, client)
        }))
    });
    registry.register(Method, "vehicle_getvelocity", |world, receiver, _| {
        let object = super::natives::engine::entity_id(world, receiver)?;
        let velocity = match world.resource::<Runtime>().planes.get(&object) {
            Some(plane) => plane.velocity,
            None => heli(world, receiver)?.velocity,
        };
        Ok(Value::Vector(velocity))
    });
    registry.register(Method, "setacceleration", |world, receiver, args| {
        let value = float(args, 0)?;
        if !value.is_finite() || value < 0.0 {
            return Err("acceleration must be finite and nonnegative".into());
        }
        heli(world, receiver)?.accel = value * MPH;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setdeceleration", |world, receiver, args| {
        let value = float(args, 0)?;
        if !value.is_finite() || value < 0.0 {
            return Err("deceleration must be finite and nonnegative".into());
        }
        heli(world, receiver)?.decel = value * MPH;
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicle_teleport", |world, receiver, args| {
        let origin = vector(args, 0)?;
        let angles = vector(args, 1)?;
        let object = super::natives::engine::entity_id(world, receiver)?;
        {
            let mut runtime = world.resource_mut::<Runtime>();
            if let Some(plane) = runtime.planes.get_mut(&object) {
                plane.origin = origin;
                plane.velocity = [0.0; 3];
            } else {
                let vehicle = runtime
                    .vehicles
                    .get_mut(&object)
                    .ok_or("receiver is not a vehicle")?;
                vehicle.goal = None;
                vehicle.path_running = false;
                vehicle.speed = 0.0;
                vehicle.velocity = [0.0; 3];
                vehicle.heading = math_iw4::angle_vectors(angles).0;
                if let Some(hover) = &mut vehicle.hover {
                    hover.center = None;
                    hover.velocity = [0.0; 3];
                }
            }
            runtime.entities.get_mut(&object).unwrap().linked_to = None;
            runtime.set_object_field(object, "origin", Value::Vector(origin));
            runtime.set_object_field(object, "angles", Value::Vector(angles));
            runtime.set_object_field(object, "veh_speed", Value::Float(0.0));
        }
        if let Some(mut mechanics) = world.get_resource_mut::<super::mechanics::Mechanics>() {
            mechanics.cancel(object);
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicledriveto", |world, receiver, args| {
        let goal = vector(args, 0)?;
        let speed = float(args, 1)?.max(0.0) * MPH;
        let vehicle = heli(world, receiver)?;
        vehicle.ground = true;
        vehicle.max_speed = speed;
        vehicle.accel = vehicle.accel.max(speed);
        vehicle.decel = vehicle.decel.max(speed);
        vehicle.path_running = false;
        set_goal(vehicle, goal, true);
        Ok(Value::Undefined)
    });
    registry.register(Method, "attachpath", |world, receiver, args| {
        heli(world, receiver)?;
        let object = super::natives::engine::entity_id(world, arg(args, 0)?)?;
        let origin = node(&mut world.resource_mut::<Runtime>(), object)?;
        let vehicle = heli(world, receiver)?;
        vehicle.path_node = Some(object);
        vehicle.path_running = false;
        vehicle.goal = None;
        if vehicle.slot == SP_SLOT
            && let Value::Object(id) = *receiver
        {
            // `AttachPath` puts an SP vehicle on the path's node (arcadia's
            // Stryker spawns 13 k u away and is attached to its street path).
            let mut runtime = world.resource_mut::<Runtime>();
            let angles = match runtime.object_field(object, "angles") {
                Value::Vector(angles) => Some(angles),
                _ => None,
            };
            runtime.set_object_field(id, "origin", Value::Vector(origin));
            if let Some(angles) = angles {
                runtime.set_object_field(id, "angles", Value::Vector(angles));
                runtime.vehicles.get_mut(&id).unwrap().heading = math_iw4::angle_vectors(angles).0;
            }
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "startpath", |world, receiver, args| {
        let attached = heli(world, receiver)?.path_node;
        let object = match args.first() {
            Some(value) => super::natives::engine::entity_id(world, value)?,
            None => attached.ok_or("vehicle has no attached path")?,
        };
        let origin = node(&mut world.resource_mut::<Runtime>(), object)?;
        let Value::Object(id) = *receiver else {
            return Err("receiver is not a vehicle".into());
        };
        heli(world, receiver)?.path_running = true;
        path_goal(&mut world.resource_mut::<Runtime>(), id, object, origin);
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicle_dospawn", |world, receiver, args| {
        let object = super::natives::engine::entity_id(world, receiver)?;
        if args.is_empty() && super::players::single_player(world) {
            if !is_sp_spawner(world, receiver) {
                return Err("receiver is not a vehicle spawner".into());
            }
            return sp_spawn(world, object);
        }
        let definition = string(args, 0)?;
        let owner = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 1)?)
            .ok_or("vehicle owner is not a player")?;
        let (origin, angles, model) = {
            let mut runtime = world.resource_mut::<Runtime>();
            if !runtime.entities[&object]
                .classname
                .starts_with("script_vehicle")
                || runtime.entities[&object].kind == EntityKind::Vehicle
            {
                return Err("receiver is not a vehicle spawner".into());
            }
            (
                vec_field(&mut runtime, object, "origin"),
                vec_field(&mut runtime, object, "angles"),
                runtime.object_field(object, "model"),
            )
        };
        let Value::String(model) = model else {
            return Err("vehicle spawner has no model".into());
        };
        let frame = crate::frame::FrameWorld::from_world(world);
        let weapon = frame.vehicle_turret_weapon(&definition);
        let compass = frame.vehicle_compass(&definition).cloned();
        spawn_vehicle(
            world,
            "script_vehicle",
            origin,
            angles,
            &model,
            Some(Heli {
                owner: Some(owner),
                weapon,
                compass,
                ..Default::default()
            }),
        )
    });
    registry.register(Function, "spawnvehicle", |world, _, args| {
        let model = string(args, 0)?;
        let targetname = string(args, 1)?;
        let definition = string(args, 2)?;
        let origin = vector(args, 3)?;
        let angles = vector(args, 4)?;
        let sp = super::players::single_player(world);
        let frame = crate::frame::FrameWorld::from_world(world);
        let weapon = frame.vehicle_turret_weapon(&definition);
        let compass = frame.vehicle_compass(&definition).cloned();
        let vehicle = spawn_vehicle(
            world,
            "script_vehicle",
            origin,
            angles,
            &model,
            Some(Heli {
                slot: if sp { SP_SLOT } else { 0 },
                weapon,
                compass: compass.filter(|_| !sp),
                ..Default::default()
            }),
        )?;
        if let Value::Object(object) = vehicle {
            world.resource_mut::<Runtime>().set_object_field(
                object,
                "targetname",
                Value::string(&targetname),
            );
        }
        Ok(vehicle)
    });
    registry.register(Function, "spawnhelicopter", |world, _, args| {
        let owner = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("spawnHelicopter owner is not a player")?;
        let (origin, angles) = (vector(args, 1)?, vector(args, 2)?);
        let vehicle = string(args, 3)?;
        let model = string(args, 4)?;
        let frame = crate::frame::FrameWorld::from_world(world);
        let weapon = frame.vehicle_turret_weapon(&vehicle);
        let compass = frame.vehicle_compass(&vehicle).cloned();
        spawn_vehicle(
            world,
            "script_vehicle",
            origin,
            angles,
            &model,
            Some(Heli {
                owner: Some(owner),
                compass,
                weapon,
                ..Heli::default()
            }),
        )
    });
    registry.register(Function, "spawnplane", |world, _, args| {
        let owner = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("spawnPlane owner is not a player")?;
        let classname = string(args, 1)?;
        let origin = vector(args, 2)?;
        let friendly = optional(args, 3, string)?;
        let enemy = optional(args, 4, string)?;
        let compass =
            friendly.map(|friendly| ([friendly.clone(), enemy.unwrap_or(friendly)], [32, 32]));
        let vehicle = spawn_vehicle(world, &classname, origin, [0.0; 3], "", None)?;
        if let Value::Object(object) = vehicle {
            world.resource_mut::<Runtime>().planes.insert(
                object,
                Plane {
                    owner,
                    origin,
                    velocity: [0.0; 3],
                    compass,
                },
            );
        }
        Ok(vehicle)
    });
    registry.register(Method, "setvehgoalpos", |world, receiver, args| {
        let goal = vector(args, 0)?;
        let stop = optional(args, 1, int)?.unwrap_or(0) != 0;
        let heli = heli(world, receiver)?;
        heli.path_running = false;
        set_goal(heli, goal, stop);
        Ok(Value::Undefined)
    });
    registry.register(Method, "vehicle_setspeed", |world, receiver, args| {
        let speed = float(args, 0)?.max(0.0) * MPH;
        let accel = optional(args, 1, float)?.unwrap_or(speed / MPH).max(1.0) * MPH;
        let decel = optional(args, 2, float)?.map_or(accel, |d| d.max(1.0) * MPH);
        let heli = heli(world, receiver)?;
        heli.max_speed = speed;
        heli.accel = accel;
        heli.decel = decel;
        heli.scripted_speed = true;
        Ok(Value::Undefined)
    });
    registry.register(
        Method,
        "vehicle_setspeedimmediate",
        |world, receiver, args| {
            let speed = float(args, 0)?.max(0.0) * MPH;
            let heli = heli(world, receiver)?;
            heli.max_speed = speed;
            heli.speed = speed;
            heli.scripted_speed = true;
            heli.accel = heli.accel.max(speed);
            heli.decel = heli.decel.max(speed);
            Ok(Value::Undefined)
        },
    );
    registry.register(Method, "vehicle_getspeed", |world, receiver, _| {
        // A placed `script_vehicle` nothing drives is parked. SP `_vehicle`
        // polls this every frame per vehicle; an error there cost three
        // runtime faults a poll and never let the loop see a stop.
        if !is_heli(world, receiver) && parked_vehicle(world, receiver) {
            return Ok(Value::Float(0.0));
        }
        Ok(Value::Float(heli(world, receiver)?.speed / MPH))
    });
    registry.register(Method, "setyawspeed", |world, receiver, args| {
        let speed = float(args, 0)?.max(0.0);
        heli(world, receiver)?.yaw_speed = speed;
        Ok(Value::Undefined)
    });
    registry.register(Method, "settargetyaw", |world, receiver, args| {
        let yaw = float(args, 0)?;
        heli(world, receiver)?.target_yaw = Some(yaw);
        Ok(Value::Undefined)
    });
    registry.register(Method, "cleartargetyaw", |world, receiver, _| {
        heli(world, receiver)?.target_yaw = None;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setgoalyaw", |world, receiver, args| {
        let yaw = float(args, 0)?;
        heli(world, receiver)?.goal_yaw = Some(yaw);
        Ok(Value::Undefined)
    });
    registry.register(Method, "cleargoalyaw", |world, receiver, _| {
        heli(world, receiver)?.goal_yaw = None;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setmaxpitchroll", |world, receiver, args| {
        let (pitch, roll) = (float(args, 0)?.abs(), float(args, 1)?.abs());
        let heli = heli(world, receiver)?;
        heli.max_pitch = pitch;
        heli.max_roll = roll;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setneargoalnotifydist", |world, receiver, args| {
        let dist = float(args, 0)?.max(0.0);
        heli(world, receiver)?.near_goal = dist;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setlookatent", |world, receiver, args| {
        let target = super::natives::engine::entity_id(world, arg(args, 0)?)?;
        heli(world, receiver)?.look_at = Some(target);
        Ok(Value::Undefined)
    });
    registry.register(Method, "clearlookatent", |world, receiver, _| {
        heli(world, receiver)?.look_at = None;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setturningability", |world, receiver, args| {
        let ability = float(args, 0)?;
        if !ability.is_finite() || !(0.0..=1.0).contains(&ability) {
            return Err("turning ability must be between zero and one".into());
        }
        heli(world, receiver)?.turning = ability;
        Ok(Value::Undefined)
    });
    registry.register(Method, "sethoverparams", |world, receiver, args| {
        let (radius, speed, accel) = (float(args, 0)?, float(args, 1)?, float(args, 2)?);
        if [radius, speed, accel]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(
                "hover radius, speed and acceleration must be finite and nonnegative".into(),
            );
        }
        heli(world, receiver)?.hover =
            (radius > 0.0 && speed > 0.0 && accel > 0.0).then_some(Hover {
                radius,
                speed,
                accel,
                center: None,
                velocity: [0.0; 3],
                phase: 0.0,
            });
        Ok(Value::Undefined)
    });
    registry.register(Method, "setdamagestage", |world, receiver, args| {
        heli(world, receiver)?;
        if args.len() != 1 {
            return Err("SetDamageStage expects an integer stage".into());
        }
        int(args, 0)?;
        super::registry::unavailable(
            world,
            "setdamagestage",
            "helicopter damage-stage presentation is not implemented",
        )
    });
    registry.register(Method, "vehicleturretcontrolon", |world, receiver, args| {
        let client = world
            .resource::<Runtime>()
            .player_client_of(arg(args, 0)?)
            .ok_or("parameter 1: not a player")?;
        let heli = heli(world, receiver)?;
        heli.gunner = Some(client);
        heli.on_target = false;
        Ok(Value::Undefined)
    });
    registry.register(
        Method,
        "vehicleturretcontroloff",
        |world, receiver, args| {
            world
                .resource::<Runtime>()
                .player_client_of(arg(args, 0)?)
                .ok_or("parameter 1: not a player")?;
            let heli = heli(world, receiver)?;
            heli.gunner = None;
            heli.turret = None;
            Ok(Value::Undefined)
        },
    );
    registry.register(Method, "setvehweapon", |world, receiver, args| {
        let name = string(args, 0)?;
        let weapon = crate::script_player::weapon_named(
            &crate::frame::FrameWorld::from_world(world),
            &name,
        )?;
        heli(world, receiver)?.weapon = Some(weapon);
        Ok(Value::Undefined)
    });
    registry.register(Method, "setturrettargetvec", |world, receiver, args| {
        let point = vector(args, 0)?;
        aim_turret(world, receiver, Some(TurretAim::Point(point)))
    });
    registry.register(Method, "clearturrettarget", |world, receiver, _| {
        aim_turret(world, receiver, None)
    });
    registry.register(Method, "fireweapon", fire_weapon);
    registry.register(Method, "useby", use_by);
    // Vehicle turrets here traverse freely, so only the optional sight trace
    // from the muzzle can refuse a point.
    registry.register(
        Method,
        "vehicle_canturrettargetpoint",
        |world, receiver, args| {
            let Value::Object(id) = *receiver else {
                return Err("receiver is not a vehicle".into());
            };
            heli(world, receiver)?;
            let point = vector(args, 0)?;
            if optional(args, 1, int)?.unwrap_or(0) == 0 {
                return Ok(Value::Int(1));
            }
            let from = super::turrets::muzzle(world, id);
            let ignore = super::natives::engine::trace_ignore(world, args.get(2))
                .with(super::turrets::ignore_self(world, id));
            let outcome = super::natives::engine::entity_trace(
                world,
                from,
                point,
                crate::bullet_collision::MASK_SHOT,
                ignore,
            );
            Ok(Value::Int(
                matches!(outcome, crate::bullet_collision::TraceOutcome::Miss { .. }).into(),
            ))
        },
    );
    registry.register(Method, "vehicle_finishdamage", |world, receiver, args| {
        let Some((object, _)) = world.resource::<Runtime>().entity(receiver) else {
            return Err("receiver is not a vehicle".into());
        };
        let attacker = arg(args, 1)?.clone();
        let amount = int(args, 2)?;
        let flags = optional(args, 3, int)?.unwrap_or(0);
        let means = string(args, 4)?;
        let weapon = string(args, 5)?;
        let point = vector(args, 6)?;
        let dir = vector(args, 7)?;
        let part = optional(args, 11, string)?.unwrap_or_default();
        let hit = FinishedDamage {
            amount,
            flags,
            means: &means,
            weapon: &weapon,
            point,
            dir,
            part: &part,
        };
        finish_damage(world, object, attacker, &hit);
        Ok(Value::Undefined)
    });
}

struct FinishedDamage<'a> {
    amount: i32,
    flags: i32,
    means: &'a str,
    weapon: &'a str,
    point: [f32; 3],
    dir: [f32; 3],
    part: &'a str,
}

/// Takes the health and notifies `damage`, then `death` when it runs out.
fn finish_damage(world: &mut World, object: u64, attacker: Value, hit: &FinishedDamage<'_>) {
    let receiver = &Value::Object(object);
    let FinishedDamage {
        amount,
        flags,
        means,
        weapon,
        point,
        dir,
        part,
    } = *hit;
    let mut runtime = world.resource_mut::<Runtime>();
    let before = match runtime.object_field(object, "health") {
        Value::Int(health) => health,
        Value::Float(health) => health as i32,
        _ => 0,
    };
    let after = before.saturating_sub(amount);
    runtime.set_object_field(object, "health", Value::Int(after));
    let model = runtime.object_field(object, "model");
    drop(runtime);
    raise(
        world,
        receiver.clone(),
        "damage",
        vec![
            Value::Int(amount),
            attacker.clone(),
            Value::Vector(dir),
            Value::Vector(point),
            Value::string(means),
            model,
            Value::string(""),
            Value::string(part),
            Value::Int(flags),
            Value::string(weapon),
        ],
    );
    if before > 0 && after <= 0 {
        raise(world, receiver.clone(), "death", vec![attacker]);
    }
}

pub(crate) fn damage(
    world: &mut World,
    object: u64,
    hit: &super::entity_damage::EntityHit,
    attacker: Value,
    weapon: &str,
    part: &str,
) {
    let args = vec![
        Value::Undefined,
        attacker,
        Value::Int(hit.amount),
        Value::Int(hit.flags),
        Value::string(hit.means),
        Value::string(weapon),
        Value::Vector(hit.point),
        Value::Vector(hit.dir),
        Value::string("none"),
        Value::Int(0),
        Value::Int(0),
        Value::string(part),
    ];
    if super::players::single_player(world) {
        // SP has no vehicle damage callback: the engine applies it.
        let hit = FinishedDamage {
            amount: hit.amount,
            flags: hit.flags,
            means: hit.means,
            weapon,
            point: hit.point,
            dir: hit.dir,
            part,
        };
        finish_damage(world, object, args[1].clone(), &hit);
        return;
    }
    let now = super::players::now_ms(world);
    let _ = run_now(world, DAMAGE, Value::Object(object), args, now);
}

/// `IW4L_HELI_LOG=1`: once a second, each flying vehicle's position, goal, speed and gunner.
fn heli_log() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var("IW4L_HELI_LOG").is_ok_and(|v| v == "1"))
}

fn approach_angle(from: f32, to: f32, step: f32) -> f32 {
    let delta = math_iw4::angle_subtract(to, from);
    if delta.abs() <= step {
        to
    } else {
        from + step.copysign(delta)
    }
}

pub(crate) fn advance(world: &mut World) {
    {
        let mut runtime = world.resource_mut::<Runtime>();
        let ids: Vec<u64> = runtime.planes.keys().copied().collect();
        for id in ids {
            if !runtime.entities.contains_key(&id) {
                runtime.planes.remove(&id);
                continue;
            }
            let origin = vec_field(&mut runtime, id, "origin");
            let plane = runtime.planes.get_mut(&id).unwrap();
            plane.velocity = std::array::from_fn(|i| (origin[i] - plane.origin[i]) / TICK_S);
            plane.origin = origin;
        }
    }
    let ids: Vec<u64> = world
        .resource::<Runtime>()
        .vehicles
        .keys()
        .copied()
        .collect();
    let now = crate::level_time_ms(world.resource::<crate::step::StepRequest>().tick);
    for id in ids {
        if world
            .resource::<Runtime>()
            .vehicles
            .get(&id)
            .is_some_and(|v| v.drive.is_some())
            && world.resource::<Runtime>().entities.contains_key(&id)
        {
            for (note, args) in super::vehicle_drive::step(world, id) {
                raise(world, Value::Object(id), note, args);
            }
            continue;
        }
        let (gunner, weapon) = world
            .resource::<Runtime>()
            .vehicles
            .get(&id)
            .map_or((None, None), |heli| (heli.gunner, heli.weapon));
        let control = gunner
            .filter(|client| world.resource::<Runtime>().players.contains_key(client))
            .and_then(|client| gunner_input(world, client));
        let fire_ms = weapon
            .and_then(|weapon| crate::frame::FrameWorld::from_world(world).combat_facts_for(weapon))
            .map_or(100, |facts| facts.fire_time_ms.max(1));
        let mut runtime = world.resource_mut::<Runtime>();
        if !runtime.entities.contains_key(&id) {
            runtime.vehicles.remove(&id);
            continue;
        }
        sync_path_direction(&mut runtime, id);
        let origin = vec_field(&mut runtime, id, "origin");
        let angles = vec_field(&mut runtime, id, "angles");
        let look_at = runtime.vehicles[&id]
            .look_at
            .filter(|target| runtime.entities.contains_key(target));
        let look_at = look_at.map(|target| vec_field(&mut runtime, target, "origin"));
        let heli = runtime.vehicles.get_mut(&id).unwrap();
        let before = heli.speed;
        let mut notes = Vec::new();
        let mut next;
        match heli.goal.filter(|_| !(heli.arrived && heli.stop_at_goal)) {
            Some(goal) => {
                let to: [f32; 3] = std::array::from_fn(|i| goal[i] - origin[i]);
                let dist = to.iter().map(|v| v * v).sum::<f32>().sqrt();
                let wanted = if heli.stop_at_goal {
                    heli.max_speed.min((2.0 * heli.decel * dist).sqrt())
                } else {
                    heli.max_speed
                };
                heli.speed = if wanted > heli.speed {
                    (heli.speed + heli.accel * TICK_S).min(wanted)
                } else {
                    (heli.speed - heli.decel * TICK_S).max(wanted)
                };
                if dist > f32::EPSILON {
                    if heli.turning >= 1.0 {
                        heli.heading = to.map(|v| v / dist);
                    } else {
                        let desired = math_iw4::vect_to_angles(to);
                        let current = math_iw4::vect_to_angles(heli.heading);
                        let step = 180.0 * heli.turning * TICK_S;
                        heli.heading = math_iw4::angle_vectors([
                            approach_angle(current[0], desired[0], step),
                            approach_angle(current[1], desired[1], step),
                            0.0,
                        ])
                        .0;
                    }
                }
                let step = (heli.speed * TICK_S).min(dist);
                next = std::array::from_fn(|i| origin[i] + heli.heading[i] * step);
                let left = next
                    .iter()
                    .zip(goal)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                if heli.near_goal > 0.0 && !heli.near_notified && left <= heli.near_goal {
                    heli.near_notified = true;
                    notes.push("near_goal");
                }
                if !heli.arrived && left <= ARRIVED {
                    heli.arrived = true;
                    if heli.stop_at_goal {
                        next = goal;
                        heli.speed = 0.0;
                    }
                    notes.push("goal");
                }
            }
            None => {
                heli.speed = (heli.speed - heli.decel * TICK_S).max(0.0);
                next = std::array::from_fn(|i| origin[i] + heli.heading[i] * heli.speed * TICK_S);
                if heli.speed <= f32::EPSILON
                    && let Some(hover) = &mut heli.hover
                {
                    next = hover.advance(origin);
                }
            }
        }
        let desired_yaw = heli
            .target_yaw
            .or(heli.arrived.then_some(heli.goal_yaw).flatten())
            .or(look_at
                .map(|at| math_iw4::vect_to_angles([at[0] - next[0], at[1] - next[1], 0.0])[1]))
            .or(
                (heli.speed > 1.0 && heli.heading[..2].iter().any(|v| v.abs() > 0.01))
                    .then(|| math_iw4::vect_to_angles(heli.heading)[1]),
            )
            .unwrap_or(angles[1]);
        let yaw = approach_angle(angles[1], desired_yaw, heli.yaw_speed * TICK_S);
        let turn = math_iw4::angle_subtract(yaw, angles[1]) / TICK_S;
        let accel = (heli.speed - before) / TICK_S;
        let pitch = (accel / MPH)
            .clamp(-heli.max_pitch, heli.max_pitch)
            .clamp(-25.0, 25.0);
        let roll = (turn * 0.25)
            .clamp(-heli.max_roll, heli.max_roll)
            .clamp(-35.0, 35.0);
        match control {
            Some((eye, view, attack)) => {
                // Aim from the gunner's eye, carried with the vehicle this tick.
                heli.turret = Some(TurretAim::Point(std::array::from_fn(|i| {
                    eye[i] + next[i] - origin[i] + view[i] * GUNNER_RANGE
                })));
                heli.on_target = true;
                if attack && now >= heli.next_fire_ms {
                    heli.next_fire_ms = now + fire_ms;
                    notes.push("turret_fire");
                }
            }
            None if heli.gunner.is_some() => {
                heli.gunner = None;
                heli.turret = None;
            }
            None => {}
        }
        if heli.turret.is_some() && !heli.on_target {
            heli.on_target = true;
            notes.push("turret_on_target");
        }
        if heli_log() && now % 1000 < crate::MATCH_TICK_MS as i32 {
            diag::info!(
                Sim,
                "heli {id}: at {:.0} {:.0} {:.0} goal={:?} speed={:.0}/{:.0} gunner={:?} control={} notes={notes:?}",
                next[0],
                next[1],
                next[2],
                heli.goal.map(|g| g.map(|v| v as i32)),
                heli.speed,
                heli.max_speed,
                heli.gunner,
                control.is_some()
            );
        }
        heli.velocity = std::array::from_fn(|i| (next[i] - origin[i]) / TICK_S);
        let reached_node = (heli.path_running && heli.arrived)
            .then_some(heli.path_node)
            .flatten();
        let speed = heli.speed;
        if let Some(reached) = reached_node {
            let reverse = runtime.vehicles[&id].reverse;
            let successor =
                path_successor(&mut runtime, reached, reverse).and_then(|next| match next {
                    Some(next) => Ok(Some((next, node(&mut runtime, next)?))),
                    None => Ok(None),
                });
            match successor {
                Ok(Some((node, goal))) => path_goal(&mut runtime, id, node, goal),
                Ok(None) => {
                    let vehicle = runtime.vehicles.get_mut(&id).unwrap();
                    vehicle.path_running = false;
                    if vehicle.slot == SP_SLOT {
                        notes.push("reached_end_node");
                    }
                    notes.push("end_of_path");
                }
                Err(error) => {
                    runtime.vehicles.get_mut(&id).unwrap().path_running = false;
                    runtime.pending_notifies.push((
                        Value::Object(id),
                        "path_error".into(),
                        vec![Value::string(&error)],
                    ));
                }
            }
            runtime.pending_notifies.push((
                Value::Object(reached),
                "trigger".into(),
                vec![Value::Object(id)],
            ));
        }
        if std::env::var("IW4L_VEH_AUTODRIVE").is_ok_and(|v| v == "2")
            && now % 2000 == 0
            && runtime.vehicles[&id].slot == SP_SLOT
        {
            let goal = runtime.vehicles[&id].goal.unwrap_or([0.0; 3]);
            diag::info!(
                Sim,
                "vehicle: {id} at ({:.0} {:.0} {:.0}) goal ({:.0} {:.0} {:.0}) {:.0} mph",
                next[0],
                next[1],
                next[2],
                goal[0],
                goal[1],
                goal[2],
                speed / MPH
            );
        }
        let ground = runtime.vehicles[&id].ground && runtime.vehicles[&id].hover.is_none();
        runtime.set_object_field(id, "origin", Value::Vector(next));
        runtime.set_object_field(id, "angles", Value::Vector([pitch, yaw, roll]));
        runtime.set_object_field(id, "veh_speed", Value::Float(speed / MPH));
        drop(runtime);
        if ground {
            follow_ground(world, id, origin, next, yaw, roll);
        }
        for note in notes {
            raise(world, Value::Object(id), note, Vec::new());
        }
    }
}

/// Rise a ground vehicle can climb, and drop it can fall, in one tick.
const GROUND_STEP: f32 = 48.0;
const GROUND_FALL: f32 = 256.0;
const GROUND_MASK: u32 = 0x0080_0211;

/// A `vehicledriveto` vehicle rides the terrain between its goals: the goal
/// line only steers it; height and pitch come from the ground under it.
fn follow_ground(
    world: &mut World,
    id: u64,
    origin: [f32; 3],
    next: [f32; 3],
    yaw: f32,
    roll: f32,
) {
    let start = [next[0], next[1], next[2].max(origin[2]) + GROUND_STEP];
    let end = [next[0], next[1], next[2].min(origin[2]) - GROUND_FALL];
    let hit = super::natives::engine::trace(
        world,
        start,
        end,
        [-16.0, -16.0, 0.0],
        [16.0, 16.0, 16.0],
        GROUND_MASK,
    );
    if hit.startsolid != 0 || hit.fraction >= 1.0 || hit.normal[2] < 0.45 {
        return;
    }
    let forward = math_iw4::angle_vectors([0.0, yaw, 0.0]).0;
    let n = hit.normal;
    let dot = forward[0] * n[0] + forward[1] * n[1] + forward[2] * n[2];
    let along = [
        forward[0] - n[0] * dot,
        forward[1] - n[1] * dot,
        forward[2] - n[2] * dot,
    ];
    let pitch = math_iw4::vect_to_angles(along)[0];
    let mut runtime = world.resource_mut::<Runtime>();
    runtime.set_object_field(id, "origin", Value::Vector(hit.endpos));
    runtime.set_object_field(id, "angles", Value::Vector([pitch, yaw, roll]));
    if let Some(vehicle) = runtime.vehicles.get_mut(&id) {
        vehicle.velocity = std::array::from_fn(|i| (hit.endpos[i] - origin[i]) / TICK_S);
    }
}

pub(crate) fn compass_rows(world: &mut World) -> Vec<crate::CompassVehicle> {
    let runtime = world.resource::<Runtime>();
    let mut rows: Vec<_> = runtime
        .vehicles
        .iter()
        .filter_map(|(id, heli)| Some((*id, heli.owner?, heli.compass.clone()?)))
        .collect();
    rows.extend(
        runtime
            .planes
            .iter()
            .filter_map(|(id, plane)| Some((*id, plane.owner, plane.compass.clone()?))),
    );
    rows.sort_by_key(|(id, _, _)| *id);
    rows.into_iter()
        .filter_map(|(id, owner, (icons, size))| {
            if !world.resource::<Runtime>().live(&id) {
                return None;
            }
            let Value::Vector(origin) = super::players::entity_field(world, id, "origin") else {
                return None;
            };
            let yaw = match super::players::entity_field(world, id, "angles") {
                Value::Vector(v) => v[1],
                _ => 0.0,
            };
            let mut team = super::players::entity_field(world, id, "team");
            if team == Value::Undefined {
                team = super::players::load_field(world, owner, "sessionteam")
                    .unwrap_or(Value::Undefined);
            }
            let team = match team {
                Value::String(team) if &*team == "axis" => entity_iw4::TEAM_AXIS,
                Value::String(team) if &*team == "allies" => entity_iw4::TEAM_ALLIES,
                _ => entity_iw4::TEAM_FREE,
            };
            Some(crate::CompassVehicle {
                origin,
                yaw,
                owner,
                team,
                icons,
                size,
            })
        })
        .collect()
}

pub(crate) fn hud_targets(world: &World) -> Vec<crate::VehicleHudTarget> {
    let runtime = world.resource::<Runtime>();
    let mut rows: Vec<_> = runtime
        .vehicles
        .iter()
        .filter_map(|(id, vehicle)| {
            if !runtime.live(id) || vehicle.slot >= VEHICLE_SLOTS {
                return None;
            }
            let entity = runtime.entities.get(id)?;
            Some(crate::VehicleHudTarget {
                slot: vehicle.slot,
                entity: u16::try_from(
                    crate::frame::script_mover_by_id(world, entity.presence?)?
                        .state
                        .number,
                )
                .ok()?,
                model: entity.presence?,
                owner: crate::ClientId(vehicle.owner.unwrap_or(0)),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.slot);
    rows
}
