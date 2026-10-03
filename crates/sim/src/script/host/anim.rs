use super::animtree::ScriptAnimTree;
use super::args::{arg, float, kind, optional, string, vector};
use super::arrays::new_array;
use super::mechanics::Mechanics;
use super::natives::engine::entity_id;
use crate::frame::FrameWorld;
use crate::script::{Arc, Namespace, NativeRegistry, Runtime, Value};
use anim_iw4::{XANIM_NONLOOP_END_PARK, XANIM_WEIGHT_FLOOR};
use bevy_ecs::prelude::World;
use xmodel_runtime::{
    AnimClip, XAnimNodeDefinition, XAnimNodeId, XAnimNodeKind, XAnimNodeState, XAnimSemanticNode,
    XAnimSemanticNodeKind, XAnimTreeDefinition, XAnimTreeRuntime,
};

const FLAGGED: u8 = 1;
const KNOB: u8 = 2;
const ALL: u8 = 4;
const LIMITED: u8 = 8;
const RESTART: u8 = 16;

const DEFAULT_BLEND: f32 = 0.2;

/// A script entity's animtree instance: the shared definition, the per-node
/// goal weights/times in an `XAnimTreeRuntime` (the state a snapshot carries)
/// and the notify name each flagged node reports its notetracks under.
#[derive(Clone, Debug)]
pub(crate) struct EntityAnim {
    tree: Arc<ScriptAnimTree>,
    runtime: XAnimTreeRuntime,
    flags: Vec<Option<Arc<str>>>,
}

impl EntityAnim {
    pub(crate) fn new(tree: Arc<ScriptAnimTree>) -> Self {
        Self {
            runtime: XAnimTreeRuntime::new(Arc::clone(&tree.definition)),
            flags: vec![None; tree.nodes.len()],
            tree,
        }
    }

    fn state(&self, node: u16) -> XAnimNodeState {
        self.runtime.states()[node as usize]
    }

    fn put(&mut self, node: u16, state: XAnimNodeState) -> Result<(), String> {
        self.runtime
            .set_state(XAnimNodeId(node), state)
            .map_err(|e| e.to_string())
    }

    fn set_goal(&mut self, node: u16, weight: f32, time: f32) -> Result<(), String> {
        let mut state = self.state(node);
        state.goal_weight = weight;
        state.goal_time = time;
        if time <= 0.0 {
            state.weight = weight;
        }
        self.put(node, state)
    }

    fn zero_siblings(&mut self, node: u16, time: f32) -> Result<(), String> {
        let Some(parent) = self.tree.nodes[node as usize].parent else {
            return Ok(());
        };
        for sibling in self.tree.nodes[parent as usize].children.clone() {
            if sibling != node {
                self.set_goal(sibling, 0.0, time)?;
            }
        }
        Ok(())
    }

