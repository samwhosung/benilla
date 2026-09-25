//! **A body's batches, merged by material** — one mesh entity per *material group* of a dressed
//! body instead of one per authored M2 batch (the crowd rig's first proposal).
//!
//! A geared level-60 body stands as 12–27 mesh parts binding 5–16 distinct materials (the dress
//! census's `parts=`/`mats=`, read off a 40-man raid at the Stormwind auction house): the skin
//! composite serves the torso, arms, legs, hands and feet as separate opaque batches, the hair
//! texture serves the hair and the facial geosets, and every one of those was its own entity —
//! its own row in every per-entity sweep Bevy and we run each frame (bounds, specialization,
//! visibility, extraction, the material walk), its own phase item, its own draw with its own
//! bind-group and vertex-buffer sets in wgpu's pass encoder. The raid's cost is that population,
//! not a hot function (1929), and per body about 40 % of it is batches that draw with a material
//! the batch beside it already binds.
//!
//! **What merges.** Two visible batches of one body join when they would be *indistinguishable*
//! to everything downstream of the spawn: the same six-handle material set (steady, the two
//! interior lanes, the fade twins, the depth-prime twin — every component derived from them is
//! then identical), the same blend/sidedness, both static or both skinned, and neither carrying
//! per-batch state the merge cannot represent: an animated alpha track, a billboard, a welded
//! billboard seam, a ground-decal quad. Transparent batches never meet — their material key
//! carries the authored batch order (`model_render::MatKey::batch_order`), so each has its own
//! handle by construction and painter's order is untouched. The key is *static*: which slot a
//! batch's texture comes from and its authored flags, never the texture itself — so a gear change
//! that swaps the composite re-points a group's material in place exactly as it re-pointed a
//! part's, and only a change to the *visible set* (a geoset the new gear hides or reveals)
//! rebuilds the groups it touches.
//!
//! **What a group is.** A synthetic [`EntityPart`] cloned from its first member, with the
//! geometry replaced by the members' concatenation and the render forms built from it the way
//! `model_forms` builds a batch's — the static form `RENDER_WORLD`-only, the skinned twin keeping
//! its main-world copy for the picker (0834's contract, unchanged). The concatenated
//! [`RenderSubmesh`] is also the group's [`PickMesh`], so the mouseover ray still tests the
//! resident geometry it always did. Merged forms are cached per (model geometry, member set):
//! forty humans in the same gear silhouette share them the way they shared the per-batch forms.
//!
//! Attach models (helm, shoulders, held items) are their own M2s with their own roots and are not
//! in scope here; a body's billboard cards keep their own spawner (`dress::spawn_billboard_part`).

use std::collections::HashMap;
use std::sync::Arc;

use benilla_formats::{ModelBlend, RenderSubmesh};
use bevy::asset::AssetId;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_formats::CharSkinSlot;

use super::super::EntityPart;
use super::char_skin::CharSkinMaterials;
use super::dress::part_materials;

/// The members of a merged group, on the group's entity beside its `DressedPart` (whose `index`
/// is the first member). Absent on a singleton.
#[derive(Component, Clone)]
pub(in crate::entities) struct DressedGroup(pub(in crate::entities) Arc<[u32]>);

