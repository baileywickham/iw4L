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
    max_cpu: u64,
}

static WINDOW: LazyLock<Mutex<Window>> = LazyLock::new(Mutex::default);

pub(crate) struct Census {
    pub(crate) threads: usize,
    pub(crate) waiters: usize,
    pub(crate) objects: usize,
    pub(crate) arrays: usize,
    pub(crate) queued: usize,
    pub(crate) errors: u64,
    pub(crate) entities: usize,
    pub(crate) presences: usize,
}

pub(crate) fn tick(
    now: i64,
    run_ns: u64,
    heap_ns: u64,
    cpu_ns: u64,
    census: impl FnOnce() -> Census,
) {
    let Ok(mut window) = WINDOW.lock() else {
        return;
    };
    window.ticks += 1;
    window.run_ns += run_ns;
    window.heap_ns += heap_ns;
    window.max_ns = window.max_ns.max(run_ns + heap_ns);
    window.max_cpu = window.max_cpu.max(cpu_ns);
    if window.ticks < 200 {
        return;
    }
    let ticks = f64::from(window.ticks);
    let c = census();
    diag::info!(
        Sim,
        "gsc stats: t={:.1}s ticks={} run_ms={:.3} heap_ms={:.3} max_ms={:.3} max_cpu_ms={:.3} threads={} waiters={} queued={} objects={} arrays={} errors={} entities={} presences={}",
        now as f64 / 1000.0,
        window.ticks,
        window.run_ns as f64 / ticks / 1e6,
        window.heap_ns as f64 / ticks / 1e6,
        window.max_ns as f64 / 1e6,
        window.max_cpu as f64 / 1e6,
        c.threads,
        c.waiters,
        c.queued,
        c.objects,
        c.arrays,
        c.errors,
        c.entities,
        c.presences,
    );
    *window = Window::default();
}

/// `IW4L_GSC_STATS=1` also names long script ticks: a tick at or over
/// `IW4L_GSC_HITCH_MS` (default 50) logs its phases, the natives that took
/// the most time and the thread resumptions (by the function they resumed in).
static HITCH_NS: LazyLock<u64> = LazyLock::new(|| {
    std::env::var("IW4L_GSC_HITCH_MS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map_or(50_000_000, |ms| (ms * 1e6) as u64)
});

#[derive(Default)]
struct Hitch {
    phases: Vec<(&'static str, u64)>,
    natives: std::collections::HashMap<&'static str, (u32, u64)>,
    resumes: std::collections::HashMap<String, (u32, u64)>,
}

static HITCH: LazyLock<Mutex<Hitch>> = LazyLock::new(Mutex::default);

thread_local! {
    /// Instructions executed per script function this tick (stats only).
    static OPS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn count_op(function: usize) {
    OPS.with(|ops| {
        let mut ops = ops.borrow_mut();
        if ops.len() <= function {
            ops.resize(function + 1, 0);
        }
        ops[function] += 1;
    });
}

/// Takes this tick's instruction counts: (function, count), largest first.
fn take_ops() -> (u64, Vec<(usize, u64)>) {
    OPS.with(|ops| {
        let mut ops = ops.borrow_mut();
        let total = ops.iter().sum();
        let mut rows: Vec<(usize, u64)> = ops
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(f, n)| (f, *n))
            .collect();
        ops.iter_mut().for_each(|n| *n = 0);
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        rows.truncate(12);
        (total, rows)
    })
}

pub(crate) fn hitch_phase(name: &'static str, ns: u64) {
    if let Ok(mut h) = HITCH.lock() {
        h.phases.push((name, ns));
    }
}

pub(crate) fn hitch_native(name: &'static str, ns: u64) {
    if let Ok(mut h) = HITCH.lock() {
        let row = h.natives.entry(name).or_default();
        row.0 += 1;
        row.1 += ns;
    }
}

pub(crate) fn hitch_resume(function: impl FnOnce() -> String, ns: u64) {
    if ns < 200_000 {
        return;
    }
    if let Ok(mut h) = HITCH.lock() {
        let row = h.resumes.entry(function()).or_default();
        row.0 += 1;
        row.1 += ns;
    }
}

/// Closes one tick's hitch record; logs it when the tick was long.
pub(crate) fn hitch_end(now: i64, total_ns: u64, cpu_ns: u64, name: impl Fn(usize) -> String) {
    let (ops_total, ops) = take_ops();
    let Ok(mut h) = HITCH.lock() else {
        return;
    };
    let h = std::mem::take(&mut *h);
    if total_ns < *HITCH_NS && cpu_ns < *HITCH_NS {
        return;
    }
    let ops = ops
        .iter()
        .map(|(f, n)| format!("{}={n}", name(*f)))
        .collect::<Vec<_>>()
        .join(" ");
    fn top<K: std::fmt::Display>(rows: Vec<(K, (u32, u64))>) -> String {
        let mut rows = rows;
        rows.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        rows.iter()
            .take(10)
            .map(|(k, (n, ns))| format!("{k}={:.1}ms/{n}", *ns as f64 / 1e6))
            .collect::<Vec<_>>()
            .join(" ")
    }
    let phases = h
        .phases
        .iter()
        .map(|(k, ns)| format!("{k}={:.1}", *ns as f64 / 1e6))
        .collect::<Vec<_>>()
        .join(" ");
    diag::info!(
        Sim,
        "gsc hitch: t={:.2}s total_ms={:.1} cpu_ms={:.1} phases[{phases}] natives[{}] resumes[{}] ops={ops_total} ops_by_fn[{ops}]",
        now as f64 / 1000.0,
        total_ns as f64 / 1e6,
        cpu_ns as f64 / 1e6,
        top(h.natives.into_iter().collect()),
        top(h.resumes.into_iter().collect()),
    );
}
