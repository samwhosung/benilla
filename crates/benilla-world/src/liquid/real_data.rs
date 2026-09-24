//! The liquid queries against the real client files: surfaces built as the spawn paths build them,
//! asked what the running client asks. Each test skips without an install.

use bevy::prelude::*;

use super::query::{
    liquid_at, submersion_at, wet_footprint, LiquidClaim, LiquidSource, WaterChunkInfo, WmoPool,
};
use crate::wmo_portal::WmoRoom;
use benilla_assets::coords::{bevy_to_wow, placement_rotation, wow_to_bevy};
use benilla_formats::{parse_wmo_root, wmo_group_liquid_mesh, LiquidMesh, Submersion};

/// The world-placed, owner-tagged liquid around a position: its tiles' MCLQ and every WMO
/// placement's MLIQ, each placement under a synthetic instance id for the app's entity.
struct LiquidScene {
    surfaces: Vec<WaterChunkInfo>,
    /// Each placement with its group boxes in model space, for the containment test.
    placements: Vec<Placement>,
}

struct Placement {
    instance: Entity,
    model: String,
    transform: Transform,
    group_boxes: Vec<([f32; 3], [f32; 3])>,
}

impl LiquidScene {
    /// The room a world position stands in: the first placement and group whose box contains it,
    /// coarser than the app's down-ray but enough to name the building.
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

    /// The model path of a room's placement.
    fn model_of(&self, room: WmoRoom) -> &str {
        self.placements
            .iter()
            .find(|p| p.instance == room.instance)
            .map_or("<none>", |p| p.model.as_str())
    }
}

/// The [`LiquidScene`] around a position; `None` without an install.
fn liquid_scene(map: &str, wow: [f32; 3]) -> Option<LiquidScene> {
    let data = benilla_formats::wow_data()?;
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let (cx, cy) = benilla_formats::world_to_tile(wow[0], wow[1]);
    let mut scene = LiquidScene {
        surfaces: Vec::new(),
        placements: Vec::new(),
    };
    let mut seen_placements: Vec<u32> = Vec::new();
    // A building is placed in every tile it straddles, so its MODF may sit in a neighbour tile.
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
                // Once per `unique_id`, as the streamer dedups a straddling building.
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
                            // The app's own constructor, so the test pins the app's floor rule.
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

/// The surface the query answers at a position, and the highest wet vertex of those over its XY.
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

/// Blackrock's lava stairs at `-7531.21 -1123.64 172.58`, in `blackrock.wmo` group 038: a 55×82
/// grid running 167.29 to 175.00 under a ~7° yaw, whose maximum sits 2.42 yd over the feet.
#[test]
fn blackrock_lava_is_below_the_feet_not_above_it() {
    let feet = [-7531.21_f32, -1123.64, 172.58];
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

/// Felfire Hill's river bank at `1983.97 -2875.84 98.00`: one MCNK's MCLQ falls 95.78 to 99.56,
/// and the water is at the player's soles.
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

/// Uldaman at `-6152.73 -2969.59 213.73`: the player is in `kz_uldaman_a.wmo` group 22, which has
/// no MLIQ here, 186 yd under group 1 of a `md_mushroomcave.wmo` placement's pool.
#[test]
fn uldaman_is_not_submerged_in_a_mushroom_caves_pool() {
    let feet = [-6152.73_f32, -2969.59, 213.73];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The control, off the surfaces with no delegation: the cave's pool is over this XY.
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

    // The floor alone rejects it, even for an unclassified subject 186 yd under the cave's room.
    assert!(
        liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown).is_none(),
        "the cave's pool is below-the-floor rejected even for an unclassified subject"
    );

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

/// Undercity at `1732.68 187.01 -63.59`: the eye's room (group 182) holds slime at −64.48, under
/// the eye, while groups 7 and 10 of the same placement run 115 yd overhead. The server's `.gps`
/// agrees the eye is dry: liquid level −64.478561.
#[test]
fn undercitys_upper_channels_do_not_submerge_the_rooms_below() {
    let eye = [1732.68_f32, 187.01, -63.59];
    let Some(scene) = liquid_scene("Azeroth", eye) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The control, with no delegation: the pools over this XY are up at 51.98.
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

    // They belong to the eye's own placement, so only the floor can reject them.
    let room = scene
        .containing_room(eye)
        .expect("the eye stands inside a placement");
    assert!(
        scene.model_of(room).contains("undercity"),
        "the containing placement is Undercity (got {})",
        scene.model_of(room)
    );

    assert_eq!(
        submersion_at(scene.surfaces.iter(), eye, LiquidClaim::inside(room)),
        Submersion::Dry,
        "a pool 115 yd overhead, in another storey of the same building, must not submerge the eye"
    );
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

/// Undercity's Rogues' Quarter at `1414.08 53.00 -62.26`, 95 yd under Tirisfal's lake: the player,
/// the camera eye and a unit all ask this one predicate.
#[test]
fn the_rogues_quarter_is_not_under_tirisfals_lake() {
    let feet = [1414.08_f32, 53.00, -62.26];
    let Some(scene) = liquid_scene("Azeroth", feet) else {
        eprintln!("skipping: no WoW client data");
        return;
    };
    // The control: an unscoped subject is submerged in the ADT lake 95 yd up.
    let unscoped = liquid_at(scene.surfaces.iter(), feet, LiquidClaim::Unknown)
        .expect("Tirisfal's water covers this XY");
    assert!(
        (unscoped.surface_z - 32.93).abs() < 0.05
            && (unscoped.surface_z - feet[2] - 95.19).abs() < 0.05,
        "the surface B60 reported (got {})",
        unscoped.surface_z
    );
    // Inside Undercity, none of its 38 liquid groups covers this XY.
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

/// The Felfire Hill channel's slope, the one `player::swim`'s regression test drives against the
/// swim latch's 1/36 yd band.
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
