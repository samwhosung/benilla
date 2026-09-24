//! `shakecensus`: the 24 shipped `CameraShakes.dbc` presets and all that names one: creature
//! models (`CreatureModelData` fields 11 and 12, footstep and death thud), the 9
//! `SpellEffectCameraShakes.dbc` groups that `SpellVisualKit` field 14 names, and `$SHK` animation
//! events. Every named id must land on a real row, or the column map is wrong.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use benilla_formats::Chain;

/// Which creature models name a preset, and through which column.
#[derive(Default)]
struct Users {
    footstep: Vec<String>,
    death_thud: Vec<String>,
}

/// A model path's comparison key: DBCs name models `.mdx`, the archives hold `.m2`, in any case.
fn mdx_key(path: &str) -> String {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    match p.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => p,
    }
}

fn short(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_string()
}

pub fn shakecensus(chain: &mut Chain) -> Result<()> {
    let shakes = benilla_formats::load_camera_shakes(chain)?;
    let creatures = benilla_formats::load_creature_catalog(chain)?;

    let mut models: Vec<(u32, String, u32, u32)> = creatures
        .shaking_models()
        .map(|(id, path, f, t)| (id, path.to_string(), f, t))
        .collect();
    models.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

    let mut users: BTreeMap<u32, Users> = BTreeMap::new();
    for (model_id, path, footstep, thud) in &models {
        let label = format!("{} ({model_id})", short(path));
        if *footstep != 0 {
            users
                .entry(*footstep)
                .or_default()
                .footstep
                .push(label.clone());
        }
        if *thud != 0 {
            users.entry(*thud).or_default().death_thud.push(label);
        }
    }

    // The spell side's two hops: the groups naming each preset, the kits naming each group.
    let mut group_users: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for g in shakes.groups() {
        for id in g.shakes() {
            let entry = group_users.entry(id).or_default();
            if !entry.contains(&g.id) {
                entry.push(g.id);
            }
        }
    }
    for groups in group_users.values_mut() {
        groups.sort_unstable();
    }
    let visuals = benilla_formats::load_spell_visual_catalog(chain)?;
    let mut kits_by_group: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for kit_id in visuals.kit_ids() {
        if let Some(group) = visuals.kit(kit_id).and_then(|k| k.shake) {
            kits_by_group.entry(group).or_default().push(kit_id);
        }
    }
    for kits in kits_by_group.values_mut() {
        kits.sort_unstable();
    }

    println!("CameraShakes.dbc — {} presets\n", shakes.len());
    println!(
        "{:>4}  {:>4} {:>4}  {:>9} {:>9} {:>8} {:>7} {:>7}  named by",
        "id", "type", "dir", "amplitude", "frequency", "duration", "phase", "coeff"
    );
    let mut rows: Vec<_> = shakes.iter().collect();
    rows.sort_by_key(|r| r.id);
    for r in rows {
        let mut parts = Vec::new();
        if let Some(u) = users.get(&r.id) {
            if !u.footstep.is_empty() {
                parts.push(format!("footstep: {}", u.footstep.join(", ")));
            }
            if !u.death_thud.is_empty() {
                parts.push(format!("death thud: {}", u.death_thud.join(", ")));
            }
        }
        if let Some(g) = group_users.get(&r.id) {
            parts.push(format!(
                "group{} {}",
                if g.len() == 1 { "" } else { "s" },
                g.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
        let named = if parts.is_empty() {
            "—  NOTHING NAMES THIS ROW".to_string()
        } else {
            parts.join("  ·  ")
        };
        println!(
            "{:>4}  {:>4} {:>4}  {:>9.3} {:>9.3} {:>8.3} {:>7.3} {:>7.3}  {}",
            r.id,
            r.shake_type,
            r.direction,
            r.amplitude,
            r.frequency,
            r.duration,
            r.phase,
            r.coefficient,
            named
        );
    }

    // Every value the creature columns carry must resolve; a miss means the column map is wrong.
    let mut dangling = Vec::new();
    for (model_id, path, footstep, thud) in &models {
        for (col, id) in [
            ("FootstepShakeSize", footstep),
            ("DeathThudShakeSize", thud),
        ] {
            if *id != 0 && shakes.get(*id).is_none() {
                dangling.push(format!("{path} ({model_id}): {col} = {id}"));
            }
        }
    }
    println!(
        "\n{} of the shipped creature models name a shake; {} dangling id(s)",
        models.len(),
        dangling.len()
    );
    for d in &dangling {
        println!("  DANGLING {d}");
    }

    println!(
        "\nSpellEffectCameraShakes.dbc — {} groups (the id space every SPELL-side producer speaks)\n",
        shakes.group_len()
    );
    println!("{:>5}  {:>14}  named by SpellVisualKit", "group", "shakes");
    let mut groups: Vec<_> = shakes.groups().collect();
    groups.sort_by_key(|g| g.id);
    let mut group_dangling = Vec::new();
    for g in groups {
        let slots = g
            .shakes()
            .map(|id| {
                if shakes.get(id).is_none() {
                    group_dangling.push(format!("group {}: shake {id}", g.id));
                }
                id.to_string()
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let kits = match kits_by_group.get(&g.id) {
            None => "—  (reached only by a $SHK animation event, if at all)".to_string(),
            Some(k) => format!(
                "{} kit(s): {}",
                k.len(),
                k.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
            ),
        };
        println!("{:>5}  {:>14}  {}", g.id, slots, kits);
    }
    let kit_shakes: usize = kits_by_group.values().map(Vec::len).sum();
    println!(
        "\n{kit_shakes} of the shipped SpellVisualKit rows name a group; {} dangling slot(s)",
        group_dangling.len()
    );
    for d in &group_dangling {
        println!("  DANGLING {d}");
    }
    for group in kits_by_group.keys() {
        if shakes.group(*group).is_none() {
            println!("  DANGLING kit group {group} (no such SpellEffectCameraShakes row)");
        }
    }

    // The third producer, the `$SHK` animation event, whose payload is a group id, never a preset.
    println!("\n$SHK animation events — the third producer (GameObject / DynamicObject only)\n");
    // Only the GameObject (typemask 0x20, `0x5f3e20`) and DynamicObject (0x40, `0x5d58c0`)
    // dispatchers decode `$SHK`; on a creature it reaches `CGUnit_C::HandleAnimEvent` and on a
    // bone-attached `CEffect` the fixed `$SND`/`$HIT` router `0x61f6f0`, and neither decodes it.
    // A DynamicObject's area model is `SpellVisual` field 12, gated by field 11 (`0x5d57c0`):
    // Inferno's visual 4859 and Meteor's 7479 plant one (effects 2362, 3007) with `$SHK` group 2.
    let gos = benilla_formats::load_gameobject_catalog(chain)?;
    let go_models: BTreeSet<String> = gos.iter().map(|(_, path)| mdx_key(path)).collect();
    let creature_models: BTreeSet<String> = creatures.model_paths().map(mdx_key).collect();
    let dynobj_models: BTreeSet<String> = visuals
        .visuals()
        .filter(|(_, v)| v.area_gate != 0 && v.area_effect != 0)
        .filter_map(|(_, v)| visuals.effect_path(v.area_effect))
        .map(mdx_key)
        .collect();
    let mut shk_models = 0u32;
    let mut shk_marks = 0u32;
    let mut shk_live = 0u32;
    let mut shk_groups: BTreeMap<u32, u32> = BTreeMap::new();
    let mut shk_live_groups: BTreeMap<u32, u32> = BTreeMap::new();
    for name in crate::scan::m2_names(chain, None)? {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let mut rows = Vec::new();
        let mut groups_here = Vec::new();
        for a in benilla_formats::parse_m2_animations(&bytes) {
            for e in a.events.iter().filter(|e| &e.ident == b"$SHK") {
                *shk_groups.entry(e.data).or_default() += 1;
                groups_here.push(e.data);
                shk_marks += 1;
                rows.push(format!(
                    "    seq {:>2} anim {:>3} {:>7.3}s  group {}{}",
                    a.seq_index,
                    a.anim_id,
                    e.time,
                    e.data,
                    match shakes.group(e.data) {
                        Some(g) => format!(
                            " → {}",
                            g.shakes()
                                .map(|id| id.to_string())
                                .collect::<Vec<_>>()
                                .join(" · ")
                        ),
                        None => "  DANGLING (no such group)".to_string(),
                    }
                ));
            }
        }
        if !rows.is_empty() {
            shk_models += 1;
            let key = mdx_key(&name);
            let live = go_models.contains(&key) || dynobj_models.contains(&key);
            if live {
                shk_live += rows.len() as u32;
                for g in &groups_here {
                    *shk_live_groups.entry(*g).or_default() += 1;
                }
            }
            let host = if go_models.contains(&key) {
                "GameObject display — LIVE"
            } else if dynobj_models.contains(&key) {
                "DynamicObject area model (SpellVisual field 12) — LIVE"
            } else if creature_models.contains(&key) {
                "creature model — INERT (CGUnit_C::HandleAnimEvent decodes no $SHK)"
            } else {
                "no host names it — INERT"
            };
            println!("  {name}   [{host}]");
            for r in rows {
                println!("{r}");
            }
        }
    }
    let tally = |m: &BTreeMap<u32, u32>| {
        if m.is_empty() {
            "none".to_string()
        } else {
            m.iter()
                .map(|(g, n)| format!("{g}×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    println!(
        "\n{shk_models} model(s) author $SHK, {shk_marks} marker(s); groups named: {}",
        tally(&shk_groups)
    );
    println!(
        "{shk_live} marker(s) sit on a host that DECODES the tag; live groups: {}",
        tally(&shk_live_groups)
    );

    // Two tiers: a preset a table names can still have nothing that fires it, as a group no kit
    // and no live `$SHK` marker names is dead.
    let named: BTreeSet<u32> = users
        .keys()
        .copied()
        .chain(group_users.keys().copied())
        .collect();
    let live_groups: BTreeSet<u32> = kits_by_group
        .keys()
        .copied()
        .chain(shk_live_groups.keys().copied())
        .collect();
    let fired: BTreeSet<u32> = users
        .keys()
        .copied()
        .chain(
            shakes
                .groups()
                .filter(|g| live_groups.contains(&g.id))
                .flat_map(|g| g.shakes()),
        )
        .collect();
    let missing = |set: &BTreeSet<u32>| {
        let out: Vec<String> = shakes
            .iter()
            .map(|r| r.id)
            .filter(|id| !set.contains(id))
            .collect::<BTreeSet<_>>()
            .iter()
            .map(u32::to_string)
            .collect();
        if out.is_empty() {
            "none".to_string()
        } else {
            out.join(", ")
        }
    };
    println!(
        "\nreachability: {} of {} presets are NAMED by a table (unnamed: {})",
        named.len(),
        shakes.len(),
        missing(&named)
    );
    println!(
        "              {} of {} have a live PRODUCER behind them (dead: {})",
        fired.len(),
        shakes.len(),
        missing(&fired)
    );
    Ok(())
}