    fn subtree(&self, node: u16) -> Vec<u16> {
        let mut out = vec![node];
        let mut at = 0;
        while at < out.len() {
            out.extend(self.tree.nodes[out[at] as usize].children.clone());
            at += 1;
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn set(
        &mut self,
        node: u16,
        root: Option<u16>,
        weight: f32,
        time: f32,
        rate: f32,
        mode: u8,
        flag: Option<Arc<str>>,
    ) -> Result<(), String> {
        if let Some(root) = root {
            if !self.tree.is_under(node, root) {
                return Err(format!(
                    "%{} is not under %{}",
                    self.tree.nodes[node as usize].name, self.tree.nodes[root as usize].name
                ));
            }
            let mut at = node;
            while at != root {
                self.zero_siblings(at, time)?;
                at = self.tree.nodes[at as usize].parent.unwrap();
            }
        } else if mode & KNOB != 0 {
            self.zero_siblings(node, time)?;
        }
        let weight = if weight < XANIM_WEIGHT_FLOOR {
            0.0
        } else {
            weight
        };
        self.set_goal(node, weight, time)?;
        let mut state = self.state(node);
        state.rate = rate;
        self.put(node, state)?;
        if mode & RESTART != 0 {
            for id in self.subtree(node) {
                let mut state = self.state(id);
                state.time = 0.0;
                state.old_time = 0.0;
                state.cycle_count = 0;
                state.old_cycle_count = 0;
                self.put(id, state)?;
            }
        }
        if mode & LIMITED == 0 {
            let mut at = self.tree.nodes[node as usize].parent;
            while let Some(parent) = at {
                self.set_goal(parent, 1.0, time)?;
                at = self.tree.nodes[parent as usize].parent;
            }
        }
        self.flags[node as usize] = flag;
        Ok(())
    }

    fn clear(&mut self, node: u16, time: f32) -> Result<(), String> {
        for id in self.subtree(node) {
            self.set_goal(id, 0.0, time)?;
        }
        Ok(())
    }

    fn advance(&mut self, dtime: f32, notes: &mut Vec<(Arc<str>, Arc<str>)>) -> Result<(), String> {
        self.runtime
            .update_inherited_rate(dtime)
            .map_err(|e| e.to_string())?;
        for node in 0..self.tree.nodes.len() as u16 {
            let state = self.state(node);
            if state.weight != 0.0
                && state.time < 1.0
                && self
                    .tree
                    .clip(node)
                    .is_some_and(|clip| clip.frequency() == 0.0)
            {
                self.put(node, XAnimNodeState { time: 1.0, ..state })?;
            }
        }
        for node in 0..self.tree.nodes.len() {
            let Some(flag) = self.flags[node].clone() else {
                continue;
            };
            let Some(leaf) = self.dominant_leaf(node as u16) else {
                continue;
            };
            let Some(clip) = self.tree.clip(leaf) else {
                continue;
            };
            let state = self.state(leaf);
            crossed(clip, &state, |note| {
                notes.push((Arc::clone(&flag), note.into()))
            });
        }
        self.free_idle_nodes()
    }

    /// The part of the tree that reaches the pose: weighted nodes under
    /// weighted ancestors that lead to a clip. State is cut to time and
    /// weight (the rest stays default and costs no wire bytes), so an
    /// unchanged pose compares equal from tick to tick.
    fn active_pose(&self) -> Result<Option<(Vec<XAnimSemanticNode>, XAnimTreeRuntime)>, String> {
        let states = self.runtime.states();
        let defs = self.tree.definition.nodes();
        let mut on = vec![false; defs.len()];
        for node in 0..defs.len() {
            on[node] = states[node].weight > 0.0
                && defs[node].parent.is_none_or(|parent| on[parent.0 as usize]);
        }
        let mut keep = vec![false; defs.len()];
        for node in (0..defs.len()).rev() {
            keep[node] =
                on[node] && (keep[node] || matches!(defs[node].kind, XAnimNodeKind::Leaf { .. }));
            if let (true, Some(parent)) = (keep[node], defs[node].parent) {
                keep[parent.0 as usize] = true;
            }
        }
        let mut remap = vec![u16::MAX; defs.len()];
        let mut nodes = Vec::new();
        let mut definition = Vec::new();
        for node in (0..defs.len()).filter(|node| keep[*node]) {
            remap[node] = nodes.len() as u16;
            let parent = defs[node]
                .parent
                .map(|parent| XAnimNodeId(remap[parent.0 as usize]));
            let (kind, clip, parts) = match &defs[node].kind {
                XAnimNodeKind::Blend => (XAnimSemanticNodeKind::Blend, None, None),
                XAnimNodeKind::Additive => (XAnimSemanticNodeKind::Additive, None, None),
                XAnimNodeKind::Leaf { parts, .. } => (
                    XAnimSemanticNodeKind::Leaf,
                    Some(self.tree.nodes[node].name.to_string()),
                    *parts,
                ),
            };
            let state = XAnimNodeState {
                time: states[node].time,
                weight: states[node].weight,
                ..XAnimNodeState::default()
            };
            nodes.push(XAnimSemanticNode {
                parent,
                kind,
                clip,
                parts,
                state,
            });
            definition.push(XAnimNodeDefinition {
                parent,
                kind: defs[node].kind.clone(),
            });
        }
        if nodes.is_empty() {
            return Ok(None);
        }
        let definition = XAnimTreeDefinition::new(definition).map_err(|e| e.to_string())?;
        let mut runtime = XAnimTreeRuntime::new(Arc::new(definition));
        for (node, semantic) in nodes.iter().enumerate() {
            runtime
                .set_state(XAnimNodeId(node as u16), semantic.state)
                .map_err(|e| e.to_string())?;
        }
        Ok(Some((nodes, runtime)))
    }

    fn dominant_leaf(&self, node: u16) -> Option<u16> {
        let states = self.runtime.states();
        let mut best: Option<(f32, u16)> = None;
        let mut stack = vec![(node, 1.0f32)];
        while let Some((at, weight)) = stack.pop() {
            let weight = if at == node {
                1.0
            } else {
                weight * states[at as usize].weight
            };
            if weight <= 0.0 && at != node {
                continue;
            }
            let children = self.tree.nodes[at as usize].children.clone();
            if children.is_empty() {
                if self.tree.clip(at).is_some()
                    && states[at as usize].weight != 0.0
                    && best.is_none_or(|(w, id)| weight > w || (weight == w && at < id))
                {
                    best = Some((weight, at));
                }
                continue;
            }
            stack.extend(children.rev().map(|child| (child, weight)));
        }
        best.map(|(_, id)| id)
    }

    fn free_idle_nodes(&mut self) -> Result<(), String> {
        let count = self.tree.nodes.len();
        let mut live = vec![false; count];
        for node in (0..count).rev() {
            let state = self.runtime.states()[node];
            live[node] = state.weight != 0.0
                || state.goal_weight != 0.0
                || self.tree.nodes[node]
                    .children
                    .clone()
                    .any(|child| live[child as usize]);
        }
        for (node, live) in live.into_iter().enumerate() {
            let state = self.runtime.states()[node];
            if !live && state != XAnimNodeState::default() {
                self.put(node as u16, XAnimNodeState::default())?;
                self.flags[node] = None;
            }
        }
        Ok(())
    }
}

fn crossed(clip: &AnimClip, state: &XAnimNodeState, mut deliver: impl FnMut(&str)) {
    let (old, new) = (state.old_time, state.time);
    let cycles = state.cycle_count.wrapping_sub(state.old_cycle_count);
    if old == new && cycles == 0 {
        return;
    }
    if !clip.looping && old >= XANIM_NONLOOP_END_PARK {
        return;
    }
    let mut notes: Vec<(f32, &str)> = clip
        .notifies
        .iter()
        .map(|note| (note.time, note.name.as_str()))
        .collect();
    if !notes
        .iter()
        .any(|(_, name)| name.eq_ignore_ascii_case("end"))
    {
        notes.push((1.0, "end"));
    }
    notes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let fresh = old == 0.0 && state.old_cycle_count == 0;
    let after = |t: f32| t > old || (fresh && t >= old);
    if cycles == 0 {
        let end = if !clip.looping && new >= XANIM_NONLOOP_END_PARK {
            1.0
        } else {
            new
        };
        for (t, name) in &notes {
            if after(*t) && *t <= end {
                deliver(name);
            }
        }
        return;
    }
    for (t, name) in &notes {
        if after(*t) {
            deliver(name);
        }
    }
    if cycles > 1 {
        for (_, name) in &notes {
            deliver(name);
        }
    }
    for (t, name) in &notes {
        if *t <= new && *t < 1.0 {
            deliver(name);
        }
    }
}

pub(crate) fn advance_anims(world: &mut World, dtime: f32) {
    world.resource_scope::<Mechanics, _>(|world, mut mechanics| {
        if mechanics.anims.is_empty() {
            return;
        }
        let runtime = world.resource::<Runtime>();
        mechanics
            .anims
            .retain(|object, _| runtime.entities.contains_key(object));
        let mut order: Vec<(i32, u64)> = mechanics
            .anims
            .keys()
            .map(|object| (runtime.entities[object].number, *object))
            .collect();
        order.sort_unstable();
        for (_, object) in order {
            let mut notes = Vec::new();
            let anim = mechanics.anims.get_mut(&object).unwrap();
            if let Err(error) = anim.advance(dtime, &mut notes) {
                diag::warn!(Sim, "gsc: animtree {} on entity: {error}", anim.tree.name);
            }
            mechanics
                .anim_notes
                .extend(notes.into_iter().map(|(flag, note)| (object, flag, note)));
        }
    });
}

/// Put each animated entity's active tree on its DObj: the snapshot carries
/// it to clients and the authority poses bullet collision and tags with it.
pub(crate) fn publish_anims(world: &mut World) {
    let runtime = world.resource::<Runtime>();
    let mechanics = world.resource::<Mechanics>();
    let mut poses = Vec::new();
    for (object, entity) in &runtime.entities {
        let Some(presence) = entity.presence else {
            continue;
        };
        let pose = match mechanics.anims.get(object).map(EntityAnim::active_pose) {
            Some(Ok(pose)) => pose,
            Some(Err(error)) => {
                diag::warn!(
                    Sim,
                    "gsc: animtree pose on entity {}: {error}",
                    entity.number
                );
                None
            }
            None => None,
        };
        poses.push((presence, pose));
    }
    let mut frame = FrameWorld::from_world(world);
    for (presence, pose) in poses {
        if let Some(dobj) = frame
            .collision_owner_mut(presence)
            .and_then(|row| row.dobj.as_mut())
        {
            dobj.set_script_tree(pose);
        }
    }
}

fn anim_value(args: &[Value], index: usize) -> Result<(Arc<str>, Arc<str>), String> {
    match arg(args, index)? {
        Value::Animation { tree, name } => Ok((tree.clone(), name.clone())),
        other => Err(format!("{} is not an animation", kind(other))),
    }
}

fn with_anim<T>(
    world: &mut World,
    receiver: &Value,
    change: impl FnOnce(&mut EntityAnim) -> Result<T, String>,
) -> Result<T, String> {
    let id = entity_id(world, receiver)?;
    let mut mechanics = world.resource_mut::<Mechanics>();
    let anim = mechanics
        .anims
        .get_mut(&id)
        .ok_or("entity has no animtree; call useanimtree first")?;
    change(anim)
}

fn node(anim: &EntityAnim, args: &[Value], index: usize) -> Result<u16, String> {
    let (tree, name) = anim_value(args, index)?;
    if !tree.eq_ignore_ascii_case(&anim.tree.name) {
        return Err(format!(
            "%{name} is from animtree {tree}, the entity uses {}",
            anim.tree.name
        ));
    }
    anim.tree
        .node(&name)
        .ok_or_else(|| format!("%{name} is not in animtree {}", anim.tree.name))
}

fn finite(value: f32, what: &str) -> Result<f32, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(format!("{what} must be a non-negative number"))
    }
}

