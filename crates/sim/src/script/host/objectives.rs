use super::args::{arg, int, string, vector};
use crate::frame::FrameWorld;
use crate::script::runtime::type_name;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use crate::{CompassObjective, ObjectiveMatch, ObjectiveMessage, ObjectiveState, ScriptEffect};
use bevy_ecs::prelude::World;
use gamemode_iw4::Team;

const MAX_OBJECTIVES: i32 = 32;
const ENGINE_SERVER_INFO: &[&str] = &[
    "ui_bomb_timer",
    "mapname",
    "g_gametype",
    "r_lightgridenabletweaks",
    "r_lightgridintensity",
    "r_lightgridcontrast",
];

#[derive(Clone, Debug, Default)]
pub(crate) struct ScriptObjective {
    state: ObjectiveState,
    origin: [f32; 3],
    entity: Option<Value>,
    team: Team,
    icon: String,
    text: String,
    text_args: Vec<String>,
    message: ObjectiveMessage,
    message_ms: i32,
}

impl ScriptObjective {
    pub(crate) fn entity(&self) -> Option<&Value> {
        self.entity.as_ref()
    }
}

fn index(args: &[Value]) -> Result<u8, String> {
    let index = int(args, 0)?;
    if !(0..MAX_OBJECTIVES).contains(&index) {
        return Err(format!("index {index} is an illegal objective index"));
    }
    Ok(index as u8)
}

fn state(args: &[Value], at: usize) -> Result<ObjectiveState, String> {
    let name = string(args, at)?;
    ObjectiveState::from_script(&name).ok_or_else(|| format!("Illegal objective state \"{name}\""))
}

fn text_arg(value: &Value) -> String {
    match value {
        Value::Int(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::String(v) => v.to_string(),
        Value::LocalizedString(v) => v.to_string(),
        _ => String::new(),
    }
}

/// SP prints an objective's text on the HUD (the typewriter line) when it is
/// given, updated, completed or failed, unless the `_nomessage` form was used.
fn print_message(world: &mut World, index: u8, message: ObjectiveMessage) {
    if !super::players::single_player(world) {
        return;
    }
    let now = crate::level_time_ms(world.resource::<crate::step::StepRequest>().tick);
    let mut row = objective(world, index);
    if row.text.is_empty() {
        return;
    }
    row.message = message;
    row.message_ms = now;
}

fn state_message(state: ObjectiveState) -> Option<ObjectiveMessage> {
    match state {
        ObjectiveState::Active | ObjectiveState::Current => Some(ObjectiveMessage::Updated),
        ObjectiveState::Done => Some(ObjectiveMessage::Completed),
        ObjectiveState::Failed => Some(ObjectiveMessage::Failed),
        _ => None,
    }
}

fn set_text(world: &mut World, index: u8, args: &[Value]) {
    let Some(Value::LocalizedString(text)) = args.get(1) else {
        return;
    };
    let mut row = objective(world, index);
    row.text = text.to_string();
    row.text_args = args[2..].iter().map(text_arg).collect();
}

fn objective(world: &mut World, index: u8) -> bevy_ecs::world::Mut<'_, ScriptObjective> {
    world
        .resource_mut::<Runtime>()
        .map_unchanged(|runtime| runtime.engine.objectives.entry(index).or_default())
}

