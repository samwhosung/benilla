//! Skin profiles: the embedded (pre-Wrath) `M2View`s and the external Wrath (3.3.5) `*.skin`
//! files. A profile is the mesh a model renders from — a `u16` lookup into the base vertices, the
//! triangle corners over it, and the submeshes and batches that draw it. The base `M2Vertex`
//! carries position, normal, UV and the bone data; a profile adds only the topology.

use benilla_bytes::{capped, ByteExt};

use crate::error::{Error, Result};
use crate::model::M2Model;

/// One skin submesh: its triangle range and geoset id.
pub struct SkinSection {
    /// `skinSectionId`, the geoset id `group*100 + variant` that character geoset selection
    /// toggles; 0 on a single-geoset model.
    pub id: u16,
    pub triangle_start: u16,
    pub triangle_count: u16,
}

/// One skin draw batch (section + material + texture-combo references).
pub struct SkinBatch {
    /// Batch flags (texUnit `+0x00`), raw; the renderer ignores them.
    pub flags: u16,
    /// Shader or render-order selector (texUnit `+0x02`), raw; the renderer ignores it.
    pub shader_id: u16,
    pub skin_section_index: u16,
    pub texture_combo_index: u16,
    /// Index into [`crate::M2Model::texture_unit_lookup`], the stage's coordinate source (texUnit
    /// `+0x12`, `0x70b8a4`).
    pub texture_coord_combo_index: u16,
    pub material_index: u16,
    /// Index into [`crate::M2Model::color_alpha_tracks`], `0xffff` for none (texUnit `+0x08`).
    pub color_index: u16,
    /// Index into [`crate::M2Model::transparency_lookup`] (texUnit `+0x14`).
    pub weight_combo_index: u16,
    /// Texture count (texUnit `+0x0e`); at 0 the transparency factor is not applied.
    pub texture_count: u16,
    /// Index into [`crate::M2Model::texture_transform_lookup`] (texUnit `+0x16`, `0x70b897`).
    pub texture_transform_combo_index: u16,
}

/// A decoded skin profile.
pub struct Skin {
    indices: Vec<u16>,
    triangles: Vec<u16>,
    submeshes: Vec<SkinSection>,
    batches: Vec<SkinBatch>,
}
impl Skin {
    pub fn indices(&self) -> &Vec<u16> {
        &self.indices
    }
    pub fn triangles(&self) -> &Vec<u16> {
        &self.triangles
    }
    pub fn submeshes(&self) -> &Vec<SkinSection> {
        &self.submeshes
    }
    pub fn batches(&self) -> &Vec<SkinBatch> {
        &self.batches
    }
}

// ── Record readers shared by the embedded and external profiles ──────────────────────────────

/// `n` `u16`s at `o`; the run must fit the file, so a short one is `Truncated`.
fn read_u16s(bytes: &[u8], n: usize, o: usize) -> Result<Vec<u16>> {
    let slice = bytes.bytes_at(o, n * 2).ok_or(Error::Truncated)?;
    let mut v = Vec::with_capacity(n);
    for c in slice.as_chunks::<2>().0 {
        v.push(c.u16_at(0).ok_or(Error::Truncated)?);
    }
    Ok(v)
}

/// A submesh's geoset id and triangle range. The range sits at `+8`/`+0x0a` whether the record is
/// 32 bytes (pre-260) or 48 (from it), so the readers of both profile kinds share it.
fn read_submesh(s: &[u8]) -> Result<SkinSection> {
    Ok(SkinSection {
        id: s.u16_at(0).ok_or(Error::Truncated)?, // skinSectionId @ +0x00
        triangle_start: s.u16_at(8).ok_or(Error::Truncated)?,
        triangle_count: s.u16_at(10).ok_or(Error::Truncated)?,
    })
}

/// A 24-byte batch (`ssTextureUnits`): the section it draws plus its material and texture-combo
/// references. The field offsets are the embedded `M2TextureUnit`'s, so both readers share it.
fn read_batch(u: &[u8]) -> Result<SkinBatch> {
    Ok(SkinBatch {
        flags: u.u16_at(0).ok_or(Error::Truncated)?,
        shader_id: u.u16_at(2).ok_or(Error::Truncated)?,
        skin_section_index: u.u16_at(4).ok_or(Error::Truncated)?,
        material_index: u.u16_at(0x0a).ok_or(Error::Truncated)?,
        texture_combo_index: u.u16_at(0x10).ok_or(Error::Truncated)?,
        texture_coord_combo_index: u.u16_at(0x12).ok_or(Error::Truncated)?,
        color_index: u.u16_at(0x08).ok_or(Error::Truncated)?,
        weight_combo_index: u.u16_at(0x14).ok_or(Error::Truncated)?,
        texture_count: u.u16_at(0x0e).ok_or(Error::Truncated)?,
        texture_transform_combo_index: u.u16_at(0x16).ok_or(Error::Truncated)?,
    })
}