fn set_anim(
    world: &mut World,
    receiver: &Value,
    args: &[Value],
    mode: u8,
) -> Result<Value, String> {
    let mut at = 0;
    let flag = if mode & FLAGGED != 0 {
        at += 1;
        Some(Arc::<str>::from(string(args, 0)?))
    } else {
        None
    };
    let anim_at = at;
    at += 1;
    let root_at = (mode & ALL != 0).then(|| {
        at += 1;
        at - 1
    });
    let weight = finite(optional(args, at, float)?.unwrap_or(1.0), "goal weight")?;
    let time = finite(
        optional(args, at + 1, float)?.unwrap_or(DEFAULT_BLEND),
        "blend time",
    )?;
    let rate = finite(optional(args, at + 2, float)?.unwrap_or(1.0), "rate")?;
    with_anim(world, receiver, |anim| {
        let target = node(anim, args, anim_at)?;
        let root = root_at.map(|i| node(anim, args, i)).transpose()?;
        anim.set(target, root, weight, time, rate, mode, flag)
    })?;
    Ok(Value::Undefined)
}

fn clip_named(world: &mut World, args: &[Value], index: usize) -> Result<Arc<AnimClip>, String> {
    let (_, name) = anim_value(args, index)?;
    let frame = FrameWorld::from_world(world);
    frame
        .content()
        .script_anims()
        .clip(&name)
        .or_else(|| frame.player_anim_clip_named(&name))
        .ok_or_else(|| format!("animation %{name} is not loaded in the simulation"))
}

