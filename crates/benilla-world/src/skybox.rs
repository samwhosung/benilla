//! The skybox: an authored sky M2 standing in for the `Light.dbc` gradient dome, drawn
//! camera-anchored at the far depth ([`crate::sky_order`]) through the ordinary M2 material lane
//! ([`M2BatchMaterials::skybox`]).
//!
//! Two slots feed it. A WMO root names a model in MOSB, and the reference draws it when any group
//! the portal flood reaches carries flag `0x40000` (`0x6b42e0` inside the flood `0x6b41c0`,
//! published to `[0xca8080]`, read at `0x681282`). The ghost sky (`LightSkybox.dbc`) fills the DBC
//! slot, whose weight 1.0 skips the WMO slot outright (`0x6d4ac1`–`0x6d4acc`), so it is taken
//! first.
//!
//! The WMO slot's weight is the camera-in-WMO interior crossfade `[0xce9bdc]`
//! (`0x6d4810(0, [0xca8080], [0xce9bdc])`), the number the MFOG fog lerp rides. Above
//! `[0x808aac]` = 0.99 a slot replaces the whole celestial pass (`0x6d4a3b`): stars, sun disc,
//! both moons, gradient band and cloud dome; below it they draw under the sky. The glare quads
//! draw on their own path (`0x483740` → `0x6d48c0` → `0x7e57e0`), and the fog, ambient and
//! diffuse are untouched.

use std::collections::HashSet;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;

use crate::model_render::M2BatchMaterials;
use crate::view::WorldCamera;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::WmoModel;
use benilla_assets::{LockRecover, WorldAssets};

/// The MOGP/MOGI group flag asking for the root's MOSB sky; the loader keeps the MOGP copy in
/// `WmoGroupNav::flags`.
const SHOW_SKYBOX: u32 = 0x40000;

/// The skybox model this frame asks for; `None` leaves the [`crate::sky`] gradient dome.
#[derive(Resource, Default, PartialEq, Eq)]
pub struct CameraSkybox(pub Option<String>);

/// The skybox slot's weight this frame: the interior crossfade [`crate::lighting::WmoCrossfade`]
/// (±0.25/s, `[0x8115b0]`) for a WMO sky, 1.0 for the ghost sky (`0x6d2260`), and 0 when none
/// resolves.
#[derive(Resource, Default, PartialEq)]
pub struct SkyboxWeight(pub f32);

impl SkyboxWeight {
    /// Whether the skybox replaces the whole celestial pass: weight above `[0x808aac]` = 0.99
    /// (`0x6d49e8`/`0x6d4a1f`); below it the sky blends over the six elements.
    pub fn replaces_celestial(&self) -> bool {
        self.0 > 0.99
    }
}

/// One batch of a built skybox model.
#[derive(Component)]
struct SkyboxPart {
    /// The skybox model path this batch belongs to.
    path: String,
    /// The batch's authored-blend material, drawn at weight 1.0.
    steady: Handle<WowModelMaterial>,
    /// The blend-promotion twin for `0 < weight < 1` (`0x811fe0`); `steady` itself when the batch
    /// already blends.
    fade_blend: Handle<WowModelMaterial>,
}

/// A batch wholly weighted to one parentless rotation-only bone, so it spins rigidly about the
/// bone's pivot ([`benilla_formats::BoneSpin`]) with no joint children, which would lag the
/// post-propagation anchor a frame. Only `CavernsOfTimeSky.m2`'s four asteroid belts spin.
#[derive(Component)]
struct SkyboxSpin {
    /// The bone's pivot, in the same (Bevy) space the batch's vertices were baked into.
    pivot: Vec3,
    spin: benilla_formats::BoneSpin,
}

/// Skybox paths built this session, failed loads included, so a failure is not retried per frame.
#[derive(Resource, Default)]
struct BuiltSkyboxes(HashSet<String>);

