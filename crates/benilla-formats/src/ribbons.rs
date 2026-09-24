//! Vanilla (build 5875 / MD20 v256) M2 **ribbon emitter** parsing — the weapon trails, wisp
//! streamers, and spell-missile trails. Read straight from raw bytes, like `particles`.
//!
//! Byte layout is the reference's, read off the MD20 relocation fixup `0x71ef40` + the
//! ctor/render reads:
//!
//! ```text
//! header array : count @ MD20+0x134, ptr @ MD20+0x138        record stride 0xdc
//! +0x04 boneIndex(u16)  +0x08 position(C3Vector, bone-local)
//! +0x14 textureIndices(M2Array<u16> → M2 textures)  +0x1c materialIndices(M2Array<u16> → the
//!       M2 render-flags table @MD20+0x84, stride 4: flags u16 + blend u16 — the blend source)
//! +0x24 colorTrack(M2Track<C3Vector>)  +0x40 alphaTrack(M2Track<fixed16>)
//! +0x5c heightAboveTrack(M2Track<f32>)  +0x78 heightBelowTrack(M2Track<f32>)
//! +0x94 edgesPerSecond(f32)  +0x98 edgeLifetime(f32, clamped ≥ 0.25)  +0x9c gravity(f32)
//! +0xa0 textureRows(u16)  +0xa2 textureCols(u16)
//! +0xa4 texSlotTrack(M2Track<u16>)  +0xc0 visibilityTrack(M2Track<u8>)
//! ```
//!
//! The **look tracks** (color, alpha, heightAbove/Below) parse as full keyed tracks through the
//! shared [`crate::value_track`] kernel, band-rebased like the particle emission tracks — the
//! one-shot spell effects key them: `HolySmite_Low_Chest.m2` flares its slash ribbons' height
//! `0 → 0.167 → 0` over 267 ms and fades their alpha `1 → 0`, so the old value[0] bake read a
//! permanent zero height and Smite's impact slash never drew (the 0141 lesson, surfaced).
//! `texSlot` stays value[0]-baked (constant in the shipped corpus).
//!
//! The **visibility** gate track (`+0xc0`) parses as [`RibbonVisibility`]: the keys of every
//! sequence, band-local, sampled **step** (a `u8` track is never blended, so both of the
//! reference's sampler arms copy the same raw `values[k0]`). It is the source of the enable byte
//! `block+0xbc` — ctor `0x71b34c` = 0, loader default `0x70f80e` = 1, then written every frame
//! from the sampled value at `0x7176ee` / `0x717714` inside `0x714260`; `0x718960` only reads it,
//! and no equipment/attach/sheathe writer exists anywhere in the binary. Clearing it **kills the
//! whole ribbon's draw** (`0x7080c2` → the collect loop's continue at `0x708263`). Settled for
//! decision 1017; decision 1013 had unwound the whole mechanism on the strength of a *guessed*
//! equip/attach route.
//!
//! The thrown weapon keys its flight trail OFF in Stand (worn in the hand) and Impact (landed),
//! ON only in InFlight. Keeping the keys — rather than the old band-start bool per sequence — is
//! not tidiness: shipped ribbons **do** key the gate mid-sequence. `G_FrostTrap.m2` lights its
//! low rig 534 ms into Spawn (`867` inside the `333..1067` band) and its twelve upper streamers
//! 200 ms into `Custom0` (`4200` inside `4000..5400`) — so a placed trap shows the low rig alone
//! and the upper twelve fire only when it springs.
//!
//! **NOT transcribed**, both narrow, both recorded so the next reader knows they are open: the
//! reference gates re-sampling on `timestamps.count > Model+0x8c`, which is 0 on an instance's
//! first frame and 1 after — so a **single-key** track samples once and then latches, where we
//! re-answer every frame (same result for a constant track, which is that whole population). And
//! its key window is `interpolationRanges[rangeIdx]` with `rangeIdx` the **emitter's own bone's**
//! active slot, not the model's, `lo >= hi` returning `lo` unsearched; we window by the playing
//! sequence's band, which agrees wherever the ribbon's bone plays what the model plays.

use std::io::Cursor;

use anyhow::Result;
use benilla_m2::parse_m2;

use crate::value_track::{seq0_band, track_keys_with, ValueTrack};
use crate::ParticleBlend;

