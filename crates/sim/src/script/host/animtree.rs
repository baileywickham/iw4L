use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use xmodel_runtime::{
    AnimClip, XAnimNodeDefinition, XAnimNodeId, XAnimNodeKind, XAnimTreeDefinition,
};

pub const ANIMTREE_ADDITIVE: u8 = 1;
pub const ANIMTREE_LOOPSYNC: u8 = 2;
pub const ANIMTREE_NONLOOPSYNC: u8 = 4;
pub const ANIMTREE_COMPLETE: u8 = 8;

pub type ScriptXAnimSource = Arc<dyn Fn(&str) -> Option<Arc<AnimClip>> + Send + Sync>;

/// The `animtrees/*.atr` captured from the zones plus the xanims they name.
/// Trees are built on their first `useanimtree`, so a map that never animates
/// a script entity decodes no clips.
#[derive(Default)]
pub struct ScriptAnimLibrary {
    texts: BTreeMap<String, String>,
    clips: Option<ScriptXAnimSource>,
    trees: Mutex<BTreeMap<String, Result<Arc<ScriptAnimTree>, String>>>,
}

impl std::fmt::Debug for ScriptAnimLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptAnimLibrary")
            .field("trees", &self.texts.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl ScriptAnimLibrary {
    pub fn new(
        texts: impl IntoIterator<Item = (String, String)>,
        clips: ScriptXAnimSource,
    ) -> Self {
        Self {
            texts: texts
                .into_iter()
                .map(|(name, text)| (name.to_ascii_lowercase(), text))
                .collect(),
            clips: Some(clips),
            trees: Mutex::default(),
        }
    }

    pub fn tree_names(&self) -> impl Iterator<Item = &str> {
        self.texts.keys().map(String::as_str)
    }

    pub(crate) fn clip(&self, name: &str) -> Option<Arc<AnimClip>> {
        self.clips.as_ref().and_then(|clips| clips(name))
    }

    pub(crate) fn tree(&self, name: &str) -> Result<Arc<ScriptAnimTree>, String> {
        let key = name.to_ascii_lowercase();
        let mut trees = self.trees.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tree) = trees.get(&key) {
            return tree.clone();
        }
        let built = match self.texts.get(&key) {
            Some(text) => ScriptAnimTree::build(&key, text, |clip| self.clip(clip)),
            None => Err(format!("animtree {name} is not loaded")),
        };
        match &built {
            Ok(tree) => diag::info!(
                Sim,
                "gsc: animtree {key} nodes={} leaves={} missing_xanims={} duplicates={}",
                tree.nodes.len(),
                tree.leaves,
                tree.missing,
                tree.duplicates
            ),
            Err(error) => diag::warn!(Sim, "gsc: animtree {key}: {error}"),
        }
        trees.insert(key, built.clone());
        built
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScriptAnimNode {
    pub name: Arc<str>,
    pub parent: Option<u16>,
    pub children: Range<u16>,
    pub flags: u8,
}

#[derive(Debug)]
pub(crate) struct ScriptAnimTree {
    pub name: Arc<str>,
    pub nodes: Vec<ScriptAnimNode>,
    pub definition: Arc<XAnimTreeDefinition>,
    index: HashMap<String, u16>,
    leaves: usize,
    missing: usize,
    duplicates: usize,
}

impl ScriptAnimTree {
    pub(crate) fn node(&self, name: &str) -> Option<u16> {
        self.index.get(&name.to_ascii_lowercase()).copied()
    }

    pub(crate) fn clip(&self, node: u16) -> Option<&Arc<AnimClip>> {
        match &self.definition.nodes().get(node as usize)?.kind {
            XAnimNodeKind::Leaf { clip, .. } => Some(clip),
            _ => None,
        }
    }

    pub(crate) fn is_under(&self, node: u16, ancestor: u16) -> bool {
        let mut at = Some(node);
        while let Some(id) = at {
            if id == ancestor {
                return true;
            }
            at = self.nodes[id as usize].parent;
        }
        false
    }

    fn build(
        name: &str,
        text: &str,
        clip: impl Fn(&str) -> Option<Arc<AnimClip>>,
    ) -> Result<Arc<Self>, String> {
        let top = parse_atr(text)?;
        let mut nodes = vec![ScriptAnimNode {
            name: "root".into(),
            parent: None,
            children: 0..0,
            flags: 0,
        }];
        let mut queue = VecDeque::from([(0u16, top)]);
        while let Some((parent, entries)) = queue.pop_front() {
            let first = nodes.len();
            for entry in entries {
                let id = u16::try_from(nodes.len()).map_err(|_| "animtree has too many nodes")?;
                nodes.push(ScriptAnimNode {
                    name: entry.name.into(),
                    parent: Some(parent),
                    children: 0..0,
                    flags: entry.flags,
                });
                queue.push_back((id, entry.children));
            }
            nodes[parent as usize].children = first as u16..nodes.len() as u16;
        }
        let mut index = HashMap::with_capacity(nodes.len());
        let mut duplicates = 0;
        for (id, node) in nodes.iter().enumerate() {
            match index.entry(node.name.to_ascii_lowercase()) {
                std::collections::hash_map::Entry::Occupied(_) => duplicates += 1,
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(id as u16);
                }
            }
        }
        let (mut leaves, mut missing) = (0, 0);
        let definition = nodes
            .iter()
            .map(|node| XAnimNodeDefinition {
                parent: node.parent.map(XAnimNodeId),
                kind: if !node.children.is_empty() {
                    if node.flags & ANIMTREE_ADDITIVE != 0 {
                        XAnimNodeKind::Additive
                    } else {
                        XAnimNodeKind::Blend
                    }
                } else if node.parent.is_none() {
                    XAnimNodeKind::Blend
                } else {
                    leaves += 1;
                    match clip(&node.name) {
                        Some(clip) => XAnimNodeKind::Leaf { clip, parts: None },
                        None => {
                            missing += 1;
                            XAnimNodeKind::Blend
                        }
                    }
                },
            })
            .collect();
        let definition = XAnimTreeDefinition::new(definition).map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            name: name.into(),
            nodes,
            definition: Arc::new(definition),
            index,
            leaves,
            missing,
            duplicates,
        }))
    }
}

