//! Corpus scans over which sequence is playing: GameObject substates, the effect-model lifecycle,
//! the idle slot, clockless models, and the markers a sequence carries.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result};
use benilla_formats::Chain;

use crate::model_key;

/// The GameObject arm's LUT (`.data 0x8607e4`): internal substate → the `AnimationData.dbc` id.
const SUBSTATE_ANIM: [u16; 13] = [
    145, // 0  Spawn      (one-shot on create, `0x5f382f` → `0x5f8c50`)
    147, // 1  Closed     (rest)
    148, // 2  Open       (motion)
    149, // 3  Opened     (rest)
    146, // 4  Close      (motion)
    150, // 5  Destroy    (motion)
    151, // 6  Destroyed  (rest)
    152, // 7  Rebuild    (motion)
    153, 154, 155, 156, // 8..11 Custom0-3 (one-shots through `0x5f8c50`)
    157, // 12 Despawn
];

/// The transition substates, which slot 14 (`0x5f4120`) advances at the arm's window end
/// (2 Open → 3 Opened, 4 Close → 1 Closed, 5 Destroy → 6 Destroyed), whatever the clip's loop bit.
const MOTION_SUBSTATES: [usize; 3] = [2, 4, 5];

/// The six substates a `GAMEOBJECT_STATE` × `GAMEOBJECT_ANIMPROGRESS` pair produces (`0x5f3c30`);
/// Spawn (0), Custom0-3 (8..11) and Despawn (12) come from the one-shot channel `0x5f8c50`.
const REACHABLE: [(usize, &str); 6] = [
    (1, "READY  settled"),
    (4, "READY  mid    "),
    (3, "ACTIVE settled"),
    (2, "ACTIVE mid    "),
    (6, "ALT    settled"),
    (5, "ALT    mid    "),
];

/// The four-way remap (`0x5f3972`) for a model that lacks the substate's LUT id, as `(id, rate0)`:
/// `rate0` marks the two legs that freeze a motion clip at frame 0 in place of a missing rest pose.
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
        // Outside the door group the id goes to op4 as is.
        other => (other, false),
    }
}

/// op4's id resolve (`0x7121a0` via `0x711bf0`): `playableAnimationLookup` when the id is in
/// range, then `animationLookup` to a file slot. With no slot the reference arms nothing.
fn go_resolve_slot(m: &benilla_m2::M2Model, id: u16) -> Option<(u16, u16)> {
    let played = m
        .playable_animation_lookup
        .get(id as usize)
        .map_or(id, |p| p.resolved_id);
    let slot = *m.animation_lookup.get(played as usize)?;
    (slot != 0xffff).then_some((played, slot))
}

/// The loader seed (`0x71019b`): id 0 when the model owns what id 0 resolves to, else the
/// file-order-first sequence's own id (`animations[0]`).
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
        go_resolve_slot(m, seqs.first()?.anim_id)
    }
}

/// Resolve, for every `GameObjectDisplayInfo` model, what the reference's GameObject arm plays in
/// each state substate, printing the models where that differs from the loader seed.
pub fn goanimscan(chain: &mut Chain) -> Result<()> {
    let catalog =
        benilla_formats::load_gameobject_catalog(chain).context("GameObjectDisplayInfo.dbc")?;
    // Model path → its display ids; many displays share a model.
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
    // A motion substate on a looping sequence (flags bit 0 clear) wraps until the completion
    // advance (`0x5f4120`) ends it; the replay range sets how many bands that window spans.
    let (mut looping_motion, mut looping_motion_sensitive, mut multi_replay) = (0u32, 0u32, 0u32);
    // The arm rolls a weighted variation (`0x5f3aee` pushes -1) where the loader seed takes
    // variation 0 (`0x71019b`), so a chain on a substate's id reaches sequences the seed cannot
    // (`ONYZIASLAIRLAVATRAP`: two Stands, only the 10%-weighted second spurts lava).
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
            // `slot` is a file slot, not an index here (zero-duration sequences are dropped).
            let played =
                armed.and_then(|(_, slot)| seqs.iter().find(|s| s.seq_index == slot as usize));
            let motion = MOTION_SUBSTATES.contains(&sub);
            flaps |= motion && !r0 && played.is_some_and(|s| s.looping);
            replays |= played.is_some_and(|s| (s.min_replay, s.max_replay) != (0, 0));
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
        // Print only state-sensitive models; on the rest every substate plays the seed's sequence.
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
         as \"should this clip repeat?\" they swing for ever"
    );
    println!(
        "  authoring a non-empty replay range on a reachable substate: {multi_replay}  — R > 1 \
         would make the transition several band lengths, so 0 here means one window IS the swing"
    );
    println!("  hitting a rate-0 freeze leg (a motion clip standing in for a missing rest pose): {rate0}");
    Ok(())
}

