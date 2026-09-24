//! The static merge's batch classes, and the `WOW_MERGE_CENSUS=1` census that tallies every
//! spawned batch by class and prints the table once the stream has been quiet for 2 s. A dev
//! reading, so the tallies are statics rather than state threaded through the assembler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bevy::prelude::*;

/// Whether `WOW_MERGE_CENSUS=1` armed the census, read once.
pub fn census_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_MERGE_CENSUS").as_deref() == Ok("1"))
}

/// The classes in the merge predicate's short-circuit order; the indices are the atomics' layout,
/// so append, never reorder.
const CLASS_NAMES: [&str; 9] = [
    "excluded: anim/billboard/mat-anim",
    "interior prop",
    "wmo order-free never-fade", // never merges: WMO group geometry is refused per site
    "wmo transparent",
    "wmo FINITE-FADE (anomaly)", // WMOs enroll at radius = INFINITY, so a count here is an anomaly
    "doodad order-free never-fade", // merges
    "doodad order-free fader",   // merges; the dense class
    "doodad transparent never-fade",
    "doodad transparent fader",
];
const MERGE_LANES: [usize; 3] = [1, 5, 6];

static ROWS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static VERTS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
/// Distinct would-be merge keys per class (`MergeSite::census_key`), the predicted blob count, so
/// a lane's row reduction (rows − blobs) reads off the table. Spawn-time only, so a mutex is fine.
static KEYS: std::sync::OnceLock<Mutex<[std::collections::HashSet<u64>; 9]>> =
    std::sync::OnceLock::new();

fn keys() -> &'static Mutex<[std::collections::HashSet<u64>; 9]> {
    KEYS.get_or_init(|| Mutex::new(std::array::from_fn(|_| std::collections::HashSet::new())))
}

/// Zero the tallies, at the map-change teardown (`drop_streamed_world`): they never decrement on
/// tile unload, so after a teleport the table would print both maps' sum.
pub fn reset() {
    for a in ROWS.iter().chain(VERTS.iter()) {
        a.store(0, Ordering::Relaxed);
    }
    for set in keys().lock().unwrap().iter_mut() {
        set.clear();
    }
}

/// One batch's merge class, built once by the assembler for both the census and the merge.
/// `excluded`: any animation (anim host, billboard, alpha, uv or rgb anim, per-sequence material);
/// `order_free`: `Opaque|AlphaTest`, not additive, since only transparent-pass batches carry draw
/// order; `never_fade`: `radius > NEVER_FADE_RADIUS`, as WMO group geometry's infinite radius is.
pub struct BatchClass {
    pub excluded: bool,
    pub interior_prop: bool,
    pub order_free: bool,
    pub never_fade: bool,
}

impl BatchClass {
    /// The merge predicate: static and order-free. A site's own refusal lives in its `divert`.
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

/// Print the table once the stream has been quiet for 2 s; any new tally re-arms it.
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
        // `net` (rows − blobs) is the rows a lane would delete; prop-class keys omit the
        // referrer-set and probe-slot axes, so their net is an upper bound.
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
