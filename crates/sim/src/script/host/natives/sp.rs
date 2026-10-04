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
    /// `Target_Set` entities (missile lock / HUD targets) and their offsets.
    lock_targets: BTreeMap<u64, [f32; 3]>,
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

/// Profile fields live in [`crate::SpProfile`], which the host loads from and saves to disk.
pub(crate) fn profile_value(world: &World, name: &str) -> Value {
    let key = name.to_ascii_lowercase();
    let digits = crate::SpProfile::DIGIT_FIELDS.contains(&key.as_str());
    match world.resource::<crate::SpProfile>().get(&key) {
        Some(text) if digits => Value::string(text),
        Some(text) => text
            .trim()
            .parse::<i32>()
            .map_or_else(|_| Value::string(text), Value::Int),
        None if digits => Value::string(&"0".repeat(PROFILE_DIGITS)),
        None if key == "autoaim" => Value::Int(1),
        None => Value::Int(0),
    }
}

pub(crate) fn set_profile_value(world: &mut World, name: &str, value: Value) {
    let text = match &value {
        Value::Float(value) => (*value as i32).to_string(),
        other => describe(other),
    };
    diag::info!(Sim, "spec ops: profile {name}={text}");
    world.resource_mut::<crate::SpProfile>().set(name, text);
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

/// Dvars the EOG summary menus read (`sp_eog_summary`, `coop_eog_summary`, notify popups).
pub(crate) fn is_eog_dvar(name: &str) -> bool {
    name.starts_with("ui_")
        || name.starts_with("player_")
        || matches!(name, "elapsed_mission_time" | "solo_play" | "coop")
}

/// `maps\_endmission::coop_eog_summary` fills dvars for the EOG menu, then opens it; the result
/// and the summary table the mission built also go to the log.
pub(crate) fn eog_summary(world: &mut World, menu: &str) {
    let dvars = world.resource::<Runtime>().dvars.clone();
    let get = |name: &str| dvars.get(name).cloned().unwrap_or_default();
    let outcome = match get("ui_mission_success").as_str() {
        "1" => "success",
        _ => "failed",
    };
    let fields = [
        "finished_time",
        "star_count",
        "targets_hit",
        "friendlies_hit",
    ]
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

/// Saved view-scale dvars reach the clients that draw them: `cg_fovScale` scales every
/// player's FOV, `cg_playerFovScale<N>` the FOV of player N (`_ac130` zooms the gunner per
/// weapon, `_cobrapilot` the pilot). The client multiplies both into its lens FOV.
fn view_fov_scale(world: &mut World, name: &str, value: Option<&Value>) {
    let targets: Vec<u32> = {
        let players = world.resource::<Runtime>().players.keys().copied();
        match name.strip_prefix("cg_playerfovscale") {
            Some(index) => match index.parse::<usize>() {
                Ok(index) => players.skip(index).take(1).collect(),
                Err(_) => return,
            },
            None if name == "cg_fovscale" => players.collect(),
            None => return,
        }
    };
    let scale = match value {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f32,
        Some(Value::String(text)) => text.trim().parse().unwrap_or(1.0),
        _ => 1.0,
    };
    let dvar = if name == "cg_fovscale" {
        "cg_fovscale"
    } else {
        "cg_playerfovscale"
    };
    for client in targets {
        diag::info!(Sim, "spec ops: client {client} {dvar} {scale:.3}");
        super::player::publish_client_dvar(world, client, dvar, format!("{scale}"));
    }
}

/// Logs the difficulty `_gameskill` applied to a player when it changes: the skill the scripts
/// read and the player fields actor hits and incoming damage use.
fn log_difficulty(world: &mut World, player: u64, skill: i32) {
    // Per player: two players alternate here every frame in co-op.
    static LAST: std::sync::Mutex<BTreeMap<u64, String>> = std::sync::Mutex::new(BTreeMap::new());
    let mut runtime = world.resource_mut::<Runtime>();
    let fields = [
        "attackeraccuracy",
        "damagemultiplier",
        "deathinvulnerabletime",
    ]
    .map(|name| format!("{name}={}", describe(&runtime.object_field(player, name))));
    let line = format!("gameskill={skill} {}", fields.join(" "));
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if last.get(&player) != Some(&line) {
        diag::info!(Sim, "spec ops: player {player} {line}");
        last.insert(player, line);
    }
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};
    // Lock-on targets: kept as a set so scripts can register and query them
    // (`_attack_heli` stops at an unbound `Target_Set`). No HUD reticle is drawn.
    registry.register(Function, "target_set", |world, _, args| {
        let target = entity_id(world, args.first().ok_or("target_set: no entity")?)?;
        let offset = match args.get(1) {
            Some(Value::Vector(v)) => *v,
            _ => [0.0; 3],
        };
        world
            .resource_mut::<Runtime>()
            .sp
            .lock_targets
            .insert(target, offset);
        Ok(Value::Undefined)
    });
    registry.register(Function, "target_remove", |world, _, args| {
        if let Ok(target) = entity_id(world, args.first().ok_or("target_remove: no entity")?) {
            world
                .resource_mut::<Runtime>()
                .sp
                .lock_targets
                .remove(&target);
        }
        Ok(Value::Undefined)
    });
    registry.register(Function, "target_istarget", |world, _, args| {
        let target = entity_id(world, args.first().ok_or("target_istarget: no entity")?)?;
        let set = world
            .resource::<Runtime>()
            .sp
            .lock_targets
            .contains_key(&target);
        Ok(Value::Int(set.into()))
    });
    registry.register(Function, "target_getarray", |world, _, _| {
        let live: Vec<Value> = {
            let runtime = world.resource::<Runtime>();
            runtime
                .sp
                .lock_targets
                .keys()
                .filter(|id| runtime.live(id))
                .map(|id| Value::Object(*id))
                .collect()
        };
        super::super::arrays::new_array(world, live)
    });
    registry.register(Function, "target_isincircle", |_, _, _| Ok(Value::Int(0)));
    registry.register(Function, "target_setturretaquire", |_, _, _| {
        Ok(Value::Undefined)
    });
    registry.register(Function, "setsaveddvar", |world, receiver, args| {
        let set_dvar = world
            .resource::<NativeRegistry>()
            .get(Function, "setdvar")
            .ok_or("setdvar is not bound")?;
        let result = set_dvar(world, receiver, args)?;
        let name = string(args, 0)?.to_ascii_lowercase();
        view_fov_scale(world, &name, args.get(1));
        Ok(result)
    });
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

    registry.register(Method, "getplayersetting", |world, receiver, args| {
        let name = string(args, 0)?.to_ascii_lowercase();
        match name.as_str() {
            "gameskill" => {
                let skill = world
                    .resource::<Runtime>()
                    .dvars
                    .get("g_gameskill")
                    .and_then(|text| text.trim().parse::<i32>().ok())
                    .unwrap_or(1);
                if let Value::Object(player) = receiver {
                    log_difficulty(world, *player, skill);
                }
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
    register_presentation(registry);
}

/// SP builtins with no simulation effect here: sound EQ/reverb, lighting and
/// screen effects, look-at text, saves, badplaces, glass, and vehicle physics
/// knobs on vehicles that are not simulated. Queries answer as for a level
/// with none of these.
fn register_presentation(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};
    for name in [
        "deactivateeq",
        "seteqlerp",
        "setreverb",
        "eqoff",
        "eqon",
        "seteq",
        "seteqbands",
        "deactivatereverb",
        "setsoundblend",
        "stopsoundchannel",
        "startusingheroonlylighting",
        "startusinglessfrequentlighting",
        "setlightcolor",
        "setwatersheeting",
        "painvisionon",
        "painvisionoff",
        "setvehiclelookattext",
        "setlookattext",
        "playerclearstreamorigin",
        "playersetstreamorigin",
        "laserforceon",
        "laserforceoff",
        "laseraltviewon",
        "laseraltviewoff",
        "enableaimassist",
        "disableaimassist",
        "dontinterpolate",
        "playrumblelooponentity",
        "setwaypointedgestyle_rotatingicon",
        "setwaypointiconoffscreenonly",
        "setplayerintelfound",
        "joltbody",
        "vibrate",
        "setairresistance",
        "restoredefaultdroppitch",
        "hidepart_allinstances",
        "lerpviewangleclamp",
        "vehicle_turnengineoff",
        "setfriendlychain",
        "startragdollfromimpact",
        "resumespeed",
        "setwaitspeed",
        "vehphys_crash",
        "dontcastshadows",
        "hideonclient",
        "showonclient",
        "setswitchnode",
        "setproneanimnodes",
        "updateprone",
        "enterprone",
        "exitprone",
        "pushplayer",
        // The breach charge plays as the viewmodel raise; the rig carries the player.
        "enablebreaching",
        "disablebreaching",
        "allowcrouch",
        "allowprone",
        "allowstand",
        "vehphys_disablecrashing",
        "vehphys_enablecrashing",
        "setsuppressiontime",
        "setplayerspread",
        "setaispread",
        "setturretignoregoals",
        "hideallparts",
        // A dog's knock-down: pitch to the ground, the player's HUD and hands.
        "clearpitchorient",
        "hidehud",
        "showhud",
        "hideviewmodel",
        "showviewmodel",
        "allowlean",
        // A dead vehicle's slot; vehicles here hold none.
        "freevehicle",
    ] {
        registry.register_missing(Method, name, |_, _, _| Ok(Value::Undefined));
    }
    for name in [
        "soundsettimescalefactor",
        "setsunlight",
        "setblur",
        "destroyglass",
        "objective_additionalposition",
        "commitsave",
        "badplace_brush",
        "badplace_cylinder",
        "badplace_arc",
        "badplace_delete",
        "physicsjitter",
        "cinematicingamesync",
        "target_setjavelinonly",
        "precachenightvisioncodeassets",
        "setculldist",
        "sethalfresparticles",
    ] {
        registry.register_missing(Function, name, |_, _, _| Ok(Value::Undefined));
    }
    for name in [
        "iswaitingonsound",
        "vehicle_isphysveh",
        "isinscriptedstate",
        "getplayerintelisfound",
    ] {
        registry.register_missing(Method, name, |_, _, _| Ok(Value::Int(0)));
    }
    for name in ["issaverecentlyloaded", "commitwouldbevalid"] {
        registry.register_missing(Function, name, |_, _, _| Ok(Value::Int(0)));
    }
    registry.register_missing(Function, "issavesuccessful", |_, _, _| Ok(Value::Int(1)));
    for name in ["savegame", "savegamenocommit"] {
        registry.register_missing(Function, name, |_, _, _| Ok(Value::Int(0)));
    }
    registry.register_missing(Function, "getglass", |_, _, _| Ok(Value::Undefined));
    registry.register_missing(Function, "getglassarray", |world, _, _| {
        super::super::arrays::new_array(world, Vec::new())
    });
    registry.register_missing(Method, "getwheelsurface", |_, _, _| {
        Ok(Value::string("dirt"))
    });
    registry.register_missing(Method, "getlightcolor", |_, _, _| {
        Ok(Value::Vector([1.0; 3]))
    });
    registry.register_missing(Method, "getplayerviewheight", |_, _, _| {
        Ok(Value::Float(60.0))
    });
    registry.register_missing(Method, "getnormalizedmovement", |_, _, _| {
        Ok(Value::Vector([0.0; 3]))
    });
    registry.register_missing(Method, "getcentroid", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        Ok(world.resource_mut::<Runtime>().object_field(id, "origin"))
    });
    // Actors do not operate turrets yet; `useturret` only records the turret
    // so `getturret` stops `_vehicle_aianim`/`_mgturret` re-mounting each second.
    registry.register_missing(Method, "useturret", |world, receiver, args| {
        let id = entity_id(world, receiver)?;
        let turret = super::super::args::arg(args, 0)?.clone();
        world
            .resource_mut::<Runtime>()
            .set_object_field(id, "code_turret", turret);
        Ok(Value::Undefined)
    });
    registry.register_missing(Method, "getturret", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        let turret = world
            .resource_mut::<Runtime>()
            .object_field(id, "code_turret");
        Ok(match turret {
            Value::Object(object) if world.resource::<Runtime>().live(&object) => turret,
            _ => Value::Undefined,
        })
    });
    registry.register_missing(Method, "stopuseturret", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        world
            .resource_mut::<Runtime>()
            .set_object_field(id, "code_turret", Value::Undefined);
        Ok(Value::Undefined)
    });
    registry.register_missing(Function, "getmapsunlight", |_, _, _| {
        Ok(Value::Vector([1.0; 3]))
    });
    // `linktoblendtotag( ent, tag, blendtime )`: links at once, without the blend.
    registry.register_missing(Method, "linktoblendtotag", |world, receiver, args| {
        let link = world
            .resource::<NativeRegistry>()
            .get(Method, "linkto")
            .ok_or("linkto is not bound")?;
        link(world, receiver, &args[..args.len().min(2)])
    });
    registry.register_missing(Function, "getfreeaicount", |world, _, _| {
        let used = world.resource::<crate::actor::ActorPool>().actors.len();
        Ok(Value::Int(
            crate::actor::MAX_ACTORS.saturating_sub(used) as i32
        ))
    });
    registry.register_missing(Function, "isenemyteam", |_, _, args| {
        let a = string(args, 0)?;
        let b = string(args, 1)?;
        let side = |team: &str| matches!(team, "allies" | "axis" | "team3");
        Ok(Value::Int((side(&a) && side(&b) && a != b).into()))
    });
}
