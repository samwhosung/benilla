//! The liquid subsystem against the **real 1.12.1 client files** — the tests that need actual ADT
//! and WMO bytes rather than a synthetic quad, so they live apart from either half's unit tests and
//! reach across both: they build surfaces the way `super::surface`'s spawn paths do and then ask
//! `super::query` the same questions the running client asks.
//!
//! Every one skips when the client isn't present (the repo never carries Blizzard data).

use bevy::prelude::*;

use super::query::{
    liquid_at, submersion_at, wet_footprint, LiquidClaim, LiquidSource, WaterChunkInfo, WmoPool,
};
use crate::wmo_portal::WmoRoom;
use benilla_assets::coords::{bevy_to_wow, placement_rotation, wow_to_bevy};
use benilla_formats::{parse_wmo_root, wmo_group_liquid_mesh, LiquidMesh, Submersion};

/// Every loaded liquid surface covering a position's tile neighbourhood, world-placed and
/// **owner-tagged** — the ADT's own MCLQ chunks and every WMO placement's MLIQ groups, i.e. the same
/// candidate set the running client's `WaterChunkInfo` query sees, scoped the same way.
///
/// Each WMO placement gets a synthetic instance id (its index here) standing in for the
/// `WmoPortalInstance` entity the app spawns; `containing_room` then answers the question the app's
/// interior down-ray answers at runtime — *which* placement the subject is standing in — so a test
/// can pose the real query with the real claim.
struct LiquidScene {
    surfaces: Vec<WaterChunkInfo>,
    /// Per placement: its synthetic instance id, model path, placement transform, and group
    /// bounding boxes (WMO model space) — the containment test.
    placements: Vec<Placement>,
}

struct Placement {
    instance: Entity,
    model: String,
    transform: Transform,
    group_boxes: Vec<([f32; 3], [f32; 3])>,
}

impl LiquidScene {
    /// The room a world position stands in: the first placement one of whose group bounding boxes
    /// contains it, and that group.
    ///
    /// A coarser test than the app's down-ray (which races collision FACES and the terrain), and
    /// deliberately so — a test that re-implemented the down-ray would be pinning the test's copy of
    /// it. Group-box containment is enough to answer "which building is this position in", which is
    /// the only thing these two sites turn on.
    fn containing_room(&self, wow: [f32; 3]) -> Option<WmoRoom> {
        self.placements.iter().find_map(|p| {
            let local = bevy_to_wow(
                p.transform
                    .compute_affine()
                    .inverse()
                    .transform_point3(wow_to_bevy(wow)),
            );
            let gi = p
                .group_boxes
                .iter()
                .position(|(lo, hi)| (0..3).all(|i| local[i] >= lo[i] && local[i] <= hi[i]))?;
            Some(WmoRoom {
                instance: p.instance,
                group: gi as u16,
            })
        })
    }

    /// Which model path a room's placement came from — so a test can state *which building* it
    /// believes the player is in rather than trusting an index.
    fn model_of(&self, room: WmoRoom) -> &str {
        self.placements
            .iter()
            .find(|p| p.instance == room.instance)
            .map_or("<none>", |p| p.model.as_str())
    }
}

