//! Corpus scans over **particle and ribbon emitters** — which of the emitter system's features
//! the shipped data actually authors, and which shapes a consumer can get wrong.
//!
//! The feature census (`partcensus`), the flipbook ramps (`cellscan`), the per-slot emission
//! windows (`partslotscan`), the 3-D geometry-model shards (`shardcensus`), and the draw-order
//! population effects share with their owner's own transparent batches (`fxordercensus`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use benilla_formats::Chain;

use crate::model_key;

/// Sweep every `.m2` in the chain and list the models carrying RIBBON emitters, with each
/// model's `+0xc0` **enable-gate** census beside the count — the population instrument for
/// "which trails are state-dependent, and where does the gate actually key".
///
/// A `GATED` line names the sequences a trail is dark in; `MID-SEQ` marks the models whose gate
/// flips *inside* a band rather than only at its start, which a band-start-only reader cannot
/// express (decision 1011 — `G_FrostTrap`'s streamers light 200 ms into the trigger and nowhere
/// else, so a band-start read says "never" and an ungated consumer says "always").
pub fn ribbonscan(chain: &mut Chain) -> Result<()> {
    let names = super::m2_names(chain, None)?;
    let (mut scanned, mut hits, mut gated, mut mid_seq) = (0u32, 0u32, 0u32, 0u32);
    let mut blends: BTreeMap<String, u32> = BTreeMap::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let n = benilla_formats::m2_ribbon_emitter_count(&bytes);
        if n == 0 {
            continue;
        }
        hits += 1;
        let defs = benilla_formats::parse_m2_ribbon_emitters(&bytes).unwrap_or_default();
        // Per ribbon: the sequences it can be dark in, and whether the gate keys mid-band.
        let mut notes: Vec<String> = Vec::new();
        let (mut any_gate, mut any_mid) = (false, false);
        for (i, d) in defs.iter().enumerate() {
            let Some(vis) = &d.visible else { continue };
            any_gate = true;
            let mut per: Vec<String> = vis
                .per_anim()
                .map(|(anim, keys)| {
                    any_mid |= keys.len() > 1;
                    let states: Vec<String> = keys
                        .iter()
                        .map(|&(t, on)| format!("{t:.3}s{}", if on { "+" } else { "-" }))
                        .collect();
                    format!("{anim}:{}", states.join(","))
                })
                .collect();
            per.sort();
            notes.push(format!("    r{i}  {}", per.join("  ")));
        }
        // The BLEND census beside the gate one. A ribbon takes its blend from a real M2 material
        // (`materialIndices[0]` → render-flags `blend`), so unlike a particle emitter it can in
        // principle author any mode — and the two the parse folds (`Opaque`, covering file modes
        // 0/1/5/6) render with no alpha test at all, which is exactly what painted the particle
        // side's AlphaKey debris as solid squares. Tallied here so the fold stays a MEASURED
        // approximation instead of an assumed-empty one.
        for d in &defs {
            let raw = d.blend_mode.map_or("?".into(), |m| m.to_string());
            *blends
                .entry(format!("{:?}({raw})", d.blend))
                .or_insert(0u32) += 1;
        }
        gated += u32::from(any_gate);
        mid_seq += u32::from(any_mid);
        let tag = match (any_gate, any_mid) {
            (true, true) => "  [GATED MID-SEQ]",
            (true, false) => "  [GATED]",
            _ => "",
        };
        println!("{n:>2}  {name}{tag}");
        for line in notes {
            println!("{line}");
        }
    }
    let blend_tally: Vec<String> = blends.iter().map(|(b, n)| format!("{b} {n}")).collect();
    eprintln!(
        "{scanned} models scanned, {hits} with ribbons — {gated} carry an enable gate, \
         {mid_seq} of those key it MID-SEQUENCE (a band-start-only read misses their whole \
         ON window).  Blends: {}",
        blend_tally.join(" · ")
    );
    Ok(())
}

