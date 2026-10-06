//! `iw4l gsc-audit <so_mission>... | all`: walks a Spec Ops mission's zones
//! headlessly, compiles its SP program the way a match install does, and lists
//! the scripts `iw4sp.exe` runs by name that the zones carry but the program
//! leaves out. One TSV per mission lands in `<artifacts>/gsc-audit/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use asset_transport::{GamesRoot, find_runtime_common_mp, find_zone_file, find_zone_for_tree};
use sim::script::{Catalog, Iw4SpStartup, SourceOrigin, SourceResolver, engine_named};

struct Zones(assets::ScriptSources);
impl SourceResolver for Zones {
    fn read(&self, module: &str) -> Result<String, String> {
        self.read_bytes(module)
            .map(|bytes| sim::script::decode_source(&bytes))
    }
    fn read_bytes(&self, module: &str) -> Result<Vec<u8>, String> {
        self.0.read(module)
    }
    fn origin(&self, module: &str) -> SourceOrigin {
        match self.0.origin(module) {
            Some(assets::ScriptSourceOrigin::Packaged) => SourceOrigin::Packaged,
            Some(assets::ScriptSourceOrigin::BuiltIn) => SourceOrigin::BuiltIn,
            None => SourceOrigin::External,
        }
    }
}

/// Module prefixes worth listing when an SP zone carries them but the program
/// leaves them out (`common_mp`'s MP characters are not).
const NOTABLE: [&str; 5] = [
    "animscripts/",
    "aitype/",
    "character/",
    "xmodelalias/",
    "codescripts/",
];

pub fn run(games: GamesRoot, artifacts: PathBuf, missions: Vec<String>) {
    let missions = match missions.as_slice() {
        [all] if all == "all" => every_mission(&games),
        _ => missions,
    };
    let out = artifacts.join("gsc-audit");
    let _ = std::fs::create_dir_all(&out);
    let mut aggregate: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for mission in &missions {
        match audit(&games, mission, &out) {
            Ok(missing) => {
                for module in missing {
                    aggregate.entry(module).or_default().insert(mission.clone());
                }
            }
            Err(error) => println!("gsc-audit: {mission}: {error}"),
        }
    }
    println!("gsc-audit: {} missions", missions.len());
    for (module, missions) in &aggregate {
        println!(
            "gsc-audit: missing {module} in {} missions: {}",
            missions.len(),
            missions.iter().cloned().collect::<Vec<_>>().join(" ")
        );
    }
    if aggregate.is_empty() {
        println!("gsc-audit: no engine-named script missing");
    }
}

fn every_mission(games: &GamesRoot) -> Vec<String> {
    let Ok(found) = find_zone_file(games, "so_killspree_favela") else {
        return Vec::new();
    };
    let dir = found.path.parent().unwrap_or(Path::new("."));
    let mut missions: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".ff")?;
            stem.starts_with("so_").then(|| stem.to_owned())
        })
        .filter(|stem| {
            asset_transport::split_so_mission(&dir.join(format!("{stem}.ff")))
                .1
                .is_some()
        })
        .collect();
    missions.sort();
    missions
}

fn audit(games: &GamesRoot, mission: &str, out: &Path) -> Result<Vec<String>, String> {
    let found = find_zone_file(games, mission)?;
    let (base, addon) = asset_transport::split_so_mission(&found.path);
    let addon = addon.ok_or("not a Spec Ops mission (no base map)")?;
    let started = Instant::now();
    let zones = [
        ("common_mp", find_runtime_common_mp(games, &base)?.path),
        ("common", find_zone_for_tree(&addon, "common")?.path),
        ("map", base),
        ("mission", addon),
    ];
    let mut sources = assets::ScriptSources::default();
    let mut owner: BTreeMap<String, &str> = BTreeMap::new();
    let mut counts = Vec::new();
    for (label, path) in &zones {
        let walked = assets::zone_script_sources(path)?;
        counts.push(format!("{label}={}", walked.len()));
        for module in walked.modules() {
            owner.insert(module.to_owned(), label);
        }
        sources.overlay(walked);
    }
    let walk_ms = started.elapsed().as_millis();
    let sources = Zones(sources);
    let started = Instant::now();
    let mut startup = Iw4SpStartup::new(mission, sources.0.modules());
    let catalog = Catalog::iw4sp().with_native_stubs(true);
    let program = startup
        .load(&sources, &catalog)
        .map_err(|fault| format!("compile: {fault}"))?;
    let compile_ms = started.elapsed().as_millis();
    let compiled: BTreeSet<&str> = program
        .modules()
        .iter()
        .map(|m| m.module.as_str())
        .collect();
    let mut tsv = String::from("module\tzone\tcompiled\tengine_named\n");
    let dropped: BTreeSet<&str> = startup
        .dropped
        .iter()
        .map(|fault| fault.location.module.as_str())
        .collect();
    let mut missing = Vec::new();
    let mut uncompiled = Vec::new();
    for (module, zone) in &owner {
        let is_compiled = compiled.contains(module.as_str());
        let named = engine_named(mission, module);
        tsv.push_str(&format!("{module}\t{zone}\t{is_compiled}\t{named}\n"));
        if is_compiled {
            continue;
        }
        if named && !dropped.contains(module.as_str()) {
            missing.push(module.clone());
        } else if *zone != "common_mp" && NOTABLE.iter().any(|prefix| module.starts_with(prefix)) {
            uncompiled.push(module.clone());
        }
    }
    let _ = std::fs::write(out.join(format!("{mission}.tsv")), tsv);
    println!(
        "gsc-audit: {mission} zones {} union={} roots={} compiled={} functions={} walk_ms={walk_ms} compile_ms={compile_ms} dropped={}",
        counts.join(" "),
        owner.len(),
        startup.roots.len(),
        compiled.len(),
        program.function_count(),
        dropped.len()
    );
    for fault in &startup.dropped {
        println!("gsc-audit: {mission} dropped {fault}");
    }
    for module in &missing {
        println!("gsc-audit: {mission} missing {module} ({})", owner[module]);
    }
    println!(
        "gsc-audit: {mission} uncompiled {}",
        uncompiled
            .iter()
            .map(|m| format!("{m}({})", owner[m]))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(missing)
}
