//! The spatial dedup cache: a `HashMap` whose entries remember where they were last used from, so
//! a sweep can drop the art of a place you have left.

use std::collections::HashMap;
use std::hash::Hash;

/// The dwell floor: nothing is dropped within this many wall seconds of its last use, however far
/// away. A free-fly camera at 500 yd/s crosses the radius in seconds, and distance alone would
/// drop art it rebuilds on the way back; at gameplay speed the floor never binds.
const MIN_DWELL_SECS: f32 = 30.0;

/// Horizontal distance between two WoW-space points; height is ignored, as the streamer's window
/// is a tile square.
fn plan_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}

/// A dedup cache that a sweep can empty of the art of places left behind. A lookup is
/// [`Self::fetch`], which counts as a use; a `None` stamp means used since the last sweep, so a
/// hit costs no stamping.
pub struct SpatialCache<K, V> {
    map: HashMap<K, (V, Option<Stamp>)>,
}

/// The view focus and the time of the first sweep after an entry's last use.
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
    /// Live entry count.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Drop everything: the map-change teardown.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Fetch a value, counting the hit as a use. It clones out so the miss path can insert in the
    /// same expression.
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

    /// Get or build in place, counted as a use, for a value too big to clone out.
    pub fn or_insert_with(&mut self, key: K, build: impl FnOnce() -> V) -> &mut V {
        let slot = self.map.entry(key).or_insert_with(|| (build(), None));
        slot.1 = None;
        &mut slot.0
    }

    /// Stamp everything used since the last sweep at `focus` and `now`, then drop every entry both
    /// more than `radius` yd away and idle for [`MIN_DWELL_SECS`]; returns how many were dropped.
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
    /// A sweep just past the dwell floor, the earliest that distance can expire anything.
    const LATER: f32 = NOW + MIN_DWELL_SECS + 1.0;
    const HOME: [f32; 3] = [0.0, 0.0, 0.0];
    const FAR: [f32; 3] = [4000.0, 0.0, 0.0];
    /// Five ADT tiles; any radius exercises the same code.
    const R: f32 = 2667.0;

    #[test]
    fn the_first_sweep_stamps_rather_than_judges() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        // No stamp yet, so it cannot be too far.
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
        // Back inside the radius, the same entry: no rebuild.
        assert_eq!(c.scope(HOME, R, NOW + 6.0), 0);
        assert_eq!(c.fetch(&1), Some(10));
        // Away past the floor, it does expire.
        c.scope(HOME, R, NOW + 7.0);
        assert_eq!(c.scope(FAR, R, NOW + 7.0 + MIN_DWELL_SECS), 1);
    }

    #[test]
    fn altitude_is_not_distance() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        c.scope(HOME, R, NOW);
        assert_eq!(c.scope([0.0, 0.0, 10_000.0], R, LATER), 0);
    }

    /// Eviction frees memory only if dropping the last handle frees the asset.
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

    #[test]
    fn an_unswept_cache_never_expires() {
        let mut c: SpatialCache<u32, u32> = SpatialCache::default();
        c.insert(1, 10);
        assert_eq!(c.fetch(&1), Some(10));
        assert_eq!(c.len(), 1);
    }
}
