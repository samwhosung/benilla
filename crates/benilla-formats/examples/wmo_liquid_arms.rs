//! Which WMO water arm each shipped liquid group takes. The reference's dispatch `0x6b62e0` splits
//! water (nibbles 0/4/8) on the owning group's `MOGP.flags & 0x48`: set runs the exterior kernel
//! `0x6b6630`, a 9-float vertex with an up normal, binding `MapObjExtWater0.bls` at `0x6b6654`;
//! clear runs the interior kernel `0x6b6420`, a 6-float vertex with no normal, coloured from the
//! map object's lights (`CMapObj+0x1d8`). Magma and slime (nibbles 2/3/6/7) take `0x6b68f0`
//! whatever the flags, and neither arm is the ADT path (`0x6851b0`, `0x685010`, `ocean0_s.bls`).
//! Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example wmo_liquid_arms`

use std::collections::BTreeMap;

use benilla_formats::{wmo_group_header, wmo_group_liquid_mesh, Chain};

/// The reference's interior test, zero meaning interior (`0x6b3f90`; the liquid dispatch
/// `0x6b62e0` tests `[owner+0x10] & 0x48`).
const EXTERIOR_MASK: u32 = 0x48;

#[derive(Default, Clone, Copy)]
struct Tally {
    groups: u32,
    wet_tiles: u32,
}

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data()
        .ok_or_else(|| anyhow::anyhow!("no 1.12.1 install found (set $WOW_DATA)"))?;
    let reader = Chain::open(&data)?;

    let mut by_arm: BTreeMap<(bool, String), Tally> = BTreeMap::new();
    // Per building: (interior, exterior) water groups.
    let mut buildings: BTreeMap<String, (u32, u32)> = BTreeMap::new();

    for entry in reader.list()? {
        let name = entry.name;
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".wmo") {
            continue;
        }
        // A group file's stem ends `_NNN`; a root's does not.
        let stem = lower.trim_end_matches(".wmo");
        let is_group = stem
            .rsplit('_')
            .next()
            .is_some_and(|t| t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()));
        if !is_group {
            continue;
        }
        let Ok(bytes) = reader.read(&name) else {
            continue;
        };
        let Some(mesh) = wmo_group_liquid_mesh(&bytes) else {
            continue;
        };
        let Some(header) = wmo_group_header(&bytes) else {
            continue;
        };
        let interior = header.flags & EXTERIOR_MASK == 0;
        let wet = mesh.wet.iter().filter(|w| **w).count() as u32;

        let t = by_arm
            .entry((interior, format!("{:?}", mesh.kind)))
            .or_default();
        t.groups += 1;
        t.wet_tiles += wet;

        let root = stem
            .rsplit_once('_')
            .map_or(stem, |(r, _)| r)
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(stem)
            .to_string();
        if !mesh.kind.is_fullbright() {
            let e = buildings.entry(root).or_default();
            if interior {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }

    println!("WMO liquid groups by ARM and kind (interior = MOGP.flags & 0x48 == 0)\n");
    println!(
        "{:<10} {:<8} {:>8} {:>11}  kernel",
        "arm", "kind", "groups", "wet tiles"
    );
    let (mut int_g, mut ext_g, mut int_t, mut ext_t) = (0, 0, 0, 0);
    for ((interior, kind), t) in &by_arm {
        // Only category 0 splits on the flag; the fullbright kinds share `0x6b68f0` either way.
        let water = !matches!(kind.as_str(), "Magma" | "Slime");
        println!(
            "{:<10} {:<8} {:>8} {:>11}  {}",
            if *interior { "INTERIOR" } else { "EXTERIOR" },
            kind,
            t.groups,
            t.wet_tiles,
            match (water, interior) {
                (true, true) => "0x6b6420  (interior water)",
                (true, false) => "0x6b6630  (exterior water, MapObjExtWater0.bls)",
                (false, _) => "0x6b68f0  (magma/slime — arm-blind)",
            }
        );
        if !water {
            continue; // the totals below are the water arms; fullbright takes neither
        }
        if *interior {
            int_g += t.groups;
            int_t += t.wet_tiles;
        } else {
            ext_g += t.groups;
            ext_t += t.wet_tiles;
        }
    }
    println!(
        "\nWATER arms — INTERIOR {int_g} groups / {int_t} wet tiles   EXTERIOR {ext_g} groups / {ext_t} wet tiles"
    );

    // Buildings with water on both arms; Undercity's liquid is slime, which takes neither.
    let mixed: Vec<_> = buildings
        .iter()
        .filter(|(_, (i, e))| *i > 0 && *e > 0)
        .collect();
    println!("\nbuildings carrying WATER on BOTH arms: {}", mixed.len());
    for (root, (i, e)) in mixed.iter().take(20) {
        println!("  {root:<40} interior {i:>3}  exterior {e:>3}");
    }

    for site in ["stormwind", "blackfathom_instance"] {
        if let Some((i, e)) = buildings.get(site) {
            println!("\n{site} water groups: interior {i}, exterior {e}");
        }
    }
    Ok(())
}
