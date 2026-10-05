//! The single-player player's engine side: damage without MP callbacks, the death and
//! health shields, invulnerability and `kill`, and the last-stand pose `_coop` asks for
//! through `self.laststand`. Spec Ops co-op (down, revive, bleed out) is built on these.

use super::super::args::int;
use super::super::players::player_object;
use super::player::player;
use crate::frame::FrameWorld;
use crate::script::runtime::raise;
use crate::script::{Namespace, NativeRegistry, Runtime, Value};
use crate::script_player::{self, Hit};
use crate::world::ClientId;
use bevy_ecs::prelude::World;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpShields {
    /// Lethal damage leaves 1 health and notifies `deathshield` instead of killing.
    pub death: bool,
    pub health: bool,
    pub invulnerable: bool,
}

fn shields(world: &World, client: u32) -> SpShields {
    world
        .resource::<Runtime>()
        .players
        .get(&client)
        .map_or_else(SpShields::default, |slot| slot.sp_shields)
}

fn set_shields(world: &mut World, client: u32, change: impl FnOnce(&mut SpShields)) {
    if let Some(slot) = world.resource_mut::<Runtime>().players.get_mut(&client) {
        change(&mut slot.sp_shields);
    }
}

/// SP `G_Damage` on a player: no script callback, the engine takes the health,
/// notifies `damage`, then either `deathshield` (co-op down) or death.
pub(crate) fn damage(world: &mut World, tick: crate::Tick, hit: &Hit) {
    let client = hit.victim.0;
    let victim = player_object(world, client);
    if victim == Value::Undefined || hit.amount <= 0 {
        return;
    }
    let guard = shields(world, client);
    if guard.invulnerable {
        return;
    }
    let attacker = match super::super::players::attacker_object(world, hit.attacker) {
        Value::Undefined => world_entity(world),
        attacker => attacker,
    };
    let Value::Object(object) = victim else {
        return;
    };
    // `setcandamage( false )`: a dog's knock-down holds the player out of the fight.
    if !world.resource::<Runtime>().entities[&object].accepts_damage(hit.flags) {
        return;
    }
    let amount = super::super::players::sp_damage_amount(world, tick, object, hit);
    if amount <= 0 {
        return;
    }
    let weapon = script_player::weapon_name(&FrameWorld::from_world(world), hit.weapon);
    let (health, shielded) = {
        let mut frame = FrameWorld::from_world(world);
        if !frame
            .client_meta(hit.victim)
            .is_some_and(|m| m.lifecycle == crate::ClientLifecycle::Alive)
        {
            return;
        }
        let Some(ps) = frame.player_mut(hit.victim) else {
            return;
        };
        movement_iw4::update_damage_timer(ps, amount, Some(hit.dir));
        // `P_DamageFeedback`: the HUD's hit_direction arcs and the view kick read these.
        if hit.dir == [0.0; 3] {
            ps.damage_yaw = 255;
            ps.damage_pitch = 255;
        } else {
            let angles = math_iw4::vect_to_angles(hit.dir);
            ps.damage_pitch = (angles[0] / 360.0 * 256.0) as u32 & 0xff;
            ps.damage_yaw = (angles[1] / 360.0 * 256.0) as u32 & 0xff;
        }
        ps.damage_count = ps.damage_count.saturating_add(1);
        ps.damage_event = ps.damage_event.wrapping_add(1);
        let after = ps.health - amount;
        let shielded = after <= 0 && guard.death;
        ps.health = if shielded { 1 } else { after.max(0) };
        (ps.health, shielded)
    };
    let part = weapon_iw4::HITLOC_NAMES
        .get(usize::from(hit.hitloc))
        .copied()
        .unwrap_or("none");
    let args = vec![
        Value::Int(amount),
        attacker.clone(),
        Value::Vector(hit.dir),
        Value::Vector(hit.point),
        Value::string(hit.means),
        Value::string(""),
        Value::string(""),
        Value::string(part),
        Value::Int(hit.flags),
        Value::String(weapon.into()),
    ];
    diag::info!(
        Sim,
        "spec ops: client={client} damaged {amount} (raw {}) means={} health={health}{} by {}",
        hit.amount,
        hit.means,
        if shielded { " (death shield)" } else { "" },
        super::super::players::attacker_label(world, hit.attacker)
    );
    raise(world, victim.clone(), "damage", args.clone());
    if shielded {
        raise(world, victim, "deathshield", args);
    } else if health <= 0 {
        kill(world, tick, client, hit.attacker.and_then(crate::Attacker::client), attacker);
    }
}