/// Sweep every `.m2` (under `prefix`, if given) and list the models whose particle emitters
/// carry any bit of `mask` in their file flags — see the `Partscan` command doc. One line per
/// matching emitter: its index, full flag word, shape/type, and the model path.
pub fn partscan(chain: &mut Chain, mask: u32, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut hits, mut emitters) = (0u32, 0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let Ok(defs) = benilla_formats::parse_m2_particle_emitters(&bytes) else {
            continue;
        };
        let mut hit = false;
        for (i, d) in defs.iter().enumerate() {
            if d.flags & mask != 0 {
                hit = true;
                emitters += 1;
                println!(
                    "e{i} flags {:#010x}  {:?} {}  {name}",
                    d.flags,
                    d.shape,
                    match d.head_tail {
                        0 => "head",
                        1 => "tail",
                        _ => "head+tail",
                    },
                );
            }
        }
        if hit {
            hits += 1;
        }
    }
    eprintln!(
        "{scanned} models scanned, {hits} with mask {mask:#x} emitters ({emitters} emitters)"
    );
    Ok(())
}

/// The census's raw-extras view of one on-disk emitter record: the fields the shipped
/// [`benilla_formats::ParticleEmitterDef`] deliberately does not carry (yet). Read straight off
/// the record bytes (stride/header per the parser's module doc): the two model-filename M2Arrays
/// at `+0x18` (geometry model — 3-D "model particles") and `+0x20` (recursion model — per-particle
/// child emitters), and the emission-rate track's interpolation word (`+0xdc`, 0 = step).
struct RecordExtras {
    geometry_model: Option<String>,
    recursion_model: Option<String>,
    rate_interp: u16,
    rate_keys: u32,
}

/// Read the raw extras for every emitter record in an M2 (empty if not an MD20 or no emitters).
fn record_extras(bytes: &[u8]) -> Vec<RecordExtras> {
    const STRIDE: usize = 0x1f8;
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let str_at = |count: usize, ofs: usize| -> Option<String> {
        if count == 0 || ofs == 0 || ofs + count > bytes.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&bytes[ofs..ofs + count])
            .trim_end_matches('\0')
            .to_string();
        (!s.is_empty()).then_some(s)
    };
    if bytes.len() < 0x144 || &bytes[..4] != b"MD20" {
        return Vec::new();
    }
    let count = u32_at(0x13c) as usize;
    let base = u32_at(0x140) as usize;
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let e = base + i * STRIDE;
            RecordExtras {
                geometry_model: str_at(u32_at(e + 0x18) as usize, u32_at(e + 0x1c) as usize),
                recursion_model: str_at(u32_at(e + 0x20) as usize, u32_at(e + 0x24) as usize),
                rate_interp: u16_at(e + 0xdc),
                rate_keys: u32_at(e + 0xdc + 0x14),
            }
        })
        .collect()
}

/// model key → the spells whose visual chain plays that model (any kit stage, or the missile).
fn spell_attribution(chain: &mut Chain) -> HashMap<String, Vec<(u32, String)>> {
    let (Ok(spells), Ok(visuals)) = (
        benilla_formats::load_spell_catalog(chain),
        benilla_formats::load_spell_visual_catalog(chain),
    ) else {
        eprintln!("(spell/visual DBCs unavailable — census runs without spell attribution)");
        return HashMap::new();
    };
    let mut map: HashMap<String, Vec<(u32, String)>> = HashMap::new();
    for (id, sp) in spells.iter() {
        let Some(st) = visuals.stages(sp.visual) else {
            continue;
        };
        let mut push = |effect: u32| {
            if effect != 0 {
                if let Some(p) = visuals.effect_path(effect) {
                    map.entry(crate::model_key(p))
                        .or_default()
                        .push((id, sp.name.clone()));
                }
            }
        };
        for kit_id in [st.precast, st.cast, st.impact, st.state, st.channel] {
            if kit_id == 0 {
                continue;
            }
            if let Some(kit) = visuals.kit(kit_id) {
                for (_, effect) in kit.effects() {
                    push(effect);
                }
            }
        }
        push(st.missile_model);
    }
    map
}

/// One census dimension's tally. `model_count` counts every distinct model exactly;
/// `models` keeps the first 64 (sorted) for the example listings.
#[derive(Default)]
struct Tally {
    emitters: u32,
    model_count: u32,
    last_model: String,
    models: std::collections::BTreeSet<String>,
}

impl Tally {
    /// The walk visits each model's emitters consecutively, so "new model" is a change of name.
    fn hit(&mut self, model: &str) {
        self.emitters += 1;
        if self.last_model != model {
            self.model_count += 1;
            self.last_model = model.to_string();
        }
        if self.models.len() < 64 {
            self.models.insert(model.to_string());
        }
    }
}