/// Build the [`LiquidScene`] around a position. Empty (`None`) when the client isn't present.
fn liquid_scene(map: &str, wow: [f32; 3]) -> Option<LiquidScene> {
    let data = benilla_formats::wow_data()?;
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let (cx, cy) = benilla_formats::world_to_tile(wow[0], wow[1]);
    let mut scene = LiquidScene {
        surfaces: Vec::new(),
        placements: Vec::new(),
    };
    let mut seen_placements: Vec<u32> = Vec::new();
    // A building as big as Blackrock is placed in every tile it straddles, so its MODF may sit
    // in a neighbour of the tile the position falls in.
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            let (tx, ty) = ((cx as i32 + dx) as u32, (cy as i32 + dy) as u32);
            let Ok(tile) = benilla_formats::load_tile_mesh(&mut chain, map, tx, ty) else {
                continue;
            };
            for lq in tile.chunks.iter().flat_map(|c| c.liquids.iter()) {
                scene.surfaces.push(wet_footprint(
                    lq,
                    &Transform::IDENTITY,
                    LiquidSource::AdtChunk,
                ));
            }
            for w in &tile.wmos {
                // A straddling building appears in each tile's MODF — spawn it once, as the
                // streamer does (it dedups on the same `unique_id`).
                if seen_placements.contains(&w.unique_id) {
                    continue;
                }
                seen_placements.push(w.unique_id);
                let transform = Transform {
                    translation: wow_to_bevy(w.position),
                    rotation: placement_rotation(w.rotation),
                    scale: Vec3::ONE,
                };
                let root_path = w.model.to_ascii_lowercase();
                let Ok(bytes) = chain.read_file(&root_path) else {
                    continue;
                };
                let Ok(root) = parse_wmo_root(&bytes) else {
                    continue;
                };
                let instance =
                    Entity::from_raw_u32(scene.placements.len() as u32).expect("valid entity id");
                scene.placements.push(Placement {
                    instance,
                    model: root_path.clone(),
                    transform,
                    group_boxes: root
                        .group_infos()
                        .iter()
                        .map(|g| (g.bbox_min, g.bbox_max))
                        .collect(),
                });
                let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
                for gi in 0..root.group_count() {
                    let Ok(gb) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                        continue;
                    };
                    if let Some(lq) = wmo_group_liquid_mesh(&gb) {
                        scene.surfaces.push(wet_footprint(
                            lq_ref(&lq),
                            &transform,
                            // Built through the APP's own constructor, not a copy of the rule —
                            // a test that re-derived the floor would pin its own version of 0701.
                            LiquidSource::WmoGroup(WmoPool::new(
                                Some(WmoRoom {
                                    instance,
                                    group: gi as u16,
                                }),
                                &transform,
                                root.group_infos().get(gi as usize),
                            )),
                        ));
                    }
                }
            }
        }
    }
    (!scene.surfaces.is_empty()).then_some(scene)
}

fn lq_ref(lq: &LiquidMesh) -> &LiquidMesh {
    lq
}

/// The verdict at a position, and the highest wet vertex among the surfaces that claim it —
/// i.e. what the query answers now, beside what the chunk-maximum rule used to answer.
fn verdict(map: &str, wow: [f32; 3], claim: LiquidClaim) -> Option<(f32, f32)> {
    let all = liquid_scene(map, wow)?.surfaces;
    let hit = liquid_at(all.iter(), wow, claim)?;
    let old = all
        .iter()
        .filter(|w| w.surface_z_at(wow[0], wow[1]).is_some())
        .map(|w| w.chunk_max_z())
        .fold(f32::MIN, f32::max);
    Some((hit.surface_z, old))
}

/// **Blackrock Mountain's lava** at the director's `.go xyz -7531.21 -1123.64 172.58` (indoors,
/// `blackrock.wmo` group 038 — a 55×82 magma grid running 167.29 → 175.00 under a ~7° yaw
/// placement). The old chunk-maximum answered 175.00, i.e. 2.42 yd OVER the feet and well past
/// the 1.52 yd swim line, on a staircase whose lava is metres below.
#[test]
fn blackrock_lava_is_below_the_feet_not_above_it() {
    let feet = [-7531.21_f32, -1123.64, 172.58];
    // Blackrock's own placement owns the magma, and the player is in it.
    let scene = liquid_scene("Azeroth", feet);
    let claim = scene
        .as_ref()
        .and_then(|s| s.containing_room(feet))
        .map_or(LiquidClaim::Unknown, LiquidClaim::inside);
    let Some((surface, old_max)) = verdict("Azeroth", feet, claim) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    assert!(
        (old_max - 175.00).abs() < 0.05,
        "the chunk maximum this replaces (got {old_max})"
    );
    assert!(
        (surface - 168.45).abs() < 0.05,
        "the lava under the feet is the cell's own height (got {surface})"
    );
    assert!(
        surface < feet[2],
        "surface {surface} must be UNDER the feet {} — standing on the stairs, not swimming",
        feet[2]
    );
}

/// **Felfire Hill's river** at the director's `.go xyz 1983.97 -2875.84 98.00` (outdoors, one
/// MCNK's 9×9 MCLQ falling 95.78 → 99.56 across the chunk). The old chunk-maximum answered
/// 99.56 — 1.56 yd over the feet, just past the 1.52 yd swim line — while the player stands on
/// the bank with the water at their soles.
#[test]
fn felfire_hill_river_does_not_swim_on_the_bank() {
    let feet = [1983.97_f32, -2875.84, 98.00];
    let Some((surface, old_max)) = verdict("Kalimdor", feet, LiquidClaim::Outdoors) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    assert!(
        (old_max - 99.56).abs() < 0.05,
        "the chunk maximum this replaces (got {old_max})"
    );
    assert!(
        surface < feet[2],
        "surface {surface} must be UNDER the feet {} — standing on the bank",
        feet[2]
    );
    assert!(
        (feet[2] - surface) < 1.0,
        "…but only just: the water is at the player's soles (got {surface})"
    );
}

