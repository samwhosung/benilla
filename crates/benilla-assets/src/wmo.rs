//! WMO asset loader: a root file plus its group files (`<stem>_NNN.wmo`), one [`ModelSubmesh`] per
//! group render batch. Groups are read with [`LoadContext::read_asset_bytes`], not `load()`, which
//! would recurse into this loader. The app builds the meshes, paced, as a city's thousands of
//! batches would land in one frame.

use crate::column_grid::ColumnGrid;
use std::sync::Arc;

use benilla_formats::{
    accumulate_wmo_group_camera_collision, accumulate_wmo_group_camera_only_collision,
    accumulate_wmo_group_collision, parse_wmo_lights, parse_wmo_root, wmo_group_doodad_refs,
    wmo_group_footprint_tris, wmo_group_header, wmo_group_light_refs, wmo_group_liquid_mesh,
    wmo_group_submeshes, CollisionMesh, FootprintTris, LiquidMesh, WmoDoodad, WmoDoodadSet, WmoFog,
    WmoGroupInfo, WmoLight, WmoPortalInfo, WmoPortalRef,
};
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::reflect::TypePath;

use crate::model::ModelSubmesh;

/// One WMO group's portal-cull data: MOGP flags (the flood defers on EXTERIOR `0x8`), the MOGI box
/// and its slice of [`WmoModel::portal_refs`]; an unreadable group never floods.
#[derive(Clone, Copy)]
pub struct WmoGroupNav {
    pub flags: u32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub ref_start: u16,
    pub ref_count: u16,
    /// The `WMOAreaTable.WMOGroupID` key, MOGP `uniqueID`; 0 when none.
    pub area_table_id: u32,
    /// MOGP fog indices (disk `+0x30`) into [`WmoModel::fogs`], for the fog selector (`0x69de20`).
    pub fog_indices: [u8; 4],
    /// MOGP `groupLiquid` (`0xf` = none): when set, the whole group reads submerged at every Z.
    pub group_liquid: u32,
}

/// A loaded WMO building; `Default` is an empty test scaffold, never a loader product.
#[derive(Asset, TypePath, Clone, Default)]
pub struct WmoModel {
    /// The `WMOAreaTable.WMOID` key (root `MOHD.wmoID`).
    pub wmo_id: u32,
    pub submeshes: Vec<ModelSubmesh>,
    /// The group of each entry in [`Self::submeshes`], so the portal cull can hide a whole group.
    pub submesh_group: Vec<u16>,
    /// The portal graph (MOPV, MOPT, MOPR) in model space; with none, every group stays visible.
    pub portal_vertices: Vec<[f32; 3]>,
    pub portal_infos: Vec<WmoPortalInfo>,
    pub portal_refs: Vec<WmoPortalRef>,
    /// Per-group cull data, indexed by absolute group index.
    pub group_nav: Vec<WmoGroupNav>,
    /// The root's MFOG records; record 0 is the building default.
    pub fogs: Vec<WmoFog>,
    /// The root's MOSB skybox model, drawn while the camera is in a group flagged `0x40000`.
    pub skybox: Option<String>,
    /// Per-group walking-collision faces (non-DETAIL, no orientation filter): the reference's
    /// walking BSP set (`0x6be250`, mask `0x84`) that the current-group down-ray casts on.
    pub group_collision_tris: Vec<Vec<[[f32; 3]; 3]>>,
    /// Per-group camera-only faces (DETAIL `0x04` set, NOCAMCOLLIDE `0x02` clear). Deviation: the
    /// down-ray tries them when the walking and portal legs miss and no terrain lies below the eye,
    /// because the reference reads outside there and blanks the building around a camera sealed in
    /// an all-DETAIL pocket.
    pub group_camera_only_tris: Vec<Vec<[[f32; 3]; 3]>>,
    /// Per-group AABB of [`Self::group_collision_tris`], from the faces rather than MOGI: an
    /// authored MOGI box can sit above its own floor (Northshire Abbey group 3, by 1.5 yd).
    pub group_collision_bounds: Vec<Option<([f32; 3], [f32; 3])>>,
    /// Per-group [`ColumnGrid`] over [`Self::group_collision_tris`]; `None` when too few to index.
    pub group_collision_grids: Vec<Option<ColumnGrid>>,
    /// The union of [`Self::group_collision_bounds`]: the whole-building broad phase.
    pub collision_bounds: Option<([f32; 3], [f32; 3])>,
    /// Walking collision across all groups in WMO-local coords, DETAIL faces excluded.
    pub collision: Option<CollisionMesh>,
    /// Camera and line-of-sight collision: DETAIL kept, NOCAMCOLLIDE dropped.
    pub collision_camera: Option<CollisionMesh>,
    /// The root's MODD doodads in WMO model space, spawned by doodad set.
    pub doodads: Vec<WmoDoodad>,
    /// The MODS ranges into [`Self::doodads`]: set 0 always, plus the one a placement picks.
    pub doodad_sets: Vec<WmoDoodadSet>,
    /// The root's MOLT fixture lights in WMO model space, spawned as point lights.
    pub lights: Vec<WmoLight>,
    /// Per-group MOGI interior flag and box, in WMO model space.
    pub group_bounds: Vec<WmoGroupInfo>,
    /// Per-MODD lighting base, parallel to [`Self::doodads`]. A MODD doodad never takes the
    /// footprint down-ray: its create (`0x694e90`) never calls `SetMatrix` (`0x698d20`).
    pub doodad_base: Vec<DoodadBase>,
    /// Per-MODD first referencing group, whose MOLR set the interior lane folds; the
    /// interior/exterior class reads [`Self::doodad_groups`].
    pub doodad_owner: Vec<Option<u16>>,
    /// Per-MODD every group whose MODR names it: the portal-cull key, as the reference draws
    /// doodads per visible group (`0x695aa0`, MODR at `+0xe8`, count `+0x144`, walk `0x698720`).
    pub doodad_groups: Vec<Arc<[u16]>>,
    /// Per-group footprint faces (render faces, MOCV, MOPY flags); `None` for exterior groups and
    /// groups without MOCV. A GameObject M2 down-rays them for its MOCV lighting (`0x69e4c0`).
    pub group_footprints: Vec<Option<FootprintTris>>,
    /// Root MOMT `ground_type` per material: the `TerrainType.dbc` id of a face's MOPY material.
    pub material_ground_type: Vec<u32>,
    /// Root MOMT `diffColor` per material, RGB 0..1: an interior MLIQ pool's colour, by material.
    pub material_diff_color: Vec<[f32; 3]>,
    /// Per-group AABB of the faces [`Self::group_footprints`] uses: the footprint broad phase.
    pub group_footprint_bounds: Vec<Option<([f32; 3], [f32; 3])>>,
    /// Per-group [`ColumnGrid`] over [`Self::group_footprints`]; `None` when not indexed.
    pub group_footprint_grids: Vec<Option<ColumnGrid>>,
    /// Per-group MOLR refs into [`Self::lights`]: a MODD prop folds its owning group's list, a
    /// GameObject its footprint-hit group's.
    pub group_light_refs: Vec<Vec<u16>>,
    /// Per-group MLIQ liquid surface in WMO model space; `None` for a group without one.
    pub group_liquids: Vec<Option<LiquidMesh>>,
}

