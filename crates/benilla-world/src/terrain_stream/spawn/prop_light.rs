//! WMO prop lighting: a placed prop's resolved light and the interior SH and particle folds.

use std::sync::Arc;

use benilla_assets::M2Model;
use bevy::prelude::*;

/// One WMO doodad prop at its world transform, spawned once its M2 loads.
pub(crate) struct WmoDoodadInst {
    pub(crate) handle: Handle<M2Model>,
    pub(crate) transform: Transform,
    /// The groups whose MODR names this prop, its portal-cull key: drawn while any is visible.
    /// Empty for a MODD no group names: never instantiated by the reference, drawn here with no
    /// room gate.
    pub(crate) groups: Arc<[u16]>,
    pub(crate) light: PropLight,
    pub(crate) spawned: bool,
}

/// A WMO prop's lighting, its MOLR lights already in world space for the spawn-time fold.
pub(crate) enum PropLight {
    Exterior,
    Interior {
        /// The ambient word, `cap96(MODD.colour)` (0–1 RGB).
        ambient: [f32; 3],
        /// The diffuse word, `floor112(MODD.colour)`, committed on the fixed interior axis.
        diffuse: [f32; 3],
        /// The owning group's MOLR omni lights.
        lights: Vec<PropLobeLight>,
    },
}

impl PropLight {
    /// The lighting lane for the mouseover inspector: `sky-lit`, or the interior words it commits.
    pub(crate) fn inspector_label(&self) -> String {
        let hex = |c: &[f32; 3]| hex_word(*c);
        match self {
            PropLight::Exterior => "sky-lit".into(),
            PropLight::Interior {
                ambient,
                diffuse,
                lights,
            } => format!(
                "interior amb {} dif {} · {} MOLR",
                hex(ambient),
                hex(diffuse),
                lights.len()
            ),
        }
    }
}

/// A committed light word (0–1 RGB) as `#rrggbb`, the one form every prop inspector prints.
pub fn hex_word(c: [f32; 3]) -> String {
    let b = c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
    format!("#{:02x}{:02x}{:02x}", b[0], b[1], b[2])
}

/// One MOLR light as the interior folds read it: world position and colour × intensity.
pub struct PropLobeLight {
    pub pos: Vec3,
    pub color_i: [f32; 3],
    pub atten_start: f32,
    pub atten_end: f32,
}

/// The fixed-function light at world up that every lit particle of a model in a WMO room takes. A
/// particle batch (TYPE 4, `0x70d8b0`) goes through `0x70ca50`, which binds no vertex program, so
/// `0x70baf0` takes the device-light commit `0x71c730`: `max(N·L, 0)` over the room's committed
/// words, not [`fold_interior_probe`]'s SH lobe. Slot 0 is the committed diffuse (`0x71c2f0`
/// rebuilds `1.25·P − 0.25·DC`, and one directional has `P == DC`), with `N·L` 0.9 on the fixed
/// axis; slots 1..3 are the ≤3 nearest MOLT points, diffuse only, under `1/(0.7d + 0.03d²)`.
/// Unclamped: GL clamps the lit colour, the product, which the particle sim does.
pub fn interior_light_up(
    ambient: [f32; 3],
    diffuse: [f32; 3],
    ref_point: Vec3,
    lights: &[PropLobeLight],
) -> [f32; 3] {
    // Toward-light, Bevy space: the axis the SH fold puts the diffuse word on.
    let axis = Vec3::new(-0.30822, 0.9, -0.30822).normalize();
    let mut lit = Vec3::from_array(ambient) + Vec3::from_array(diffuse) * axis.y.max(0.0);
    // The ≤3 nearest: the reference's 4-entry max-heap by squared distance (`0x71bf90`) minus
    // slot 0. Membership is the light's disk window, as in the SH fold; the reference's is
    // `0x6a7ac0`'s radius test against the proxy's WMO instances.
    let mut near: Vec<(f32, &PropLobeLight)> = lights
        .iter()
        .map(|l| ((l.pos - ref_point).length(), l))
        .filter(|(d, l)| *d < l.atten_end)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (dist, l) in near.into_iter().take(3) {
        let up_dot = (l.pos.y - ref_point.y) / dist.max(1e-4);
        if up_dot <= 0.0 {
            continue;
        }
        let atten = 0.7 * dist + 0.03 * dist * dist;
        if atten > 0.0 {
            lit += Vec3::from_array(l.color_i) * (up_dot / atten);
        }
    }
    lit.to_array()
}

/// An interior light as its 7-row SH probe, each MOLR lobe windowed by its distance from
/// `ref_point` (`0x69e1c0`: full inside `attenStart`, none past `attenEnd`, linear between).
pub fn fold_interior_probe(
    ambient: [f32; 3],
    diffuse: [f32; 3],
    ref_point: Vec3,
    lights: &[PropLobeLight],
) -> [bevy::math::Vec4; 7] {
    // Toward-light, Bevy space: wow (0.30822, 0.30822, 0.9) → (−y, z, −x).
    let mut lobes: Vec<(Vec3, [f32; 3])> = vec![(Vec3::new(-0.30822, 0.9, -0.30822), diffuse)];
    for l in lights {
        let dv = l.pos - ref_point;
        let dist = dv.length();
        let gain = if dist <= l.atten_start {
            1.0
        } else if dist >= l.atten_end || l.atten_end <= l.atten_start {
            0.0
        } else {
            1.0 - (dist - l.atten_start) / (l.atten_end - l.atten_start)
        };
        if gain > 0.0 {
            lobes.push((dv / dist.max(1e-4), l.color_i.map(|c| c * gain)));
        }
    }
    crate::lighting::prop_probe_coeffs(ambient, &lobes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's interior leg (`0x6a7300` → `0x71bce0`/`0x71bc70` → `0x71c2f0` →
    /// `0x71c730`) on a (70, 60, 50) floor sample: `sample/255 · (2.4·0.9 + 1.0)` per channel.
    #[test]
    fn the_committed_interior_light_at_world_up_is_the_reference_worked_case() {
        let sample = [70.0f32, 60.0, 50.0].map(|c| c / 255.0);
        let ambient = sample; // cap96 does not fire at max 70
        let diffuse = sample.map(|c| c * 2.4); // floor168 boosts 70 -> 168 uniformly
        let got = interior_light_up(ambient, diffuse, Vec3::ZERO, &[]);
        for (k, s) in sample.into_iter().enumerate() {
            let want = s * 3.16;
            assert!(
                (got[k] - want).abs() < 1e-5,
                "ch{k}: got {} want {want}",
                got[k]
            );
        }
    }

    /// With N world up, `max(N·L, 0)` has no wrap-around, unlike the SH lobe.
    #[test]
    fn a_room_light_below_the_emitter_contributes_nothing() {
        let lamp = |y: f32| PropLobeLight {
            pos: Vec3::new(0.0, y, 0.0),
            color_i: [1.0, 1.0, 1.0],
            atten_start: 0.0,
            atten_end: 100.0,
        };
        let dark = [0.0; 3];
        let below = interior_light_up(dark, dark, Vec3::ZERO, &[lamp(-5.0)]);
        assert_eq!(below, [0.0; 3], "a light under the floor lights nothing");
        let above = interior_light_up(dark, dark, Vec3::ZERO, &[lamp(5.0)]);
        // N·L = 1 straight overhead; GL attenuation 1/(0.7·5 + 0.03·25) = 1/4.25.
        for ch in above {
            assert!((ch - 1.0 / 4.25).abs() < 1e-5, "got {ch}");
        }
    }
}
