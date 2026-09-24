//! WMO **root**-file parsing: the shared tables a group resolves against — doodads (MODS/MODN/MODD),
//! group infos (MOGI), the portal graph (MOPV/MOPT/MOPR), fog records (MFOG) — plus the root id.

use std::io::Cursor;

use anyhow::Result;
use benilla_wmo::{parse_wmo, ParsedWmo};

use super::find_wmo_chunk;

/// A placed WMO **doodad** (an MODD entry): an M2 prop the WMO root positions *inside* itself —
/// candle stands, banners, braziers, chandeliers — as opposed to geometry baked into the WMO mesh
/// (pews, walls). Resolved + in WMO model space (WoW axes, Z up); compose with the WMO instance's
/// world transform at spawn (`benilla_assets::coords::wmo_doodad_local`).
#[derive(Debug, Clone)]
pub struct WmoDoodad {
    /// M2 path (MMDX-style, `.mdx`/`.mdl`); `m2_url`/`load_m2_mesh` normalize it to `.m2`. Empty ⇒
    /// the MODN name offset didn't resolve (skipped at spawn).
    pub model: String,
    /// Position in WMO model space (WoW axes).
    pub position: [f32; 3],
    /// Orientation quaternion `(x, y, z, w)` in WMO model space.
    pub orientation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
    /// The MODD entry's **baked colour** (`+0x24`, BGRA on disk → stored `[r, g, b, a]`): the WMO
    /// baker's authored local-lighting product for this placement. For an INTERIOR-group doodad this
    /// IS the slot-0 base light — the real client copies it at doodad create into the diffuse word
    /// (floor-raised to HSV value 112) and the ambient word (capped to max 96), never running any
    /// footprint/attach sample (`0x694e90` create → `6950de call 0x6a77e0(&MODD.colour,
    /// …, 0x70, …, 0x60)`; in a reference capture the abbey stands'
    /// decoded diffuse words equal these bytes verbatim).
    pub color: [u8; 4],
}

/// A WMO **doodad set** (an MODS entry): a contiguous range `[start, start+count)` into the
/// [`WmoRoot::doodads`] list. Set 0 is the global set (always shown); a placement's MODF
/// `doodad_set` field selects one *additional* set to show by index.
#[derive(Debug, Clone, Copy)]
pub struct WmoDoodadSet {
    pub start: u32,
    pub count: u32,
}

/// Per-WMO-**group** info from the root's MOGI chunk: the group's interior/exterior class and its
/// axis-aligned bounding box (WMO model space, WoW axes). A placed MODD doodad's class comes from
/// its OWNING group (the MODR reference that instantiates it — see [`super::wmo_group_doodad_refs`]);
/// moving ENTITIES classify by the position-cast face down-ray instead (the client's own indoor
/// bit — never these boxes, whose authored bounds can float above their own floor polys). The box
/// is a loose AABB approximation of the group's true BSP/portal volume.
#[derive(Debug, Clone, Copy)]
pub struct WmoGroupInfo {
    /// `groupFlags & 0x48 == 0` — neither EXTERIOR (`0x8`) nor exterior-lit (`0x40`): a true interior
    /// group. The same test the client branches on at `0x6b3f90` and that [`crate::models::RenderSubmesh::interior`] uses.
    pub interior: bool,
    /// `groupFlags & 0x40000` — **SHOW_SKYBOX**: this group draws the root's [`WmoRoot::skybox`] model
    /// in place of the `Light.dbc` gradient dome. The bit's identity is a corpus correlation
    /// (`benilla-extract skyboxscan`): across the 5875 chain it appears on a group **iff** its root
    /// carries a non-empty MOSB — Stratholme's city shell sets it on 61 of 83 groups (its open streets,
    /// which are authored as INTERIOR groups), while the sibling dungeon `Stratholme.wmo` (MOSB empty)
    /// sets it on none of 92.
    pub show_skybox: bool,
    /// Group bounding-box min, WMO model space (WoW axes).
    pub bbox_min: [f32; 3],
    /// Group bounding-box max, WMO model space (WoW axes).
    pub bbox_max: [f32; 3],
}

/// A parsed WMO **root**: the shared material/texture tables the group submeshes resolve against,
/// plus the resolved doodad tables (sets + placed props). Build with [`parse_wmo_root`], then feed
/// each group file's bytes to [`super::wmo_group_submeshes`]. The Bevy WMO `AssetLoader`
/// orchestrates the root + group reads; the chain-reading equivalent is [`super::load_wmo`].
pub struct WmoRoot {
    /// The raw parse — the group-side batch build reads the material/texture tables through it.
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

    /// The root's placed doodads (MODD), names resolved, indexable by the ranges in [`Self::doodad_sets`].
    pub fn doodads(&self) -> &[WmoDoodad] {
        &self.doodads
    }

    /// The root's doodad sets (MODS) — ranges into [`Self::doodads`].
    pub fn doodad_sets(&self) -> &[WmoDoodadSet] {
        &self.doodad_sets
    }

