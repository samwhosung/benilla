//! `thudcensus`: the death thud a corpse makes as it lands (`$DTH`, `0x6236e0`): the full
//! `DeathThudLookups.dbc` grid, `SizeClass × TerrainTypeSoundID` to land and water `SoundEntries`
//! kits, and the creature models that key a `$DTH`. An empty cell is authored silence (Sand and
//! Soggy have no water column), as is a model with no `$DTH`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use benilla_formats::Chain;

/// A model path's comparison key: DBCs name models `.mdx`, the archives hold `.m2`.
fn mdx_key(path: &str) -> String {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    match p.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => p,
    }
}

/// The `SizeClass` names the kits carry, `DeathThudSmallDirt` to `DeathThudColossalWood`.
const SIZE_NAMES: [&str; 5] = ["Small", "Medium", "Large", "Giant", "Colossal"];

fn size_name(class: i32) -> String {
    match usize::try_from(class).ok().and_then(|c| SIZE_NAMES.get(c)) {
        Some(n) => format!("{class} {n}"),
        None => format!("{class} ??"),
    }
}

pub fn thudcensus(chain: &mut Chain) -> Result<()> {
    let thuds = benilla_formats::load_death_thud_catalog(chain)?;
    let steps = benilla_formats::load_footstep_catalog(chain)?;
    let kits = benilla_formats::load_sound_kit_catalog(chain)?;
    let creatures = benilla_formats::load_creature_catalog(chain)?;

    // ── 1 · the table ────────────────────────────────────────────────────────────────────────
    // The terrain axis with the TerrainType rows naming each id; an unnamed sound is unreachable.
    let mut terrain_names: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for terrain in 0..64 {
        if let Some(sound) = steps.sound_class_of(terrain) {
            terrain_names.entry(sound).or_default().push(terrain);
        }
    }
    let axis: Vec<u32> = thuds.terrain_sounds().collect();
    let classes: BTreeSet<u32> = thuds.size_classes();
    let classes_all = classes.clone();

    println!(
        "DeathThudLookups.dbc — {} rows over {} size class(es) × {} TerrainTypeSounds id(s)\n",
        thuds.len(),
        classes.len(),
        axis.len()
    );
    let mut dangling: Vec<String> = Vec::new();
    let name_of = |id: u32, dangling: &mut Vec<String>, what: &str| -> String {
        if id == 0 {
            return "—".to_string();
        }
        match kits.get(id) {
            Some(k) => format!("{id} {}", k.name),
            None => {
                dangling.push(format!("{what}: SoundEntries {id} does not exist"));
                format!("{id} DANGLING")
            }
        }
    };
    for &sound in &axis {
        let reached = match terrain_names.get(&sound) {
            None => "  ** no TerrainType names it **".to_string(),
            Some(t) => format!(
                "  TerrainType {}",
                t.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            ),
        };
        println!("terrain sound {sound}{reached}");
        for &class in &classes {
            let cell = match thuds.resolve(class, sound) {
                None => "—  NO ROW".to_string(),
                Some((land, water)) => format!(
                    "{:<28}  water {}",
                    name_of(land, &mut dangling, &format!("({class}, {sound}) land")),
                    name_of(water, &mut dangling, &format!("({class}, {sound}) water")),
                ),
            };
            println!("    {:<12}  {cell}", size_name(class as i32));
        }
    }

    // ── 2 · the population ───────────────────────────────────────────────────────────────────
    // 2971 displays override their model's size class, so each model prints the whole set.
    let mut resolved: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (display, model) in creatures.display_models() {
        if let Some(class) = creatures.size_class(display) {
            resolved.entry(model).or_default().insert(class);
        }
    }
    let mut by_key: BTreeMap<String, Vec<(u32, i32)>> = BTreeMap::new();
    let mut off_axis = Vec::new();
    for (id, path, class) in creatures.sized_models() {
        if !(0..=4).contains(&class) {
            off_axis.push(format!("{path} ({id}): SizeClass = {class}"));
        }
        by_key.entry(mdx_key(path)).or_default().push((id, class));
    }

    println!("\n$DTH animation events — which creature models thud at all\n");
    let mut keyed = 0u32;
    let mut marks = 0u32;
    let mut silent: Vec<String> = Vec::new();
    let mut keyed_classes: BTreeMap<u32, u32> = BTreeMap::new();
    for name in crate::scan::m2_names(chain, None)? {
        let key = mdx_key(&name);
        let Some(models) = by_key.get(&key) else {
            continue; // not a creature model: `CGUnit_C::HandleAnimEvent` never sees its events
        };
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let mut rows = Vec::new();
        for a in benilla_formats::parse_m2_animations(&bytes) {
            for e in a.events.iter().filter(|e| &e.ident == b"$DTH") {
                marks += 1;
                rows.push(format!(
                    "    seq {:>2} anim {:>3} {:>7.3}s",
                    a.seq_index, a.anim_id, e.time
                ));
            }
        }
        let sizes: BTreeSet<u32> = models
            .iter()
            .flat_map(|(id, _)| resolved.get(id).into_iter().flatten().copied())
            .collect();
        let label = format!(
            "{name}   [{}]",
            if sizes.is_empty() {
                "no display reaches it".to_string()
            } else {
                sizes
                    .iter()
                    .map(|c| size_name(*c as i32))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        );
        if rows.is_empty() {
            silent.push(label);
            continue;
        }
        keyed += 1;
        for c in &sizes {
            *keyed_classes.entry(*c).or_default() += 1;
        }
        println!("  {label}");
        for r in rows {
            println!("{r}");
        }
    }

    println!(
        "\n{keyed} creature model(s) key a $DTH ({marks} marker(s)); {} key none",
        silent.len()
    );
    for c in classes {
        println!(
            "  {:<12}  {} keyed model(s)",
            size_name(c as i32),
            keyed_classes.get(&c).copied().unwrap_or(0)
        );
    }
    if !silent.is_empty() {
        println!("\nkeying no $DTH — silent in the reference too:");
        for s in &silent {
            println!("  {s}");
        }
    }
    for d in &dangling {
        println!("  DANGLING {d}");
    }
    for o in &off_axis {
        println!("  OFF-AXIS {o}  (the reference's unsigned `>= 5` gate silences this model)");
    }
    println!(
        "\n{} dangling sound id(s); {} model(s) off the 0..=4 size axis",
        dangling.len(),
        off_axis.len()
    );

    // ── 3 · where a corpse is silent ─────────────────────────────────────────────────────────
    // Both lookups key on `TerrainType.SoundID`, and TerrainType 10 "None", the default of a WMO
    // surface whose `MOMT+0x20` is unset, has `SoundID` 0. `FootstepTerrainLookup` has rows there
    // for 17 footstep classes and `DeathThudLookups` none, so a corpse on such a floor is silent.
    let ftl_at_zero: Vec<u32> = (0..256)
        .filter(|c| steps.resolve_terrain(*c, 10).is_some())
        .collect();
    let dtl_at_zero = classes_all
        .iter()
        .filter(|c| thuds.resolve(**c, 0).is_some())
        .count();
    println!(
        "\nSilence indoors — TerrainType 10 \"None\" (SoundID 0): {} footstep class(es) have a \
         lookup row, {dtl_at_zero} size class(es) have a thud row",
        ftl_at_zero.len()
    );

    let mut wmo_roots = 0u32;
    let mut materials = 0u32;
    let mut by_terrain: BTreeMap<u32, u32> = BTreeMap::new();
    let mut silent_roots = 0u32;
    let mut thudding: Vec<String> = Vec::new();
    for path in crate::scan::wmo_roots(chain, None)? {
        let Ok(bytes) = chain.read_file(&path) else {
            continue;
        };
        let Ok(root) = benilla_formats::parse_wmo_root(&bytes) else {
            continue;
        };
        wmo_roots += 1;
        let mut any_thud = false;
        for ground_type in root.material_ground_types() {
            materials += 1;
            *by_terrain.entry(ground_type).or_default() += 1;
            // Size class 0 stands for all: every class 0..=4 has rows on the same terrain axis.
            if steps
                .sound_class_of(ground_type)
                .is_some_and(|ts| thuds.resolve(0, ts).is_some())
            {
                any_thud = true;
            }
        }
        if any_thud {
            thudding.push(path);
        } else {
            silent_roots += 1;
        }
    }
    println!(
        "\n{wmo_roots} WMO root(s), {materials} MOMT material(s) — the `MOMT+0x20` TerrainType each \
         surface carries\n"
    );
    for (terrain, n) in &by_terrain {
        let sound = steps.sound_class_of(*terrain);
        let verdict = match sound {
            Some(0) | None => "SILENT — no thud row".to_string(),
            Some(ts) => match thuds.resolve(0, ts) {
                Some(_) => format!("thuds (terrain sound {ts})"),
                None => format!("SILENT — no thud row (terrain sound {ts})"),
            },
        };
        println!(
            "  TerrainType {:>3}  {:>6} material(s)  {:>5.1}%  {verdict}",
            terrain,
            n,
            100.0 * f64::from(*n) / f64::from(materials.max(1)),
        );
    }
    println!(
        "\n{silent_roots} of {wmo_roots} WMO roots have NO surface that can thud; {} do",
        thudding.len()
    );
    // Untruncated: the list answers whether a given building thuds.
    for p in &thudding {
        println!("  thuds somewhere: {p}");
    }
    Ok(())
}
