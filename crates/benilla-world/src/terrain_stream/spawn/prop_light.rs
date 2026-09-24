//! The WMO prop-light machinery — a placed prop's resolved lighting and the interior SH fold.
//!
//! Split out of `terrain_stream.rs` (0830 named the carve; 0832 executed it): this is spawn-side
//! state and math — consumed by the placement assembler, the WMO-gameobject prop lane
//! (`crate::entities`' `wmo_props`) and the GameObject footprint classifier (`crate::interior`) —
//! and none of it touches the streamer's residency logic. Paths outside the streamer are stable:
//! `terrain_stream` re-exports [`fold_interior_probe`] and [`PropLobeLight`].

use std::sync::Arc;

use benilla_assets::M2Model;
use bevy::prelude::*;

/// One WMO doodad prop instance: its M2 handle and the **world** transform (the WMO instance
/// transform composed with the doodad's WMO-local transform), spawned once the M2 asset loads.
pub(crate) struct WmoDoodadInst {
    pub(crate) handle: Handle<M2Model>,
    pub(crate) transform: Transform,
    /// Every WMO group whose MODR names this prop — the portal-cull key, so a prop is hidden with
    /// the rooms it furnishes and drawn while any one of them is visible. Empty for a MODD no group
    /// references (the reference never instantiates one at all; we still show it, uncullable,
    /// rather than change what draws today).
    pub(crate) groups: Arc<[u16]>,
    /// The prop's lighting ([`PropLight`], from `WmoModel::doodad_base` composed with this
    /// placement): exterior sky-lit, or the interior MODD-colour base + its owning group's MOLR
    /// lights placed in world space — folded into the prop's SH probe once its M2 loads (the fold
    /// reference point needs the M2 bounds).
    pub(crate) light: PropLight,
    pub(crate) spawned: bool,
}

/// A WMO prop's placement-resolved lighting: the asset-level [`DoodadBase`](benilla_assets::DoodadBase)
/// with the owning group's MOLR lights already transformed to WORLD (Bevy) space — so the
/// spawn-time SH fold needs only the loaded M2's bounds (its reference point) and nothing from the
/// WMO asset.
pub(crate) enum PropLight {
    Exterior,
    Interior {
        /// `cap96(MODD.colour)` — the ambient word (0–1 RGB).
        ambient: [f32; 3],
        /// `floor112(MODD.colour)` — the diffuse word, committed on the fixed interior axis.
        diffuse: [f32; 3],
        /// The owning group's MOLR omni lights: world (Bevy) position, colour × intensity, and the
        /// disk `attenStart`/`attenEnd` window (the fold's range gate).
        lights: Vec<PropLobeLight>,
    },
}

