//! The **ribbon-trail census** (`WOW_TRAIL_CENSUS=<secs>[,<every>]`) — the instrument that turns
//! *"the weapon particles trail behind while the lift goes up"* into a line of numbers.
//!
//! A streak in a screenshot cannot say whether it is **right**. A weapon trail is *supposed* to
//! smear — that is the whole effect, and its length is `host speed × edgeLifetime` by design. The
//! bug the director reported is not "there is a streak", it is that the streak is drawn against
//! the **world** while the host is standing still on a moving **deck**. Those two look identical
//! in a still frame and are one subtraction apart in numbers:
//!
//! - **`world_dy`** — the strip's vertical extent in world space, and **the observable**. A rider
//!   standing still on a moving deck must draw a streak that is *short in world space*: with the
//!   ride frame the edges are stored on the deck and re-projected through its live pose, so the
//!   whole strip travels rigidly with the car and never stretches. Without it the edges are frozen
//!   in the world while the car climbs out from under them, and the strip grows to
//!   `deck speed × edgeLifetime`. So on a **vertical** lift the reading is simply: `world_dy ≈ 0`
//!   is right, `world_dy ≈ deck speed × edgeLifetime` is the bug (decision 1591's unbuilt gap,
//!   1661's fix).
//! - **`deck_ext`** — the strip's extent measured *inside the transport's frame*. **It cannot
//!   differ from the world reading in Y**: `A` is `translate · Rz` (see
//!   [`benilla_world::ride_frame::ride_matrix`]), and a yaw plus a translation leaves a vertical
//!   extent invariant. It earns its column only on a **turning** transport — a boat swinging
//!   through a bend, where the in-plane extent does differ — and stands as a self-check on a lift,
//!   where it must equal the world one. (This column was first written claiming `world_dy` large
//!   with `deck_dy` small was the finding; that pair is unreachable by construction, and saying so
//!   here is cheaper than the next reader re-deriving it.)
//!
//! ```text
//! TRAIL_CENSUS t=85.0 trails=1 riding=1 stamped=1 deck=0xf12000104a00477a deckY=101.96 worst=0.00
//! TRAIL 168v1 bone=24  edges=25 life=0.50s vis=- | world_dy= 0.00 world_ext= 0.25 deck_ext= 0.25
//! ```
//!
//! Off a transport `deck=` reads `-` and the deck columns are the world ones, which is the control:
//! this census must not change on the ground, where a streak *should* smear behind a moving host.
//!
//! Pair it with the checkout's probe identity (the `probe` skill) and a `.go` onto the Thunder Bluff lift:
//!
//! ```text
//! WOW_USER=probeN WOW_PASS=pprobeN WOW_CHAR=Probe<n> WOW_NOSOUND=1 \
//!   WOW_PROBE_CHAT=".go xyz -1286.2 189.7 132.0 1" \
//!   WOW_TRAIL_CENSUS="35,5" WOW_PROBE_EXIT_AT=75 cargo run -q -p benilla
//! ```
//!
//! The repeat form is the point: one census says what the strip looked like at an instant, a
//! series across the lift's travel says whether the deck-relative spread *grows with the ride*.

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

/// [`TrailCensusPlugin`] state: when the next census fires, and how often after that (`0` = once).
#[derive(Resource)]
struct TrailCensus {
    next: f32,
    every: f32,
}

/// The axis-aligned extent of a point cloud — `(dx, dy, dz)`. Returns zeros for fewer than two
/// points, which is a trail that has committed nothing yet rather than a trail that is perfect.
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

/// One line per live ribbon trail, worst deck-relative spread first — the order that puts the
/// finding at the top when there are dozens of trails in a scene.
fn fire_trail_census(
    mut probe: ResMut<TrailCensus>,
    time: ProbeClock,
    // `ViewVisibility` is OPTIONAL and a trail never has one: a trail entity is spawned with a
    // `Transform` and nothing else, deliberately (`ribbons.rs`'s `fade` note — an emitter entity
    // carrying a `Visibility` would enlist in a second writer's query). Asking for
    // it as a required component matched ZERO trails in every scene, on the ground and on a lift
    // alike, and printed `trails=0` as if the client had spawned none.
    trails: Query<(Entity, &RibbonTrail, Option<&ViewVisibility>)>,
    // The deck's live pose. Disjoint from the trails above by construction — a transport is never
    // a ribbon — and read through the same `ride_matrix` the sim folds with, so the instrument
    // cannot drift from the thing it measures.
    decks: Query<&GlobalTransform, Without<RibbonTrail>>,
    guids: Query<&crate::net::Guid>,
    // The STAMP itself, counted separately from the trails: "nothing is riding" and "something is
    // riding but its trail isn't folded" are different bugs with different fixes, and a census
    // that cannot tell them apart sends you to the wrong half of the system.
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
        // Into the deck's frame. Off a transport this is the identity, so the two columns agree
        // and the line reads as its own control.
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