/// One MODD doodad's lighting base. The reference fills an interior doodad's words at create from
/// the MODD colour: `0x694e90` calls `0x6a77e0`, diffuse floor `0x70`, ambient cap `0x60`.
#[derive(Clone, PartialEq, Debug)]
pub enum DoodadBase {
    /// The sky-lit lane of an exterior-group doodad.
    Exterior,
    /// An interior-group doodad: the MODD-colour base and the owning group's light refs.
    Interior(InteriorPropBase),
}

/// An interior MODD doodad's base light, independent of the time of day.
#[derive(Clone, PartialEq, Debug)]
pub struct InteriorPropBase {
    /// The ambient word, [`cap96`] of the MODD colour, RGB 0..1.
    pub ambient: [f32; 3],
    /// The diffuse word, [`floor112`] of the MODD colour, lit along the fixed axis
    /// (−0.30822, −0.30822, −0.9), never the sun.
    pub diffuse: [f32; 3],
    /// The owning group's MOLR refs into [`WmoModel::lights`], the only point lights it folds
    /// (range-gated by attenStart and attenEnd); empty means none, its own flame included.
    pub light_refs: Vec<u16>,
}

/// Per-group AABBs of the vertices the footprint faces index, not of every position; shared with
/// the test fixtures.
pub fn footprint_tri_bounds(
    footprints: &[Option<FootprintTris>],
) -> Vec<Option<([f32; 3], [f32; 3])>> {
    footprints
        .iter()
        .map(|fp| {
            let fp = fp.as_ref()?;
            let mut bounds: Option<([f32; 3], [f32; 3])> = None;
            for &i in &fp.indices {
                let Some(v) = fp.positions.get(i as usize) else {
                    continue;
                };
                let (min, max) = bounds.get_or_insert((*v, *v));
                for a in 0..3 {
                    min[a] = min[a].min(v[a]);
                    max[a] = max[a].max(v[a]);
                }
            }
            bounds
        })
        .collect()
}

