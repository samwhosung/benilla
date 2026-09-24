//! Corpus scans over **placed world content** — what a WMO root or an ADT tile block puts in
//! the world, and how much of it the reference would even draw.
//!
//! One `.wmo` root's own tables (`wmodoodads`, `skyboxscan`) and one map region's placements
//! (`placescan`, `doodadscan`). How a placed prop is LIT is a separate question — [`super::lighting`].

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use benilla_formats::{Chain, M2AnimSummary};

use crate::{model_key, yn};

/// Dump a WMO root's placed-prop tables: every MODD doodad with its MODS set membership and its
/// OWNING group(s) read from the group files' MODR lists — the relation the reference instantiates
/// from (`0x695aa0` loops a *visible* group's own refs). A prop referenced by NO group is never
/// created by the real client at all — the
/// divergence decision 0689 names and benilla still spawns. This answers "which props exist here,
/// who owns them, and which would the reference even draw" in one read (the B30/B32 question).
pub fn wmodoodads(chain: &mut Chain, raw_path: &str, filter: Option<&str>) -> Result<()> {
    let root_path = raw_path.replace('/', "\\").to_ascii_lowercase();
    let bytes = chain
        .read_file(&root_path)
        .with_context(|| format!("reading WMO root '{root_path}'"))?;
    let root = benilla_formats::parse_wmo_root(&bytes)
        .with_context(|| format!("parsing WMO root '{root_path}'"))?;
    let set_names = mods_set_names(&bytes);

    // MODD index -> the groups whose MODR reference it (the ownership relation).
    let stem = root_path.strip_suffix(".wmo").unwrap_or(&root_path);
    let mut owners: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
    let mut groups_read = 0u32;
    for gi in 0..root.group_count() {
        let group_path = format!("{stem}_{gi:03}.wmo");
        let Ok(gbytes) = chain.read_file(&group_path) else {
            continue;
        };
        groups_read += 1;
        for r in benilla_formats::wmo_group_doodad_refs(&gbytes) {
            owners.entry(r).or_default().push(gi);
        }
    }

    println!(
        "{} doodad(s), {} set(s), {} group(s) ({} group file(s) read)",
        root.doodads().len(),
        root.doodad_sets().len(),
        root.group_count(),
        groups_read,
    );
    for (si, s) in root.doodad_sets().iter().enumerate() {
        let name = set_names.get(si).map(String::as_str).unwrap_or("?");
        println!(
            "set {si:>2}  [{:>5}..{:>5})  count {:>5}  {name}",
            s.start,
            s.start + s.count,
            s.count
        );
    }

    let needle = filter.map(str::to_ascii_lowercase);
    let infos = root.group_infos();
    let mut shown = 0u32;
    let mut orphans_total = 0u32;
    let mut orphans_shown = 0u32;
    for (i, d) in root.doodads().iter().enumerate() {
        let orphan = !owners.contains_key(&(i as u16));
        if orphan {
            orphans_total += 1;
        }
        if let Some(n) = &needle {
            if !model_key(&d.model).contains(n.as_str()) {
                continue;
            }
        }
        shown += 1;
        if orphan {
            orphans_shown += 1;
        }
        let sets: Vec<String> = root
            .doodad_sets()
            .iter()
            .enumerate()
            .filter(|(_, s)| (s.start..s.start + s.count).contains(&(i as u32)))
            .map(|(si, _)| si.to_string())
            .collect();
        let owner_cell = match owners.get(&(i as u16)) {
            Some(gs) => gs
                .iter()
                .map(|&g| {
                    let class = match infos.get(g as usize) {
                        Some(gi) if gi.interior => "INT",
                        Some(_) => "EXT",
                        None => "?",
                    };
                    format!("g{g}({class})")
                })
                .collect::<Vec<_>>()
                .join(" "),
            None => "ORPHAN".into(),
        };
        println!(
            "modd {i:>5}  pos ({:>8.2}, {:>8.2}, {:>8.2})  scale {:.3}  color #{:02x}{:02x}{:02x}{:02x}  sets [{}]  {owner_cell}  {}",
            d.position[0], d.position[1], d.position[2],
            d.scale,
            d.color[0], d.color[1], d.color[2], d.color[3],
            sets.join(","),
            d.model,
        );
    }
    match needle {
        Some(_) => eprintln!(
            "{shown} doodad(s) matched ({orphans_shown} ORPHAN); {orphans_total} orphan(s) among all {}",
            root.doodads().len()
        ),
        None => eprintln!(
            "{orphans_total} of {} doodad(s) are ORPHANS (in no group's MODR — the reference never instantiates these)",
            root.doodads().len()
        ),
    }
    Ok(())
}