    /// The root's per-group info (MOGI) — interior flag + bounding box, for classifying which placed
    /// doodads are interior (see [`WmoGroupInfo`]).
    pub fn group_infos(&self) -> &[WmoGroupInfo] {
        &self.group_infos
    }

    /// Per-material **`TerrainType.dbc` id** (MOMT+0x20), indexed by the MOPY per-face material id
    /// — what walking on each material sounds like, shared root-wide across every group.
    ///
    /// The footstep chain's WMO leg (decision 1161): the client's down-ray arbitrates terrain
    /// against a building's collision faces, and when the building wins it re-rays that group's
    /// RENDER faces and resolves `MOPY[face].material_id → MOMT[id]+0x20` (`0x6a26c0`).
    pub fn material_ground_types(&self) -> Vec<u32> {
        match &self.parsed {
            ParsedWmo::Root(r) => r.materials.iter().map(|m| m.ground_type).collect(),
            _ => Vec::new(),
        }
    }

    /// Per-material MOMT `diffColor` (`+0x1C`), RGB 0..1. Read for one consumer: an **interior**
    /// WMO liquid pool's body colour is `MOMT[MLIQ.materialId].diffColor`, taken raw, because the
    /// reference's interior water kernel runs unlit with no pixel shader (`0x6b6420`). Indexed by
    /// [`LiquidMesh::material_id`].
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

    /// The root's portal graph (MOPV/MOPT/MOPR) — the data that drives per-group visibility culling
    /// (see [`WmoPortals`]). Empty for WMOs with no portals (most single-group props).
    pub fn portals(&self) -> &WmoPortals {
        &self.portals
    }

    /// The root's fog records (MFOG) — indexed by a group header's [`super::WmoGroupHeader::fog_indices`]
    /// (see [`WmoFog`] for what's pinned vs pending about their consumption).
    pub fn fogs(&self) -> &[WmoFog] {
        &self.fogs
    }

    /// The root's **MOSB** skybox model — the M2 a [`WmoGroupInfo::show_skybox`] group draws instead of
    /// the `Light.dbc` gradient dome. `None` when the chunk is absent or holds only the empty string
    /// (the overwhelming majority: five roots in the whole 5875 chain author one).
    pub fn skybox(&self) -> Option<&str> {
        self.skybox.as_deref()
    }
}

/// Parse a WMO **root** file from bytes. `Err` if the bytes aren't a WMO root.
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

/// Parse the root's **MOSB** chunk — a single null-terminated model path, or the 4 zero bytes that
/// stand for "no skybox" (which is what almost every root carries). Returned normalized to `.m2`, the
/// extension the loaders take; the chunk itself names the authoring-time `.mdx`.
fn parse_skybox(bytes: &[u8]) -> Option<String> {
    let mosb = find_wmo_chunk(bytes, b"BSOM")?;
    let end = mosb.iter().position(|&b| b == 0).unwrap_or(mosb.len());
    let raw = std::str::from_utf8(&mosb[..end]).ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(crate::models::model_path(raw))
}

/// Parse the root's **MOGI** group table (`[flags:u32][bbox:6×f32][nameOff:i32]`, stride 32) into the
/// per-group interior flag + bounding box used to classify placed doodads (see [`WmoGroupInfo`]). The
/// flags here mirror the per-group MOGP header flags ([`crate::models::RenderSubmesh::interior`] reads the latter).
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

/// The WMO root's **portal graph** — the three root chunks that drive per-group visibility culling
/// (the portal recursion `0x6b41c0`). All coordinates are WMO model space (WoW axes, Z up), the
/// same space as [`WmoGroupInfo`] bounds. A group references its portals through a slice of
/// [`Self::refs`] given by the group header's `portal_ref_start`/`portal_ref_count`
/// ([`super::wmo_group_header`]).
#[derive(Debug, Clone, Default)]
pub struct WmoPortals {
    /// **MOPV** — the portal polygon vertices, pooled; a portal indexes a contiguous run via
    /// [`WmoPortalInfo::start_vertex`]/[`WmoPortalInfo::count`].
    pub vertices: Vec<[f32; 3]>,
    /// **MOPT** — one entry per portal: its vertex run + separating plane.
    pub infos: Vec<WmoPortalInfo>,
    /// **MOPR** — portal references: per-group edges of the portal graph (which portal joins which
    /// neighbouring group, and on which side of the plane that neighbour lies).
    pub refs: Vec<WmoPortalRef>,
}

/// One **MOPT** portal (20 bytes on disk): a polygon (a run of [`WmoPortals::vertices`]) plus the
/// plane it lies in. `plane` is `[nx, ny, nz, dist]` — a unit normal and the signed plane offset, so a
/// point `p` is at signed distance `normal·p + dist` from the portal.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalInfo {
    pub start_vertex: u16,
    pub count: u16,
    pub plane: [f32; 4],
}

