//! WMO root-file parsing: the shared tables a group resolves against, doodads (MODS/MODN/MODD),
//! group infos (MOGI), portals (MOPV/MOPT/MOPR) and fogs (MFOG), plus the root id.

use std::io::Cursor;

use anyhow::Result;
use benilla_wmo::{parse_wmo, ParsedWmo};

use super::find_wmo_chunk;

/// A placed WMO doodad (an MODD entry): an M2 prop in WMO model space (WoW axes, Z up), composed
/// with the instance's world transform at spawn (`benilla_assets::coords::wmo_doodad_local`).
#[derive(Debug, Clone)]
pub struct WmoDoodad {
    /// The M2 path as authored (`.mdx`/`.mdl`), normalized to `.m2` by the loaders; empty when the
    /// MODN offset does not resolve.
    pub model: String,
    pub position: [f32; 3],
    /// Orientation quaternion, `(x, y, z, w)`.
    pub orientation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
    /// The baked colour at `+0x24`, BGRA on disk, stored RGBA: an interior doodad's base light, set
    /// at create as diffuse (HSV value ≥ 112) and ambient (≤ 96), with no footprint sample
    /// (`0x694e90`, `0x6a77e0`).
    pub color: [u8; 4],
}

/// A doodad set (an MODS entry), the range `[start, start + count)` of [`WmoRoot::doodads`]. Set 0
/// always shows; a placement's MODF `doodad_set` adds one more.
#[derive(Debug, Clone, Copy)]
pub struct WmoDoodadSet {
    pub start: u32,
    pub count: u32,
}

/// A group's MOGI entry: its interior class and loose bounding box, WMO model space. A doodad's
/// class is its owning group's (the MODR list holding it); a moving entity's is the reference's
/// down-ray indoor bit, never these boxes, which can float above the group's floor.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupInfo {
    /// `groupFlags & 0x48 == 0`: neither exterior (`0x8`) nor exterior-lit (`0x40`), the test at
    /// `0x6b3f90` and in [`crate::models::RenderSubmesh::interior`].
    pub interior: bool,
    /// `groupFlags & 0x40000`: draw [`WmoRoot::skybox`] instead of the `Light.dbc` dome. A corpus
    /// correlation (`benilla-extract skyboxscan`): only roots with a non-empty MOSB set it.
    pub show_skybox: bool,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
}

/// A parsed WMO root, the tables [`super::wmo_group_submeshes`] resolves each group against.
pub struct WmoRoot {
    /// The raw parse, whose material and texture tables the group batch build reads.
    pub(super) parsed: ParsedWmo,
    doodads: Vec<WmoDoodad>,
    doodad_sets: Vec<WmoDoodadSet>,
    group_infos: Vec<WmoGroupInfo>,
    portals: WmoPortals,
    fogs: Vec<WmoFog>,
    skybox: Option<String>,
}

impl WmoRoot {
    /// Number of group files (`<stem>_NNN.wmo`) this root references.
    pub fn group_count(&self) -> u32 {
        match &self.parsed {
            ParsedWmo::Root(r) => r.n_groups,
            _ => 0,
        }
    }

    /// The placed doodads (MODD) with names resolved; [`Self::doodad_sets`] index into it.
    pub fn doodads(&self) -> &[WmoDoodad] {
        &self.doodads
    }

    /// The doodad sets (MODS), ranges into [`Self::doodads`].
    pub fn doodad_sets(&self) -> &[WmoDoodadSet] {
        &self.doodad_sets
    }

    /// Per-group info (MOGI), in group order.
    pub fn group_infos(&self) -> &[WmoGroupInfo] {
        &self.group_infos
    }

    /// Per-material `TerrainType.dbc` id (MOMT `+0x20`), the footstep sound, indexed by MOPY face
    /// material. When the down-ray hits a building, the reference re-rays that group's render
    /// faces and reads `MOMT[MOPY[face].material_id] + 0x20` (`0x6a26c0`).
    pub fn material_ground_types(&self) -> Vec<u32> {
        match &self.parsed {
            ParsedWmo::Root(r) => r.materials.iter().map(|m| m.ground_type).collect(),
            _ => Vec::new(),
        }
    }

