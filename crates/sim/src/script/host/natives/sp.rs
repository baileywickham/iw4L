//! Single-player builtins that need no actor system: saved dvars, threat-bias
//! bookkeeping, drones, profile fields and mission results.

use super::super::args::{float, kind, string};
use super::engine::entity_id;
use crate::script::host::entities::EntityKind;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use bevy_ecs::prelude::World;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub(crate) struct SpState {
    threat_groups: BTreeSet<String>,
    threat_bias: BTreeMap<(String, String), i32>,
    entity_groups: BTreeMap<u64, String>,
    profile: BTreeMap<String, Value>,
}

/// `missionSOHighestDifficulty` and `missionHighestDifficulty` hold one digit per level.
const PROFILE_DIGITS: usize = 64;

/// The bias `attacker`'s threat-bias group holds against `target`'s
/// (`setthreatbias( targetgroup, attackergroup, bias )`); `i32::MIN` means ignore.
pub(crate) fn threat_bias(world: &World, attacker: u64, target: u64) -> i32 {
    let sp = &world.resource::<Runtime>().sp;
    let (Some(a), Some(t)) = (
        sp.entity_groups.get(&attacker),
        sp.entity_groups.get(&target),
    ) else {
        return 0;
    };
    sp.threat_bias
        .get(&(t.clone(), a.clone()))
        .copied()
        .unwrap_or(0)
}

pub(crate) fn profile_value(world: &World, name: &str) -> Value {
    let key = name.to_ascii_lowercase();
    if let Some(value) = world.resource::<Runtime>().sp.profile.get(&key) {
        return value.clone();
    }
    match key.as_str() {
        "missionsohighestdifficulty" | "missionhighestdifficulty" => {
            Value::string(&"0".repeat(PROFILE_DIGITS))
        }
        "autoaim" => Value::Int(1),
        _ => Value::Int(0),
    }
}

pub(crate) fn set_profile_value(world: &mut World, name: &str, value: Value) {
    diag::info!(Sim, "spec ops: profile {name}={}", describe(&value));
    world
        .resource_mut::<Runtime>()
        .sp
        .profile
        .insert(name.to_ascii_lowercase(), value);
}

fn describe(value: &Value) -> String {
    if let Value::String(text) = value {
        return text.to_string();
    }
    crate::script::runtime::to_text(value).unwrap_or_else(|| kind(value).to_owned())
}

fn classname_team(classname: &str) -> &'static str {
    if classname.contains("_ally_") {
        "allies"
    } else if classname.contains("_enemy_") || classname.contains("_axis_") {
        "axis"
    } else {
        "neutral"
    }
}

fn level_field(world: &mut World, name: &str) -> String {
    let mut runtime = world.resource_mut::<Runtime>();
    let value = runtime.object_field(0, name);
    describe(&value)
}

fn mission_result(world: &mut World, outcome: &str) {
    let fields = [
        "script",
        "challenge_start_time",
        "challenge_end_time",
        "finished_time",
        "targets_hit",
        "friendlies_hit",
        "star_count",
    ]
    .map(|name| format!("{name}={}", level_field(world, name)));
    diag::info!(Sim, "spec ops: mission {outcome} {}", fields.join(" "));
}

