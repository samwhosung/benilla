//! **Which buildings' embedded pools have no visibility authority**, over the whole shipped corpus:
//! `cargo run -p benilla-formats --example wmo_ownerless_pools`.
//!
//! A placed WMO gets a `WmoPortalInstance` only when it has a portal graph **or** an authored
//! `WMOAreaTable` identity — `has_portals || m.wmo_id != 0`, the spawn site in
//! `benilla-world/src/terrain_stream/spawn/mod.rs`. Everything about a building's *rooms* hangs off
//! that instance: it is the per-placement entity the group-visible set lives on, and it is what a
//! pool's `WmoGroupVis`/`WmoRoom` scope is keyed to.
//!
//! So a root that has **neither** portals **nor** a wmoID is instance-less — and until decision 1652
//! that meant its embedded MLIQ pools were tagged with *neither* `WmoGroupVis` nor `ExteriorScene`,
//! i.e. no system ever wrote their `Visibility` at all, while the same placement's walls were
//! tagged `ExteriorScene` unconditionally and window-culled normally. A sealed well room drawing a
//! disc of water in mid-air is what that *would* look like from outside.
//!
//! Whether any shipped root actually falls in that cell is the question — an unreachable branch and
//! a widespread defect are the same code, and only the corpus tells them apart.
//!
//! **The answer is zero** (run 2026-08-27): of 815 roots, 448 carry no portal graph
//! and exactly **one** carries `wmoID == 0` — `pvp_alterac_ent01.wmo` — and that one *does* have
//! portals. The two conditions never coincide in shipped data, so the instance-less branch is dead
//! and 1652's unconditional tag on pools closes a hole no building reaches. That is worth a census
//! rather than a guess in either direction: it is the difference between a fix that earns a sighting
//! and one that must never be credited with one.
//!
//! Two facts per root, both read from the bytes the runtime reads:
//!
//! * **instance** — `MOPT` and `MOPR` both non-empty (`benilla_assets::WmoModel::portal_infos` /
//!   `portal_refs`), or `MOHD.wmoID @0x20 != 0` ([`benilla_formats::wmo_root_id`], the
//!   `WMOAreaTable.WMOID` key).
//! * **liquid** — any group file for which [`benilla_formats::wmo_group_liquid_mesh`] yields a
//!   surface. That is the *same* call `WmoModel::group_liquids` makes, so a root counted wet here is
//!   exactly a root that spawns pool entities at placement. (`resolve_shared_liquid_cells` can drop
//!   one group's mesh afterwards, but only in favour of a sibling group of the same root, so it
//!   cannot change a root's wet/dry verdict.) A group whose MOGP `groupLiquid` @`0x34` declares
//!   whole-group submersion but carries no `MLIQ` grid (13 in the archive) is **not**
//!   counted: it spawns no pool entity, so there is nothing for an instance to own. The liquid-group
//!   total cross-checks against `wmo_liquid_arms`, which reaches the same groups from the other side
//!   — the listfile's `_NNN` group files rather than `MOHD.nGroups` per root.
//!
//! Roots are also split by whether the world actually **places** them — an unplaced WMO can never be
//! stood next to, so it cannot show the defect. Placements come from every ADT's `MODF` plus the
//! WDT global-WMO of the 20 WMO-only maps (scanning only ADTs would call the Deeprun
//! Tram and every instance-shaped dungeon unplaced).
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use benilla_formats::{
    parse_wmo_portals, parse_wmo_root, wmo_group_liquid_mesh, wmo_root_id, Chain,
};

/// What one root says about the two axes.
struct Root {
    /// `MOHD.wmoID @0x20` — the `WMOAreaTable.WMOID` key; `0` ⇒ the building has no authored identity.
    wmo_id: u32,
    /// `MOPT` ∧ `MOPR` both non-empty — the spawn site's `has_portals`.
    has_portals: bool,
    /// Group files the root declares (`MOHD.nGroups`).
    groups: u32,
    /// Groups whose MLIQ resolves to a liquid surface.
    liquid_groups: u32,
    /// Wet cells across those surfaces — the size of what draws.
    wet_tiles: u32,
    /// Maps the root is placed on (empty ⇒ never placed).
    maps: BTreeSet<String>,
}

impl Root {
    /// The spawn site's test, verbatim: no instance ⇒ nothing owns this building's rooms.
    fn has_instance(&self) -> bool {
        self.has_portals || self.wmo_id != 0
    }
}

/// Normalize a chain path the way the MPQ hash compares them, so ADT `MWMO` names and listfile
/// names land on the same key.
fn key(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Every WMO root the world actually places, mapped to the maps it appears on: ADT `MODF` first,
/// then the WDT global-WMO that IS the world on a WMO-only map.
fn placed_roots(chain: &Chain) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut placed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // `World\Maps\<map>\<map>_XX_YY.adt` — the map name is the third path component.
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

    // A group file's stem ends `_NNN`; a root's does not. The chain lists both.
    //
    // The listfile is NOT a complete index (`Chain::list`'s own doc: a file absent from every
    // archive's listfile is readable by name but not enumerated), so the scanned set is the UNION of
    // the listed roots and every root an ADT/WDT actually references. A placed-but-unlisted root is
    // exactly the one this census must not miss, and the two counts printed below say whether the
    // union ever added anything.
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
        // `parse_wmo_root` is the group-count authority (`MOHD.nGroups`); a file that fails it is
        // not a root and has no rooms to own.
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

    // The two axes on their own. The cross-tab cannot say WHICH half of `has_portals || wmo_id != 0`
    // carries a root, and "no root is instance-less" is a claim that has to be falsifiable from the
    // same run rather than taken on the strength of one collapsed cell.
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

    // Every root on the rarer axis, named: `wmoID == 0` is what makes the instance depend on the
    // portal graph at all, so the handful that have it are the whole falsifiable surface of the
    // cross-tab's empty cell.
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
