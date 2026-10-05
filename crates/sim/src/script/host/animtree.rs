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
/// `prewarm` builds the trees (and decodes their clips) on a background
/// thread while the level loads; a tree asked for before that finishes is
/// waited for, or built on the spot when nothing is building it. Building is
/// a pure function of the texts and clips, so who builds it does not matter.
#[derive(Default)]
pub struct ScriptAnimLibrary {
    texts: BTreeMap<String, String>,
    clips: Option<ScriptXAnimSource>,
    trees: Mutex<Trees>,
    built: std::sync::Condvar,
}

#[derive(Default)]
struct Trees {
    done: BTreeMap<String, Result<Arc<ScriptAnimTree>, String>>,
    building: std::collections::BTreeSet<String>,
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
            built: std::sync::Condvar::new(),
        }
    }

    /// Builds every tree off the calling thread, largest first. The first
    /// `useanimtree` of `generic_human` decoded ~3,500 clips inside a script
    /// frame (50–200 ms).
    ///
    /// Only levels with actors (`generic_human`, single player) prewarm: a
    /// multiplayer level keeps building on first use and decodes nothing it
    /// never animates.
    pub fn prewarm(library: &Arc<Self>) {
        if !library.texts.contains_key("generic_human") || library.clips.is_none() {
            return;
        }
        let library = Arc::clone(library);
        let spawned = std::thread::Builder::new()
            .name("animtree-prewarm".into())
            .spawn(move || {
                let mut names: Vec<&String> = library.texts.keys().collect();
                names.sort_by_key(|name| std::cmp::Reverse(library.texts[*name].len()));
                for name in names {
                    {
                        let mut trees = library.lock();
                        if trees.done.contains_key(name) || !trees.building.insert(name.clone()) {
                            continue;
                        }
                    }
                    let built = library.build(name);
                    let mut trees = library.lock();
                    trees.building.remove(name);
                    trees.done.entry(name.clone()).or_insert(built);
                    library.built.notify_all();
                }
            });
        if let Err(error) = spawned {
            diag::warn!(Sim, "gsc: animtree prewarm thread: {error}");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Trees> {
        self.trees.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn build(&self, key: &str) -> Result<Arc<ScriptAnimTree>, String> {
        let started = std::time::Instant::now();
        let built = match self.texts.get(key) {
            Some(text) => ScriptAnimTree::build(key, text, |clip| self.clip(clip)),
            None => Err(format!("animtree {key} is not loaded")),
        };
        match &built {
            Ok(tree) => diag::info!(
                Sim,
                "gsc: animtree {key} nodes={} leaves={} missing_xanims={} duplicates={} clip_bytes~{} built_in={}ms thread={}",
                tree.nodes.len(),
                tree.leaves,
                tree.missing,
                tree.duplicates,
                tree.clip_bytes(),
                started.elapsed().as_millis(),
                std::thread::current().name().unwrap_or("?")
            ),
            Err(error) => diag::warn!(Sim, "gsc: animtree {key}: {error}"),
        }
        built
    }

    pub fn tree_names(&self) -> impl Iterator<Item = &str> {
        self.texts.keys().map(String::as_str)
    }

    pub(crate) fn clip(&self, name: &str) -> Option<Arc<AnimClip>> {
        self.clips.as_ref().and_then(|clips| clips(name))
    }

    pub(crate) fn tree(&self, name: &str) -> Result<Arc<ScriptAnimTree>, String> {
        let key = name.to_ascii_lowercase();
        let mut trees = self.lock();
        loop {
            if let Some(tree) = trees.done.get(&key) {
                return tree.clone();
            }
            if !trees.building.contains(&key) {
                break;
            }
            trees = self.built.wait(trees).unwrap_or_else(|p| p.into_inner());
        }
        trees.building.insert(key.clone());
        drop(trees);
        let built = self.build(&key);
        let mut trees = self.lock();
        trees.building.remove(&key);
        let tree = trees.done.entry(key).or_insert(built).clone();
        self.built.notify_all();
        tree
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

    /// Rough decoded size of the distinct clips the leaves hold.
    fn clip_bytes(&self) -> usize {
        use xmodel_runtime::{FrameIndices, Keyed, Rotation, Translation};
        fn keyed<T>(keyed: &Keyed<T>) -> usize {
            keyed.values.len() * std::mem::size_of::<T>()
                + match &keyed.frames {
                    FrameIndices::Dense => 0,
                    FrameIndices::Sparse(frames) => frames.len() * 2,
                }
        }
        let mut seen = std::collections::HashSet::new();
        let mut bytes = 0;
        for node in self.definition.nodes() {
            let XAnimNodeKind::Leaf { clip, .. } = &node.kind else {
                continue;
            };
            if !seen.insert(Arc::as_ptr(clip)) {
                continue;
            }
            bytes += std::mem::size_of::<AnimClip>();
            for track in &clip.tracks {
                bytes += std::mem::size_of_val(track) + track.name.len();
                bytes += match &track.rotation {
                    Rotation::HalfKeyed(k) | Rotation::FullKeyed(k) => keyed(k),
                    _ => 0,
                };
                bytes += match &track.translation {
                    Translation::SmallKeyed(k) | Translation::FullKeyed(k) => keyed(k),
                    _ => 0,
                };
            }
        }
        bytes
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
        let mut missing_names = Vec::new();
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
                            missing_names.push(node.name.as_ref());
                            XAnimNodeKind::Blend
                        }
                    }
                },
            })
            .collect();
        let definition = XAnimTreeDefinition::new(definition).map_err(|e| e.to_string())?;
        if std::env::var("IW4L_ANIMTREE_MISSING").is_ok_and(|v| v == "1") {
            for chunk in missing_names.chunks(64) {
                diag::info!(Sim, "gsc: animtree {name} missing: {}", chunk.join(" "));
            }
        }
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