/// Sweep every `.m2` (under `prefix`, if given) and census which particle-emitter FEATURES the
/// corpus actually authors — see the `Partcensus` command doc.
pub fn partcensus(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let attribution = spell_attribution(chain);
    let names = super::m2_names(chain, prefix)?;

    let mut tallies: std::collections::BTreeMap<&'static str, Tally> =
        std::collections::BTreeMap::new();
    let (mut scanned, mut with_emitters, mut total_emitters) = (0u32, 0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let Ok(defs) = benilla_formats::parse_m2_particle_emitters(&bytes) else {
            continue;
        };
        if defs.is_empty() {
            continue;
        }
        with_emitters += 1;
        let extras = record_extras(&bytes);
        let key = crate::model_key(&name);
        for (i, d) in defs.iter().enumerate() {
            total_emitters += 1;
            let mut hit = |k: &'static str| tallies.entry(k).or_default().hit(&key);
            match d.shape {
                benilla_formats::ParticleShape::Plane => hit("shape:plane"),
                benilla_formats::ParticleShape::Sphere => hit("shape:sphere"),
                benilla_formats::ParticleShape::Spline => hit("shape:SPLINE"),
            }
            // ANISOTROPIC plane rectangles — the population on which the areaLength↔areaWidth axis
            // pairing is observable at all (a SQUARE area renders identically either way). 0563 and
            // 0566 both pre-named "a 90°-wrong ANISOTROPIC effect" as this lane's suspect, but no
            // instrument could LIST that population, so the swapped pairing outlived both audits
            // until Gressil's 0.1 × 1.1 blade smoke drew its curtain across the blade. Bucketed by
            // aspect so a thin curtain (load-bearing) separates from a near-square (invisible).
            let now = d.params.sample(None, 0.0, 0.0);
            if d.shape == benilla_formats::ParticleShape::Plane {
                let lo = now.area_length.abs().min(now.area_width.abs());
                let hi = now.area_length.abs().max(now.area_width.abs());
                if hi > 1e-4 {
                    let aspect = if lo > 1e-4 { hi / lo } else { f32::INFINITY };
                    if aspect >= 4.0 {
                        hit("plane-area:ANISOTROPIC >=4:1");
                    } else if aspect >= 1.5 {
                        hit("plane-area:anisotropic >=1.5:1");
                    }
                }
            }
            // ANIMATED parameter channels (decision 0844): the population the value[0] flatten
            // silently mis-rendered — Frost Nova's emission radius riding its ring out, Arcane
            // Explosion's riding its dome. Tallied per channel so the census names WHICH knob
            // actually moves in the corpus.
            const PARAM_KEYS: [&str; 9] = [
                "param-anim:speed",
                "param-anim:speedVar",
                "param-anim:latitude",
                "param-anim:longitude",
                "param-anim:gravity",
                "param-anim:lifespan",
                "param-anim:areaLength",
                "param-anim:areaWidth",
                "param-anim:zSource",
            ];
            for (c, (_, slots)) in d.params.channel_views().iter().enumerate() {
                if slots
                    .iter()
                    .any(|k| k.is_some_and(|k| k.len() > 1 && k.iter().any(|&(_, v)| v != k[0].1)))
                {
                    hit(PARAM_KEYS[c]);
                }
            }
            match d.head_tail {
                0 => hit("type:head"),
                1 => hit("type:tail"),
                _ => hit("type:head+tail"),
            }
            match d.blend {
                benilla_formats::ParticleBlend::Add => hit("blend:add"),
                benilla_formats::ParticleBlend::Alpha => hit("blend:alpha"),
                benilla_formats::ParticleBlend::AlphaKey => hit("blend:alphakey"),
                benilla_formats::ParticleBlend::Opaque => hit("blend:opaque"),
            }
            // The raw blend byte disambiguates what the parsed enum folds: 5 = Mod, 6 = Mod2x.
            {
                const STRIDE: usize = 0x1f8;
                let base = u32::from_le_bytes(bytes[0x140..0x144].try_into().unwrap()) as usize;
                match bytes[base + i * STRIDE + 0x28] {
                    5 => hit("blend:MOD(5)"),
                    6 => hit("blend:MOD2X(6)"),
                    _ => {}
                }
            }
            if d.flags & 0x1 == 0 {
                hit("flag:LIT(0x1 clear)");
            }
            for (bit, label) in [
                (0x8u32, "flag:0x8 texenv"),
                (0x10, "flag:0x10 model-space"),
                (0x20, "flag:0x20 scale-by-instance"),
                (0x40, "flag:0x40 MOTION-VEL-INHERIT"),
                (0x80, "flag:0x80 KILL-OUTBOUND"),
                (0x100, "flag:0x100 sphere-up"),
                (0x200, "flag:0x200"),
                (0x400, "flag:0x400 tail-age-clamp"),
                (0x800, "flag:0x800 SPAWN-PATH-SPREAD"),
                (0x1000, "flag:0x1000 xy-quad"),
                (0x2000, "flag:0x2000 GROUND-SNAP"),
                (0x4000, "flag:0x4000 FOLLOW-DELTA"),
                (0x8000, "flag:0x8000 burst"),
            ] {
                if d.flags & bit != 0 {
                    hit(label);
                }
            }
            if d.flags >> 16 != 0 {
                hit("flag:HIGH-BITS(>0xffff)");
            }
            if d.spin > 0.0 {
                hit("spin:positive");
            }
            if d.spin < 0.0 {
                hit("spin:negative");
            }
            if d.twinkle_percent < 1.0 {
                hit("twinkle:percent<1");
            }
            if u32::from(d.tile_cols) * u32::from(d.tile_rows) > 1 {
                hit("tiles:atlas");
                if !d.tile_cols.is_power_of_two() {
                    hit("tiles:NON-POW2-COLS");
                }
            }
            if now.z_source != 0.0 {
                hit("kernel:zSource");
            }
            if now.gravity < 0.0 {
                hit("kernel:negative-gravity");
            }
            if d.shape == benilla_formats::ParticleShape::Sphere
                && now.vertical_range > 3.0
                && now.horizontal_range == 0.0
            {
                hit("kernel:edge-on-ring(lat±π,lon0)");
            }
            if let Some(x) = extras.get(i) {
                if x.geometry_model.is_some() {
                    hit("MODEL-PARTICLES(geometry)");
                }
                if x.recursion_model.is_some() {
                    hit("CHILD-EMITTERS(recursion)");
                }
                if x.rate_keys > 1 && x.rate_interp != 0 {
                    hit("rate:LERP-RAMP(interp!=0)");
                    // A BURST emitter with a lerp rate track would arm a near-zero count at its
                    // rising edge — if the corpus authored one, the burst edge law needs a re-look.
                    if d.burst() {
                        hit("rate:BURST+LERP(suspect)");
                    }
                }
            }
        }
    }

    println!(
        "== particle feature census  prefix={}  ({scanned} models scanned, {with_emitters} with emitters, {total_emitters} emitters)",
        prefix.unwrap_or("<all>"),
    );
    for (k, t) in &tallies {
        let ex: Vec<&str> = t.models.iter().take(3).map(|s| s.as_str()).collect();
        println!(
            "{:>6} emitters  {:>4} models  {k}  e.g. {}",
            t.emitters,
            t.model_count,
            ex.join(" · ")
        );
    }

    // The full model list — with spell attribution — for the dimensions that decide mechanism
    // scope (the UPPERCASE keys: unimplemented or folded legs, plus the odd corners).
    let detail: &[&str] = &[
        "MODEL-PARTICLES(geometry)",
        "CHILD-EMITTERS(recursion)",
        "shape:SPLINE",
        "blend:MOD(5)",
        "blend:MOD2X(6)",
        "flag:LIT(0x1 clear)",
        "flag:0x40 MOTION-VEL-INHERIT",
        "flag:0x80 KILL-OUTBOUND",
        "flag:0x800 SPAWN-PATH-SPREAD",
        "flag:0x2000 GROUND-SNAP",
        "flag:0x4000 FOLLOW-DELTA",
        "flag:HIGH-BITS(>0xffff)",
        "tiles:NON-POW2-COLS",
        "rate:LERP-RAMP(interp!=0)",
    ];
    for k in detail {
        let Some(t) = tallies.get(k) else { continue };
        println!();
        println!(
            "=== {k}  ({} emitters, {} models)",
            t.emitters, t.model_count
        );
        for m in t.models.iter().take(16) {
            let spells = attribution.get(m).map_or(String::new(), |v| {
                let mut seen = HashSet::new();
                let names: Vec<String> = v
                    .iter()
                    .filter(|(id, _)| seen.insert(*id))
                    .take(3)
                    .map(|(id, n)| format!("{id} {n}"))
                    .collect();
                if names.is_empty() {
                    String::new()
                } else {
                    format!(
                        "   [{}{}]",
                        names.join(", "),
                        if v.len() > 3 { ", …" } else { "" }
                    )
                }
            });
            println!("  {m}{spells}");
        }
        if t.model_count as usize > 16 {
            println!("  … {} more", t.model_count as usize - 16);
        }
    }
    Ok(())
}

