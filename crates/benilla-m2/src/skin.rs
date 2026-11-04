//! Embedded skin profiles: indices, triangles, submeshes and batches.

use benilla_bytes::{capped, ByteExt};

use crate::error::{Error, Result};
use crate::model::M2Model;

/// One skin submesh: its triangle range, geoset id, bone palette and depth-key centre.
pub struct SkinSection {
    /// `skinSectionId`, the geoset id `group*100 + variant` that character geoset selection
    /// toggles; 0 on a single-geoset model.
    pub id: u16,
    pub triangle_start: u16,
    pub triangle_count: u16,
    /// The first bone lookup (`+0x0e`), a key of the opaque order (`0x70ab21`).
    pub bone_lookup: u16,
    /// The vertex bone-weight count (`+0x10`), digit `B` of the vertex shader id (`0x71de50`).
    pub bone_influences: u16,
    /// The bone the depth-key centre rides (`+0x12`, `0x707d6a`).
    pub center_bone: u16,
    /// The depth-key centre (`+0x14`), model space, raw WoW axes (`0x707d7a`).
    pub center: [f32; 3],
}

/// One skin draw batch (section + material + texture-combo references).
pub struct SkinBatch {
    /// Batch flags (texUnit `+0x00`), raw; the renderer ignores them.
    pub flags: u16,
    /// The priority plane (texUnit `+0x02`), read signed (`movsx` at `0x707c76`).
    pub priority_plane: i16,
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
    /// The unit's layer on its submesh (texUnit `+0x0c`), 0 to n−1: the transparent order's last
    /// key before the opaque order (`0x70afa3`).
    pub layer: u16,
}

/// A decoded embedded skin profile.
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

        // `bytes_at` checks the whole run first, so `n` needs no `capped`.
        let read_u16s = |n: usize, o: usize| -> Result<Vec<u16>> {
            let slice = bytes.bytes_at(o, n * 2).ok_or(Error::Truncated)?;
            let mut v = Vec::with_capacity(n);
            for c in slice.as_chunks::<2>().0 {
                v.push(c.u16_at(0).ok_or(Error::Truncated)?);
            }
            Ok(v)
        };
        let indices = read_u16s(n_idx, o_idx)?;
        let triangles = read_u16s(n_tri, o_tri)?;

        // M2SkinSection: 32 bytes before v260, 48 from it; the triangle range is @8 and @10.
        let sec = if self.version < 260 { 32 } else { 48 };
        // `n_sub` and `n_bat` are unvalidated, so the reservations are capped.
        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, sec, sub_avail));
        for i in 0..n_sub {
            let s = bytes
                .bytes_at(o_sub + i * sec, sec)
                .ok_or(Error::Truncated)?;
            let f = |o: usize| s.f32_at(o).ok_or(Error::Truncated);
            submeshes.push(SkinSection {
                id: s.u16_at(0).ok_or(Error::Truncated)?, // skinSectionId @ +0x00
                triangle_start: s.u16_at(8).ok_or(Error::Truncated)?,
                triangle_count: s.u16_at(10).ok_or(Error::Truncated)?,
                bone_lookup: s.u16_at(0x0e).ok_or(Error::Truncated)?,
                bone_influences: s.u16_at(0x10).ok_or(Error::Truncated)?,
                center_bone: s.u16_at(0x12).ok_or(Error::Truncated)?,
                center: [f(0x14)?, f(0x18)?, f(0x1c)?],
            });
        }
        // M2TextureUnit, a batch: 24 bytes.
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, 24, bat_avail));
        for i in 0..n_bat {
            let u = bytes.bytes_at(o_bat + i * 24, 24).ok_or(Error::Truncated)?;
            batches.push(SkinBatch {
                flags: u.u16_at(0).ok_or(Error::Truncated)?,
                priority_plane: u.u16_at(2).ok_or(Error::Truncated)? as i16,
                skin_section_index: u.u16_at(4).ok_or(Error::Truncated)?,
                material_index: u.u16_at(0x0a).ok_or(Error::Truncated)?,
                texture_combo_index: u.u16_at(0x10).ok_or(Error::Truncated)?,
                texture_coord_combo_index: u.u16_at(0x12).ok_or(Error::Truncated)?,
                color_index: u.u16_at(0x08).ok_or(Error::Truncated)?,
                weight_combo_index: u.u16_at(0x14).ok_or(Error::Truncated)?,
                texture_count: u.u16_at(0x0e).ok_or(Error::Truncated)?,
                texture_transform_combo_index: u.u16_at(0x16).ok_or(Error::Truncated)?,
                layer: u.u16_at(0x0c).ok_or(Error::Truncated)?,
            });
        }
        Ok(Skin {
            indices,
            triangles,
            submeshes,
            batches,
        })
    }
}
