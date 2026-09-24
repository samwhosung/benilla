//! Corpus scans over **which sequence is playing** — the animation-arm side, where being on the
//! wrong sequence (or on none at all) silently mis-renders everything keyed to it.
//!
//! The GameObject substate arm (`goanimscan`), the effect-model Stand/Hold/Decay lifecycle
//! (`fxlifescan`), the loader-idle slot the per-sequence bakes degrade away from (`idleslotscan`),
//! and the models with no keyed bone at all, so no clock ever advances (`seqclockscan`).
//!
//! Plus the arm's *reach*: which models author a sound-event marker the rig gate never arms
//! (`soundeventscan`) — the same failure one level down, where the sequence is right and nothing
//! runs its clock.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result};
use benilla_formats::Chain;

use crate::model_key;

/// The GameObject animation arm's LUT (`.data 0x8607e4`):
/// internal **substate** → the `AnimationData.dbc` id the object layer arms.
const SUBSTATE_ANIM: [u16; 13] = [
    145, // 0  Spawn      — NO client path produces this substate
    147, // 1  Closed     (rest)
    148, // 2  Open       (motion)
    149, // 3  Opened     (rest)
    146, // 4  Close      (motion)
    150, // 5  Destroy    (motion)
    151, // 6  Destroyed  (rest)
    152, // 7  Rebuild    (motion)
    153, 154, 155, 156, // 8..11 Custom0-3 — reachable only via SMSG_GAMEOBJECT_CUSTOM_ANIM
    157, // 12 Despawn
];

/// The **transient** (transition-motion) substates among the reachable ones — the rows slot 14
/// `0x5f4120` advances off at the arm's window end (2 Open → 3 Opened, 4 Close → 1 Closed,
/// 5 Destroy → 6 Destroyed). Their duration is the object layer's, never the clip's loop bit.
const MOTION_SUBSTATES: [usize; 3] = [2, 4, 5];

/// The six substates a `GAMEOBJECT_STATE` × `GAMEOBJECT_ANIMPROGRESS` pair can actually produce
/// (`0x5f3c30`). Substate 0 (Spawn) has no producer at all, and 8..12 come from other opcodes
/// entirely.
const REACHABLE: [(usize, &str); 6] = [
    (1, "READY  settled"),
    (4, "READY  mid    "),
    (3, "ACTIVE settled"),
    (2, "ACTIVE mid    "),
    (6, "ALT    settled"),
    (5, "ALT    mid    "),
];

/// The four-way remap (`0x5f3972`): what the arm actually requests when the model doesn't author
/// the substate's LUT id. Returns `(id, rate0)` — `rate0` marks the two legs that freeze a *motion*
/// clip at frame 0 to stand in for a missing *rest* pose.
fn go_remap(m: &benilla_m2::M2Model, id: u16) -> (u16, bool) {
    if m.owns_animation(id) {
        return (id, false);
    }
    match id {
        // Close missing: keep it (op4 resolves onward) if Open exists, else fall to Closed.
        146 => (if m.owns_animation(148) { 146 } else { 147 }, false),
        // Closed missing: keep it if Close exists; else freeze Open at frame 0; else Stand.
        147 if m.owns_animation(146) => (147, false),
        147 if m.owns_animation(148) => (148, true),
        147 => (0, false),
        // Open missing: keep it if Close exists; else Destroy if present; else Opened.
        148 => (
            if m.owns_animation(146) {
                148
            } else if m.owns_animation(150) {
                150
            } else {
                149
            },
            false,
        ),
        // Opened missing: keep it if Open exists; else freeze Close at frame 0; else Destroyed.
        149 if m.owns_animation(148) => (149, false),
        149 if m.owns_animation(146) => (146, true),
        149 => (151, false),
        // Outside the door group there is no remap — the id goes to op4 as-is.
        other => (other, false),
    }
}

/// op4's own id → played sequence resolve (`0x7121a0` via `0x711bf0`): the model's
/// `playableAnimationLookup` row (when the id is in range), then `animationLookup` to a file slot.
/// `None` when nothing playable comes out — the reference arms nothing and the pose simply stands.
fn go_resolve_slot(m: &benilla_m2::M2Model, id: u16) -> Option<(u16, u16)> {
    let played = m
        .playable_animation_lookup
        .get(id as usize)
        .map_or(id, |p| p.resolved_id);
    let slot = *m.animation_lookup.get(played as usize)?;
    (slot != 0xffff).then_some((played, slot))
}

/// The generic loader seed (`0x71019b`): resolve id 0, and arm **id 0** when the model owns what
/// that resolves to — only the degenerate leg (owning nothing reachable) falls back to the raw
/// `animations[0]` dword.
fn go_loader_seed(
    m: &benilla_m2::M2Model,
    seqs: &[benilla_formats::ModelAnimation],
) -> Option<(u16, u16)> {
    let resolved = m
        .playable_animation_lookup
        .first()
        .map_or(0, |p| p.resolved_id);
    if m.owns_animation(resolved) {
        go_resolve_slot(m, 0)
    } else {
        // The degenerate leg: `animations[0]`'s low16 — the file-order-first sequence's own id.
        go_resolve_slot(m, seqs.first()?.anim_id)
    }
}

