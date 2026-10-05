use bevy_ecs::prelude::Resource;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::script::host;
use crate::script::{Fault, Native, Program, StringTable, Value};

/// Script objects and arrays by id. Every field and element access looks
/// one up, so they are hashed (ids are sequential; one multiply spreads
/// them) rather than kept in an ordered map. Nothing iterates them in an
/// order that reaches script; the fields and elements inside stay ordered.
pub(crate) type IdMap<V> =
    std::collections::HashMap<u64, V, std::hash::BuildHasherDefault<IdHasher>>;

#[derive(Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(self.0 ^ u64::from(*byte));
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// The script objects' field maps, counting every mutable access: a reader
/// that derived something from fields (the collision settle) can tell that
/// nothing was written since. Reads go through `Deref`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Objects {
    map: IdMap<BTreeMap<u32, Value>>,
    writes: u64,
}

impl std::ops::Deref for Objects {
    type Target = IdMap<BTreeMap<u32, Value>>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl Objects {
    pub(crate) fn get_mut(&mut self, id: &u64) -> Option<&mut BTreeMap<u32, Value>> {
        self.writes += 1;
        self.map.get_mut(id)
    }

    pub(crate) fn insert(
        &mut self,
        id: u64,
        fields: BTreeMap<u32, Value>,
    ) -> Option<BTreeMap<u32, Value>> {
        self.writes += 1;
        self.map.insert(id, fields)
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&u64, &mut BTreeMap<u32, Value>) -> bool) {
        self.writes += 1;
        self.map.retain(keep);
    }