struct AtrEntry {
    name: String,
    flags: u8,
    children: Vec<AtrEntry>,
}

enum AtrToken {
    Name(String),
    Colon,
    Open,
    Close,
}

fn atr_tokens(text: &str) -> Vec<(AtrToken, usize)> {
    let bytes = text.as_bytes();
    let (mut at, mut line, mut tokens) = (0, 0, Vec::new());
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                line += 1;
                at += 1;
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at < bytes.len() && !(bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/')) {
                    line += usize::from(bytes[at] == b'\n');
                    at += 1;
                }
                at += 2;
            }
            b'{' | b'}' | b':' => {
                tokens.push((
                    match bytes[at] {
                        b'{' => AtrToken::Open,
                        b'}' => AtrToken::Close,
                        _ => AtrToken::Colon,
                    },
                    line,
                ));
                at += 1;
            }
            c if c.is_ascii_whitespace() => at += 1,
            _ => {
                let start = at;
                while at < bytes.len()
                    && !bytes[at].is_ascii_whitespace()
                    && !matches!(bytes[at], b'{' | b'}' | b':')
                    && !(bytes[at] == b'/' && matches!(bytes.get(at + 1), Some(b'/' | b'*')))
                {
                    at += 1;
                }
                tokens.push((AtrToken::Name(text[start..at].to_owned()), line));
            }
        }
    }
    tokens
}

fn parse_atr(text: &str) -> Result<Vec<AtrEntry>, String> {
    let mut levels: Vec<Vec<AtrEntry>> = vec![Vec::new()];
    let mut tokens = atr_tokens(text).into_iter().peekable();
    while let Some((token, line)) = tokens.next() {
        match token {
            AtrToken::Name(name) => {
                let mut flags = 0;
                if matches!(tokens.peek(), Some((AtrToken::Colon, at)) if *at == line) {
                    tokens.next();
                    while let Some((AtrToken::Name(flag), at)) = tokens.peek() {
                        if *at != line {
                            break;
                        }
                        flags |= match flag.to_ascii_lowercase().as_str() {
                            "additive" => ANIMTREE_ADDITIVE,
                            "loopsync" => ANIMTREE_LOOPSYNC,
                            "nonloopsync" => ANIMTREE_NONLOOPSYNC,
                            "complete" => ANIMTREE_COMPLETE,
                            other => {
                                return Err(format!("line {}: unknown flag {other}", line + 1));
                            }
                        };
                        tokens.next();
                    }
                }
                levels.last_mut().unwrap().push(AtrEntry {
                    name,
                    flags,
                    children: Vec::new(),
                });
            }
            AtrToken::Open => {
                if levels.last().unwrap().is_empty() {
                    return Err(format!("line {}: '{{' without a node", line + 1));
                }
                levels.push(Vec::new());
            }
            AtrToken::Close => {
                let children = levels.pop().unwrap();
                let Some(parent) = levels.last_mut().and_then(|level| level.last_mut()) else {
                    return Err(format!("line {}: unbalanced '}}'", line + 1));
                };
                parent.children = children;
            }
            AtrToken::Colon => return Err(format!("line {}: ':' without a node", line + 1)),
        }
    }
    match levels.len() {
        1 => Ok(levels.pop().unwrap()),
        _ => Err("unbalanced '{'".into()),
    }
}