/// Sweep every model named by **GameObjectDisplayInfo.dbc** and resolve, per model, what the
/// reference's GameObject animation arm plays in each reachable `GAMEOBJECT_STATE` ×
/// `GAMEOBJECT_ANIMPROGRESS` substate — see the `Goanimscan` command doc.
pub fn goanimscan(chain: &mut Chain) -> Result<()> {
    let catalog =
        benilla_formats::load_gameobject_catalog(chain).context("GameObjectDisplayInfo.dbc")?;
    // displayId → path, deduped to one entry per model (many displays share a model).
    let mut models: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (id, path) in catalog.iter() {
        let key = model_key(path);
        if key.ends_with(".m2") {
            models.entry(key).or_default().push(id);
        }
    }
    for ids in models.values_mut() {
        ids.sort_unstable();
    }
    let (mut parsed, mut no_seq, mut blind, mut sensitive, mut needs_remap, mut rate0) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    // The transition half: a MOTION substate whose resolved sequence is bit-0-clear
    // is one the kernel wraps for ever — so it is bounded only by the object layer's completion
    // advance (`0x5f4120`), and a consumer that arms it by the loop bit instead flaps. And `replay`
    // decides whether that window is one band or several.
    let (mut looping_motion, mut looping_motion_sensitive, mut multi_replay) = (0u32, 0u32, 0u32);
    // The VARIATION half (`0x5f3aee: push -1`): the GameObject arm rolls a `_rand`-weighted
    // variation, while the loader seed underneath it (`0x71019b`) takes an explicit variation 0. So
    // a model that authors a CHAIN on a reachable substate's id plays something the seed never
    // reaches — and a consumer resolving the id to its head variation renders the whole rest of the
    // chain unreachable. Onyxia's lava traps are the case (`ONYZIASLAIRLAVATRAP`: Stand ×2, and
    // only the 10 %-weighted second one spurts lava).
    let mut chained = 0u32;
    let mut chained_paths: Vec<String> = Vec::new();
    for (path, displays) in &models {
        let Ok(bytes) = chain.read_file(path) else {
            continue;
        };
        let Ok(fmt) = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes)) else {
            continue;
        };
        let m = fmt.model();
        parsed += 1;
        let seqs = benilla_formats::parse_m2_animations(&bytes);
        if seqs.is_empty() {
            no_seq += 1;
            continue;
        }
        let seed = go_loader_seed(m, &seqs);
        let mut lines = Vec::new();
        let (mut differs, mut remapped, mut froze) = (false, false, false);
        let (mut flaps, mut replays) = (false, false);
        let mut has_chain = false;
        for (sub, label) in REACHABLE {
            let lut = SUBSTATE_ANIM[sub];
            let (req, r0) = go_remap(m, lut);
            let armed = go_resolve_slot(m, req);
            remapped |= req != lut;
            froze |= r0;
            differs |= armed.map(|(_, s)| s) != seed.map(|(_, s)| s);
            // The played sequence's own kernel law. `slot` is the M2's FILE slot, which is not this
            // list's index (zero-duration sequences are dropped), so match on `seq_index`.
            let played =
                armed.and_then(|(_, slot)| seqs.iter().find(|s| s.seq_index == slot as usize));
            // A transient substate (a transition motion) armed on a band the kernel wraps: what
            // ends it is the completion advance (`0x5f4120`), never the clip.
            let motion = MOTION_SUBSTATES.contains(&sub);
            flaps |= motion && !r0 && played.is_some_and(|s| s.looping);
            replays |= played.is_some_and(|s| (s.min_replay, s.max_replay) != (0, 0));
            // A variation chain on THIS substate's armed id: >1 sequence sharing it.
            let variations =
                armed.map_or(0, |(id, _)| seqs.iter().filter(|s| s.anim_id == id).count());
            has_chain |= variations > 1;
            lines.push(format!(
                "   {label} sub{sub}  lut {lut}{}  ->  {}{}",
                if req == lut {
                    String::new()
                } else {
                    format!(" (remap {req})")
                },
                match armed {
                    Some((id, slot)) => format!("id {id} slot {slot}"),
                    None => "NOTHING".to_string(),
                },
                match played {
                    None => String::new(),
                    Some(s) => format!(
                        "  {}{}{}",
                        if s.looping { "loop " } else { "clamp" },
                        if r0 {
                            "  [rate 0 — frozen]"
                        } else if motion {
                            "  MOTION"
                        } else {
                            ""
                        },
                        if (s.min_replay, s.max_replay) == (0, 0) {
                            String::new()
                        } else {
                            format!("  replay {}..{}", s.min_replay, s.max_replay)
                        }
                    ),
                },
            ));
        }
        if has_chain {
            chained += 1;
            chained_paths.push(path.clone());
        }
        flaps.then(|| looping_motion += 1);
        (flaps && differs).then(|| looping_motion_sensitive += 1);
        replays.then(|| multi_replay += 1);
        if differs {
            sensitive += 1;
        } else {
            blind += 1;
        }
        remapped.then(|| needs_remap += 1);
        froze.then(|| rate0 += 1);
        // Only the state-SENSITIVE models are worth printing: on every other one the arm lands on
        // the same sequence the loader seed already holds, so `GAMEOBJECT_STATE` is unobservable.
        if differs {
            println!("{path}  ({} sequences, displays {displays:?})", seqs.len());
            println!(
                "   loader seed              ->  {}",
                match seed {
                    Some((id, slot)) => format!("id {id} slot {slot}"),
                    None => "NOTHING".to_string(),
                }
            );
            for l in lines {
                println!("{l}");
            }
        }
    }
    println!(
        "\n{} GameObjectDisplayInfo M2 models, {parsed} parsed, {no_seq} with no sequences",
        models.len()
    );
    println!(
        "  STATE-BLIND    {blind}  — every reachable substate lands on the loader seed's own \
         sequence, so GAMEOBJECT_STATE cannot be seen on this model at all"
    );
    println!(
        "  STATE-SENSITIVE {sensitive}  — at least one substate plays something else: exactly the \
         models a GO type that skips the arm renders in the wrong pose"
    );
    println!("  needing the four-way remap on some substate: {needs_remap}");
    println!(
        "  authoring a VARIATION CHAIN on a reachable substate: {chained}  — the models the arm's \
         `variationIdx = -1` roll can reach and the loader seed's explicit variation 0 cannot"
    );
    for p in &chained_paths {
        println!("      {p}");
    }
    println!(
        "  arming a LOOPING band on a transition (motion) substate: {looping_motion} \
         ({looping_motion_sensitive} of them state-SENSITIVE, i.e. the transition is a clip the \
         rest pose isn't)  — the completion advance is the only thing that ends these; read \
         as \"should this clip repeat?\" they swing for ever (decision 1151)"
    );
    println!(
        "  authoring a non-empty replay range on a reachable substate: {multi_replay}  — R > 1 \
         would make the transition several band lengths, so 0 here means one window IS the swing"
    );
    println!("  hitting a rate-0 freeze leg (a motion clip standing in for a missing rest pose): {rate0}");
    Ok(())
}