/// The MODS set names (`char name[20]` per 32-byte record) — `WmoDoodadSet` keeps only the ranges,
/// so read the names off the raw root bytes here (top-level chunks are `[magic][size][data]`, magic
/// on disk reversed: MODS → `SDOM`).
fn mods_set_names(bytes: &[u8]) -> Vec<String> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size = u32::from_le_bytes([
            bytes[off + 4],
            bytes[off + 5],
            bytes[off + 6],
            bytes[off + 7],
        ]) as usize;
        let Some(data_end) = (off + 8).checked_add(size) else {
            return Vec::new();
        };
        if data_end > bytes.len() {
            return Vec::new();
        }
        if &bytes[off..off + 4] == b"SDOM" {
            return bytes[off + 8..data_end]
                .as_chunks::<32>()
                .0
                .iter()
                .map(|rec| {
                    let name = &rec[..20];
                    let end = name.iter().position(|&b| b == 0).unwrap_or(20);
                    String::from_utf8_lossy(&name[..end]).into_owned()
                })
                .collect();
        }
        off = data_end;
    }
    Vec::new()
}

/// Sweep every WMO **root** in the chain and report the two halves of the skybox mechanism: which
/// roots author a **MOSB** skybox model, and which carry groups flagged `0x40000`
/// ([`benilla_formats::WmoGroupInfo::show_skybox`]).
///
/// This is the instrument that *identifies* the flag — and **exactly how far that identification
/// reaches is the point**. `0x40000` is undocumented, so the cross-tab is what establishes it means
/// anything at all: the bit never appears on a group whose root names no skybox, across all 815
/// roots. That is a one-way implication, `flag ⇒ MOSB`, and the summary prints it so the claim is
/// re-checkable in one command rather than trusted from a decision record.
///
/// **It does not, and cannot, say which group the RENDERER tests** — and reading it as if it did is
/// the mistake decision 0767 made (superseded by 0773). The law is that `0x40000` is tested
/// inside the portal flood (`0x6b42e0` in `0x6b41c0`) on the group being *visited*, so the predicate
/// is "any flood-reached group carries the bit". A census over static asset bytes has no way to see
/// that distinction; only the binary did.
///
/// It is also the population instrument for the mechanism: which buildings in 1.12 replace the
/// `Light.dbc` gradient dome with an authored sky, and how much of each one does it. Stratholme's
/// city shell sets the bit on 61 of its 83 groups; the only other roots that set it at all are the
/// four Caverns of Time shells, which ship in the 5875 data with no 1.12 instance to enter.
pub fn skyboxscan(chain: &mut Chain) -> Result<()> {
    // Roots only, because only the root carries MOSB/MOGI.
    let names = super::wmo_roots(chain, None)?;

    // The cross-tab that identifies the flag: roots with/without a MOSB × groups with/without 0x40000.
    let (mut both, mut mosb_only, mut flag_only, mut neither) = (0u32, 0u32, 0u32, 0u32);
    let mut scanned = 0u32;
    let mut hits: Vec<(String, String, usize, usize)> = Vec::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Ok(root) = benilla_formats::parse_wmo_root(&bytes) else {
            continue; // a group file that slipped the name filter, or a truncated root
        };
        scanned += 1;
        let groups = root.group_infos();
        let flagged = groups.iter().filter(|g| g.show_skybox).count();
        match (root.skybox(), flagged > 0) {
            (Some(sky), true) => {
                both += 1;
                hits.push((name, sky.to_string(), flagged, groups.len()));
            }
            (Some(sky), false) => {
                mosb_only += 1;
                hits.push((
                    name,
                    format!("{sky}  (NO group sets 0x40000)"),
                    0,
                    groups.len(),
                ));
            }
            (None, true) => {
                flag_only += 1;
                hits.push((name, "(no MOSB)".into(), flagged, groups.len()));
            }
            (None, false) => neither += 1,
        }
    }

    hits.sort();
    println!("{:<62} {:>7}  skybox model", "WMO root", "groups");
    for (name, sky, flagged, total) in &hits {
        println!("{name:<62} {flagged:>3}/{total:<3}  {sky}");
    }
    println!();
    println!("{scanned} WMO root(s) scanned");
    println!("  MOSB skybox AND >=1 group with 0x40000 : {both}");
    println!("  MOSB skybox but NO group with 0x40000  : {mosb_only}");
    println!("  group(s) with 0x40000 but NO MOSB      : {flag_only}");
    println!("  neither                                : {neither}");
    if flag_only == 0 {
        println!(
            "\n=> 0x40000 NEVER appears without a MOSB ({flag_only} counter-examples in {scanned} \
             roots), and {mosb_only} root(s) name a skybox no group asks for. So the bit is real and \
             both halves matter — but this census establishes only 'flag implies MOSB'."
        );
        println!(
            "   It does NOT say WHICH group the renderer tests, and reading it as if it did is the \
             mistake decision 0767 made (superseded by 0773). The carved law: 0x40000 is tested \
             inside the portal flood (0x6b42e0 in 0x6b41c0) on the group being VISITED, never on the \
             group the camera stands in — so the predicate is 'any FLOOD-REACHED group carries the \
             bit, and the root names a MOSB'. Stratholme's King's Square is the counter-example that \
             settles it: the camera's own group (39) is EXTERIOR and unflagged, and the reference \
             paints the sky there anyway."
        );
    } else {
        println!(
            "\n=> {flag_only} group(s) set 0x40000 with no MOSB to draw — that would break even the \
             weak 'flag implies MOSB' reading this census rests on; re-derive before building on it."
        );
    }
    Ok(())
}

