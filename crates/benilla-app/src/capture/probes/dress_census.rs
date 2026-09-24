//! The **dress census** (`WOW_DRESS_CENSUS=<secs>[,<every>]`) — one line per streamed player near
//! the body saying what the wire asked for, what we resolved, and what is actually hanging off the
//! skeleton right now.
//!
//! It exists because those are three different things and a screenshot conflates all three. A
//! helm or cloak the player chose to hide, drawn anyway, is exactly a gap between the
//! first and the third: the descriptor carried `PLAYER_FLAGS_HIDE_HELM`, the resolver ignored it,
//! and a helm model hung off the head. Nothing in a picture separates that from "this character
//! simply has a helm equipped", which is why the report needed a reader rather than an eye.
//!
//! ```text
//! DRESS_CENSUS t=20.0 players=2 hiding-helm=1 hiding-cloak=1 contradictions=0 radius=120 body=(-8949.95,-132.49,83.57)
//! DRESS 0xF150000000000038 d=   0.0 flags=0x00020c02 hide=helm+cloak helm=0      cloak=0      settled=1 spawned=[main,shL,shR] Probethree
//! DRESS 0xF150000000000079 d=  12.4 flags=0x00020002 hide=-          helm=32154  cloak=41205  settled=1 spawned=[main,off,helm,shL,shR] Naz
//! ```
//!
//! - **`flags` / `hide`** are the wire's own: `PLAYER_FLAGS` and its `HIDE_HELM 0x400` /
//!   `HIDE_CLOAK 0x800` bits, which are PUBLIC — so every player in range is readable here, not
//!   only our own body, and "does a remote player's preference reach us" is a line rather than an
//!   argument.
//! - **`helm` / `cloak`** are the resolved `ItemDisplayInfo` ids ([`Equipment`]) — `0` means the
//!   body is dressed without that piece, whether because nothing is equipped there or because the
//!   preference suppressed it.
//! - **`spawned`** is the attach-slot list actually standing under the unit
//!   ([`HeldAttached::spawned_slots`]) — the visual truth, read off the ECS rather than inferred.
//! - **`contradictions`** is the headline number and the whole point: a body whose flags say hide
//!   while a `helm` display or a spawned `helm` slot says otherwise is tagged `!!` and counted.
//!   B123 reproduces as `contradictions=1`; the fix reads `contradictions=0` with `hide=` still
//!   set, which is the distinction "the helm is gone" alone cannot make (it is also what a naked
//!   head looks like when the *preference* never arrived).
//!
//! ```text
//! WOW_USER=probeN WOW_PASS=pprobeN WOW_CHAR=<geared body> WOW_NOSOUND=1 \
//!   WOW_DRESS_CENSUS="20,10" WOW_PROBE_EXIT_AT=45 cargo run -q -p benilla
//! ```

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::{Equipment, HeldAttached, ATTACH_SLOT_NAMES};
use crate::names::NameCache;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};

/// How far from the body the census looks, in yards — the unit-visual census's radius, for the
/// same reason: comfortably past what the server streams, so an empty census means an empty scene.
const DEFAULT_RADIUS: f32 = 120.0;

pub(crate) struct DressCensusPlugin;

impl Plugin for DressCensusPlugin {
    fn build(&self, app: &mut App) {
        let raw = std::env::var("WOW_DRESS_CENSUS").unwrap_or_default();
        let mut parts = raw
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(0.0));
        let at = parts.next().filter(|v| *v > 0.0).unwrap_or(20.0);
        let every = parts.next().unwrap_or(0.0);
        app.insert_resource(DressCensus { next: at, every })
            .add_systems(Update, fire_dress_census);
    }
}

/// [`DressCensusPlugin`] state: when the next census fires, and how often after that (`0` = once).
#[derive(Resource)]
struct DressCensus {
    next: f32,
    every: f32,
}

/// What the census reads per entity: identity, kind, pose, the resolved worn set, and the attach
/// roots actually standing.
type DressQuery = (
    Entity,
    &'static Guid,
    &'static NetEntity,
    &'static Transform,
    &'static ObjectStore,
    Option<&'static Equipment>,
    Option<&'static HeldAttached>,
);

