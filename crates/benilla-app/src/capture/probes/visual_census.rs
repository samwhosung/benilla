//! `WOW_UNIT_VISUALS=<secs>[,<every>]`: per streamed entity, which visual its display got. A debug
//! cube means no model we could load; an invisible trigger creature has a model that draws nothing
//! in the reference, and should draw nothing here.
//!
//! - `cube`: the [`FallbackCube`] marker; `cubes=` on the summary counts them.
//! - `meshes`: direct render children, the body's own batches; `cube=0 meshes=0` is a trigger.
//! - `held`: attach slots on the skeleton ([`HeldAttached::spawned_slots`]), which `meshes`
//!   misses since an attached model is a grandchild.
//! - `pick`: parts the mouseover ray-tests across [`crate::target::hover::pick_model_roots`], or
//!   fallback-box children; `pick=0` is selectable only by nameplate.
//! - `PENDING`: a visual not built yet, as opposed to one that built nothing.
//!
//! The repeat form tells a transient cube from a stuck one. How to run it: `docs/CONTRIBUTING.md`,
//! "Running it unattended".

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::{FallbackCube, HeldAttached, VisualAttached, ATTACH_SLOT_NAMES};
use crate::names::NameCache;
use crate::net::{Guid, NetEntity, SelfPlayer};
use crate::target::hover::pick_model_roots;

/// Census radius in yards, past the server's visibility radius; `WOW_UNIT_VISUALS_RADIUS` sets it.
const DEFAULT_RADIUS: f32 = 120.0;

pub(crate) struct UnitVisualsPlugin;

impl Plugin for UnitVisualsPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_UNIT_VISUALS").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
        let at = parts.next().filter(|v| *v > 0.0).unwrap_or(45.0);
        let every = parts.next().unwrap_or(0.0);
        let radius = std::env::var("WOW_UNIT_VISUALS_RADIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_RADIUS);
        app.insert_resource(UnitVisuals {
            next: at,
            every,
            radius,
        })
        .add_systems(Update, fire_unit_visuals);
    }
}

/// [`UnitVisualsPlugin`] state; `every` of 0 fires once.
#[derive(Resource)]
struct UnitVisuals {
    next: f32,
    every: f32,
    radius: f32,
}

/// What the census reads per entity.
type VisualQuery = (
    Entity,
    &'static Guid,
    &'static NetEntity,
    &'static Transform,
    Option<&'static Children>,
    Has<VisualAttached>,
    // The attach slots on this unit's bones, which `Children` does not reach.
    Option<&'static HeldAttached>,
    // Its mount, the third chained-model root the pick offers.
    Option<&'static crate::entities::mount::MountChild>,
    // For a `TYPEID_CORPSE`, which has no name: bone pile or dressed body.
    Option<&'static crate::net::ObjectStore>,
);

/// One line per streamed entity within [`UnitVisuals::radius`], cubes first, under a summary line.
fn fire_unit_visuals(
    mut probe: ResMut<UnitVisuals>,
    time: ProbeClock,
    names: Res<NameCache>,
    body: Query<&Transform, With<SelfPlayer>>,
    entities: Query<VisualQuery>,
    cubes: Query<(), With<FallbackCube>>,
    meshes: Query<(), With<Mesh3d>>,
    // The pick's populations as `update_hover` reads them: skinned parts per chained model root,
    // else fallback-box children.
    child_sets: Query<&Children>,
    rig_parts: Query<(), With<benilla_world::rig_palette::RigPart>>,
    box_parts: Query<(), With<benilla_world::interact::CreaturePickPart>>,
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
        println!("UNIT_VISUALS t={now:.1} NO BODY — not in world, nothing measured");
        return;
    };
    let radius2 = probe.radius * probe.radius;

    // `(is_cube, sort key, line)`; cubes sort first.
    let mut rows: Vec<(bool, i64, String)> = Vec::new();
    let (mut cube_n, mut blank_n, mut pending_n) = (0u32, 0u32, 0u32);
    let mut cube_displays: Vec<u32> = Vec::new();
    for (entity, guid, net, t, children, attached, held, mount, store) in &entities {
        if matches!(net.kind, EntityKind::DynamicObject | EntityKind::Other)
            || t.translation.distance_squared(body.translation) > radius2
        {
            continue;
        }
        let kids = children.map(|c| c.iter()).into_iter().flatten();
        let (mut cube, mut mesh_n) = (false, 0u32);
        for kid in kids {
            cube |= cubes.contains(kid);
            mesh_n += u32::from(meshes.contains(kid));
        }
        // The cube is a mesh child; exclude it so `meshes` counts real geometry.
        mesh_n = mesh_n.saturating_sub(u32::from(cube));
        let display = net.display_id.unwrap_or(0);
        if !attached {
            pending_n += 1;
        } else if cube {
            cube_n += 1;
            if !cube_displays.contains(&display) {
                cube_displays.push(display);
            }
        } else if mesh_n == 0 {
            blank_n += 1;
        }
        // What the mouseover tests, via the picker's own [`pick_model_roots`]; the fallback-box
        // leg mirrors `update_hover`'s AABB path for units with no skinned parts.
        let worn = held.map_or(&[][..], |h| h.spawned_slots().as_slice());
        let mut pick_n: u32 = pick_model_roots(entity, worn, mount.map(|m| m.0))
            .filter_map(|root| child_sets.get(root).ok())
            .flat_map(|kids| kids.iter())
            .filter(|kid| rig_parts.contains(*kid))
            .count() as u32;
        if pick_n == 0 {
            pick_n = children
                .map(|c| c.iter())
                .into_iter()
                .flatten()
                .filter(|kid| box_parts.contains(*kid))
                .count() as u32;
        }
        let dist = t.translation.distance(body.translation);
        let held: Vec<&str> = held
            .map(|h| {
                h.spawned_slots()
                    .iter()
                    .zip(ATTACH_SLOT_NAMES)
                    .filter_map(|(slot, name)| slot.map(|_| name))
                    .collect()
            })
            .unwrap_or_default();
        rows.push((
            cube,
            (dist * 100.0) as i64,
            format!(
                "UVIS {:#018x} {:<12} display={display:<6} d={dist:6.1} cube={} meshes={mesh_n:<3} \
                 pick={pick_n:<3} held=[{:<12}] {:<8} {}",
                guid.0,
                format!("{:?}", net.kind),
                u8::from(cube),
                held.join(","),
                if attached { "attached" } else { "PENDING" },
                // The name, or for a corpse which fork it took.
                match (net.kind, store) {
                    (EntityKind::Corpse, Some(s)) if s.0.corpse_is_bones() => "<bones>",
                    (EntityKind::Corpse, Some(_)) => "<corpse body>",
                    _ => names.peek(guid.0).unwrap_or("?"),
                },
            ),
        ));
    }
    rows.sort_by_key(|(cube, dist, _)| (!*cube, *dist));
    let at = bevy_to_wow(body.translation);
    println!(
        "UNIT_VISUALS t={now:.1} entities={} cubes={cube_n} cube_displays={cube_displays:?} \
         no-geometry={blank_n} pending={pending_n} radius={:.0} body=({:.2},{:.2},{:.2})",
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
