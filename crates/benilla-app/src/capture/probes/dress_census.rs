//! `WOW_DRESS_CENSUS=<secs>[,<every>]`: one line per streamed player near the body giving what the
//! wire asked for, what we resolved and what hangs off the skeleton.
//!
//! - `flags`/`hide`: `PLAYER_FLAGS` and its `HIDE_HELM 0x400`/`HIDE_CLOAK 0x800` bits, a public
//!   field, so every player in range is readable.
//! - `helm`/`cloak`: the resolved `ItemDisplayInfo` ids ([`Equipment`]); `0` is not dressed.
//! - `spawned`: the attach slots standing under the unit ([`HeldAttached::spawned_slots`]).
//! - `contradictions`: bodies whose flags hide a piece that is dressed anyway, tagged `!!`.
//!
//! How to run it: `docs/CONTRIBUTING.md`, "Running it unattended".

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use super::ProbeClock;
use crate::entities::{Equipment, HeldAttached, ATTACH_SLOT_NAMES};
use crate::names::NameCache;
use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};

/// Census radius in yards, past what the server streams; `WOW_DRESS_CENSUS_RADIUS` overrides.
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

/// [`DressCensusPlugin`] state; `every` of 0 fires once.
#[derive(Resource)]
struct DressCensus {
    next: f32,
    every: f32,
}

/// What the census reads per entity.
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
    // The self body's parts, one `DRESS_PART` line each.
    body_parts: crate::entities::BodyPartsDesc,
    entities: Query<DressQuery>,
    // `parts=`/`mats=`: mesh parts under the unit and the distinct materials they bind.
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

    // `(is_contradiction, sort key, line)`; contradictions sort first.
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
        // The wire hides a piece that is resolved onto the body or standing as an attach model.
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