/// `maps\_endmission::coop_eog_summary` fills dvars for the EOG menu, then opens it; the menu is not
/// drawn yet, so the result and the summary table the mission built go to the log.
pub(crate) fn eog_summary(world: &mut World, menu: &str) {
    let dvars = world.resource::<Runtime>().dvars.clone();
    let get = |name: &str| dvars.get(name).cloned().unwrap_or_default();
    let outcome = match get("ui_mission_success").as_str() {
        "1" => "success",
        _ => "failed",
    };
    let fields = ["finished_time", "star_count", "targets_hit", "friendlies_hit"]
        .map(|name| format!("{name}={}", level_field(world, name)));
    diag::info!(
        Sim,
        "spec ops: mission {outcome} map={} time={} {} menu={menu}",
        level_field(world, "script"),
        get("elapsed_mission_time"),
        fields.join(" ")
    );
    for row in 1..=6 {
        let cells = [1, 2].map(|col| get(&format!("ui_eog_r{row}c{col}_player1")));
        if cells.iter().any(|cell| !cell.is_empty()) {
            diag::info!(Sim, "spec ops: eog row {row}: {} | {}", cells[0], cells[1]);
        }
    }
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};
    if let Some(set_dvar) = registry.get(Function, "setdvar") {
        registry.register(Function, "setsaveddvar", set_dvar);
    }
    // SP `notifyOnCommand( notify, command )` binds the command for every player.
    registry.register(Function, "notifyoncommand", |world, _, args| {
        let bind = world
            .resource::<NativeRegistry>()
            .get(Method, "notifyonplayercommand")
            .ok_or("notifyonplayercommand is not bound")?;
        let players: Vec<u64> = world
            .resource::<Runtime>()
            .players
            .values()
            .map(|slot| slot.object)
            .collect();
        for player in players {
            bind(world, &Value::Object(player), args)?;
        }
        Ok(Value::Undefined)
    });
    registry.register(Function, "squared", |_, _, args| {
        let value = float(args, 0)?;
        Ok(Value::Float(value * value))
    });

    registry.register(Function, "createthreatbiasgroup", |world, _, args| {
        let name = string(args, 0)?.to_ascii_lowercase();
        world
            .resource_mut::<Runtime>()
            .sp
            .threat_groups
            .insert(name);
        Ok(Value::Undefined)
    });
    registry.register(Function, "threatbiasgroupexists", |world, _, args| {
        let name = string(args, 0)?.to_ascii_lowercase();
        let exists = world.resource::<Runtime>().sp.threat_groups.contains(&name);
        Ok(Value::Int(exists.into()))
    });
    registry.register(Function, "setthreatbias", |world, _, args| {
        let a = string(args, 0)?.to_ascii_lowercase();
        let b = string(args, 1)?.to_ascii_lowercase();
        let bias = super::super::args::int(args, 2)?;
        world
            .resource_mut::<Runtime>()
            .sp
            .threat_bias
            .insert((a, b), bias);
        Ok(Value::Undefined)
    });
    registry.register(Function, "setignoremegroup", |world, _, args| {
        let a = string(args, 0)?.to_ascii_lowercase();
        let b = string(args, 1)?.to_ascii_lowercase();
        world
            .resource_mut::<Runtime>()
            .sp
            .threat_bias
            .insert((a, b), i32::MIN);
        Ok(Value::Undefined)
    });
    registry.register(Function, "getthreatbias", |world, _, args| {
        let a = string(args, 0)?.to_ascii_lowercase();
        let b = string(args, 1)?.to_ascii_lowercase();
        let bias = world
            .resource::<Runtime>()
            .sp
            .threat_bias
            .get(&(a, b))
            .copied();
        Ok(Value::Int(bias.unwrap_or(0)))
    });
    registry.register(Method, "setthreatbiasgroup", |world, receiver, args| {
        let id = entity_id(world, receiver)?;
        let name = match args.first() {
            None | Some(Value::Undefined) => None,
            Some(_) => Some(string(args, 0)?.to_ascii_lowercase()),
        };
        let groups = &mut world.resource_mut::<Runtime>().sp.entity_groups;
        match name {
            Some(name) => groups.insert(id, name),
            None => groups.remove(&id),
        };
        Ok(Value::Undefined)
    });
    registry.register(Method, "getthreatbiasgroup", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        Ok(world
            .resource::<Runtime>()
            .sp
            .entity_groups
            .get(&id)
            .map_or(Value::Undefined, |name| Value::string(name)))
    });

    // A drone is a script model standing where its spawner is; without aitypes it has no
    // character model and carries no weapon.
    registry.register(Method, "spawndrone", |world, receiver, _| {
        let spawner_id = entity_id(world, receiver)?;
        let mut runtime = world.resource_mut::<Runtime>();
        let classname = runtime.entities[&spawner_id].classname.to_string();
        let origin = runtime.object_field(spawner_id, "origin");
        let angles = runtime.object_field(spawner_id, "angles");
        let id = runtime.create_entity(EntityKind::Spawned, "script_model")?;
        runtime.set_object_field(id, "origin", origin);
        runtime.set_object_field(id, "angles", angles);
        runtime.set_object_field(id, "team", Value::string(classname_team(&classname)));
        runtime.set_object_field(id, "weapon", Value::string("none"));
        Ok(Value::Object(id))
    });

    registry.register(Method, "getplayersetting", |world, _, args| {
        let name = string(args, 0)?.to_ascii_lowercase();
        match name.as_str() {
            "gameskill" => {
                let skill = world
                    .resource::<Runtime>()
                    .dvars
                    .get("g_gameskill")
                    .and_then(|text| text.trim().parse::<i32>().ok())
                    .unwrap_or(1);
                Ok(Value::Int(skill))
            }
            _ => Ok(Value::Int(0)),
        }
    });
    registry.register(Function, "getcommandfromkey", |_, _, _| {
        Ok(Value::string(""))
    });
    registry.register(Function, "weaponhasthermalscope", |_, _, args| {
        let name = string(args, 0)?;
        Ok(Value::Int(name.contains("thermal").into()))
    });

    registry.register(Function, "missionsuccess", |world, _, _| {
        mission_result(world, "success");
        Ok(Value::Undefined)
    });
    registry.register(Function, "missionfailed", |world, _, _| {
        mission_result(world, "failed");
        Ok(Value::Undefined)
    });
    for name in ["updategamerprofile", "updategamerprofileall"] {
        registry.register(Function, name, |_, _, _| Ok(Value::Undefined));
    }
    for name in ["uploadscore", "uploadtime"] {
        registry.register(Method, name, |_, _, _| Ok(Value::Undefined));
    }
}
