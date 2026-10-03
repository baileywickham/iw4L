//! Single-player actors (AI). The script entity is the identity; `Actor` holds
//! the engine state GSC reaches through actor and sentient fields and methods.

pub(crate) mod fields;
mod fields_iw4sp;
pub(crate) mod path;

use crate::script::Value;
use bevy_ecs::prelude::Resource;
use path::{ActorPath, PathSearch};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorId(pub u32);

/// The engine's AI cap (`MAX_AI`).
pub(crate) const MAX_ACTORS: usize = 32;

/// The script goal (`actor_goal_s`): radius and height live in the `goalradius`
/// and `goalheight` fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Goal {
    pub pos: [f32; 3],
    pub node: Option<u16>,
    pub volume: Option<u64>,
    pub entity: Option<u64>,
}

/// `orientmode`: script orientation overrides the code's choice until `face default`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Orient {
    #[default]
    Default,
    Current,
    Motion,
    Enemy,
    Angle(f32),
    Point([f32; 3]),
}

/// `movemode`, as the engine names it to script.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MoveMode {
    #[default]
    Stop,
    StopSoon,
    Walk,
    Run,
}

impl MoveMode {
    /// Script only sees `stop`, `walk` and `run`; the last 200 ms walk in.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::StopSoon => "walk",
            Self::Walk => "walk",
            Self::Run => "run",
        }
    }
}

/// What an actor knows about one sentient (`lastknownpos`, `lastknowntime`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Known {
    /// Last time it was seen (or reported by a squadmate or by its damage).
    pub time_ms: i64,
    pub pos: [f32; 3],
    /// Where its eye was when this actor last saw it (`lastenemysightpos`).
    pub sight_pos: Option<[f32; 3]>,
    pub seen_ms: Option<i64>,
}

