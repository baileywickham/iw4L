//! `IW4L_GSC_USAGE=1`: counts stubbed natives, runtime errors and field use on
//! actors and spawners for the whole process, and logs the histograms at quit.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

static ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var("IW4L_GSC_USAGE").is_ok_and(|value| value == "1"));

#[derive(Default)]
struct Usage {
    stubs: BTreeMap<String, u64>,
    errors: BTreeMap<String, u64>,
    sites: BTreeMap<String, u64>,
    reads: BTreeMap<String, u64>,
    writes: BTreeMap<String, u64>,
}

static USAGE: LazyLock<Mutex<Usage>> = LazyLock::new(Mutex::default);

pub(crate) fn enabled() -> bool {
    *ENABLED
}

fn bump(select: impl FnOnce(&mut Usage) -> &mut BTreeMap<String, u64>, key: String) {
    if let Ok(mut usage) = USAGE.lock() {
        *select(&mut usage).entry(key).or_default() += 1;
    }
}

pub(crate) fn stub(name: &str) {
    if enabled() {
        bump(|u| &mut u.stubs, name.to_owned());
    }
}

pub(crate) fn error(site: &str, message: &str) {
    if enabled() {
        let mut key = String::with_capacity(message.len());
        for (i, part) in message.split('\'').enumerate() {
            key.push_str(if i % 2 == 1 { "'X'" } else { part });
        }
        bump(|u| &mut u.sites, site.to_owned());
        bump(|u| &mut u.errors, key);
    }
}

pub(crate) fn field(owner: &str, name: &str, write: bool) {
    let key = format!("{owner}.{name}");
    if write {
        bump(|u| &mut u.writes, key);
    } else {
        bump(|u| &mut u.reads, key);
    }
}

fn histogram(label: &str, counts: &BTreeMap<String, u64>, limit: usize) {
    let mut rows: Vec<_> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let total: u64 = counts.values().sum();
    let body: Vec<String> = rows
        .iter()
        .take(limit)
        .map(|(k, n)| format!("{k}={n}"))
        .collect();
    diag::info!(
        Sim,
        "gsc usage: {label} distinct={} total={total} {}",
        rows.len(),
        body.join(" ")
    );
}

/// Logs the histograms; called once when the process exits.
pub fn report() {
    if !enabled() {
        return;
    }
    let Ok(usage) = USAGE.lock() else {
        return;
    };
    histogram("stubs", &usage.stubs, usize::MAX);
    histogram("errors", &usage.errors, usize::MAX);
    histogram("error_sites", &usage.sites, 40);
    histogram("field_reads", &usage.reads, usize::MAX);
    histogram("field_writes", &usage.writes, usize::MAX);
}

static STATS: LazyLock<bool> =
    LazyLock::new(|| std::env::var("IW4L_GSC_STATS").is_ok_and(|value| value == "1"));

/// `IW4L_GSC_STATS=1`: every 200 authority ticks, logs where the scheduler
/// spent its time and how large the script heap and wait lists are.
pub(crate) fn stats_enabled() -> bool {
    *STATS
}

#[derive(Default)]
struct Window {
    ticks: u32,
    run_ns: u64,
    heap_ns: u64,
    max_ns: u64,
}

static WINDOW: LazyLock<Mutex<Window>> = LazyLock::new(Mutex::default);

pub(crate) struct Census {
    pub(crate) threads: usize,
    pub(crate) waiters: usize,
    pub(crate) objects: usize,
    pub(crate) arrays: usize,
    pub(crate) queued: usize,
    pub(crate) errors: u64,
}

pub(crate) fn tick(now: i64, run_ns: u64, heap_ns: u64, census: impl FnOnce() -> Census) {
    let Ok(mut window) = WINDOW.lock() else {
        return;
    };
    window.ticks += 1;
    window.run_ns += run_ns;
    window.heap_ns += heap_ns;
    window.max_ns = window.max_ns.max(run_ns + heap_ns);
    if window.ticks < 200 {
        return;
    }
    let ticks = f64::from(window.ticks);
    let c = census();
    diag::info!(
        Sim,
        "gsc stats: t={:.1}s ticks={} run_ms={:.3} heap_ms={:.3} max_ms={:.3} threads={} waiters={} queued={} objects={} arrays={} errors={}",
        now as f64 / 1000.0,
        window.ticks,
        window.run_ns as f64 / ticks / 1e6,
        window.heap_ns as f64 / ticks / 1e6,
        window.max_ns as f64 / 1e6,
        c.threads,
        c.waiters,
        c.queued,
        c.objects,
        c.arrays,
        c.errors,
    );
    *window = Window::default();
}
