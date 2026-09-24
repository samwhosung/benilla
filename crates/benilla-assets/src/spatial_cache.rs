//! **The spatial dedup cache** — a `HashMap` whose entries remember *where they were last used
//! from*, so a sweep can drop the art of a place you have left.
//!
//! A drop-in for the `HashMap<K, Handle<_>>` these caches used to be: the only difference at a call
//! site is that a lookup is [`SpatialCache::fetch`] (it counts as a use). `None` in an entry's stamp
//! means "used since the last sweep" — that inverted spelling is what removes all the plumbing,
//! because *not* being stamped is the cheap default and stamping happens once per sweep instead of
//! once per hit.
//!
//! It lives here, below the renderer, because it is the cache the asset foundation itself is built
//! out of (`WorldAssets`' textures and model materials) and because it knows nothing about a world:
//! [`SpatialCache::scope`] is handed the focus, the radius and the clock as three plain scalars and
//! never asks anyone for them. The *instrumentation* around it — which caches exist,
//! what a census row is, where the focus comes from — stayed up in `art_scope`, which is the half
//! that reads the camera and the character roster.

use std::collections::HashMap;
use std::hash::Hash;

/// The **dwell floor**: nothing is dropped within this many wall seconds of its last use, however far
/// away you have gone. Distance decides *what* expires; this decides how soon it is allowed to.
///
/// Found by the director's first real run, and it is the case the distance rule alone gets wrong. The
/// detached free-fly camera (`F`, `FlyCam::speed` 40–100 yd/s, ×5 on Ctrl) travels at up to 500 yd/s —
/// so it crosses the 2667 yd radius in **five seconds**, and a sweep then drops a whole city's dedup
/// that the same camera re-creates on the way back. Their 48-second leg evicted 25 570 entries and
/// rebuilt `pmat` 3086 → 15354 doing exactly that: the eviction was *correct* by the radius and
/// useless in effect, because nothing had finished being wanted.
///
/// 30 s covers a 15 000 yd round trip at boosted fly speed. Under gameplay speed it never binds at
/// all (a gryphon needs ~107 s to cover the radius), so B131's own case is unchanged — this only
/// stops the thrash at speeds the radius was never sized for. Residency stays bounded either way:
/// what you hold is at most the art of the last 30 seconds of travel, which does not grow with how
/// long the process has run.
const MIN_DWELL_SECS: f32 = 30.0;

/// Horizontal (tile-plane) distance between two wow-space points. Height is deliberately ignored:
/// residency is a 2D question — the streamer's window is a tile square, and art 200 yd below a
/// gryphon is in front of you, not away from you.
fn plan_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

/// A dedup cache whose entries remember **where they were last used from**, so a sweep can drop the
/// art of a place you have left. A drop-in for the `HashMap<K, Handle<_>>` these caches used to be:
/// the only difference at a call site is that a lookup is [`Self::fetch`] (it counts as a use).
///
/// `None` in the stamp means "used since the last sweep" — see the module docs for why that inverted
/// spelling is what removes all the plumbing.
pub struct SpatialCache<K, V> {
    map: HashMap<K, (V, Option<Stamp>)>,
}

/// Where and when an entry was last wanted: the view focus at the first sweep after its last use.
/// Both halves are load-bearing — `at` decides whether it is far enough to expire, `t` whether it has
/// been idle long enough to be allowed to ([`MIN_DWELL_SECS`]).
#[derive(Clone, Copy)]
struct Stamp {
    at: [f32; 3],
    t: f32,
}

impl<K, V> Default for SpatialCache<K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash, V> SpatialCache<K, V> {
    /// Live entry count — what the census reports and the map-change evictors log.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache holds nothing. Here because a `pub len` without it is a lint, and because
    /// "did the map-change teardown actually empty this?" is a real question at a call site.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Drop everything (the map-scope teardown, decision 0729 — still the hard reset).
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Fetch a deduped value, **counting the hit as a use** so it restarts its distance grace.
    /// Clones out rather than lending a reference: every caller wants an owned `Handle` anyway, and
    /// ending the borrow at the call is what lets the miss path insert in the same expression.
    pub fn fetch(&mut self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let slot = self.map.get_mut(key)?;
        slot.1 = None;
        Some(slot.0.clone())
    }

