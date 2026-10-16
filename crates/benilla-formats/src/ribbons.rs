//! M2 ribbon emitters (weapon trails, wisp streamers, missile trails), read from raw bytes in the
//! reference's layout (relocation fixup `0x71ef40`), version-aware through [`M2Layout`]:
//!
//! ```text
//! header array : count @ lay.ribbon, ptr @ lay.ribbon+4      stride 0xdc pre-Wrath, 0xb0 in Wrath
//! +0x04 boneIndex(u16)  +0x08 position(C3Vector, bone-local)
//! +0x14 textureIndices(M2Array<u16> → M2 textures)  +0x1c materialIndices(M2Array<u16> → the
//!       M2 render-flags table @lay.materials, stride 4: flags u16, blend u16)
//! then, `lay.track_stride` apart: colorTrack(M2Track<C3Vector>), alphaTrack(M2Track<fixed16>),
//!       heightAboveTrack(M2Track<f32>), heightBelowTrack(M2Track<f32>) — +0x24/+0x40/+0x5c/+0x78
//!       pre-Wrath, +0x24/+0x38/+0x4c/+0x60 in Wrath
//! edgesPerSecond(f32)  edgeLifetime(f32, clamped ≥ 0.25)  gravity(f32)
//! textureRows(u16)  textureCols(u16)  texSlotTrack(M2Track<u16>)  visibilityTrack(M2Track<u8>)
//! ```
//!
//! In a Wrath file the tracks are the 20-byte nested shape (each sequence its own key clip), and
//! the record closes with the 3.3.5 trailer `PriorityPlane`, `RibbonColorIndex` and
//! `TextureTransformLookupIndex` — four bytes benilla does not read. The template annotates the
//! WotLK colour and alpha tracks `sT`/`V` (a flat `sTimestamp` list); benilla reads them in the
//! file-wide Wrath nested shape instead — the structure every other 3.3.5 track carries, and the
//! one the axe's bone tracks prove. The shipped axe authors no ribbons, so this stands on the
//! template's field order alone.
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
use benilla_m2::{parse_m2, M2Layout};