const STRIDE: usize = 0xdc;
const HDR_COUNT: usize = 0x134;
const HDR_PTR: usize = 0x138;
/// The M2 render-flags (materials) table: `{flags u16, blend u16}` per entry.
const HDR_RENDER_FLAGS: usize = 0x84;

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// value[0] of a vanilla 0x1c M2Track whose values have `elem_size` bytes, decoded by `read`.
/// Track tail: values `{count @ +0x14, offset @ +0x18}`. `default` when empty/out-of-range.
fn track_first<T>(
    b: &[u8],
    track: usize,
    elem_size: usize,
    default: T,
    read: impl Fn(&[u8], usize) -> T,
) -> T {
    if track + 0x1c > b.len() {
        return default;
    }
    let n = le_u32(b, track + 0x14);
    let ofs = le_u32(b, track + 0x18) as usize;
    if n == 0 || ofs + elem_size > b.len() {
        return default;
    }
    read(b, ofs)
}

/// One parsed vanilla M2 ribbon emitter, reduced to what the renderer needs. Positions are
/// **bone-local** (WoW axes) — the runtime transforms through the host bone's live matrix each
/// frame, exactly like a particle emitter's origin.
#[derive(Debug, Clone)]
pub struct RibbonEmitterDef {
    pub bone: u16,
    /// Emission origin in the host bone's frame (the reference multiplies it by the bone matrix
    /// per frame — `0x718960`).
    pub position: [f32; 3],
    /// Trail texture (`.blp` path via `textureIndices[0]` → the M2 textures table). `None` if
    /// unresolved (the consumer skips the ribbon).
    pub texture: Option<String>,
    /// Blend mode from `materialIndices[0]` → the M2 render-flags entry's blend u16 (the same
    /// EGxBlend folding as particle/batch blends).
    pub blend: ParticleBlend,
    /// The RAW authored blend u16 behind [`Self::blend`] (`None` when the material index does not
    /// resolve). The fold is lossy, so a census cannot say *which* mode a folded ribbon authored
    /// without it — and re-walking the material array downstream is how the two readings drift.
    pub blend_mode: Option<u16>,
    /// Trail tint (colorTrack, RGB 0..1, keyed on the clip clock; constant white when unkeyed).
    pub color: ValueTrack<[f32; 3]>,
    /// Trail opacity (alphaTrack, **int16** fix16/32767, keyed — the slash's fade-out; constant
    /// 1.0 when unkeyed).
    pub alpha: ValueTrack,
    /// Cross-section half-widths (yards) above/below the bone path, keyed — the slash's
    /// flare-and-collapse. Sampled at edge-commit time (each edge keeps the width it was born
    /// with; the reference stores the vertex pair per edge).
    pub height_above: ValueTrack,
    pub height_below: ValueTrack,
    /// Edge spawn rate (edges/second) — with `edge_lifetime`, sizes the ring.
    pub edges_per_second: f32,
    /// Edge age-out (seconds) — the reference clamps to ≥ 0.25 at load; so do we.
    pub edge_lifetime: f32,
    /// Downward sag applied to live edges (the reference's `2·g·dt` per-frame term).
    pub gravity: f32,
    /// Texture atlas tiling (1×1 = the whole texture) + the (baked) slot index.
    pub tile_rows: u16,
    pub tile_cols: u16,
    pub tex_slot: u16,
    /// The `+0xc0` **enable** track, per sequence and keyed within it (see the module doc).
    /// `None` = no gate — keyless, global-seq-clocked, or ON everywhere (the always-on majority:
    /// enchant trails, wisps). `Some(v)` = the trail goes dark somewhere, and the consumer must
    /// sample [`RibbonVisibility::at`] against the sequence its host is playing, every frame.
    pub visible: Option<RibbonVisibility>,
}

/// A ribbon's `+0xc0` enable track, resolved per sequence: for each `AnimationData.dbc` id the
/// model authors, the gate's step keys inside that sequence's band, rebased to **band-local
/// seconds**. Entry 0 of each list is the value the band opens on (the nearest-previous key at
/// the band start), so a sample never has to look outside the sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonVisibility {
    by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>>,
}

