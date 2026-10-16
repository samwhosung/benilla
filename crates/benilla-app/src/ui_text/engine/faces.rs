//! Reading the client's own TTFs, and what has to come off the raw bytes: one metric and the
//! charmap.

use cosmic_text::skrifa::raw::{tables::cmap::PlatformId, FontRef, TableProvider};
use cosmic_text::FontSystem;

/// The four client TTFs, plain TTF in `fonts.MPQ`, read through the patch chain. Index 0, Friz
/// Quadrata, is the fallback face and is required.
pub(super) const CLIENT_FONTS: &[&str] = &[
    "Fonts\\FRIZQT__.TTF",
    "Fonts\\ARIALN.TTF",
    "Fonts\\MORPHEUS.TTF",
    "Fonts\\SKURRI.TTF",
];

/// The face's ascender as a fraction of the em, `hhea.asc / (hhea.asc + |hhea.desc|)`, off the raw
/// sfnt bytes. The reference puts the baseline at `cellTop + round(em · ratio)` (`[CGxFont+0x17c]`,
/// set at `0x5ca030`, reaching the placement kernel `0x5d1360` through `0x5ca160` and `0x5d1120`);
/// FreeType's scaled ascender (`asc/upem`: 0.965 for Friz, where this ratio is 0.794) is not on
/// that path.
pub(super) fn hhea_ascent_ratio(bytes: &[u8]) -> Option<f32> {
    let num = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
    let (mut asc, mut desc) = (None, None);
    for i in 0..num {
        let rec = bytes.get(12 + 16 * i..12 + 16 * i + 16)?;
        let toff = u32::from_be_bytes(rec[8..12].try_into().ok()?) as usize;
        if &rec[0..4] == b"hhea" {
            asc = Some(i16::from_be_bytes(bytes.get(toff + 4..toff + 6)?.try_into().ok()?) as f32);
            desc = Some(i16::from_be_bytes(bytes.get(toff + 6..toff + 8)?.try_into().ok()?) as f32);
        }
    }
    let (asc, desc) = (asc?, desc?);
    // The denominator is narrowed to f32 as the reference does (`fstp m32` at `0x5ca0be`); the
    // callers floor `em · ratio + 0.5`.
    let denom = asc + desc.abs();
    (denom > 0.0 && asc > 0.0).then_some(asc / denom)
}

/// The index of the cmap encoding record the reference reads characters through. The face open
/// selects the Unicode charmap (`FT_Select_Charmap(face, 'unic')`, `0x5d015d` → `0x7ce8d0`): the
/// first record whose platform and encoding FreeType's table (`0x7d8e00`, rows `0x81eb70` to
/// `0x81ebe8`) names Unicode, platform 2 or 0 with any encoding or platform 3 encoding 1; that
/// table has no platform 3 encoding 10. `None` fails the open (`0x5d0169`).
pub(super) fn unicode_charmap(bytes: &[u8]) -> Option<u16> {
    let cmap = FontRef::from_index(bytes, 0).ok()?.cmap().ok()?;
    let i = cmap.encoding_records().iter().position(|r| {
        matches!(
            (r.platform_id(), r.encoding_id()),
            (PlatformId::ISO | PlatformId::Unicode, _) | (PlatformId::Windows, 1)
        )
    })?;
    u16::try_from(i).ok()
}

/// The glyph `ch` maps to through encoding record `record`, `FT_Get_Char_Index` (`0x7ce960`):
/// `None` for a code point the charmap lacks, which FreeType answers with glyph 0.
pub(super) fn charmap_glyph(bytes: &[u8], index: u32, record: u16, ch: char) -> Option<u16> {
    let gid = FontRef::from_index(bytes, index)
        .ok()?
        .cmap()
        .ok()?
        .subtable(record)
        .ok()?
        .map_codepoint(ch)?;
    u16::try_from(gid.to_u32()).ok().filter(|&g| g != 0)
}

