use std::sync::Arc;

use crate::{
    AnimClip, DObjPoseRequest, HidePartBits, XAnimNodeDefinition, XAnimNodeId, XAnimNodeKind,
    XAnimNodeState, XAnimTreeDefinition, XAnimTreeError, XAnimTreeRuntime,
};
use anim_iw4::PartBits;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DObjModelDescriptor {
    pub model: String,
    pub parent_model: Option<u16>,
    pub attach_tag: Option<String>,
    pub ignore_collision: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DObjCompositionDescriptor {
    pub revision: u32,
    pub models: Vec<DObjModelDescriptor>,
}

impl DObjCompositionDescriptor {
    pub fn single(revision: u32, model: String) -> Self {
        Self {
            revision,
            models: vec![DObjModelDescriptor {
                model,
                parent_model: None,
                attach_tag: None,
                ignore_collision: false,
            }],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XAnimSemanticNodeKind {
    Blend,
    Additive,
    Leaf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct XAnimSemanticNode {
    pub parent: Option<XAnimNodeId>,
    pub kind: XAnimSemanticNodeKind,
    pub clip: Option<String>,
    pub parts: Option<PartBits>,
    pub state: XAnimNodeState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct XAnimTreeSnapshot {
    pub definition_revision: u32,
    pub state_revision: u32,
    pub nodes: Vec<XAnimSemanticNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DObjSemanticState {
    pub composition: DObjCompositionDescriptor,
    pub pose_revision: u32,
    pub tree: Option<XAnimTreeSnapshot>,
    pub requested_parts: Option<PartBits>,
    pub hide_part_bits: HidePartBits,
}

impl DObjSemanticState {
    pub fn bind_pose(model: String, composition_revision: u32, pose_revision: u32) -> Self {
        Self {
            composition: DObjCompositionDescriptor::single(composition_revision, model),
            pose_revision,
            tree: None,
            requested_parts: None,
            hide_part_bits: HidePartBits::default(),
        }
    }

    pub fn one_leaf(
        model: String,
        clip: String,
        composition_revision: u32,
        pose_revision: u32,
        time: f32,
    ) -> Self {
        Self {
            composition: DObjCompositionDescriptor::single(composition_revision, model),
            pose_revision,
            tree: Some(XAnimTreeSnapshot {
                definition_revision: 1,
                state_revision: pose_revision,
                nodes: vec![XAnimSemanticNode {
                    parent: None,
                    kind: XAnimSemanticNodeKind::Leaf,
                    clip: Some(clip),
                    parts: None,
                    state: XAnimNodeState {
                        time,
                        old_time: time,
                        weight: 1.0,
                        goal_weight: 1.0,
                        rate: 1.0,
                        ..XAnimNodeState::default()
                    },
                }],
            }),
            requested_parts: None,
            hide_part_bits: HidePartBits::default(),
        }
    }

    pub fn resolve_request(
        &self,
        mut resolve_clip: impl FnMut(&str) -> Option<Arc<AnimClip>>,
    ) -> Result<DObjPoseRequest, SemanticResolveError> {
        let tree = self
            .tree
            .as_ref()
            .map(|tree| tree.resolve(&mut resolve_clip))
            .transpose()?;
        Ok(DObjPoseRequest {
            tree,
            requested_parts: self.requested_parts,
            hide_part_bits: self.hide_part_bits,
        })
    }

    /// The tree advanced `dtime_seconds` past its snapshot: leaf times by
    /// their inherited rates, weights toward their goals (presentation between
    /// snapshots; the next snapshot replaces it). `None` without a tree, with
    /// nothing to advance or when a clip does not resolve.
    pub fn advanced(
        &self,
        dtime_seconds: f32,
        resolve_clip: impl FnMut(&str) -> Option<Arc<AnimClip>>,
    ) -> Option<Self> {
        let tree = self.tree.as_ref()?;
        if dtime_seconds <= 0.0 {
            return None;
        }
        let mut runtime = tree.resolve(resolve_clip).ok()?;
        runtime.update_inherited_rate(dtime_seconds).ok()?;
        let mut out = self.clone();
        let nodes = &mut out.tree.as_mut()?.nodes;
        for (node, state) in nodes.iter_mut().zip(runtime.states()) {
            node.state.time = state.time;
            node.state.weight = state.weight;
            node.state.goal_time = state.goal_time;
        }
        Some(out)
    }
}

impl XAnimTreeSnapshot {
    pub fn resolve(
        &self,
        mut resolve_clip: impl FnMut(&str) -> Option<Arc<AnimClip>>,
    ) -> Result<XAnimTreeRuntime, SemanticResolveError> {
        let mut definitions = Vec::with_capacity(self.nodes.len());
        for (node, semantic) in self.nodes.iter().enumerate() {
            let kind = match semantic.kind {
                XAnimSemanticNodeKind::Blend => XAnimNodeKind::Blend,
                XAnimSemanticNodeKind::Additive => XAnimNodeKind::Additive,
                XAnimSemanticNodeKind::Leaf => {
                    let name = semantic
                        .clip
                        .as_deref()
                        .ok_or(SemanticResolveError::LeafClipMissing { node })?;
                    let clip = resolve_clip(name).ok_or_else(|| {
                        SemanticResolveError::ClipUnavailable {
                            node,
                            clip: name.to_owned(),
                        }
                    })?;
                    XAnimNodeKind::Leaf {
                        clip,
                        parts: semantic.parts,
                    }
                }
            };
            definitions.push(XAnimNodeDefinition {
                parent: semantic.parent,
                kind,
            });
        }
        let definition = Arc::new(XAnimTreeDefinition::new(definitions)?);
        let mut runtime = XAnimTreeRuntime::new(definition);
        for (node, semantic) in self.nodes.iter().enumerate() {
            runtime.set_state(XAnimNodeId(node as u16), semantic.state)?;
        }
        Ok(runtime)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticResolveError {
    LeafClipMissing { node: usize },
    ClipUnavailable { node: usize, clip: String },
    Tree(XAnimTreeError),
}

impl From<XAnimTreeError> for SemanticResolveError {
    fn from(value: XAnimTreeError) -> Self {
        Self::Tree(value)
    }
}