/// List individual doodad (MDDF) and WMO (MODF) placements around a world position whose model
/// path contains `filter` (case-insensitive) — the per-placement position / Euler rotation /
/// scale / uniqueId ground truth an orientation investigation needs (`doodadscan` only
/// aggregates).
pub fn placescan(
    chain: &mut Chain,
    map: &str,
    center_x: f32,
    center_y: f32,
    tile_radius: u32,
    filter: &str,
) -> Result<()> {
    let tiles = benilla_formats::load_tiles_around(chain, map, center_x, center_y, tile_radius)
        .with_context(|| format!("loading tiles around ({center_x}, {center_y}) on {map}"))?;
    eprintln!("{} tile(s) loaded", tiles.len());
    let needle = filter.to_ascii_lowercase();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut hits = 0u32;
    for (_, tile) in &tiles {
        for d in &tile.doodads {
            if seen.insert(d.unique_id) && model_key(&d.model).contains(&needle) {
                hits += 1;
                println!(
                    "MDDF uid {:>8}  pos ({:>9.2}, {:>9.2}, {:>8.2})  rot deg ({:>7.2}, {:>7.2}, {:>7.2})  scale {:.3}  {}",
                    d.unique_id,
                    d.position[0], d.position[1], d.position[2],
                    d.rotation[0], d.rotation[1], d.rotation[2],
                    d.scale,
                    d.model,
                );
            }
        }
        for w in &tile.wmos {
            if seen.insert(w.unique_id) && w.model.to_ascii_lowercase().contains(&needle) {
                hits += 1;
                println!(
                    "MODF uid {:>8}  pos ({:>9.2}, {:>9.2}, {:>8.2})  rot deg ({:>7.2}, {:>7.2}, {:>7.2})  {}",
                    w.unique_id,
                    w.position[0], w.position[1], w.position[2],
                    w.rotation[0], w.rotation[1], w.rotation[2],
                    w.model,
                );
            }
        }
    }
    eprintln!("{hits} placement(s) matched '{filter}'");
    Ok(())
}

