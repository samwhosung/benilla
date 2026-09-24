//! The per-instance `MeshTag` channel: the one home of its bit layout and its writer protocol.
//!
//! Every `WowModelMaterial` submesh carries a Bevy `MeshTag`, a `u32` the shader reads per
//! instance. Bits 31 and 30 are flags ([`HIGHLIGHT_BIT`], [`INTERIOR_FOG_BIT`]), masked off before
//! the payload decodes. In the payload the rig and alpha fields mean the same in every mode, and
//! the bits between them switch meaning on material state:
//!
//! - Rig field (bits 19..=29): the instance slot into the shared buffer's slot-keyed regions, `0`
//!   for none. The vertex stage reads the skin palette with it ([`crate::rig_palette`]) only under
//!   `WOW_RIG_SKIN`, which comes from the mesh's own joints; the fragment stage reads the body tint
//!   ([`crate::instance_tint`]) for every part. Every part carries its unit's slot, so a tinted
//!   unit tints whole (an attached model inherits its parent CM2's colours, `0x714000`); worn gear
//!   owns a rider slot and takes the tint and the straddle waterline through its `ParentModel`
//!   chain. Written at spawn ([`rig_bits`]) and carried by every writer but [`with_rig`].
//! - Alpha (bits 0..=5): the fade alpha, multiplying the cutout alpha. A whole payload of `0` is
//!   the untagged, opaque sentinel.
//! - Exterior payload (the default): bits 6..=13 are the ground-shade byte (`0` lit, `255` fully
//!   MCSH-shadowed), mixing the batch's lit sun level toward the shaded one (`wow_model.wgsl`).
//!   Entities ramp it ([`crate::entity_shade`]); statics leave it `0` and shade per material
//!   (`sun_scale.x`). Bits 14..=18 are reserved.
//! - Interior probe slot (an interior M2 prop or entity: interior material, `model_flags.z` set,
//!   not a WMO): bits 6..=18 are the SH-probe table slot ([`crate::lighting::PropProbes`]).
//!   [`probe_bits`] always carries a non-zero alpha, so slot 0 is valid.
//!
//! The writers, deconflicted by field and by order:
//!
//! 1. `model_fade::apply_render_fade`, the appear and despawn ramps, owns the alpha and the
//!    material while a `RenderFade` lives; the other alpha writers skip those entities.
//! 2. `interior::classify_entity_interior` owns the light law of `InteriorLit` parts (their
//!    non-alpha payload) and their fog bit. The law is where the part stands, re-asked when it
//!    moves; the fog bit is whether that room is on the camera's chain, re-read every frame. It
//!    runs through a fade, carrying the alpha; only the material defers to 1, which picks the
//!    law's blend twin (`FadeMaterials::material_for`).
//! 3. `model_render::visibility::apply_model_visibility` drives the `DoodadFade` distance fade and
//!    glow-card dimming, never on a lit interior prop, and owns the fog bit of room-bound WMO
//!    content (`WmoGroupVis`), the reference's per-group `[0xca7f00]` gate; a WMO part holds no
//!    `InteriorLit`, so it never meets 2.
//! 4. `player::apply_self_model_fade`, the first-person feather, runs after 1-3 and owns the self
//!    body's alpha while it feathers; on the frame it ends it restores the alpha itself (2 carries
//!    alpha through) and hands the material back to the law.
//! 5. `entity_shade::update_ground_shade` owns the shade byte of entity parts and runs after 2 to
//!    re-assert it over 2's exterior reset. The byte shares bits 6..=13 with the probe slot, so it
//!    writes only through an [`ExteriorPayload`].
//!
//! A new payload writer claims reserved bits through a typed accessor here, never a whole-payload
//! convention of its own. Bit 31's one writer, `target::highlight::apply_highlight`, runs in
//! `PostUpdate` after the `Update` payload writers, which drop the flag, and re-asserts it.

/// Marks an instance whose bits 6..=18 are an interior probe slot ([`probe_bits`]): every batch of
/// a lit interior MODD prop. The shade writer skips it even under a WMO-display GameObject, whose
/// props ride the entity for transform only: their light is their own baked MODD colour (the
/// reference's `CMapDoodadDef` provider `0x6a8050`).
#[derive(bevy::prelude::Component)]
pub struct InteriorProbePayload;

/// Bit 31: the hover/target model-brighten flag, the reference's per-model highlight emissive
/// (`SetHighlight` `0x614550` writes the config RGB into the CM2), added to the lighting sum.
pub const HIGHLIGHT_BIT: u32 = 0x8000_0000;