    /// Mutable accesses so far; unchanged means no field was written.
    pub(crate) fn writes(&self) -> u64 {
        self.writes
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub(crate) struct Runtime {
    pub(crate) program: Option<Arc<Program>>,
    pub(crate) natives: Vec<Native>,
    pub(crate) next_serial: u64,
    pub fault: Option<Fault>,
    pub(crate) fault_reported: bool,
    pub(crate) errors: BTreeMap<(String, String), u64>,
    /// Deleted entities scripts may still hold. They keep their fields until
    /// the end of the frame that deleted them, then read as undefined.
    pub(crate) dead: std::collections::BTreeSet<u64>,
    pub(crate) dying: Vec<u64>,
    pub(crate) last_tick: Option<crate::Tick>,
    pub(crate) started: bool,
    pub(crate) objects: Objects,
    pub(crate) next_object: u64,
    pub(crate) arrays: IdMap<crate::script::ScriptArray>,
    pub(crate) dynamic_symbols: BTreeMap<Arc<str>, u32>,
    pub(crate) buckets: BTreeMap<i64, VecDeque<u64>>,
    pub(crate) spawned: Vec<u64>,
    pub(crate) waiters: Vec<crate::script::Waiter>,
    pub(crate) dvars: BTreeMap<String, String>,
    pub(crate) local_presentation_dvars: bool,
    pub(crate) local_presentation_client: Option<crate::ClientId>,
    pub(crate) pending_local_dvars: Vec<(crate::TargetBoxDvar, String)>,
    pub(crate) loading: bool,
    pub(crate) precached: BTreeMap<(&'static str, String), i32>,
    pub(crate) presented: BTreeMap<&'static str, Vec<Value>>,
    pub(crate) unsupported: BTreeMap<&'static str, u64>,
    pub(crate) budget: usize,
    pub(crate) suspended: Vec<u64>,
    pub(crate) suspended_frames: usize,
    /// Endons that fired on a suspended thread; applied when its child yields.
    pub(crate) pending_unwinds: Vec<(u64, usize)>,
    pub(crate) entities: BTreeMap<u64, host::entities::ScriptEntity>,
    pub(crate) hud_slots: BTreeMap<u64, usize>,
    pub(crate) next_entity_number: i32,
    pub(crate) tables: Arc<BTreeMap<String, StringTable>>,
    pub(crate) rng: u32,
    pub(crate) pending_notifies: Vec<(Value, Arc<str>, Vec<Value>)>,
    pub(crate) signals: Vec<Arc<str>>,
    pub(crate) engine: host::entities::EngineState,
    pub(crate) players: BTreeMap<u32, host::players::PlayerSlot>,
    pub(crate) menu_answers: BTreeMap<u32, VecDeque<host::players::MenuAnswer>>,
    pub(crate) personal_classes: BTreeMap<(u32, u32), crate::ClassDef>,
    pub(crate) weapon_bridge: BTreeMap<u32, Vec<(u32, u32)>>,
    pub(crate) disconnects: std::collections::BTreeSet<u32>,
    pub(crate) kicks: BTreeMap<u32, String>,
    pub(crate) joined: std::collections::BTreeSet<u32>,
    pub(crate) current_hit: Option<crate::script_player::Hit>,
    pub(crate) deaths: VecDeque<(u32, &'static str, Vec<Value>)>,
    pub(crate) exit_level: bool,
    pub(crate) shown: BTreeMap<u64, host::presence::Shown>,
    pub(crate) retired_presence: Vec<(crate::ScriptModelId, bool)>,
    pub(crate) next_spawned_presence: u32,
    pub(crate) blasts: Vec<host::entity_damage::ScriptBlast>,
    pub(crate) hits: Vec<host::entity_damage::ScriptHit>,
    pub(crate) use_held: std::collections::BTreeSet<u32>,
    /// Each player's origin at the last trigger pass (touches are swept).
    pub(crate) touch_origins: BTreeMap<u32, [f32; 3]>,
    pub(crate) fired_once: std::collections::BTreeSet<u64>,
    pub(crate) require_look_at: std::collections::BTreeSet<u64>,
    pub(crate) server_info: std::collections::BTreeSet<String>,
    pub(crate) missiles: BTreeMap<crate::ProjectileId, u64>,
    pub(crate) missiles_seen_ms: i32,
    pub(crate) grenade_touches: Vec<host::triggers::GrenadeTouch>,
    pub(crate) lingering: Vec<(i64, u64)>,
    pub(crate) pending_deletes: Vec<u64>,
    pub(crate) vehicles: BTreeMap<u64, host::vehicles::Heli>,
    pub(crate) planes: BTreeMap<u64, host::vehicles::Plane>,
    pub(crate) use_selected: BTreeMap<u32, u64>,
    pub(crate) t5: host::natives::t5::T5State,
    pub(crate) restart: Option<Arc<host::restart::RestartPlan>>,
    pub(crate) finished: bool,
    pub(crate) pending_restart: Option<bool>,
    pub(crate) restored_pers: BTreeMap<u32, host::restart::Detached>,
    pub(crate) path_nodes: Vec<Option<u64>>,
    /// SP level entries; they start once the whole party has joined and spawned.
    pub(crate) player_entries: Vec<String>,
    /// Players the SP entry waits for (Spec Ops co-op: the lobby size at match start).
    pub(crate) party: usize,
    /// When the first party member joined; a missing partner stops being waited for later.
    pub(crate) party_since_ms: Option<i64>,
    pub(crate) sp: host::natives::sp::SpState,
    /// `next_object` at the last heap collection, and ticks since it.
    pub(crate) heap_mark: u64,
    pub(crate) heap_ticks: u32,
    /// Bumped when a waiter can newly be waiting on a deleted entity (an
    /// entity deleted, or a wait registered on one): resuming a thread only
    /// scans the wait list for that when it changed since the tick's sweep.
    pub(crate) doom_epoch: u64,
    /// The poses the last collision settle read (see `presence::settle_collision`).
    pub(crate) settle_cache: host::presence::SettleCache,
}

impl Runtime {
    pub(crate) fn program_fingerprint(&self) -> Option<[u8; 32]> {
        self.program.as_ref().map(|program| program.fingerprint())
    }

    pub(crate) fn live(&self, id: &u64) -> bool {
        self.objects.contains_key(id) && !self.dead.contains(id)
    }

    pub(crate) fn symbol(&mut self, name: &str) -> u32 {
        let program = self.program.as_ref().unwrap();
        if let Some(&id) = program.symbol_ids.get(name) {
            return id;
        }
        let next = (program.symbols.len() + self.dynamic_symbols.len()) as u32;
        *self.dynamic_symbols.entry(name.into()).or_insert(next)
    }
}
