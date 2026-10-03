//! Path nodes for actors: cylinder queries through the node tree, a resumable
//! A* over node links, and the path an actor follows.

use crate::{SimPathGraph, SimPathTreeNode};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// `Path_NearestNode`'s search radius for a sentient.
pub(crate) const NEAREST_NODE_DIST: f32 = 192.0;
/// Node type `Begin`: the start of a negotiation (traverse) link.
const NODE_BEGIN: u32 = 16;
const PNF_DONTLINK: u16 = 1;
const NO_NODE: u16 = u16::MAX;

fn dist2_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    dist2_sq(a, b) + (a[2] - b[2]).powi(2)
}

/// `Path_NodesInCylinder` with the "any type but BAD, no DONTLINK" filter, closest first.
pub(crate) fn nodes_in_cylinder(
    graph: &SimPathGraph,
    origin: [f32; 3],
    radius: f32,
    height: f32,
) -> Vec<u16> {
    let mut found = Vec::new();
    let mut take = |index: u16| {
        let Some(node) = graph.nodes.get(index as usize) else {
            return;
        };
        let d = dist2_sq(node.origin, origin);
        if d <= radius * radius
            && node.node_type != 0
            && node.spawnflags & PNF_DONTLINK == 0
            && (node.origin[2] - origin[2]).powi(2) <= height * height
        {
            found.push((d, index));
        }
    };
    if graph.tree.is_empty() {
        (0..graph.nodes.len()).for_each(|i| take(i as u16));
    } else {
        let mut stack = vec![0u32];
        while let Some(mut at) = stack.pop() {
            loop {
                match graph.tree.get(at as usize) {
                    Some(SimPathTreeNode::Split {
                        axis,
                        dist,
                        children,
                    }) => {
                        let d = origin[(*axis as usize).min(2)] - dist;
                        let [low, high] = *children;
                        let next = if radius < d {
                            high
                        } else if -radius <= d {
                            if let Some(low) = low {
                                stack.push(low);
                            }
                            high
                        } else {
                            low
                        };
                        match next {
                            Some(next) => at = next,
                            None => break,
                        }
                    }
                    Some(SimPathTreeNode::Leaf { nodes }) => {
                        nodes.iter().for_each(|n| take(*n));
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    found.into_iter().map(|(_, i)| i).collect()
}

#[derive(Clone, Copy, Debug)]
struct Open {
    estimate: f32,
    node: u16,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then(other.node.cmp(&self.node))
    }
}

pub(crate) enum SearchStep {
    Pending,
    Found(Vec<u16>),
    Failed,
}

/// A* from one node to another over links that are not disconnected. Keeps its
/// frontier between calls, so a tick that grants no budget repeats nothing.
#[derive(Clone, Debug)]
pub(crate) struct PathSearch {
    pub from: u16,
    pub to: u16,
    open: BinaryHeap<Open>,
    cost: Vec<f32>,
    parent: Vec<u16>,
    pub expanded: u32,
}

impl PathSearch {
    pub(crate) fn new(graph: &SimPathGraph, from: u16, to: u16) -> Self {
        let n = graph.nodes.len();
        let mut cost = vec![f32::INFINITY; n];
        cost[from as usize] = 0.0;
        let mut open = BinaryHeap::new();
        open.push(Open {
            estimate: dist_sq(
                graph.nodes[from as usize].origin,
                graph.nodes[to as usize].origin,
            )
            .sqrt(),
            node: from,
        });
        Self {
            from,
            to,
            open,
            cost,
            parent: vec![NO_NODE; n],
            expanded: 0,
        }
    }

    pub(crate) fn run(&mut self, graph: &SimPathGraph, budget: &mut u32) -> SearchStep {
        let goal = graph.nodes[self.to as usize].origin;
        while *budget > 0 {
            let Some(Open { estimate, node }) = self.open.pop() else {
                return SearchStep::Failed;
            };
            let here = self.cost[node as usize];
            let origin = graph.nodes[node as usize].origin;
            if estimate > here + dist_sq(origin, goal).sqrt() + 0.01 {
                continue;
            }
            *budget -= 1;
            self.expanded += 1;
            if node == self.to {
                let mut nodes = vec![node];
                let mut at = node;
                while at != self.from {
                    at = self.parent[at as usize];
                    nodes.push(at);
                }
                nodes.reverse();
                return SearchStep::Found(nodes);
            }
            for link in &graph.nodes[node as usize].links {
                if link.disconnect_count > 0 {
                    continue;
                }
                let Some(next) = graph.nodes.get(link.to as usize) else {
                    continue;
                };
                let cost = here + link.dist.max(1.0);
                if cost < self.cost[link.to as usize] {
                    self.cost[link.to as usize] = cost;
                    self.parent[link.to as usize] = node;
                    self.open.push(Open {
                        estimate: cost + dist_sq(next.origin, goal).sqrt(),
                        node: link.to,
                    });
                }
            }
        }
        SearchStep::Pending
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PathPoint {
    pub pos: [f32; 3],
    pub node: Option<u16>,
    /// The segment into this point is a negotiation link (`animscripts/traverse`).
    pub traverse: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ActorPath {
    pub points: Vec<PathPoint>,
    pub next: usize,
    pub final_goal: [f32; 3],
    pub nodes: usize,
}

impl ActorPath {
    pub(crate) fn direct(goal: [f32; 3]) -> Self {
        Self {
            points: vec![PathPoint {
                pos: goal,
                node: None,
                traverse: false,
            }],
            next: 0,
            final_goal: goal,
            nodes: 0,
        }
    }

    /// Node origins along `nodes`, then the goal when it is not the last node.
    pub(crate) fn through(graph: &SimPathGraph, nodes: &[u16], goal: [f32; 3]) -> Self {
        let mut points: Vec<PathPoint> = Vec::with_capacity(nodes.len() + 1);
        for (i, &n) in nodes.iter().enumerate() {
            let traverse = i > 0 && graph.nodes[nodes[i - 1] as usize].node_type == NODE_BEGIN;
            points.push(PathPoint {
                pos: graph.nodes[n as usize].origin,
                node: Some(n),
                traverse,
            });
        }
        if points.last().is_none_or(|p| dist_sq(p.pos, goal) > 1.0) {
            points.push(PathPoint {
                pos: goal,
                node: None,
                traverse: false,
            });
        }
        Self {
            points,
            next: 0,
            final_goal: goal,
            nodes: nodes.len(),
        }
    }

    pub(crate) fn current(&self) -> Option<&PathPoint> {
        self.points.get(self.next)
    }

    /// Whether the actor may skip straight from here to the point after `next`.
    pub(crate) fn may_skip(&self) -> bool {
        self.points
            .get(self.next + 1)
            .is_some_and(|after| !after.traverse && !self.points[self.next].traverse)
    }

    /// Path length left from `from`.
    pub(crate) fn remaining(&self, from: [f32; 3]) -> f32 {
        let mut at = from;
        let mut total = 0.0;
        for p in &self.points[self.next.min(self.points.len())..] {
            total += dist_sq(at, p.pos).sqrt();
            at = p.pos;
        }
        total
    }
}