/// One **MOPR** portal reference (8 bytes on disk): an edge in a group's portal slice. `portal`
/// indexes [`WmoPortals::infos`]; `group` is the neighbouring group reached through it; `side`
/// (`±1`) orients the plane test — the camera must be on the `side`-facing half-space to enter.
#[derive(Debug, Clone, Copy)]
pub struct WmoPortalRef {
    pub portal: u16,
    pub group: u16,
    pub side: i16,
}

/// Parse the root's portal chunks (**MOPV**/**MOPT**/**MOPR**) into the portal graph. Absent chunks
/// yield empty vecs (a WMO with no portals — most single-group props). Strides/field offsets are
/// VERIFIED from the client's consumer in `WoW.exe` 5875 (parser `0x6c3a60`): MOPV stride `0xc`
/// (`C3Vector`), MOPT stride `0x14` (`u16 start, u16 count, C4Plane`), MOPR stride `0x8`
/// (`u16 portal, u16 group, i16 side, u16 filler`).
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

/// One **MFOG** fog record (48 bytes on disk — stride `0x30`, the client's own
/// (root parser `0x6c3a60`: `CMapObj+0x158` mfog_ptr, stride `0x30`)). Fields are the verbatim
/// v17 on-disk layout; *which* record an interior group selects and how the client consumes
/// `start_scalar` are still open — parse now, apply only once pinned. Position/radii are WMO model
/// space, same as [`WmoGroupInfo`] bounds.
#[derive(Debug, Clone, Copy)]
pub struct WmoFog {
    pub flags: u32,
    /// Fog sphere centre, WMO model space (WoW axes).
    pub pos: [f32; 3],
    /// Inner radius — full fog inside this.
    pub radius_inner: f32,
    /// Outer radius — no fog influence beyond this.
    pub radius_outer: f32,
    /// Land fog end distance (world units from the eye).
    pub fog_end: f32,
    /// Land fog start, as a scalar of `fog_end` per the v17 layout (client transform still open).
    pub fog_start_scalar: f32,
    /// Land fog colour — raw on-disk dword (`u32 LE`; byte-order decode deferred to the fold).
    pub color: u32,
    /// Underwater fog end distance.
    pub uw_fog_end: f32,
    /// Underwater fog start scalar.
    pub uw_fog_start_scalar: f32,
    /// Underwater fog colour — raw on-disk dword.
    pub uw_color: u32,
}

/// Parse the root's **MFOG** fog records (stride 48). Absent chunk yields an empty vec — but note
/// every shipped 5875 root carries at least one MFOG record (the "null fog", huge `fog_end`).
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

/// The root's `MOHD.wmoID` @ `0x20` — the `WMOAreaTable.WMOID` key for the whole building
/// (byte-verified 2026-07-03: NSabbey.wmo → 59, the abbey rows; Chapel.wmo → 656). `0` when
/// the header is too short (no 5875 root is).
pub fn wmo_root_id(root_bytes: &[u8]) -> u32 {
    find_wmo_chunk(root_bytes, b"DHOM")
        .filter(|mohd| mohd.len() >= 0x24)
        .map(|mohd| u32::from_le_bytes([mohd[0x20], mohd[0x21], mohd[0x22], mohd[0x23]]))
        .unwrap_or(0)
}

/// Parse the root's doodad tables straight from the raw bytes: MODS (sets), MODN (name blob), MODD
/// (defs). We parse these three small fixed-layout chunks ourselves rather than via `benilla-wmo`
/// because an MODD's name is a **byte offset into the MODN blob**, which a split-on-NUL `Vec<String>`
/// discards — so we read the blob + offsets directly. Records: MODS = 32 B
/// (`name[20]`, `start:u32`, `count:u32`, pad), MODD = 40 B (`nameOff+flags:u32`, `pos:3×f32`,
/// `quat:4×f32`, `scale:f32`, `color:4×u8`).
fn parse_wmo_doodads(bytes: &[u8]) -> (Vec<WmoDoodad>, Vec<WmoDoodadSet>) {
    let modn = find_wmo_chunk(bytes, b"NDOM").unwrap_or(&[]);
    // Resolve a MODN byte offset → the NUL-terminated M2 path at that offset.
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
                // The baked colour at +0x24 — CImVector is BGRA in memory; store as RGBA.
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
        // MOPV: 3 vertices.
        let mut mopv = Vec::new();
        for v in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            mopv.extend(f3(v));
        }
        // MOPT: one portal — startVertex 0, count 3, plane (0,0,1, -2.5).
        let mut mopt = Vec::new();
        mopt.extend_from_slice(&0u16.to_le_bytes());
        mopt.extend_from_slice(&3u16.to_le_bytes());
        mopt.extend(f3([0.0, 0.0, 1.0]));
        mopt.extend_from_slice(&(-2.5f32).to_le_bytes());
        // MOPR: two refs — (portal 0, group 1, side +1), (portal 0, group 7, side -1).
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
        // Two 48-byte MFOG records: a "null fog" (huge end) and a room fog.
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
