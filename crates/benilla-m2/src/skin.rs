//! Embedded skin-profile parsing: [`Skin`]/[`SkinSection`]/[`SkinBatch`] and
//! [`M2Model::parse_embedded_skin`], which decodes one of the model's embedded skin profiles
//! (indices / triangles / submeshes / batches) by file offset.

use benilla_bytes::{capped, ByteExt};

use crate::error::{Error, Result};
use crate::model::M2Model;

/// One skin submesh section (we keep the triangle range the renderer iterates, plus the section's
/// `skinSectionId` — the geoset/mesh-part ID the character compositor selects visibility by).
pub struct SkinSection {
    /// `skinSectionId` (mesh-part / geoset ID): `group*100 + variant`. For a character model this is
    /// what geoset selection toggles (hide all but the chosen hair/facial-hair/body variants); `0` for a
    /// model with a single unnamed geoset (most creatures/doodads).
    pub id: u16,
    pub triangle_start: u16,
    pub triangle_count: u16,
}

/// One skin draw batch (section + material + texture-combo references).
pub struct SkinBatch {
    /// Batch flags (texUnit `+0x00`) — kept raw, unclassified. Read by the diagnostic dumps while
    /// we carve which bits the reference's batch path actually reads; the renderer ignores them.
    pub flags: u16,
    /// Shader / render-order selector (texUnit `+0x02`), same status as [`Self::flags`].
    pub shader_id: u16,
    pub skin_section_index: u16,
    pub texture_combo_index: u16,
    /// Index into [`crate::M2Model::texture_unit_lookup`] → where this stage's texture coordinates
    /// come from (a UV channel, or a generated environment coordinate). texUnit `+0x12`
    /// (`0x70b8a4`).
    pub texture_coord_combo_index: u16,
    pub material_index: u16,
    /// Direct index into [`crate::M2Model::color_alpha_tracks`] (`0xffff` = none). texUnit `+0x08`.
    pub color_index: u16,
    /// Index into [`crate::M2Model::transparency_lookup`] → a transparency track. texUnit `+0x14`.
    pub weight_combo_index: u16,
    /// Texture count (texUnit `+0x0e`); `0` ⇒ the transparency factor is not applied (verified gate).
    pub texture_count: u16,
    /// Index into [`crate::M2Model::texture_transform_lookup`] → a texture transform (UV
    /// animation). texUnit `+0x16` (`0x70b897`).
    pub texture_transform_combo_index: u16,
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
    /// Decode embedded skin profile `index` from the original file `bytes`. The skin profiles are an
    /// array of 44-byte `M2View` headers at `views.offset`; each references its indices / triangles /
    /// submeshes / batches by file offset.
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

        // `bytes_at(o, n * 2)` bounds-checks the whole run before any element is read, so — unlike
        // the header-driven reservations above — `n` is already provably within the file's own size
        // once this succeeds; no separate `capped()` needed for the `Vec::with_capacity(n)` below.
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

        // M2SkinSection: 32 bytes pre-Wrath (v<260), 48 with the later centre-bounds; indexStart @8,
        // indexCount @10 are the triangle range either way.
        let sec = if self.version < 260 { 32 } else { 48 };
        // Reservations capped (decision 0064): `n_sub`/`n_bat` come straight from the M2View header,
        // unvalidated, and the loops below check bounds only per-element — same shape as the header
        // arrays above, so the same guard applies.
        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, sec, sub_avail));
        for i in 0..n_sub {
            let s = bytes
                .bytes_at(o_sub + i * sec, sec)
                .ok_or(Error::Truncated)?;
            submeshes.push(SkinSection {
                id: s.u16_at(0).ok_or(Error::Truncated)?, // skinSectionId @ +0x00
                triangle_start: s.u16_at(8).ok_or(Error::Truncated)?,
                triangle_count: s.u16_at(10).ok_or(Error::Truncated)?,
            });
        }
        // M2TextureUnit (batch): 24 bytes — skinSectionIndex@4, colorIndex@8, materialIndex@0xa,
        // textureCount@0xe, textureComboIndex@0x10, textureCoordComboIndex@0x12 (the env-vs-UV
        // source), weightComboIndex@0x14 (the visibility-factor refs), textureTransformComboIndex@0x16
        // (UV animation).
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, 24, bat_avail));
        for i in 0..n_bat {
            let u = bytes.bytes_at(o_bat + i * 24, 24).ok_or(Error::Truncated)?;
            batches.push(SkinBatch {
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