    /// Per-material MOMT `diffColor` (`+0x1C`), RGB 0..1, by [`LiquidMesh::material_id`]: an
    /// interior liquid's raw body colour, the reference's interior water being unlit (`0x6b6420`).
    pub fn material_diff_colors(&self) -> Vec<[f32; 3]> {
        match &self.parsed {
            ParsedWmo::Root(r) => r
                .materials
                .iter()
                .map(|m| {
                    let [red, green, blue] = m.diff_color;
                    [
                        f32::from(red) / 255.0,
                        f32::from(green) / 255.0,
                        f32::from(blue) / 255.0,
                    ]
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The portal graph (MOPV/MOPT/MOPR); empty for most single-group props.
    pub fn portals(&self) -> &WmoPortals {
        &self.portals
    }

    /// The fog records (MFOG), indexed by [`super::WmoGroupHeader::fog_indices`].
    pub fn fogs(&self) -> &[WmoFog] {
        &self.fogs
    }

    /// The MOSB skybox model a [`WmoGroupInfo::show_skybox`] group draws; five 5875 roots have one.
    pub fn skybox(&self) -> Option<&str> {
        self.skybox.as_deref()
    }
}

/// Parse a WMO root file.
pub fn parse_wmo_root(bytes: &[u8]) -> Result<WmoRoot> {
    let parsed =
        parse_wmo(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing WMO root: {e}"))?;
    if !matches!(parsed, ParsedWmo::Root(_)) {
        anyhow::bail!("not a WMO root file");
    }
    let (doodads, doodad_sets) = parse_wmo_doodads(bytes);
    let group_infos = parse_wmo_group_infos(bytes);
    let portals = parse_wmo_portals(bytes);
    let fogs = parse_wmo_fogs(bytes);
    let skybox = parse_skybox(bytes);
    Ok(WmoRoot {
        parsed,
        doodads,
        doodad_sets,
        group_infos,
        portals,
        fogs,
        skybox,
    })
}

/// MOSB: one NUL-terminated `.mdx` path, or four zero bytes for none; returned as `.m2`.
fn parse_skybox(bytes: &[u8]) -> Option<String> {
    let mosb = find_wmo_chunk(bytes, b"BSOM")?;
    let end = mosb.iter().position(|&b| b == 0).unwrap_or(mosb.len());
    let raw = std::str::from_utf8(&mosb[..end]).ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(crate::models::model_path(raw))
}

/// MOGI, stride 32: `flags: u32, bbox: 6 × f32, nameOff: i32`; the flags mirror each group's MOGP
/// header flags.
fn parse_wmo_group_infos(bytes: &[u8]) -> Vec<WmoGroupInfo> {
    let Some(mogi) = find_wmo_chunk(bytes, b"IGOM") else {
        return Vec::new();
    };
    mogi.as_chunks::<32>()
        .0
        .iter()
        .map(|rec| {
            let f = |i: usize| f32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            let flags = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            WmoGroupInfo {
                interior: (flags & 0x48) == 0,
                show_skybox: (flags & 0x40000) != 0,
                bbox_min: [f(4), f(8), f(12)],
                bbox_max: [f(16), f(20), f(24)],
            }
        })
        .collect()
}

/// The portal graph in WMO model space, driving per-group visibility (the reference's `0x6b41c0`).
/// A group's portals: [`Self::refs`] from its header's `portal_ref_start`, `portal_ref_count` long.
#[derive(Debug, Clone, Default)]
pub struct WmoPortals {
    /// MOPV: the pooled portal polygon vertices.
    pub vertices: Vec<[f32; 3]>,
    /// MOPT: one entry per portal.
    pub infos: Vec<WmoPortalInfo>,
    /// MOPR: the graph's edges, each a portal from a group to a neighbour.
    pub refs: Vec<WmoPortalRef>,
}

/// One MOPT portal, 20 bytes: a run of [`WmoPortals::vertices`] and its plane `[nx, ny, nz, dist]`,
/// a point's signed distance being `normal·p + dist`.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalInfo {
    pub start_vertex: u16,
    pub count: u16,
    pub plane: [f32; 4],
}

/// One MOPR reference, 8 bytes: `portal` indexes [`WmoPortals::infos`], `group` is the neighbour
/// behind it, and `side` (±1) is the half-space the camera must be in to enter.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalRef {
    pub portal: u16,
    pub group: u16,
    pub side: i16,
}

/// Parse MOPV/MOPT/MOPR at the strides the reference's root parser `0x6c3a60` reads: MOPV `0xc`
/// (`C3Vector`), MOPT `0x14` (`u16 start, u16 count, C4Plane`), MOPR `0x8` (`u16 portal,
/// u16 group, i16 side, u16 filler`). An absent chunk is empty.
pub fn parse_wmo_portals(bytes: &[u8]) -> WmoPortals {
    let f = |b: &[u8], i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u16 = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let i16 = |b: &[u8], i: usize| i16::from_le_bytes([b[i], b[i + 1]]);

    let vertices = find_wmo_chunk(bytes, b"VPOM") // MOPV
        .map(|c| {
            c.as_chunks::<12>()
                .0
                .iter()
                .map(|r| [f(r, 0), f(r, 4), f(r, 8)])
                .collect()
        })
        .unwrap_or_default();
    let infos = find_wmo_chunk(bytes, b"TPOM") // MOPT
        .map(|c| {
            c.as_chunks::<20>()
                .0
                .iter()
                .map(|r| WmoPortalInfo {
                    start_vertex: u16(r, 0),
                    count: u16(r, 2),
                    plane: [f(r, 4), f(r, 8), f(r, 12), f(r, 16)],
                })
                .collect()
        })
        .unwrap_or_default();
    let refs = find_wmo_chunk(bytes, b"RPOM") // MOPR
        .map(|c| {
            c.as_chunks::<8>()
                .0
                .iter()
                .map(|r| WmoPortalRef {
                    portal: u16(r, 0),
                    group: u16(r, 2),
                    side: i16(r, 4),
                })
                .collect()
        })
        .unwrap_or_default();
    WmoPortals {
        vertices,
        infos,
        refs,
    }
}

/// One MFOG fog record, the v17 on-disk layout at the reference's stride `0x30` (root parser
/// `0x6c3a60`, `CMapObj+0x158`), read field for field. Position and radii are WMO model space.
#[derive(Debug, Clone, Copy)]
pub struct WmoFog {
    pub flags: u32,
    /// Fog sphere centre.
    pub pos: [f32; 3],
    /// Inner radius: full fog inside it.
    pub radius_inner: f32,
    /// Outer radius: no fog influence beyond it.
    pub radius_outer: f32,
    /// Land fog end, world units from the eye.
    pub fog_end: f32,
    /// Land fog start, as a fraction of `fog_end`.
    pub fog_start_scalar: f32,
    /// Land fog colour, the raw on-disk dword.
    pub color: u32,
    pub uw_fog_end: f32,
    /// Underwater fog start, as a fraction of `uw_fog_end`.
    pub uw_fog_start_scalar: f32,
    /// Underwater fog colour, the raw on-disk dword.
    pub uw_color: u32,
}

/// Parse the MFOG records; every 5875 root carries at least one, a null fog with a huge `fog_end`.
pub fn parse_wmo_fogs(bytes: &[u8]) -> Vec<WmoFog> {
    let Some(mfog) = find_wmo_chunk(bytes, b"GOFM") else {
        return Vec::new();
    };
    let f = |b: &[u8], i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let u = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    mfog.as_chunks::<48>()
        .0
        .iter()
        .map(|r| WmoFog {
            flags: u(r, 0),
            pos: [f(r, 4), f(r, 8), f(r, 12)],
            radius_inner: f(r, 0x10),
            radius_outer: f(r, 0x14),
            fog_end: f(r, 0x18),
            fog_start_scalar: f(r, 0x1c),
            color: u(r, 0x20),
            uw_fog_end: f(r, 0x24),
            uw_fog_start_scalar: f(r, 0x28),
            uw_color: u(r, 0x2c),
        })
        .collect()
}

/// `MOHD.wmoID` at `0x20`, the building's `WMOAreaTable.WMOID` key; 0 for a short header.
pub fn wmo_root_id(root_bytes: &[u8]) -> u32 {
    find_wmo_chunk(root_bytes, b"DHOM")
        .filter(|mohd| mohd.len() >= 0x24)
        .map(|mohd| u32::from_le_bytes([mohd[0x20], mohd[0x21], mohd[0x22], mohd[0x23]]))
        .unwrap_or(0)
}

/// Parse MODS (32 B: `name[20], start: u32, count: u32, pad`), MODN and MODD (40 B: `nameOff +
/// flags: u32, pos: 3×f32, quat: 4×f32, scale: f32, color: 4×u8`) from the raw bytes: an MODD name
/// is a byte offset into MODN, which `benilla-wmo`'s split `Vec<String>` loses.
fn parse_wmo_doodads(bytes: &[u8]) -> (Vec<WmoDoodad>, Vec<WmoDoodadSet>) {
    let modn = find_wmo_chunk(bytes, b"NDOM").unwrap_or(&[]);
    let resolve = |off: usize| -> String {
        if off >= modn.len() {
            return String::new();
        }
        let end = modn[off..]
            .iter()
            .position(|&b| b == 0)
            .map_or(modn.len(), |p| off + p);
        String::from_utf8_lossy(&modn[off..end]).into_owned()
    };

    let mut doodads = Vec::new();
    if let Some(modd) = find_wmo_chunk(bytes, b"DDOM") {
        for rec in modd.as_chunks::<40>().0 {
            let f = |i: usize| f32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            let name_and_flags = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
            doodads.push(WmoDoodad {
                model: resolve((name_and_flags & 0x00FF_FFFF) as usize),
                position: [f(4), f(8), f(12)],
                orientation: [f(16), f(20), f(24), f(28)],
                scale: f(32),
                // `CImVector` at `+0x24` is BGRA; stored RGBA.
                color: [rec[38], rec[37], rec[36], rec[39]],
            });
        }
    }

    let mut doodad_sets = Vec::new();
    if let Some(mods) = find_wmo_chunk(bytes, b"SDOM") {
        for rec in mods.as_chunks::<32>().0 {
            let u = |i: usize| u32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
            doodad_sets.push(WmoDoodadSet {
                start: u(20),
                count: u(24),
            });
        }
    }
    (doodads, doodad_sets)
}

#[cfg(test)]
mod tests {
    use super::super::test_bytes::{chunk, f3};
    use super::*;

    #[test]
    fn parses_portal_vertices_infos_and_refs() {
        let mut mopv = Vec::new();
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            mopv.extend(f3(v));
        }
        let mut mopt = Vec::new();
        mopt.extend_from_slice(&0u16.to_le_bytes());
        mopt.extend_from_slice(&3u16.to_le_bytes());
        mopt.extend(f3([0.0, 0.0, 1.0]));
        mopt.extend_from_slice(&(-2.5f32).to_le_bytes());
        let mut mopr = Vec::new();
        for (p, g, s) in [(0u16, 1u16, 1i16), (0, 7, -1)] {
            mopr.extend_from_slice(&p.to_le_bytes());
            mopr.extend_from_slice(&g.to_le_bytes());
            mopr.extend_from_slice(&s.to_le_bytes());
            mopr.extend_from_slice(&0u16.to_le_bytes()); // filler
        }
        let mut bytes = Vec::new();
        bytes.extend(chunk(b"VPOM", &mopv));
        bytes.extend(chunk(b"TPOM", &mopt));
        bytes.extend(chunk(b"RPOM", &mopr));

        let p = parse_wmo_portals(&bytes);
        assert_eq!(p.vertices.len(), 3);
        assert_eq!(p.vertices[2], [0.0, 0.0, 1.0]);
        assert_eq!(p.infos.len(), 1);
        assert_eq!(p.infos[0].start_vertex, 0);
        assert_eq!(p.infos[0].count, 3);
        assert_eq!(p.infos[0].plane, [0.0, 0.0, 1.0, -2.5]);
        assert_eq!(p.refs.len(), 2);
        assert_eq!(
            (p.refs[0].portal, p.refs[0].group, p.refs[0].side),
            (0, 1, 1)
        );
        assert_eq!(
            (p.refs[1].portal, p.refs[1].group, p.refs[1].side),
            (0, 7, -1)
        );
    }

    #[test]
    fn absent_portal_chunks_yield_empty_graph() {
        let p = parse_wmo_portals(&[]);
        assert!(p.vertices.is_empty() && p.infos.is_empty() && p.refs.is_empty());
    }

    #[test]
    fn parses_mfog_records() {
        // A null fog (huge end) and a room fog.
        let mut mfog = Vec::new();
        for (pos, r_in, r_out, end, start_scalar, color) in [
            (
                [0.0f32, 0.0, 0.0],
                0.0f32,
                0.0f32,
                444.4445f32,
                0.25f32,
                0xff000000u32,
            ),
            ([10.0, -4.0, 2.5], 5.0, 20.0, 80.0, 0.1, 0xff102030),
        ] {
            mfog.extend_from_slice(&0u32.to_le_bytes()); // flags
            mfog.extend(f3(pos));
            for v in [r_in, r_out, end, start_scalar] {
                mfog.extend_from_slice(&v.to_le_bytes());
            }
            mfog.extend_from_slice(&color.to_le_bytes());
            // Underwater triple: end, start scalar, colour.
            for v in [222.2f32, 0.5] {
                mfog.extend_from_slice(&v.to_le_bytes());
            }
            mfog.extend_from_slice(&0xff445566u32.to_le_bytes());
        }
        let bytes = chunk(b"GOFM", &mfog);

        let fogs = parse_wmo_fogs(&bytes);
        assert_eq!(fogs.len(), 2);
        assert_eq!(fogs[0].fog_end, 444.4445);
        assert_eq!(fogs[0].color, 0xff000000);
        assert_eq!(fogs[1].pos, [10.0, -4.0, 2.5]);
        assert_eq!(fogs[1].radius_inner, 5.0);
        assert_eq!(fogs[1].radius_outer, 20.0);
        assert_eq!(fogs[1].fog_start_scalar, 0.1);
        assert_eq!(fogs[1].uw_fog_end, 222.2);
        assert_eq!(fogs[1].uw_color, 0xff445566);
        assert!(parse_wmo_fogs(&[]).is_empty());
    }
}