impl M2Model {
    /// Decode embedded skin profile `index` from the file `bytes`: one of the 44-byte `M2View`
    /// headers, each naming its arrays by file offset.
    pub fn parse_embedded_skin(&self, bytes: &[u8], index: usize) -> Result<Skin> {
        let (count, ofs) = self.views;
        if index >= count as usize {
            return Err(Error::Truncated);
        }
        let vp = ofs as usize + index * 44; // M2View header: 5 M2Arrays (40) + bone count (4)
        let rd_arr = |o: usize| -> Result<(usize, usize)> {
            Ok((
                bytes.u32_at(o).ok_or(Error::Truncated)? as usize,
                bytes.u32_at(o + 4).ok_or(Error::Truncated)? as usize,
            ))
        };
        let (n_idx, o_idx) = rd_arr(vp)?;
        let (n_tri, o_tri) = rd_arr(vp + 8)?;
        // properties @ vp+16 (unused)
        let (n_sub, o_sub) = rd_arr(vp + 24)?;
        let (n_bat, o_bat) = rd_arr(vp + 32)?;

        let indices = read_u16s(bytes, n_idx, o_idx)?;
        let triangles = read_u16s(bytes, n_tri, o_tri)?;

        // `ssSubmeshes`: 32 bytes before v260, 48 from it; the triangle range is @8 and @0x0a.
        let sec = if self.version < 260 { 32 } else { 48 };
        // `n_sub` and `n_bat` are unvalidated, so the reservations are capped.
        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, sec, sub_avail));
        for i in 0..n_sub {
            let s = bytes
                .bytes_at(o_sub + i * sec, sec)
                .ok_or(Error::Truncated)?;
            submeshes.push(read_submesh(s)?);
        }
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, 24, bat_avail));
        for i in 0..n_bat {
            let u = bytes.bytes_at(o_bat + i * 24, 24).ok_or(Error::Truncated)?;
            batches.push(read_batch(u)?);
        }
        Ok(Skin {
            indices,
            triangles,
            submeshes,
            batches,
        })
    }

    /// Decode a Wrath (3.3.5) external `*.skin` file: `"SKIN"`, five `(count, offset)` arrays, and
    /// a `boneCountMax`. The vertex list is a `u16` per profile vertex, an index into the base
    /// `M2Vertex`s; the base vertex carries position, normal, UV and the bone data, so the profile
    /// adds only the triangle corners and the submeshes and batches that draw them.
    pub fn parse_skin_file(&self, bytes: &[u8]) -> Result<Skin> {
        // `SKIN`, then five `(count, offset)` arrays and a `boneCountMax` (`+0x2c`): 48 bytes.
        if bytes.len() < 0x30 || &bytes[0..4] != b"SKIN" {
            return Err(Error::NotSkin);
        }
        let rd_arr = |o: usize| -> Result<(usize, usize)> {
            Ok((
                bytes.u32_at(o).ok_or(Error::Truncated)? as usize,
                bytes.u32_at(o + 4).ok_or(Error::Truncated)? as usize,
            ))
        };
        // `vertices` (`+0x04`): a `u16` per profile vertex, an index into the base vertices.
        let (n_vert, o_vert) = rd_arr(0x04)?;
        // `indices` (`+0x0c`): a `u16` per triangle corner, an index into the profile's `vertices`.
        let (n_idx, o_idx) = rd_arr(0x0c)?;
        // `bones` (`+0x14`): a `u8` × 4 per vertex. The base vertices carry the same binding, so
        // benilla renders through them and this array is not read.
        // `submeshes` (`+0x1c`): 48-byte `ssSubmeshes`; a `.skin` is Wrath-only, so always 48.
        let (n_sub, o_sub) = rd_arr(0x1c)?;
        // `batches` (`+0x24`): 24-byte `ssTextureUnits`. `boneCountMax` (`+0x2c`) is not read.
        let (n_bat, o_bat) = rd_arr(0x24)?;

        // The profile's vertex list becomes `Skin.indices` (the lookup into the base vertices);
        // every entry must land in the base list.
        let indices = read_u16s(bytes, n_vert, o_vert)?;
        if indices.iter().any(|&g| (g as usize) >= self.vertices.len()) {
            return Err(Error::Truncated);
        }
        // The profile's triangle corners become `Skin.triangles`; each indexes into `vertices`.
        let triangles = read_u16s(bytes, n_idx, o_idx)?;
        if triangles.iter().any(|&t| (t as usize) >= n_vert) {
            return Err(Error::Truncated);
        }

        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, 48, sub_avail));
        for i in 0..n_sub {
            let s = bytes.bytes_at(o_sub + i * 48, 48).ok_or(Error::Truncated)?;
            submeshes.push(read_submesh(s)?);
        }
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, 24, bat_avail));
        for i in 0..n_bat {
            let u = bytes.bytes_at(o_bat + i * 24, 24).ok_or(Error::Truncated)?;
            batches.push(read_batch(u)?);
        }
        Ok(Skin {
            indices,
            triangles,
            submeshes,
            batches,
        })
    }
}
