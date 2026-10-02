use std::collections::HashMap;

use asset_iw4::size as sz;
use fastfile_iw4::{Ptr, ScriptStrings, ZonePtr, ZoneStream};

const TREE_DEPTH_CAP: usize = 64;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathLink {
    pub to: u16,
    pub dist: f32,
    pub disconnect_count: u8,
    pub negotiation_link: u8,
    pub flags: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathNode {
    pub node_type: u32,
    pub spawnflags: u16,
    pub targetname: String,
    pub target: String,
    pub script_noteworthy: String,
    pub script_linkname: String,
    pub animscript: String,
    pub origin: [f32; 3],
    pub angle: f32,
    pub forward: [f32; 2],
    pub radius: f32,
    pub min_use_dist_sq: f32,
    pub overlap_nodes: [i16; 2],
    pub links: Vec<PathLink>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathTreeNode {
    Split {
        axis: i32,
        dist: f32,
        children: [Option<u32>; 2],
    },
    Leaf {
        nodes: Vec<u16>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathData {
    pub nodes: Vec<PathNode>,
    pub chain_node_count: u32,
    pub chain_node_for_node: Vec<u16>,
    pub node_for_chain_node: Vec<u16>,
    pub vis: Vec<u8>,
    pub tree: Vec<PathTreeNode>,
    pub tree_roots: u32,
}

impl PathData {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn link_count(&self) -> usize {
        self.nodes.iter().map(|n| n.links.len()).sum()
    }

    pub fn report_line(&self) -> String {
        format!(
            "pathdata: nodes={} links={} tree={} chain={} vis={}",
            self.nodes.len(),
            self.link_count(),
            self.tree.len(),
            self.chain_node_count,
            self.vis.len()
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleTrackObstacle {
    pub origin: [f32; 2],
    pub radius: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleTrackSector {
    pub start_edge_dir: [f32; 2],
    pub start_edge_dist: f32,
    pub left_edge_dir: [f32; 2],
    pub left_edge_dist: f32,
    pub right_edge_dir: [f32; 2],
    pub right_edge_dist: f32,
    pub sector_length: f32,
    pub sector_width: f32,
    pub total_prior_length: f32,
    pub total_following_length: f32,
    pub obstacles: Vec<VehicleTrackObstacle>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleTrackSegment {
    pub target_name: String,
    pub sectors: Vec<VehicleTrackSector>,
    pub next_branches: Vec<u32>,
    pub prev_branches: Vec<u32>,
    pub end_edge_dir: [f32; 2],
    pub end_edge_dist: f32,
    pub total_length: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleTrack {
    pub segments: Vec<VehicleTrackSegment>,
}

impl VehicleTrack {
    pub fn report_line(&self) -> String {
        format!(
            "vehicletrack: segments={} sectors={}",
            self.segments.len(),
            self.segments.iter().map(|s| s.sectors.len()).sum::<usize>()
        )
    }
}

pub fn addon_map_ents_entity_string<'a>(s: &'a ZoneStream<'_>) -> Option<&'a str> {
    let ents = s.addon_map_ents()?;
    let bytes = s.slice_at(ents.entity_string?, 0, ents.entity_chars).ok()?;
    let text = core::str::from_utf8(bytes).ok()?;
    Some(text.trim_end_matches('\0'))
}

fn offset(s: &ZoneStream<'_>, p: Ptr, field: usize) -> Option<Ptr> {
    match s.ptr_at(p, field).ok()? {
        ZonePtr::Offset(q) => Some(s.resolve_alias(q)),
        _ => None,
    }
}

fn f32s<const N: usize>(s: &ZoneStream<'_>, p: Ptr, off: usize) -> [f32; N] {
    std::array::from_fn(|i| s.f32_at(p, off + i * 4).unwrap_or(0.0))
}

fn u16s(s: &ZoneStream<'_>, p: Option<Ptr>, count: usize) -> Vec<u16> {
    let Some(p) = p else {
        return Vec::new();
    };
    (0..count).map_while(|i| s.u16_at(p, i * 2).ok()).collect()
}

fn script_string(s: &ZoneStream<'_>, strings: &ScriptStrings, p: Ptr, off: usize) -> String {
    match s.u16_at(p, off) {
        Ok(0) | Err(_) => String::new(),
        Ok(i) => strings.get(s, i).unwrap_or("").to_owned(),
    }
}

pub fn build_path_data(s: &ZoneStream<'_>, strings: &ScriptStrings) -> Option<PathData> {
    let g = s.path_data()?;
    let node_stride = s.layout(sz::PATH_NODE, 168);
    let link_field = s.layout(60, 64);
    let mut nodes = Vec::with_capacity(g.node_count);
    if let Some(base) = g.nodes {
        for i in 0..g.node_count {
            let n = base.at(i * node_stride);
            let link_count = s.u16_at(n, 56).unwrap_or(0) as usize;
            let links = match offset(s, n, link_field) {
                Some(l) => (0..link_count)
                    .map(|k| {
                        let l = l.at(k * sz::PATH_LINK);
                        PathLink {
                            dist: s.f32_at(l, 0).unwrap_or(0.0),
                            to: s.u16_at(l, 4).unwrap_or(0),
                            disconnect_count: s.u8_at(l, 6).unwrap_or(0),
                            negotiation_link: s.u8_at(l, 7).unwrap_or(0),
                            flags: s.u8_at(l, 8).unwrap_or(0),
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            nodes.push(PathNode {
                node_type: s.u32_at(n, 0).unwrap_or(0),
                spawnflags: s.u16_at(n, 4).unwrap_or(0),
                targetname: script_string(s, strings, n, 6),
                script_linkname: script_string(s, strings, n, 8),
                script_noteworthy: script_string(s, strings, n, 10),
                target: script_string(s, strings, n, 12),
                animscript: script_string(s, strings, n, 14),
                origin: f32s(s, n, 20),
                angle: s.f32_at(n, 32).unwrap_or(0.0),
                forward: f32s(s, n, 36),
                radius: s.f32_at(n, 44).unwrap_or(0.0),
                min_use_dist_sq: s.f32_at(n, 48).unwrap_or(0.0),
                overlap_nodes: [s.i16_at(n, 52).unwrap_or(-1), s.i16_at(n, 54).unwrap_or(-1)],
                links,
            });
        }
    }

    let vis = match g.vis {
        Some(p) => s
            .slice_at(p, 0, g.vis_bytes)
            .map_or_else(|_| Vec::new(), <[u8]>::to_vec),
        None => Vec::new(),
    };

    let mut tree = TreeBuild {
        stride: s.layout(sz::PATHNODE_TREE, 24),
        index: HashMap::new(),
        out: Vec::new(),
    };
    if let Some(base) = g.tree {
        for i in 0..g.tree_count {
            let p = base.at(i * tree.stride);
            tree.index.insert(p, i as u32);
            tree.out.push(PathTreeNode::Leaf { nodes: Vec::new() });
        }
        for i in 0..g.tree_count {
            tree.fill(s, base.at(i * tree.stride), i as u32, 0);
        }
    }

    Some(PathData {
        nodes,
        chain_node_count: g.chain_node_count as u32,
        chain_node_for_node: u16s(s, g.chain_node_for_node, g.node_count),
        node_for_chain_node: u16s(s, g.node_for_chain_node, g.node_count),
        vis,
        tree: tree.out,
        tree_roots: g.tree_count as u32,
    })
}

struct TreeBuild {
    stride: usize,
    index: HashMap<Ptr, u32>,
    out: Vec<PathTreeNode>,
}

impl TreeBuild {
    fn fill(&mut self, s: &ZoneStream<'_>, t: Ptr, slot: u32, depth: usize) {
        let axis = s.i32_at(t, 0).unwrap_or(-1);
        let union_off = 8;
        let node = if axis >= 0 {
            let width = s.pointer_bytes();
            let mut children = [None; 2];
            for (k, child) in children.iter_mut().enumerate() {
                *child = offset(s, t, union_off + k * width).and_then(|c| self.child(s, c, depth));
            }
            PathTreeNode::Split {
                axis,
                dist: s.f32_at(t, 4).unwrap_or(0.0),
                children,
            }
        } else {
            let count = s.i32_at(t, union_off).unwrap_or(0).max(0) as usize;
            PathTreeNode::Leaf {
                nodes: u16s(s, offset(s, t, s.layout(12, 16)), count),
            }
        };
        self.out[slot as usize] = node;
    }

    fn child(&mut self, s: &ZoneStream<'_>, c: Ptr, depth: usize) -> Option<u32> {
        if let Some(&i) = self.index.get(&c) {
            return Some(i);
        }
        if depth >= TREE_DEPTH_CAP {
            return None;
        }
        let i = self.out.len() as u32;
        self.index.insert(c, i);
        self.out.push(PathTreeNode::Leaf { nodes: Vec::new() });
        self.fill(s, c, i, depth + 1);
        Some(i)
    }
}

pub fn build_vehicle_track(s: &ZoneStream<'_>) -> Option<VehicleTrack> {
    let g = s.vehicle_track()?;
    let stride = s.layout(sz::VEHICLE_SEGMENT, 72);
    let sector_stride = s.layout(sz::VEHICLE_SECTOR, 72);
    let Some(base) = g.segments else {
        return Some(VehicleTrack::default());
    };
    let index_of = |p: Ptr| -> Option<u32> {
        let delta = p.offset.checked_sub(base.offset)? as usize;
        (p.block == base.block && delta.is_multiple_of(stride) && delta / stride < g.segment_count)
            .then_some((delta / stride) as u32)
    };
    let branches = |seg: Ptr, field: usize, count_off: usize| -> Vec<u32> {
        let count = s.u32_at(seg, count_off).unwrap_or(0) as usize;
        let Some(arr) = offset(s, seg, field) else {
            return Vec::new();
        };
        (0..count)
            .filter_map(|k| offset(s, arr, k * s.pointer_bytes()).and_then(index_of))
            .collect()
    };
    let segments = (0..g.segment_count)
        .map(|i| {
            let seg = base.at(i * stride);
            let sector_count = s.u32_at(seg, s.layout(8, 16)).unwrap_or(0) as usize;
            let sectors = match offset(s, seg, s.layout(4, 8)) {
                Some(arr) => (0..sector_count)
                    .map(|k| {
                        let sec = arr.at(k * sector_stride);
                        let obstacle_count = s.u32_at(sec, s.layout(56, 64)).unwrap_or(0) as usize;
                        let obstacles = match offset(s, sec, s.layout(52, 56)) {
                            Some(o) => (0..obstacle_count)
                                .map(|j| {
                                    let o = o.at(j * sz::VEHICLE_OBSTACLE);
                                    VehicleTrackObstacle {
                                        origin: f32s(s, o, 0),
                                        radius: s.f32_at(o, 8).unwrap_or(0.0),
                                    }
                                })
                                .collect(),
                            None => Vec::new(),
                        };
                        VehicleTrackSector {
                            start_edge_dir: f32s(s, sec, 0),
                            start_edge_dist: s.f32_at(sec, 8).unwrap_or(0.0),
                            left_edge_dir: f32s(s, sec, 12),
                            left_edge_dist: s.f32_at(sec, 20).unwrap_or(0.0),
                            right_edge_dir: f32s(s, sec, 24),
                            right_edge_dist: s.f32_at(sec, 32).unwrap_or(0.0),
                            sector_length: s.f32_at(sec, 36).unwrap_or(0.0),
                            sector_width: s.f32_at(sec, 40).unwrap_or(0.0),
                            total_prior_length: s.f32_at(sec, 44).unwrap_or(0.0),
                            total_following_length: s.f32_at(sec, 48).unwrap_or(0.0),
                            obstacles,
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            VehicleTrackSegment {
                target_name: offset(s, seg, 0)
                    .and_then(|p| s.cstr(p).ok())
                    .unwrap_or("")
                    .to_owned(),
                sectors,
                next_branches: branches(seg, s.layout(12, 24), s.layout(16, 32)),
                prev_branches: branches(seg, s.layout(20, 40), s.layout(24, 48)),
                end_edge_dir: f32s(s, seg, s.layout(28, 52)),
                end_edge_dist: s.f32_at(seg, s.layout(36, 60)).unwrap_or(0.0),
                total_length: s.f32_at(seg, s.layout(40, 64)).unwrap_or(0.0),
            }
        })
        .collect();
    Some(VehicleTrack { segments })
}