/// `AnimationData.dbc` ids of the effect-model lifecycle triple (names read from the real DBC).
const ANIM_STAND: u16 = 0;
const ANIM_HOLD: u16 = 158;
const ANIM_DECAY: u16 = 159;

/// Sweep every `.m2` (optionally under a path prefix) and census the **effect-model animation
/// lifecycle**: which models author the `Stand`(0) → `Hold`(158) → `Decay`(159) triple, and what a
/// consumer that arms ONE sequence and never advances would render for each.
///
/// The reference arms an effect instance's default track — `animationLookup[0]`, i.e. `Stand` — and
/// a model that also owns `Hold` authors `Stand` as a **birth** (grow-in, clamp flag set) with the
/// sustained pulse living in the separate looping `Hold` sequence, and the fade-out in `Decay`.
/// A one-sequence consumer therefore FREEZES on the last frame of the birth for the effect's whole
/// life — no pulse, no fade — which is exactly what "the Ice Barrier shield is frozen" looks like.
///
/// The classification is that failure, made countable:
/// - **FREEZE** — owns `Hold`, and the armed `Stand` clamps: the reference pulses, we hold a pose.
/// - **hold-loops** — owns `Hold` but `Stand` itself loops: still wrong (the wrong clip), but moving.
/// - **decay-only** — owns `Decay` and no `Hold`: only the reap leg is unrendered.
///
/// It also counts the **file-order divergence**: the reference arms `animationLookup[0]`, so a
/// consumer that arms the file's *first slot* instead is additionally wrong on any model whose slot
/// 0 is not its `Stand` (the `DuelingFlag.m2` Spawn/Stand/Despawn shape). The two
/// bugs are independent and the closing line separates them.
///
/// The population instrument for the mechanism, so it is closed corpus-wide rather than spell by
/// spell; `m2seq` then explains one model in full.
pub fn fxlifescan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut freeze, mut hold_loops, mut decay_only) = (0u32, 0u32, 0u32, 0u32);
    let (mut no_stand, mut slot0_not_stand) = (0u32, 0u32);
    let mut by_dir: BTreeMap<String, u32> = BTreeMap::new();
    let mut rows: Vec<(String, String, String, f32, f32, f32)> = Vec::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let seqs = benilla_formats::parse_m2_animations(&bytes);
        if seqs.is_empty() {
            continue;
        }
        // What the reference arms: `animationLookup[0]` — Stand — never the file-order-first slot
        // (they diverge on any model whose sequence 0 is not its Stand).
        let armed = seqs.iter().find(|s| s.anim_id == ANIM_STAND);
        let hold = seqs.iter().find(|s| s.anim_id == ANIM_HOLD);
        let decay = seqs.iter().find(|s| s.anim_id == ANIM_DECAY);
        if hold.is_none() && decay.is_none() {
            continue; // neither leg authored — no lifecycle to miss
        }
        let class = match (hold.is_some(), armed) {
            (true, Some(a)) if !a.looping => {
                freeze += 1;
                "FREEZE"
            }
            (true, _) => {
                hold_loops += 1;
                "hold-loops"
            }
            (false, _) => {
                decay_only += 1;
                "decay-only"
            }
        };
        // The file-order divergence, independent of the lifecycle bug: what a slot-0 consumer arms
        // versus the reference's `animationLookup[0]`.
        let arm = match (armed, seqs[0].anim_id) {
            (None, first) => {
                no_stand += 1;
                format!("no-Stand(slot0={first})")
            }
            (Some(_), ANIM_STAND) => "slot0".to_string(),
            (Some(_), first) => {
                slot0_not_stand += 1;
                format!("slot0={first}!")
            }
        };
        let top = name.split_once('\\').map(|(d, _)| d).unwrap_or("<root>");
        *by_dir.entry(top.to_ascii_lowercase()).or_default() += 1;
        rows.push((
            name,
            class.to_string(),
            arm,
            armed.map_or(0.0, |a| a.duration),
            hold.map_or(0.0, |s| s.duration),
            decay.map_or(0.0, |s| s.duration),
        ));
    }
    rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    println!(
        "model                                                        class       arm         stand   hold  decay"
    );
    for (name, class, arm, stand, hold, decay) in rows.iter().take(80) {
        println!("{name:<60}  {class:<10}  {arm:<10}  {stand:>5.2}  {hold:>5.2}  {decay:>5.2}");
    }
    if rows.len() > 80 {
        println!("… and {} more", rows.len() - 80);
    }
    println!(
        "\n{} of {scanned} models author a Hold(158)/Decay(159) lifecycle leg",
        rows.len()
    );
    println!("  FREEZE      {freeze:>5}  (owns Hold; the armed Stand clamps — a pose, where the reference pulses)");
    println!("  hold-loops  {hold_loops:>5}  (owns Hold; the armed Stand loops — moving, but the wrong clip)");
    println!("  decay-only  {decay_only:>5}  (owns Decay only — just the reap leg unrendered)");
    println!(
        "of those, the file-order divergence (a slot-0 consumer arms the wrong sequence outright): \
         {slot0_not_stand} with slot 0 != Stand, {no_stand} with no Stand at all"
    );
    // Named in full, never left to the row cap: this set is small and each member is a distinct
    // "arms the wrong sequence from frame one" case.
    for (name, _, arm, ..) in rows.iter().filter(|r| r.2 != "slot0") {
        println!("  {name:<58}  {arm}");
    }
    println!("by top-level directory:");
    for (dir, n) in &by_dir {
        println!("  {dir:<16} {n:>5}");
    }
    Ok(())
}

