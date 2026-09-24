//! **Within-map art residency** — the distance half of decision 0729's map-scope teardown.
//!
//! 0729 gave the world-art dedup caches a *reset point*: a cross-map transition clears them, and the
//! loading screen pays for the rebuild. B131 is the case that argument does not reach — a long flight
//! **inside** one map never fires that transition, so nothing ever evicts and `mats`/`images` ratchet
//! for as long as you stay on a continent (decision 0785's open half: 26.8 k materials / 2.8 k images
//! after ten minutes here; 3.2 → 4.9 GiB of VRAM on the reporter's tour).
//!
//! This module turns clear-on-*transition* into clear-on-*distance*: every cache entry remembers the
//! view focus it was last used from, and a sweep drops what was last wanted more than
//! [`DEFAULT_RADIUS_YD`] away. So residency is bounded by *where you are*, not by how long the process
//! has run, and the map-change clear stays exactly as it was — the hard reset on top of the soft one.
//!
//! **Distance decides what expires; a dwell floor decides how soon.** [`MIN_DWELL_SECS`] exists
//! because the radius alone is sized for gameplay speed — the detached free-fly camera crosses it in
//! five seconds and would lose a city's dedup only to rebuild it on the way back (found on the
//! director's first real run; see the constant).
//!
//! **Distance, not a TTL**, and the difference is safety rather than taste: a time-expired entry can
//! be one the streamer is about to ask for again from ten yards away, and dropping it costs a
//! duplicate; a distance-expired entry is one whose tiles unloaded long ago. The radius is therefore
//! floored at the streamer's own reach ([`radius_floor`]) so eviction can never outrun the thing that
//! placed the art.
//!
//! **Two properties worth knowing before reading the code**, both of which buy the design its
//! smallness:
//!
//! 1. **Stamping needs no plumbing.** A *use* (a [`SpatialCache::fetch`] hit or an insert) clears the
//!    stamp to `None` — "used since the last sweep" — and the *sweep* is what writes today's focus
//!    onto every `None`. So no call site has to know where the camera is, and a cache nothing sweeps
//!    (the `Local<MaterialCache>`s in the glue booth and the portrait bake) simply never expires.
//! 2. **Nothing needs ordering.** The stamp is at worst one sweep interval stale (≈25 yd at flight
//!    speed, against a 2.6 km radius) and `due` being read a frame before it is set only moves the
//!    sweep one frame. There are no `.before`/`.after` constraints anywhere in this module.
//!
//! The failure mode if the radius were ever too small: evicting the dedup for art that is still
//! drawn. That is not a visual bug and not a leak — the drawn entity holds its own handles, and the
//! next spawn of that art re-creates one duplicate material which becomes the new cache entry. It
//! costs a lost batch, and it self-heals. That asymmetry (too-eager = mild and transient, too-lazy =
//! the bug we are fixing) is why the floor is a clamp and not a warning.

use std::hash::Hash;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::Real;

use benilla_assets::SpatialCache;
use benilla_formats::TILE_SIZE;

/// Wall seconds between sweeps ([`Real`], not the virtual clock — this is a housekeeping cadence,
/// and decision 0789's reading applies: "what time is it" is not "how much world time to advance").
/// One `retain` over ~30 k entries a second is not measurable; the interval exists so it is not run
/// per frame, and it doubles as the stamp's resolution.
const SWEEP_SECS: f32 = 1.0;

/// Default eviction radius in yards — five ADT tiles (2667 yd). Chosen as the smallest round
/// multiple of the tile grid comfortably clear of [`radius_floor`] (2560 yd): far enough that art
/// still on screen is never evicted, close enough that leaving one city for another recovers it
/// (Stormwind → Booty Bay is ~5500 yd, Ironforge → Stormwind ~4250).
const DEFAULT_RADIUS_YD: f32 = 5.0 * TILE_SIZE;

/// The smallest radius that cannot evict art the streamer is still holding: the far **corner** of
/// the widest tile block the residency window can keep, plus a tile of margin.
///
/// A stamp records where *the viewer* was when the art was last wanted, not where the art is — so an
/// entry can be up to a streaming reach "behind" the art it belongs to. Flooring the radius at the
/// block's circumscribed radius means you have to leave the art's neighbourhood entirely before its
/// dedup expires. The window follows the live `farclip` (1513), so the floor is taken at the clamp's
/// **max** — the widest the window can ever be — rather than tracking the slider: a floor that
/// shrank with the view distance would buy nothing (the default clears it either way) and would
/// have to re-resolve on every drag.
fn radius_floor() -> f32 {
    crate::terrain_stream::window::max_resident_reach_yd(*crate::view::FARCLIP_RANGE.end())
}

