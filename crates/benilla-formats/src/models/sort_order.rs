//! The M2 scene's transparent order (`0x70ae10`): the keys one element carries and the comparator
//! that sorts lists 1 and 2 (`0x70aa00`). A batch's file keys are read once at load
//! ([`BatchOrder`]); the per-frame keys, the two depths and the instance, are the drawer's
//! ([`TransparentElement`]).
//!
//! The keys, at the default `M2Faster` 1, whose scene mode is `0x17` (`0x4013b1`):
//!
//! 1. under mode bit 4, the kind's priority (`0x873184`: batches 0, ribbons and emitters 1), so
//!    every batch draws before every ribbon and particle. Not built here: [`transparent_cmp`]
//!    orders batches only, and benilla's ribbons and particles keep their own place in the list;
//! 2. `+0x14` descending: the instance key under depth fill, else the batch's own centre depth;
//! 3. `+0x8` descending: a depth-fill copy before every element of its key;
//! 4. `+0x28` ascending, signed: the texture unit's priority plane (`0x707c76`);
//! 5. under mode bit 2, `+0x38` ascending: the vertex shader id (`0x70aeed`);
//! 6. `+0x18` descending: the batch centre's depth, the largest float for a depth-writing batch
//!    under depth fill (`0x707fb3`);
//! 7. the instance, then the kind (all 0 here);
//! 8. for kinds 0 to 2, the unit's layer `+0x0c` ascending (`0x70afa3`);
//! 9. the opaque order (`0x70aa30`): inside one instance, the submesh's first bone lookup, the
//!    material's flags and blend, the textures, their count, the coordinate entries and the unit.

use std::cmp::Ordering;

/// One batch's file keys for the transparent order. Floats are kept as bits so a material key can
/// hash it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BatchOrder {
    /// The texture unit's priority plane (`+0x02`, signed), the element's `+0x28`.
    pub plane: i16,
    /// The vertex shader id (`0x71de50`) after the scene's remap (`0x70b040`), without the
    /// light count a lit batch adds (`0x707cf6`): one instance's lit batches all add the same.
    pub vertex_shader: u32,
    /// The unit's layer on its submesh (`+0x0c`), compared after the instance (`0x70afa3`).
    pub layer: u16,
    /// The material writes depth (flag `0x10` clear): under depth fill such a translucent batch
    /// takes the largest float as its centre depth (`0x707fa7`, `0x707fb3`).
    pub depth_write: bool,
    /// The submesh's first bone lookup (`+0x0e`), the opaque order's first key inside one
    /// instance (`0x70ab21`).
    pub bone_lookup: u16,
    /// The material's flags and blend words (`0x70aba7`, `0x70abc1`).
    pub material_flags: u16,
    pub blend: u16,
    /// Each stage's texture: one plus the first index in the model's texture table that loads
    /// the same texture (the same file, or the same replaceable type), 0 past the table.
    ///
    /// Deviation: the reference compares the texture objects' addresses (`0x41af70` on instance
    /// `+0xa4`, `0x70ac45`-`0x70ac69`); the model's own texture order stands in for them, because
    /// those addresses are a fact of the run's allocator and not of the file. Entries sharing a
    /// handle still tie.
    pub textures: [u16; 2],
    /// The unit's texture count (`+0x0e`), which bounds the stage compares (`0x70ac96`).
    pub texture_count: u16,
    /// Each stage's coordinate entry (header `+0xa0` at unit `+0x12`, `0x70acbd`).
    pub coords: [u16; 2],
    /// The unit's index in its view: the unit pointer, compared last (`0x70ad14`).
    pub unit: u16,
    /// The submesh's depth-key centre (`+0x14`), model space, raw WoW axes, as `f32` bits.
    pub center: [u32; 3],
    /// The bone the centre rides (`+0x12`, `0x707d6a`).
    pub center_bone: u16,
}

impl BatchOrder {
    /// The depth-key centre as floats, raw WoW axes.
    pub fn center(&self) -> [f32; 3] {
        self.center.map(f32::from_bits)
    }
}

/// The vertex shader id of a batch (`0x71de50`), `5·(3·(3·Z + t0) + t1)` with
/// `Z = G + 2·L + 4·B + 20·P`, through the scene's remap of `B` (`0x70b040`): under mode `0x10`
/// a nonzero bone-weight count becomes 4. `P` is 0, as pixel shaders are not requested at the
/// default `M2UsePixelShaders` 0. `stages[k]` is stage `k`'s digit: 0 past the texture count,
/// 2 for a generated environment coordinate, 1 for a texture transform, else 0.
pub fn vertex_shader_id(material_flags: u16, bone_influences: u16, stages: [u32; 2]) -> u32 {
    let g = u32::from(material_flags & 0x2 == 0); // fogged
    let l = u32::from(material_flags & 0x1 == 0); // lit
                                                  // Mode `0x17` has `0x10` and not `0x8`: `neg; sbb; and 4` (`0x70b083`).
    let b = if bone_influences != 0 { 4 } else { 0 };
    let z = g + 2 * l + 4 * b;
    5 * (3 * (3 * z + stages[0]) + stages[1])
}

