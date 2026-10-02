#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimPathLink {
    pub to: u16,
    pub dist: f32,
    pub disconnect_count: u8,
    pub negotiation_link: u8,
    pub flags: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimPathNode {
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
    pub links: Vec<SimPathLink>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SimPathTreeNode {
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
pub struct SimPathGraph {
    pub nodes: Vec<SimPathNode>,
    pub chain_node_count: u32,
    pub chain_node_for_node: Vec<u16>,
    pub node_for_chain_node: Vec<u16>,
    pub vis: Vec<u8>,
    pub tree: Vec<SimPathTreeNode>,
    pub tree_roots: u32,
}

const NODE_TYPE_NAMES: [&str; 20] = [
    "BAD NODE",
    "Path",
    "Cover Stand",
    "Cover Crouch",
    "Cover Crouch Window",
    "Cover Prone",
    "Cover Right",
    "Cover Left",
    "Ambush",
    "Exposed",
    "Conceal Stand",
    "Conceal Crouch",
    "Conceal Prone",
    "Door",
    "Door Interior",
    "Scripted",
    "Begin",
    "End",
    "Turret",
    "Guard",
];

impl SimPathNode {
    pub fn type_name(&self) -> &'static str {
        NODE_TYPE_NAMES
            .get(self.node_type as usize)
            .copied()
            .unwrap_or(NODE_TYPE_NAMES[0])
    }

    pub fn angles(&self) -> [f32; 3] {
        [0.0, self.angle, 0.0]
    }

    pub fn key(&self, key: &str) -> Option<&str> {
        match key {
            "targetname" => Some(&self.targetname),
            "target" => Some(&self.target),
            "script_noteworthy" => Some(&self.script_noteworthy),
            "script_linkname" => Some(&self.script_linkname),
            "animscript" => Some(&self.animscript),
            _ => None,
        }
    }
}

impl SimPathGraph {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn matching(&self, value: &str, key: &str) -> Result<Vec<usize>, String> {
        if SimPathNode::default().key(key).is_none() {
            return Err(format!("key '{key}' is not a node string field"));
        }
        Ok(self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !value.is_empty() && n.key(key) == Some(value))
            .map(|(i, _)| i)
            .collect())
    }
}
