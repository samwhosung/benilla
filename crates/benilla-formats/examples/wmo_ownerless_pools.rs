//! Which WMO roots have embedded pools but no `WmoPortalInstance` to own them. A placement gets an
//! instance only with a portal graph or an authored `WMOAreaTable` identity, the spawn site's
//! `has_portals || m.wmo_id != 0` (`terrain_stream::spawn`), and a pool's `WmoGroupVis`/`WmoRoom`
//! scope is keyed to it. No shipped root is both portal-less and unnamed: the only `wmoID == 0`
//! root, `pvp_alterac_ent01.wmo`, has portals.
//! `cargo run -p benilla-formats --example wmo_ownerless_pools`
//!
//! A root has liquid when [`benilla_formats::wmo_group_liquid_mesh`] yields a surface for any of
//! its groups, the call `WmoModel::group_liquids` makes; a group flooded by `groupLiquid` alone
//! spawns no pool and is not counted. Placements come from every ADT's `MODF` and each WMO-only
//! map's WDT global WMO. Output is Blizzard data: never commit it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use benilla_formats::{
    parse_wmo_portals, parse_wmo_root, wmo_group_liquid_mesh, wmo_root_id, Chain,
};

/// What one root says about the two axes.
struct Root {
    /// `MOHD.wmoID` (`+0x20`), the `WMOAreaTable.WMOID` key; 0 means no authored identity.
    wmo_id: u32,
    /// `MOPT` and `MOPR` both non-empty, the spawn site's `has_portals`.
    has_portals: bool,
    /// Group files the root declares (`MOHD.nGroups`).
    groups: u32,
    /// Groups whose MLIQ resolves to a liquid surface.
    liquid_groups: u32,
    wet_tiles: u32,
    /// Maps the root is placed on; empty if never placed.
    maps: BTreeSet<String>,
}

impl Root {
    /// The spawn site's test: without an instance nothing owns the building's rooms.
    fn has_instance(&self) -> bool {
        self.has_portals || self.wmo_id != 0
    }
}

