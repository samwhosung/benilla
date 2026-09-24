//! The camera-in-interior WMO fog: which MFOG record's fog the camera's room wants this frame.
//!
//! The selector `0x69de20` bails when the root has exactly one MFOG record (`0x69de4b`), as five in
//! six shipped roots do, and the room keeps the scene fog; past the bail it seeds with record 0
//! (`0x69ded2`). `select_wmo_fog` then blends over the seed, by the radius falloff and nearest
//! last, every record the group's fog indices name, index 0 included, that holds the camera inside
//! its outer radius with `flags & 1` clear. The 4 s crossfade is `crate::lighting`'s. A
//! portal-less prop runs no flood and never engages; no shipped prop is an enterable room.

use benilla_formats::WmoFog;
use bevy::prelude::*;

/// The MFOG fog target for the camera this frame: `None` when no containing group of the camera is
/// a true interior, or the root has under two records. Written by the PVS pass
/// ([`super::compute_wmo_pvs`]), read by `crate::lighting`.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub struct CameraWmoFog(pub Option<WmoFogTarget>);

/// One resolved MFOG fog triple in the record's own units; the staging transform is the consumer's
/// (`end = min(end, farclip)`, `start = end × start_scalar`, `0x6cef43`–`0x6cef62`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WmoFogTarget {
    /// Fog colour, RGB 0..1 as filed (the MFOG dword reads `0xAARRGGBB`).
    pub(crate) color: [f32; 3],
    /// Fog end distance (yd), unclamped.
    pub(crate) end: f32,
    /// Fog start as a fraction of end: the 1.12 client multiplies, never subtracts (`0x6cef56`).
    pub(crate) start_scalar: f32,
}

/// The fog target for a camera at `eye_local` (WMO model space, WoW axes) in a group with
/// `fog_indices`; `None` on the one-record bail.
pub(super) fn select_wmo_fog(
    fogs: &[WmoFog],
    fog_indices: [u8; 4],
    eye_local: [f32; 3],
) -> Option<WmoFogTarget> {
    // The one-record bail (`0x69de4b`): the room keeps the scene fog.
    if fogs.len() < 2 {
        return None;
    }
    // Candidates from the group's fog indices: radius-tested, `flags & 1` excluded, sorted so the
    // nearest blends last (the client folds its up-to-4-nearest array).
    let mut cands: Vec<(f32, &WmoFog)> = fog_indices
        .iter()
        .filter_map(|&i| fogs.get(i as usize))
        .filter(|rec| rec.flags & 1 == 0)
        .filter_map(|rec| {
            let d = dist(eye_local, rec.pos);
            (d <= rec.radius_outer).then_some((d, rec))
        })
        .collect();
    cands.sort_by(|a, b| b.0.total_cmp(&a.0)); // farthest first, nearest folds last
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

/// MFOG colour dword to RGB 0..1: B,G,R,A in memory (`CImVector`) reads as LE `0xAARRGGBB`.
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
        let fogs = [rec(0, [0.0; 3], 0.0, 0.0, 444.4, 0xffffffff)];
        assert!(select_wmo_fog(&fogs, [0; 4], [0.0; 3]).is_none());
    }

    #[test]
    fn out_of_radius_and_flagged_records_leave_the_seed() {
        // The Goldshire inn's shape: the groups point at record 1, which carries `flags & 1`.
        let fogs = [
            rec(0, [0.0; 3], 0.0, 3.36, 194.4, 0xfffad890),
            rec(1, [12.3, -0.6, 2.8], 0.0, 3.36, 83.3, 0xfffdcf9e),
        ];
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [12.3, -0.6, 2.8]).expect("engaged");
        assert!((t.end - 194.4).abs() < 1e-3, "seed wins: {t:?}");
        // The dword decodes as ARGB: warm cream.
        assert!((t.color[0] - 250.0 / 255.0).abs() < 1e-4);
        assert!((t.color[2] - 144.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn candidate_blends_toward_the_room_fog_by_radius_weight() {
        let fogs = [
            rec(0, [0.0; 3], 0.0, 0.0, 200.0, 0xff000000),
            rec(0, [10.0, 0.0, 0.0], 5.0, 25.0, 80.0, 0xffffffff),
        ];
        // At the record's centre (d = 0 ≤ inner): full weight, the room fog verbatim.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [10.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 80.0).abs() < 1e-4);
        // Half-way through the falloff band (d = 15, w = 0.5): the midpoint.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [25.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 140.0).abs() < 1e-3, "got {t:?}");
        // Beyond the outer radius: the seed alone.
        let t = select_wmo_fog(&fogs, [1, 0, 0, 0], [40.0, 0.0, 0.0]).unwrap();
        assert!((t.end - 200.0).abs() < 1e-4);
    }
}
