//! Engine actor and sentient fields, from the tables in `iw4sp.exe`
//! (`scripts/gen-iw4sp-fields.py`). Entity fields (`origin`, `health`,
//! `targetname`, ...) stay on the script entity like any other entity.

use super::fields_iw4sp::{IW4SP_ACTOR_FIELDS, IW4SP_ENTITY_FIELDS, IW4SP_SENTIENT_FIELDS};
use crate::script::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// The engine's `fieldtype_t`; not every type has an actor field.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldType {
    Int,
    Short,
    Byte,
    Float,
    String,
    CString,
    Vector,
    Entity,
    EntHandle,
    Angle,
    ActorHandle,
    SentientHandle,
    Time,
    PathNode,
    Object,
    Model,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FieldDef {
    pub name: &'static str,
    /// Offset in the engine struct, kept for reverse-engineering cross-checks.
    #[allow(dead_code)]
    pub offset: u32,
    pub kind: FieldType,
    pub read_only: bool,
    /// The engine reads or writes it through code rather than a plain store.
    #[allow(dead_code)]
    pub custom: bool,
}

impl FieldDef {
    pub(crate) const fn new(
        name: &'static str,
        offset: u32,
        kind: FieldType,
        read_only: bool,
        custom: bool,
    ) -> Self {
        Self {
            name,
            offset,
            kind,
            read_only,
            custom,
        }
    }
}

static FIELDS: LazyLock<BTreeMap<&'static str, &'static FieldDef>> = LazyLock::new(|| {
    IW4SP_SENTIENT_FIELDS
        .iter()
        .chain(IW4SP_ACTOR_FIELDS)
        .map(|def| (def.name, def))
        .collect()
});

pub(crate) fn actor_field(name: &str) -> Option<&'static FieldDef> {
    FIELDS.get(name).copied()
}

/// Entity fields scripts may not store to on an actor (`classname`, `model`, ...).
pub(crate) fn read_only_entity_field(name: &str) -> bool {
    IW4SP_ENTITY_FIELDS
        .iter()
        .any(|def| def.read_only && def.name == name)
}

pub(crate) const TEAMS: [&str; 5] = ["axis", "allies", "team3", "neutral", "dead"];

/// What an actor reads before anything wrote the field (`Actor_Init` values).
pub(crate) fn default_value(def: &FieldDef) -> Value {
    match def.name {
        "type" => return Value::string("human"),
        "accuracy" | "attackeraccuracy" | "finalaccuracy" => return Value::Float(1.0),
        "fovcosine" | "fovcosinebusy" => return Value::Float(0.5),
        "maxsightdistsqrd" => return Value::Float(8192.0 * 8192.0),
        "maxvisibledist" => return Value::Float(8192.0),
        "goalradius" => return Value::Float(1500.0),
        "goalheight" => return Value::Float(80.0),
        "walkdist" => return Value::Float(256.0),
        "walkdistfacingmotion" => return Value::Float(64.0),
        "interval" => return Value::Float(96.0),
        "pathenemyfightdist" | "pathenemylookahead" => return Value::Float(192.0),
        "meleeattackdist" => return Value::Float(64.0),
        "upaimlimit" | "rightaimlimit" => return Value::Float(45.0),
        "downaimlimit" | "leftaimlimit" => return Value::Float(-45.0),
        "grenadeweapon" => return Value::string("none"),
        "combatmode" => return Value::string("cover"),
        "alertlevel" => return Value::string("noncombat"),
        "damagemod" => return Value::string("MOD_UNKNOWN"),
        "movemode" => return Value::string("run"),
        "anim_pose" => return Value::string("stand"),
        "groundtype" => return Value::string("default"),
        "stairsstate" => return Value::string("none"),
        "allowpain" | "dropweapon" | "drawoncompass" | "pushable" | "facemotion"
        | "safetochangescript" => {
            return Value::Int(1);
        }
        _ => {}
    }
    match def.kind {
        FieldType::Int | FieldType::Short | FieldType::Byte | FieldType::Time => Value::Int(0),
        FieldType::Float | FieldType::Angle => Value::Float(0.0),
        FieldType::String | FieldType::CString | FieldType::Model => Value::string(""),
        FieldType::Vector => Value::Vector([0.0; 3]),
        FieldType::Entity
        | FieldType::EntHandle
        | FieldType::ActorHandle
        | FieldType::SentientHandle
        | FieldType::PathNode
        | FieldType::Object => Value::Undefined,
    }
}

fn type_name(value: &Value) -> &'static str {
    crate::script::host::args::kind(value)
}

/// The value a script store leaves in the field, or why the store is refused.
pub(crate) fn coerce(def: &FieldDef, value: &Value) -> Result<Value, String> {
    if def.read_only {
        return Err(format!("actor field {} is read-only", def.name));
    }
    if def.name == "team" || def.name == "type" {
        return match value {
            Value::String(text) => Ok(Value::String(text.clone())),
            other => Err(format!("{} is {}, not a string", def.name, type_name(other))),
        };
    }
    // Enumerated fields (`combatmode`, `alertlevel`, `grenadeweapon`, ...) are ints the
    // engine names through its own setter and getter.
    let named = def.custom
        && matches!(def.kind, FieldType::Int | FieldType::Short | FieldType::Byte)
        && matches!(value, Value::String(_));
    if named {
        return Ok(value.clone());
    }
    let refused = || {
        Err(format!(
            "actor field {} cannot hold {}",
            def.name,
            type_name(value)
        ))
    };
    match def.kind {
        FieldType::Int | FieldType::Short | FieldType::Byte | FieldType::Time => match value {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(n) => Ok(Value::Int(*n as i32)),
            _ => refused(),
        },
        FieldType::Float | FieldType::Angle => match value {
            Value::Int(n) => Ok(Value::Float(*n as f32)),
            Value::Float(n) => Ok(Value::Float(*n)),
            _ => refused(),
        },
        FieldType::String | FieldType::CString | FieldType::Model => match value {
            Value::String(_) => Ok(value.clone()),
            Value::Undefined => Ok(Value::string("")),
            _ => refused(),
        },
        FieldType::Vector => match value {
            Value::Vector(_) => Ok(value.clone()),
            _ => refused(),
        },
        FieldType::Entity
        | FieldType::EntHandle
        | FieldType::ActorHandle
        | FieldType::SentientHandle
        | FieldType::PathNode
        | FieldType::Object => match value {
            Value::Object(_) | Value::Undefined => Ok(value.clone()),
            _ => refused(),
        },
    }
}
