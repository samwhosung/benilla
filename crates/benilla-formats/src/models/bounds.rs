//! An M2's authored bounds ([`M2Bounds`]) — the header sphere/box, the vertex-derived radius our
//! renderer computes, and the selection-ring **footprint** read from the model's Stand-animation
//! bounding box (the exact model-local input the real client's living-unit selection ring uses).
//! No render-batch concern here — just the bounds/measurement reads [`load_m2_bounds`]/
//! [`parse_m2_bounds`] expose to placement + the selection ring.

use std::io::Cursor;

use anyhow::{Context, Result};
use benilla_bytes::ByteExt;
use benilla_m2::parse_m2;

use crate::Chain;

use super::model_path;

/// An M2's authored bounds (read straight from the model header) alongside the vertex-derived radius
/// our renderer currently computes — so the distance-fade size bucket can be checked against the value
/// the reference's placement builder actually uses (`FUN_00694bc0` → `rec+0x68`). `sphere_radius` and
/// `bbox_*` are in model-local yards (multiply by the placement scale for the world radius).
#[derive(Clone, Copy, Debug)]
pub struct M2Bounds {
    /// `M2Header.bounding_sphere_radius` — the authored bounding-sphere radius (the reference's source).
    pub sphere_radius: f32,
    /// `M2Header.bounding_box_min/max` — the authored AABB; its centre is the sphere centre.
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Max vertex distance from the model **origin** — what `build_model` currently uses as the radius.
    pub vert_max_from_origin: f32,
    /// The selection-ring **footprint** (model-local, pre-scale): `sqrt(0.5 · sqrt(dx² + dy²))`, where
    /// `dx,dy` are the horizontal (X,Y) extents of the unit's **Stand** animation bounding box (the M2
    /// sequence CAaBox). This is the exact model-local input the real client's living-unit selection ring
    /// uses — matched to the reference pixels (`0x608e00`/`0x60aee0`): the ring's world radius =
    /// this × `OBJECT_FIELD_SCALE_X`. The nested sqrt is a range-compressor (which is why the
    /// render-sphere never fit). Falls back to the header render-box XY extents for a model with no
    /// animation sequences.
    ///
    /// A **degenerate** box — `max.x == min.x` *and* `max.y == min.y` — takes
    /// [`DEGENERATE_RING_FOOTPRINT`] instead of the formula, which is the writer's own first branch
    /// and not a floor of ours (decision 1658).
    pub ring_footprint: f32,
    /// Model-space **Z** of attachment id 17 — the reference's follow-camera pivot height
    /// (`0x50ca90`: `feet + (attach17.z + 0.0972)·scale`). `None` for a model with no slot-17
    /// attachment; the camera then falls back to a fraction of the box. See [`M2Bounds::pivot_z`].
    pub pivot_z: Option<f32>,
    /// The **Stand** animation box's vertical extent, `max.z − min.z` (model-local, pre-scale) — the
    /// chat bubble's anchor height above the unit's feet (× model scale, + 0.7 yd).
    ///
    /// The same `0x711a20` query the selection ring above is sized from, taking the box's Z instead
    /// of its XY: `0x4b0e38 call 0x711a20` reads the **MD20 header image** — file bytes, no bone
    /// matrix anywhere in the call tree — and returns `out+0x20 − out+0x14`, which is that
    /// CAaBox's Z extent. Benilla anchored the bubble on the posed PlayerName attachment instead,
    /// on a recorded INFERRED claim that `0x608640` and `0x711a20` were "both the head-region
    /// attachment height"; that equivalence is **REFUTED** — they differ precisely on
    /// animated-vs-static, `0x608640` reading the live posed palette (it bobs) and this reading a
    /// file constant. The overhead *name* keeps `0x608640`; only the bubble takes this. See 1406.
    ///
    /// Falls back to the header render-box Z extent for a model with no sequences, like the ring.
    pub stand_box_z: f32,
    /// How far the camera's framing pivot drops when this body **swims** (model-local yards,
    /// pre-scale): `StandSeq.bounds.max.z − SwimSeq.bounds.max.z`, floored at zero.
    ///
    /// The reference keeps **three** framing-pivot presets side by side (`cam+0x11c`/`+0x120`/
    /// `+0x124`, rebuilt from the model by `0x50ca90`) and picks one per frame at `0x50f880`. All
    /// three start from the same base — the `attach17.z + 0.0972222 [0x808ab0]` neck height
    /// [`M2Bounds::pivot_z`] carries — and only the swim one is then pulled down, by exactly this:
    /// `0x50ccf6 fsubr [esi+0x124]` subtracts `S · (box0.max.z − box42.max.z)`, where the two boxes
    /// come from `0x711a20(model, 0)` and `0x711a20(model, 0x2a)` — the same `M2Sequence` `CAaBox`
    /// query [`stand_box`] reads, on animation id **0** (Stand) and id **42** (Swim). Off the
    /// shipped `HumanMale.m2` at scale 1 it gives `+0x11c = +0x120 = 1.9002692` and
    /// `+0x124 = 1.5120120` — a drop of `0.3882572`, the number the fixture test pins.
    ///
    /// It is a **delta**, not a height, because that is the shape of the byte: the base is added to
    /// all three presets at `0x50cc0c`–`0x50cc2e` and only `+0x124` is decremented, so a consumer
    /// gets the swim preset as `pivot_height − this`, then the shared `[5/6, 15.0]` clamp
    /// (`0x50ca90`, `0x50d00d`–`0x50d092`).
    ///
    /// **`0.0` when the model has no Swim sequence** — which is every non-character model, and the
    /// reference's own answer too: `0x50cc67`–`0x50cd02` runs only when `0x711960` reports *both*
    /// id 0 and id 42 present, so a body that cannot swim keeps the standing preset in every state.
    ///
    /// **The zoom pair is NOT built.** `0x50f880`'s other leg picks `+0x11c` (zoomed-in) or
    /// `+0x120` (zoomed-out) on `cam+0x198 < 1.8315` — and on a scale-1 human the reference computes
    /// those two *equal*, so nothing here yet says what makes them differ. benilla has one standing
    /// preset and this drop; the threshold and whatever authors the zoomed pair apart are absent,
    /// not stubbed.
    pub swim_pivot_drop: f32,
}