/// Bit 30: fog with the interior triple (shared-light rows 18-19, the camera-crossfaded MFOG)
/// instead of the scene fog. Its owners set it when the model stands in a WMO interior (the
/// reference's per-unit classification `0x71c110`, collector `+0x184`, lane by `[node+0xc]&2`) and
/// its room is on the camera's `[0xca7f00]` chain this frame (`[P+0x98] != 0`). With the camera
/// outside, the interior triple equals the scene's (the crossfade at `t = 0`). The unit's lane
/// pick (`0x6c31e0`) has no room test of its own, unlike the wall drawer's per-group `0x6b5190`.
pub(crate) const INTERIOR_FOG_BIT: u32 = 0x4000_0000;

/// Bits 0..=5, both modes: the fade alpha as a 6-bit fraction (`63` = opaque).
const ALPHA_MASK: u32 = 0x0000_003f;
const ALPHA_MAX: f32 = 63.0;
/// Bits 6..=13 of the exterior payload: the ground-shade byte.
const SHADE_MASK: u32 = 0x0000_3fc0;
const SHADE_SHIFT: u32 = 6;
/// Bits 6..=18 of the interior payload: the SH-probe table slot (13 bits, 8192 slots).
const PROBE_MASK: u32 = 0x0007_ffc0;
const PROBE_SHIFT: u32 = 6;
/// Bits 19..=29, both modes: the rig slot, `0` for none.
const RIG_MASK: u32 = 0x3ff8_0000;
const RIG_SHIFT: u32 = 19;
/// The 11-bit rig field's slot count, slot 0 (none) included; `crate::rig_palette` sizes to it.
pub const MAX_RIG_SLOTS: usize = 1 << 11;

/// An interior-probe spawn tag for a rig-less part (the static-prop spawner): the slot, an opaque
/// alpha and [`INTERIOR_FOG_BIT`] set. `apply_model_visibility` rewrites that bit every frame for
/// room-bound WMO content; a prop no group's MODR references has no `WmoGroupVis` and keeps it
/// set (the reference never instantiates such a prop).
pub(crate) fn probe_bits(slot: u16) -> u32 {
    INTERIOR_FOG_BIT | (u32::from(slot) << PROBE_SHIFT) | alpha_bits(1.0)
}

/// The alpha field a whole-payload rewrite carries, the untagged `0` (which [`alpha_bits`] never
/// writes) made opaque: it lets the classifier re-lane a part mid-fade.
fn carried_alpha(tag: u32) -> u32 {
    match tag & ALPHA_MASK {
        0 => ALPHA_MASK,
        a => a,
    }
}

/// Rewrites a tag as an interior-probe payload, keeping the rig and alpha fields: the classifier's
/// Bake law. It leaves [`INTERIOR_FOG_BIT`] to [`with_interior_fog`]: standing indoors is not
/// enough, the room must also be on the camera's `[0xca7f00]` chain.
pub(crate) fn with_interior_probe(tag: u32, slot: u16) -> u32 {
    (tag & RIG_MASK) | (u32::from(slot) << PROBE_SHIFT) | carried_alpha(tag)
}

/// Rewrites a tag as a fresh exterior payload, keeping the rig and alpha fields: the classifier's
/// outdoor reclaim, whose shade byte the shade writer restores later the same frame.
pub(crate) fn with_exterior_reset(tag: u32) -> u32 {
    (tag & RIG_MASK) | carried_alpha(tag)
}

/// A spawned part's initial tag: its rig slot (`0` if unskinned) and starting alpha, the only two
/// fields a spawner may set; the rest belong to the later read-modify-write writers.
pub fn spawn_tag(rig_slot: u16, alpha: f32) -> u32 {
    rig_bits(rig_slot) | alpha_bits(alpha)
}

/// A spawn tag's rig field; slot `0`, no rig, is never allocated by [`crate::rig_palette`].
pub fn rig_bits(slot: u16) -> u32 {
    debug_assert!((slot as usize) < MAX_RIG_SLOTS);
    u32::from(slot) << RIG_SHIFT
}

/// Read back a tag's rig field (`0` = not skinned).
pub fn rig_of(tag: u32) -> u16 {
    ((tag & RIG_MASK) >> RIG_SHIFT) as u16
}

/// Rewrites the rig field alone: the lazy-rig writer, the one exception to written-once, whose
/// doodad slot arrives at its first draw-wake and leaves under table pressure.
pub(crate) fn with_rig(tag: u32, slot: u16) -> u32 {
    (tag & !RIG_MASK) | rig_bits(slot)
}