/// What names a registered face to the shaper: its id, family and three CSS axes, read off the
/// face. `cosmic-text` 0.16 matches on all three, so attrs naming only the family ask for a
/// normal-weight, normal-style face and silently get another one when this face is bold or italic.
pub(super) struct Registered {
    pub(super) id: fontdb::ID,
    pub(super) family: String,
    pub(super) weight: fontdb::Weight,
    pub(super) style: fontdb::Style,
    pub(super) stretch: fontdb::Stretch,
}

/// Register a raw TTF into `font_system`'s database, named by what the loaded face says.
pub(super) fn register_font(
    font_system: &mut FontSystem,
    bytes: Vec<u8>,
) -> anyhow::Result<Registered> {
    let source = fontdb::Source::Binary(
        std::sync::Arc::new(bytes) as std::sync::Arc<dyn AsRef<[u8]> + Sync + Send>
    );
    let ids = font_system.db_mut().load_font_source(source);
    let id = *ids
        .first()
        .ok_or_else(|| anyhow::anyhow!("font source produced no faces (not a valid TTF?)"))?;
    let info = font_system
        .db_mut()
        .face(id)
        .ok_or_else(|| anyhow::anyhow!("face {id:?} vanished right after loading"))?;
    let family = info
        .families
        .first()
        .map(|(name, _)| name.clone())
        .ok_or_else(|| anyhow::anyhow!("face {id:?} carries no family name"))?;
    Ok(Registered {
        id,
        family,
        weight: info.weight,
        style: info.style,
        stretch: info.stretch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-table sfnt whose `cmap` holds `records` in order, each `(platform, encoding)` over
    /// its own format 12 subtable mapping `ch` to `glyph`.
    fn sfnt_with_cmap(records: &[(u16, u16, char, u32)]) -> Vec<u8> {
        let mut cmap = Vec::new();
        cmap.extend(0u16.to_be_bytes());
        cmap.extend((records.len() as u16).to_be_bytes());
        let first = 4 + 8 * records.len();
        for (i, &(pid, eid, ..)) in records.iter().enumerate() {
            cmap.extend(pid.to_be_bytes());
            cmap.extend(eid.to_be_bytes());
            cmap.extend(((first + 28 * i) as u32).to_be_bytes());
        }
        for &(.., ch, glyph) in records {
            for half in [12u16, 0] {
                cmap.extend(half.to_be_bytes());
            }
            for word in [28u32, 0, 1, ch as u32, ch as u32, glyph] {
                cmap.extend(word.to_be_bytes());
            }
        }
        let mut font = Vec::new();
        for half in [1u16, 0, 1, 16, 0, 0] {
            font.extend(half.to_be_bytes());
        }
        font.extend(b"cmap");
        for word in [0u32, 28, cmap.len() as u32] {
            font.extend(word.to_be_bytes());
        }
        font.extend(cmap);
        font
    }

    /// FreeType's table names platform 3 encoding 1 Unicode and not encoding 10 (`0x81eb70`), and
    /// the open takes the first Unicode record, whatever the later ones map.
    #[test]
    fn the_charmap_is_the_first_record_freetype_calls_unicode() {
        let font = sfnt_with_cmap(&[
            (3, 0, 'A', 4),
            (3, 10, 'A', 5),
            (3, 1, 'B', 6),
            (0, 3, 'A', 7),
        ]);
        assert_eq!(unicode_charmap(&font), Some(2));
        assert_eq!(charmap_glyph(&font, 0, 2, 'B'), Some(6));
        assert_eq!(
            charmap_glyph(&font, 0, 2, 'A'),
            None,
            "a later record does not answer"
        );
        let iso = sfnt_with_cmap(&[(1, 0, 'A', 4), (2, 1, 'A', 5)]);
        assert_eq!(unicode_charmap(&iso), Some(1));
        let symbol = sfnt_with_cmap(&[(3, 0, 'A', 4), (3, 10, 'A', 5)]);
        assert_eq!(
            unicode_charmap(&symbol),
            None,
            "no Unicode record: the face does not open"
        );
    }
}