    /// Install a freshly-built value, counted as a use.
    pub fn insert(&mut self, key: K, value: V) {
        self.map.insert(key, (value, None));
    }

    /// Get-or-build in place, counted as a use — for the caches whose value is too big to clone out
    /// ([`crate::clutter::ClutterGeometry`]'s decoded submeshes).
    pub fn or_insert_with(&mut self, key: K, build: impl FnOnce() -> V) -> &mut V {
        let slot = self.map.entry(key).or_insert_with(|| (build(), None));
        slot.1 = None;
        &mut slot.0
    }

    /// Stamp everything used since the last sweep at `focus`/`now`, then drop every entry that is
    /// **both** more than `radius` yards away **and** idle for at least [`MIN_DWELL_SECS`]. Returns
    /// how many were dropped.
    ///
    /// The two conditions are not redundant. Distance alone expires art the moment a fast camera
    /// pulls away from it, which is how the director's free-fly leg lost a city's dedup and rebuilt
    /// it seconds later; the dwell floor is what makes "not immediate" true at any speed, not just at
    /// gameplay speed.
    pub fn scope(&mut self, focus: [f32; 3], radius: f32, now: f32) -> usize {
        let before = self.map.len();
        self.map.retain(|_, (_, stamp)| match *stamp {
            None => {
                *stamp = Some(Stamp { at: focus, t: now });
                true
            }
            Some(s) => plan_dist(s.at, focus) <= radius || now - s.t < MIN_DWELL_SECS,
        });
        before - self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    /// When the art is used and stamped.
    const NOW: f32 = 1000.0;
    /// A sweep past the dwell floor — the earliest moment distance is *allowed* to expire anything.
    /// Every "…is dropped" assertion sweeps here, which is itself the dwell floor's regression test:
    /// swap this back to `NOW` and three of them fail (verified).
    const LATER: f32 = NOW + MIN_DWELL_SECS + 1.0;
    const HOME: [f32; 3] = [0.0, 0.0, 0.0];
    const FAR: [f32; 3] = [4000.0, 0.0, 0.0];
    /// Five ADT tiles — `art_scope`'s default eviction radius at the time these were written. Any
    /// radius exercises the same code; the number is here only so the distances read naturally.
    const R: f32 = 2667.0;

    /// A fresh entry survives its first sweep — that is the sweep that *stamps* it. Without this the
    /// grace period would depend on where in the second the art happened to load.
    #[test]
    fn the_first_sweep_stamps_rather_than_judges() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        // Swept from the far side immediately: the entry has no stamp yet, so it cannot be too far.
        assert_eq!(c.scope(FAR, R, NOW), 0);
        assert_eq!(c.len(), 1);
        // Now it is stamped at FAR, so a later sweep from HOME drops it.
        assert_eq!(c.scope(HOME, R, LATER), 1);
        assert!(c.fetch(&1).is_none());
    }