/// One element of a transparent list, as `0x70ae10` compares it.
#[derive(Debug, Clone, Copy)]
pub struct TransparentElement<'a> {
    pub order: &'a BatchOrder,
    /// `+0x14`, a squared distance: larger draws first.
    pub key_depth: f32,
    /// `+0x8`: the depth-fill copy, which draws before its key's other elements.
    pub fill_copy: bool,
    /// `+0x18`, a squared distance or the largest float: larger draws first.
    pub batch_depth: f32,
    /// The instance, compared ascending (`0x70af56`): any fixed order of instances.
    pub instance: u64,
}

/// A float key compared as the reference does (`fcomp`, `test ah,0x41` then `test ah,5`): the
/// greater first, an unordered pair equal.
fn far_first(a: f32, b: f32) -> Ordering {
    if a > b {
        Ordering::Less
    } else if a < b {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// The transparent order (`0x70ae10`) between two batches: [`Ordering::Less`] draws `a` first.
pub fn transparent_cmp(a: &TransparentElement<'_>, b: &TransparentElement<'_>) -> Ordering {
    far_first(a.key_depth, b.key_depth).then_with(|| tie_cmp(a, b))
}

/// [`transparent_cmp`] past its first key.
fn tie_cmp(a: &TransparentElement<'_>, b: &TransparentElement<'_>) -> Ordering {
    let (oa, ob) = (a.order, b.order);
    // `+0x8`, signed, the greater first (`0x70aea1`).
    b.fill_copy
        .cmp(&a.fill_copy)
        // The priority plane, signed (`0x70aec7`).
        .then(oa.plane.cmp(&ob.plane))
        // Mode bit 2 (`0x70aeed`), unsigned.
        .then(oa.vertex_shader.cmp(&ob.vertex_shader))
        // `+0x18` (`0x70af18`).
        .then_with(|| far_first(a.batch_depth, b.batch_depth))
        // The instance (`0x70af56`); the kind ties for batches.
        .then(a.instance.cmp(&b.instance))
        // The layer, unsigned (`0x70afab`).
        .then(oa.layer.cmp(&ob.layer))
        .then_with(|| opaque_tail(oa, ob))
}

/// The opaque order (`0x70aa30`) between two batches of one instance and model, whose model,
/// instance, shader and own-table keys tie: the submesh's bone lookup, then the material, the
/// textures and the unit.
fn opaque_tail(a: &BatchOrder, b: &BatchOrder) -> Ordering {
    let n = usize::from(a.texture_count.min(b.texture_count)).min(2);
    a.bone_lookup
        .cmp(&b.bone_lookup)
        .then(a.material_flags.cmp(&b.material_flags))
        .then(a.blend.cmp(&b.blend))
        .then(a.textures[..n].cmp(&b.textures[..n]))
        .then(a.texture_count.cmp(&b.texture_count))
        .then(a.coords[..n].cmp(&b.coords[..n]))
        .then(a.unit.cmp(&b.unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(order: &BatchOrder, key: f32, batch: f32) -> TransparentElement<'_> {
        TransparentElement {
            order,
            key_depth: key,
            fill_copy: false,
            batch_depth: batch,
            instance: 1,
        }
    }

    /// `HangingSkullLight01`'s pair under depth fill: the additive glow (plane 1) is batch 0, the
    /// skull (plane 0) batch 1; one instance key, so the plane puts the skull first.
    #[test]
    fn the_plane_orders_one_instance_ahead_of_file_order_and_depth() {
        let glow = BatchOrder {
            plane: 1,
            unit: 0,
            ..Default::default()
        };
        let skull = BatchOrder {
            plane: 0,
            unit: 1,
            ..Default::default()
        };
        // The glow's centre farther than the skull's: depth and file order both put it first.
        let (g, s) = (el(&glow, 9.0, 9.5), el(&skull, 9.0, 8.0));
        assert_eq!(
            transparent_cmp(&s, &g),
            Ordering::Less,
            "the skull, then the glow"
        );
    }

    /// A model frame has no depth fill (`0x76ccf5`, `0x707a48`): `+0x14` is each batch's own
    /// centre, and it leads the plane.
    #[test]
    fn without_depth_fill_the_far_batch_leads_the_plane() {
        let near_low = BatchOrder::default();
        let far_high = BatchOrder {
            plane: 4,
            ..Default::default()
        };
        let (a, b) = (el(&near_low, 10.0, 10.0), el(&far_high, 40.0, 40.0));
        assert_eq!(transparent_cmp(&b, &a), Ordering::Less);
    }

    /// Inside one plane the vertex shader id leads the centre depth (`0x70aeed`), and a
    /// depth-writing batch at the largest float ties through to the layer, not the file order.
    #[test]
    fn the_shader_then_the_depth_then_the_layer() {
        let unlit = BatchOrder {
            vertex_shader: 30,
            ..Default::default()
        };
        let lit = BatchOrder {
            vertex_shader: 60,
            ..Default::default()
        };
        assert_eq!(
            transparent_cmp(&el(&unlit, 1.0, 1.0), &el(&lit, 1.0, 50.0)),
            Ordering::Less,
            "the lower id first, whatever the depth"
        );
        let first_in_file = BatchOrder {
            layer: 1,
            unit: 0,
            ..Default::default()
        };
        let second_in_file = BatchOrder {
            layer: 0,
            unit: 1,
            ..Default::default()
        };
        let (a, b) = (
            el(&first_in_file, 1.0, f32::MAX),
            el(&second_in_file, 1.0, f32::MAX),
        );
        assert_eq!(transparent_cmp(&b, &a), Ordering::Less, "layer 0 first");
        let (a, b) = (el(&first_in_file, 1.0, 2.0), el(&second_in_file, 1.0, 3.0));
        assert_eq!(
            transparent_cmp(&b, &a),
            Ordering::Less,
            "a finite depth decides first"
        );
    }

    /// A depth-fill copy draws before every element of its key (`0x707ff7`, `0x70aea1`).
    #[test]
    fn a_fill_copy_leads_its_instance() {
        let o = BatchOrder {
            plane: 3,
            ..Default::default()
        };
        let p = BatchOrder::default();
        let mut copy = el(&o, 5.0, f32::MAX);
        copy.fill_copy = true;
        assert_eq!(transparent_cmp(&copy, &el(&p, 5.0, 1.0)), Ordering::Less);
        assert_eq!(
            transparent_cmp(&el(&p, 6.0, 1.0), &copy),
            Ordering::Less,
            "but not before a farther key"
        );
    }

    /// The texture compares stop at the shorter unit, then the count decides (`0x70ac96`).
    #[test]
    fn textures_compare_over_the_shorter_unit_then_by_count() {
        let one = BatchOrder {
            textures: [3, 0],
            texture_count: 1,
            unit: 0,
            ..Default::default()
        };
        let two = BatchOrder {
            textures: [3, 1],
            texture_count: 2,
            unit: 1,
            ..Default::default()
        };
        assert_eq!(opaque_tail(&one, &two), Ordering::Less);
        let other = BatchOrder {
            textures: [2, 9],
            texture_count: 2,
            unit: 2,
            ..Default::default()
        };
        assert_eq!(
            opaque_tail(&other, &one),
            Ordering::Less,
            "texture 2 before 3"
        );
    }

    /// An unordered depth compares equal and passes to the next key (`test ah,5`, `jp`).
    #[test]
    fn a_nan_depth_ties() {
        let a = BatchOrder {
            plane: 0,
            ..Default::default()
        };
        let b = BatchOrder {
            plane: 1,
            ..Default::default()
        };
        assert_eq!(
            transparent_cmp(&el(&b, f32::NAN, 1.0), &el(&a, 2.0, 1.0)),
            Ordering::Greater
        );
    }

    /// `Z = G + 2L + 4B + 20P`, then the stage digits (`0x71de50`); mode `0x17` folds every
    /// weighted submesh's `B` to 4 (`0x70b040`).
    #[test]
    fn the_vertex_shader_id_is_mixed_radix_with_b_remapped() {
        // Lit, fogged, unweighted, one plain stage: Z = 3.
        assert_eq!(vertex_shader_id(0, 0, [0, 0]), 5 * (3 * (3 * 3)));
        // Unlit and unfogged, B 1 → 4, an env stage then a transformed one: Z = 16.
        assert_eq!(vertex_shader_id(0x3, 1, [2, 1]), 5 * (3 * (3 * 16 + 2) + 1));
        assert_eq!(
            vertex_shader_id(0, 3, [0, 0]),
            vertex_shader_id(0, 1, [0, 0]),
            "every nonzero weight count is 4"
        );
        assert!(
            vertex_shader_id(0x1, 0, [0, 0]) < vertex_shader_id(0, 0, [0, 0]),
            "unlit before lit"
        );
    }
}