/// Sweep every `.m2` (optionally under a path prefix) and list the models where the **loader-idle
/// sequence is not file slot 0** while benilla's render content gate declines to arm it — so every
/// per-sequence bake (`EmitTiming`/`EmitParams`/`AlphaAnim`) degrades to slot 0, a sequence the
/// instance is not playing (found on the Stormwind battlefield banner).
///
/// The reference arms the loader-idle sequence on **every** M2 instance at load (`0x70ebd0`'s
/// tail), so "which sequence is this playing" always has an answer.
/// benilla skips the arm when looping the idle would render identically to the static mesh — sound
/// for the *mesh*, and silently wrong for anything keyed on the sequence *identity*. The two only
/// disagree visibly when the idle is not slot 0, which is the `DuelingFlag` shape:
/// `Spawn(145) / Stand(0) / Despawn(157)`, where slot 0 is the **Spawn flourish**.
///
/// Reports, per model: the emitters whose enabled/rate gate at t = 0 differs between slot 0 and the
/// idle slot (`FX`), and the batches whose combined material-alpha factor differs (`ALPHA`).
pub fn idleslotscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut unarmed, mut window) = (0u32, 0u32, 0u32);
    let (mut fx_models, mut alpha_models) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let anims = benilla_formats::parse_m2_animations(&bytes);
        if anims.is_empty() {
            continue;
        }
        scanned += 1;
        let idle_id = benilla_formats::parse_m2_playable_animation_lookup(&bytes)
            .unwrap_or_default()
            .first()
            .map_or(0, |p| p.resolved_id);
        // benilla's render content gate (`benilla_assets`' `idle_pose_differs`): a bind-pose idle
        // is not armed at all, so nothing ever overrides the per-sequence bakes' opening slot.
        if anims
            .iter()
            .any(|a| a.anim_id == idle_id && !a.is_rest_pose())
        {
            continue;
        }
        unarmed += 1;
        let idle = anims
            .iter()
            .find(|a| a.anim_id == idle_id)
            .map_or(0, |a| a.seq_index);
        if idle == 0 {
            continue; // degrading to slot 0 happens to BE the idle slot — no divergence
        }
        window += 1;
        let mut lines = Vec::new();
        let defs = benilla_formats::parse_m2_particle_emitters(&bytes).unwrap_or_default();
        let mut fx = 0u32;
        for (i, d) in defs.iter().enumerate() {
            let on = |s: usize| {
                d.timing.emitting(Some(s), 0.0, 0.0) && d.timing.rate(Some(s), 0.0, 0.0) > 0.0
            };
            if on(0) != on(idle) {
                fx += 1;
                lines.push(format!(
                    "    FX    emitter {i:>2}: slot 0 {:>3}, idle slot {idle} {:>3}  tex {}",
                    if on(0) { "ON" } else { "off" },
                    if on(idle) { "ON" } else { "off" },
                    d.texture.as_deref().unwrap_or("NONE"),
                ));
            }
        }
        let dir = name.rsplit_once('\\').map_or("", |(d, _)| d);
        let subs = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]).unwrap_or_default();
        let mut alpha = 0u32;
        for (i, s) in subs.iter().enumerate() {
            let Some(a) = &s.alpha_anim else { continue };
            let (f0, fi) = (a.sample(Some(0), 0.0, 0.0), a.sample(Some(idle), 0.0, 0.0));
            if (f0 - fi).abs() > 1e-3 {
                alpha += 1;
                lines.push(format!(
                    "    ALPHA batch   {i:>2}: slot 0 {f0:.2}, idle slot {idle} {fi:.2}"
                ));
            }
        }
        if !lines.is_empty() {
            fx_models += u32::from(fx > 0);
            alpha_models += u32::from(alpha > 0);
            println!("{name}  (idle slot {idle} of {} sequences)", anims.len());
            for l in lines {
                println!("{l}");
            }
        }
    }
    eprintln!(
        "{scanned} models with sequences, {unarmed} whose idle the content gate leaves unarmed, \
         {window} of those with idle slot != 0 (the divergence window): \
         {fx_models} model(s) whose EMITTERS gate differently, \
         {alpha_models} whose MATERIAL ALPHA differs"
    );
    Ok(())
}

