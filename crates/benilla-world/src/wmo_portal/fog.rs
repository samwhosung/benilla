//! The camera-in-interior **WMO fog** resolve — which MFOG record's fog the room wants this frame.
//!
//! The byte law (selector `0x69de20`, sole caller the scene-fog orchestrator `0x6cee30`): when the
//! camera stands in a WMO, the scene fog crossfades toward the building's **own** MFOG fog — this
//! is why a storm's veil never fogs the Goldshire inn's tavern. Selection: **seed = record 0**
//! (the WMO default fog), then walk the camera group's MOGP fog-index byte list (≤4 slots): a
//! record is a candidate iff the WMO-local camera is within its `largerRadius` and
//! `!(flags & 1)`; candidates blend over the seed by weight
//! `1 − (d − smallerRadius)/(largerRadius − smallerRadius)`, nearest last. The selector's
//! `count == 1` bail means **no interior fog engages — the room keeps the SCENE fog, storm veil
//! included**. Settled EMPIRICALLY by two director ref-shots (2026-07-13): the forge (one "null
//! fog" record) shows the storm's even veil on every interior surface, near walls included (the
//! negative-start floor), while the inn (two authored records) is warm and clear. A brief
//! interim reading ("record 0 still engages") made one-record rooms too crisp — the thing that
//! had made the veil look wrong before it was the mist volume filling the room (a separate,
//! since-fixed bug), not the veil itself. Open: byte-confirm the staging slots simply hold scene
//! values at count == 1.
//!
//! This module only picks the **target** triple ([`CameraWmoFog`], written by the PVS pass — the
//! camera's group falls out of the portal flood's down-ray seed for free). The 4-second crossfade
//! ramp and the actual scene-fog lerp live in `crate::lighting` (`update_time_lighting`), the
//! benilla twin of `0x6cee30` — and the ramp value it publishes (`WmoCrossfade`) is ALSO the WMO
//! skybox's slot alpha: "the skybox alpha and the interior fog blend are the same number"
//! (the shared weight, `0xce9bdc`; `crate::skybox`).
//!
//! One explicitly-chosen reading remains: portal-less props never run the flood (all-visible fast
//! path), so they never engage — no shipped prop is an enterable room. (The claim mask itself is
//! bit 0x8 alone, `0x6be451`.)

use benilla_formats::WmoFog;
use bevy::prelude::*;

/// The MFOG fog target selected for the camera this frame (`None` = the camera does not stand in
/// a WMO interior group, or the building carries no MFOG at all — no shipped root does). Written
/// by the PVS pass ([`super::compute_wmo_pvs`]); consumed by the scene-fog stage
/// (`crate::lighting`).
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub struct CameraWmoFog(pub Option<WmoFogTarget>);

/// One resolved MFOG fog triple, in the record's own units — the staging transform
/// (`end = min(end, farclip)`, `start = end × start_scalar`, `0x6cef43–62`) is the consumer's.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WmoFogTarget {
    /// Fog colour, linear-file RGB 0..1 (the MFOG dword is `0xAARRGGBB`).
    pub(crate) color: [f32; 3],
    /// Fog end distance (yd), unclamped.
    pub(crate) end: f32,
    /// Fog start as a **fraction of end** — 1.12.1 multiplies, never subtracts (`fmul 0x6cef56`).
    pub(crate) start_scalar: f32,
}