/// What makes two of one body's batches interchangeable downstream of the spawn. Built from the
/// part's *slot* and flags, never from a resolved texture — see the module doc.
#[derive(Clone, PartialEq, Eq, Hash)]
struct MergeKey {
    /// The slot the material comes from: a character slot, or the batch's own built materials
    /// (identified by their handles — the same six for every batch sharing a texture).
    source: MaterialSource,
    blend: ModelBlend,
    additive: bool,
    two_sided: bool,
    skinned: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum MaterialSource {
    Char(CharSkinSlot),
    Own([Option<AssetId<WowModelMaterial>>; 6]),
}

/// The key a part merges under, or `None` for a part that stays a singleton.
fn merge_key(part: &EntityPart) -> Option<MergeKey> {
    if part.billboard.is_some()
        || part.welded_billboard
        || part.alpha_anim.is_some()
        || part.ground_quad.is_some()
    {
        return None;
    }
    let id = |h: &Option<Handle<WowModelMaterial>>| h.as_ref().map(Handle::id);
    let source = match part.char_slot {
        Some(slot) => MaterialSource::Char(slot),
        None => MaterialSource::Own([
            Some(part.material.id()),
            id(&part.material_interior),
            id(&part.material_interior_bake),
            id(&part.material_interior_bake_blend),
            id(&part.fade_blend),
            id(&part.zfill),
        ]),
    };
    Some(MergeKey {
        source,
        blend: part.blend,
        additive: part.additive,
        two_sided: part.two_sided,
        skinned: part.skinned_mesh.is_some(),
    })
}

/// One spawn unit: the member indices into the body's parts, first member lowest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::entities) struct BodyGroup {
    pub(in crate::entities) members: Vec<u32>,
}

impl BodyGroup {
    pub(in crate::entities) fn first(&self) -> u32 {
        self.members[0]
    }
}

/// Partition the shown parts of a body into spawn groups, in first-member order. `shows` is the
/// geoset predicate the dress and the redress both evaluate.
pub(in crate::entities) fn group_parts(
    parts: &[EntityPart],
    shows: impl Fn(&EntityPart) -> bool,
) -> Vec<BodyGroup> {
    let mut groups: Vec<BodyGroup> = Vec::new();
    let mut by_key: HashMap<MergeKey, usize> = HashMap::new();
    for (i, part) in parts.iter().enumerate() {
        if !shows(part) {
            continue;
        }
        let i = u32::try_from(i).expect("a model's batch count fits u32");
        match merge_key(part) {
            Some(key) => match by_key.get(&key) {
                Some(&g) => groups[g].members.push(i),
                None => {
                    by_key.insert(key, groups.len());
                    groups.push(BodyGroup { members: vec![i] });
                }
            },
            None => groups.push(BodyGroup { members: vec![i] }),
        }
    }
    groups.sort_by_key(BodyGroup::first);
    groups
}

/// The members' geometry as one submesh: attributes concatenated, indices re-based. Every member
/// shares the first's material facts by construction of the key, so the first's texture, flags
/// and slots are the group's.
fn concat_geometry(first: &RenderSubmesh, rest: &[&RenderSubmesh]) -> RenderSubmesh {
    let mut out = first.clone();
    // Per-vertex attributes that are either full-length or absent on every member: a mismatch
    // in presence would leave the group half-attributed, so the merge key keeps such batches
    // apart only implicitly (a model authors these per skin, not per batch); assert the
    // invariant rather than guess.
    for sub in rest {
        let base = u32::try_from(out.positions.len()).expect("merged vertex count fits u32");
        out.positions.extend_from_slice(&sub.positions);
        out.uvs.extend_from_slice(&sub.uvs);
        out.indices.extend(sub.indices.iter().map(|i| i + base));
        extend_matched(&mut out.normals, &sub.normals, out.positions.len());
        extend_matched(&mut out.joints, &sub.joints, out.positions.len());
        extend_matched(&mut out.weights, &sub.weights, out.positions.len());
        extend_matched(
            &mut out.vertex_colors,
            &sub.vertex_colors,
            out.positions.len(),
        );
    }
    out
}

/// Extend a per-vertex attribute so it stays either exactly full-length or empty: a member
/// without it drops the attribute for the whole group (the renderer recomputes normals; a
/// missing skin on any member means the group is not skinned, which the key already ensured).
fn extend_matched<T: Copy>(dst: &mut Vec<T>, src: &[T], full: usize) {
    if dst.is_empty() && src.is_empty() {
        return;
    }
    let before = full - src.len().min(full);
    if dst.len() == before && !src.is_empty() {
        dst.extend_from_slice(src);
    } else {
        dst.clear();
    }
}