/// Sweep every `.m2` (optionally under a path prefix) and list the models whose animation is
/// authored **entirely outside the bone tracks** — sequences exist, per-sequence consumers exist
/// (particle emitters, ribbons, material alpha/colour, UV transforms), and **not one sequence
/// keys a bone**.
///
/// That combination is the blind spot of a renderer whose sequence clock rides a *bone* animation
/// clip: with no keyed bone there is no clip, so there is no player, so nothing ever says which
/// sequence is playing or how far into it — and every per-sequence consumer degrades to "file slot
/// 0, at t = 0", permanently. The emitters still build, pool and tick; they just read the wrong
/// column of a table that never advances. Found on the Molten Core rune + flame ring (decision
/// 0941): the rune's slot 0 is its *Closed* band, where every emission rate key is 0 (no flames at
/// all), and the ring's spline spawn-window opens `0 → 1` over its first second, so frozen at t=0
/// all 180 particles are born at one point of a 2.8-yd circle — one bright blob.
///
/// The `[GO]` mark is the cross-tab that matters: a `GameObjectDisplayInfo` model reaches the
/// world through the **hosted** clock (`EmitClock::Host`), which is the lane that freezes at
/// `t = 0`. A placed doodad of the same shape still runs its spawn-age clock (only its *slot* is
/// pinned — decision 0760's axis), so it is wrong in a different, milder way.
///
/// PARTIAL models — some sequences key bones, some don't — are tallied, not listed: there the
/// clock exists, but the unkeyed sequences are unreachable (no clip to arm), so a GameObject
/// substate or a creature animation id that resolves to one plays nothing.
pub fn seqclockscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    // Which models a GameObject can display — the hosted-clock lane.
    let go_models: HashSet<String> = benilla_formats::load_gameobject_catalog(chain)
        .map(|c| c.iter().map(|(_, p)| model_key(p)).collect())
        .unwrap_or_default();
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut with_seqs, mut frozen, mut frozen_go, mut partial, mut inert) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut frozen_emitters = 0usize;
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let seqs = benilla_formats::parse_m2_animations(&bytes);
        if seqs.is_empty() {
            continue;
        }
        with_seqs += 1;
        // Exactly the runtime's own test (`build_animation_clip`): a sequence becomes a clip iff
        // some bone track produced a key inside its band.
        let keyed = seqs
            .iter()
            .filter(|a| {
                a.bones.iter().any(|b| {
                    !b.translation.is_empty() || !b.rotation.is_empty() || !b.scale.is_empty()
                })
            })
            .count();
        // Counted over EVERY model with sequences, consumers or not: it measures clip
        // REACHABILITY (can an arm find this sequence at all), which is a property of the model.
        if keyed > 0 && keyed < seqs.len() {
            partial += 1;
        }
        let Ok(s) = benilla_formats::parse_m2_animation_summary(&bytes) else {
            continue;
        };
        // The per-sequence consumers: everything that samples a track on the playing sequence's
        // clock. A constant (≤1 key) colour/alpha track is excluded — it reads the same in every
        // slot, so a frozen clock costs it nothing.
        let consumers = s.particle_emitter_count
            + s.ribbon_emitter_count
            + s.color_alpha_tracks.1
            + s.color_rgb_tracks.1
            + s.transparency_tracks.1
            + s.texture_transform_count;
        if keyed == 0 && consumers == 0 {
            // The same shape with nothing to sample: a clock costs it nothing and buys it
            // nothing. Counted because it IS the cost side of giving every sequenced model a
            // clock — these are the models that gain an inert one.
            inert += 1;
        }
        if consumers > 0 && keyed == 0 {
            frozen += 1;
            frozen_emitters += s.particle_emitter_count;
            let is_go = go_models.contains(&name.to_ascii_lowercase());
            if is_go {
                frozen_go += 1;
            }
            println!(
                "{}{name}\n    seqs {:>3} (0 keyed) · emitters {:>2} ribbons {} · alpha {} rgb {} \
                 transp {} · uvanim {} · gseq {}",
                if is_go { "[GO] " } else { "     " },
                seqs.len(),
                s.particle_emitter_count,
                s.ribbon_emitter_count,
                s.color_alpha_tracks.1,
                s.color_rgb_tracks.1,
                s.transparency_tracks.1,
                s.texture_transform_count,
                s.global_seq_channels.len(),
            );
        }
    }
    eprintln!(
        "{scanned} models scanned, {with_seqs} with sequences: {frozen} CLOCKLESS \
         ({frozen_emitters} emitters) — sequences + per-sequence consumers, not one keyed bone; \
         {frozen_go} of them GameObject display models (the hosted lane, frozen at slot 0 t=0). \
         {partial} PARTIAL models have unkeyed sequences a clip lookup can never reach; \
         {inert} more are boneless with nothing to sample (a clock there is inert)."
    );
    Ok(())
}

