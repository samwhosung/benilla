//! M2 ribbon emitters (weapon trails, wisp streamers, missile trails), read from raw bytes in the
//! reference's layout (relocation fixup `0x71ef40`):
//!
//! ```text
//! header array : count @ MD20+0x134, ptr @ MD20+0x138        record stride 0xdc
//! +0x04 boneIndex(u16)  +0x08 position(C3Vector, bone-local)
//! +0x14 textureIndices(M2Array<u16> → M2 textures)  +0x1c materialIndices(M2Array<u16> → the
//!       M2 render-flags table @MD20+0x84, stride 4: flags u16, blend u16)
//! +0x24 colorTrack(M2Track<C3Vector>)  +0x40 alphaTrack(M2Track<fixed16>)
//! +0x5c heightAboveTrack(M2Track<f32>)  +0x78 heightBelowTrack(M2Track<f32>)
//! +0x94 edgesPerSecond(f32)  +0x98 edgeLifetime(f32, clamped ≥ 0.25)  +0x9c gravity(f32)
//! +0xa0 textureRows(u16)  +0xa2 textureCols(u16)
//! +0xa4 texSlotTrack(M2Track<u16>)  +0xc0 visibilityTrack(M2Track<u8>)
//! ```
//!
//! Color, alpha and the two heights are keyed tracks rebased onto the first sequence's band;
//! `texSlot` is its first value, constant in the shipped files. The visibility track sets the
//! enable byte `block+0xbc` (ctor `0x71b34c` 0, loader `0x70f80e` 1, then every frame at
//! `0x7176ee`/`0x717714` in `0x714260`; nothing else writes it, and `0x718960` only reads it), and
//! a cleared byte skips the ribbon's whole draw (`0x7080c2`, `0x708263`). It keys inside a
//! sequence: `G_FrostTrap.m2` lights its upper streamers 200 ms into `Custom0`.
//!
//! The reference samples a single-key track once and latches it, where this resamples every frame
//! (the same for a constant track), and windows keys by the emitter bone's active sequence, where
//! this uses the model's (the same wherever the bone plays the model's sequence).

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

/// The first value of a 0x1c-byte M2Track (values count @ +0x14, offset @ +0x18), or `default`.
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

/// One M2 ribbon emitter, as the renderer needs it.
#[derive(Debug, Clone)]
pub struct RibbonEmitterDef {
    pub bone: u16,
    /// The origin in the host bone's frame, times the bone matrix every frame (`0x718960`).
    pub position: [f32; 3],
    /// `textureIndices[0]`'s path; `None` if unresolved, and the ribbon is skipped.
    pub texture: Option<String>,
    /// `materialIndices[0]`'s render-flags blend, folded into a `ParticleBlend`.
    pub blend: ParticleBlend,
    /// The raw blend behind the lossy [`Self::blend`] fold, when the material resolves.
    pub blend_mode: Option<u16>,
    /// Trail tint (colorTrack, RGB 0..1, keyed on the clip clock; constant white when unkeyed).
    pub color: ValueTrack<[f32; 3]>,
    /// Trail opacity (alphaTrack, int16 / 32767; constant 1.0 when unkeyed).
    pub alpha: ValueTrack,
    /// Half-widths (yards) above and below the path; each edge keeps the width it was born with.
    pub height_above: ValueTrack,
    pub height_below: ValueTrack,
    /// Edges per second; with `edge_lifetime`, sizes the ring.
    pub edges_per_second: f32,
    /// Edge lifetime in seconds, clamped to at least 0.25 at load as the reference does.
    pub edge_lifetime: f32,
    /// Downward sag applied to live edges (the reference's `2·g·dt` per-frame term).
    pub gravity: f32,
    /// Texture atlas tiling (1×1 is the whole texture) and the slot index.
    pub tile_rows: u16,
    pub tile_cols: u16,
    pub tex_slot: u16,
    /// The `+0xc0` enable track, `None` when always on; otherwise [`RibbonVisibility::at`] is
    /// sampled on the host's playing sequence every frame.
    pub visible: Option<RibbonVisibility>,
}

/// The `+0xc0` enable track per `AnimationData.dbc` id: step keys in band-local seconds, the
/// first being the value the band opens on.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonVisibility {
    by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>>,
}

impl RibbonVisibility {
    /// Whether the trail emits `t` seconds into `anim`, stepped. An unauthored sequence takes
    /// Stand's answer, and without one the load default, on (`0x70f80e`).
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

/// The `+0xc0` gate track as a [`RibbonVisibility`]; `None` when keyless, on a global-sequence
/// clock, or on everywhere. Sequences: MD20 count @ `0x1c`, offset @ `0x20`, stride `0x44`, with
/// `anim_id` @ +0 and the band's start and end @ +4 and +8.
fn visibility_by_anim(bytes: &[u8], vis_track: usize) -> Option<RibbonVisibility> {
    if vis_track + 0x1c > bytes.len() {
        return None;
    }
    // A global-sequence clock is not per animation: treated as always on.
    if le_u16(bytes, vis_track + 0x02) != 0xffff {
        return None;
    }
    let tn = le_u32(bytes, vis_track + 0x0c) as usize;
    let tofs = le_u32(bytes, vis_track + 0x10) as usize;
    let vn = le_u32(bytes, vis_track + 0x14) as usize;
    let vofs = le_u32(bytes, vis_track + 0x18) as usize;
    let n = tn.min(vn);
    if n == 0 || tofs + n * 4 > bytes.len() || vofs + n > bytes.len() {
        return None; // keyless: the always-on default
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
        // The band opens on the nearest-previous key, or the first key before any.
        let opening = keys
            .iter()
            .take_while(|&&(t, _)| t <= start)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, v)| v);
        let mut band: Vec<(f32, bool)> = vec![(0.0, opening)];
        // Then each change strictly inside the band, band-local.
        for &(t, v) in keys.iter().filter(|&&(t, _)| t > start && t <= end) {
            let local = (t - start) as f32 / 1000.0;
            if band.last().is_some_and(|&(_, prev)| prev != v) {
                band.push((local, v));
            }
        }
        any_off |= band.iter().any(|&(_, v)| !v);
        by_anim.entry(anim).or_insert(band); // variations share an id; the first wins
    }
    // Never dark: no gate.
    any_off.then_some(RibbonVisibility { by_anim })
}