/// `AnimationData.dbc` ids of the effect-model lifecycle: Stand, Hold, Decay.
const ANIM_STAND: u16 = 0;
const ANIM_HOLD: u16 = 158;
const ANIM_DECAY: u16 = 159;

/// Census the effect models with a `Hold` (158) or `Decay` (159) leg. The reference arms
/// `animationLookup[0]`, `Stand`, which beside a `Hold` is a clamped birth before the looping
/// pulse, so a consumer that never advances freezes on its last frame (`FREEZE`); `hold-loops`
/// has a looping `Stand`, `decay-only` no `Hold`. It also lists the models whose file slot 0 is
/// not `Stand` (`DuelingFlag.m2`: Spawn, Stand, Despawn), where arming slot 0 is wrong outright.
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
        // The reference arms `animationLookup[0]`, Stand, not file slot 0.
        let armed = seqs.iter().find(|s| s.anim_id == ANIM_STAND);
        let hold = seqs.iter().find(|s| s.anim_id == ANIM_HOLD);
        let decay = seqs.iter().find(|s| s.anim_id == ANIM_DECAY);
        if hold.is_none() && decay.is_none() {
            continue; // no lifecycle leg authored
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
    for (name, _, arm, ..) in rows.iter().filter(|r| r.2 != "slot0") {
        println!("  {name:<58}  {arm}");
    }
    println!("by top-level directory:");
    for (dir, n) in &by_dir {
        println!("  {dir:<16} {n:>5}");
    }
    Ok(())
}

/// List the models whose loader-idle sequence is not file slot 0 and is a rest pose the content
/// gate leaves unarmed, with the emitters (`FX`) and batches (`ALPHA`) that differ at t = 0
/// between the two slots. The reference arms the idle on every instance at load (`0x70ebd0`'s
/// tail), so a per-sequence bake follows it, never slot 0 (`DuelingFlag.m2`'s slot 0 is Spawn).
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
        // The content gate (`benilla_assets`' `idle_pose_differs`) arms no rig for a rest pose.
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
            continue; // the idle is slot 0: nothing differs
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

/// List the models whose sequences drive per-sequence consumers (emitters, ribbons, material
/// alpha and colour, UV transforms) while no sequence keys a bone, so their sequence clock cannot
/// hang on bone curves. `[GO]` marks a `GameObjectDisplayInfo` model, whose emitters run on the
/// host's clock (`EmitClock::Host`); models where only some sequences key a bone are `PARTIAL`.
pub fn seqclockscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    // The models a GameObject can display, whose emitters run on the host's clock.
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
        // Whether a sequence poses a bone, as `build_animation_clip`'s flag decides it.
        let keyed = seqs
            .iter()
            .filter(|a| {
                a.bones.iter().any(|b| {
                    !b.translation.is_empty() || !b.rotation.is_empty() || !b.scale.is_empty()
                })
            })
            .count();
        // Counted over every model with sequences, consumers or not.
        if keyed > 0 && keyed < seqs.len() {
            partial += 1;
        }
        let Ok(s) = benilla_formats::parse_m2_animation_summary(&bytes) else {
            continue;
        };
        // Everything that samples a track on the playing sequence's clock; a constant (≤1 key)
        // colour or alpha track reads the same in every slot and is left out.
        let consumers = s.particle_emitter_count
            + s.ribbon_emitter_count
            + s.color_alpha_tracks.1
            + s.color_rgb_tracks.1
            + s.transparency_tracks.1
            + s.texture_transform_count;
        if keyed == 0 && consumers == 0 {
            // No keyed bone and nothing to sample: a clock here is inert.
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
         {frozen_go} of them GameObject display models. {partial} PARTIAL models key a bone in \
         some sequences only; {inert} more key no bone and have nothing to sample."
    );
    Ok(())
}

