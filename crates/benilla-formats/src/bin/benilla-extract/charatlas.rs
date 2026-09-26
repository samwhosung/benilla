//! `charatlas`: composite one character's body atlas off the chain and report what painted what:
//! the equipment blits in [`benilla_formats::equip_blits`] order with the file each resolved to
//! (`MISSING` leaves base skin), the rows each tile repaints against the naked body, and the
//! geosets the equipment selects. Tiles scale with the skin atlas resolution. Geoset 1302, the
//! robe skirt, samples normalized atlas rows 112-223 across the LegUpper and LegLower tiles, so a
//! boot's LegLower blit shows on a robe's hem.

use anyhow::{Context, Result};
use benilla_formats::{
    equip_blits, equip_tile, forearm_dressed, load_item_display_catalog, scale_body_tile,
    BlitSource, Chain, CharSections, CharacterGeosets, EmblemLayer, EquipGeosets, GuildEmblem,
    ItemDisplay,
};

/// The body's ten fixed tiles in its 256² atlas (the reference's bbox table `0xb42450`).
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

/// The worn bodyslots the composite takes, in `equipment` order (bodyslot - 2).
const SLOT_NAMES: [&str; 8] = [
    "shirt", "chest", "belt", "pants", "boots", "wrist", "gloves", "tabard",
];

/// One appearance and what it wears: the whole input, so its command line reproduces a report.
pub struct Look {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub facial_hair: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    /// The eight worn display ids in `equipment` order; 0 is an empty slot.
    pub slots: [u32; 8],
    /// The guild tabard; it paints only over a tabard display that asks for an emblem.
    pub emblem: Option<GuildEmblem>,
}

/// A BLP2's size and alpha depth, which picks the blit: replace (0), 1-bit key (1) or blend (2+).
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

    let base_path = sections
        .skin_texture(look.race, look.sex, look.skin)
        .context("no base skin row for this appearance")?;
    let (atlas_width, atlas_height, _) =
        blp_shape(chain, base_path).context("reading base skin dimensions")?;

    // (1) The plan, in the composite's order; worn garments and the three emblem layers share it.
    println!("\nequipment blits (by ascending cell; later covers earlier within a tile):");
    for step in equip_blits(&equipment, look.emblem, false) {
        let (_x, y, w, h) = scale_body_tile(
            equip_tile(step.layer).expect("layer < 8"),
            atlas_width,
            atlas_height,
        );
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
                    "  RESAMPLED"
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
        let (x, y, tw, th) = scale_body_tile((x, y, tw, th), dressed.width, dressed.height);
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

    // (3) The geosets this equipment selects; `m2batch` on the race model prints their UV extents.
    let mut eq = EquipGeosets::default();
    for (i, d) in equipment.iter().enumerate() {
        if let Some(d) = d {
            eq.bodyslots[i] = Some(d.geoset_groups);
        }
    }
    // The shirt-cuff geoset's gate is the ArmLower tile's occupancy, read off the same plan.
    eq.forearm_dressed = forearm_dressed(&equipment);
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