/// A group's built render forms: the merged geometry (the group's `PickMesh`), its static form,
/// its skinned twin when every member skins, and the static form's build-time bound.
#[derive(Clone)]
pub(in crate::entities) struct MergedForms {
    pub(in crate::entities) geometry: Arc<RenderSubmesh>,
    pub(in crate::entities) static_mesh: Handle<Mesh>,
    pub(in crate::entities) skinned_mesh: Option<Handle<Mesh>>,
    pub(in crate::entities) aabb: Option<Aabb>,
}

/// The merged-form cache, keyed by (the first member's geometry, the member set) — the geometry
/// `Arc` is the model's own, shared by every unit of that display, so the key names one model's
/// one silhouette. Strong handles: a cached form outlives its units the way a model's per-batch
/// forms do, and the whole table drops past [`MERGED_FORMS_CAP`] entries rather than growing
/// with every silhouette a session ever sees.
#[derive(Resource, Default)]
pub(crate) struct MergedFormsCache(HashMap<(usize, Vec<u32>), MergedForms>);

const MERGED_FORMS_CAP: usize = 1024;

impl MergedFormsCache {
    /// The forms of `group` over `parts`, built on a miss. A singleton borrows the model's own
    /// forms and never enters the cache.
    pub(in crate::entities) fn forms(
        &mut self,
        parts: &[EntityPart],
        group: &BodyGroup,
        meshes: &mut Assets<Mesh>,
    ) -> MergedForms {
        let first = &parts[group.first() as usize];
        if group.members.len() == 1 {
            return MergedForms {
                geometry: first.geometry.clone(),
                static_mesh: first.mesh.clone(),
                skinned_mesh: first.skinned_mesh.clone(),
                aabb: first.aabb,
            };
        }
        let key = (Arc::as_ptr(&first.geometry) as usize, group.members.clone());
        if let Some(f) = self.0.get(&key) {
            return f.clone();
        }
        if self.0.len() >= MERGED_FORMS_CAP {
            self.0.clear();
        }
        let rest: Vec<&RenderSubmesh> = group.members[1..]
            .iter()
            .map(|&i| &*parts[i as usize].geometry)
            .collect();
        let merged = concat_geometry(&first.geometry, &rest);
        let stat = benilla_assets::submesh_to_static_mesh(&merged);
        let aabb = stat.compute_aabb();
        // The skinned twin exists only when every member skins AND the concatenation kept a
        // full joint set — a group drawn static on a rigged body would stand in bind pose.
        let skinned = (group
            .members
            .iter()
            .all(|&i| parts[i as usize].skinned_mesh.is_some())
            && merged.joints.len() == merged.positions.len())
        .then(|| meshes.add(benilla_assets::submesh_to_skinned_mesh(&merged)));
        let forms = MergedForms {
            geometry: Arc::new(merged),
            static_mesh: meshes.add(stat),
            skinned_mesh: skinned,
            aabb,
        };
        self.0.insert(key, forms.clone());
        forms
    }
}

/// The group as the part the spawner sees: the first member with the merged forms in place.
pub(in crate::entities) fn group_part(
    parts: &[EntityPart],
    group: &BodyGroup,
    forms: MergedForms,
) -> EntityPart {
    let mut part = parts[group.first() as usize].clone();
    part.geometry = forms.geometry;
    part.mesh = forms.static_mesh;
    part.skinned_mesh = forms.skinned_mesh;
    part.aabb = forms.aabb;
    part
}

/// Do two members resolve the same materials? The key promises it for the static half; this is
/// the check the character half makes at dress time (a slot resolving to `None` — no look, no
/// tables — falls back to the batch's own materials, which the key did not compare).
pub(in crate::entities) fn same_materials(
    parts: &[EntityPart],
    group: &BodyGroup,
    char_mats: &CharSkinMaterials,
) -> bool {
    let first = part_materials(&parts[group.first() as usize], char_mats);
    let ids = |m: &super::dress::PartMaterials<'_>| {
        [
            Some(m.steady.id()),
            m.interior.map(Handle::id),
            m.fade_blend.map(Handle::id),
            m.bake.map(Handle::id),
            m.bake_blend.map(Handle::id),
            m.zfill.map(Handle::id),
        ]
    };
    let want = ids(&first);
    group.members[1..]
        .iter()
        .all(|&i| ids(&part_materials(&parts[i as usize], char_mats)) == want)
}

