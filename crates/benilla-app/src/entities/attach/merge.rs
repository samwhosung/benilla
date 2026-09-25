//! A body's batches merged by material: one mesh entity per group of batches that nothing
//! downstream of the spawn can tell apart (the same six-handle material set, blend, sidedness and
//! skinning, and no alpha track, billboard, welded seam or ground quad), not one per M2 batch.
//! A batch's own materials are keyed by its authored order (`MatKey`'s `batch_order`), so two
//! transparent batches never merge. The key names a texture slot, never a texture, so a gear
//! change re-points a group in place and only a change to the shown set rebuilds groups.

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

/// A merged group's member indices, beside its `DressedPart` (whose `index` is the first member);
/// absent on a singleton.
#[derive(Component, Clone)]
pub(in crate::entities) struct DressedGroup(pub(in crate::entities) Arc<[u32]>);

/// What makes two of a body's batches interchangeable: the material slot and flags, never a
/// resolved texture.
#[derive(Clone, PartialEq, Eq, Hash)]
struct MergeKey {
    /// A character slot, or the batch's own six built material handles.
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

/// Partition a body's parts that `shows` (the geoset predicate) into spawn groups, in
/// first-member order.
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

/// The members' geometry as one submesh, indices re-based; the key makes the first member's
/// material facts the group's.
fn concat_geometry(first: &RenderSubmesh, rest: &[&RenderSubmesh]) -> RenderSubmesh {
    let mut out = first.clone();
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

/// Extend a per-vertex attribute so it stays full-length or empty: one member without it drops it
/// for the whole group.
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

/// A group's render forms; `geometry` is also its `PickMesh`.
#[derive(Clone)]
pub(in crate::entities) struct MergedForms {
    pub(in crate::entities) geometry: Arc<RenderSubmesh>,
    pub(in crate::entities) static_mesh: Handle<Mesh>,
    pub(in crate::entities) skinned_mesh: Option<Handle<Mesh>>,
    pub(in crate::entities) aabb: Option<Aabb>,
}

/// Merged forms keyed by (the first member's geometry `Arc`, the member set): one model's one
/// silhouette, shared by every unit of that display. Cleared whole past [`MERGED_FORMS_CAP`].
#[derive(Resource, Default)]
pub(crate) struct MergedFormsCache(HashMap<(usize, Vec<u32>), MergedForms>);

const MERGED_FORMS_CAP: usize = 1024;

impl MergedFormsCache {
    /// The forms of `group`, built on a miss; a singleton borrows the model's own, uncached.
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
        // Skinned only when every member skins and the joints survived the concatenation.
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

/// Whether every member resolves the same materials at dress time: a character slot with no
/// look falls back to each batch's own materials, which the key does not compare.
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

/// Split into singletons every group that fails [`same_materials`].
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
            part(1, 400),  // gloves: the skin's material
            part(1, 500),  // boots: the skin's material, hidden below
            part(2, 200),  // facial: the hair's material, but a welded billboard
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