/// [`CameraSkybox`] is settled after this set; the dome's gate runs after it, so the two backdrops
/// agree within a frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SkyboxResolve;

/// Resolves the wanted skybox, builds it on first need, shows it and pins it to the camera.
pub(crate) struct SkyboxPlugin;

impl Plugin for SkyboxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraSkybox>()
            .init_resource::<SkyboxWeight>()
            .init_resource::<BuiltSkyboxes>()
            .add_systems(
                Update,
                (resolve_camera_skybox, build_skybox, apply_skybox_visibility)
                    .chain()
                    // After the PVS pass, whose flood this reads the same frame.
                    .after(crate::wmo_portal::WmoPvsSet)
                    // After the lighting resolve, so the weight is this frame's crossfade, as the
                    // fog's is.
                    .after(crate::lighting::LightingResolveSet)
                    .in_set(SkyboxResolve),
            )
            // Camera-anchored placement runs after propagation, off this frame's camera pose.
            .add_systems(
                PostUpdate,
                follow_camera.in_set(crate::billboard::BillboardPlace),
            );
    }
}

/// Resolve the wanted skybox and its weight. A WMO sky needs a group of the placement's flood PVS,
/// not the camera's own group, to carry [`SHOW_SKYBOX`] (`0x6b42e0`, `ebx` the group visited) and
/// its root to name a MOSB; the weight rides the down-ray claim's crossfade, a separate resolver.
fn resolve_camera_skybox(
    instances: Query<&crate::wmo_portal::WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    sampler: Option<Res<crate::lighting::LightSampler>>,
    viewer: Res<crate::view::Viewer>,
    current_map: Option<Res<crate::world_map::CurrentMap>>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    crossfade: Res<crate::lighting::WmoCrossfade>,
    mut want: ResMut<CameraSkybox>,
    mut weight: ResMut<SkyboxWeight>,
) {
    // The ghost sky first, as its slot skips the WMO one. Resolved from the atmosphere's map and
    // camera position, off the same ghost flag (`PLAYER_FLAGS` 0x10), so the two switch together.
    if viewer.ghost {
        let ghost_sky = sampler.as_ref().and_then(|s| {
            let pos = bevy_to_wow(cam.single().ok()?.translation());
            let map = current_map.as_ref().map_or(0, |m| m.0);
            s.0.ghost_skybox(map, pos)
        });
        if let Some(sky) = ghost_sky {
            if want.0.as_deref() != Some(sky) {
                want.0 = Some(sky.to_owned());
            }
            // The DBC slot's weight is 1.0 whenever filled (`0x6d26cb`/`0x6d26d0`): it pops in.
            weight.set_if_neq(SkyboxWeight(1.0));
            return;
        }
    }
    // `min()`, not the first match: query order is unstable across frames, and two overlapping
    // Caverns of Time shells both qualify.
    let resolved = instances
        .iter()
        .filter_map(|inst| {
            let model = wmos.get(&inst.handle)?;
            // The MOSB test first: 810 of the game's 815 WMO roots name no skybox.
            let sky = model.skybox.as_deref()?;
            model
                .group_nav
                .iter()
                .enumerate()
                .any(|(i, nav)| {
                    // Fail closed, unlike the cull: a lookup miss here would paint a sky over the
                    // whole world.
                    nav.flags & SHOW_SKYBOX != 0 && inst.visible.get(i).copied().unwrap_or(false)
                })
                .then(|| sky.to_owned())
        })
        .min();
    if want.0 != resolved {
        want.0 = resolved;
    }
    // The WMO slot's weight is the interior crossfade `[0xce9bdc]`; a name seen through a doorway
    // at weight 0 fills the slot, which `0x6d4afe` declines to draw.
    weight.set_if_neq(SkyboxWeight(match want.0 {
        Some(_) => crossfade.t(),
        None => 0.0,
    }));
}

