//! The **unit-visual census** (`WOW_UNIT_VISUALS=<secs>[,<every>]`) — the instrument that closes
//! B13's symptom: *"the invisible trigger NPC displays as a black block"*.
//!
//! Every site of that bug arrived as a screenshot of a black slab, and a screenshot cannot say
//! which of the two things a slab is. Both look identical and they call for opposite fixes:
//!
//! - the entity's display named **no model we could load** — a gap of ours, and the cube is the
//!   debug signal that says so (it renders black rather than its authored red only because an
//!   unlit `StandardMaterial` catches no light in our scene);
//! - the entity's display named a model which **draws nothing** — how an invisible trigger
//!   creature hides in the real client, and nothing should be drawn at all.
//!
//! One line per streamed entity says which, without an eye:
//!
//! ```text
//! UVIS 0xF130003A7200013B Unit  display=13069 d=6.4  cube=1 meshes=0 pick=1 held=[] Zandalarian Event Generator
//! ```
//!
//! - **`cube`** is the [`FallbackCube`] marker — literally the arm that spawned it, not a guess
//!   from the picture. The headline `cubes=` count is the number that names the bug.
//! - **`meshes`** counts the entity's spawned render children — the BODY's own batches. `cube=0
//!   meshes=0` is the *correct* reading for a trigger creature: attached, and drawing nothing.
//! - **`held`** is the attach-slot list actually hanging off its skeleton
//!   ([`HeldAttached::spawned_slots`], the dress census's own reader). It is here because
//!   `meshes` counts *direct* children and an attached model is a grandchild, so a unit whose
//!   whole visible self is a held weapon — the Naxxramas `Unholy Axe`, an `InvisibleStalker` body
//!   with an axe in its hand — reads `meshes=0` whether the axe is there or not. That blind spot
//!   is what let decision 1656's defect sit under a green census; `held=[main]` is the line that
//!   says the weapon arrived.
//! - **`pick`** counts the parts the **mouseover** would actually ray-test — every skinned part
//!   across the unit's chained model roots ([`crate::target::hover::pick_model_roots`]: the body,
//!   everything it wears, its mount), plus its fallback-box children where it has no skinned parts
//!   at all. `pick=0` means the unit cannot be hovered or clicked *at all* and only its name plate
//!   can select it — which is the second half of what the Naxxramas weapon mobs reported and what
//!   neither `meshes` nor `held` could say: `held=[main]` proves the axe is *attached*, never that
//!   it is *clickable*.
//! - **`pending`** marks an entity whose visual has not been built yet (a model still streaming) —
//!   never to be confused with one that built nothing, which is the distinction the census exists
//!   to keep.
//!
//! Pair it with the checkout's probe identity (the `probe` skill) and a `.go` to the reported spot:
//!
//! ```text
//! WOW_USER=probeN WOW_PASS=pprobeN WOW_CHAR=Probe<n> WOW_NOSOUND=1 \
//!   WOW_PROBE_CHAT=".go xyz -11847.0 1280.9 3.2 0" \
//!   WOW_UNIT_VISUALS="45,15" WOW_PROBE_EXIT_AT=90 cargo run -q -p benilla
//! ```
//!
//! The repeat form is the load-race half: a census at 45 s and again at 60 s says whether a `cube`
//! is a *verdict* or just an entity whose model had not landed yet — the same transient-vs-ratchet
//! separation `WOW_GROUND_CENSUS` makes for the under-floor report.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::{FallbackCube, HeldAttached, VisualAttached, ATTACH_SLOT_NAMES};
use crate::names::NameCache;
use crate::net::{Guid, NetEntity, SelfPlayer};
use crate::target::hover::pick_model_roots;

/// How far from the body the census looks, in yards. Comfortably past the server's own creature
/// visibility radius for a spot the operator has just `.go`ne to, so "nothing found" means the
/// scene is empty rather than the window being tight.
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

/// [`UnitVisualsPlugin`] state: when the next census fires, how often after that (`0` = once), and
/// how far from the body to look.
#[derive(Resource)]
struct UnitVisuals {
    next: f32,
    every: f32,
    radius: f32,
}

/// What the census reads per entity: identity, kind + display, pose, and whether its visual has
/// been built yet.
type VisualQuery = (
    Entity,
    &'static Guid,
    &'static NetEntity,
    &'static Transform,
    Option<&'static Children>,
    Has<VisualAttached>,
    // The attach slots standing under this unit — the visual truth for everything that rides a
    // BONE rather than the entity, which `Children` above cannot see.
    Option<&'static HeldAttached>,
    // …and its mount, the third chained-model root the pick offers.
    Option<&'static crate::entities::mount::MountChild>,
    // The descriptor, read for exactly one thing here: which half of the corpse fork a
    // `TYPEID_CORPSE` row is on. A corpse has no name-cache entry, so without this
    // every corpse row reads `Corpse … ?` and cannot say whether `meshes=0` means "the bone-pile
    // model is missing" or "the dressed body is missing" — opposite findings.
    Option<&'static crate::net::ObjectStore>,
);

/// One line per streamed entity within [`UnitVisuals::radius`] of the body — cubes first, then
/// everything else — under a summary line naming the count that matters.
fn fire_unit_visuals(
    mut probe: ResMut<UnitVisuals>,
    time: ProbeClock,
    names: Res<NameCache>,
    body: Query<&Transform, With<SelfPlayer>>,
    entities: Query<VisualQuery>,
    cubes: Query<(), With<FallbackCube>>,
    meshes: Query<(), With<Mesh3d>>,
    // The pick's own populations, read exactly as `update_hover` reads them: the skinned parts of
    // each chained model root (pass 1/2) and, for a unit with none, its fallback-box children.
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

    // `(is_cube, sort key, line)`. Cubes sort to the top: they are the finding, and a busy city
    // census is long.
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
        // A cube IS a mesh child; count it as the cube it is so `meshes` reads as real geometry.
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
        // What the mouseover would test. The enumeration is the picker's own
        // ([`pick_model_roots`]) so the census can never report a set the pick doesn't use; the
        // fallback-box leg mirrors `update_hover`'s AABB path, which owns exactly the units the
        // skinned path does not.
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
                // The name, or — for a corpse, which never has one — which fork it took.
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