/// **B85 — Uldaman reads as UNDERWATER** (director repro, `.go xyz -6152.73 -2969.59 213.73`; the
/// `/liquid` line read `VERDICT Still surface z 399.64 (+185.91 over feet)`, `WmoGroup … WET-CELL`).
///
/// The claiming pool is **another building's**. At these feet the player stands in
/// `kz_uldaman_a.wmo` (WMOAreaTable id 1218) group 22 — which carries no MLIQ over this XY at all —
/// while the surface answering `+185.91` is group 1 of a nearby `md_mushroomcave.wmo` placement,
/// whose every group box excludes the player: their feet sit at local z −191.15 under a pool that
/// runs local z −5.65…0.00, i.e. 186 yd overhead in a cave they have never entered.
///
/// A footprint has no floor, so before 0696 "indoors" (a bare bool) admitted every MLIQ surface on
/// the map and the lowest one won. The two arms below are the bug and the fix on the same bytes.
#[test]
fn uldaman_is_not_submerged_in_a_mushroom_caves_pool() {
    let feet = [-6152.73_f32, -2969.59, 213.73];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The bug, on the shipped files: the pool B85 reported really is over this XY and really is
    // 185.91 yd up. Read off the surfaces with NO delegation at all — the reported number is a
    // property of the files, so the control must not run through any rule this test also judges.
    let reported = scene
        .surfaces
        .iter()
        .filter_map(|w| w.surface_z_at(feet[0], feet[1]))
        .filter(|z| *z > feet[2])
        .min_by(f32::total_cmp)
        .expect("the mushroom cave's pool is what B85 reported");
    assert!(
        (reported - 399.64).abs() < 0.05,
        "the surface B85 reported (got {reported})"
    );
    assert!(reported - feet[2] > 185.0, "…and 186 yd overhead");

    // Since 0701 that pool is rejected TWICE over, and the second bound is independent of the
    // first: even a subject with no interior claim at all is out of the cave's room, 186 yd under
    // its floor. B85 would not have needed the owner half.
    assert!(
        liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown).is_none(),
        "the cave's pool is below-the-floor rejected even for an unclassified subject"
    );

    // The fix: the player's room is Uldaman's own placement, and Uldaman has no pool here.
    let room = scene
        .containing_room(feet)
        .expect("the player stands inside a placement");
    assert!(
        scene.model_of(room).contains("uldaman"),
        "the containing placement is Uldaman, not the cave (got {})",
        scene.model_of(room)
    );
    assert!(
        liquid_at(scene.surfaces.iter(), feet, LiquidClaim::inside(room)).is_none(),
        "in Uldaman, no liquid — the cave's pool belongs to a building the player is not in"
    );
}

/// **The Undercity STOREY bug** — the same wrong-underwater-filter as B60, one
/// storey further in, and the half owner scoping could not reach. Live repro at
/// `.go xyz 1732.68 187.01 -65.70`, where `WOW_FOG_DUMP` read:
///
/// ```text
/// [submerged] Slime claim Inside(WmoRoom { instance: …, group: 182 }) eye [1731.9 187.0 -63.59]
///             over-xy  Slime z -64.48 g182,  Slime z 51.98 g10,  Slime z 51.98 g7
/// ```
///
/// The eye's own room (group 182) holds slime at −64.48, *below* the eye and so not submerging it.
/// What turned the screen green were groups 7 and 10 — Undercity's upper channels at z 51.98,
/// **115 yd overhead** — which owner scoping admits because they are the same placement.
///
/// The server agrees the eye is dry there: `.gps` reports `Liquid level: -64.478561`, i.e. 0.9 yd
/// *under* the eye.
#[test]
fn undercitys_upper_channels_do_not_submerge_the_rooms_below() {
    let eye = [1732.68_f32, 187.01, -63.59];
    let Some(scene) = liquid_scene("Azeroth", eye) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The control, straight off the files with no delegation: the pools that were claiming the eye
    // really are up at 51.98, and really are over this XY.
    let overhead: Vec<f32> = scene
        .surfaces
        .iter()
        .filter_map(|w| w.surface_z_at(eye[0], eye[1]))
        .filter(|z| *z > eye[2])
        .collect();
    assert!(
        !overhead.is_empty() && overhead.iter().all(|z| (z - 51.98).abs() < 0.05),
        "the surfaces over the eye are Undercity's upper channels at 51.98 (got {overhead:?})"
    );

    // …and they belong to the eye's OWN placement, which is why 0696's owner scoping let them
    // through: this is a storey bug, not a building bug.
    let room = scene
        .containing_room(eye)
        .expect("the eye stands inside a placement");
    assert!(
        scene.model_of(room).contains("undercity"),
        "the containing placement is Undercity (got {})",
        scene.model_of(room)
    );

    // The fix, asked of the rule the SCREEN runs: a pool never claims below its own room's floor,
    // so nothing over the eye submerges it and the filter stays off.
    assert_eq!(
        submersion_at(scene.surfaces.iter(), eye, LiquidClaim::inside(room)),
        Submersion::Dry,
        "a pool 115 yd overhead, in another storey of the same building, must not submerge the eye"
    );
    // And the room's OWN slime is untouched — step down into it and the screen goes green properly.
    let inside_the_slime = [eye[0], eye[1], -66.0];
    assert_eq!(
        submersion_at(
            scene.surfaces.iter(),
            inside_the_slime,
            LiquidClaim::inside(room)
        ),
        Submersion::Slime,
        "the floor bounds the pool to its room; it does not cost the room its own swim"
    );
    let hit = liquid_at(
        scene.surfaces.iter(),
        inside_the_slime,
        LiquidClaim::inside(room),
    )
    .expect("…and the surface itself still answers");
    assert!(
        (hit.surface_z - -64.48).abs() < 0.05,
        "…at the height the server reports (got {})",
        hit.surface_z
    );
}

