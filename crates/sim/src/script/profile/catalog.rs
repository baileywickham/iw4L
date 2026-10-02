use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Namespace {
    Function,
    Method,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    Script,
    Player,
    Entity,
    HudElem,
    ScriptMover,
    PlayerCommand,
    Helicopter,
    Vehicle,
    Actor,
    Sentient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub namespace: Namespace,
    pub name: &'static str,
    pub owner: Owner,
    pub developer: bool,
}
impl Builtin {
    pub const fn new(
        namespace: Namespace,
        name: &'static str,
        owner: Owner,
        developer: bool,
    ) -> Self {
        Self {
            namespace,
            name,
            owner,
            developer,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Catalog {
    realm: crate::script::Realm,
    names: BTreeMap<Namespace, BTreeMap<&'static str, Builtin>>,
    stub_natives: bool,
}
impl Catalog {
    pub fn iw4() -> Self {
        Self::from_list(crate::script::Realm::Iw4, super::iw4_catalog::IW4)
    }
    pub fn iw4sp() -> Self {
        Self::from_list(crate::script::Realm::Iw4Sp, super::iw4sp_catalog::IW4SP)
    }
    pub fn t5() -> Self {
        Self::from_list(crate::script::Realm::T5, super::t5_catalog::T5)
    }
    /// Unknown builtins compile and unbound natives install as stubs that fail per call.
    pub fn with_native_stubs(mut self, enabled: bool) -> Self {
        self.stub_natives = enabled;
        self
    }
    pub fn stub_natives(&self) -> bool {
        self.stub_natives
    }
    fn from_list(realm: crate::script::Realm, list: &[Builtin]) -> Self {
        let mut catalog = Self {
            realm,
            names: BTreeMap::new(),
            stub_natives: false,
        };
        for builtin in list {
            catalog.insert(builtin.clone());
        }
        catalog
    }
    pub fn realm(&self) -> crate::script::Realm {
        self.realm
    }
    fn insert(&mut self, builtin: Builtin) {
        self.names
            .entry(builtin.namespace)
            .or_default()
            .insert(builtin.name, builtin);
    }
    pub fn get(&self, namespace: Namespace, name: &str) -> Option<&Builtin> {
        self.names.get(&namespace)?.get(name)
    }
    pub(crate) fn stub(&self, namespace: Namespace, name: &str) -> Option<Builtin> {
        use std::sync::{Mutex, OnceLock};
        static NAMES: OnceLock<Mutex<std::collections::BTreeSet<&'static str>>> = OnceLock::new();
        if !self.stub_natives {
            return None;
        }
        let mut names = NAMES.get_or_init(Default::default).lock().ok()?;
        let name = match names.get(name) {
            Some(&name) => name,
            None => {
                let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
                names.insert(name);
                name
            }
        };
        Some(Builtin::new(namespace, name, Owner::Script, false))
    }
    pub fn iter(&self) -> impl Iterator<Item = &Builtin> {
        self.names.values().flat_map(BTreeMap::values)
    }
}