/// Resolve the fog target for a camera standing in group `fog_indices` of a WMO with `fogs`
/// records, `eye_local` in WMO model space (WoW axes). `None` on the `count == 1` bail.
pub(super) fn select_wmo_fog(
    fogs: &[WmoFog],
    fog_indices: [u8; 4],
    eye_local: [f32; 3],
) -> Option<WmoFogTarget> {
    // The `count == 1` bail (`0x69de20`'s head): a one-record room keeps the SCENE fog — the
    // ref forge's even storm veil (see the module doc; director-settled 2026-07-13).
    if fogs.len() < 2 {
        return None;
    }
    // Candidates from the group's fog-index list: radius-tested, `flags & 1` excluded, then
    // distance-sorted so the NEAREST blends last (the client folds its ≤4-nearest array).
    let mut cands: Vec<(f32, &WmoFog)> = fog_indices
        .iter()
        .filter_map(|&i| fogs.get(i as usize))
        .filter(|rec| rec.flags & 1 == 0)
        .filter_map(|rec| {
            let d = dist(eye_local, rec.pos);
            (d <= rec.radius_outer).then_some((d, rec))
        })
        .collect();
    cands.sort_by(|a, b| b.0.total_cmp(&a.0)); // farthest first — nearest folds last
    let mut acc = triple(&fogs[0]); // seed: the WMO default fog
    for (d, rec) in cands {
        let span = rec.radius_outer - rec.radius_inner;
        let w = if span > 0.0 {
            (1.0 - (d - rec.radius_inner) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let t = triple(rec);
        for (a, b) in acc.color.iter_mut().zip(t.color) {
            *a += (b - *a) * w;
        }
        acc.end += (t.end - acc.end) * w;
        acc.start_scalar += (t.start_scalar - acc.start_scalar) * w;
    }
    Some(acc)
}

fn triple(rec: &WmoFog) -> WmoFogTarget {
    WmoFogTarget {
        color: unpack_argb(rec.color),
        end: rec.fog_end,
        start_scalar: rec.fog_start_scalar,
    }
}

/// MFOG colour dword → RGB 0..1 (`CImVector` in memory is B,G,R,A ⇒ the LE dword reads `0xAARRGGBB`).
fn unpack_argb(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(flags: u32, pos: [f32; 3], r_in: f32, r_out: f32, end: f32, color: u32) -> WmoFog {
        WmoFog {
            flags,
            pos,
            radius_inner: r_in,
            radius_outer: r_out,
            fog_end: end,
            fog_start_scalar: 0.25,
            color,
            uw_fog_end: 0.0,
            uw_fog_start_scalar: 0.0,
            uw_color: 0,
        }
    }

    #[test]
    fn single_record_keeps_the_scene_fog() {
        // The forge/chapel shape: one "null fog" record ⇒ NO engagement — the room keeps the
        // scene fog, storm veil included (the ref forge's even indoor veil; director-settled
        // 2026-07-13 after a brief record-0 interim made those rooms too crisp).
        let fogs = [rec(0, [0.0; 3], 0.0, 0.0, 444.4, 0xffffffff)];
        assert!(select_wmo_fog(&fogs, [0; 4], [0.0; 3]).is_none());
    }

    #[test]
    fn out_of_radius_and_flagged_records_leave_the_seed() {
        // The Goldshire inn's shape: default record 0 + a flags&1 record 1 the groups point at —
        // record 1 is excluded from the candidate walk, so the seed (record 0) is the fog.
        let fogs = [
            rec(0, [0.0; 3], 0.0, 3.36, 194.4, 0xfffad890),
            rec(1, [12.3, -0.6, 2.8], 0.0, 3.36, 83.3, 0xfffdcf9e),
        ];
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [12.3, -0.6, 2.8]).expect("engaged");
        assert!((t.end - 194.4).abs() < 1e-3, "seed wins: {t:?}");
        // The dword decodes ARGB → warm cream.
        assert!((t.color[0] - 250.0 / 255.0).abs() < 1e-4);
        assert!((t.color[2] - 144.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn candidate_blends_toward_the_room_fog_by_radius_weight() {
        let fogs = [
            rec(0, [0.0; 3], 0.0, 0.0, 200.0, 0xff000000),
            rec(0, [10.0, 0.0, 0.0], 5.0, 25.0, 80.0, 0xffffffff),
        ];
        // Camera at the record's centre (d = 0 ≤ inner) → full weight, the room fog verbatim.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [10.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 80.0).abs() < 1e-4);
        // Half-way through the falloff band (d = 15 → w = 0.5) → the midpoint.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [25.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 140.0).abs() < 1e-3, "got {t:?}");
        // Beyond the outer radius → the seed alone.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [40.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 200.0).abs() < 1e-4);
    }
}