/// **B60 — Undercity's Rogues' Quarter under Tirisfal's water** (director repro,
/// `.go xyz 1414.08 53.00 -62.26`; the `/liquid` line read `AdtChunk Still WET-CELL surface z 32.93
/// (+95.19 over feet)` while the player's own VERDICT was already `none`).
///
/// The ADT chunk's lake covers the XY 95 yd overhead and the rooms are cut into the rock beneath it.
/// The player's query had delegated since 0634 and read dry; the **camera-eye** probe and the
/// **per-unit** swim marker had not, so the screen took the underwater filter and the NPCs swam on
/// dry stone. All three subjects run this one predicate now, so the assertion below is what each of
/// them asks.
#[test]
fn the_rogues_quarter_is_not_under_tirisfals_lake() {
    let feet = [1414.08_f32, 53.00, -62.26];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The control: the ADT surface really is there, really covers this XY, and really is 95 yd up —
    // an un-delegated subject (the pre-0696 camera and creature marker) is submerged in it.
    let unscoped = liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown)
        .expect("Tirisfal's water covers this XY");
    assert!(
        (unscoped.surface_z - 32.93).abs() < 0.05
            && (unscoped.surface_z - feet[2] - 95.19).abs() < 0.05,
        "the surface B60 reported (got {})",
        unscoped.surface_z
    );
    // The fix: a subject standing in Undercity reads Undercity's own liquid — and there is none
    // over this XY, in any of its 38 liquid groups.
    let room = scene
        .containing_room(feet)
        .expect("the player stands inside a placement");
    assert!(
        scene.model_of(room).contains("undercity"),
        "the containing placement is Undercity (got {})",
        scene.model_of(room)
    );
    assert!(
        liquid_at(scene.surfaces.iter(), feet, LiquidClaim::inside(room)).is_none(),
        "indoors, the ADT lake overhead is not this subject's liquid"
    );
}

/// **The gradient the swim law is sized against**: Felwood's Felfire Hill
/// channel, along the run the live probe swam. A liquid surface is a heightfield, and *how far
/// from flat* is exactly what decides whether the swim latch's 1/36-yd hysteresis band can
/// absorb travelling along it — so the slope `player::swim`'s regression test drives is pinned
/// here against the shipped ADT instead of living as a constant someone can only take on faith.
#[test]
fn the_felfire_channel_falls_about_a_tenth_of_a_yard_per_yard() {
    let (downstream, upstream) = ([1953.97_f32, -2866.84, 0.0], [2013.97_f32, -2866.84, 0.0]);
    let Some(all) = liquid_scene("Kalimdor", downstream).map(|s| s.surfaces) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    let z = |at: [f32; 3]| {
        liquid_at(all.iter(), at, LiquidClaim::Outdoors)
            .unwrap_or_else(|| panic!("no river at {at:?}"))
            .surface_z
    };
    let slope = (z(upstream) - z(downstream)) / (upstream[0] - downstream[0]);
    assert!(
        (slope - 0.099).abs() < 0.005,
        "the channel's gradient over 60 yd (got {slope})"
    );
}
