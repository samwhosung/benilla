//! `charprocs`: census the `SpellVisualKit` **CharProc** columns (fields 15–34) — the character-half
//! of a kit: what a kit does to the *body* (its alpha, its tint) rather than at an attach point.
//!
//! The scope instrument for the aura-state CharProc system (B114 — "Stealth shows nothing on the
//! character"). It answers, from the shipped table rather than from expectation: which proc **types**
//! exist, how many kits carry each, which **lifecycle stage** reaches them from a live spell, and —
//! for the state stage, whose kits live for an aura's whole life — exactly which spells reach which
//! proc with which parameter. A type that shows up only behind `cast`/`impact` is a discrete-play
//! concern; a type behind `state` is an aura-lifetime one.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use benilla_formats::{char_proc_type, Chain, SpellVisualCatalog, TrailProc, VisualStages};

/// A `SpellVisual` stage's column selector — one of the five lifecycle-kit fields.
type StagePick = fn(&VisualStages) -> u32;

/// The five `SpellVisual` lifecycle stages, in field order — the label plus its column selector.
const STAGES: [(&str, StagePick); 5] = [
    ("precast", |s| s.precast),
    ("cast", |s| s.cast),
    ("impact", |s| s.impact),
    ("state", |s| s.state),
    ("channel", |s| s.channel),
];

/// A human label for the proc types benilla models by name.
fn type_name(ty: i32) -> &'static str {
    match ty {
        char_proc_type::TINT => "TINT (body RGB)",
        char_proc_type::ALPHA => "ALPHA (body translucency)",
        char_proc_type::ANIM_RATE => "ANIM_RATE (body anim playback rate — 0 freezes the pose)",
        char_proc_type::CHAIN_CHANNEL => "CHAIN (beam; the channel-stage key — 0955)",
        char_proc_type::CHAIN_CAST => "CHAIN (beam; the cast-stage key — 0955)",
        char_proc_type::WEAPON_TRAIL => "TRAIL (weapon swing ribbon, $WTB→$WTT)",
        _ => "(unmodelled)",
    }
}