/// Read an M2's authored bounds + the vertex-derived radius (see [`M2Bounds`]). Uses the same path
/// normalisation as [`super::load_m2_mesh`] so `.mdx`/`.mdl` doodad paths resolve identically.
pub fn load_m2_bounds(chain: &mut Chain, raw_path: &str) -> Result<M2Bounds> {
    let path = model_path(raw_path);
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading M2 {path}"))?;
    parse_m2_bounds(&bytes).with_context(|| format!("parsing M2 bounds {path}"))
}

/// The ring footprint the real client stores for a **degenerate** box — one whose X *and* Y extents
/// are both exactly zero. Byte-read at `0x60af4f..0x60af67`: the
/// writer `0x60aee0` compares `max.x==min.x` and `max.y==min.y` and, when both hold, stores the
/// literal `0x3f99999a` = **1.2** into `[unit+0xcf0]` without ever running the `sqrt(0.5·sqrt(dx²+dy²))`
/// formula. It never fires for the four life-size units measured, but it does for the whole
/// trigger-creature family: an `InvisibleStalker` body authors all 135 sequence boxes at zero, so
/// this **is** its ring — and the Naxxramas weapon mobs, whose visible self is the axe in that
/// body's hand, are exactly where a player sees it. Ours read 0 and drew a ring the width of a coin
/// (decision 1658).
///
/// It is the model-less fallback too: "no box to measure" and "a box that measures zero" are the
/// same question, and this is the reference's answer to it.
pub const DEGENERATE_RING_FOOTPRINT: f32 = 1.2;

/// Resolve an animation **id** to its `M2Sequence` record index — the animation-lookup indirection
/// every sequence-box read below goes through. The lookup array is at MD20 `0x24`(count)/`0x28`
/// (offset), one `u16` per animation id; the sequence array it indexes is at `0x1c`/`0x20`. An id
/// is NOT a record number (a chicken's record 0 is a flap, its Stand sits at index 2), and a model
/// that simply does not author an animation is the common case, not an error: `None` for a missing
/// lookup, an id past its end, the `0xffff` "absent" sentinel, or an index past the sequence array.
///
/// This is the client's own `0x711960(id)` presence test folded into the `0x711a20(id)` box query —
/// the pair `0x50ca90` calls on ids 0 and 42 before it will build the swim preset at all.
fn seq_index(bytes: &[u8], anim_id: usize) -> Option<usize> {
    let anim_count = bytes.u32_at(0x1c)? as usize;
    let lookup_count = bytes.u32_at(0x24)? as usize;
    let lookup_ofs = bytes.u32_at(0x28)? as usize;
    if anim_count == 0 || anim_id >= lookup_count {
        return None;
    }
    match bytes.u16_at(lookup_ofs.checked_add(anim_id * 2)?) {
        Some(i) if (i as usize) != 0xffff && (i as usize) < anim_count => Some(i as usize),
        _ => None,
    }
}

/// The byte offset of a resolved sequence record — stride `0x44` from the array at MD20 `0x20`,
/// with the `CAaBox` at record `+0x24` (min `C3Vector` `+0x24`, max `C3Vector` `+0x30`, so `max.z`
/// is `+0x38`).
fn seq_record(bytes: &[u8], idx: usize) -> Option<usize> {
    let anim_ofs = bytes.u32_at(0x20)? as usize;
    anim_ofs.checked_add(idx * 0x44)
}

/// The **Stand** sequence's record — animation id 0 through [`seq_index`], falling back to record 0
/// for a model whose lookup is absent or points nowhere (the pre-existing behaviour the ring and
/// the chat bubble are pinned to; every model that has sequences at all has *something* to stand on).
/// `None` only when the model has no sequences.
fn stand_record(bytes: &[u8]) -> Option<usize> {
    if bytes.u32_at(0x1c)? as usize == 0 {
        return None;
    }
    seq_record(bytes, seq_index(bytes, 0).unwrap_or(0))
}