/// One line per streamed player within [`DEFAULT_RADIUS`], contradictions first.
fn fire_dress_census(
    mut probe: ResMut<DressCensus>,
    time: ProbeClock,
    names: Res<NameCache>,
    body: Query<(Entity, &Transform), With<SelfPlayer>>,
    // The self body's parts, one line each (`DRESS_PART`) — the draw-population inventory.
    body_parts: crate::entities::BodyPartsDesc,
    entities: Query<DressQuery>,
    // The body's draw population (`parts=`/`mats=`): every mesh part hanging under the unit —
    // body batches, each attach model's batches, cards — and how many DISTINCT materials
    // they bind. The gap between the two is the ceiling on merging a body's batches by
    // material (1929's first proposal): what a merge could collapse, read off a dressed body
    // instead of guessed from a model's batch table.
    children: Query<&Children>,
    parts: Query<&MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>,
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
    let Ok((self_entity, body)) = body.single() else {
        println!("DRESS_CENSUS t={now:.1} NO BODY — not in world, nothing measured");
        return;
    };
    let radius = std::env::var("WOW_DRESS_CENSUS_RADIUS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(DEFAULT_RADIUS);

    // `(is_contradiction, sort key, line)`. A contradiction sorts to the top: it is the finding.
    let mut rows: Vec<(bool, i64, String)> = Vec::new();
    let (mut hiding_helm, mut hiding_cloak, mut bad) = (0u32, 0u32, 0u32);
    for (unit, guid, net, t, store, equipment, attached) in &entities {
        if net.kind != EntityKind::Player
            || t.translation.distance_squared(body.translation) > radius * radius
        {
            continue;
        }
        let flags = store.0.player_flags();
        let (hide_helm, hide_cloak) = (store.0.player_hides_helm(), store.0.player_hides_cloak());
        hiding_helm += u32::from(hide_helm);
        hiding_cloak += u32::from(hide_cloak);
        let eq = equipment.copied().unwrap_or_default();
        let spawned: Vec<&str> = attached
            .map(|a| a.spawned_slots())
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, e)| e.map(|_| ATTACH_SLOT_NAMES[i]))
            .collect();
        // The contradiction: the wire asked for a piece to be hidden and it is dressed anyway —
        // either resolved onto the body or standing as an attach model. This is B123's symptom,
        // stated as a predicate instead of a screenshot.
        let helm_shown = eq.helm != 0 || spawned.contains(&"helm");
        let contradiction = (hide_helm && helm_shown) || (hide_cloak && eq.cloak != 0);
        bad += u32::from(contradiction);
        let hide = match (hide_helm, hide_cloak) {
            (true, true) => "helm+cloak",
            (true, false) => "helm",
            (false, true) => "cloak",
            (false, false) => "-",
        };
        let dist = t.translation.distance(body.translation);
        let (n_parts, n_mats) = draw_population(unit, &children, &parts);
        rows.push((
            contradiction,
            (dist * 100.0) as i64,
            format!(
                "DRESS {:#018x} d={dist:6.1} flags={flags:#010x} hide={hide:<10} \
                 helm={:<6} cloak={:<6} settled={} spawned=[{}] parts={n_parts} mats={n_mats} {}{}",
                guid.0,
                eq.helm,
                eq.cloak,
                u8::from(eq.settled),
                spawned.join(","),
                names.peek(guid.0).unwrap_or("?"),
                if contradiction { "  !!" } else { "" },
            ),
        ));
    }
    rows.sort_by_key(|(bad, dist, _)| (!*bad, *dist));
    let at = bevy_to_wow(body.translation);
    println!(
        "DRESS_CENSUS t={now:.1} players={} hiding-helm={hiding_helm} hiding-cloak={hiding_cloak} \
         contradictions={bad} radius={radius:.0} body=({:.2},{:.2},{:.2})",
        rows.len(),
        at[0],
        at[1],
        at[2],
    );
    for (_, _, line) in &rows {
        println!("{line}");
    }
    for line in body_parts.describe(self_entity, &children) {
        println!("{line}");
    }
}

/// Mesh parts under `unit` (every depth) and the distinct materials they bind.
fn draw_population(
    unit: Entity,
    children: &Query<&Children>,
    parts: &Query<&MeshMaterial3d<benilla_assets::materials::WowModelMaterial>>,
) -> (usize, usize) {
    let mut n = 0usize;
    let mut mats = std::collections::HashSet::new();
    for e in children.iter_descendants(unit) {
        if let Ok(m) = parts.get(e) {
            n += 1;
            mats.insert(m.0.id());
        }
    }
    (n, mats.len())
}