fn world_entity(world: &World) -> Value {
    world
        .resource::<Runtime>()
        .engine
        .world
        .map_or(Value::Undefined, Value::Object)
}

fn kill(world: &mut World, tick: crate::Tick, client: u32, by: Option<ClientId>, attacker: Value) {
    let id = ClientId(client);
    {
        let mut frame = FrameWorld::from_world(world);
        if !frame
            .client_meta(id)
            .is_some_and(|m| m.lifecycle == crate::ClientLifecycle::Alive)
        {
            return;
        }
        set_last_stand(&mut frame, id, false);
        script_player::kill(&mut frame, tick, id, by, None);
    }
    diag::info!(Sim, "spec ops: client={client} killed");
    let victim = player_object(world, client);
    raise(world, victim, "death", vec![attacker]);
}

/// `_coop` marks a downed player with `self.laststand`; the engine answers with the
/// last-stand pose: low view, crawl speed, pistol only.
pub(crate) fn set_last_stand(frame: &mut FrameWorld, id: ClientId, on: bool) {
    let Some(ps) = frame.player_mut(id) else {
        return;
    };
    let flag = playerstate_iw4::pm_flags::LAST_STAND;
    if on == (ps.pm_flags & flag != 0) {
        return;
    }
    if on {
        ps.pm_type = playerstate_iw4::PM_TYPE_LAST_STAND;
        ps.view_height_target = movement_iw4::view_height::LAST_STAND;
        ps.pm_flags |= flag;
    } else {
        ps.pm_type = 0;
        ps.pm_flags &= !flag;
        ps.view_height_target = movement_iw4::view_height::STAND;
    }
    diag::info!(
        Sim,
        "spec ops: client={} last stand {}",
        id.0,
        if on { "down" } else { "up" }
    );
}

fn on(args: &[Value]) -> Result<bool, String> {
    match args.first() {
        None | Some(Value::Undefined) => Ok(true),
        Some(_) => Ok(int(args, 0)? != 0),
    }
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::Method;
    registry.register(Method, "enabledeathshield", |world, receiver, args| {
        let client = player(world, receiver)?;
        let enabled = on(args)?;
        set_shields(world, client, |s| s.death = enabled);
        Ok(Value::Undefined)
    });
    registry.register(Method, "enablehealthshield", |world, receiver, args| {
        let client = player(world, receiver)?;
        let enabled = on(args)?;
        set_shields(world, client, |s| s.health = enabled);
        Ok(Value::Undefined)
    });
    registry.register(Method, "enableinvulnerability", |world, receiver, _| {
        let client = player(world, receiver)?;
        set_shields(world, client, |s| s.invulnerable = true);
        Ok(Value::Undefined)
    });
    registry.register(Method, "disableinvulnerability", |world, receiver, _| {
        let client = player(world, receiver)?;
        set_shields(world, client, |s| s.invulnerable = false);
        Ok(Value::Undefined)
    });
    // SP builds the player's world body by calling this (`self setModel(...)`, head attach).
    registry.register(Method, "setmodelfunc", |world, receiver, args| {
        player(world, receiver)?;
        let Some(Value::Function(function)) = args.first() else {
            return Err("setmodelfunc takes a function reference".into());
        };
        crate::script::runtime::spawn_function(world, *function, receiver.clone(), Vec::new())?;
        Ok(Value::Undefined)
    });
}

pub(crate) fn kill_player(world: &mut World, receiver: &Value) -> Result<Value, String> {
    let client = player(world, receiver).map_err(|_| "kill: not a player or actor".to_string())?;
    let tick = world.resource::<crate::step::StepRequest>().tick;
    let me = player_object(world, client);
    kill(world, tick, client, Some(ClientId(client)), me);
    Ok(Value::Undefined)
}
