//! Single-player actors (AI). The script entity is the identity; `Actor` holds
//! the engine state GSC reaches through actor and sentient fields and methods.

pub(crate) mod fields;
mod fields_iw4sp;

use crate::script::Value;
use bevy_ecs::prelude::Resource;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(pub u32);

/// The engine's AI cap (`MAX_AI`).
pub(crate) const MAX_ACTORS: usize = 32;

#[derive(Clone, Debug)]
pub(crate) struct Actor {
    pub object: u64,
    pub team: Arc<str>,
    pub species: Arc<str>,
    /// Engine actor and sentient fields scripts have written; unwritten ones read
    /// their engine default.
    pub fields: BTreeMap<&'static str, Value>,
    /// The running animscript (`animscripts/<name>::main`) and its thread.
    pub animscript: Option<(Arc<str>, u64)>,
    pub animscript_started_ms: i64,
}

#[derive(Resource, Clone, Debug, Default)]
pub(crate) struct ActorPool {
    pub actors: BTreeMap<ActorId, Actor>,
    pub next: u32,
    pub spawner_teams: BTreeMap<u64, Arc<str>>,
    /// Map-placed actors (no spawner flag) waiting for the first actor think.
    pub pending: Vec<u64>,
}

impl ActorPool {
    pub(crate) fn allocate(&mut self) -> ActorId {
        let id = ActorId(self.next);
        self.next = self.next.wrapping_add(1);
        id
    }
}
