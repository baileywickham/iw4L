use super::super::args::{arg, kind, string};
use super::player::player;
use crate::script::host::players::single_player;
use crate::LocalPlayerProfile;
use crate::script::{Namespace::Method, NativeRegistry, Value};

pub(crate) fn register(registry: &mut NativeRegistry) {
    registry.register(
        Method,
        "getlocalplayerprofiledata",
        |world, receiver, args| {
            if player(world, receiver)? != 0 {
                return Ok(Value::Undefined);
            }
            let name = string(args, 0)?;
            match world.resource::<LocalPlayerProfile>().get(&name) {
                Some(value) => Ok(Value::Int(i32::from(value))),
                None if single_player(world) => Ok(super::sp::profile_value(world, &name)),
                None => Err(format!("unknown local player profile field {name}")),
            }
        },
    );
    registry.register(
        Method,
        "setlocalplayerprofiledata",
        |world, receiver, args| {
            if player(world, receiver)? != 0 {
                return Ok(Value::Undefined);
            }
            if args.len() < 2 {
                return Err("setlocalplayerprofiledata expects at least two arguments".into());
            }
            let name = string(args, 0)?;
            if world.resource::<LocalPlayerProfile>().get(&name).is_none() {
                if single_player(world) {
                    super::sp::set_profile_value(world, &name, arg(args, 1)?.clone());
                    return Ok(Value::Undefined);
                }
                return Err(format!("unknown local player profile field {name}"));
            }
            let value = match arg(args, 1)? {
                Value::Int(value) => *value as u8,
                other => return Err(format!("parameter 2 is {}, not an int", kind(other))),
            };
            world.resource_mut::<LocalPlayerProfile>().set(&name, value);
            Ok(Value::Undefined)
        },
    );
}
