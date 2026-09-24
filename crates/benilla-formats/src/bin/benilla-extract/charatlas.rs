//! `charatlas` — composite one character's body atlas off the chain and report **what painted
//! what**.
//!
//! The instrument the two "outfit texture" reports needed and nobody had. A dressed character's
//! body is one 256² atlas of ten fixed tiles (bbox table `0xb42450`), and every visible defect in
//! that class — a garment that stops early, a boot repainting a robe's hem, a bare band below the
//! knee — is one tile receiving the wrong contribution. Reading that off a screenshot means
//! guessing; reading it off the atlas means measuring.
//!
//! Three things it prints, all derived from the composite's own law
//! ([`benilla_formats::equip_blits`]), never a second transcription:
//!
//! 1. **The plan** — every equipment contribution in blit order, with the file each name actually
//!    resolved to (or `MISSING`, which is the silent skip that reads as "the texture ends early").
//! 2. **The per-tile diff** vs the same character composited naked, per atlas ROW. A tile whose
//!    lower rows go unpainted is a garment that stops early; a row count that changes when one slot
//!    is added is that slot's footprint.
//! 3. **The geosets** the same equipment selects, with the atlas rows each one samples — so "which
//!    tile does the robe's skirt read?" is answered next to "what is in that tile".
//!
//! The tile↔geoset pairing is the whole diagnosis: geoset 1302 (the robe skirt) samples atlas rows
//! 112–223, which straddles the LegUpper **and LegLower** tiles — so a boot's LegLower contribution
//! lands on a robe's hem even though the boot's own geometry is disabled under a robe.

use anyhow::{Context, Result};
use benilla_formats::{
    equip_blits, equip_tile, forearm_dressed, load_item_display_catalog, BlitSource, Chain,
    CharSections, CharacterGeosets, EmblemLayer, EquipGeosets, GuildEmblem, ItemDisplay,
};

/// The ten atlas tiles by group, for the per-tile report — the five head/left-column ones included,
/// so a head-section regression shows up in the same table as an equipment one.
const TILES: [(&str, u32, u32, u32, u32); 10] = [
    ("g0 ArmUpper", 0, 0, 128, 64),
    ("g1 ArmLower", 0, 64, 128, 64),
    ("g2 Hand", 0, 128, 128, 32),
    ("g8 HeadUpper", 0, 160, 128, 32),
    ("g9 HeadLower", 0, 192, 128, 64),
    ("g3 TorsoUpper", 128, 0, 128, 64),
    ("g4 TorsoLower", 128, 64, 128, 32),
    ("g5 LegUpper", 128, 96, 128, 64),
    ("g6 LegLower", 128, 160, 128, 64),
    ("g7 Foot", 128, 224, 128, 32),
];

/// The worn bodyslots the composite takes, in `equipment` order (bodyslot − 2).
const SLOT_NAMES: [&str; 8] = [
    "shirt", "chest", "belt", "pants", "boots", "wrist", "gloves", "tabard",
];

/// One character appearance + what it wears — the whole input, so a report is reproducible from the
/// command line that made it.
pub struct Look {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub facial_hair: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    /// The eight worn display ids in `equipment` order; `0` = the slot is empty.
    pub slots: [u32; 8],
    /// The wearer's guild tabard, or `None` for "no guild" — the same input the world composite
    /// takes (decision 1704). It only paints over a tabard whose display asks for it, so passing
    /// one without a guild-emblem tabard in the tabard slot is a no-op, on purpose.
    pub emblem: Option<GuildEmblem>,
}

/// A BLP's header as the composite cares about it: the dimensions it blits at and the alpha depth
/// that picks REPLACE (0) / 1-bit key (1) / blend (≥2) — the three that decide whether a
/// contribution *covers* the one under it or lets it through.
fn blp_shape(chain: &mut Chain, path: &str) -> Option<(u32, u32, u8)> {
    let b = chain.read_file(path).ok()?;
    if b.len() < 20 || &b[0..4] != b"BLP2" {
        return None;
    }
    let w = u32::from_le_bytes(b[12..16].try_into().ok()?);
    let h = u32::from_le_bytes(b[16..20].try_into().ok()?);
    Some((w, h, b[9]))
}