/// A chain path normalised as the MPQ hash compares it, so ADT `MWMO` and listfile names agree.
fn key(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Every placed WMO root and the maps it is on: ADT `MODF`s, then each WMO-only map's WDT global.
fn placed_roots(chain: &Chain) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut placed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // `World\Maps\<map>\<map>_XX_YY.adt`: the map name is the third path component.
    let map_of = |name: &str| key(name).split('\\').nth(2).unwrap_or("?").to_string();

    let names: Vec<String> = chain.list()?.into_iter().map(|e| e.name).collect();
    let adts: Vec<&String> = names.iter().filter(|n| key(n).ends_with(".adt")).collect();
    eprintln!("scanning {} ADTs for MODF placements…", adts.len());
    for name in adts {
        let Ok(bytes) = chain.read(name) else {
            continue;
        };
        let Ok(benilla_adt::ParsedAdt::Root(adt)) =
            benilla_adt::parse_adt(&mut Cursor::new(&*bytes))
        else {
            continue;
        };
        let map = map_of(name);
        for p in &adt.wmo_placements {
            let Some(model) = adt.wmos.get(p.name_id as usize) else {
                continue;
            };
            placed.entry(key(model)).or_default().insert(map.clone());
        }
    }

    let wdts: Vec<&String> = names.iter().filter(|n| key(n).ends_with(".wdt")).collect();
    eprintln!("scanning {} WDTs for WMO-only maps…", wdts.len());
    for name in wdts {
        let Ok(bytes) = chain.read(name) else {
            continue;
        };
        let Ok(wdt) =
            benilla_wdt::WdtReader::new(Cursor::new(&*bytes), benilla_wdt::WowVersion::Classic)
                .read()
        else {
            continue;
        };
        let Some(g) = wdt.global_wmo() else { continue };
        placed
            .entry(key(&g.model))
            .or_default()
            .insert(map_of(name));
    }
    Ok(placed)
}

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data()
        .ok_or_else(|| anyhow::anyhow!("no 1.12.1 install found (set $WOW_DATA)"))?;
    let chain = Chain::open(&data)?;
    let placed = placed_roots(&chain)?;
    eprintln!("{} distinct WMO roots placed in the world", placed.len());

    // Roots are the listed `.wmo` files whose stem lacks `_NNN`, plus every placed root the
    // listfile misses: a file absent from every listfile is readable but not enumerated.
    let listed: BTreeSet<String> = chain
        .list()?
        .into_iter()
        .map(|e| key(&e.name))
        .filter(|k| {
            let Some(stem) = k.strip_suffix(".wmo") else {
                return false;
            };
            !stem
                .rsplit('_')
                .next()
                .is_some_and(|t| t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    let unlisted: Vec<&String> = placed.keys().filter(|k| !listed.contains(*k)).collect();
    eprintln!(
        "reading {} listed WMO roots + {} placed-but-unlisted…",
        listed.len(),
        unlisted.len()
    );
    let roots: Vec<String> = listed
        .iter()
        .chain(unlisted)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut census: BTreeMap<String, Root> = BTreeMap::new();
    for k in &roots {
        let Ok(bytes) = chain.read(k) else {
            continue;
        };
        // `parse_wmo_root` gives the group count (`MOHD.nGroups`); a file it rejects has no rooms.
        let Ok(root) = parse_wmo_root(&bytes) else {
            continue;
        };
        let portals = parse_wmo_portals(&bytes);
        let stem = k.strip_suffix(".wmo").unwrap_or(k).to_string();
        let mut row = Root {
            wmo_id: wmo_root_id(&bytes),
            has_portals: !portals.refs.is_empty() && !portals.infos.is_empty(),
            groups: root.group_count(),
            liquid_groups: 0,
            wet_tiles: 0,
            maps: placed.get(k).cloned().unwrap_or_default(),
        };
        for gi in 0..row.groups {
            let Ok(gbytes) = chain.read(&format!("{stem}_{gi:03}.wmo")) else {
                continue;
            };
            let Some(mesh) = wmo_group_liquid_mesh(&gbytes) else {
                continue;
            };
            row.liquid_groups += 1;
            row.wet_tiles += mesh.wet.iter().filter(|w| **w).count() as u32;
        }
        census.insert(k.clone(), row);
    }

    // ---- the cross-tab ---------------------------------------------------------------------
    let mut cell = [[0u32; 2]; 2]; // [instance][liquid]
    for r in census.values() {
        cell[usize::from(r.has_instance())][usize::from(r.liquid_groups > 0)] += 1;
    }
    println!(
        "WMO roots in the 1.12.1 chain: {} parsed of {} found\n",
        census.len(),
        roots.len()
    );
    println!(
        "{:<34} {:>10} {:>11} {:>8}",
        "", "no liquid", "has liquid", "total"
    );
    for (i, label) in [
        "NO INSTANCE (portal-less + wmoID 0)",
        "instance (portals or wmoID)",
    ]
    .iter()
    .enumerate()
    {
        let row = cell[i]; // index 0 = no instance, matching `usize::from(has_instance())`
        println!(
            "{label:<34} {:>10} {:>11} {:>8}",
            row[0],
            row[1],
            row[0] + row[1]
        );
    }
    let (dry, wet) = (cell[0][0] + cell[1][0], cell[0][1] + cell[1][1]);
    println!("{:<34} {dry:>10} {wet:>11} {:>8}", "total", dry + wet);

    let wet_placed = census
        .values()
        .filter(|r| r.liquid_groups > 0 && !r.maps.is_empty())
        .count();
    let liquid_groups: u32 = census.values().map(|r| r.liquid_groups).sum();

    // Each axis alone: the cross-tab cannot say which half of `has_portals || wmo_id != 0` holds.
    let portal_less = census.values().filter(|r| !r.has_portals).count();
    let unnamed = census.values().filter(|r| r.wmo_id == 0).count();
    println!(
        "\naxes: {portal_less} roots have NO portal graph; {unnamed} roots have wmoID 0; \
{} are BOTH (portal-less AND unnamed)",
        cell[0][0] + cell[0][1]
    );
    println!(
        "      {wet} roots carry MLIQ liquid ({wet_placed} of them placed), \
{liquid_groups} liquid groups in all"
    );

    // Every `wmoID == 0` root by name: only these depend on their portal graph for an instance.
    println!("\nroots with wmoID 0:");
    for (path, r) in census.iter().filter(|(_, r)| r.wmo_id == 0) {
        println!(
            "  {path}  portals {}  liquid groups {}  {}",
            if r.has_portals { "YES" } else { "no" },
            r.liquid_groups,
            if r.maps.is_empty() {
                "-- never placed --".to_string()
            } else {
                r.maps.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        );
    }

    // ---- the affected set ------------------------------------------------------------------
    let affected: Vec<(&String, &Root)> = census
        .iter()
        .filter(|(_, r)| !r.has_instance() && r.liquid_groups > 0)
        .collect();
    let placed_count = affected.iter().filter(|(_, r)| !r.maps.is_empty()).count();
    println!(
        "\nownerless-with-liquid roots: {} ({placed_count} placed in the world)\n",
        affected.len()
    );
    if affected.is_empty() {
        println!("  (none — no shipped root falls in that cell)");
        return Ok(());
    }
    println!(
        "{:<52} {:>6} {:>7} {:>6}  placed on",
        "root", "groups", "wet grp", "cells"
    );
    for (path, r) in &affected {
        println!(
            "{:<52} {:>6} {:>7} {:>6}  {}",
            path,
            r.groups,
            r.liquid_groups,
            r.wet_tiles,
            if r.maps.is_empty() {
                "-- never placed --".to_string()
            } else {
                r.maps.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        );
    }
    Ok(())
}