/// Sweep every `.m2` and census its particle emitters' over-life **flipbook** fields — the
/// population instrument behind decision 0685 (the reverse-playing cell ramp).
///
/// One line per emitter that is interesting on any axis, then the totals. The axes are exactly the
/// ways a flipbook reader can be wrong, each of which shipped data does exercise:
///
/// - `INVERTED` — a `begin > end` pair. Legal, and it means *play the sheet backwards*; a reader
///   that clamps into `[begin, end]` mangles it (and in Rust panics outright).
/// - `TAIL-RAMP` — the tail streak's own ramp differs from the head's, on an emitter that draws a
///   tail. The two are independently authored (file +0x168.. vs +0x174..); handing the head's cell
///   to the streak animates it through a sheet the author pinned to one cell.
/// - `PAST-ATLAS` — a cell index beyond `rows·cols`. The reference masks the COLUMN and leaves the
///   ROW unbounded, so the index wraps to row 0 rather than holding the last cell.
/// - `REPEAT` — a per-segment repeat count ≠ 1, i.e. the sheet cycles more than once per segment.
/// - `NON-POW2` / `MID` — the two shapes the reference itself degrades on (a 1×1 fallback, and a
///   `mid` of 0/1 that walks its own sampler into a NaN). Both are empty in 1.12.1 and are here so
///   that stays checkable.
pub fn cellscan(chain: &mut Chain) -> Result<()> {
    let names = super::m2_names(chain, None)?;

    let (mut models, mut emitters) = (0u32, 0u32);
    let (mut inverted, mut tail_ramp, mut repeat_ne1) = (0u32, 0u32, 0u32);
    let (mut past_atlas, mut past_atlas_real, mut bad_tiles, mut bad_mid) =
        (0u32, 0u32, 0u32, 0u32);

    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        models += 1;
        let Ok(defs) = benilla_formats::parse_m2_particle_emitters(&bytes) else {
            continue;
        };
        for (i, e) in defs.iter().enumerate() {
            emitters += 1;
            let ol = &e.over_life;
            let at = format!("{name} [{i}] {}x{}", e.tile_rows, e.tile_cols);
            let ramps = [
                ol.head_cells[0],
                ol.head_cells[1],
                ol.tail_cells[0],
                ol.tail_cells[1],
            ];
            let pair = |r: &benilla_formats::CellRamp| (r.begin, r.end);

            if ramps.iter().any(|r| r.begin > r.end) {
                inverted += 1;
                println!(
                    "INVERTED   {at} head {:?}/{:?}",
                    pair(&ol.head_cells[0]),
                    pair(&ol.head_cells[1])
                );
            }
            // Only a tail-drawing emitter (particleType 1/2) can show a tail-ramp difference.
            if e.head_tail >= 1 && ol.tail_cells != ol.head_cells {
                tail_ramp += 1;
                println!(
                    "TAIL-RAMP  {at} head {:?}/{:?} tail {:?}/{:?}",
                    pair(&ol.head_cells[0]),
                    pair(&ol.head_cells[1]),
                    pair(&ol.tail_cells[0]),
                    pair(&ol.tail_cells[1])
                );
            }
            if ol.repeat.iter().any(|&r| r != 1.0) {
                repeat_ne1 += 1;
                println!("REPEAT     {at} {:?}", ol.repeat);
            }
            let atlas = e.tile_rows * e.tile_cols;
            if ramps.iter().any(|r| r.begin >= atlas || r.end >= atlas) {
                past_atlas += 1;
                // On a 1×1 sheet every index resolves to the same texture, so only a real atlas
                // can show the wrap.
                if atlas > 1 {
                    past_atlas_real += 1;
                    println!(
                        "PAST-ATLAS {at} head {:?}/{:?}",
                        pair(&ol.head_cells[0]),
                        pair(&ol.head_cells[1])
                    );
                }
            }
            if !e.tile_rows.is_power_of_two() || !e.tile_cols.is_power_of_two() {
                bad_tiles += 1;
                println!("NON-POW2   {at}");
            }
            if !(ol.mid > 0.0 && ol.mid < 1.0) {
                bad_mid += 1;
                println!("MID        {at} mid {}", ol.mid);
            }
        }
    }

    eprintln!(
        "{models} models / {emitters} emitters scanned\n  \
         INVERTED    {inverted}\n  \
         TAIL-RAMP   {tail_ramp}\n  \
         REPEAT      {repeat_ne1}\n  \
         PAST-ATLAS  {past_atlas} ({past_atlas_real} on a real >1x1 atlas)\n  \
         NON-POW2    {bad_tiles}\n  \
         MID 0 or 1  {bad_mid}"
    );
    Ok(())
}