use crate::value_track::{seq0_band, track_first, track_keys_with, ValueTrack};
use crate::ParticleBlend;

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
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
/// clock, or on everywhere. Sequences: count @ `lay.sequences`, offset @ +4, records
/// `lay.sequence_stride` apart with `anim_id` @ +0: pre-Wrath the band's start and end sit @ +4
/// and +8, a Wrath record owns a clock (`length` @+4) and its band is the accumulated one.
fn visibility_by_anim(bytes: &[u8], vis_track: usize, lay: &M2Layout) -> Option<RibbonVisibility> {
    if vis_track + 4 > bytes.len() {
        return None;
    }
    // A global-sequence clock is not per animation: treated as always on.
    if le_u16(bytes, vis_track + 0x02) != 0xffff {
        return None;
    }
    // Absolute keys: pre-Wrath the flat u32-ms list, Wrath the per-sequence clips rebased by
    // each band start.
    let keys: Vec<(u32, bool)> = if lay.wrath {
        let band_starts = benilla_m2::band_starts_for(bytes, lay);
        benilla_m2::track_read_wrath(
            bytes,
            vis_track,
            1,
            |b, o| b.get(o).map(|v| *v != 0),
            &band_starts,
        )
        .keys
    } else {
        if vis_track + 0x1c > bytes.len() {
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
        (0..n)
            .map(|i| (le_u32(bytes, tofs + i * 4), bytes[vofs + i] != 0))
            .collect()
    };
    if keys.is_empty() {
        return None;
    }

    let (nseq, oseq) = lay.array_at(bytes, lay.sequences)?;
    let mut by_anim: std::collections::HashMap<u16, Vec<(f32, bool)>> =
        std::collections::HashMap::new();
    let mut any_off = false;
    let mut band_acc = 0u32;
    for i in 0..nseq as usize {
        let s = oseq as usize + i * lay.sequence_stride;
        if s + lay.sequence_stride > bytes.len() {
            break;
        }
        let anim = le_u16(bytes, s);
        // Pre-Wrath records carry `start`/`end` on the shared timeline; a Wrath record owns a
        // clock (`length` @+4), so its band is the accumulated one.
        let (start, end) = if lay.wrath {
            let start = band_acc;
            let end = start.saturating_add(le_u32(bytes, s + 4));
            band_acc = end;
            (start, end)
        } else {
            let start = le_u32(bytes, s + 4);
            band_acc = start;
            (start, le_u32(bytes, s + 8))
        };
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
    if bytes.len() < 8 || &bytes[0..4] != b"MD20" {
        return Ok(Vec::new());
    }
    let Some(lay) = M2Layout::of(bytes) else {
        return Ok(Vec::new());
    };
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
    let Some((count, base)) = lay.array_at(bytes, lay.ribbon) else {
        return Ok(Vec::new());
    };
    let (count, base) = (count as usize, base as usize);
    // `count` sizes the reservation, so a hostile one is refused; 256 is far past any shipped
    // model, and checking it first keeps `count * STRIDE` from overflowing.
    if count == 0 || count > 256 || base.saturating_add(count * lay.ribbon_stride) > bytes.len() {
        return Ok(Vec::new());
    }
    let Some((rf_count, rf_base)) = lay.array_at(bytes, lay.materials) else {
        return Ok(Vec::new());
    };
    let (rf_count, rf_base) = (rf_count as usize, rf_base as usize);
    // The first sequence's band, which the keyed look tracks rebase onto, as particle tracks do.
    let band = seq0_band(bytes, &lay);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = base + i * lay.ribbon_stride;
        if e + lay.ribbon_stride > bytes.len() {
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
        // The four keyed tracks ride `lay.track_stride` apart from +0x24; the fixed fields
        // follow the last, and the slot and visibility tracks the tiles (module note).
        let t = lay.track_stride;
        let (color_ofs, alpha_ofs) = (0x24, 0x24 + t);
        let (habove_ofs, hbelow_ofs) = (alpha_ofs + t, alpha_ofs + 2 * t);
        let eps_ofs = hbelow_ofs + t;
        let (rows_ofs, cols_ofs) = (eps_ofs + 12, eps_ofs + 14);
        let (texslot_ofs, vis_ofs) = (cols_ofs + 2, cols_ofs + 2 + t);
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
            color: track_keys_with(bytes, e + color_ofs, [1.0; 3], &lay, band, 12, |b, o| {
                [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
            }),
            // fix16 keys are signed (`movsx` at `0x717933`); the draw clamps a negative to 0.
            alpha: track_keys_with(bytes, e + alpha_ofs, 1.0, &lay, band, 2, |b, o| {
                f32::from(le_u16(b, o) as i16) / 32767.0
            }),
            height_above: track_keys_with(bytes, e + habove_ofs, 0.0, &lay, band, 4, le_f32),
            height_below: track_keys_with(bytes, e + hbelow_ofs, 0.0, &lay, band, 4, le_f32),
            edges_per_second: le_f32(bytes, e + eps_ofs),
            edge_lifetime: le_f32(bytes, e + eps_ofs + 4).max(0.25),
            gravity: le_f32(bytes, e + eps_ofs + 8),
            tile_rows: le_u16(bytes, e + rows_ofs).max(1),
            tile_cols: le_u16(bytes, e + cols_ofs).max(1),
            tex_slot: track_first(bytes, e + texslot_ofs, 2, 0, le_u16, &lay),
            visible: visibility_by_anim(bytes, e + vis_ofs, &lay),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hostile_ribbon_count_yields_nothing_not_an_abort() {
        let lay = M2Layout::from_version(256).expect("vanilla is supported");
        let mut b = vec![0u8; 0x144];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&256u32.to_le_bytes());
        b[lay.ribbon..lay.ribbon + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).unwrap().is_empty());
        // A plausible count whose table the file cannot hold is refused too.
        b[lay.ribbon..lay.ribbon + 4].copy_from_slice(&3u32.to_le_bytes());
        b[lay.ribbon + 4..lay.ribbon + 8].copy_from_slice(&0x144u32.to_le_bytes());
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

    /// A minimal v264 file: one 1000 ms sequence, one Add material, one ribbon whose colour
    /// keys at 200 ms and whose trail lights 500 ms into the band.
    fn wrath_ribbon_fixture() -> Vec<u8> {
        let mut b = vec![0u8; 0x600];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&264u32.to_le_bytes());
        let w16 = |b: &mut [u8], o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let wf = |b: &mut [u8], o: usize, v: f32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // sequences @ 0x1c/0x20 → one 0x40 record at 0x300, length 1000.
        w32(&mut b, 0x1c, 1);
        w32(&mut b, 0x20, 0x300);
        w16(&mut b, 0x300, 0);
        w32(&mut b, 0x304, 1000);
        // materials @ 0x70/0x74 → one {flags 0, blend 4 (Add)} at 0x290.
        w32(&mut b, 0x70, 1);
        w32(&mut b, 0x74, 0x290);
        w16(&mut b, 0x292, 4);
        // ribbon @ 0x120/0x124 → one 0xb0 record at 0x400.
        w32(&mut b, 0x120, 1);
        w32(&mut b, 0x124, 0x400);
        w16(&mut b, 0x404, 3); // bone
        w32(&mut b, 0x414, 1);
        w32(&mut b, 0x418, 0x4e0); // textureIndices[0]
        w32(&mut b, 0x41c, 1);
        w32(&mut b, 0x420, 0x4e2); // materialIndices[0]
                                   // colour track @0x424: one clip, one key (200 ms, (0.5, 0.5, 1.0)).
        w16(&mut b, 0x424, 1);
        w16(&mut b, 0x426, 0xffff);
        w32(&mut b, 0x428, 1);
        w32(&mut b, 0x42c, 0x4e4);
        w32(&mut b, 0x430, 1);
        w32(&mut b, 0x434, 0x4ec);
        // alpha track @0x438: one key (0 ms, fix16 32767 = opaque).
        w16(&mut b, 0x438, 1);
        w16(&mut b, 0x43a, 0xffff);
        w32(&mut b, 0x43c, 1);
        w32(&mut b, 0x440, 0x51c);
        w32(&mut b, 0x444, 1);
        w32(&mut b, 0x448, 0x524);
        // heightAbove @0x44c / heightBelow @0x460: one key each (0 ms, 0.1 / 0.2).
        w16(&mut b, 0x44c, 1);
        w16(&mut b, 0x44e, 0xffff);
        w32(&mut b, 0x450, 1);
        w32(&mut b, 0x454, 0x53c);
        w32(&mut b, 0x458, 1);
        w32(&mut b, 0x45c, 0x544);
        w16(&mut b, 0x460, 1);
        w16(&mut b, 0x462, 0xffff);
        w32(&mut b, 0x464, 1);
        w32(&mut b, 0x468, 0x554);
        w32(&mut b, 0x46c, 1);
        w32(&mut b, 0x470, 0x55c);
        // The fixed fields.
        wf(&mut b, 0x474, 10.0); // edgesPerSecond
        wf(&mut b, 0x478, 0.05); // edgeLifetime → clamped to 0.25
        wf(&mut b, 0x47c, 0.0);
        w16(&mut b, 0x480, 1);
        w16(&mut b, 0x482, 1);
        // texSlot track @0x484: one key (0 ms, slot 7).
        w16(&mut b, 0x484, 1);
        w16(&mut b, 0x486, 0xffff);
        w32(&mut b, 0x488, 1);
        w32(&mut b, 0x48c, 0x56c);
        w32(&mut b, 0x490, 1);
        w32(&mut b, 0x494, 0x574);
        // visibility track @0x498: one clip (the one sequence), keys (0 ms, off), (500 ms, on).
        w16(&mut b, 0x498, 1);
        w16(&mut b, 0x49a, 0xffff);
        w32(&mut b, 0x49c, 1);
        w32(&mut b, 0x4a0, 0x582);
        w32(&mut b, 0x4a4, 1);
        w32(&mut b, 0x4a8, 0x58a);
        // The data.
        w16(&mut b, 0x4e0, 1);
        w16(&mut b, 0x4e2, 0);
        w32(&mut b, 0x4e4, 1);
        w32(&mut b, 0x4e8, 0x500);
        w32(&mut b, 0x4ec, 1);
        w32(&mut b, 0x4f0, 0x510);
        w32(&mut b, 0x500, 200);
        wf(&mut b, 0x510, 0.5);
        wf(&mut b, 0x514, 0.5);
        wf(&mut b, 0x518, 1.0);
        w32(&mut b, 0x51c, 1);
        w32(&mut b, 0x520, 0x530);
        w32(&mut b, 0x524, 1);
        w32(&mut b, 0x528, 0x538);
        w32(&mut b, 0x530, 0);
        w16(&mut b, 0x538, 32767);
        w32(&mut b, 0x53c, 1);
        w32(&mut b, 0x540, 0x54c);
        w32(&mut b, 0x544, 1);
        w32(&mut b, 0x548, 0x550);
        w32(&mut b, 0x54c, 0);
        wf(&mut b, 0x550, 0.1);
        w32(&mut b, 0x554, 1);
        w32(&mut b, 0x558, 0x564);
        w32(&mut b, 0x55c, 1);
        w32(&mut b, 0x560, 0x568);
        w32(&mut b, 0x564, 0);
        wf(&mut b, 0x568, 0.2);
        w32(&mut b, 0x56c, 1);
        w32(&mut b, 0x570, 0x57c);
        w32(&mut b, 0x574, 1);
        w32(&mut b, 0x578, 0x580);
        w32(&mut b, 0x57c, 0);
        w16(&mut b, 0x580, 7);
        // Outer ts @0x582: (2 keys, list @0x594); outer val @0x58a: (2, @0x59c).
        w32(&mut b, 0x582, 2);
        w32(&mut b, 0x586, 0x594);
        w32(&mut b, 0x58a, 2);
        w32(&mut b, 0x58e, 0x59c);
        w32(&mut b, 0x594, 0);
        w32(&mut b, 0x598, 500);
        b[0x59c] = 0;
        b[0x59d] = 1;
        M2Layout::of(&b).expect("a v264 layout");
        b
    }

    #[test]
    fn a_wrath_ribbon_reads_its_nested_tracks() {
        let b = wrath_ribbon_fixture();
        let defs = parse_m2_ribbon_emitters(&b).expect("parse the wrath ribbon");
        assert_eq!(defs.len(), 1);
        let r = &defs[0];
        assert_eq!(r.bone, 3);
        assert_eq!(r.blend, ParticleBlend::Add, "material 0 is Add");
        assert_eq!(r.blend_mode, Some(4));
        // The colour key survives the band rebase at its own 200 ms.
        assert_eq!(r.color.keys, vec![(200u32, [0.5, 0.5, 1.0])]);
        assert_eq!(r.alpha.keys, vec![(0u32, 1.0)]);
        assert_eq!(r.height_above.keys, vec![(0u32, 0.1)]);
        assert_eq!(r.height_below.keys, vec![(0u32, 0.2)]);
        assert_eq!(r.edges_per_second, 10.0);
        assert_eq!(r.edge_lifetime, 0.25, "clamped as the reference does");
        assert_eq!(r.tile_rows, 1);
        assert_eq!(r.tile_cols, 1);
        assert_eq!(r.tex_slot, 7, "the first clip's first value");
        // The trail is dark at the band's start and lights at its 500 ms key.
        let v = r.visible.as_ref().expect("the trail is visibility-gated");
        assert!(!v.at(0, 0.0), "opens dark");
        assert!(!v.at(0, 0.499));
        assert!(v.at(0, 0.5), "…and lights 500 ms in");
        assert!(v.at(0, 0.999), "…and stays lit");
    }
}
