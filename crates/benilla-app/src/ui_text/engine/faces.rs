//! Reading the client's own TTFs, and the one metric that has to come off the raw bytes.

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