/// A fade alpha as tag bits; a zero or negative alpha writes `1` (1/63), since a `0` tag is the
/// shader's untagged, opaque sentinel (`wow_model.wgsl`).
pub fn alpha_bits(alpha: f32) -> u32 {
    if alpha <= 0.0 {
        1u32
    } else {
        ((alpha.min(1.0) * ALPHA_MAX).round() as u32).max(1)
    }
}

/// Sets or clears [`INTERIOR_FOG_BIT`] alone: the probe slot says how a prop is lit, the
/// reference's `[0xca7f00]` only which fog triple its batch gets.
pub(crate) fn with_interior_fog(tag: u32, on: bool) -> u32 {
    match on {
        true => tag | INTERIOR_FOG_BIT,
        false => tag & !INTERIOR_FOG_BIT,
    }
}

/// Writes the alpha field alone: the fade writers' read-modify-write.
pub fn with_alpha(tag: u32, alpha: f32) -> u32 {
    (tag & !ALPHA_MASK) | alpha_bits(alpha)
}

/// Proof that an instance is on the exterior payload, so bits 6..=13 are its shade byte and not a
/// probe slot's low eight; [`with_shade`] and [`shade_of`] require it. The overlap is forced: the
/// four shared fields take 19 bits, leaving 13 for payloads that want 21 (shade 8, probe 13).
#[derive(Clone, Copy)]
pub struct ExteriorPayload(());

/// The payload question: `None` when bits 6..=18 hold a probe slot, as for an entity part on the
/// classifier's Bake law (`on_bake_law`, `InteriorLit::is_bake`) or a lit interior MODD prop
/// (`own_probe`, [`InteriorProbePayload`]).
pub fn exterior_payload(on_bake_law: bool, own_probe: bool) -> Option<ExteriorPayload> {
    (!on_bake_law && !own_probe).then_some(ExteriorPayload(()))
}

/// Writes the shade byte, turning an untagged `0` opaque first: a non-zero byte would otherwise
/// defeat the sentinel and decode as alpha 0.
pub(crate) fn with_shade(tag: u32, shade: u8, _: ExteriorPayload) -> u32 {
    (tag & !(ALPHA_MASK | SHADE_MASK)) | carried_alpha(tag) | (u32::from(shade) << SHADE_SHIFT)
}

/// Reads the alpha field as a fraction; the untagged `0` reads `1.0`.
pub fn alpha_of(tag: u32) -> f32 {
    if tag == 0 {
        return 1.0;
    }
    (tag & ALPHA_MASK) as f32 / ALPHA_MAX
}

/// Reads the shade byte of an exterior-payload tag (the shade writer's change gate).
pub(crate) fn shade_of(tag: u32, _: ExteriorPayload) -> u8 {
    ((tag & SHADE_MASK) >> SHADE_SHIFT) as u8
}

/// Whether the instance draws translucent, decoded as the shader does: the trigger of the
/// depth-prime twin ([`crate::zfill`]). The reference sends any batch with `A < 0.99999` to the
/// transparent lists with a twin and culls only `A ≤ 0`, so alpha fields 1..=62 are the band.
pub(crate) fn translucent(tag: u32) -> bool {
    let payload = tag & !(HIGHLIGHT_BIT | INTERIOR_FOG_BIT);
    payload != 0 && matches!(payload & ALPHA_MASK, 1..=62)
}

