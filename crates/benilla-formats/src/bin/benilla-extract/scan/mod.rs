//! Corpus-scan reports: sweeps over many models or placements that measure how big a class is
//! and where it lives, grouped by the question they answer rather than the format they read.

use anyhow::{Context, Result};
use benilla_formats::Chain;

mod geometry;
mod lighting;
mod material;
mod particles;
mod sequence;
mod skeleton;
mod world;

pub use geometry::{animboundscan, bbfacescan, bbscan, geosetscan, groundscan, normalscan};
pub use lighting::{darkpropscan, m2lightscan, shadeat};
pub use material::{
    alphascan, blendscan, entityuvscan, envmapscan, fxuvscan, texmodescan, uvslotscan, uvwrapscan,
};
pub use particles::{
    cellscan, fxordercensus, partcensus, partscan, partslotscan, ribbonscan, shardcensus,
};
pub use sequence::{
    fxlifescan, goanimscan, goslotscan, idleslotscan, seqclockscan, soundeventscan,
};
pub use skeleton::{attachscan, bonescan, eventmarkerscan};
pub use world::{doodadscan, placescan, skyboxscan, wmodoodads};

/// Every `.m2` in the chain in listfile order and casing, narrowed to a path `prefix` matched
/// case-insensitively with either slash.
pub(crate) fn m2_names(chain: &mut Chain, prefix: Option<&str>) -> Result<Vec<String>> {
    let pfx = prefix.map(|p| p.to_ascii_lowercase().replace('/', "\\"));
    Ok(chain
        .list()
        .context("listing chain contents")?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.ends_with(".m2") && pfx.as_deref().is_none_or(|p| l.starts_with(p))
        })
        .collect())
}

/// Every WMO root in the chain, narrowed like [`m2_names`]. Only a root carries the
/// MOHD/MODS/MODD/MOGI/MOLT/MOSB tables; a group file is `<stem>_NNN.wmo`, exactly three digits.
pub(crate) fn wmo_roots(chain: &mut Chain, prefix: Option<&str>) -> Result<Vec<String>> {
    let pfx = prefix.map(|p| p.to_ascii_lowercase().replace('/', "\\"));
    Ok(chain
        .list()
        .context("listing chain contents")?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            let Some(stem) = l.strip_suffix(".wmo") else {
                return false;
            };
            let group = stem.len() >= 4
                && stem.as_bytes()[stem.len() - 4] == b'_'
                && stem[stem.len() - 3..].bytes().all(|b| b.is_ascii_digit());
            !group && pfx.as_deref().is_none_or(|p| l.starts_with(p))
        })
        .collect())
}

/// Title-case an `Item\ObjectComponents\<sub>` component so its listfile casings share one key.
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
        }
        None => String::new(),
    }
}

/// The content family of an M2 path, a sweep's summary dimension: `World\` is placed doodads and
/// WMO props, `World\Goober\` GameObject displays.
pub(super) fn family_of(name: &str) -> String {
    let comps: Vec<&str> = name.split('\\').collect();
    let low = |s: &str| s.to_ascii_lowercase();
    match comps.first().map(|s| low(s)).as_deref() {
        Some("creature") => "Creature\\".to_string(),
        Some("character") => "Character\\".to_string(),
        Some("spells") => "Spells\\".to_string(),
        Some("item") if comps.get(1).map(|s| low(s)).as_deref() == Some("objectcomponents") => {
            match comps.get(2) {
                Some(sub) => format!("Item\\ObjectComponents\\{}\\", title_case(sub)),
                None => "Item\\ObjectComponents\\".to_string(),
            }
        }
        Some("world") if comps.get(1).map(|s| low(s)).as_deref() == Some("goober") => {
            "World\\Goober\\".to_string()
        }
        Some("world") => "World\\".to_string(),
        _ => "other".to_string(),
    }
}