/// Per-group column indexes over the footprint faces, shared with the test fixtures.
pub fn footprint_tri_grids(footprints: &[Option<FootprintTris>]) -> Vec<Option<ColumnGrid>> {
    footprints
        .iter()
        .map(|fp| {
            let fp = fp.as_ref()?;
            let tris: Vec<&[u16; 3]> = fp.indices.as_chunks::<3>().0.iter().collect();
            ColumnGrid::build(tris.len(), |i| {
                let mut lo = [f32::MAX; 2];
                let mut hi = [f32::MIN; 2];
                for &vi in tris[i] {
                    if let Some(v) = fp.positions.get(vi as usize) {
                        for a in 0..2 {
                            lo[a] = lo[a].min(v[a]);
                            hi[a] = hi[a].max(v[a]);
                        }
                    }
                }
                (lo, hi)
            })
        })
        .collect()
}

/// Per-group column indexes over a per-group triangle set.
pub fn collision_tri_grids(tris: &[Vec<[[f32; 3]; 3]>]) -> Vec<Option<ColumnGrid>> {
    tris.iter()
        .map(|group| {
            ColumnGrid::build(group.len(), |i| {
                let t = &group[i];
                (
                    [
                        t[0][0].min(t[1][0]).min(t[2][0]),
                        t[0][1].min(t[1][1]).min(t[2][1]),
                    ],
                    [
                        t[0][0].max(t[1][0]).max(t[2][0]),
                        t[0][1].max(t[1][1]).max(t[2][1]),
                    ],
                )
            })
        })
        .collect()
}

/// Per-group AABBs of a per-group triangle set, and their union; shared with the down-ray tests.
#[allow(clippy::type_complexity)]
pub fn collision_tri_bounds(
    tris: &[Vec<[[f32; 3]; 3]>],
) -> (
    Vec<Option<([f32; 3], [f32; 3])>>,
    Option<([f32; 3], [f32; 3])>,
) {
    let per_group: Vec<Option<([f32; 3], [f32; 3])>> = tris
        .iter()
        .map(|group| {
            let mut bounds: Option<([f32; 3], [f32; 3])> = None;
            for tri in group {
                for v in tri {
                    let (min, max) = bounds.get_or_insert((*v, *v));
                    for a in 0..3 {
                        min[a] = min[a].min(v[a]);
                        max[a] = max[a].max(v[a]);
                    }
                }
            }
            bounds
        })
        .collect();
    let union = per_group.iter().flatten().fold(
        None::<([f32; 3], [f32; 3])>,
        |acc, (gmin, gmax)| match acc {
            None => Some((*gmin, *gmax)),
            Some((mut min, mut max)) => {
                for a in 0..3 {
                    min[a] = min[a].min(gmin[a]);
                    max[a] = max[a].max(gmax[a]);
                }
                Some((min, max))
            }
        },
    );
    (per_group, union)
}

/// The ambient cap of `0x6a77e0`, in its fixed point: a colour whose max channel exceeds 96 scales
/// per channel to `(c·scale + 255) >> 8`, `scale = round(96·255/max − 0.5)`; otherwise it is raw.
pub fn cap96(c: [u8; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max <= 96 {
        return c.map(|v| f32::from(v) / 255.0);
    }
    let scale = ((96.0 * 255.0 / f32::from(max)) - 0.5).round_ties_even() as u32;
    c.map(|v| ((u32::from(v) * scale + 255) >> 8) as f32 / 255.0)
}

/// The diffuse floor of `0x6a77e0`: a colour whose max channel is below `thresh` is raised by the
/// reference's HSV round trip at value `thresh`, a scale by `thresh/max` truncated per channel.
fn floor_raise(c: [u8; 3], thresh: u8) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max >= thresh || max == 0 {
        return c.map(|v| f32::from(v) / 255.0);
    }
    // Integer math: a float `v · (thresh/max)` can land under `thresh`, truncating to `thresh − 1`.
    c.map(|v| ((u32::from(v) * u32::from(thresh)) / u32::from(max)) as f32 / 255.0)
}

/// [`floor_raise`] at the MODD create site's diffuse threshold `0x70` (112).
pub fn floor112(c: [u8; 3]) -> [f32; 3] {
    floor_raise(c, 112)
}

/// [`floor_raise`] at the GameObject footprint attach site's diffuse threshold `0xA8` (168),
/// `0x69e4c0`, the entity twin of the ADT-MDDF attach at `0x6a8410`.
pub fn floor168(c: [u8; 3]) -> [f32; 3] {
    floor_raise(c, 168)
}

/// MODD index to its first referencing group, whose MOLR set the interior lane uses. The reference
/// creates a doodad on the first visible-group walk that names it, and never one no group names.
fn modr_owners(doodad_count: usize, group_doodad_refs: &[Vec<u16>]) -> Vec<Option<u16>> {
    let mut owner: Vec<Option<u16>> = vec![None; doodad_count];
    for (gi, refs) in group_doodad_refs.iter().enumerate() {
        for &di in refs {
            if let Some(slot) = owner.get_mut(di as usize) {
                slot.get_or_insert(gi as u16);
            }
        }
    }
    owner
}

