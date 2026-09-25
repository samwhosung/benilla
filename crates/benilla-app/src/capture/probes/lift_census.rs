//! `WOW_LIFT_CENSUS=<secs>[,<every>]`: per type-11 elevator and type-15 boat, whether it
//! streamed, armed, built a model and where it is in its cycle. A transport spawns
//! [`Visibility::Hidden`] and its first ticked pose unhides it, so one that never arms is present,
//! solid and invisible.
//!
//! - `state`: `lift`/`taxi` (armed), `seed` (a type-11 waiting for its keyframe catalog), `bare`
//!   (anchored, no drive), `parked` (no anchor).
//! - `vis`/`inh`: the root's [`Visibility`] and the propagated [`InheritedVisibility`]; `hidden=`
//!   counts transports the tick never showed or judged off the live [`CurrentMap`] (`map=`).
//! - `meshes`: render descendants of the whole subtree.
//!
//! The repeat form tells a transient `state=seed` from a stuck one. How to run it:
//! `docs/CONTRIBUTING.md`, "Running it unattended".

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::camera::visibility::InheritedVisibility;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::VisualAttached;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::transport::{ElevatorSeed, Transport, TransportAnchor};
use benilla_world::world_map::CurrentMap;

/// Depth of the render-descendant walk; a GO's submeshes sit two levels down (root, anim host).
const WALK_DEPTH: u32 = 8;

pub(crate) struct LiftCensusPlugin;

impl Plugin for LiftCensusPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_LIFT_CENSUS").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
        let at = parts.next().filter(|v| *v > 0.0).unwrap_or(30.0);
        let every = parts.next().unwrap_or(0.0);
        app.insert_resource(LiftCensus { next: at, every })
            .add_systems(Update, fire_lift_census);
    }
}

/// [`LiftCensusPlugin`] state; `every` of 0 fires once.
#[derive(Resource)]
struct LiftCensus {
    next: f32,
    every: f32,
}

/// What the census reads per entity; each transport component is a distinct arm stage.
type CensusQuery = (
    &'static Guid,
    &'static NetEntity,
    &'static Transform,
    &'static ObjectStore,
    Option<&'static Transport>,
    Option<&'static TransportAnchor>,
    Has<ElevatorSeed>,
    Option<&'static Visibility>,
    Option<&'static InheritedVisibility>,
    Option<&'static Children>,
    Has<VisualAttached>,
);

/// One line per streamed transport, hidden first, then by distance. No radius: vmangos sends a
/// map's whole transport set at world entry (`Map::SendInitTransports`, `Map.cpp:1718`).
fn fire_lift_census(
    mut probe: ResMut<LiftCensus>,
    time: ProbeClock,
    current_map: Option<Res<CurrentMap>>,
    body: Query<&Transform, With<SelfPlayer>>,
    entities: Query<CensusQuery>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
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
    let at = body.single().ok().map(|t| t.translation);

    let mut rows: Vec<(bool, i64, String)> = Vec::new();
    let (mut hidden_n, mut unarmed_n, mut meshless_n) = (0u32, 0u32, 0u32);
    for (guid, net, t, store, transport, anchor, seed, vis, inherited, kids, attached) in &entities
    {
        let go_type = store.0.gameobject_type_id();
        // The GO types the reference's per-frame tick `0x630970` fires, plus anything carrying a
        // transport component, so a misread type field still lists.
        if !(net.kind == EntityKind::GameObject && matches!(go_type, 11 | 15)
            || transport.is_some()
            || anchor.is_some()
            || seed)
        {
            continue;
        }
        let state = match (transport, anchor, seed) {
            (Some(tr), _, _) => tr.drive_label(),
            (None, Some(_), true) => "seed",
            (None, Some(_), false) => "bare",
            (None, None, _) => "parked",
        };
        let hidden = vis == Some(&Visibility::Hidden) || inherited.is_some_and(|i| !i.get());
        let mesh_n = render_descendants(entity_children(kids), &children, &meshes);
        if hidden {
            hidden_n += 1;
        }
        if transport.is_none() {
            unarmed_n += 1;
        }
        if attached && mesh_n == 0 {
            meshless_n += 1;
        }
        let cycle = transport.zip(anchor).map(|(tr, a)| tr.cycle_ms(a));
        let sample = transport
            .zip(anchor)
            .map(|(tr, a)| tr.sample_at(a, current_map.as_ref().map_or(0, |m| m.0)));
        let wow = bevy_to_wow(t.translation);
        let dist = at.map_or(f32::NAN, |b| t.translation.distance(b));
        rows.push((
            hidden,
            (dist * 100.0) as i64,
            format!(
                "LIFT {:#018x} entry={:<7} disp={:<6} type={go_type:<3} state={state:<7} \
                 period={:<6} cycle={:<6} moving={} vis={:<9} inh={} meshes={mesh_n:<3} \
                 attached={} pos=({:.2},{:.2},{:.2}) d={dist:.1}",
                guid.0,
                store.0.object_entry().unwrap_or(0),
                net.display_id.unwrap_or(0),
                transport.map_or(0, Transport::period_ms),
                cycle.map_or_else(|| "-".into(), |c| c.to_string()),
                sample.map_or(0, |s| u8::from(s.moving)),
                vis.map_or_else(|| "absent".into(), |v| format!("{v:?}")),
                inherited.map_or(0, |i| u8::from(i.get())),
                u8::from(attached),
                wow[0],
                wow[1],
                wow[2],
            ),
        ));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    println!(
        "LIFT_CENSUS t={now:.1} transports={} hidden={hidden_n} unarmed={unarmed_n} \
         meshless={meshless_n} map={} body={}",
        rows.len(),
        current_map.map_or_else(|| "none".into(), |m| m.0.to_string()),
        at.map_or_else(
            || "none".into(),
            |b| {
                let w = bevy_to_wow(b);
                format!("({:.2},{:.2},{:.2})", w[0], w[1], w[2])
            }
        ),
    );
    for (_, _, line) in &rows {
        println!("{line}");
    }
}

/// The children to start the walk from, or none.
fn entity_children(kids: Option<&Children>) -> Vec<Entity> {
    kids.map(|c| c.iter().collect()).unwrap_or_default()
}

/// Counts render descendants of the whole subtree; a GO's submeshes hang under an anim host.
fn render_descendants(
    roots: Vec<Entity>,
    children: &Query<&Children>,
    meshes: &Query<(), With<Mesh3d>>,
) -> u32 {
    let mut frontier = roots;
    let mut found = 0;
    for _ in 0..WALK_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for e in frontier.drain(..) {
            found += u32::from(meshes.contains(e));
            if let Ok(kids) = children.get(e) {
                next.extend(kids.iter());
            }
        }
        frontier = next;
    }
    found
}