/// Engine state of a dying actor: the death animscript runs, then it becomes a corpse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dying {
    pub since_ms: i64,
}

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
    pub prev_animscript: Option<Arc<str>>,
    pub goal: Goal,
    pub path: Option<ActorPath>,
    pub search: Option<(PathSearch, [f32; 3])>,
    /// No new path search before this time (`pathWaitTime`).
    pub path_wait_ms: i64,
    pub claimed: Option<u16>,
    pub prev_claimed: Option<u16>,
    pub nearest: Option<(u16, [f32; 3])>,
    pub velocity: [f32; 3],
    pub lookahead_dir: [f32; 3],
    pub lookahead_dist: f32,
    pub move_mode: MoveMode,
    pub orient: Orient,
    pub anim_mode: Arc<str>,
    pub allowed_stances: u8,
    /// Last tick each entity was checked for sight, and whether it was seen.
    pub sight: BTreeMap<u64, (i64, bool)>,
    pub distance_moved: f32,
    /// `"goal"` was raised since the goal last changed (for the arrival log).
    pub goal_reached: bool,
    pub enemy: Option<u64>,
    pub known: BTreeMap<u64, Known>,
    pub entity_target: Option<u64>,
    pub last_attacker: Option<(u64, i64)>,
    pub dying: Option<Dying>,
    pub shots: u32,
    pub hits_taken: u32,
    /// A path the code goal does not own: a reacquire move or `setruntopos`.
    pub detour: Option<Detour>,
    /// No full cover search before this time; the claimed node is still rechecked.
    pub cover_search_ms: i64,
    /// `findcovernode` results, best last, popped by `getcovernode`.
    pub cover_list: Vec<u16>,
    /// `startcoverarrival`: the actor plays `cover_arrival` into its claimed node.
    pub arrival: Option<f32>,
    /// The grenade the actor is reacting to (`grenade` field).
    pub grenade: Option<u64>,
    /// Grenades already judged, so one is reacted to once.
    pub grenades_seen: Vec<u64>,
    /// The last `checkgrenadethrow*` solution: launch position and velocity.
    pub toss: Option<([f32; 3], [f32; 3])>,
    /// A grenade picked up to throw back: its weapon and the fuse it had left.
    pub picked_up: Option<(u32, i32)>,
    /// `suppressionmeter`, fed by hostile bullets passing close.
    pub suppression: f32,
    /// Last hostile bullet that suppressed the actor at cover, and since when
    /// it has been suppressed (`suppressionstarttime`).
    pub suppressed_ms: i64,
    pub suppressed_since: i64,
    pub pains: u32,
    /// `animscripted`: the `scripted` animscript holds the actor until this time.
    pub scripted_until_ms: i64,
    /// `addaieventlistener`: the AI events this actor hears as `"ai_event"` notifies.
    pub listeners: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DetourKind {
    Reacquire,
    RunTo,
    /// Out of a grenade's blast.
    Flee,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Detour {
    pub pos: [f32; 3],
    pub kind: DetourKind,
}

pub(crate) const STANCE_STAND: u8 = 1;
pub(crate) const STANCE_CROUCH: u8 = 2;
pub(crate) const STANCE_PRONE: u8 = 4;

impl Actor {
    pub(crate) fn new(object: u64, team: Arc<str>, origin: [f32; 3]) -> Self {
        Self {
            object,
            team,
            species: "human".into(),
            fields: BTreeMap::new(),
            animscript: None,
            animscript_started_ms: 0,
            prev_animscript: None,
            goal: Goal {
                pos: origin,
                ..Goal::default()
            },
            path: None,
            search: None,
            path_wait_ms: 0,
            claimed: None,
            prev_claimed: None,
            nearest: None,
            velocity: [0.0; 3],
            lookahead_dir: [0.0; 3],
            lookahead_dist: 0.0,
            move_mode: MoveMode::Stop,
            orient: Orient::Default,
            anim_mode: "normal".into(),
            allowed_stances: STANCE_STAND | STANCE_CROUCH | STANCE_PRONE,
            sight: BTreeMap::new(),
            distance_moved: 0.0,
            goal_reached: false,
            enemy: None,
            known: BTreeMap::new(),
            entity_target: None,
            last_attacker: None,
            dying: None,
            shots: 0,
            hits_taken: 0,
            detour: None,
            cover_search_ms: 0,
            cover_list: Vec::new(),
            arrival: None,
            grenade: None,
            grenades_seen: Vec::new(),
            toss: None,
            picked_up: None,
            suppression: 0.0,
            suppressed_ms: 0,
            scripted_until_ms: 0,
            suppressed_since: 0,
            pains: 0,
            listeners: 0,
        }
    }

    pub(crate) fn float_field(&self, name: &str) -> f32 {
        let value = match self.fields.get(name) {
            Some(value) => value.clone(),
            None => fields::actor_field(name).map_or(Value::Undefined, fields::default_value),
        };
        match value {
            Value::Float(v) => v,
            Value::Int(v) => v as f32,
            _ => 0.0,
        }
    }

    /// `Actor_PointAtGoal`: inside the goal cylinder (and volume, checked by the caller).
    pub(crate) fn point_in_goal_cylinder(&self, point: [f32; 3]) -> bool {
        let height = self.float_field("goalheight");
        let radius = self.float_field("goalradius");
        let d = [point[0] - self.goal.pos[0], point[1] - self.goal.pos[1]];
        (point[2] - self.goal.pos[2]).powi(2) <= height * height
            && d[0] * d[0] + d[1] * d[1] <= radius * radius
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub(crate) struct ActorPool {
    pub actors: BTreeMap<ActorId, Actor>,
    pub next: u32,
    pub spawner_teams: BTreeMap<u64, Arc<str>>,
    /// Map-placed actors (no spawner flag) waiting for the first actor think.
    pub pending: Vec<u64>,
    /// Path node index → claiming actor.
    pub claims: BTreeMap<u16, ActorId>,
    /// Actor corpses, oldest first (`ai_corpseCount` caps the queue).
    pub corpses: std::collections::VecDeque<u64>,
    /// The actors' own random stream: shot rolls and miss offsets.
    pub draws: u64,
    /// Entities made sentient by script (`makeentitysentient`) and their team.
    pub sentients: BTreeMap<u64, Arc<str>>,
    /// A sentient's nearest path node and when it was found (cover visibility).
    pub sentient_nodes: BTreeMap<u64, (i64, Option<u16>)>,
    /// Actor grenades thrown and pains played, for the run summary.
    pub grenades_thrown: u32,
    /// Bullet lines fired since the actors last looked (start, end, shooter).
    pub whizzes: Vec<([f32; 3], [f32; 3], crate::Attacker)>,
    /// AI events (gunshots, explosions, pain, death, ...) waiting for the actors' next look.
    pub events: Vec<PendingEvent>,
    /// When each player last made a footstep event.
    pub footsteps: BTreeMap<u32, i64>,
}

/// The engine's AI events (`ai_event_t`): what actors hear without seeing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AiEvent {
    Footstep,
    FootstepWalk,
    FootstepSprint,
    NewEnemy,
    Pain,
    Death,
    Explosion,
    GrenadePing,
    ProjectilePing,
    Gunshot,
    GunshotTeammate,
    SilencedShot,
    Bullet,
    ProjectileImpact,
}

/// Where an event came from: the entity that caused it (a shooter, a casualty, a
/// grenade's thrower) or the attacker the combat pipeline reported.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EventSource {
    Object(u64),
    Attacker(crate::Attacker),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PendingEvent {
    pub kind: AiEvent,
    pub source: Option<EventSource>,
    /// The casualty's attacker for pain and death.
    pub attacker: Option<u64>,
    pub at: [f32; 3],
    /// Line events (bullets, projectile impacts) run from `at` to here.
    pub end: Option<[f32; 3]>,
    pub weapon: u32,
}

impl ActorPool {
    /// A uniform draw in [0, 1) from the actor stream.
    pub(crate) fn random(&mut self) -> f32 {
        self.draws = self.draws.wrapping_add(1);
        let mut rng =
            crate::MatchRng::new(0xA1_7012 ^ self.draws.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        (rng.next_u32() >> 8) as f32 / 16_777_216.0
    }

    pub(crate) fn allocate(&mut self) -> ActorId {
        let id = ActorId(self.next);
        self.next = self.next.wrapping_add(1);
        id
    }
}