fn place(world: &mut World, index: u8, target: &Value) -> Result<(), String> {
    match target {
        Value::Vector(origin) => {
            let mut row = objective(world, index);
            row.origin = *origin;
            row.entity = None;
        }
        Value::Object(_) => objective(world, index).entity = Some(target.clone()),
        other => return Err(format!("objective position cannot be {}", type_name(other))),
    }
    Ok(())
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::Function;
    registry.register(Function, "objective_add", |world, _, args| {
        let index = index(args)?;
        let state = state(args, 1)?;
        *objective(world, index) = ScriptObjective {
            state,
            ..Default::default()
        };
        // SP: objective_add( index, state, text, position ); MP: ( index, state, position, icon ).
        if let Some(Value::LocalizedString(text)) = args.get(2) {
            diag::info!(Sim, "objective {index}: {} \"{text}\"", string(args, 1)?);
            objective(world, index).text = text.to_string();
            if let Some(target) = args.get(3) {
                place(world, index, target)?;
            }
            if let Some(message) = state_message(state) {
                print_message(world, index, message);
            }
            return Ok(Value::Undefined);
        }
        if let Some(target) = args.get(2) {
            place(world, index, target)?;
        }
        if args.len() > 3 {
            objective(world, index).icon = string(args, 3)?;
        }
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_string", |world, _, args| {
        let index = index(args)?;
        set_text(world, index, args);
        print_message(world, index, ObjectiveMessage::Updated);
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_string_nomessage", |world, _, args| {
        let index = index(args)?;
        set_text(world, index, args);
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_current", |world, _, args| {
        for arg in args {
            let Value::Int(index) = arg else { continue };
            let Ok(index) = u8::try_from(*index) else {
                continue;
            };
            if world
                .resource::<Runtime>()
                .engine
                .objectives
                .contains_key(&index)
            {
                print_message(world, index, ObjectiveMessage::Updated);
            }
        }
        Ok(Value::Undefined)
    });
    for name in [
        "objective_current_nomessage",
        "objective_additionalcurrent",
        "objective_ring",
        "objective_setpointertextoverride",
    ] {
        registry.register(Function, name, |_, _, _| Ok(Value::Undefined));
    }
    registry.register(Function, "objective_state_nomessage", |world, _, args| {
        let index = index(args)?;
        objective(world, index).state = state(args, 1)?;
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_delete", |world, _, args| {
        let index = index(args)?;
        world
            .resource_mut::<Runtime>()
            .engine
            .objectives
            .remove(&index);
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_state", |world, _, args| {
        let index = index(args)?;
        if super::players::single_player(world) {
            diag::info!(Sim, "objective {index}: {}", string(args, 1)?);
        }
        let state = state(args, 1)?;
        let changed = std::mem::replace(&mut objective(world, index).state, state) != state;
        if changed && let Some(message) = state_message(state) {
            print_message(world, index, message);
        }
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_icon", |world, _, args| {
        let index = index(args)?;
        objective(world, index).icon = string(args, 1)?;
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_position", |world, _, args| {
        let index = index(args)?;
        let origin = vector(args, 1)?;
        place(world, index, &Value::Vector(origin))?;
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_onentity", |world, _, args| {
        let index = index(args)?;
        let target = arg(args, 1)?.clone();
        if !matches!(target, Value::Object(_)) {
            return Err(format!("{} is not an entity", type_name(&target)));
        }
        place(world, index, &target)?;
        Ok(Value::Undefined)
    });
    registry.register(Function, "objective_team", |world, _, args| {
        let index = index(args)?;
        let team = match string(args, 1)?.as_str() {
            "axis" => Team::Axis,
            "allies" => Team::Allies,
            "none" | "free" | "neutral" => Team::Free,
            other => return Err(format!("'{other}' is an illegal team string")),
        };
        objective(world, index).team = team;
        Ok(Value::Undefined)
    });
}