fn span(args: &[Value], from: usize) -> Result<(f32, f32), String> {
    let start = optional(args, from, float)?.unwrap_or(0.0);
    let end = optional(args, from + 1, float)?.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&start) || !(0.0..=1.0).contains(&end) {
        return Err("animation times must be in [0, 1]".into());
    }
    Ok((start, end))
}

fn wrap_degrees(angle: f32) -> f32 {
    (angle + 180.0).rem_euclid(360.0) - 180.0
}

/// `startscriptedanim( notify, origin, angles, anim, mode, root )`: place the
/// entity at the anim's start relative to origin/angles and play it flagged
/// under root (the tree root when absent), so notetracks and "end" reach
/// `notify`. An actor follows the anim's root motion (rappels, traversals);
/// other entities stay put. Without an animtree on the entity only "end" is
/// sent, after the anim's length.
fn start_scripted_anim(
    world: &mut World,
    receiver: &Value,
    args: &[Value],
) -> Result<Value, String> {
    let flag: Arc<str> = string(args, 0)?.into();
    let origin = vector(args, 1)?;
    let angles = vector(args, 2)?;
    let clip = clip_named(world, args, 3)?;
    let id = entity_id(world, receiver)?;
    let trans = clip.abs_delta_trans(0.0);
    let (forward, right, up) = math_iw4::angle_vectors(angles);
    let start: [f32; 3] = std::array::from_fn(|i| {
        origin[i] + forward[i] * trans[0] - right[i] * trans[1] + up[i] * trans[2]
    });
    let facing = [
        angles[0],
        wrap_degrees(angles[1] + clip.abs_delta_yaw(0.0)),
        angles[2],
    ];
    let teleport = world
        .resource::<NativeRegistry>()
        .get(Namespace::Method, "teleport")
        .ok_or("teleport is not bound")?;
    if teleport(
        world,
        receiver,
        &[Value::Vector(start), Value::Vector(facing)],
    )
    .is_err()
    {
        let mut runtime = world.resource_mut::<Runtime>();
        runtime.set_object_field(id, "origin", Value::Vector(start));
        runtime.set_object_field(id, "angles", Value::Vector(facing));
    }
    let played = with_anim(world, receiver, |anim| {
        let target = node(anim, args, 3)?;
        let root = match args.get(5) {
            Some(Value::Animation { .. }) => node(anim, args, 5).unwrap_or(0),
            _ => 0,
        };
        let root = if anim.tree.is_under(target, root) {
            root
        } else {
            0
        };
        anim.set(
            target,
            Some(root),
            1.0,
            DEFAULT_BLEND,
            1.0,
            FLAGGED | KNOB | ALL | RESTART,
            Some(flag.clone()),
        )
    });
    if super::actors::actor_of(world, id).is_some() {
        let now = super::players::now_ms(world);
        world.resource_mut::<Mechanics>().start(
            id,
            super::mechanics::Motion {
                field: "origin",
                path: super::mechanics::MotionPath::Anim {
                    origin,
                    angles,
                    clip: super::mechanics::AnimRoot(clip.clone()),
                },
                start_ms: now,
                duration_ms: (clip.duration() * 1000.0) as i64,
                done: "scripted_root_done",
            },
        );
    }
    if played.is_err() {
        let due = super::players::now_ms(world) + (clip.duration() * 1000.0) as i64;
        world
            .resource_mut::<Mechanics>()
            .notify_at(due, id, flag, vec![Value::string("end")]);
    }
    Ok(Value::Undefined)
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    use Namespace::{Function, Method};

    registry.register(Method, "useanimtree", |world, receiver, args| {
        let name = match arg(args, 0)? {
            Value::AnimationTree(name) => name.clone(),
            other => return Err(format!("{} is not an animtree", kind(other))),
        };
        let id = entity_id(world, receiver)?;
        let tree = FrameWorld::from_world(world)
            .content()
            .script_anims()
            .tree(&name)?;
        let mut mechanics = world.resource_mut::<Mechanics>();
        if mechanics
            .anims
            .get(&id)
            .is_none_or(|anim| !Arc::ptr_eq(&anim.tree, &tree))
        {
            mechanics.anims.insert(id, EntityAnim::new(tree));
        }
        Ok(Value::Undefined)
    });
    registry.register(Method, "stopuseanimtree", |world, receiver, _| {
        let id = entity_id(world, receiver)?;
        world.resource_mut::<Mechanics>().anims.remove(&id);
        Ok(Value::Undefined)
    });

    macro_rules! set_anims {
        ($($name:literal => $mode:expr),* $(,)?) => {$(
            registry.register(Method, $name, |world, receiver, args| {
                set_anim(world, receiver, args, $mode)
            });
        )*};
    }
    set_anims![
        "setanim" => 0,
        "setanimknob" => KNOB,
        "setanimknoball" => KNOB | ALL,
        "setanimlimited" => LIMITED,
        "setanimknoblimited" => KNOB | LIMITED,
        "setanimknoballlimited" => KNOB | ALL | LIMITED,
        "setanimrestart" => RESTART,
        "setanimknobrestart" => KNOB | RESTART,
        "setanimknoballrestart" => KNOB | ALL | RESTART,
        "setanimlimitedrestart" => LIMITED | RESTART,
        "setanimknoblimitedrestart" => KNOB | LIMITED | RESTART,
        "setanimknoballlimitedrestart" => KNOB | ALL | LIMITED | RESTART,
        "setflaggedanim" => FLAGGED,
        "setflaggedanimknob" => FLAGGED | KNOB,
        "setflaggedanimknoball" => FLAGGED | KNOB | ALL,
        "setflaggedanimlimited" => FLAGGED | LIMITED,
        "setflaggedanimknoblimited" => FLAGGED | KNOB | LIMITED,
        "setflaggedanimrestart" => FLAGGED | RESTART,
        "setflaggedanimknobrestart" => FLAGGED | KNOB | RESTART,
        "setflaggedanimknoballrestart" => FLAGGED | KNOB | ALL | RESTART,
        "setflaggedanimlimitedrestart" => FLAGGED | LIMITED | RESTART,
        "setflaggedanimknoblimitedrestart" => FLAGGED | KNOB | LIMITED | RESTART,
    ];

    registry.register(Method, "clearanim", |world, receiver, args| {
        let time = finite(float(args, 1)?, "blend time")?;
        with_anim(world, receiver, |anim| {
            let target = node(anim, args, 0)?;
            anim.clear(target, time)
        })?;
        Ok(Value::Undefined)
    });
    registry.register(Method, "setanimtime", |world, receiver, args| {
        let time = float(args, 1)?;
        if !(0.0..=1.0).contains(&time) {
            return Err(format!("anim time {time} is not in [0, 1]"));
        }
        with_anim(world, receiver, |anim| {
            let target = node(anim, args, 0)?;
            let state = anim.state(target);
            anim.put(
                target,
                XAnimNodeState {
                    time,
                    old_time: time,
                    ..state
                },
            )
        })?;
        Ok(Value::Undefined)
    });
    registry.register(Method, "getanimtime", |world, receiver, args| {
        with_anim(world, receiver, |anim| {
            let target = node(anim, args, 0)?;
            Ok(Value::Float(anim.state(target).time))
        })
    });

    registry.register(Function, "getanimlength", |world, _, args| {
        Ok(Value::Float(clip_named(world, args, 0)?.duration()))
    });
    registry.register(Function, "animhasnotetrack", |world, _, args| {
        let clip = clip_named(world, args, 0)?;
        let note = string(args, 1)?;
        Ok(Value::Int(
            clip.notifies
                .iter()
                .any(|n| n.name.eq_ignore_ascii_case(&note))
                .into(),
        ))
    });
    registry.register(Function, "getnotetracktimes", |world, _, args| {
        let clip = clip_named(world, args, 0)?;
        let note = string(args, 1)?;
        let times: Vec<Value> = clip
            .notifies
            .iter()
            .filter(|n| n.name.eq_ignore_ascii_case(&note))
            .map(|n| Value::Float(n.time))
            .collect();
        new_array(world, times)
    });
    registry.register(Function, "getmovedelta", |world, _, args| {
        let clip = clip_named(world, args, 0)?;
        let (start, end) = span(args, 1)?;
        let from = clip.abs_delta_trans(start);
        let to = clip.abs_delta_trans(end);
        let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let (sin, cos) = (-clip.abs_delta_yaw(start)).to_radians().sin_cos();
        Ok(Value::Vector([
            delta[0] * cos - delta[1] * sin,
            delta[0] * sin + delta[1] * cos,
            delta[2],
        ]))
    });
    registry.register(Function, "getstartorigin", |world, _, args| {
        let origin = vector(args, 0)?;
        let angles = vector(args, 1)?;
        let trans = clip_named(world, args, 2)?.abs_delta_trans(0.0);
        let (forward, right, up) = math_iw4::angle_vectors(angles);
        Ok(Value::Vector(std::array::from_fn(|i| {
            origin[i] + forward[i] * trans[0] - right[i] * trans[1] + up[i] * trans[2]
        })))
    });
    registry.register(Function, "getstartangles", |world, _, args| {
        vector(args, 0)?;
        let angles = vector(args, 1)?;
        let yaw = clip_named(world, args, 2)?.abs_delta_yaw(0.0);
        Ok(Value::Vector([
            angles[0],
            wrap_degrees(angles[1] + yaw),
            angles[2],
        ]))
    });
    // `animscripted( notify, origin, angles, anim, mode, root )`: an actor runs
    // the `scripted` animscript, which calls `startscriptedanim`; anything else
    // starts the anim directly.
    registry.register(Method, "animscripted", |world, receiver, args| {
        string(args, 0)?;
        vector(args, 1)?;
        vector(args, 2)?;
        let duration = clip_named(world, args, 3)?.duration();
        let duration_ms = (duration * 1000.0) as i64;
        if super::actor_nav::begin_scripted(world, receiver, args, duration_ms).is_some() {
            return Ok(Value::Undefined);
        }
        start_scripted_anim(world, receiver, args)
    });
    registry.register(Method, "startscriptedanim", start_scripted_anim);
    registry.register(Method, "stopanimscripted", |world, receiver, _| {
        entity_id(world, receiver)?;
        super::actor_nav::end_scripted(world, receiver);
        Ok(Value::Undefined)
    });
    registry.register(Function, "getangledelta", |world, _, args| {
        let clip = clip_named(world, args, 0)?;
        let (start, end) = span(args, 1)?;
        Ok(Value::Float(wrap_degrees(
            clip.abs_delta_yaw(end) - clip.abs_delta_yaw(start),
        )))
    });
}
