//! `WOW_TRAIL_CENSUS=<secs>[,<every>]`: per ribbon trail, its extent in the world and in its
//! transport's frame. A trail's length is `host speed × edgeLifetime`; on a moving deck the edges
//! ride the deck's pose, so a still rider on a vertical lift reads `world_dy ≈ 0`, where edges
//! frozen in the world would read `deck speed × edgeLifetime`.
//!
//! - `world_dy`: the strip's vertical extent in world space.
//! - `deck_ext`: the extent in the transport's frame. [`benilla_world::ride_frame::ride_matrix`]
//!   is a translation and a yaw, so vertically it equals the world reading; it differs only on
//!   a turning transport.
//!
//! Off a transport `deck=` reads `-` and the deck columns equal the world ones. A series across
//! the lift's travel shows whether the spread grows. How to run it: `docs/CONTRIBUTING.md`,
//! "Running it unattended".

use bevy::prelude::*;

use super::ProbeClock;
use benilla_world::ribbons::RibbonTrail;
use benilla_world::ride_frame::ride_matrix;

pub(crate) struct TrailCensusPlugin;

impl Plugin for TrailCensusPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_TRAIL_CENSUS").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
        let at = parts.next().filter(|v| *v > 0.0).unwrap_or(30.0);
        let every = parts.next().unwrap_or(0.0);
        app.insert_resource(TrailCensus { next: at, every })
            .add_systems(Update, fire_trail_census);
    }
}

/// [`TrailCensusPlugin`] state; `every` of 0 fires once.
#[derive(Resource)]
struct TrailCensus {
    next: f32,
    every: f32,
}

/// The axis-aligned extent of a point cloud; zero below two points (nothing committed yet).
fn extent(points: impl Iterator<Item = Vec3>) -> Vec3 {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut n = 0u32;
    for p in points {
        lo = lo.min(p);
        hi = hi.max(p);
        n += 1;
    }
    if n < 2 {
        return Vec3::ZERO;
    }
    hi - lo
}

/// One line per live ribbon trail, worst deck-relative vertical spread first.
fn fire_trail_census(
    mut probe: ResMut<TrailCensus>,
    time: ProbeClock,
    // Optional: a trail entity carries only a `Transform` (`ribbons.rs`'s `fade` note), so a
    // required `ViewVisibility` would match no trail.
    trails: Query<(Entity, &RibbonTrail, Option<&ViewVisibility>)>,
    // The deck's live pose, read through the same `ride_matrix` the sim uses.
    decks: Query<&GlobalTransform, Without<RibbonTrail>>,
    guids: Query<&crate::net::Guid>,
    // Ride-frame stamps, counted apart from the trails: a rider whose trail is not folded shows.
    stamped: Query<Entity, With<benilla_world::ride_frame::RideFrame>>,
) {
    let now = time.elapsed_secs();
    if probe.next <= 0.0 || now < probe.next {
        return;
    }
    probe.next = if probe.every > 0.0 {
        now + probe.every
    } else {
        -1.0
    };

    let mut rows: Vec<(f32, String)> = Vec::new();
    let (mut riding, mut worst) = (0u32, 0.0f32);
    let (mut deck_name, mut deck_y) = ("-".to_string(), f32::NAN);
    for (entity, trail, vis) in &trails {
        let world: Vec<Vec3> = trail.strip_world().collect();
        let w_ext = extent(world.iter().copied());
        // Into the deck's frame; off a transport the two columns agree.
        let deck = trail
            .deck()
            .and_then(|d| decks.get(d).ok().map(|gt| (d, gt)));
        let d_ext = match deck {
            Some((_, gt)) => {
                let inv = ride_matrix(gt).inverse();
                extent(world.iter().map(|p| inv.transform_point3(*p)))
            }
            None => w_ext,
        };
        if let Some((d, gt)) = deck {
            riding += 1;
            deck_y = gt.translation().y;
            deck_name = guids
                .get(d)
                .map_or_else(|_| format!("{d}"), |g| format!("{:#018x}", g.0));
        }
        worst = worst.max(w_ext.y);
        let (_, edges) = trail.shape();
        rows.push((
            d_ext.y,
            format!(
                "TRAIL {entity} bone={:<3} edges={edges:<3} life={:.2}s vis={} | \
                 world_dy={:5.2} world_ext={:5.2} deck_ext={:5.2}",
                trail.bone(),
                trail.edge_lifetime(),
                vis.map_or('-', |v| if v.get() { '1' } else { '0' }),
                w_ext.y,
                w_ext.max_element(),
                d_ext.max_element(),
            ),
        ));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    info!(
        "TRAIL_CENSUS t={now:.1} trails={} riding={riding} stamped={} deck={deck_name} \
         deckY={deck_y:.2} worst_world_dy={worst:.2}",
        rows.len(),
        stamped.iter().count()
    );
    for (_, line) in rows {
        info!("{line}");
    }
}
