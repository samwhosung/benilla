//! **`WOW_MERGE_CENSUS=1` — the production-merge population census** ("the
//! census comes first").
//!
//! Tallies every batch the streamer walks through `spawn_model_entities` into the population
//! classes 1417's lane-1 predicate carves — anim-excluded / interior prop / WMO vs doodad ×
//! order-free vs transparent × never-fade vs fader — and prints the table once the stream has
//! been quiet for two seconds. The point is to read lane 1's yield (and every later lane's
//! go/no-go) off a live pin instead of a prior: 0922's ~40% transparent share and the
//! bracket's 21k-row delta are the numbers this either confirms or replaces.
//!
//! Statics + a quiet-timer logger, not plumbed state: the tally site is deep in the assembler
//! (which already threads eleven parameters), and this is a dev reading at a parked pin, not
//! a production mechanism — the same trade `WOW_HIT_COST` made. Wall-clock quiet is likewise
//! fine HERE (contrast the bracket's same-shaped flush timer, which 1417 retires for
//! production in favour of the weld's close rule).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bevy::prelude::*;

/// Is the census armed? One read; the assembler tally and the logger both key on it.
pub fn census_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_MERGE_CENSUS").as_deref() == Ok("1"))
}

/// The classes, in the lane-1 predicate's own short-circuit order (1417). Indices are the
/// atomics' layout — append, never reorder.
const CLASS_NAMES: [&str; 9] = [
    "excluded: anim/billboard/mat-anim",
    "interior prop",
    "wmo order-free never-fade", // measured 1:1 — never merges (1418)
    "wmo transparent",
    "wmo FINITE-FADE (anomaly)", // WMOs enroll at radius = INFINITY; nonzero here = investigate
    "doodad order-free never-fade", // merges
    "doodad order-free fader",   // merges (1418 — the dense class)
    "doodad transparent never-fade",
    "doodad transparent fader",
];
const MERGE_LANES: [usize; 3] = [1, 5, 6];

static ROWS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static VERTS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
/// Distinct would-be merge keys per class — each class's PREDICTED blob count under its
/// lane's key (built from [`MergeSite::census_key`]), so a lane's honest row reduction
/// (rows − blobs) is read off the table before the lane is built. The lane-1 smoke run is the
/// estimator's validation anchor: 6,892 measured blobs at the SW pin. Spawn-time only, so a
/// mutex is fine.
static KEYS: std::sync::OnceLock<Mutex<[std::collections::HashSet<u64>; 9]>> =
    std::sync::OnceLock::new();

fn keys() -> &'static Mutex<[std::collections::HashSet<u64>; 9]> {
    KEYS.get_or_init(|| Mutex::new(std::array::from_fn(|_| std::collections::HashSet::new())))
}

/// Zero the tallies. Called from the map-change teardown (`drop_streamed_world`): the counters
/// are cumulative and never decrement on tile unload, so without this a rig that logs in on one
/// map and teleports to another prints the SUM of both worlds — the first LBRS read did exactly
/// that (its doodad rows matched Stormwind's to the digit, because they WERE Stormwind's).
pub fn reset() {
    for a in ROWS.iter().chain(VERTS.iter()) {
        a.store(0, Ordering::Relaxed);
    }
    for set in keys().lock().unwrap().iter_mut() {
        set.clear();
    }
}

/// One batch, classified exactly as 1417's lane-1 predicate sees it — built once per batch by
/// the assembler and consumed by BOTH the census tally and the production divert, so the
/// number the census prints and the population the merge takes can never drift apart.
/// `excluded` is the bracket's anim predicate (anim host / billboard / alpha- uv- rgb-anim /
/// per-seq material); `order_free` is `Opaque|AlphaTest && !additive` (0858's law: order
/// information exists only on transparent-pass batches); `never_fade` is
/// `radius > NEVER_FADE_RADIUS` (WMO group geometry's ∞ radius lands there by construction).
pub struct BatchClass {
    pub excluded: bool,
    pub interior_prop: bool,
    pub order_free: bool,
    pub never_fade: bool,
}

impl BatchClass {
    /// The merge predicate (1417 lane 1, widened twice by 1418: faders when the fade channels
    /// moved in-shader, interior props when the probe slot did — what remains here is "static
    /// and order-free"; per-SITE refusal (WMO group geometry) lives in `divert`).
    pub fn merges(&self) -> bool {
        !self.excluded && self.order_free
    }
}

pub fn tally(class: &BatchClass, is_wmo: bool, verts: usize, key: Option<u64>) {
    let &BatchClass {
        excluded,
        interior_prop,
        order_free,
        never_fade,
    } = class;
    let class = if excluded {
        0
    } else if interior_prop {
        1
    } else if is_wmo {
        match (never_fade, order_free) {
            (false, _) => 4,
            (true, true) => 2,
            (true, false) => 3,
        }
    } else {
        match (order_free, never_fade) {
            (true, true) => 5,
            (true, false) => 6,
            (false, true) => 7,
            (false, false) => 8,
        }
    };
    ROWS[class].fetch_add(1, Ordering::Relaxed);
    VERTS[class].fetch_add(verts as u64, Ordering::Relaxed);
    if let Some(key) = key {
        keys().lock().unwrap()[class].insert(key);
    }
}

/// Print the table once the stream has been quiet for 2 s; re-arm on any new tally (a fresh
/// tile crossing prints a fresh table).
pub fn log_merge_census(time: Res<Time>, mut prev: Local<(u64, f32, bool)>) {
    let total: u64 = ROWS.iter().map(|a| a.load(Ordering::Relaxed)).sum();
    let (last, moved_at, printed) = &mut *prev;
    if total != *last {
        *last = total;
        *moved_at = time.elapsed_secs();
        *printed = false;
        return;
    }
    if total == 0 || *printed || time.elapsed_secs() - *moved_at < 2.0 {
        return;
    }
    *printed = true;
    let mut merge_rows = 0u64;
    let keys = keys().lock().unwrap();
    for (i, name) in CLASS_NAMES.iter().enumerate() {
        let rows = ROWS[i].load(Ordering::Relaxed);
        let verts = VERTS[i].load(Ordering::Relaxed);
        let blobs = keys[i].len() as u64;
        let mark = if MERGE_LANES.contains(&i) {
            "  <- merges"
        } else {
            ""
        };
        // `blobs` = distinct would-be merge keys; `net` = the rows the lane would actually
        // delete (rows − blobs) — the number the 44 ns/row derivative multiplies. Prop-class
        // keys omit the referrer-set/probe-slot axes, so their net is an optimistic BOUND.
        info!(
            "[merge-census] {name:<34} rows={rows:>7}  kverts={:>8}  blobs={blobs:>6}  net=-{:>6}{mark}",
            verts / 1000,
            rows.saturating_sub(blobs)
        );
        if MERGE_LANES.contains(&i) {
            merge_rows += rows;
        }
    }
    info!(
        "[merge-census] merge lanes: {merge_rows} of {total} rows ({:.1}%)",
        100.0 * merge_rows as f64 / total as f64
    );
}