pub fn charatlas(chain: &mut Chain, look: &Look, out: Option<&std::path::Path>) -> Result<()> {
    let catalog = load_item_display_catalog(chain).context("loading ItemDisplayInfo")?;
    let sections = CharSections::load(chain).context("loading CharSections")?;
    let geosets = CharacterGeosets::load(chain).context("loading the customization tables")?;

    let worn: Vec<Option<&ItemDisplay>> = look
        .slots
        .iter()
        .map(|id| (*id != 0).then(|| catalog.get(*id)).flatten())
        .collect();
    let equipment: [Option<&ItemDisplay>; 8] = std::array::from_fn(|i| worn[i]);

    println!(
        "race {} sex {} skin {} face {} facialHair {} hair {}/{}",
        look.race,
        look.sex,
        look.skin,
        look.face,
        look.facial_hair,
        look.hair_style,
        look.hair_color
    );
    for (i, (id, d)) in look.slots.iter().zip(&equipment).enumerate() {
        match (id, d) {
            (0, _) => {}
            (id, None) => println!("  {:6} display {id}  NOT IN CATALOG", SLOT_NAMES[i]),
            (id, Some(d)) => println!(
                "  {:6} display {id}  geosetGroups {:?}",
                SLOT_NAMES[i], d.geoset_groups
            ),
        }
    }

    // (1) The plan — the composite's own order, with what each name resolved to. Worn garments and
    // the guild tabard's three layers come through the same list, because they land in the same
    // rows and the question ("what repainted this cell?") is the same one.
    println!("\nequipment blits (by ascending cell; later covers earlier within a tile):");
    for step in equip_blits(&equipment, look.emblem, false) {
        let (_x, y, w, h) = equip_tile(step.layer).expect("layer < 8");
        let candidates = step.candidates(look.sex);
        let basename = |p: &str| p.rsplit('\\').next().unwrap_or(p).to_string();
        let (who, name) = match step.source {
            BlitSource::Worn { slot, texture } => (SLOT_NAMES[slot], texture.to_string()),
            BlitSource::Emblem { part, .. } => (
                match part {
                    EmblemLayer::Background => "gBack",
                    EmblemLayer::Border => "gBrdr",
                    EmblemLayer::Symbol => "gEmbl",
                },
                candidates.first().map_or_else(String::new, |p| basename(p)),
            ),
        };
        let resolved = candidates
            .into_iter()
            .find_map(|p| blp_shape(chain, &p).map(|s| (p, s)));
        match resolved {
            Some((path, (bw, bh, alpha))) => {
                let cover = match alpha {
                    0 => "REPLACE",
                    1 => "1-bit key",
                    _ => "blend",
                };
                let fits = if (bw, bh) == (w, h) {
                    ""
                } else {
                    "  SIZE≠TILE"
                };
                println!(
                    "  g{} y{:>3}..{:<3} cell {} {:6} {:34} → {} ({}x{}, alpha {alpha} {cover}){fits}",
                    step.layer,
                    y,
                    y + h,
                    step.column,
                    who,
                    name,
                    basename(&path),
                    bw,
                    bh,
                );
            }
            None => println!(
                "  g{} y{:>3}..{:<3} cell {} {:6} {:34} → MISSING (region left as base skin)",
                step.layer,
                y,
                y + h,
                step.column,
                who,
                name,
            ),
        }
    }

    // (2) The per-tile diff against the same appearance composited naked.
    let naked = sections
        .composite_body(
            chain,
            look.race,
            look.sex,
            look.skin,
            look.face,
            look.facial_hair,
            look.hair_style,
            look.hair_color,
            [None; 8],
            None,
            false,
        )?
        .context("no base skin row for this appearance")?;
    let dressed = sections
        .composite_body(
            chain,
            look.race,
            look.sex,
            look.skin,
            look.face,
            look.facial_hair,
            look.hair_style,
            look.hair_color,
            equipment,
            look.emblem,
            false,
        )?
        .context("no base skin row for this appearance")?;

    let stride = dressed.width as usize;
    println!(
        "\natlas {}x{} ({} mips) — rows repainted vs naked, per tile:",
        dressed.width,
        dressed.height,
        dressed.mips.len()
    );
    for (name, x, y, tw, th) in TILES {
        let painted: Vec<u32> = (0..th)
            .map(|r| {
                (0..tw)
                    .filter(|c| {
                        let i = (((y + r) as usize) * stride + (x + c) as usize) * 4;
                        dressed.mips[0][i..i + 4] != naked.mips[0][i..i + 4]
                    })
                    .count() as u32
            })
            .collect();
        let rows = painted.iter().filter(|n| **n > 0).count();
        // Where a tile stops is the diagnosis, so name the run rather than dumping 64 numbers.
        let first = painted.iter().position(|n| *n > 0);
        let last = painted.iter().rposition(|n| *n > 0);
        let span = match (first, last) {
            (Some(f), Some(l)) => format!("rows {}..{} of {th}", f, l + 1),
            _ => "untouched".into(),
        };
        println!(
            "  {name:14} y{y:>3}..{:<3}  {rows:>3}/{th} repainted  {span}",
            y + th
        );
    }

    // (3) The geosets this same equipment selects. The atlas rows each samples are the model's
    // (`benilla-extract m2batch <the race model>` prints the UV extents these come from), so the
    // two halves of "which art lands on which geometry" sit in one report.
    let mut eq = EquipGeosets::default();
    for (i, d) in equipment.iter().enumerate() {
        if let Some(d) = d {
            eq.bodyslots[i] = Some(d.geoset_groups);
        }
    }
    // B3's gate is the ArmLower tile's occupancy — read off the same plan printed above (1864).
    eq.forearm_dressed = forearm_dressed(&equipment);
    // Sorted + deduplicated at the source (`visible_geosets`).
    let ids = geosets.visible_geosets(look.race, look.sex, look.hair_style, look.facial_hair, &eq);
    println!("\nvisible geosets: {ids:?}");

    if let Some(path) = out {
        let img =
            image::RgbaImage::from_raw(dressed.width, dressed.height, dressed.mips[0].clone())
                .context("atlas mip 0 is not w*h*4 bytes")?;
        img.save(path)
            .with_context(|| format!("writing {}", path.display()))?;
        println!("\nwrote {}", path.display());
    }
    Ok(())
}