/// MODD index to every group whose MODR names it: the reference draws per visible group
/// (`0x695aa0` from the walk at `0x698720`), so a prop shows while any of them is visible.
fn modr_refs(doodad_count: usize, group_doodad_refs: &[Vec<u16>]) -> Vec<Arc<[u16]>> {
    let mut refs: Vec<Vec<u16>> = vec![Vec::new(); doodad_count];
    for (gi, group) in group_doodad_refs.iter().enumerate() {
        for &di in group {
            if let Some(slot) = refs.get_mut(di as usize) {
                // MODR may name the same doodad twice in one group; the cull only needs the set.
                if slot.last() != Some(&(gi as u16)) {
                    slot.push(gi as u16);
                }
            }
        }
    }
    refs.into_iter().map(Arc::from).collect()
}

/// Every MODD doodad's lighting base, from MODR ownership, never a spatial test; an unnamed MODD
/// is Exterior. Exterior wins: the reference keeps one def per (MODD, placement) (`0x694e90`), and
/// its classify (`0x695aa0`) never re-marks an exterior def interior while an exterior group
/// clears the interior bit, so any exterior referrer makes the prop sky-lit.
fn resolve_doodad_bases(
    doodads: &[WmoDoodad],
    groups: &[WmoGroupInfo],
    owner: &[Option<u16>],
    refs: &[Arc<[u16]>],
    group_light_refs: &[Vec<u16>],
) -> Vec<DoodadBase> {
    let interior_group = |gi: &u16| -> bool {
        groups
            .get(*gi as usize)
            .is_some_and(|g: &WmoGroupInfo| g.interior)
    };
    doodads
        .iter()
        .enumerate()
        .map(|(di, d)| {
            let Some(gi) = owner.get(di).copied().flatten() else {
                return DoodadBase::Exterior;
            };
            // Exterior is absorbing across the whole referrer set.
            let all_interior = refs
                .get(di)
                .is_some_and(|gs| !gs.is_empty() && gs.iter().all(interior_group));
            if !all_interior {
                return DoodadBase::Exterior;
            }
            let rgb = [d.color[0], d.color[1], d.color[2]];
            DoodadBase::Interior(InteriorPropBase {
                ambient: cap96(rgb),
                diffuse: floor112(rgb),
                light_refs: group_light_refs
                    .get(gi as usize)
                    .cloned()
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Loads a WMO root and its group files into a [`WmoModel`].
#[derive(Default, TypePath)]
pub struct WmoModelLoader;

impl AssetLoader for WmoModelLoader {
    type Asset = WmoModel;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<WmoModel, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let to_io = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));
        let root = parse_wmo_root(&bytes).map_err(to_io)?;

        // Lowercased so the `.wmo` strip matches; MPQ lookup is case-insensitive anyway.
        let root_path = ctx.path().path().to_string_lossy().to_ascii_lowercase();
        let stem = root_path
            .strip_suffix(".wmo")
            .unwrap_or(&root_path)
            .to_string();

        let mut submeshes = Vec::new();
        let mut submesh_group: Vec<u16> = Vec::new();
        // Pre-sized to the group count so MOPR group indices line up when a group file is skipped;
        // boxes from the root MOGI, the rest from each MOGP header.
        let mut group_nav: Vec<WmoGroupNav> = (0..root.group_count() as usize)
            .map(|gi| {
                let (bbox_min, bbox_max) = root
                    .group_infos()
                    .get(gi)
                    .map_or(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |g| {
                        (g.bbox_min, g.bbox_max)
                    });
                WmoGroupNav {
                    flags: 0,
                    bbox_min,
                    bbox_max,
                    ref_start: 0,
                    ref_count: 0,
                    area_table_id: 0,
                    fog_indices: [0; 4],
                    group_liquid: benilla_formats::NO_GROUP_LIQUID,
                }
            })
            .collect();
        let mut group_collision_tris: Vec<Vec<[[f32; 3]; 3]>> =
            vec![Vec::new(); root.group_count() as usize];
        let mut group_camera_only_tris: Vec<Vec<[[f32; 3]; 3]>> =
            vec![Vec::new(); root.group_count() as usize];
        let mut col_pos: Vec<[f32; 3]> = Vec::new();
        let mut col_idx: Vec<u32> = Vec::new();
        let mut cam_pos: Vec<[f32; 3]> = Vec::new();
        let mut cam_idx: Vec<u32> = Vec::new();
        let mut group_doodad_refs: Vec<Vec<u16>> = vec![Vec::new(); root.group_count() as usize];
        let mut group_light_refs: Vec<Vec<u16>> = vec![Vec::new(); root.group_count() as usize];
        let mut group_liquids: Vec<Option<LiquidMesh>> =
            (0..root.group_count()).map(|_| None).collect();
        let mut group_footprints: Vec<Option<FootprintTris>> =
            (0..root.group_count()).map(|_| None).collect();
        for gi in 0..root.group_count() {
            let group_url = format!("mpq://{stem}_{gi:03}.wmo");
            let Ok(gbytes) = ctx.read_asset_bytes(group_url).await else {
                continue; // a missing or unreadable group is skipped
            };
            if let (Some(h), Some(nav)) =
                (wmo_group_header(&gbytes), group_nav.get_mut(gi as usize))
            {
                nav.flags = h.flags;
                nav.ref_start = h.portal_ref_start;
                nav.ref_count = h.portal_ref_count;
                nav.area_table_id = h.area_table_id;
                nav.fog_indices = h.fog_indices;
                nav.group_liquid = h.group_liquid;
            }
            // One walking gather feeds both the per-group faces and the flat collider.
            let mut gpos: Vec<[f32; 3]> = Vec::new();
            let mut gidx: Vec<u32> = Vec::new();
            accumulate_wmo_group_collision(&gbytes, &mut gpos, &mut gidx);
            if let Some(tris) = group_collision_tris.get_mut(gi as usize) {
                for t in gidx.as_chunks::<3>().0 {
                    if let (Some(&a), Some(&b), Some(&c)) = (
                        gpos.get(t[0] as usize),
                        gpos.get(t[1] as usize),
                        gpos.get(t[2] as usize),
                    ) {
                        tris.push([a, b, c]);
                    }
                }
            }
            let base = col_pos.len() as u32;
            col_pos.extend_from_slice(&gpos);
            col_idx.extend(gidx.iter().map(|i| i + base));
            accumulate_wmo_group_camera_collision(&gbytes, &mut cam_pos, &mut cam_idx);
            let (mut dpos, mut didx): (Vec<[f32; 3]>, Vec<u32>) = (Vec::new(), Vec::new());
            accumulate_wmo_group_camera_only_collision(&gbytes, &mut dpos, &mut didx);
            if let Some(tris) = group_camera_only_tris.get_mut(gi as usize) {
                for t in didx.as_chunks::<3>().0 {
                    if let (Some(&a), Some(&b), Some(&c)) = (
                        dpos.get(t[0] as usize),
                        dpos.get(t[1] as usize),
                        dpos.get(t[2] as usize),
                    ) {
                        tris.push([a, b, c]);
                    }
                }
            }
            let subs = wmo_group_submeshes(&gbytes, &root).map_err(to_io)?;
            // The reference walks MODR at `0x695aa0` and MOLR at `0x695c00`.
            if let Some(slot) = group_doodad_refs.get_mut(gi as usize) {
                *slot = wmo_group_doodad_refs(&gbytes);
            }
            if let Some(slot) = group_light_refs.get_mut(gi as usize) {
                *slot = wmo_group_light_refs(&gbytes);
            }
            if let Some(slot) = group_footprints.get_mut(gi as usize) {
                *slot = wmo_group_footprint_tris(&gbytes);
            }
            if let Some(slot) = group_liquids.get_mut(gi as usize) {
                *slot = wmo_group_liquid_mesh(&gbytes);
            }
            for sub in subs {
                // Lowercased: Bevy's loader lookup is case-sensitive, and an uppercase `.BLP` falls
                // to type-based resolution, ambiguous with Bevy's own image loader.
                let texture = sub.texture.as_deref().map(|t| {
                    ctx.load::<Image>(format!(
                        "mpq://{}",
                        t.replace('\\', "/").to_ascii_lowercase()
                    ))
                });
                submeshes.push(ModelSubmesh {
                    texture,
                    skin_slot: sub.skin_slot,
                    geoset_id: 0,
                    char_slot: None,
                    icon_slot: false,
                    blend: sub.blend,
                    two_sided: sub.two_sided,
                    interior: sub.interior,
                    emissive: sub.emissive,
                    sidn: sub.sidn, // MOMT SIDN (0x10): the authored night-glow colour
                    window: sub.window, // MOMT WINDOW (0x20): the interior midpoint light
                    additive: sub.additive,
                    env_map: false,
                    no_depth_write: false,
                    no_depth_test: false,
                    fog_policy: sub.fog_policy,
                    billboard: None,
                    alpha_anim: None,
                    uv_anim: None,
                    uv_seq: None,
                    uv_rot_seq: None,
                    uv_scale_seq: None,
                    rgb_anim: None,
                    rgb_seq: None,
                    wmo_batch: sub.wmo_batch, // the MOBA section: an interior group's lighting
                    ground_quad: None,
                    geometry: std::sync::Arc::new(sub),
                });
                submesh_group.push(gi as u16);
            }
        }
        let doodad_owner = modr_owners(root.doodads().len(), &group_doodad_refs);
        let doodad_groups = modr_refs(root.doodads().len(), &group_doodad_refs);
        let doodad_base = resolve_doodad_bases(
            root.doodads(),
            root.group_infos(),
            &doodad_owner,
            &doodad_groups,
            &group_light_refs,
        );

        let collision = (!col_idx.is_empty()).then_some(CollisionMesh {
            positions: col_pos,
            indices: col_idx,
        });
        let collision_camera = (!cam_idx.is_empty()).then_some(CollisionMesh {
            positions: cam_pos,
            indices: cam_idx,
        });
        resolve_shared_liquid_cells(&mut group_liquids);
        let portals = root.portals();
        let (group_collision_bounds, collision_bounds) =
            collision_tri_bounds(&group_collision_tris);
        let group_footprint_bounds = footprint_tri_bounds(&group_footprints);
        let group_collision_grids = collision_tri_grids(&group_collision_tris);
        let group_footprint_grids = footprint_tri_grids(&group_footprints);
        Ok(WmoModel {
            wmo_id: benilla_formats::wmo_root_id(&bytes),
            submeshes,
            submesh_group,
            portal_vertices: portals.vertices.clone(),
            portal_infos: portals.infos.clone(),
            portal_refs: portals.refs.clone(),
            group_nav,
            fogs: root.fogs().to_vec(),
            skybox: root.skybox().map(str::to_owned),
            group_collision_tris,
            group_camera_only_tris,
            group_collision_bounds,
            group_collision_grids,
            collision_bounds,
            collision,
            collision_camera,
            doodads: root.doodads().to_vec(),
            doodad_sets: root.doodad_sets().to_vec(),
            lights: parse_wmo_lights(&bytes),
            group_bounds: root.group_infos().to_vec(),
            doodad_base,
            doodad_owner,
            doodad_groups,
            group_footprints,
            material_ground_type: root.material_ground_types(),
            material_diff_color: root.material_diff_colors(),
            group_footprint_bounds,
            group_footprint_grids,
            group_light_refs,
            group_liquids,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["wmo"]
    }
}

/// The MLIQ shared-cell gate: a cell flagged `0x80` ([`LiquidMesh::shared`]) is authored in two
/// groups and only one may draw it, or the sheet composites twice. The reference picks the drawer
/// by the portal flood's depth parity each frame (`0x6b41c0`, `0x6b4074`, `0x6b61a0`); the lowest
/// group index draws the same pixels, as both claimants carry the same alpha and heights. Cells
/// match on their XY centre, quantised to 0.01 yd only to hash.
fn resolve_shared_liquid_cells(group_liquids: &mut [Option<LiquidMesh>]) {
    let key = |m: &LiquidMesh, c: usize| -> Option<(i32, i32)> {
        let (cols, rows) = (m.grid[0] as usize, m.grid[1] as usize);
        let (xt, yt) = (cols.checked_sub(1)?, rows.checked_sub(1)?);
        let (tx, ty) = (c % xt, c / xt);
        if ty >= yt {
            return None;
        }
        let a = m.positions.get(ty * cols + tx)?;
        let b = m.positions.get((ty + 1) * cols + tx + 1)?;
        #[allow(clippy::cast_possible_truncation)]
        Some((
            (0.5 * (a[0] + b[0]) * 100.0).round() as i32,
            (0.5 * (a[1] + b[1]) * 100.0).round() as i32,
        ))
    };
    // First claimant wins, walking groups in index order.
    let mut owner: bevy::platform::collections::HashMap<(i32, i32), usize> =
        bevy::platform::collections::HashMap::default();
    for (gi, slot) in group_liquids.iter().enumerate() {
        let Some(m) = slot else { continue };
        for c in 0..m.wet.len() {
            if !m.wet[c] || !m.shared[c] {
                continue;
            }
            if let Some(k) = key(m, c) {
                owner.entry(k).or_insert(gi);
            }
        }
    }
    if owner.is_empty() {
        return;
    }
    for (gi, slot) in group_liquids.iter_mut().enumerate() {
        let Some(m) = slot else { continue };
        let keys: Vec<Option<(i32, i32)>> = (0..m.wet.len()).map(|c| key(m, c)).collect();
        let shared = m.shared.clone();
        m.retain_cells(|c| !shared[c] || keys[c].is_none_or(|k| owner.get(&k) == Some(&gi)));
        if m.indices.is_empty() {
            *slot = None; // every cell this group held belonged to a neighbour
        }
    }
}

#[cfg(test)]
mod doodad_base_tests {
    use super::*;

    fn grp(interior: bool) -> WmoGroupInfo {
        WmoGroupInfo {
            interior,
            show_skybox: false,
            bbox_min: [-1.0; 3],
            bbox_max: [1.0; 3],
        }
    }

    fn prop(color: [u8; 4]) -> WmoDoodad {
        WmoDoodad {
            model: "candle.m2".into(),
            position: [0.0; 3],
            orientation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
            color,
        }
    }

    /// Northshire Abbey's candle stands MODD[18] and MODD[24], as the reference commits them
    /// through `0x6a77e0` (cap scales 182 and 173).
    #[test]
    fn cap96_matches_the_decoded_abbey_stands_bit_exact() {
        let as_bytes = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as u8);
        assert_eq!(as_bytes(cap96([78, 76, 134])), [56, 55, 96]);
        assert_eq!(as_bytes(cap96([90, 86, 141])), [61, 59, 96]);
        assert_eq!(as_bytes(cap96([96, 40, 20])), [96, 40, 20]);
        assert_eq!(as_bytes(floor112([78, 76, 134])), [78, 76, 134]);
        assert_eq!(as_bytes(floor112([90, 86, 141])), [90, 86, 141]);
        let raised = as_bytes(floor112([56, 28, 14]));
        assert_eq!(raised[0], 112);
        assert_eq!(raised, [112, 56, 28]);
        // Black stays black in the reference too: it scales the HSV value by `thresh/max`
        // (`0x6a78a5`) and forces max to 1 only to keep that divide finite (`0x6a780e`).
        assert_eq!(as_bytes(floor112([0, 0, 0])), [0, 0, 0]);
    }

    #[test]
    fn floor168_matches_the_decoded_abbey_benches_bit_exact() {
        // The abbey inn benches as the reference commits them; 63·168/83 = 127.52 truncates to 127.
        let as_bytes = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as u8);
        assert_eq!(as_bytes(cap96([59, 65, 92])), [59, 65, 92]);
        assert_eq!(as_bytes(cap96([69, 63, 83])), [69, 63, 83]);
        assert_eq!(as_bytes(floor168([59, 65, 92])), [107, 118, 168]);
        assert_eq!(as_bytes(floor168([69, 63, 83])), [139, 127, 168]);
        assert_eq!(as_bytes(floor168([200, 30, 10])), [200, 30, 10]);
    }

    #[test]
    fn modr_ownership_picks_the_lane_and_the_light_refs() {
        let doodads = vec![
            prop([90, 86, 141, 255]), // owned by interior group 0
            prop([90, 86, 141, 255]), // owned by exterior group 1
            prop([90, 86, 141, 255]), // referenced by no group
        ];
        let groups = [grp(true), grp(false)];
        let modr = vec![vec![0u16], vec![1u16]];
        let molr = vec![vec![7u16, 9u16], vec![3u16]];
        let owner = modr_owners(doodads.len(), &modr);
        let refs = modr_refs(doodads.len(), &modr);
        assert_eq!(owner, vec![Some(0), Some(1), None]);
        let bases = resolve_doodad_bases(&doodads, &groups, &owner, &refs, &molr);
        match &bases[0] {
            DoodadBase::Interior(b) => {
                assert_eq!(b.light_refs, vec![7, 9]);
                assert_eq!(b.diffuse.map(|v| (v * 255.0).round() as u8), [90, 86, 141]);
                assert_eq!(b.ambient.map(|v| (v * 255.0).round() as u8), [61, 59, 96]);
            }
            other => panic!("interior-group doodad must take the MODD-colour base, got {other:?}"),
        }
        assert_eq!(bases[1], DoodadBase::Exterior);
        assert_eq!(bases[2], DoodadBase::Exterior);
    }

    #[test]
    fn one_exterior_referrer_makes_the_whole_prop_exterior() {
        let doodads = vec![
            prop([0, 0, 0, 255]), // g0 (interior) names it first, g1 (exterior) also names it
            prop([0, 0, 0, 255]), // interior groups only
        ];
        let groups = [grp(true), grp(false), grp(true)];
        // g0 names both props, g1 (exterior) names only prop 0, g2 (interior) names only prop 1.
        let modr = vec![vec![0u16, 1u16], vec![0u16], vec![1u16]];
        let molr = vec![vec![7u16], Vec::new(), vec![4u16]];
        let owner = modr_owners(doodads.len(), &modr);
        let refs = modr_refs(doodads.len(), &modr);
        assert_eq!(
            owner,
            vec![Some(0), Some(0)],
            "g0 is the first referrer of both"
        );
        let bases = resolve_doodad_bases(&doodads, &groups, &owner, &refs, &molr);
        assert_eq!(
            bases[0],
            DoodadBase::Exterior,
            "the exterior referrer wins even though an interior group names it first"
        );
        match &bases[1] {
            DoodadBase::Interior(b) => {
                assert_eq!(b.ambient, [0.0; 3]);
                assert_eq!(b.diffuse, [0.0; 3]);
                assert_eq!(b.light_refs, vec![7]);
            }
            other => panic!("interior-only prop must keep the MODD-colour lane, got {other:?}"),
        }
    }

    /// Booty Bay's entrance arch, `BootyBay.wmo` MODD[3], named first by interior g22 and also by
    /// exterior g42, with a black MODD colour.
    #[test]
    fn booty_bays_entrance_arch_is_sky_lit_not_a_black_silhouette() {
        let data = benilla_formats::wow_data_or_skip!();
        let stem = "World\\wmo\\Azeroth\\Buildings\\Stranglethorn_BootyBay\\BootyBay";
        let chain = benilla_formats::Chain::open(&data).expect("open vanilla patch chain");
        let bytes = chain
            .read(&format!("{stem}.wmo"))
            .expect("read BootyBay.wmo");
        let root = parse_wmo_root(&bytes).expect("parse BootyBay.wmo");
        let refs: Vec<Vec<u16>> = (0..root.group_count())
            .map(|gi| {
                chain
                    .read(&format!("{stem}_{gi:03}.wmo"))
                    .map(|g| wmo_group_doodad_refs(&g))
                    .unwrap_or_default()
            })
            .collect();

        let arch = 3usize;
        assert!(
            root.doodads()[arch]
                .model
                .to_ascii_lowercase()
                .contains("bootybayentrance_02"),
            "MODD[3] should be the entrance arch, got {}",
            root.doodads()[arch].model
        );
        assert_eq!(
            root.doodads()[arch].color,
            [0, 0, 0, 255],
            "the arch's baked MODD colour is unlit black — that is what made the lane load-bearing"
        );
        let owner = modr_owners(root.doodads().len(), &refs);
        let groups = modr_refs(root.doodads().len(), &refs);
        assert_eq!(
            owner[arch],
            Some(22),
            "an INTERIOR group still names the arch first"
        );
        assert!(
            groups[arch]
                .iter()
                .any(|&g| !root.group_infos()[g as usize].interior),
            "…and an EXTERIOR group also names it — the case exterior-wins decides"
        );
        let bases = resolve_doodad_bases(
            root.doodads(),
            root.group_infos(),
            &owner,
            &groups,
            &vec![Vec::new(); root.group_count() as usize],
        );
        assert_eq!(
            bases[arch],
            DoodadBase::Exterior,
            "the entrance arch must take the sky-lit lane, not the all-zero MODD-colour base"
        );
    }

    /// Northshire Abbey's group 3 has no MOLR, and its candle stand takes no point light in the
    /// reference, its own flame included.
    #[test]
    fn no_molr_means_no_point_lights_at_all() {
        let bases = resolve_doodad_bases(
            &[prop([120, 110, 150, 255])],
            &[grp(true)],
            &[Some(0)],
            &[Arc::from(vec![0u16])],
            &[Vec::new()],
        );
        match &bases[0] {
            DoodadBase::Interior(b) => assert!(b.light_refs.is_empty()),
            other => panic!("expected interior base, got {other:?}"),
        }
    }

    #[test]
    fn a_shared_modd_takes_its_first_referencing_groups_molr() {
        // g0 names doodad 2; g1 names 0 and 2; g2 names 1, plus a ref past the end of the list.
        let modr = vec![vec![2u16], vec![0u16, 2u16], vec![1u16, 99u16]];
        assert_eq!(
            modr_owners(3, &modr),
            vec![Some(1), Some(2), Some(0)],
            "doodad 2 is named by g0 and g1 — g0 wins"
        );
        assert_eq!(modr_owners(2, &[]), vec![None, None]);
    }

    #[test]
    fn the_cull_key_is_every_referencing_group() {
        // The fixture above: g0 names 2; g1 names 0 and 2; g2 names 1 and an out-of-range ref.
        let modr = vec![vec![2u16], vec![0u16, 2u16], vec![1u16, 99u16]];
        let refs = modr_refs(3, &modr);
        assert_eq!(&*refs[0], &[1], "doodad 0 is named by g1 alone");
        assert_eq!(&*refs[1], &[2]);
        assert_eq!(
            &*refs[2],
            &[0, 1],
            "doodad 2 hangs in two rooms — either one draws it"
        );
        // A doodad named twice by one group counts once.
        assert_eq!(&*modr_refs(1, &[vec![0u16, 0u16]])[0], &[0]);
        assert!(modr_refs(2, &[])[0].is_empty());
    }
}