    #[test]
    fn art_you_walked_away_from_is_dropped() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        c.scope(HOME, R, NOW); // stamp at home
        assert_eq!(
            c.scope([R * 0.9, 0.0, 0.0], R, LATER),
            0,
            "inside the radius"
        );
        assert_eq!(c.scope(FAR, R, LATER), 1, "beyond it");
    }

    /// A hit restarts the grace, which is what keeps art in the direction of travel resident: the
    /// same tree material is re-fetched by every new placement that streams in ahead of you.
    #[test]
    fn a_use_restarts_the_grace() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        c.scope(HOME, R, NOW);
        for step in 1..20 {
            let at = [step as f32 * R * 0.5, 0.0, 0.0];
            assert_eq!(c.fetch(&1), Some(10));
            // Swept well past the dwell floor every step, so what keeps it alive is the use.
            assert_eq!(
                c.scope(at, R, NOW + step as f32 * (MIN_DWELL_SECS + 1.0)),
                0,
                "re-used at step {step}"
            );
        }
    }

    /// `or_insert_with` builds once and counts as a use, like `fetch`.
    #[test]
    fn or_insert_with_builds_once_and_counts_as_a_use() {
        let mut c: SpatialCache<u32, Vec<u32>> = SpatialCache::default();
        let mut builds = 0;
        for _ in 0..3 {
            let v = c.or_insert_with(7, || {
                builds += 1;
                vec![1, 2, 3]
            });
            assert_eq!(v.len(), 3);
        }
        assert_eq!(builds, 1);
        assert_eq!(
            c.scope(FAR, R, LATER),
            0,
            "the last use restarted the grace"
        );
    }

    /// **The director's free-fly case.** A camera at boosted fly speed crosses the radius in ~5 s; if
    /// distance alone decided, the art it just streamed would be dropped and then rebuilt as the same
    /// camera came back. Nothing expires inside the dwell floor however far away the focus goes.
    #[test]
    fn a_fast_camera_cannot_outrun_the_dwell_floor() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        c.scope(HOME, R, NOW);
        // 500 yd/s (FlyCam 100 × the Ctrl boost 5) for five seconds: far past the radius already.
        for s in 1..=5 {
            let at = [500.0 * s as f32, 0.0, 0.0];
            assert_eq!(
                c.scope(at, R, NOW + s as f32),
                0,
                "dropped {s} s out — the free-fly thrash"
            );
        }
        // Fly all the way back inside the radius and the entry is still the same one: no rebuild.
        assert_eq!(c.scope(HOME, R, NOW + 6.0), 0);
        assert_eq!(c.fetch(&1), Some(10));
        // Stay away past the floor and it does expire — the bound is delayed, not removed.
        c.scope(HOME, R, NOW + 7.0);
        assert_eq!(c.scope(FAR, R, NOW + 7.0 + MIN_DWELL_SECS), 1);
    }

    /// Height is not distance: flying over a city must not evict it.
    #[test]
    fn altitude_is_not_distance() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        c.scope(HOME, R, NOW);
        assert_eq!(c.scope([0.0, 0.0, 10_000.0], R, LATER), 0);
    }

    /// **The mechanism the whole design rests on**: dropping the cache's handle really does free the
    /// asset. If `Assets<T>` kept the value alive past its last strong handle, eviction would cost us
    /// the dedup and recover no memory — a failure indistinguishable, from the outside, from "the fix
    /// did nothing". Checked here rather than assumed, because the *reason* B131's `images` column
    /// never fell is precisely that these caches held the last handle.
    #[test]
    fn dropping_the_last_handle_frees_the_asset() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Image>();
        let mut cache: SpatialCache<u32, Handle<Image>> = SpatialCache::default();
        let id = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let handle = images.add(Image::default());
            let id = handle.id();
            cache.insert(1, handle);
            id
        };
        app.update();
        assert!(
            app.world().resource::<Assets<Image>>().contains(id),
            "held by the cache"
        );
        cache.scope(HOME, R, NOW); // stamp it here…
        assert_eq!(cache.scope(FAR, R, LATER), 1, "…then walk away");
        // The drop is processed by `Assets::track_assets` on a later frame, not at the drop itself.
        for _ in 0..3 {
            app.update();
        }
        assert!(
            !app.world().resource::<Assets<Image>>().contains(id),
            "the image outlived its last handle — eviction would free nothing"
        );
    }

    /// A cache nobody sweeps (the `Local<MaterialCache>`s in the glue booth / portrait bake) expires
    /// nothing: without a `scope` call the stamps stay `None` forever.
    #[test]
    fn an_unswept_cache_never_expires() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        assert_eq!(c.fetch(&1), Some(10));
        assert_eq!(c.len(), 1);
    }
}