/// Sweep every `.m2` and census the **animation-driven sound emitters**: the models whose
/// sequences carry a `$DSL` (doodad sound loop) / `$DSE` (its release token) / `$DSO` (doodad
/// sound one-shot) / `$SND` (generic one-shot) event marker, the `SoundEntries` kit each names,
/// and — the column this exists for — whether the carrying sequence is **rest-posed**.
///
/// `$DSE` is in the set because it is half of `$DSL`'s lifecycle and it is **lane-specific**: the
/// placed-doodad handler `0x6951e0` has an arm for it, the GameObject dispatcher `0x5f3e20` has
/// none at all, so which models author it decides whether
/// a consumer may route the token uniformly.
///
/// Why that column decides everything: `benilla_assets`' render content gate
/// (`idle_pose_differs` → [`benilla_formats::ModelAnimation::is_rest_pose`]) skips
/// building a rig for a sequence that would render as the static mesh, and the whole point of
/// that gate is that it is a question **about pixels only**. A placed lamp's Stand band keys no
/// bone at all — it is pure rest pose — yet it carries the one `$DSL` marker that is its hum. So
/// every model in the `REST` column is one whose sound the arm can never reach through a rig: its
/// events need a clock that does not depend on there being anything to animate.
///
/// Reports the per-model rows, then a tag/kit histogram and the `REST`-gated share.
pub fn soundeventscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    const SOUND_TAGS: [&[u8; 4]; 4] = [b"$DSL", b"$DSE", b"$DSO", b"$SND"];
    let kits = benilla_formats::load_sound_kit_catalog(chain).ok();
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut carriers, mut rest_gated, mut hostless) = (0u32, 0u32, 0u32, 0u32);
    let mut per_tier: BTreeMap<&str, u32> = BTreeMap::new();
    let mut per_tag: BTreeMap<String, (u32, u32)> = BTreeMap::new(); // tag -> (markers, rest-gated)
    let mut per_kit: BTreeMap<u32, u32> = BTreeMap::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let anims = benilla_formats::parse_m2_animations(&bytes);
        if anims.is_empty() {
            continue;
        }
        scanned += 1;
        // The spawn tier `benilla_world::doodad_anim::classify` would give this model, from the
        // same three inputs it reads: a boneless model is `Static` outright (the 0035 guard);
        // otherwise the 0130 content gate picks `FirstSeq` when the loader-idle clip's pose
        // differs from bind, else `GlobalSeqOnly` when free-running channels exist, else
        // `Static`. `is_rest_pose` is the gate's own predicate (decision 0936 pushed it down
        // beside the parse so this census cannot drift from it); the tier matters here because
        // only the two animated tiers get an anim-root ENTITY at all — a `Static` carrier has
        // nothing but loose submeshes to hang an emitter on.
        let bones = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes))
            .map_or(0, |f| f.model().bones.len());
        let idle_id = benilla_formats::parse_m2_playable_animation_lookup(&bytes)
            .unwrap_or_default()
            .first()
            .map_or(0, |p| p.resolved_id);
        let gseq = !benilla_formats::parse_m2_global_sequence_bones(&bytes).is_empty();
        let tier = if bones == 0 {
            "Static"
        } else if anims
            .iter()
            .any(|a| a.anim_id == idle_id && !a.is_rest_pose())
        {
            "FirstSeq"
        } else if gseq {
            "GlobalSeq"
        } else {
            "Static"
        };
        let mut rows = Vec::new();
        let mut model_rest_gated = false;
        for a in &anims {
            let marks: Vec<_> = a
                .events
                .iter()
                .filter(|e| SOUND_TAGS.contains(&&e.ident))
                .collect();
            if marks.is_empty() {
                continue;
            }
            // The gate is asked of the sequence that CARRIES the marker: that is the one whose
            // clock has to run for the marker to be reached at all.
            let rest = a.is_rest_pose();
            model_rest_gated |= rest;
            for e in &marks {
                let tag = String::from_utf8_lossy(&e.ident).into_owned();
                let slot = per_tag.entry(tag).or_default();
                slot.0 += 1;
                slot.1 += u32::from(rest);
                *per_kit.entry(e.data).or_default() += 1;
            }
            let list: Vec<String> = marks
                .iter()
                .map(|e| {
                    let tag = String::from_utf8_lossy(&e.ident);
                    let kit = kits
                        .as_ref()
                        .and_then(|c| c.get(e.data))
                        .map_or_else(|| "?".to_string(), |k| k.name.clone());
                    format!("{:.3}s {tag}({}) {kit}", e.time, e.data)
                })
                .collect();
            rows.push(format!(
                "    seq {:>2} anim {:>3} {:<5} {:>6.3}s  {}  {}",
                a.seq_index,
                a.anim_id,
                if a.looping { "loop" } else { "clamp" },
                a.duration,
                if rest { "REST" } else { "rig " },
                list.join(", "),
            ));
        }
        if rows.is_empty() {
            continue;
        }
        carriers += 1;
        rest_gated += u32::from(model_rest_gated);
        *per_tier.entry(tier).or_default() += 1;
        if tier == "Static" {
            hostless += 1;
        }
        println!("{name}  [{tier}]");
        for r in rows {
            println!("{r}");
        }
    }
    println!("\n=== tag histogram (markers, and how many sit on a REST-posed sequence) ===");
    for (tag, (n, rest)) in &per_tag {
        println!("  {tag}  {n:>5} marker(s), {rest:>5} on a REST-posed sequence");
    }
    println!("\n=== distinct sound kits named ({}) ===", per_kit.len());
    for (id, n) in &per_kit {
        let k = kits.as_ref().and_then(|c| c.get(*id));
        println!(
            "  {id:>6}  x{n:<4} {:<40} vol {:>4.2}  minDist {:>7.2}  cutoff {:>8.2}  flags 0x{:03x}",
            k.map_or("MISSING", |k| k.name.as_str()),
            k.map_or(0.0, |k| k.volume),
            k.map_or(0.0, |k| k.min_distance),
            k.map_or(0.0, |k| k.distance_cutoff),
            k.map_or(0, |k| k.flags),
        );
    }
    println!("\n=== spawn tier of the carriers ===");
    for (t, n) in &per_tier {
        println!("  {t:<10} {n:>4} model(s)");
    }
    eprintln!(
        "{scanned} models with sequences scanned; {carriers} carry a $DSL/$DSO/$SND marker, \
         {hostless} of them on the Static tier (no anim-root entity exists to hang an emitter \
         on today); {rest_gated} carry the marker on a REST-posed sequence — the class benilla's rig gate \
         (decision 0130) never arms, so the marker is unreachable through an AnimationPlayer."
    );
    Ok(())
}