/// One sequence box's `max.z` — `rec+0x38`, the `out+0x20` field `0x711a20` returns. The camera's
/// swim preset is the difference of two of these.
fn seq_max_z(bytes: &[u8], anim_id: usize) -> Option<f32> {
    let rec = seq_record(bytes, seq_index(bytes, anim_id)?)?;
    bytes.f32_at(rec + 0x38)
}

/// Read the horizontal (X,Y) extents of the **Stand** animation's bounding box from a raw M2 — the input
/// the real client's living-unit selection ring is sized from (`0x60aee0`).
/// The animation `M2Sequence` array is at MD20 `0x1c`(count)/`0x20`(offset), stride `0x44`, with the
/// sequence's `CAaBox` at record `+0x24` (min C3 `+0x24`, max C3 `+0x30`). Stand is animation **id 0**,
/// whose *sequence index* is `animationLookup[0]` (array at `0x24`/`0x28`, `u16` each) — NOT necessarily
/// record 0 (a chicken's record 0 is a flap, its Stand sits at index 2) — resolved by [`seq_index`].
/// Returns `(dx, dy, dz)`, or `None` when the model has no sequences (caller falls back to the header
/// box). VERIFIED: reproduces the reference ring radii to ~1 mm.
///
/// The Z extent joins the pair the ring needs because the chat bubble's anchor is the **same box**
/// read on the other axis (1406) — one parse, one Stand-sequence resolution, two consumers, so the
/// ring and the bubble can never drift onto different animations.
fn stand_box(bytes: &[u8]) -> Option<(f32, f32, f32)> {
    let rec = stand_record(bytes)?;
    // CAaBox @ rec+0x24: min C3 (+0x24), max C3 (+0x30). Horizontal (X,Y) extents; squared in the ring
    // formula so an unsorted box is harmless.
    let dx = bytes.f32_at(rec + 0x30)? - bytes.f32_at(rec + 0x24)?;
    let dy = bytes.f32_at(rec + 0x34)? - bytes.f32_at(rec + 0x28)?;
    let dz = bytes.f32_at(rec + 0x38)? - bytes.f32_at(rec + 0x2c)?;
    Some((dx, dy, dz))
}

/// `AnimationData` id **42** — Swim. The second box `0x50ca90` queries (`0x50ccde call 0x711a20`
/// with `seq=0x2a`) when it builds the camera's swim framing-pivot preset.
const SWIM_ANIM_ID: usize = 42;

/// The camera's swim pivot drop (see [`M2Bounds::swim_pivot_drop`]): `StandSeq.bounds.max.z −
/// SwimSeq.bounds.max.z`, floored at zero, and `0.0` for any model missing either sequence — the
/// reference's own both-present guard (`0x711960` on ids 0 and `0x2a` at `0x50cc43`/`0x50cc4f`).
///
/// Stand resolves through [`stand_record`], not [`seq_index`] alone, so the standing side of the
/// subtraction is byte-for-byte the box the ring and the chat bubble already read.
fn swim_pivot_drop(bytes: &[u8]) -> f32 {
    let Some(stand_z) = stand_record(bytes).and_then(|rec| bytes.f32_at(rec + 0x38)) else {
        return 0.0;
    };
    let Some(swim_z) = seq_max_z(bytes, SWIM_ANIM_ID) else {
        return 0.0;
    };
    (stand_z - swim_z).max(0.0)
}

/// Read an **in-memory** M2's authored bounds + vertex-derived radius (see [`M2Bounds`]). The bytes-in
/// entry point for the Bevy `AssetLoader`; [`load_m2_bounds`] is the chain-reading wrapper.
pub fn parse_m2_bounds(bytes: &[u8]) -> Result<M2Bounds> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    let model = format.model();
    let h = &model.header;
    let vert_max_from_origin = model
        .vertices
        .iter()
        .map(|v| {
            let p = v.position;
            (p.x * p.x + p.y * p.y + p.z * p.z).sqrt()
        })
        .fold(0.0_f32, f32::max);
    // Selection-ring footprint: `sqrt(0.5 · sqrt(dx² + dy²))` of the Stand animation box's horizontal
    // extents (the real client's living-unit ring input), falling back to the header render-box XY for a
    // model with no animation sequences (the client's static-model path).
    let (rx, ry, rz) = stand_box(bytes).unwrap_or((
        h.bounding_box_max[0] - h.bounding_box_min[0],
        h.bounding_box_max[1] - h.bounding_box_min[1],
        h.bounding_box_max[2] - h.bounding_box_min[2],
    ));
    let ring_footprint = if rx == 0.0 && ry == 0.0 {
        DEGENERATE_RING_FOOTPRINT
    } else {
        (0.5 * (rx * rx + ry * ry).sqrt()).sqrt()
    };
    Ok(M2Bounds {
        sphere_radius: h.bounding_sphere_radius,
        bbox_min: h.bounding_box_min,
        bbox_max: h.bounding_box_max,
        vert_max_from_origin,
        ring_footprint,
        pivot_z: model.pivot_attach_z,
        stand_box_z: rz.max(0.0),
        swim_pivot_drop: swim_pivot_drop(bytes),
    })
}