/// Sweep every `.m2` (optionally under a path prefix) and list the emitters whose **file slot 0 is
/// dead while another slot is alive** — the shape that makes a pinned-slot-0 consumer silently
/// render nothing (decision 0760, found on `BlastedLandsLightningbolt01.m2`, B63).
///
/// The reference samples the **playing** sequence's rate window every frame (`0x71850d`); a
/// consumer that pins slot 0 instead is only correct while
/// slot 0 carries the emitter's whole story. When an author keys the burst in a *later* variation —
/// a lightning strike that fires on 5 % of arms, an ambient prop with a rare flourish — slot 0 is a
/// flat zero and the pinned consumer emits nothing, for ever, on every placement. That is invisible
/// from the outside: the emitter is built, pooled, and ticking; it just never births a particle.
///
/// `peak0` is slot 0's own peak rate, `peakN` the best any other slot reaches. A listed emitter has
/// `peak0 <= 0 < peakN`. `slots` counts FILE sequence slots (the axis `EmitTiming` bakes on).
pub fn partslotscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut with_emitters, mut emitters, mut dead0) = (0u32, 0u32, 0u32, 0u32);
    let mut models = 0u32;
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let Ok(defs) = benilla_formats::parse_m2_particle_emitters(&bytes) else {
            continue;
        };
        if defs.is_empty() {
            continue;
        }
        with_emitters += 1;
        let mut lines = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            emitters += 1;
            let views = d.timing.slot_views();
            if views.len() < 2 {
                continue; // one slot ⇒ nothing for a slot pick to get wrong
            }
            // Peak of a slot's baked rate keys; an unkeyed slot emits nothing (`None` ⇒ rate 0).
            let peak = |s: usize| -> f32 {
                views
                    .get(s)
                    .and_then(|v| v.1)
                    .map(|keys| keys.iter().map(|&(_, v)| v).fold(0.0, f32::max))
                    .unwrap_or(0.0)
            };
            let peak0 = peak(0);
            let (mut best, mut best_slot) = (0.0f32, 0usize);
            for s in 1..views.len() {
                if peak(s) > best {
                    best = peak(s);
                    best_slot = s;
                }
            }
            if peak0 <= 0.0 && best > 0.0 {
                dead0 += 1;
                lines.push(format!(
                    "    emitter {i:>2}: slots {:>2}  peak0 {peak0:>7.1}  peakN {best:>7.1} \
                     @ slot {best_slot}  tex {}",
                    views.len(),
                    d.texture.as_deref().unwrap_or("NONE"),
                ));
            }
        }
        if !lines.is_empty() {
            models += 1;
            println!("{name}");
            for l in lines {
                println!("{l}");
            }
        }
    }
    eprintln!(
        "{scanned} models scanned, {with_emitters} with emitters, {emitters} emitter(s): \
         {dead0} DEAD-IN-SLOT-0 across {models} model(s) — these emit nothing at all under a \
         pinned-slot-0 consumer"
    );
    Ok(())
}