/// Census every kit's CharProc slots, then reachability per stage, then the state-stage detail.
pub fn run(chain: &mut Chain) -> Result<()> {
    let spells = benilla_formats::load_spell_catalog(chain)?;
    let visuals = benilla_formats::load_spell_visual_catalog(chain)?;

    // 1. The raw table census: proc type → the kits carrying it.
    let mut by_type: BTreeMap<i32, BTreeSet<u32>> = BTreeMap::new();
    let mut kits_with_any = 0usize;
    for kit_id in visuals.kit_ids() {
        let Some(kit) = visuals.kit(kit_id) else {
            continue;
        };
        let mut any = false;
        for proc in kit.char_procs() {
            by_type.entry(proc.ty).or_default().insert(kit_id);
            any = true;
        }
        kits_with_any += usize::from(any);
    }
    println!(
        "SpellVisualKit: {} rows, {kits_with_any} carry at least one CharProc",
        visuals.kit_len(),
    );
    println!("\nproc type census (all kits):");
    for (ty, kits) in &by_type {
        println!(
            "  type {ty:>3}  {:>4} kit(s)   {}",
            kits.len(),
            type_name(*ty)
        );
    }

    // 2. Reachability: which stage of which spell's visual actually reaches each proc type. A kit
    //    nobody's visual chain names is authored-but-dead as far as the client is concerned.
    let mut by_stage: BTreeMap<(&str, i32), BTreeSet<u32>> = BTreeMap::new();
    // The state stage's detail rows: (spell id, name, kit, proc type, params[0]).
    let mut state_rows: Vec<(u32, String, u32, i32, f32)> = Vec::new();
    for (spell_id, display) in spells.iter() {
        let Some(stages) = visuals.stages(display.visual) else {
            continue;
        };
        for (label, pick) in STAGES {
            let kit_id = pick(stages);
            let Some(kit) = visuals.kit(kit_id) else {
                continue;
            };
            for proc in kit.char_procs() {
                by_stage.entry((label, proc.ty)).or_default().insert(kit_id);
                if label == "state" {
                    state_rows.push((
                        spell_id,
                        display.name.clone(),
                        kit_id,
                        proc.ty,
                        proc.params[0],
                    ));
                }
            }
        }
    }
    println!("\nreached from a live Spell.dbc visual chain, by stage:");
    for ((label, ty), kits) in &by_stage {
        println!(
            "  {label:8} type {ty:>3}  {:>4} kit(s)   {}",
            kits.len(),
            type_name(*ty)
        );
    }

    // 3. The state stage in full — these are the aura-lifetime procs, one line per (spell, proc).
    println!(
        "\nSTATE-stage CharProcs — the aura-lifetime set ({} spell/proc pair(s)):",
        state_rows.len()
    );
    state_rows.sort_by_key(|r| (r.3, r.2, r.0));
    for (spell_id, name, kit_id, ty, param) in &state_rows {
        println!(
            "  spell {spell_id:>6} {name:<28} kit {kit_id:<5} type {ty:>3} param0 {param:<12} {}",
            type_name(*ty)
        );
    }

    // 4. The TRAIL set in full. Its shape is unlike the others': the proc carries no model and no
    //    emitter, so a kit whose slot list is EMPTY still has a visual — and 18 of these are exactly
    //    that, which is the single fact that decides whether the proc is worth building. The census
    //    prints the slot count beside every row so that number stays checkable rather than quoted.
    let trail_kits: BTreeMap<u32, TrailProc> = visuals
        .kit_ids()
        .filter_map(|id| Some((id, visuals.kit(id)?.trail_proc()?)))
        .collect();
    let mut reach: BTreeMap<u32, BTreeSet<(&str, u32, String)>> = BTreeMap::new();
    for (spell_id, display) in spells.iter() {
        let Some(stages) = visuals.stages(display.visual) else {
            continue;
        };
        for (label, pick) in STAGES {
            let kit_id = pick(stages);
            if trail_kits.contains_key(&kit_id) {
                reach
                    .entry(kit_id)
                    .or_default()
                    .insert((label, spell_id, display.name.clone()));
            }
        }
    }
    println!(
        "\nWEAPON-TRAIL kits ({} carry the proc, {} reached from a live spell, {} with NO emitter \
         slot at all):",
        trail_kits.len(),
        reach.len(),
        trail_kits
            .keys()
            .filter(|k| reach.contains_key(k))
            .filter(|k| visuals.kit(**k).is_some_and(|v| v.effects().count() == 0))
            .count(),
    );
    for (kit_id, trail) in &trail_kits {
        let kit = visuals.kit(*kit_id);
        let slots = kit.map_or(0, |k| k.effects().count());
        let anim = kit.and_then(|k| k.anim_id);
        let [r, g, b] = trail.rgb();
        let spells = reach.get(kit_id);
        // `CharParamOne` is printed even though the type-8 arm never reads it (`0x60d80a`'s three
        // `_ftol`s take Zero/Two/Three only). Not every shipped row carries `20.0` there, and a
        // census that hides the column cannot catch that — Sinister Strike's kit 399 carries 15.0.
        let unread = kit
            .and_then(|k| {
                k.char_procs()
                    .find(|p| p.ty == char_proc_type::WEAPON_TRAIL)
            })
            .map_or(0.0, |p| p.params[1]);
        println!(
            "  kit {kit_id:<5} anim {:<6} slots {slots}  #{r:02x}{g:02x}{b:02x} a{:<4} {:>6} ms  \
             one={unread:<5} {} spell(s)",
            anim.map_or("-".to_string(), |a| a.to_string()),
            trail.alpha(),
            trail.duration_ms,
            spells.map_or(0, BTreeSet::len),
        );
        for (label, spell_id, name) in spells.into_iter().flatten().take(6) {
            println!("        {label:8} {spell_id:>6} {name}");
        }
        if spells.map_or(0, BTreeSet::len) > 6 {
            println!("        … and {} more", spells.map_or(0, BTreeSet::len) - 6);
        }
    }
    Ok(())
}

/// The kits a spell's visual chain reaches, printed as one line per stage — the per-spell view of
/// the census above (used by `spellvis`'s CharProc lines; kept here beside the type names).
pub fn print_kit_procs(visuals: &SpellVisualCatalog, kit_id: u32, indent: &str) {
    let Some(kit) = visuals.kit(kit_id) else {
        return;
    };
    for (i, proc) in kit.char_proc_slots.iter().enumerate() {
        let Some(proc) = proc else { continue };
        println!(
            "{indent}charproc[{i}] type {:>3} params {:?}  {}",
            proc.ty,
            proc.params,
            type_name(proc.ty)
        );
    }
}