/// Build the wanted skybox on first request; the models are small and few, so it stays built.
fn build_skybox(
    mut commands: Commands,
    want: Res<CameraSkybox>,
    mut built: ResMut<BuiltSkyboxes>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: M2BatchMaterials,
) {
    let Some(path) = want.0.as_deref() else {
        return;
    };
    if built.0.contains(path) {
        return;
    }
    // Before the shared light buffer exists: retry, without latching `built`.
    if !mats.ready() {
        return;
    }
    let Some(mut world_assets) = world_assets else {
        return; // assetless run: the gradient dome stays the backdrop
    };
    // The model's rigid spins by bone; none under a deterministic run, which keeps bind poses.
    let spins = if crate::dev_state::deterministic_run() {
        Default::default()
    } else {
        benilla_formats::load_m2_bone_spins(&mut world_assets.chain.lock_recover(), path)
            .unwrap_or_default()
    };
    let subs = benilla_formats::load_m2_mesh(&mut world_assets.chain.lock_recover(), path);
    let subs = match subs {
        Ok(subs) if !subs.is_empty() => subs,
        Ok(_) => {
            warn!("skybox '{path}' has no render batches — keeping the gradient dome");
            built.0.insert(path.to_string());
            return;
        }
        Err(e) => {
            warn!("skybox '{path}' failed to load, keeping the gradient dome: {e:#}");
            built.0.insert(path.to_string());
            return;
        }
    };
    for (i, sub) in subs.iter().enumerate() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        let positions: Vec<[f32; 3]> = sub
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        // Unread by the unlit sky, but the shared shader's vertex layout needs it.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            sub.normals
                .iter()
                .map(|n| wow_to_bevy(*n).to_array())
                .collect::<Vec<_>>(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
        mesh.insert_indices(Indices::U32(sub.indices.clone()));
        // The batch's authored address mode: Caverns of Time's belts wrap their UVs.
        let texture = sub
            .texture
            .as_deref()
            .and_then(|t| world_assets.texture(t, (sub.wrap_x, sub.wrap_y), &mut images));
        // `i + 1`, the authored batch order (0 is unordered): every batch shares one sort distance.
        let Some(pair) = mats.skybox(sub, texture, u16::try_from(i + 1).unwrap_or(0)) else {
            return; // light buffer vanished mid-build; `built` is unlatched, so we retry
        };
        let part = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(pair.steady.clone()),
                Transform::default(),
                Visibility::Hidden, // `apply_skybox_visibility` turns on exactly the wanted one
                // The crossfade's alpha (bits 0..=5), written only by `apply_skybox_visibility`.
                MeshTag(crate::mesh_tag::spawn_tag(0, 1.0)),
                SkyboxPart {
                    path: path.to_string(),
                    steady: pair.steady,
                    fade_blend: pair.fade_blend,
                },
            ))
            .id();
        if let Some(spin) = sole_bone(sub).and_then(|b| spins.get(&b)) {
            commands.entity(part).insert(SkyboxSpin {
                pivot: wow_to_bevy(spin.pivot),
                spin: spin.clone(),
            });
        }
    }
    built.0.insert(path.to_string());
}

/// The bone every vertex of this batch is wholly weighted to, if any: the batch's half of
/// [`benilla_formats::BoneSpin`]'s rigid condition.
fn sole_bone(sub: &benilla_formats::RenderSubmesh) -> Option<u16> {
    let bone = sub.joints.first()?[0];
    (sub.weights.len() == sub.joints.len()
        && sub
            .joints
            .iter()
            .zip(&sub.weights)
            // Not `== 1.0`: the weights are bytes normalised by their sum.
            .all(|(j, w)| j[0] == bone && w[0] > 0.999))
    .then_some(bone)
}

