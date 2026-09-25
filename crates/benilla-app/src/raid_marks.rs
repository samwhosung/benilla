//! Raid-target marker overhead billboards, the name render's marker leg (`0x6c709a`): each unit
//! holding a mark on the 8-slot board (`GroupState::raid_targets`) and without a live V-nameplate
//! (`[CGUnit+0xe60] == 0`; the plate shows its own raid icon) gets a world billboard, the unit
//! quad LUT `(-.5,1,0),(.5,1,0),(.5,0,0),(-.5,0,0)` over the 4-column `UI-RaidTargetingIcons`
//! atlas (`col = idx&3`, `row = idx>>2`, cell 0.25), indices `{0,1,3,3,1,2}`.
//!
//! Seat (`0x6c70d8`): with the unit's name shown, the bottom sits at
//! `anchor + (lineCount + 1) * scale`, one line pitch above the name block; else at the anchor.
//! `scale` is the names' height law ([`crate::nameplates::height_scale`]).
//!
//! Size (`0x6c7200`): a fixed one-unit world quad; `scale` never enters its geometry, and the
//! billboard chain (`0x7bca80`) renormalizes the basis to unit length.
//!
//! Drawn in the names' world pass, unlit and blended. Deviation: no depth write, as for the names,
//! because blended sorting looks the same.
//! The reference's third gate (`0x605f30() == 0`) is untraced and not applied.

use bevy::prelude::*;

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::nameplates::{height_scale, Nameplates};
use crate::net::GuidIndex;
use crate::ui_party::GroupState;
use crate::vplates::VPlates;
use benilla_world::view::WorldCamera;

/// The mark atlas: 4x2 icons in a 4-column grid, cell 0.25.
const MARK_TEXTURE: &str = "mpq://interface/targetingframe/ui-raidtargetingicons.blp";

/// The marker's world size (`0x6c7200`): one unit, independent of the name `scale`.
const MARK_WORLD_SIZE: f32 = 1.0;

/// One live marker: the billboard entity and the unit it rides.
#[derive(Clone, Copy)]
struct LiveMark {
    marker: Entity,
    unit: Entity,
}

/// The shared material, one lazy quad mesh per icon, and the live marker per board slot.
#[derive(Resource, Default)]
pub(crate) struct RaidMarks {
    material: Option<Handle<StandardMaterial>>,
    meshes: [Option<Handle<Mesh>>; 8],
    live: [Option<LiveMark>; 8],
}

/// A root-level mark billboard, following its unit.
#[derive(Component)]
struct RaidMarkBillboard;

/// The marker quad (`0x6c709a`) for one wire icon: the LUT, the atlas cell, the index list.
fn mark_mesh(icon: u32) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    let (u0, v0) = ((icon & 3) as f32 * 0.25, (icon >> 2) as f32 * 0.25);
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, 1.0, 0.0],
            [0.5, 1.0, 0.0],
            [0.5, 0.0, 0.0],
            [-0.5, 0.0, 0.0],
        ],
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![
            [u0, v0],
            [u0 + 0.25, v0],
            [u0 + 0.25, v0 + 0.25],
            [u0, v0 + 0.25],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 3, 3, 1, 2]));
    mesh
}

/// The marker's transform for `unit` this frame, seated per `0x6c70d8`. Generic over the
/// joint-globals filter, like [`overhead_anchor`].
fn mark_place<F: bevy::ecs::query::QueryFilter>(
    unit: Entity,
    tf: &Transform,
    plates: &Nameplates,
    facing: Quat,
    attach: &Query<&BoneAttach>,
    poses: &Query<&benilla_world::rig_anim::RigPose>,
    fallback: &Query<&OverheadFallback>,
    globals: &Query<&GlobalTransform, F>,
    mounts: &Query<(), With<crate::entities::mount::MountChild>>,
) -> Transform {
    let anchor = overhead_anchor(unit, tf, attach, poses, fallback, globals, mounts);
    // `scale` drives only the seat, never the quad size (`0x6c7200`).
    let scale = height_scale(anchor.y - tf.translation.y);
    let lift = match plates.line_count(unit) {
        Some(lines) => (lines as f32 + 1.0) * scale,
        None => 0.0,
    };
    Transform {
        translation: anchor + Vec3::Y * lift,
        rotation: facing,
        scale: Vec3::splat(MARK_WORLD_SIZE),
    }
}