/// The ten `GameObjectDisplayInfo.Sound[n]` slots, in column order — the names the reference's
/// own substate family gives them.
const GO_SLOT_NAMES: [&str; 10] = [
    "Stand", "Open", "Loop", "Close", "Destroy", "Opened", "Custom0", "Custom1", "Custom2",
    "Custom3",
];

/// The event tag that reaches each display slot (the dispatcher `0x5f3e20`'s ten call sites):
/// `$GO0..5` → slots 0..5, `$GC0..3` → slots 6..9.
fn go_slot_tag(slot: usize) -> [u8; 4] {
    if slot < 6 {
        [b'$', b'G', b'O', b'0' + slot as u8]
    } else {
        [b'$', b'G', b'C', b'0' + (slot - 6) as u8]
    }
}

/// The model sequence **file slots** the GameObject arm can ever put on screen: the loader seed
/// (`0x71019b`) plus every substate any client path produces — the six `GAMEOBJECT_STATE` ×
/// `GAMEOBJECT_ANIMPROGRESS` ones ([`REACHABLE`]), the four Custom ones (opcode `0xb3`) and
/// Despawn (`0x215`) — each through the four-way remap (`0x5f3972`) and op4's
/// `playableAnimationLookup` resolve.
///
/// This is the reachability half of [`goslotscan`]: an authored `$GOn` on a sequence outside this
/// set is a marker no arm can ever cross, so its display slot is dead however the DBC is filled.
fn go_armable_slots(
    m: &benilla_m2::M2Model,
    seqs: &[benilla_formats::ModelAnimation],
) -> HashSet<u16> {
    let mut out = HashSet::new();
    out.extend(go_loader_seed(m, seqs).map(|(_, slot)| slot));
    let substates = REACHABLE.iter().map(|(s, _)| *s).chain(8..=12);
    for sub in substates {
        let (req, _) = go_remap(m, SUBSTATE_ANIM[sub]);
        out.extend(go_resolve_slot(m, req).map(|(_, slot)| slot));
    }
    out
}