/// An M2's ribbon emitters; empty when it has none or is not a parseable M2.
pub fn parse_m2_ribbon_emitters(bytes: &[u8]) -> Result<Vec<RibbonEmitterDef>> {
    if bytes.len() < HDR_PTR + 4 || &bytes[0..4] != b"MD20" {
        return Ok(Vec::new());
    }
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
    // `count` sizes the reservation, so a hostile one is refused; 256 is far past any shipped
    // model, and checking it first keeps `count * STRIDE` from overflowing.
    if count == 0 || count > 256 || base + count * STRIDE > bytes.len() {
        return Ok(Vec::new());
    }
    let rf_count = le_u32(bytes, HDR_RENDER_FLAGS) as usize;
    let rf_base = le_u32(bytes, HDR_RENDER_FLAGS + 4) as usize;
    // The first sequence's band, which the keyed look tracks rebase onto, as particle tracks do.
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
        // Shipped ribbons author Add (4) 527 times, Alpha (2) 51 and Mod2x (6) 12
        // (`benilla-extract ribbonscan`). `ParticleBlend` has no Mod2x, so those 12 fold to
        // `Opaque` and draw unblended, where the reference multiplies.
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
            // fix16 keys are signed (`movsx` at `0x717933`); the draw clamps a negative to 0.
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

    #[test]
    fn a_hostile_ribbon_count_yields_nothing_not_an_abort() {
        let mut b = vec![0u8; HDR_PTR + 4];
        b[0..4].copy_from_slice(b"MD20");
        b[HDR_COUNT..HDR_COUNT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
        // A plausible count whose table the file cannot hold is refused too.
        b[HDR_COUNT..HDR_COUNT + 4].copy_from_slice(&3u32.to_le_bytes());
        b[HDR_PTR..HDR_PTR + 4].copy_from_slice(&((HDR_PTR + 4) as u32).to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
    }

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
        assert!(!vis.at(153, 0.0));
        assert!(!vis.at(153, 0.199));
        assert!(vis.at(153, 0.2), "step: the key's value takes effect AT it");
        assert!(vis.at(153, 1.399));
        assert!(!vis.at(153, 1.4));
        assert!(!vis.at(147, 0.0), "unlisted sequence borrows Stand");
        let no_stand = RibbonVisibility {
            by_anim: [(153u16, vec![(0.0, false)])].into_iter().collect(),
        };
        assert!(no_stand.at(147, 0.0), "no answer at all ⇒ the load default");
    }

    /// Smite's impact slash is two ribbons keyed inside the band [3300, 4433]: height 0 to 0.167
    /// and back over 267 ms, alpha 1 to 0 by 467 ms.
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
            assert_eq!(r.height_above.first(), 0.0, "value[0] bake = no slash");
            assert!((r.height_above.peak() - 0.167).abs() < 1e-3);
            assert!((r.height_above.sample_ms(200.0) - 0.167).abs() < 1e-3);
            assert_eq!(r.height_above.sample_ms(400.0), 0.0, "collapsed by 267 ms");
            assert_eq!(r.height_below.keys, r.height_above.keys, "symmetric slash");
            assert_eq!(r.alpha.sample_ms(0.0), 1.0);
            assert!(r.alpha.sample_ms(600.0) < 1e-6);
            // Rebased: every key is inside the 1133 ms clip.
            assert!(r.height_above.keys.iter().all(|&(t, _)| t <= 1133));
            assert_eq!(r.color.first(), [1.0; 3]);
            // Always on, its alpha doing the fade: no gate to darken it against Stand.
            assert_eq!(
                r.visible, None,
                "an always-on slash carries no visibility gate"
            );
        }
    }

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

    /// `G_FrostTrap.m2`: four low ribbons light 534 ms into Spawn and stay lit through Closed;
    /// twelve upper ones are dark at rest and light 200 ms into `Custom0`, when the trap springs.
    #[test]
    fn real_frost_trap_upper_streamers_light_only_inside_the_trigger() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Goober\\G_FrostTrap.m2")
            .expect("read G_FrostTrap.m2");
        let defs = parse_m2_ribbon_emitters(&bytes).expect("parse ribbons");
        assert_eq!(defs.len(), 16, "the frost trap authors sixteen trails");

        // Ribbons 0..4: key 867 inside Spawn's 333..1067 band.
        for r in &defs[0..4] {
            let v = r.visible.as_ref().expect("the low swirl IS gated");
            assert!(!v.at(145, 0.0), "Spawn opens dark");
            assert!(v.at(145, 0.6), "…and lights 534 ms in");
            assert!(v.at(147, 0.0), "Closed: lit — the placed trap's swirl");
        }
        // Ribbons 4..16: key 4200 inside the trigger's 4000..5400 band.
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