/// Split every group whose members do not resolve the same materials into singletons — the
/// dress-time guard behind [`same_materials`]. Cheap: the resolve is a slot match.
pub(in crate::entities) fn guard_groups(
    groups: Vec<BodyGroup>,
    parts: &[EntityPart],
    char_mats: &CharSkinMaterials,
) -> Vec<BodyGroup> {
    let mut out = Vec::with_capacity(groups.len());
    for g in groups {
        if same_materials(parts, &g, char_mats) {
            out.push(g);
        } else {
            out.extend(g.members.iter().map(|&m| BodyGroup { members: vec![m] }));
        }
    }
    out.sort_by_key(BodyGroup::first);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(x: f32) -> RenderSubmesh {
        RenderSubmesh {
            positions: vec![[x, 0.0, 0.0], [x, 1.0, 0.0], [x, 0.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
            joints: vec![[1, 0, 0, 0]; 3],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
            ..Default::default()
        }
    }

    #[test]
    fn concatenation_rebases_indices_and_keeps_full_attributes() {
        let a = tri(0.0);
        let b = tri(5.0);
        let m = concat_geometry(&a, &[&b]);
        assert_eq!(m.positions.len(), 6);
        assert_eq!(m.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(m.normals.len(), 6);
        assert_eq!(m.joints.len(), 6);
        assert_eq!(m.weights.len(), 6);
        assert!(m.vertex_colors.is_empty(), "absent on both ⇒ absent");
    }

    fn part(material: u128, geoset: u16) -> EntityPart {
        EntityPart {
            mesh: Handle::default(),
            geometry: Arc::new(tri(0.0)),
            aabb: None,
            skinned_mesh: Some(Handle::default()),
            material: Handle::from(bevy::asset::uuid::Uuid::from_u128(material)),
            material_interior: None,
            material_interior_bake: None,
            material_interior_bake_blend: None,
            fade_blend: None,
            zfill: None,
            blend: ModelBlend::Opaque,
            additive: false,
            two_sided: false,
            geoset_id: geoset,
            char_slot: None,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            rgb_anim: None,
            rgb_seq: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            ground_quad: None,
        }
    }

    #[test]
    fn batches_group_by_material_and_state_stays_a_singleton() {
        let mut parts = vec![
            part(1, 0),    // skin
            part(2, 100),  // hair
            part(1, 400),  // gloves — the skin's material
            part(1, 500),  // boots — the skin's material, hidden below
            part(2, 200),  // facial — the hair's material, but animated: singleton
            part(3, 1500), // cloak: static form, its own material
        ];
        parts[4].welded_billboard = true;
        parts[5].skinned_mesh = None;
        let groups = group_parts(&parts, |p| p.geoset_id != 500);
        let members: Vec<Vec<u32>> = groups.iter().map(|g| g.members.clone()).collect();
        assert_eq!(members, vec![vec![0, 2], vec![1], vec![4], vec![5]]);
        assert_eq!(groups[0].first(), 0);
    }

    #[test]
    fn a_static_and_a_skinned_batch_never_share_a_group() {
        let mut parts = vec![part(1, 0), part(1, 1)];
        parts[1].skinned_mesh = None;
        let groups = group_parts(&parts, |_| true);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn a_member_without_an_attribute_drops_it_for_the_group() {
        let a = tri(0.0);
        let mut b = tri(5.0);
        b.normals.clear();
        let m = concat_geometry(&a, &[&b]);
        assert!(
            m.normals.is_empty(),
            "half-attributed is worse than recomputed"
        );
        assert_eq!(m.joints.len(), 6);
    }
}