/// Walks the 8-slot board, resolves each marked guid to its streamed entity (an unstreamed unit
/// draws nothing), applies the plate exclusion and (re)builds the billboards. Runs after the name
/// driver, whose line counts seat it; [`place_raid_marks`] re-seats every frame.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn drive_raid_marks(
    mut commands: Commands,
    group: Res<GroupState>,
    index: Res<GuidIndex>,
    vplates: Res<VPlates>,
    plates: Res<Nameplates>,
    units: Query<&Transform>,
    camera: Query<&Transform, With<WorldCamera>>,
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
    mut marks: ResMut<RaidMarks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    let Ok(cam_tf) = camera.single() else {
        return;
    };
    let facing = cam_tf.rotation;
    let material = marks
        .material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(asset_server.load::<Image>(MARK_TEXTURE)),
                // The names' world-pass state: unlit, depth-tested, blended, no depth write.
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                ..default()
            })
        })
        .clone();

    for slot in 0..8usize {
        let guid = group.raid_targets[slot];
        let unit = (guid != 0)
            .then(|| index.0.get(&guid).copied())
            .flatten()
            // The plate exclusion and a despawning unit both drop the marker.
            .filter(|e| !vplates.0.contains(e) && units.contains(*e));
        let stale = match (marks.live[slot], unit) {
            (Some(live), Some(unit)) if live.unit == unit => continue, // placed per frame below
            (live, _) => live,
        };
        if let Some(live) = stale {
            if let Ok(mut e) = commands.get_entity(live.marker) {
                e.despawn();
            }
            marks.live[slot] = None;
        }
        let Some(unit) = unit else {
            continue;
        };
        let mesh = marks.meshes[slot]
            .get_or_insert_with(|| meshes.add(mark_mesh(slot as u32)))
            .clone();
        let place = units.get(unit).map(|tf| {
            mark_place(
                unit,
                tf,
                &plates,
                facing,
                &anchor_q.0,
                &anchor_q.1,
                &anchor_q.2,
                &anchor_q.3,
                &anchor_q.4,
            )
        });
        let marker = commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                place.unwrap_or_default(),
                RaidMarkBillboard,
            ))
            .id();
        marks.live[slot] = Some(LiveMark { marker, unit });
    }
}

/// Seats every live marker from this frame's propagated pose, in the nameplate placer's window,
/// so a moving unit's mark never trails its name.
#[allow(clippy::type_complexity)]
fn place_raid_marks(
    marks: Res<RaidMarks>,
    plates: Res<Nameplates>,
    camera: Query<&Transform, (With<WorldCamera>, Without<RaidMarkBillboard>)>,
    units: Query<&Transform, (Without<RaidMarkBillboard>, Without<WorldCamera>)>,
    mut mark_tfs: Query<(&mut Transform, &mut GlobalTransform), With<RaidMarkBillboard>>,
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform, Without<RaidMarkBillboard>>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
) {
    let Ok(cam_tf) = camera.single() else {
        return;
    };
    let facing = cam_tf.rotation;
    for live in marks.live.iter().flatten() {
        let (Ok(tf), Ok((mut mtf, mut mglobal))) =
            (units.get(live.unit), mark_tfs.get_mut(live.marker))
        else {
            continue; // spawned this frame and not flushed, or the unit is despawning
        };
        let place = mark_place(
            live.unit,
            tf,
            &plates,
            facing,
            &anchor_q.0,
            &anchor_q.1,
            &anchor_q.2,
            &anchor_q.3,
            &anchor_q.4,
        );
        *mtf = place;
        *mglobal = GlobalTransform::from(place);
    }
}

/// Registers the marker driver, after the name driver and the V-plate drive, and the placer.
pub(crate) struct RaidMarksPlugin;

impl Plugin for RaidMarksPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RaidMarks>()
            .add_systems(
                Update,
                drive_raid_marks
                    .after(crate::nameplates::drive_nameplates)
                    .after(crate::vplates::VPlateSet),
            )
            .add_systems(
                PostUpdate,
                place_raid_marks
                    .after(bevy::transform::TransformSystems::Propagate)
                    .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skull is Lua index 8, wire icon 7.
    #[test]
    fn mark_mesh_matches_the_lut_and_atlas_laws() {
        let mesh = mark_mesh(0);
        let pos = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .unwrap()
            .to_vec();
        assert_eq!(
            pos,
            vec![
                [-0.5, 1.0, 0.0],
                [0.5, 1.0, 0.0],
                [0.5, 0.0, 0.0],
                [-0.5, 0.0, 0.0]
            ],
            "the 0xce875c unit-quad LUT"
        );
        let cell = |icon: u32| {
            let mesh = mark_mesh(icon);
            let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() {
                bevy::mesh::VertexAttributeValues::Float32x2(v) => v.clone(),
                other => panic!("uv attribute shape: {other:?}"),
            };
            uvs[0] // the TL corner names the cell
        };
        assert_eq!(cell(0), [0.0, 0.0], "star: col 0, row 0");
        assert_eq!(cell(3), [0.75, 0.0], "triangle: col 3, row 0");
        assert_eq!(cell(4), [0.0, 0.25], "moon: col 0, row 1");
        assert_eq!(cell(7), [0.75, 0.25], "skull: col 3, row 1");
    }
}
