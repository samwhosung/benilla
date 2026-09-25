//! `WOW_GROUND_CENSUS=<secs>[,<every>]`: per ground-clamped body ([`ground_derived`], including a
//! player the server moves on a spline), whether it sits below the pose the server sent.
//!
//! - `seat`: the server's Z ([`GroundClamped::seat_y`]), before the ground clamp.
//! - `drop`: `seat − z`, how far the clamp pulled the unit down; `sunk` counts those past
//!   [`SUNK_YD`].
//! - `above`: the lowest walkable surface strictly above the feet.
//! - `terrain`: the MCNK height under it, what the clamp finds before a building's collider loads.
//!
//! The repeat form tells a transient sink during loading from one that stays. How to run it:
//! `docs/CONTRIBUTING.md`, "Running it unattended".

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use super::ProbeClock;
use crate::names::NameCache;
use crate::net::NetCommands;
use crate::net::{
    ground_derived, CreatureSwimming, Embodied, GroundClamped, Guid, NetEntity, RemoteMotion,
    SelfPlayer, Spline,
};

/// A drop past this (yd) counts as sunk: clear of the clamp's small corrections, under a storey.
const SUNK_YD: f32 = 0.5;

/// Where the stack walk starts above the feet (yd): past one storey, below most roofs.
const CEILING_YD: f32 = 12.0;

/// A surface within this of the feet (yd) is the one the unit stands on.
const ABOVE_EPS: f32 = 0.05;

pub(crate) struct GroundCensusPlugin;

impl Plugin for GroundCensusPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_GROUND_CENSUS").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
        let at = parts.next().filter(|v| *v > 0.0).unwrap_or(30.0);
        let every = parts.next().unwrap_or(0.0);
        let radius = std::env::var("WOW_GROUND_CENSUS_RADIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80.0);
        app.insert_resource(GroundCensus {
            next: at,
            every,
            radius,
        })
        .add_systems(Update, fire_ground_census);
    }
}

/// [`GroundCensusPlugin`] state; `every` of 0 fires once, `radius` from `WOW_GROUND_CENSUS_RADIUS`.
#[derive(Resource)]
struct GroundCensus {
    next: f32,
    every: f32,
    radius: f32,
}

/// What the census reads per unit; a swimmer is exempt from the clamp.
type CensusQuery = (
    &'static Guid,
    &'static NetEntity,
    &'static Transform,
    Option<&'static GroundClamped>,
    Option<&'static Spline>,
    Has<CreatureSwimming>,
    // The rest of `ground_derived`'s inputs, so the listing is the clamp's own population.
    Has<Embodied>,
    Has<RemoteMotion>,
);

/// One line per [`ground_derived`] body within [`GroundCensus::radius`], worst drop first, under
/// a summary line.
fn fire_ground_census(
    mut probe: ResMut<GroundCensus>,
    time: ProbeClock,
    // Warm from first sight of every unit; a `None` is a name still in flight.
    names: Res<NameCache>,
    net_commands: Res<NetCommands>,
    world: benilla_world::collision::WorldCollision,
    point: benilla_world::world_point::WorldPoint,
    body: Query<&Transform, With<SelfPlayer>>,
    units: Query<CensusQuery>,
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
    let Ok(body) = body.single() else {
        println!("GROUND_CENSUS t={now:.1} NO BODY — not in world, nothing measured");
        return;
    };
    let radius2 = probe.radius * probe.radius;

    let mut rows: Vec<(f32, bool, String)> = Vec::new();
    for (guid, net, t, clamped, spline, swimming, embodied, relayed) in &units {
        if !ground_derived(
            net.kind,
            spline.is_some(),
            clamped.is_some(),
            embodied,
            relayed,
        ) || t.translation.distance_squared(body.translation) > radius2
        {
            continue;
        }
        let z = t.translation.y;
        let seat = clamped.map_or(z, |c| c.seat_y);
        let drop = seat - z;
        let terrain = point.terrain_height_under(t.translation);
        let above = lowest_surface_above(&world, t.translation);
        let wow = bevy_to_wow(t.translation);
        let name = names
            .resolve(guid.0, &net_commands)
            .unwrap_or("?")
            .to_string();
        rows.push((
            drop,
            swimming,
            format!(
                "UGD {:#018x} {} {name:?} display={} pos=({:.2},{:.2},{:.2}) z={z:.2} seat={seat:.2} \
                 drop={drop:+.2} terrain={} above={} spline={} swim={}",
                guid.0,
                // The listing includes spline-driven players.
                if net.kind == EntityKind::Player {
                    "player"
                } else {
                    "unit"
                },
                // The key `WOW_CLAMP_TRACE=<display>` filters its per-frame readout by.
                net.display_id.unwrap_or(0),
                wow[0],
                wow[1],
                wow[2],
                terrain.map_or_else(|| "none".into(), |v| format!("{v:.2}")),
                above.map_or_else(|| "none".into(), |v| format!("{v:.2} ({:+.2})", v - z)),
                u8::from(spline.is_some()),
                u8::from(swimming),
            ),
        ));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    // A swimmer's wire Z is its swim depth and its seat is stale: listed, not counted.
    let sunk = rows
        .iter()
        .filter(|(d, swim, _)| *d > SUNK_YD && !swim)
        .count();
    let worst = rows
        .iter()
        .find(|(_, swim, _)| !swim)
        .map_or(0.0, |(d, _, _)| *d);
    let at = bevy_to_wow(body.translation);
    println!(
        "GROUND_CENSUS t={now:.1} units={} sunk={sunk} worst={worst:+.2} radius={:.0} \
         body=({:.2},{:.2},{:.2})",
        rows.len(),
        probe.radius,
        at[0],
        at[1],
        at[2],
    );
    for (_, _, line) in &rows {
        println!("{line}");
    }
}

/// The lowest walkable surface strictly above `feet`: casts down from [`CEILING_YD`] overhead,
/// restarting just under each hit, so the nearest floor overhead wins over the roof.
fn lowest_surface_above(
    world: &benilla_world::collision::WorldCollision,
    feet: Vec3,
) -> Option<f32> {
    let mut from = feet.y + CEILING_YD;
    let mut found = None;
    for _ in 0..8 {
        let reach = from - feet.y;
        if reach <= ABOVE_EPS {
            break;
        }
        let origin = Vec3::new(feet.x, from, feet.z);
        // A miss ends the walk keeping what it found: a cast can miss the feet's own surface by
        // a float hair.
        let Some(hit) = world.ray_body(origin, Dir3::NEG_Y, reach) else {
            break;
        };
        let y = origin.y - hit.distance;
        if y <= feet.y + ABOVE_EPS {
            break;
        }
        found = Some(y);
        from = y - 0.02;
    }
    found
}