/// Bulk-scan placed doodads (MDDF) and WMO doodad-set-0 props (MODF → MODS/MODD) across a
/// `(2·tile_radius+1)²` block of ADT tiles around a world position, and report how much of that
/// content animates.
pub fn doodadscan(
    chain: &mut Chain,
    map: &str,
    center_x: f32,
    center_y: f32,
    tile_radius: u32,
) -> Result<()> {
    let tiles = benilla_formats::load_tiles_around(chain, map, center_x, center_y, tile_radius)
        .with_context(|| format!("loading tiles around ({center_x}, {center_y}) on {map}"))?;
    eprintln!("{} tile(s) loaded", tiles.len());

    // Direct M2 placements (MDDF) — deduped by uniqueId, which a tile-straddling doodad
    // repeats identically across every tile it touches (decision-0021 terrain streamer's own
    // dedup key; see `benilla_formats::Doodad::unique_id`).
    let mut seen_doodad_ids: HashSet<u32> = HashSet::new();
    let mut m2_instances: HashMap<String, u32> = HashMap::new();
    // WMO placements (MODF) — same dedup, by their own uniqueId.
    let mut seen_wmo_ids: HashSet<u32> = HashSet::new();
    let mut wmo_instances: HashMap<String, u32> = HashMap::new();
    for (_, tile) in &tiles {
        for d in &tile.doodads {
            if seen_doodad_ids.insert(d.unique_id) {
                *m2_instances.entry(model_key(&d.model)).or_insert(0) += 1;
            }
        }
        for w in &tile.wmos {
            if seen_wmo_ids.insert(w.unique_id) {
                *wmo_instances.entry(w.model.clone()).or_insert(0) += 1;
            }
        }
    }
    let direct_m2_instances: u32 = m2_instances.values().sum();
    eprintln!(
        "{direct_m2_instances} MDDF doodad placement(s) across {} unique M2 model(s)",
        m2_instances.len()
    );
    eprintln!(
        "{} MODF WMO placement(s) across {} unique WMO model(s)",
        seen_wmo_ids.len(),
        wmo_instances.len()
    );

    // Fold each unique WMO's doodad-set-**0** M2 props into the same instance table (set 0 is
    // the WMO's always-on global set, per `WmoDoodadSet` doc), each multiplied by that WMO's
    // own (deduped) placement count — one building placement = one instance of every set-0 prop
    // it carries.
    let mut wmo_root_failures = 0u32;
    for (wmo_path, &count) in &wmo_instances {
        let root_path = wmo_path.to_ascii_lowercase(); // matches `load_wmo`'s own normalization
        let root = chain
            .read_file(&root_path)
            .ok()
            .and_then(|bytes| benilla_formats::parse_wmo_root(&bytes).ok());
        let Some(root) = root else {
            wmo_root_failures += 1;
            continue;
        };
        let Some(set0) = root.doodad_sets().first() else {
            continue;
        };
        let range = set0.start as usize..(set0.start as usize + set0.count as usize);
        for wd in root.doodads().get(range).unwrap_or(&[]) {
            if !wd.model.is_empty() {
                *m2_instances.entry(model_key(&wd.model)).or_insert(0) += count;
            }
        }
    }
    if wmo_root_failures > 0 {
        eprintln!("  ({wmo_root_failures} WMO root(s) failed to read/parse — skipped)");
    }

    // Per-unique-model animation summary — one parse per model regardless of instance count.
    let mut summaries: HashMap<String, M2AnimSummary> = HashMap::new();
    let mut parse_failures: Vec<(String, String)> = Vec::new();
    for model in m2_instances.keys() {
        match benilla_formats::load_m2_animation_summary(chain, model) {
            Ok(s) => {
                summaries.insert(model.clone(), s);
            }
            Err(e) => parse_failures.push((model.clone(), e.to_string())),
        }
    }

    let total_instances: u32 = m2_instances.values().sum();
    let total_models = m2_instances.len();
    println!();
    println!(
        "=== totals ({total_instances} M2 instance(s), {total_models} unique model(s), {} parse failure(s)) ===",
        parse_failures.len()
    );

    type ChannelCheck = (&'static str, fn(&M2AnimSummary) -> bool);
    let checks: [ChannelCheck; 9] = [
        ("seq-0 bone motion", |s| s.seq0_has_bone_motion),
        ("moving seq0, >1 variation", |s| {
            s.seq0_has_bone_motion && s.seq0_variation_count > 1
        }),
        ("global-seq bones", |s| !s.global_seq_channels.is_empty()),
        ("animated transparency", |s| s.transparency_tracks.1 > 0),
        ("animated color", |s| {
            s.color_rgb_tracks.1 > 0 || s.color_alpha_tracks.1 > 0
        }),
        ("texture transforms", |s| s.texture_transform_count > 0),
        ("particles", |s| s.particle_emitter_count > 0),
        ("emitter on moving bone", |s| {
            s.emitter_bones.iter().any(|e| e.chain_animated())
        }),
        ("ribbons", |s| s.ribbon_emitter_count > 0),
    ];
    let report_row = |label: &str, pred: &dyn Fn(&M2AnimSummary) -> bool| {
        let inst: u32 = m2_instances
            .iter()
            .filter_map(|(m, &c)| summaries.get(m).filter(|s| pred(s)).map(|_| c))
            .sum();
        let models = summaries.values().filter(|s| pred(s)).count();
        println!(
            "  {label:22} {inst:>6} instances ({:5.1}%)   {models:>4} models ({:5.1}%)",
            100.0 * f64::from(inst) / f64::from(total_instances.max(1)),
            100.0 * models as f64 / total_models.max(1) as f64,
        );
    };
    for (label, pred) in checks {
        report_row(label, &pred);
    }
    report_row("NO animated channel", &|s| s.is_fully_static());

    println!();
    println!("=== top 30 models by instance count ===");
    println!(
        "{:>6}  {:>4} {:>4} {:>5} {:>5} {:>4} {:>4} {:>4}  model",
        "count", "seq0", "gseq", "trns", "clr", "txfm", "part", "ribn"
    );
    let mut ranked: Vec<(&String, &u32)> = m2_instances.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (model, &count) in ranked.iter().take(30) {
        match summaries.get(*model) {
            Some(s) => println!(
                "{count:>6}  {:>4} {:>4} {:>5} {:>5} {:>4} {:>4} {:>4}  {model}",
                yn(s.seq0_has_bone_motion),
                yn(!s.global_seq_channels.is_empty()),
                yn(s.transparency_tracks.1 > 0),
                yn(s.color_rgb_tracks.1 > 0 || s.color_alpha_tracks.1 > 0),
                yn(s.texture_transform_count > 0),
                yn(s.particle_emitter_count > 0),
                yn(s.ribbon_emitter_count > 0),
            ),
            None => println!("{count:>6}  <parse failed>  {model}"),
        }
    }

    // The rare material channels by NAME (each is <1% of instances, so the top-30 table
    // almost never surfaces them): the exact models the phase-2/3 material-animation work
    // verifies against.
    println!();
    println!("=== material-channel models (animated transparency / color / UV) ===");
    let mut rare: Vec<(&String, &u32)> = m2_instances
        .iter()
        .filter(|(m, _)| {
            summaries.get(*m).is_some_and(|s| {
                s.transparency_tracks.1 > 0
                    || s.color_rgb_tracks.1 > 0
                    || s.color_alpha_tracks.1 > 0
                    || s.texture_transform_count > 0
            })
        })
        .collect();
    rare.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (model, &count) in &rare {
        let s = &summaries[*model];
        println!(
            "{count:>6}  trns:{} clr:{} txfm:{}  {model}",
            s.transparency_tracks.1,
            s.color_rgb_tracks.1 + s.color_alpha_tracks.1,
            s.texture_transform_count,
        );
    }

    // Moving-seq0 models with a variation chain, by NAME (the random-variation arm `0x695100`):
    // the exact placed models where variationIdx −1 vs 0 is visible at all.
    println!();
    println!("=== moving-seq0 multi-variation models ===");
    let mut varied: Vec<(&String, &u32)> = m2_instances
        .iter()
        .filter(|(m, _)| {
            summaries
                .get(*m)
                .is_some_and(|s| s.seq0_has_bone_motion && s.seq0_variation_count > 1)
        })
        .collect();
    varied.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (model, &count) in &varied {
        let s = &summaries[*model];
        println!(
            "{count:>6}  {} variation(s) of seq0  {model}",
            s.seq0_variation_count
        );
    }

    // Emitters hosted on a moving bone chain, by NAME (0130 phase 4 grounding): the exact
    // placed models where emitter bone-follow is visible at all — an emitter on a static
    // chain sits at its rest pose whether or not we attach it.
    println!();
    println!("=== emitter-on-moving-bone models ===");
    let mut movers: Vec<(&String, &u32)> = m2_instances
        .iter()
        .filter(|(m, _)| {
            summaries
                .get(*m)
                .is_some_and(|s| s.emitter_bones.iter().any(|e| e.chain_animated()))
        })
        .collect();
    movers.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (model, &count) in &movers {
        let s = &summaries[*model];
        let moving = s
            .emitter_bones
            .iter()
            .filter(|e| e.chain_animated())
            .count();
        println!(
            "{count:>6}  {moving}/{} emitter(s) on moving bones  {model}",
            s.emitter_bones.len()
        );
    }

    if !parse_failures.is_empty() {
        println!();
        println!("=== parse failures ({}) ===", parse_failures.len());
        for (model, err) in &parse_failures {
            println!("  {model}: {err}");
        }
    }

    Ok(())
}
