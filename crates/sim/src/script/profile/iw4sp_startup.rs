use crate::script::{Catalog, Fault, Program, SourceResolver};

/// A module `iw4sp.exe` runs by a name it builds rather than one a script
/// references: `maps/%s`, `codescripts/delete`, `codescripts/struct`,
/// `aitype/%s` (`main`/`spawner`/`precache` per actor classname), and the
/// animscripts: `animscripts/%s` per AI state (`cover_left`, `reactions`, …),
/// `animscripts/%s/%s` per species (`dog/dog_*`, `civilian/civilian_*`),
/// `animscripts/traverse/%s` from path nodes and a turret weapon's `script`
/// (`saw/stand`, `hummer_turret/minigun_stand`). The zones carry only the
/// animscripts their levels use, so every one of them counts.
pub fn engine_named(map: &str, module: &str) -> bool {
    module.strip_prefix("maps/") == Some(map)
        || module == "codescripts/delete"
        || module == "codescripts/struct"
        || module.starts_with("aitype/")
        || module.starts_with("animscripts/")
}

pub struct Iw4SpStartup {
    pub roots: Vec<String>,
    pub entries: Vec<String>,
    /// Engine-named roots left out because they do not compile.
    pub dropped: Vec<Fault>,
    map: String,
}
impl Iw4SpStartup {
    /// `modules` is every script the zones carry; the ones the engine runs by
    /// name are roots, since nothing in script references them.
    pub fn new<'a>(map: &str, modules: impl Iterator<Item = &'a str>) -> Self {
        let mut roots = vec![
            "codescripts/delete".to_owned(),
            "codescripts/struct".to_owned(),
            format!("maps/{map}"),
        ];
        for module in modules {
            if engine_named(map, module) && !roots.iter().any(|root| root == module) {
                roots.push(module.to_owned());
            }
        }
        let map = format!("maps/{map}");
        Self {
            roots,
            entries: vec![format!("{map}::main")],
            dropped: Vec::new(),
            map,
        }
    }

    /// Compiles the roots. An engine-named root the zones ship broken (unused by
    /// the game, e.g. `traverse/stairs_down`) is dropped.
    pub fn load(
        &mut self,
        resolver: &impl SourceResolver,
        catalog: &Catalog,
    ) -> Result<Program, Fault> {
        loop {
            let roots: Vec<&str> = self.roots.iter().map(String::as_str).collect();
            match Program::load(resolver, &roots, catalog) {
                Err(e)
                    if e.location.module != self.map
                        && !e.location.module.starts_with("codescripts/")
                        && self.roots.contains(&e.location.module) =>
                {
                    diag::warn!(Sim, "gsc: engine-named script dropped: {e}");
                    self.roots.retain(|root| *root != e.location.module);
                    self.dropped.push(e);
                }
                result => return result,
            }
        }
    }
}
