//! Reading the client's own TTFs, and the one metric that has to come off the raw bytes.

use cosmic_text::FontSystem;

/// The four client TTFs (verified present in `fonts.MPQ`, plain TTF — no exotic wrapper), read
/// through the app's own patch chain rather than `std::fs` (there is no `std::fs` path to client
/// data — see [`WorldAssets`]). Index 0 (Friz Quadrata) is the fallback face and is required.
pub(super) const CLIENT_FONTS: &[&str] = &[
    "Fonts\\FRIZQT__.TTF",
    "Fonts\\ARIALN.TTF",
    "Fonts\\MORPHEUS.TTF",
    "Fonts\\SKURRI.TTF",
];

/// The face's baseline ascender as a fraction of the em — `hhea.asc / (hhea.asc + |hhea.desc|)` —
/// straight from the raw sfnt bytes. This is the term in the client's glyph *placement* law: the
/// ink hangs from the pixel ascender `[CGxFont+0x17c] = round(em · asc/(asc+|desc|))`, threaded
/// unchanged into the glyph placement kernel (0x5d1360) as the operand that fixes
/// `baseline = cellTop + ascender` (the call chain `0x5ca160 → 0x5d1120 [ebp+0xc] → 0x5d1360
/// [ebp+0x10]`, where the cell's own `[[FT_Face+0x54]+0x68]` is the per-glyph `bitmap_top`, the
/// *subordinate* operand). This is the `ComputeRasterMetrics`
/// `load_param`; the FreeType scaled hhea ascender (`asc/upem` ≈ 0.965) appears NOWHERE in the
/// placement path — seating with it drops every line ~3px too low for Friz (965/1215 ≈ 0.794 vs
/// 0.965 → baseline row 10 vs 13 in a 13-tall cell). A tiny table-directory walk (`hhea` →
/// ascender@+4, descender@+6); `None` on any malformed/missing table.
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
    // The denominator is narrowed to f32 exactly as the binary does (`fstp m32` @0x5ca0d?),
    // then the ratio; `em · ratio + 0.5` floors to the load_param at the call site below.
    let denom = asc + desc.abs();
    (denom > 0.0 && asc > 0.0).then_some(asc / denom)
}

/// What a registered face answers to when a caller has to name it to the shaper: its id, the
/// family string, and the three CSS axes `cosmic-text` matches on.
///
/// **The axes are not decoration**. `Attrs::matches` in cosmic-text 0.16 filters
/// the candidate set on `style` and `stretch`, and `fontdb::Database::query` then does CSS
/// matching over `weight` as well — so an `Attrs::new().family(…)` built with the *defaults*
/// asks for a NORMAL-weight, NORMAL-style face and quietly gets somebody else when the face it
/// named is bold or italic. Reading them back off the face means the attrs describe the face we
/// actually loaded rather than the one we assumed.
pub(super) struct Registered {
    pub(super) id: fontdb::ID,
    pub(super) family: String,
    pub(super) weight: fontdb::Weight,
    pub(super) style: fontdb::Style,
    pub(super) stretch: fontdb::Stretch,
}

/// Registers `bytes` (raw TTF) into `font_system`'s `fontdb`, returning the face's id and the
/// four things a caller needs to name it back to the shaper — all read off the just-loaded face,
/// with no reliance on hardcoding Blizzard's font-name strings.
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