impl RibbonVisibility {
    /// Whether the trail emits during `anim` at `t` seconds into the clip. Step
    /// (nearest-previous) — the track's own `interp == 0`, which is what every shipped gate
    /// carries. A sequence this model doesn't author falls back to `Stand`(0)'s answer, then to
    /// the reference's load default: **enabled** (`0x70f80e` writes `block+0xbc = 1`).
    pub fn at(&self, anim: u16, t: f32) -> bool {
        let Some(keys) = self.by_anim.get(&anim).or_else(|| self.by_anim.get(&0)) else {
            return true;
        };
        keys.iter()
            .take_while(|&&(kt, _)| kt <= t)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, on)| on)
    }

    /// Every `(anim id, keys)` pair, for instruments that census the gate.
    pub fn per_anim(&self) -> impl Iterator<Item = (u16, &[(f32, bool)])> {
        self.by_anim.iter().map(|(&a, k)| (a, k.as_slice()))
    }
}

/// Parse a ribbon's `+0xc0` gate track (`M2Track<u8>`, sequence-timeline) into a
/// [`RibbonVisibility`]: per `anim_id`, the keys that fall inside that sequence's band, rebased to
/// band-local seconds, preceded by the nearest-previous value at the band start. `None` when there
/// is nothing to gate:
///
/// - a keyless / out-of-range track (the reference's enabled-at-load default), or
/// - a **global-sequence** clock (`gseq != 0xffff`) — a free-running loop, not per-sequence, or
/// - a track that resolves ON at every key of every sequence (a keyed-but-always-on author).
///
/// Sequences: MD20 count @ `0x1c`, offset @ `0x20`, stride `0x44` — `anim_id` @ +0, band `[start,
/// end]` @ +4/+8 (the same walk as [`crate::value_track::seq0_band`], one sequence wider).
fn visibility_by_anim(bytes: &[u8], vis_track: usize) -> Option<RibbonVisibility> {
    if vis_track + 0x1c > bytes.len() {
        return None;
    }
    // gseq != 0xffff: a global-sequence free clock, not keyed per animation — leave it always-on.
    if le_u16(bytes, vis_track + 0x02) != 0xffff {
        return None;
    }
    let tn = le_u32(bytes, vis_track + 0x0c) as usize;
    let tofs = le_u32(bytes, vis_track + 0x10) as usize;
    let vn = le_u32(bytes, vis_track + 0x14) as usize;
    let vofs = le_u32(bytes, vis_track + 0x18) as usize;
    let n = tn.min(vn);
    if n == 0 || tofs + n * 4 > bytes.len() || vofs + n > bytes.len() {
        return None; // keyless → the always-on default
    }
    let keys: Vec<(u32, bool)> = (0..n)
        .map(|i| (le_u32(bytes, tofs + i * 4), bytes[vofs + i] != 0))
        .collect();

    let nseq = le_u32(bytes, 0x1c) as usize;
    let oseq = le_u32(bytes, 0x20) as usize;
    let mut by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>> =
        std::collections::HashMap::new();
    let mut any_off = false;
    for i in 0..nseq {
        let s = oseq + i * 0x44;
        if s + 0x0c > bytes.len() {
            break;
        }
        let anim = le_u16(bytes, s);
        let (start, end) = (le_u32(bytes, s + 4), le_u32(bytes, s + 8));
        // The band opens on the nearest-previous key (M2 step interpolation); before the first
        // key, the first key's value; keyless-safe default ON.
        let opening = keys
            .iter()
            .take_while(|&&(t, _)| t <= start)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, v)| v);
        let mut band: Vec<(f32, bool)> = vec![(0.0, opening)];
        // …then every key strictly inside the band, band-local. `G_FrostTrap`'s streamers live
        // entirely here — their whole ON window opens 200 ms into the trigger sequence.
        for &(t, v) in keys.iter().filter(|&&(t, _)| t > start && t <= end) {
            let local = (t - start) as f32 / 1000.0;
            if band.last().is_some_and(|&(_, prev)| prev != v) {
                band.push((local, v));
            }
        }
        any_off |= band.iter().any(|&(_, v)| !v);
        by_anim.entry(anim).or_insert(band); // variations share an id — the head sequence wins
    }
    // Nothing dark anywhere ⇒ effectively always-on: no gate to carry.
    any_off.then_some(RibbonVisibility { by_anim })
}