/// Sweep every `.m2` (optionally under a path prefix) and census the corpus's **3-D model
/// particles**: the emitters whose record carries a geometry-model reference (`+0x18` — a shard
/// instance renders that GEOMETRY model's own submeshes, not a billboard quad). Per OWNER model:
/// the owner-last draw-order rung its shards are stamped with at runtime
/// (`owner_last_rung(m2_owner_reach(owner submeshes))`); per referenced GEOMETRY model: the
/// material family tuples its render submeshes author — (blend, two_sided, additive,
/// no_depth_write, no_depth_test), the exact key the shard's material is built from.
///
/// This is the **ground truth for the pipeline-warm menagerie's shard rows**: warming a pipeline
/// per (rung × family) cell is only worth its table if the corpus's cells are enumerable, and this
/// census is what enumerates them — the rung histogram sizes the rung axis, the family-tuple set
/// (split by transparent-pass membership, the renderer's own test) sizes the material axis, and
/// the unresolved-path count bounds what the table can never cover.
pub fn shardcensus(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;

    // A submesh's material family: the tuple the shard material is keyed by, rendered compactly
    // (`Blend+2s+add`), plus its transparent-pass membership — the renderer's own test, where
    // `additive` forces the pass whatever the authored blend says (`model_material`).
    let fam = |s: &benilla_formats::RenderSubmesh| {
        let transparent = s.additive
            || matches!(
                s.blend,
                benilla_formats::ModelBlend::Blend
                    | benilla_formats::ModelBlend::Mod
                    | benilla_formats::ModelBlend::Mod2x
            );
        let label = format!(
            "{:?}{}{}{}{}",
            s.blend,
            if s.two_sided { "+2s" } else { "" },
            if s.additive { "+add" } else { "" },
            if s.no_depth_write { "+ndw" } else { "" },
            if s.no_depth_test { "+ndt" } else { "" },
        );
        (label, transparent)
    };

    // Each distinct geometry model is read, parsed, and tallied exactly once; the cache keeps its
    // (family, transparent) list per submesh for the per-pair lines. `None` = unresolvable.
    let mut geo_cache: BTreeMap<String, Option<Vec<(String, bool)>>> = BTreeMap::new();
    let mut fam_transparent: BTreeMap<String, u32> = BTreeMap::new();
    let mut fam_opaque: BTreeMap<String, u32> = BTreeMap::new();
    let mut rungs: BTreeMap<u32, u32> = BTreeMap::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut parse_failures: Vec<(String, String)> = Vec::new();
    let (mut scanned, mut owners) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        if benilla_formats::parse_m2_particle_emitters(&bytes).is_err() {
            continue;
        }
        // Distinct geometry references, normalized the way the chain resolves model paths
        // (lowercase, `\` separators, `.mdx`/`.mdl` → `.m2` — `model_key` mirrors the crate's own
        // `model_path`).
        let geos: BTreeSet<String> = record_extras(&bytes)
            .iter()
            .filter_map(|e| e.geometry_model.as_deref())
            .map(|p| model_key(&p.replace('/', "\\")))
            .collect();
        if geos.is_empty() {
            continue;
        }
        owners += 1;
        // The rung the owner's shards are stamped with: the renderer's own bound and rung, at
        // placement scale 1 (a scaled placement multiplies the reach).
        let dir = name.rsplit_once('\\').map_or("", |(d, _)| d);
        let owner_subs =
            benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]).unwrap_or_default();
        let reach = benilla_formats::m2_owner_reach(&owner_subs);
        let rung = benilla_formats::owner_last_rung(reach);
        *rungs.entry(rung as u32).or_default() += 1;
        for geo in geos {
            if !geo_cache.contains_key(&geo) {
                let parsed = match chain.read_file(&geo) {
                    Ok(gb) => {
                        let gdir = geo.rsplit_once('\\').map_or("", |(d, _)| d);
                        match benilla_formats::parse_m2_render_submeshes(&gb, gdir, &[]) {
                            Ok(gsubs) => {
                                let fams: Vec<(String, bool)> = gsubs.iter().map(&fam).collect();
                                for (label, transparent) in &fams {
                                    let tally = if *transparent {
                                        &mut fam_transparent
                                    } else {
                                        &mut fam_opaque
                                    };
                                    *tally.entry(label.clone()).or_default() += 1;
                                }
                                Some(fams)
                            }
                            Err(e) => {
                                parse_failures.push((geo.clone(), e.to_string()));
                                None
                            }
                        }
                    }
                    Err(_) => {
                        unresolved.push(geo.clone());
                        None
                    }
                };
                geo_cache.insert(geo.clone(), parsed);
            }
            let fams = match &geo_cache[&geo] {
                Some(fams) if fams.is_empty() => "(no submeshes)".to_string(),
                Some(fams) => fams
                    .iter()
                    .map(|(l, _)| l.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", "),
                None => "UNRESOLVED".to_string(),
            };
            println!("rung {rung:>2.0}  reach {reach:>8.2}  {name} -> {geo}  [{fams}]");
        }
    }

    eprintln!(
        "{scanned} models scanned\n  \
         {owners} own a geometry-model (3-D shard) emitter\n  \
         {} distinct geometry models referenced\n  \
         {} geometry paths unresolvable in the chain",
        geo_cache.len(),
        unresolved.len()
    );
    eprintln!("  owner rung histogram (placement scale 1):");
    for (rung, n) in &rungs {
        eprintln!("    rung {rung:>2}  {n:>5}");
    }
    eprintln!("  geometry family tuples — TRANSPARENT pass (submeshes across distinct models):");
    for (label, n) in &fam_transparent {
        eprintln!("    {label:<24} {n:>5}");
    }
    eprintln!("  geometry family tuples — opaque/mask pass:");
    for (label, n) in &fam_opaque {
        eprintln!("    {label:<24} {n:>5}");
    }
    if !unresolved.is_empty() {
        eprintln!("  unresolvable geometry paths:");
        for p in &unresolved {
            eprintln!("    {p}");
        }
    }
    if !parse_failures.is_empty() {
        eprintln!("  geometry parse failures:");
        for (p, e) in &parse_failures {
            eprintln!("    {p}: {e}");
        }
    }
    Ok(())
}