impl PropLight {
    /// The prop's lighting lane, as the mouseover inspector says it. A WMO prop that looks wrong is
    /// almost always on the wrong *lane* or carrying an unbaked base — and neither is visible from
    /// the model path alone, which is why Booty Bay's black entrance arch read as a texture bug for
    /// as long as it did (decision 0969). `sky-lit` is the exterior lane; the interior lane prints
    /// the MODD-colour words it actually commits, so a base of `#000000` names itself on hover.
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

/// A committed light word (0–1 RGB) as `#rrggbb` — the reading every prop inspector prints, here
/// once so the terrain lane's label and the WMO-gameobject lane's (`entities::wmo_props`) cannot
/// drift into two different renderings of the same number.
pub fn hex_word(c: [f32; 3]) -> String {
    let b = c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
    format!("#{:02x}{:02x}{:02x}", b[0], b[1], b[2])
}

/// One MOLR-referenced light as the interior fold consumes it (world Bevy space, colour
/// pre-multiplied by the authored intensity). Shared by the MODD prop spawn fold and the
/// GameObject footprint lane ([`crate::interior`] via [`crate::wmo_portal`]'s verdict).
pub struct PropLobeLight {
    pub pos: Vec3,
    pub color_i: [f32; 3],
    pub atten_start: f32,
    pub atten_end: f32,
}

/// The **fixed-function** lane's committed light, evaluated at the world up axis — the single
/// constant every LIT particle of a model standing in a WMO room is multiplied by.
///
/// A particle draw does **not** take the SH vertex program a mesh batch takes, and that is not an
/// approximation either way — it is a structural fork. The batch-record TYPE dispatch
/// (`70b613`/`70b61e jmp [eax*4+0x70b728]`) sends TYPE 0 (mesh) and TYPE 2 (batched doodads) to
/// `0x70baf0` with the frame's `M2UseShaders` mask bit cached at `[esi+0x32f0]`, but TYPE 4
/// (**particles**, `0x70d8b0`) — with 1, 3 and 5 — through the shared helper `0x70ca50`, which
/// writes no such field and NULL-binds **both** program slots (`70ca74 SetState(0x40, 0)`,
/// `70ca80 SetState(0x3f, 0)`). `0x70b360` zeroes `[esi+0x32f0]` at the head of every batch
/// (`70b607`), and only `0x70cb30` (TYPE 0) and `0x70d330` (TYPE 2) ever write it back — so
/// `0x70baf0`'s `70bb9c test eax,eax` reads 0 for a particle and `70bba4 je 0x70bdf2` takes the
/// **fixed-function device-light commit** `0x71c730` unconditionally.
///
/// So the curve here is the hardware's `max(N·L, 0)`, **not** [`fold_interior_probe`]'s SH lobe —
/// a mesh and its own particles share the room's committed WORDS and legitimately differ in the
/// function applied to them (0.900 vs 0.869 on the key axis; the SH lobe also wraps around the
/// back, where this clamps to nothing). Decision 1709 supersedes 1705's reading.
///
/// - **Slot 0** is the key light. `0x71c2f0` reconstructs it from the context's linear moments as
///   `1.25·P − 0.25·DC` with **no clamp** (`0x4549a0` is three dword moves and a `ret 0xc`); with
///   the interior leg's single directional `P == DC` exactly, so it reduces to the committed
///   diffuse itself, and the ambient's `0.25·(DC − P)` correction is zero. `N` is world up and the
///   axis is the fixed `(−0.30822, −0.30822, −0.9)`, so `N·L` is **0.9 at every camera angle**.
/// - **Slots 1..3** are the ≤3 nearest MOLT points, diffuse-only (ambient and specular forced to 0
///   at `71c7e3`/`71c7e6`), under GL's own distance attenuation `1/(0.7d + 0.03d²)` — constant
///   term zero. Because N is world up, a room light at or below the emitter contributes exactly
///   nothing.
///
/// Unclamped by design: the reference clamps the **product**, not the term (GL clamps the lit
/// vertex colour), which is where `sim`'s fold does it.
pub fn interior_light_up(
    ambient: [f32; 3],
    diffuse: [f32; 3],
    ref_point: Vec3,
    lights: &[PropLobeLight],
) -> [f32; 3] {
    // Toward-light, Bevy space — the same axis the SH fold puts the diffuse word on.
    let axis = Vec3::new(-0.30822, 0.9, -0.30822).normalize();
    let mut lit = Vec3::from_array(ambient) + Vec3::from_array(diffuse) * axis.y.max(0.0);
    // The ≤3 NEAREST in range, which is the reference's 4-entry max-heap by squared distance
    // (`0x71bf90`, `71bfca cmp esi,0x4`) minus slot 0. Membership is our own disk window — the
    // reference's is `0x6a7ac0`'s radius test against the proxy's WMO instances, which we model
    // the same way for the SH lane; see the residual in decision 1709.
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

/// Fold one interior committed light into its 7-row SH probe: the ambient word + the diffuse word
/// as a directional on the FIXED interior axis + each MOLR lobe windowed by its disk
/// attenStart/attenEnd from `ref_point` (the byte-verified `0x69e1c0` falloff: d ≤ start → 1;
/// d ≥ end → excluded; else linear). One definition for both interior lanes — the MODD prop
/// (spawn-time, MODD-colour words) and the GameObject footprint (classify-time, MOCV-derived
/// words); the SH closed form itself is [`prop_probe_coeffs`](crate::lighting::prop_probe_coeffs).
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

    /// GOLDEN — the exact worked case for the WENTITY interior leg (`0x6a7300`
    /// → `0x71bce0`/`0x71bc70` → `0x71c2f0` → `0x71c730`), reproduced end to end. A floor MOCV
    /// sample of `(70, 60, 50)`: the HSV boost fires (`70 < 168`, a uniform ×2.4 to `(168,144,120)`)
    /// and the cap does not (`70 <= 96`, so the ambient word is the sample itself). With one
    /// directional and no points, `0x71c2f0`'s `P == DC` exactly, so the committed diffuse IS the
    /// boosted word and the ambient correction is zero — leaving `sample/255 · (2.4·0.9 + 1.0)`
    /// = `sample/255 · 3.16` per channel. The tone-shaping is the classifier's (`floor168`/`cap96`);
    /// this pins the arithmetic below it.
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

    /// A MOLT point light reaches a particle **only from above**. The normal is world up and the
    /// term is the hardware's `max(N·L, 0)`, so a room light at or below the emitter contributes
    /// exactly zero — the fixed-function lane has no wrap-around, which is precisely where it
    /// parts company with the SH lobe a mesh batch takes.
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