/// Resolve `$WOW_ART_RADIUS` (yards; `0` or negative ⇒ eviction off, the A/B leg that reproduces the
/// unbounded behaviour on a fixed build) against the streamer's reach. An explicitly-requested radius
/// below [`radius_floor`] is raised **and** warned about, rather than silently obeyed: a knob that
/// quietly means something else than it says is how decision 0789 happened.
fn radius_from_env() -> f32 {
    let floor = radius_floor();
    match std::env::var("WOW_ART_RADIUS")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
    {
        Some(r) if r <= 0.0 => 0.0,
        Some(r) if r < floor => {
            warn!(
                "art-scope: WOW_ART_RADIUS={r} is inside the streamer's own reach ({floor:.0} yd \
                 at the widest view distance) — using {floor:.0}"
            );
            floor
        }
        Some(r) => r,
        None => DEFAULT_RADIUS_YD.max(floor),
    }
}

/// Which cache a census row is about. Fixed set, in journal-column order — the CSV's columns only
/// ever grow at the end, so this enum's order is part of the file format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtSlot {
    /// `Placements::materials` — streamed doodad/WMO submesh materials.
    PlaceMats,
    /// `ModelMaterials` — every authored-batch material: creatures, players, GameObjects, the
    /// character composites, the booth scenes.
    ModelMats,
    /// `SkinComposites` — composited character body atlases.
    Skins,
    /// `WorldAssets::model_materials` — the ground-clutter material dedup.
    ClutterMats,
    /// `WorldAssets::textures` — decoded world BLPs by (path, wrap).
    Textures,
    /// `ClutterGeometry` — CPU submesh copies of every detail M2 decoded.
    ClutterGeo,
}

impl ArtSlot {
    /// Every slot, in journal-column order.
    pub const ALL: [ArtSlot; 6] = [
        ArtSlot::PlaceMats,
        ArtSlot::ModelMats,
        ArtSlot::Skins,
        ArtSlot::ClutterMats,
        ArtSlot::Textures,
        ArtSlot::ClutterGeo,
    ];

    /// The journal's column name for this slot.
    pub(crate) fn column(self) -> &'static str {
        match self {
            ArtSlot::PlaceMats => "pmat",
            ArtSlot::ModelMats => "emat",
            ArtSlot::Skins => "skin",
            ArtSlot::ClutterMats => "cmat",
            ArtSlot::Textures => "tex",
            ArtSlot::ClutterGeo => "cgeo",
        }
    }

    fn idx(self) -> usize {
        match self {
            ArtSlot::PlaceMats => 0,
            ArtSlot::ModelMats => 1,
            ArtSlot::Skins => 2,
            ArtSlot::ClutterMats => 3,
            ArtSlot::Textures => 4,
            ArtSlot::ClutterGeo => 5,
        }
    }
}

/// Per-cache residency, refreshed by every [`ArtScope::apply`] call — the instrument that localizes
/// a residency ratchet to **one cache** in a single journal row, instead of the run-length probe that
/// found B131. `Assets<T>::len()` says "materials are growing"; this says which map is holding them.
#[derive(Resource, Default)]
pub struct ArtCensus {
    live: [usize; ArtSlot::ALL.len()],
    dropped: [usize; ArtSlot::ALL.len()],
}

impl ArtCensus {
    /// Live entries in one cache as of its last sweep.
    pub fn live(&self, slot: ArtSlot) -> usize {
        self.live[slot.idx()]
    }

    /// Entries dropped by distance across every cache since the run began.
    pub fn dropped_total(&self) -> usize {
        self.dropped.iter().sum()
    }
}

/// The sweep's shared state: the radius, this tick's focus, and whether this frame is a sweep frame.
#[derive(Resource, Default)]
pub struct ArtScopeState {
    /// Eviction radius in yards; `0` ⇒ never evict. Zero until [`configure_art_scope`] runs, so
    /// nothing can be dropped before the streamer's reach is known.
    radius: f32,
    /// The view focus, wow coords (`crate::terrain_stream::view_focus` — the same ladder the
    /// streamer and the WDL ring use). `None` when there is no focus at all (no avatar, no camera),
    /// which suspends both stamping and sweeping.
    focus: Option<[f32; 3]>,
    /// Set on the one frame per [`SWEEP_SECS`] that sweeps.
    due: bool,
    last_sweep: f32,
    /// Wall seconds since app start, this frame — the clock the dwell floor is measured on.
    now: f32,
}