/// Census the `$DSL` (loop), `$DSE` (its release), `$DSO` and `$SND` (one-shots) markers per model,
/// the `SoundEntries` kit each names, and whether the carrying sequence is a rest pose (`REST`),
/// which gets no rig ([`benilla_formats::ModelAnimation::is_rest_pose`]), so only a rig-free clock
/// reaches its marker (a lamp's Stand keys no bone yet carries its `$DSL` hum). `$DSE` has an arm
/// in the placed-doodad handler `0x6951e0` and none in the GameObject dispatcher `0x5f3e20`.
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
        // The spawn tier `benilla_world::doodad_anim::classify` gives this model: `Static` when
        // boneless, else `FirstSeq` when the idle is not a rest pose, else `GlobalSeqOnly` with
        // global-sequence channels, else `Static`.
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
            // Asked of the sequence that carries the marker: its clock must run to reach it.
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
         {hostless} of them on the Static tier (the marker alone arms their clock); \
         {rest_gated} carry the marker on a REST-posed sequence, which gets no rig, so only the \
         rig-free sequence clock reaches it."
    );
    Ok(())
}

/// The sound-slot names of the ten `GameObjectDisplayInfo.Sound[n]` columns, in column order.
const GO_SLOT_NAMES: [&str; 10] = [
    "Stand", "Open", "Loop", "Close", "Destroy", "Opened", "Custom0", "Custom1", "Custom2",
    "Custom3",
];

/// The event tag that plays each display slot through `0x5f3e20`: `$GO0..5` → slots 0..5,
/// `$GC0..3` → slots 6..9.
fn go_slot_tag(slot: usize) -> [u8; 4] {
    if slot < 6 {
        [b'$', b'G', b'O', b'0' + slot as u8]
    } else {
        [b'$', b'G', b'C', b'0' + (slot - 6) as u8]
    }
}

/// The file slots the GameObject arm can put on screen: the loader seed (`0x71019b`), the six
/// state substates through the four-way remap (`0x5f3972`), and the Custom0-3 (opcode `0xb3`) and
/// Despawn (`0x215`) one-shots, resolved through `playableAnimationLookup`. Spawn, which the
/// create path arms (`0x5f382f`), is left out, so a marker only its sequence carries reads as
/// unarmable.
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

/// Census the GameObject display sound slots against the `$GOn`/`$GCn` model events, the only
/// path that plays them (`0x5f4010`, called from `0x5f3e20` alone).
pub fn goslotscan(chain: &mut Chain) -> Result<()> {
    let catalog =
        benilla_formats::load_gameobject_catalog(chain).context("GameObjectDisplayInfo.dbc")?;
    let sounds =
        benilla_formats::load_gameobject_sounds(chain).context("GameObjectDisplayInfo.dbc")?;
    let kits = benilla_formats::load_sound_kit_catalog(chain).ok();
    // Model path → the displays that name it, so each model is parsed once.
    let mut per_model: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (id, path) in catalog.iter() {
        if sounds.slots(id).is_some() {
            per_model.entry(model_key(path)).or_default().push(id);
        }
    }
    for ids in per_model.values_mut() {
        ids.sort_unstable();
    }
    // Per slot: displays with a kit, of those with the tag authored, of those on an armable
    // sequence, and the live ones whose kit loops (flag 0x200), which `0x5f4010` plays through
    // the emitter pool (`0x461d80`) rather than as a one-shot.
    let mut filled = [0u32; 10];
    let mut tagged = [0u32; 10];
    let mut reachable = [0u32; 10];
    let mut looping = [0u32; 10];
    // Filled columns whose model authors no reaching tag: dead data the reference never plays.
    let mut dead = [0u32; 10];
    // Tags authored where the column is 0: the tag fires and `0x458830` fails on the null id.
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