/// A readable decode for the probes (`WOW_PICK`'s shading dump), here so no probe re-derives the
/// masks. Bits 6..=18 print both ways, shade and slot, since only the material says which.
pub fn describe(tag: u32) -> String {
    if tag == 0 {
        return "0 (untagged ⇒ opaque)".to_string();
    }
    let flags = match (tag & HIGHLIGHT_BIT != 0, tag & INTERIOR_FOG_BIT != 0) {
        (true, true) => " hi+fog",
        (true, false) => " hi",
        (false, true) => " fog",
        (false, false) => "",
    };
    // The raw masks, with no `ExteriorPayload` witness: printing both readings is the point.
    format!(
        "{tag:#010x}{flags} α {:.3} shade {} / slot {} rig {}",
        alpha_of(tag),
        (tag & SHADE_MASK) >> SHADE_SHIFT,
        (tag & PROBE_MASK) >> PROBE_SHIFT,
        rig_of(tag),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A witness for the tests about the bits rather than the population.
    fn ext() -> ExteriorPayload {
        exterior_payload(false, false).expect("neither probe population")
    }

    #[test]
    fn describe_prints_both_readings_of_the_shared_bits() {
        // The 0 sentinel prints by name, never as the alpha 0 it does not mean.
        assert!(describe(0).contains("untagged"));
        let t = with_shade(alpha_bits(1.0), 255, ext());
        assert!(describe(t).contains("shade 255"), "{}", describe(t));
        assert!(describe(t).contains("slot 255"), "{}", describe(t));
        // A probe payload: its slot and its baked-in fog flag.
        let t = probe_bits(6660);
        assert!(describe(t).contains("slot 6660"), "{}", describe(t));
        assert!(describe(t).contains("fog"), "{}", describe(t));
        let t = rig_bits(1234) | alpha_bits(1.0);
        assert!(describe(t).contains("rig 1234"), "{}", describe(t));
        assert!(describe(HIGHLIGHT_BIT | alpha_bits(0.5)).contains("hi"));
        assert!(describe(HIGHLIGHT_BIT | INTERIOR_FOG_BIT | 1).contains("hi+fog"));
    }

    #[test]
    fn highlight_bit_is_orthogonal_to_both_payloads() {
        for a in [0.0, f32::MIN_POSITIVE, 0.25, 0.5, 1.0] {
            assert_eq!(alpha_bits(a) & HIGHLIGHT_BIT, 0);
        }
        assert_eq!(with_shade(alpha_bits(1.0), 255, ext()) & HIGHLIGHT_BIT, 0);
        assert_eq!(probe_bits(8191) & HIGHLIGHT_BIT, 0);
        assert_eq!(rig_bits(2047) & HIGHLIGHT_BIT, 0);
        // Both field writers keep a set flag.
        assert_eq!(
            with_alpha(HIGHLIGHT_BIT | 0x3fff_ffff, 0.5) & HIGHLIGHT_BIT,
            HIGHLIGHT_BIT
        );
        assert_eq!(
            with_shade(HIGHLIGHT_BIT | 0x3f, 7, ext()) & HIGHLIGHT_BIT,
            HIGHLIGHT_BIT
        );
    }

    #[test]
    fn alpha_bits_never_hits_the_opaque_sentinel() {
        assert_eq!(alpha_bits(0.0), 1);
        assert_eq!(alpha_bits(-0.5), 1);
        assert_ne!(alpha_bits(f32::MIN_POSITIVE), 0);
        assert_eq!(alpha_bits(1.0), ALPHA_MASK);
        assert_eq!(alpha_bits(1.0) & (SHADE_MASK | PROBE_MASK | RIG_MASK), 0);
    }

    #[test]
    fn alpha_and_shade_fields_compose() {
        let t = with_shade(alpha_bits(1.0), 200, ext());
        assert_eq!(shade_of(t, ext()), 200);
        let t = with_alpha(t, 0.25);
        assert_eq!(shade_of(t, ext()), 200);
        assert_eq!(t & ALPHA_MASK, alpha_bits(0.25));
        let t = with_shade(t, 10, ext());
        assert_eq!(t & ALPHA_MASK, alpha_bits(0.25));
        assert_eq!(shade_of(t, ext()), 10);
    }

    #[test]
    fn probe_bits_compose_with_the_alpha_field() {
        let t = probe_bits(6660);
        assert_eq!((t & PROBE_MASK) >> PROBE_SHIFT, 6660);
        assert_eq!(t & ALPHA_MASK, ALPHA_MASK); // opaque, so the 0 sentinel cannot fire
        let t = with_alpha(t, 0.25);
        assert_eq!((t & PROBE_MASK) >> PROBE_SHIFT, 6660); // the slot survives a fade
        assert_eq!(t & ALPHA_MASK, alpha_bits(0.25));
        assert_eq!(t & HIGHLIGHT_BIT, 0);
        // The max slot leaves the rig field clear, and the fog flag is baked in.
        assert_eq!(probe_bits(8191) & RIG_MASK, 0);
        assert_eq!(probe_bits(8191) & INTERIOR_FOG_BIT, INTERIOR_FOG_BIT);
    }

    #[test]
    fn rig_field_survives_every_runtime_writer() {
        let spawn = rig_bits(1000) | alpha_bits(1.0);
        assert_eq!(rig_of(spawn), 1000);
        assert_eq!(rig_of(with_alpha(spawn, 0.3)), 1000); // fades (writers 1, 3, 4)
        assert_eq!(rig_of(with_shade(spawn, 200, ext())), 1000); // the ground-shade ramp (writer 5)
        let indoor = with_interior_probe(spawn, 4321); // the classifier's Bake law (writer 2)
        assert_eq!(rig_of(indoor), 1000);
        assert_eq!((indoor & PROBE_MASK) >> PROBE_SHIFT, 4321);
        assert_eq!(
            indoor & INTERIOR_FOG_BIT,
            0,
            "payload only — the flag is decided apart"
        );
        let outdoor = with_exterior_reset(indoor); // …and its outdoor reclaim
        assert_eq!(rig_of(outdoor), 1000);
        assert_eq!(outdoor & (PROBE_MASK | INTERIOR_FOG_BIT), 0);
        assert_eq!(outdoor & ALPHA_MASK, ALPHA_MASK);
        // Alpha and probe compose under the rig field.
        let t = with_alpha(indoor, 0.5);
        assert_eq!(rig_of(t), 1000);
        assert_eq!((t & PROBE_MASK) >> PROBE_SHIFT, 4321);
    }

    #[test]
    fn interior_fog_bit_survives_the_field_writers() {
        // A feathering or MCSH-ramping indoor unit keeps its room fog.
        let t = INTERIOR_FOG_BIT | alpha_bits(1.0);
        assert_eq!(with_alpha(t, 0.25) & INTERIOR_FOG_BIT, INTERIOR_FOG_BIT);
        assert_eq!(
            with_shade(t, 191, ext()) & INTERIOR_FOG_BIT,
            INTERIOR_FOG_BIT
        );
        // It never leaks into the payload fields.
        assert_eq!(shade_of(t, ext()), 0);
        assert_eq!(t & ALPHA_MASK, ALPHA_MASK);
        assert_eq!((probe_bits(6660) & PROBE_MASK) >> PROBE_SHIFT, 6660);
    }

    #[test]
    fn a_law_rewrite_carries_the_fade_alpha() {
        let mid_ramp = alpha_bits(0.25);
        // Into the interior lane and back out again, at a quarter alpha the whole way.
        let indoor = with_interior_probe(rig_bits(7) | mid_ramp, 1234);
        assert_eq!(
            indoor & ALPHA_MASK,
            mid_ramp,
            "the ramp survives the re-lane"
        );
        assert_eq!((indoor & PROBE_MASK) >> PROBE_SHIFT, 1234);
        assert_eq!(rig_of(indoor), 7);
        let outdoor = with_exterior_reset(indoor);
        assert_eq!(outdoor & ALPHA_MASK, mid_ramp, "and the reclaim too");
        assert_eq!(rig_of(outdoor), 7);
        assert_eq!(outdoor & (PROBE_MASK | INTERIOR_FOG_BIT), 0);
        // An opaque part's Bake payload is the spawn constructor's, less the fog flag.
        assert_eq!(
            with_interior_fog(with_interior_probe(alpha_bits(1.0), 1234), true),
            probe_bits(1234),
            "an opaque part's Bake payload is unchanged"
        );
        assert_eq!(with_exterior_reset(probe_bits(1234)), alpha_bits(1.0));
        // The untagged sentinel becomes opaque, never alpha 0.
        assert_eq!(with_interior_probe(0, 1234) & ALPHA_MASK, ALPHA_MASK);
        assert_eq!(with_exterior_reset(0) & ALPHA_MASK, ALPHA_MASK);
    }

    /// A shade write into a probe payload leaves a valid, wrong slot, `(slot & 0x1f00) | byte`:
    /// another prop's probe, or a zeroed row that draws black, with no error anywhere.
    #[test]
    fn a_shade_write_renames_a_probe_slot_instead_of_breaking_it() {
        let slot = 440u16; // 0b1_1011_1000: bits 14..=18 hold 1, bits 6..=13 hold 184
        let tag = with_shade(probe_bits(slot), 191, ext());
        assert_eq!(
            (tag & PROBE_MASK) >> PROBE_SHIFT,
            447,
            "the boat's shade byte replaced the slot's low 8 bits: 1<<8 | 191"
        );
        assert_eq!(
            alpha_of(tag),
            1.0,
            "and the alpha field rode through intact"
        );
    }

    #[test]
    fn with_shade_materializes_the_untagged_sentinel_as_opaque() {
        let t = with_shade(0, 128, ext());
        assert_eq!(t & ALPHA_MASK, ALPHA_MASK);
        assert_eq!(shade_of(t, ext()), 128);
        // Likewise under the highlight bit, whose payload still reads 0.
        let t = with_shade(HIGHLIGHT_BIT, 128, ext());
        assert_eq!(t & ALPHA_MASK, ALPHA_MASK);
    }
}