impl ArtScopeState {
    /// The view focus this frame, wow coords — what the sweep measures from. Exposed for the FPS
    /// journal: its `x,y,z` are the *avatar*, which stands still through a whole detached free-fly, so
    /// on that leg the position columns say nothing about where the art was being asked for. This is
    /// the column that does.
    pub fn focus(&self) -> Option<[f32; 3]> {
        self.focus
    }
}

/// What an owning module needs to scope its caches: the policy, plus the census to report into.
#[derive(SystemParam)]
pub struct ArtScope<'w> {
    state: Res<'w, ArtScopeState>,
    census: ResMut<'w, ArtCensus>,
}

impl ArtScope<'_> {
    /// Sweep one cache (on sweep frames) and record its residency. Safe to call for a cache that is
    /// empty, unconfigured, or in a run with eviction switched off — it degrades to a census read.
    pub fn apply<K: Eq + Hash, V>(&mut self, cache: &mut SpatialCache<K, V>, slot: ArtSlot) {
        let dropped = match self.state.focus {
            Some(focus) if self.state.due && self.state.radius > 0.0 => {
                cache.scope(focus, self.state.radius, self.state.now)
            }
            _ => 0,
        };
        let i = slot.idx();
        self.census.live[i] = cache.len();
        self.census.dropped[i] += dropped;
        if dropped > 0 {
            debug!(
                "art-scope: dropped {dropped} {} entries beyond {:.0} yd ({} live)",
                slot.column(),
                self.state.radius,
                cache.len()
            );
        }
    }
}

/// Resolve the radius once at startup (the floor is a constant of the view-distance clamp).
fn configure_art_scope(mut state: ResMut<ArtScopeState>) {
    state.radius = radius_from_env();
    if state.radius > 0.0 {
        info!(
            "art-scope: within-map art evicts beyond {:.0} yd of the view focus",
            state.radius
        );
    } else {
        warn!("art-scope: eviction OFF ($WOW_ART_RADIUS=0) — within-map residency is unbounded");
    }
}

/// Publish this frame's focus and decide whether it is a sweep frame. Deliberately unordered against
/// every consumer — see the module docs.
fn track_art_scope(
    mut state: ResMut<ArtScopeState>,
    time: Res<Time<Real>>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    camera: Query<&Transform, With<crate::view::WorldCamera>>,
) {
    let cam = camera.single().ok().map(|c| c.translation);
    state.focus = (focus.body_pos().is_some() || cam.is_some()).then(|| focus.resolve(cam));
    let now = time.elapsed_secs();
    state.now = now;
    state.due = now - state.last_sweep >= SWEEP_SECS;
    if state.due {
        state.last_sweep = now;
    }
}

/// Owns the sweep policy + the census. The caches themselves stay private to their modules; each
/// registers its own three-line system that hands its caches to [`ArtScope::apply`].
pub(crate) struct ArtScopePlugin;

impl Plugin for ArtScopePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArtCensus>()
            .init_resource::<ArtScopeState>()
            .add_systems(Startup, configure_art_scope)
            .add_systems(Update, track_art_scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The cache's own behaviour (stamping, the dwell floor, altitude, handle drop) is tested where
    // the cache now lives — `benilla_assets::spatial_cache`. What is left here is the half that
    // stayed: how wide the sweep reaches, and that an unconfigured run still bounds itself.

    /// Eviction must never outrun the streamer: the default radius clears the far corner of the
    /// widest block the window keeps at the clamp's max view distance.
    #[test]
    fn the_radius_clears_the_streamers_own_reach() {
        assert!(
            DEFAULT_RADIUS_YD > radius_floor(),
            "default {DEFAULT_RADIUS_YD} must clear the floor {}",
            radius_floor()
        );
        // (26 + 1 + 16) chunks per axis at 777, the corner of that, plus a tile: ~2560 yd.
        assert!((radius_floor() - 2560.0).abs() < 1.0, "{}", radius_floor());
    }

    /// With `$WOW_ART_RADIUS` unset (every ordinary run, and the test binary) the radius is the
    /// default, floored — never zero, so a shipped build always bounds itself.
    #[test]
    fn an_unconfigured_run_still_bounds_itself() {
        assert_eq!(radius_from_env(), DEFAULT_RADIUS_YD.max(radius_floor()));
    }
}