/// Sweep every `.m2` (optionally under a path prefix) and count, per model, the two halves of the
/// **owner-last draw-order** law: the EFFECTS a model authors (particle emitters + ribbon trails)
/// and the TRANSPARENT-pass batches of its own body those effects must draw after (decisions
/// 0719/0721). A model with both is one the rung actually changes; a model with effects and no
/// transparent batch of its own never had the defect and is listed only in the totals.
///
/// This is the population instrument the two decisions were argued from. "Does this fix anything
/// besides the voidwalker's eyes?" is not a question to answer by naming plausible creatures —
/// it is a count, and the count is what says whether the mechanism closes a class or a case.
pub fn fxordercensus(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    // Per top-level content family (Creature / Item / Spells / World / …), so the totals say
    // WHERE the class lives rather than only how big it is.
    let mut family: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let (mut scanned, mut with_fx, mut at_risk) = (0u32, 0u32, 0u32);
    let mut rungs: BTreeMap<u32, u32> = BTreeMap::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let emitters = benilla_formats::parse_m2_particle_emitters(&bytes)
            .map(|e| e.len())
            .unwrap_or(0);
        let trails = benilla_formats::m2_ribbon_emitter_count(&bytes);
        if emitters + trails == 0 {
            continue;
        }
        with_fx += 1;
        // The occluders: batches the renderer puts in the one distance-sorted transparent list.
        // `additive` forces that pass whatever the authored blend says (`model_material`), so the
        // test mirrors the renderer's rather than reading the blend word alone.
        let dir = name.rsplit_once('\\').map_or("", |(d, _)| d);
        let subs = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]).unwrap_or_default();
        let occluders: Vec<&benilla_formats::RenderSubmesh> = subs
            .iter()
            .filter(|s| {
                s.additive
                    || matches!(
                        s.blend,
                        benilla_formats::ModelBlend::Blend
                            | benilla_formats::ModelBlend::Mod
                            | benilla_formats::ModelBlend::Mod2x
                    )
            })
            .collect();
        let transparent = occluders.len();
        // The renderer's own bound and the renderer's own rung — not a re-derivation of them.
        let reach = benilla_formats::m2_owner_reach(&subs);
        let fam = name.split('\\').next().unwrap_or("?").to_string();
        family.entry(fam).or_default().0 += 1;
        if transparent == 0 {
            continue; // effects, but nothing of its own that could paint over them
        }
        at_risk += 1;
        family
            .entry(name.split('\\').next().unwrap_or("?").to_string())
            .or_default()
            .1 += 1;
        // At placement scale 1 — the survey number; a scaled placement multiplies the reach.
        let rung = benilla_formats::owner_last_rung(reach);
        *rungs.entry(rung as u32).or_default() += 1;
        println!(
            "{transparent:>3} transp {emitters:>2} emit {trails:>2} ribb  reach {reach:>8.2} \
             rung {rung:>2.0}  {name}"
        );
    }
    eprintln!(
        "{scanned} models scanned\n  \
         {with_fx} author effects (emitters and/or ribbons)\n  \
         {at_risk} of those ALSO author transparent batches of their own — the population the \
         owner-last rung changes"
    );
    eprintln!("  by family (with-effects / at-risk):");
    for (fam, (n, risk)) in &family {
        eprintln!("    {fam:<24} {n:>5} / {risk:>5}");
    }
    eprintln!("  rung distribution (at-risk models, placement scale 1):");
    for (rung, n) in &rungs {
        eprintln!("    rung {rung:>2}  {n:>5}");
    }
    Ok(())
}