pub(crate) fn publish(world: &mut World) {
    let vehicles = super::vehicles::compass_rows(world);
    let vehicle_targets = super::vehicles::hud_targets(world);
    let rows: Vec<(u8, ScriptObjective)> = world
        .resource::<Runtime>()
        .engine
        .objectives
        .iter()
        .map(|(index, row)| (*index, row.clone()))
        .collect();
    let mut compass = Vec::with_capacity(rows.len());
    for (index, row) in rows {
        let origin = match &row.entity {
            Some(Value::Object(id)) if world.resource::<Runtime>().live(id) => {
                match super::players::entity_field(world, *id, "origin") {
                    Value::Vector(origin) => origin,
                    _ => row.origin,
                }
            }
            _ => row.origin,
        };
        compass.push(CompassObjective {
            index,
            state: row.state,
            origin,
            team: row.team,
            icon: row.icon,
            text: row.text,
            text_args: row.text_args,
            message: row.message,
            message_ms: row.message_ms,
        });
    }
    let runtime = world.resource::<Runtime>();
    let score = |team: &str| runtime.engine.team_scores.get(team).copied().unwrap_or(0);
    let scores = [0, score("axis"), score("allies")];
    let engine = ENGINE_SERVER_INFO
        .iter()
        .filter(|name| !runtime.server_info.contains(**name))
        .filter_map(|name| Some((name.to_string(), runtime.dvars.get(*name)?.clone())));
    let server_info = runtime
        .server_info
        .iter()
        .map(|name| {
            let value = runtime.dvars.get(name).cloned().unwrap_or_default();
            (name.clone(), value)
        })
        .chain(engine)
        .chain(
            runtime
                .dvars
                .iter()
                .filter(|(name, _)| {
                    crate::is_postfx_dvar(name) && !runtime.server_info.contains(*name)
                })
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .collect();
    let game_end_time = runtime.engine.game_end_time;
    let slow_motion = runtime.engine.slow_motion;
    let ambient = runtime
        .program
        .is_some()
        .then(|| runtime.engine.ambient.clone().unwrap_or_default());
    let ac130_ambient = runtime
        .program
        .is_some()
        .then(|| runtime.engine.ac130_ambient.clone().unwrap_or_default());
    let rumble_aliases = runtime
        .precached
        .iter()
        .filter(|((kind, _), _)| *kind == "rumble")
        .map(|((_, name), index)| (*index, name.clone()))
        .collect();
    let scripted_effects = runtime.program.is_some();
    let naked_vision = runtime.engine.naked_vision.clone();
    let thermal_vision = runtime.engine.thermal_vision.clone();
    let thermal_body_material = runtime.engine.thermal_body_material.clone();
    let missile_vision = runtime.engine.missile_vision.clone();
    let night_vision = runtime.engine.night_vision.clone();
    let pain_vision = runtime.engine.pain_vision.clone();
    let fog = runtime.engine.fog;
    let now = crate::level_time_ms(world.resource::<crate::step::StepRequest>().tick);
    let rows: Vec<(u64, super::entities::PersistentFx)> = runtime
        .engine
        .effects
        .iter()
        .filter(|(id, _)| runtime.entities.contains_key(id))
        .map(|(id, fx)| (*id, fx.clone()))
        .collect();
    let mut runtime = world.resource_mut::<Runtime>();
    runtime
        .engine
        .effects
        .retain(|id, _| rows.iter().any(|(row, _)| row == id));
    runtime.engine.earthquakes.retain(|quake| quake.active(now));
    let earthquakes = runtime.engine.earthquakes.clone();
    let enemy_actors = enemy_actors(world);
    let mut frame = FrameWorld::from_world(world);
    let effects = rows
        .into_iter()
        .map(|(id, fx)| ScriptEffect {
            id: id as u32,
            effect: frame.effect_name_index(&fx.name),
            origin: fx.origin,
            forward: fx.forward,
            up: fx.up,
            start_ms: fx.start_ms,
            repeat_ms: fx.repeat_ms,
            cull_distance: fx.cull_distance,
        })
        .collect();
    frame.objectives = ObjectiveMatch {
        scores,
        compass,
        vehicles,
        vehicle_targets,
        server_info,
        game_end_time,
        slow_motion,
        ambient,
        ac130_ambient,
        rumble_aliases,
        scripted_effects,
        effects,
        fog,
        earthquakes,
        naked_vision,
        thermal_vision,
        thermal_body_material,
        missile_vision,
        night_vision,
        pain_vision,
        enemy_actors,
    };
}

/// Entity numbers of live actors hostile to the players: SP draws their weapon
/// fire on the compass as enemy pings.
fn enemy_actors(world: &World) -> Vec<u16> {
    let runtime = world.resource::<Runtime>();
    let Some(pool) = world.get_resource::<crate::actor::ActorPool>() else {
        return Vec::new();
    };
    let mut out: Vec<u16> = pool
        .actors
        .values()
        .filter(|a| a.dying.is_none() && super::actor_combat::hostile(&a.team, "allies"))
        .filter_map(|a| u16::try_from(runtime.entities.get(&a.object)?.number).ok())
        .collect();
    out.sort_unstable();
    out
}