/// Parse an M2's ribbon emitters (see the module doc). Empty when the model has none or isn't a
/// parseable M2.
pub fn parse_m2_ribbon_emitters(bytes: &[u8]) -> Result<Vec<RibbonEmitterDef>> {
    if bytes.len() < HDR_PTR + 4 || &bytes[0..4] != b"MD20" {
        return Ok(Vec::new());
    }
    // Texture paths from `benilla-m2`'s textures table (same source as particle textures).
    let textures: Vec<Option<String>> = match parse_m2(&mut Cursor::new(bytes)) {
        Ok(fmt) => fmt
            .model()
            .textures
            .iter()
            .map(|t| {
                let f = t
                    .filename
                    .string
                    .to_string_lossy()
                    .trim_end_matches('\0')
                    .to_string();
                (!f.is_empty()).then_some(f)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let count = le_u32(bytes, HDR_COUNT) as usize;
    let base = le_u32(bytes, HDR_PTR) as usize;
    // The particle twin's refusal (`particles.rs`), for the same reason: `count` is a raw header
    // u32 that sizes the reservation below (~250 B per emitter — u32::MAX is a terabyte and an
    // allocator abort), and the table must fit in the file. 256 is far past anything authored:
    // the corpus scan cited below found 590 ribbons over 176 models, a handful per model. It is
    // checked first so `count * STRIDE` cannot overflow.
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Ok(Vec::new());
    }
    let rf_count = le_u32(bytes, HDR_RENDER_FLAGS) as usize;
    let rf_base = le_u32(bytes, HDR_RENDER_FLAGS + 4) as usize;
    // The first sequence's absolute time band — the keyed look tracks rebase onto it
    // (`value_track::seq0_band`; the same window the particle tracks use).
    let band = seq0_band(bytes);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = base + i * STRIDE;
        if e + STRIDE > bytes.len() {
            break;
        }
        // textureIndices[0] → the M2 textures table → path.
        let texture = {
            let n = le_u32(bytes, e + 0x14);
            let ofs = le_u32(bytes, e + 0x18) as usize;
            (n > 0 && ofs + 2 <= bytes.len())
                .then(|| le_u16(bytes, ofs) as usize)
                .and_then(|ti| textures.get(ti).cloned().flatten())
        };
        // materialIndices[0] → render-flags entry → blend u16.
        let blend_mode = {
            let n = le_u32(bytes, e + 0x1c);
            let ofs = le_u32(bytes, e + 0x20) as usize;
            (n > 0 && ofs + 2 <= bytes.len())
                .then(|| le_u16(bytes, ofs) as usize)
                .filter(|&m| m < rf_count && rf_base + m * 4 + 4 <= bytes.len())
                .map(|m| le_u16(bytes, rf_base + m * 4 + 2))
        };
        // The fold, MEASURED against the corpus by `benilla-extract ribbonscan` (176 models, 590
        // ribbons): Add(4) 527 · Alpha(2) 51 · Mod2x(6) 12. Nothing authors 0, 1 or 5, so the
        // AlphaKey arm below is the law rather than a live case — but it is the law, and the
        // particle side proved what folding it into `Opaque` costs.
        //
        // KNOWN GAP, deliberately not fixed here: those 12 Mod2x ribbons fall into `Opaque` and
        // render with no blend at all, while a Mod2x PARTICLE folds the other way (to `Alpha`).
        // `ParticleBlend` cannot express Mod/Mod2x, so straightening it means widening that enum
        // and re-verifying both lanes — its own change, not a rider on the AlphaKey fix.
        let blend = match blend_mode {
            Some(3 | 4) => ParticleBlend::Add,
            Some(2) => ParticleBlend::Alpha,
            Some(1) => ParticleBlend::AlphaKey,
            Some(_) => ParticleBlend::Opaque,
            None => ParticleBlend::Add, // unresolved material: trails are near-always additive
        };
        out.push(RibbonEmitterDef {
            bone: le_u16(bytes, e + 0x04),
            position: [
                le_f32(bytes, e + 0x08),
                le_f32(bytes, e + 0x0c),
                le_f32(bytes, e + 0x10),
            ],
            texture,
            blend,
            blend_mode,
            color: track_keys_with(bytes, e + 0x24, [1.0; 3], band, 12, |b, o| {
                [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
            }),
            // fix16 keys are SIGNED (`movsx` at the alpha site `0x717933` — the same decode as
            // `benilla_m2::track_fix16`; the record's stride-2 values at +0x40 are int16 outright).
            // The strip's own draw clamps a negative to 0 below.
            alpha: track_keys_with(bytes, e + 0x40, 1.0, band, 2, |b, o| {
                f32::from(le_u16(b, o) as i16) / 32767.0
            }),
            height_above: track_keys_with(bytes, e + 0x5c, 0.0, band, 4, le_f32),
            height_below: track_keys_with(bytes, e + 0x78, 0.0, band, 4, le_f32),
            edges_per_second: le_f32(bytes, e + 0x94),
            edge_lifetime: le_f32(bytes, e + 0x98).max(0.25),
            gravity: le_f32(bytes, e + 0x9c),
            tile_rows: le_u16(bytes, e + 0xa0).max(1),
            tile_cols: le_u16(bytes, e + 0xa2).max(1),
            tex_slot: track_first(bytes, e + 0xa4, 2, 0, le_u16),
            visible: visibility_by_anim(bytes, e + 0xc0),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header's ribbon count (0x134) sized the reservation straight off the file — at ~250 B
    /// per `RibbonEmitterDef`, u32::MAX is a terabyte request and an allocator abort, where the
    /// particle twin had refused the same shape all along. Refused the same way now.
    #[test]
    fn a_hostile_ribbon_count_yields_nothing_not_an_abort() {
        let mut b = vec![0u8; HDR_PTR + 4];
        b[0..4].copy_from_slice(b"MD20");
        b[HDR_COUNT..HDR_COUNT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
        // And a table the file cannot hold, at a plausible count, is refused too.
        b[HDR_COUNT..HDR_COUNT + 4].copy_from_slice(&3u32.to_le_bytes());
        b[HDR_PTR..HDR_PTR + 4].copy_from_slice(&((HDR_PTR + 4) as u32).to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
    }

    /// [`RibbonVisibility::at`]'s sampling law, on a hand-built gate: STEP (nearest-previous, the
    /// track's own `interp == 0`), the band-opening entry answers before the first in-band key,
    /// and a sequence the model doesn't author falls back to `Stand`(0) and then to the
    /// reference's enabled-at-load default.
    #[test]
    fn visibility_samples_step_and_falls_back_to_stand() {
        let vis = RibbonVisibility {
            by_anim: [
                (0u16, vec![(0.0, false)]),
                (153u16, vec![(0.0, false), (0.2, true), (1.4, false)]),
            ]
            .into_iter()
            .collect(),
        };
        // Step within the band: dark, lit, dark — and lit right ON the key, not after it.
        assert!(!vis.at(153, 0.0));
        assert!(!vis.at(153, 0.199));
        assert!(vis.at(153, 0.2), "step: the key's value takes effect AT it");
        assert!(vis.at(153, 1.399));
        assert!(!vis.at(153, 1.4));
        // A sequence this gate doesn't list borrows Stand(0)'s answer…
        assert!(!vis.at(147, 0.0), "unlisted sequence borrows Stand");
        // …and with no Stand either, the load default stands: enabled.
        let no_stand = RibbonVisibility {
            by_anim: [(153u16, vec![(0.0, false)])].into_iter().collect(),
        };
        assert!(no_stand.at(147, 0.0), "no answer at all ⇒ the load default");
    }

    /// The real `HolySmite_Low_Chest.m2` (Smite's impact): its yellow slash IS its two ribbon
    /// emitters, whose look tracks are KEYED inside the seq band [3300, 4433] — height flares
    /// `0 → 0.167 → 0` over the first 267 ms and alpha fades `1 → 0` by 467 ms. The old
    /// value[0] bake read a permanent zero height, so the slash never spawned (the director's
    /// "we have the glitter but not the slash"). Pins the band rebase and the keyed samples.
    #[test]
    fn real_holy_smite_slash_ribbons_are_keyed_flares() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Spells\\HolySmite_Low_Chest.m2")
            .expect("read HolySmite_Low_Chest.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 2);
        for r in &defs {
            // Height: born at 0 (the value[0] trap), flaring to 0.167 yd at 200 ms clip time.
            assert_eq!(r.height_above.first(), 0.0, "value[0] bake = no slash");
            assert!((r.height_above.peak() - 0.167).abs() < 1e-3);
            assert!((r.height_above.sample_ms(200.0) - 0.167).abs() < 1e-3);
            assert_eq!(r.height_above.sample_ms(400.0), 0.0, "collapsed by 267 ms");
            assert_eq!(r.height_below.keys, r.height_above.keys, "symmetric slash");
            // Alpha: full through the flare, gone by 467 ms.
            assert_eq!(r.alpha.sample_ms(0.0), 1.0);
            assert!(r.alpha.sample_ms(600.0) < 1e-6);
            // Band rebase happened: every key sits on the 0-based clip clock, inside the
            // 1133 ms clip.
            assert!(r.height_above.keys.iter().all(|&(t, _)| t <= 1133));
            assert_eq!(r.color.first(), [1.0; 3]);
            // Smite's slash is a one-shot flash, not visibility-gated per sequence — the trail is
            // ON wherever it plays (its alpha, above, does the fade). So it must carry NO gate,
            // or the spawn site would wrongly darken it against a Stand default.
            assert_eq!(
                r.visible, None,
                "an always-on slash carries no visibility gate"
            );
        }
    }

    /// The thrown dagger's flight trail is **visibility-gated per sequence** (`+0xc0`): its ribbon
    /// is dark in Stand (worn in the hand) and Impact (landed), lit only in InFlight (flying). This
    /// is the seam the multi-sequence gate closes — the worn item resting in Stand shows no trail;
    /// the InFlight missile does. Pins the per-sequence resolve against the shipped asset.
    #[test]
    fn real_thrown_dagger_trail_is_lit_only_in_flight() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Item\\ObjectComponents\\Weapon\\Thrown_1H_Dagger_A_01.m2")
            .expect("read Thrown_1H_Dagger_A_01.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 1, "the dagger authors one trail ribbon");
        let vis = defs[0]
            .visible
            .as_ref()
            .expect("the thrown dagger's trail IS visibility-gated");
        assert!(!vis.at(0, 0.0), "Stand (worn in hand): dark");
        assert!(vis.at(144, 0.0), "InFlight (flying): lit");
        assert!(!vis.at(191, 0.0), "Impact (landed): dark");
    }

    /// The placed **Frost Trap** — the model that proved the gate is keyed *inside* a sequence,
    /// not constant across it. Its sixteen ribbons split in two: four low
    /// ones on bones 45–48 at model z 0.129 that light 534 ms into Spawn and stay lit through
    /// Closed — the tuft a placed trap shows, riding up off the crown on the verified `+g·t²` —
    /// and twelve upper ones on bones 33–44 at z ≈ 1.55 that are dark in **every** rest state and
    /// light only 200 ms into `Custom0`, when the trap springs.
    ///
    /// A band-start-only read answers "dark" for `Custom0` too, so the upper rig would never fire;
    /// an ungated consumer draws all sixteen for ever, which is half of the tall column the
    /// director reported (the other half was gravity's sign — see `benilla::ribbons`).
    #[test]
    fn real_frost_trap_upper_streamers_light_only_inside_the_trigger() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Goober\\G_FrostTrap.m2")
            .expect("read G_FrostTrap.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 16, "the frost trap authors sixteen trails");

        // Ribbons 0..4: the low swirl at the trap's base. Dark as Spawn opens, lit 534 ms in
        // (key 867 inside the 333..1067 band), and lit for all of Closed — what a placed trap shows.
        for r in &defs[0..4] {
            let v = r.visible.as_ref().expect("the low swirl IS gated");
            assert!(!v.at(145, 0.0), "Spawn opens dark");
            assert!(v.at(145, 0.6), "…and lights 534 ms in");
            assert!(v.at(147, 0.0), "Closed: lit — the placed trap's swirl");
        }
        // Ribbons 4..16: the upper streamers. Dark in every rest state; lit only 200 ms into the
        // trigger (key 4200 inside the 4000..5400 band) — the column, and only when sprung.
        for r in &defs[4..16] {
            let v = r.visible.as_ref().expect("the upper streamers ARE gated");
            assert!(!v.at(147, 0.0), "Closed: dark — no column on a placed trap");
            assert!(!v.at(148, 0.0), "Open: dark");
            assert!(!v.at(153, 0.0), "the trigger OPENS dark");
            assert!(v.at(153, 0.5), "…and lights 200 ms in");
            assert!(!v.at(153, 1.45), "…then goes dark again at the band end");
        }
    }
}
