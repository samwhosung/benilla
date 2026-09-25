//! The **transport census** (`WOW_LIFT_CENSUS=<secs>[,<every>]`) — the instrument that turns
//! *"the Thunder Bluff elevators aren't there"* into a line of numbers.
//!
//! An absent lift is a screenshot, and a screenshot cannot separate the four ways a type-11
//! elevator (or a type-15 boat) can fail to appear — each of which calls for a different fix:
//!
//! - the server never streamed it (**no entity** — nothing on the wire, our count is 0);
//! - it streamed but never **armed** (`state=seed`/`bare`): a transport spawns
//!   [`Visibility::Hidden`] and is unhidden by its first ticked pose, so an arm
//!   that never completes leaves a car that is present, solid, and permanently invisible —
//!   B168's "an invisible wall in its place";
//! - it armed and ticks, but the model never built (`meshes=0`) — an asset gap;
//! - it armed, ticks and draws, and the car is simply somewhere else in its cycle
//!   (`cycle=`/`pos=` say where, against the authored keyframes).
//!
//! ```text
//! LIFT 0xF11000000000504B entry=4170   disp=360   type=11 state=lift  period=30033 cycle=17421
//!      moving=0 vis=Inherited inh=1 meshes=4 attached=1 pos=(-1286.24,189.72,130.08) d=9.4
//! ```
//!
//! - **`state`** is the arm verdict read off the components themselves: `lift`/`taxi` (armed),
//!   `seed` (a type-11 waiting for its keyframe catalog), `bare` (an anchored transport with no
//!   drive — the boat arm waiting on a template, or a type-11 whose create carried no entry),
//!   `parked` (no anchor: a pathless type-11 the arm deliberately released, or a type the wire
//!   never flagged).
//! - **`vis`/`inh`** are the root's own [`Visibility`] and the propagated
//!   [`InheritedVisibility`]. `vis=Hidden` on a transport is the finding: either the tick never ran
//!   for it, or it ran and judged the car off-map. The headline `hidden=` count is that number, and
//!   `map=` beside it is the live [`CurrentMap`] the tick judged against — the pair that named
//!   decision 1654 (`hidden=11 … map=1`, every lift on Kalimdor, all of them armed and ticking).
//! - **`meshes`** counts render descendants (the whole subtree, not just direct children — a GO
//!   model hangs its submeshes under an anim host).
//!
//! Pair it with the checkout's probe identity (the `probe` skill) and a `.go` to the reported spot:
//!
//! ```text
//! WOW_USER=probeN WOW_PASS=pprobeN WOW_CHAR=Probe<n> WOW_NOSOUND=1 \
//!   WOW_PROBE_CHAT=".go xyz -1286.2 189.7 132.0 1" \
//!   WOW_LIFT_CENSUS="30,15" WOW_PROBE_EXIT_AT=75 cargo run -q -p benilla
//! ```
//!
//! The repeat form is the load-race half: a census at 30 s and again at 45 s says whether a
//! `state=seed` is a transient (the catalog had not opened yet) or the verdict.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::camera::visibility::InheritedVisibility;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::VisualAttached;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use crate::transport::{ElevatorSeed, Transport, TransportAnchor};
use benilla_world::world_map::CurrentMap;

/// How deep the render-descendant walk goes. A GO's submeshes sit at most a couple of levels
/// under the net root (root → anim host → submesh); eight is slack, not a limit anything reaches.
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

/// [`LiftCensusPlugin`] state: when the next census fires, and how often after that (`0` = once).
#[derive(Resource)]
struct LiftCensus {
    next: f32,
    every: f32,
}

/// What the census reads per entity: identity, kind/display, pose, the three transport
/// components (each one a distinct arm stage), the visibility pair, and the visual gate.
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

/// One line per streamed transport GameObject — hidden ones first, then by distance — under a
/// summary line naming the counts that matter. No radius: vmangos sends a map's **whole** transport
/// set at world entry and keeps it resident (`Map::SendInitTransports`, `Map.cpp:1719`; every
/// type-11 joins `m_transports` at `Map::Add`) — decision 0678 measured the same thing from a
/// ship's lanterns at 4853 yd. So the population is a handful and all of it is in reach.
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
        // Every ticking GO type (the pair the reference's per-frame tick `0x630970` fires) plus
        // anything already wearing a transport component — so a type field we misread still shows
        // up rather than vanishing from the instrument that exists to find it.
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

/// A borrowed child slice, or an empty one — the `Option<&Children>` unwrap the walk starts from.
fn entity_children(kids: Option<&Children>) -> Vec<Entity> {
    kids.map(|c| c.iter().collect()).unwrap_or_default()
}

/// Count the render descendants of a subtree — the whole subtree, because a GameObject's
/// submeshes hang under an anim host rather than directly off the net root.
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