/// Census the **GameObject display sound slots** against the model events that are the only thing
/// that can reach them — see the `Goslotscan` command doc for what the join proves.
pub fn goslotscan(chain: &mut Chain) -> Result<()> {
    let catalog =
        benilla_formats::load_gameobject_catalog(chain).context("GameObjectDisplayInfo.dbc")?;
    let sounds =
        benilla_formats::load_gameobject_sounds(chain).context("GameObjectDisplayInfo.dbc")?;
    let kits = benilla_formats::load_sound_kit_catalog(chain).ok();
    // displayId → model path, so the same model parsed once serves every display that names it.
    let mut per_model: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (id, path) in catalog.iter() {
        if sounds.slots(id).is_some() {
            per_model.entry(model_key(path)).or_default().push(id);
        }
    }
    for ids in per_model.values_mut() {
        ids.sort_unstable();
    }
    // Per slot: displays carrying a kit · of those, models authoring the tag · of those, on an
    // armable sequence · and how many of the live ones name a LOOPING (0x200) kit — the lane
    // select `0x458830` hands `0x5f4010`, which is the emitter-pool arm rather than a one-shot.
    let mut filled = [0u32; 10];
    let mut tagged = [0u32; 10];
    let mut reachable = [0u32; 10];
    let mut looping = [0u32; 10];
    // Filled columns whose model authors no reaching tag at all — dead data in the reference, and
    // exactly what a state-transition-driven consumer would play that the reference never does.
    let mut dead = [0u32; 10];
    // Markers authored where the display's column is 0 — the tag fires and `0x458830` fails a null
    // id, so the reference is silent. Counted per tag so "authored but silent" is visible.
    let mut silent_tag = [0u32; 10];
    let mut unarmable_tag = [0u32; 10];
    let (mut models_seen, mut wmo_or_missing) = (0u32, 0u32);
    let mut rows: Vec<String> = Vec::new();
    let mut loop_rows: Vec<String> = Vec::new();
    for (path, displays) in &per_model {
        if !path.ends_with(".m2") {
            wmo_or_missing += displays.len() as u32;
            continue;
        }
        let Ok(bytes) = chain.read_file(path) else {
            wmo_or_missing += displays.len() as u32;
            continue;
        };
        let Ok(fmt) = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes)) else {
            wmo_or_missing += displays.len() as u32;
            continue;
        };
        let m = fmt.model();
        let seqs = benilla_formats::parse_m2_animations(&bytes);
        models_seen += 1;
        let armable = go_armable_slots(m, &seqs);
        // slot → the sequences authoring its tag, as `(seq_index, anim_id, time, armable)`.
        let mut authored: [Vec<(usize, u16, f32, bool)>; 10] = Default::default();
        for a in &seqs {
            for e in &a.events {
                for (slot, list) in authored.iter_mut().enumerate() {
                    if e.ident == go_slot_tag(slot) {
                        list.push((
                            a.seq_index,
                            a.anim_id,
                            e.time,
                            armable.contains(&(a.seq_index as u16)),
                        ));
                    }
                }
            }
        }
        for &display in displays {
            let slots = sounds.slots(display).copied().unwrap_or([0; 10]);
            let mut line: Vec<String> = Vec::new();
            for slot in 0..10 {
                let kit = slots[slot];
                let auth = &authored[slot];
                if kit == 0 {
                    silent_tag[slot] += u32::from(!auth.is_empty());
                    continue;
                }
                filled[slot] += 1;
                let live = auth.iter().any(|(_, _, _, armable)| *armable);
                if auth.is_empty() {
                    dead[slot] += 1;
                } else {
                    tagged[slot] += 1;
                    if live {
                        reachable[slot] += 1;
                    } else {
                        unarmable_tag[slot] += 1;
                    }
                }
                let k = kits.as_ref().and_then(|c| c.get(kit));
                let flags = k.map_or(0, |k| k.flags);
                let is_loop = flags & benilla_formats::sound_kit_flags::LOOPING != 0;
                if live && is_loop {
                    looping[slot] += 1;
                    loop_rows.push(format!(
                        "  display {display:>5} {path}  slot {slot} {}  kit {kit} {} flags 0x{flags:03x}",
                        GO_SLOT_NAMES[slot],
                        k.map_or("MISSING", |k| k.name.as_str()),
                    ));
                }
                let where_ = if auth.is_empty() {
                    "DEAD: model authors no tag".to_string()
                } else {
                    auth.iter()
                        .map(|(seq, id, t, armable)| {
                            format!(
                                "seq{seq}/anim{id}@{t:.3}s{}",
                                if *armable { "" } else { "!" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                line.push(format!(
                    "{}={kit}/0x{flags:03x}{}[{where_}]",
                    GO_SLOT_NAMES[slot],
                    if is_loop { " LOOP" } else { "" },
                ));
            }
            if !line.is_empty() {
                rows.push(format!(
                    "  display {display:>5}  {path}\n      {}",
                    line.join("  ")
                ));
            }
        }
    }
    println!("=== every display row with a filled sound column, and what reaches it ===");
    for r in &rows {
        println!("{r}");
    }
    println!(
        "\n=== per-slot census ({} display rows carry any sound kit) ===",
        sounds.len()
    );
    println!(
        "  slot  name      filled  reached  DEAD(no tag)  tag-unarmable  looping-kit  \
         tag-on-zero-slot"
    );
    for slot in 0..10 {
        println!(
            "  {slot:>4}  {:<9} {:>6}  {:>7}  {:>12}  {:>13}  {:>11}  {:>16}",
            GO_SLOT_NAMES[slot],
            filled[slot],
            reachable[slot],
            dead[slot],
            unarmable_tag[slot],
            looping[slot],
            silent_tag[slot],
        );
    }
    debug_assert!(filled
        .iter()
        .zip(&tagged)
        .zip(&dead)
        .all(|((f, t), d)| *f == *t + *d));
    if !loop_rows.is_empty() {
        println!(
            "\n=== live slots whose kit is LOOPING (0x200) — the `0x461d80` emitter-pool lane ==="
        );
        for r in &loop_rows {
            println!("{r}");
        }
    }
    eprintln!(
        "{models_seen} distinct M2 models named by a sound-carrying display scanned \
         ({wmo_or_missing} display rows name a .wmo or an unreadable model). `filled` counts \
         display rows with a non-zero column; `tagged` those whose model authors the reaching \
         $GOn/$GCn tag; `reached` those where that tag sits on a sequence the GO arm can play."
    );
    Ok(())
}