/// Show the wanted skybox at the slot weight and hide every other, the sole `Visibility`, material
/// and `MeshTag` writer for these entities. As the reference: hidden at weight 0 (`0x6d4afe`), the
/// weight in every batch's alpha (`0x710cb0` → `[CM2Model+0x180]`), and below 1 the batch
/// promoted to SRC_ALPHA blending whatever its mode (`0x811fe0`).
fn apply_skybox_visibility(
    want: Res<CameraSkybox>,
    weight: Res<SkyboxWeight>,
    mut parts: Query<(
        &SkyboxPart,
        &mut Visibility,
        &mut MeshMaterial3d<WowModelMaterial>,
        &mut MeshTag,
    )>,
) {
    for (part, mut vis, mut mat, mut tag) in &mut parts {
        let show = weight.0 > 0.0 && want.0.as_deref() == Some(part.path.as_str());
        let target = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if *vis != target {
            *vis = target;
        }
        if !show {
            continue;
        }
        let handle = if weight.0 < 1.0 {
            &part.fade_blend
        } else {
            &part.steady
        };
        if mat.0 != *handle {
            mat.0 = handle.clone();
        }
        let bits = crate::mesh_tag::with_alpha(tag.0, weight.0);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

/// Pin the box to the camera, world-aligned and at authored scale, the model's origin at the eye as
/// the reference places it (`0x707680` given a zeroed recentre vector, `0x6d4b3a`–`0x6d4b48`):
/// `StratholmeSkybox` is authored off-centre, so recentring it would be wrong. A spinning batch
/// turns about its bone's pivot inside the anchor, `T(eye) · T(pivot) · R(t) · T(−pivot)`, on the
/// scene clock; the reference's phase starts at load, which a 66.7 s ring does not show.
#[allow(clippy::type_complexity)]
fn follow_camera(
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    mut parts: Query<
        (&mut Transform, &mut GlobalTransform, Option<&SkyboxSpin>),
        (With<SkyboxPart>, Without<WorldCamera>),
    >,
) {
    let Some(cam_gt) = cam.iter().next() else {
        return;
    };
    let now = time.elapsed_secs();
    for (mut tf, mut gt, spin) in &mut parts {
        let (rot, pivot) = match spin {
            Some(s) => (
                benilla_assets::coords::wow_rotation_to_bevy(s.spin.sample(now)),
                s.pivot,
            ),
            None => (Quat::IDENTITY, Vec3::ZERO),
        };
        tf.translation = cam_gt.translation() + pivot - rot * pivot;
        tf.rotation = rot;
        tf.scale = Vec3::ONE;
        // Propagation already ran this frame: the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the real art: the seventeen other batches are wholly on bone 0, which has no track.
    #[test]
    fn only_the_belt_batches_of_the_caverns_sky_resolve_to_a_spinning_bone() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::Chain::open(&data).expect("open vanilla patch chain");
        const SKY: &str = "Environments\\Stars\\CavernsOfTimeSky.m2";
        let subs = benilla_formats::load_m2_mesh(&mut chain, SKY).expect("load the sky");
        let spins = benilla_formats::load_m2_bone_spins(&mut chain, SKY).expect("its spins");

        let bones: Vec<Option<u16>> = subs.iter().map(sole_bone).collect();
        assert_eq!(bones.len(), 21, "21 authored batches");
        // Batches 5..=8 are the belts (`benilla-extract m2batch`); every other rides bone 0.
        assert_eq!(
            &bones[5..=8],
            &[Some(1), Some(2), Some(3), Some(3)],
            "the belt batches and the bones they ride"
        );
        assert!(
            bones
                .iter()
                .enumerate()
                .filter(|(i, _)| !(5..=8).contains(i))
                .all(|(_, b)| *b == Some(0)),
            "every non-belt batch is wholly on bone 0: {bones:?}"
        );

        let spinning = bones
            .iter()
            .filter(|b| b.and_then(|b| spins.get(&b)).is_some())
            .count();
        assert_eq!(spinning, 4, "exactly the four belt batches turn");
    }
}
