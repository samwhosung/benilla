//! The celestial draw-order ladder — the reference's **fixed sky pass order**, expressed as
//! `Transparent3d` sort biases.
//!
//! The real client draws the frame `sky → opaque world → weather → glare`, and inside its sky pass
//! (`CSky::Render 0x6d4940`, one squashed depth slice `[0.975, 0.98]`, depth-write off, painter's
//! order): **stars → sun disc → white moon → moon02 → gradient strip (additive) → cloud dome** —
//! so the clouds composite over a setting sun, rain falls in front of the whole sky, and the glare
//! quads (their own back slice `[0.995, 1.0]`, the frame's LAST draw) paint over everything the
//! z-buffer leaves visible.
//!
//! Bevy instead sorts every transparent by `view-z of the mesh center + depth_bias`, drawn
//! back-to-front — and our celestial entities are camera-anchored, so their raw distances are
//! sort-order accidents (the camera-centred cloud dome sat at ~0 = drawn LAST, painting clouds
//! over rain and over the glare — backwards on both counts). These biases turn the accident back
//! into the reference's fixed order: spaced far below any real in-scene view-z (≳ −far plane, a
//! few ×10³) so world distances can never reorder the sky and every world transparent draws over
//! it, with the glare *above* zero so it draws after the world and the rain — but still below
//! [`Rung::NAMEPLATE`], which keeps world text on top of the flare (the
//! reference draws its text later still).
//!
//! Precipitation deliberately has NO bias: an unbiased view-z of a few tens of units lands rain
//! after the biased sky and before the glare — the reference's `weather` slot — without pinning
//! rain-vs-world-transparent order we have no byte law for.
//!
//! ## Which way the bias points — the sign, read off Bevy 0.18 (never guessed again)
//!
//! Every rung below is a *signed* number in one direction, and the direction was inverted here for
//! a week (decision 0639 — the whole ladder ran upside down; names sorted UNDER the water that
//! erased them). Verified in the bevy 0.18.1 sources, so the next edit can check rather than
//! recall:
//!
//! - `ViewRangefinder3d::distance` (`bevy_render/src/render_phase/rangefinder.rs`) returns the
//!   **view-space z** of the point — *negative* in front of the camera, and MORE negative the
//!   farther away (its own unit test puts points behind the eye at `+1`/`+2`).
//! - `Transparent3d` sorts **ascending** on that value (`bevy_core_pipeline/src/core_3d/mod.rs`:
//!   "Values increase towards the camera. Back-to-front ordering for transparent means we need an
//!   ascending sort") — so the first item drawn is the most negative = the farthest.
//! - The queue adds the material's bias to it (`bevy_pbr/src/material.rs`,
//!   `RenderPhaseType::Transparent`: `rangefinder.distance(&center) + depth_bias`).
//!
//! ⇒ **A POSITIVE bias draws LATER (on top); a NEGATIVE bias draws EARLIER (behind).** Bevy's own
//! `StandardMaterial::depth_bias` doc says it in words: "a positive depth bias will render closer
//! to the camera while negative values cause the material to render behind other objects".
//!
//! The magnitude is not free, because the same field is *also* the rasterizer's depth bias
//! (`bevy_pbr/src/pbr_material.rs` packs it into the pipeline key → `DepthBiasState::constant`,
//! applied in depth ULPs ≈ 2⁻²³ *relative*, the same knob [`crate::target::ring`] uses at 8192 to
//! beat coplanar noise). It is harmless for the sky rungs (every sky fragment overwrites its own
//! depth, below) but it perturbs the depth TEST of anything depth-tested, so the world-side rungs
//! stay as small as their ordering job allows.
//!
//! ## The depth law — every sky vertex pins the far depth
//!
//! The biases above order the sky *against itself*. What orders it against the **world** is depth,
//! and there the reference gives one rule for the whole pass: the sky draws FIRST, in a squashed
//! back slice (`[0.975, 0.98]`, the glare further back at `[0.995, 1.0]`), depth-write off — so the
//! opaque world simply paints over it, and no sky element can ever land in front of world geometry.
//!
//! Our sky draws *after* the world (Bevy's transparent pass), so the depth **test** has to do that
//! job — which it only does if the sky's depth is behind everything. Each shell used to rely on its
//! own camera-anchored radius for that (`far·0.85` discs, `far·0.87` clouds, `far·0.88` stars,
//! `far·0.9` gradient dome), on the assumption that world geometry is always nearer. **It isn't:**
//! the WDL horizon ring ([`crate::wdl`]) streams ±5 tiles ≈ 2.9 km and is drawn out to the far plane
//! (3 km), so distant hills land in a band *behind* every shell — and stars, clouds and discs then
//! passed the depth test in front of terrain the reference would have occluded them with (the
//! sighting: stars showing *through* a fogged mountain range at night, decision 0588).
//!
//! So every sky VERTEX pins its clip z to `SKY_FAR_CLIP_Z = 0.0` — reverse-Z "infinitely far" —
//! under Bevy's `GreaterEqual` test, in the one vertex stage every sky material shares
//! (`sky_vertex.wgsl`, [`SKY_VERTEX_SHADER`]; `sky.wgsl`, `star.wgsl`, `cloud.wgsl` and
//! `celestial.wgsl` are fragment stages that write colour only). Clip z = 0 interpolates to NDC
//! depth 0 for every fragment whatever w is, so a sky element survives only where the depth
//! buffer still holds the clear value: exactly "the world paints over the sky", independent of
//! any shell radius. The shells now decide only *screen size and sky-internal parallax*, never
//! occlusion.
//!
//! It was a `@builtin(frag_depth)` write in each fragment shader until decision 2016. Same number,
//! wrong stage: a pipeline whose fragment decides its own depth cannot be early-Z rejected, so
//! every dome fragment under a hill, a wall or a leaf was shaded in full and discarded late — a
//! full-screen gradient (and the cloud dome, and the star patches) paid for the covered fraction
//! of the screen on every frame. Pinned at the vertex the rasterizer rejects them first. The
//! rasterizer's `DepthBiasState` constant then has to be zero on these pipelines
//! ([`sky_pipeline_state`]): the rung each sky material carries in `depth_bias` is a SORT key,
//! and the base `StandardMaterial` packs the same f32 into the bias constant — harmless while
//! the fragment overwrote its depth, a perturbation of the pinned zero once it doesn't.

use bevy::render::render_resource::RenderPipelineDescriptor;

/// The one vertex stage every sky material draws through — the far-depth pin (module doc, "The
/// depth law"). A sky `MaterialExtension` returns this from `vertex_shader()`; the test below
/// holds each of them to it.
pub(crate) const SKY_VERTEX_SHADER: &str = "embedded://benilla_world/shaders/sky_vertex.wgsl";

/// The pipeline state every sky material's `specialize` applies (module doc, "The depth law"):
/// the rasterizer depth-bias constant back to zero, so the sort rung the material carries in
/// `StandardMaterial::depth_bias` stays sort-only and the vertex stage's pinned far depth reaches
/// the depth test untouched. Same split the model lane makes for its far-side and skybox twins
/// (`benilla_assets::materials`, `WowModelExt::specialize`).
pub(crate) fn sky_pipeline_state(descriptor: &mut RenderPipelineDescriptor) {
    if let Some(ds) = descriptor.depth_stencil.as_mut() {
        ds.bias.constant = 0;
    }
}

/// Stars — the first celestial draw (`0x6d4a3f`): everything else in the sky paints over them, so
/// theirs is the ladder's LOWEST rung (see the sign law above).
pub(crate) const STARS_BIAS: f32 = -1.0e6;
/// **The WMO skybox** ([`crate::skybox`]) — the building-owned painted sky, which replaces this
/// whole ladder while it draws, so its rung never competes with one above. It needs a rung at all
/// because a skybox is **an ordinary M2 and not an opaque backdrop**, which is what this module
/// asserted until decision 1264: `CavernsOfTimeSky.m2` is 21 batches across four blend modes — a
/// painted cube, six additive star sheets on the cube's own faces, five alpha-blended planet cards
/// and three alpha-tested asteroid belts on rotating bones. Drawn opaque (the old lane read
/// positions, UVs and one texture and nothing else) the additive star sheet — near-white RGB whose
/// stars live in its ALPHA — paints a flat white sheet over the painted sky. That is the director's
/// "the whole ceiling is white" in Caverns of Time.
///
/// The blended half of a skybox therefore lands in the transparent pass, camera-anchored, so its
/// view-z is a few tens of yards and it would sort *after* the world. This rung sinks it under
/// every world transparent: the deepest of those is [`FAR_SIDE_BIAS`] plus a far-plane view-z,
/// ≈ −4.3e4, and the shell's own view-z rides on top of the rung — all four properties of the band
/// are machine-checked in `model_render`'s
/// `the_skybox_band_orders_its_batches_on_one_pipeline_key`. Kept as small as that job allows (the
/// magnitude rationale above), because the rung is also what the batch-order epsilon has to
/// survive: see `model_render::SKYBOX_ORDER_EPS`.
pub(crate) const WMO_SKYBOX_BIAS: f32 = -6.0e4;
/// The sun disc — second (`0x7e5b90` via `0x6d4a47`).
pub(crate) const SUN_DISC_BIAS: f32 = -8.2e5;
/// The white moon — third; where the discs cross, the moon paints over the sun.
pub(crate) const WHITE_MOON_BIAS: f32 = -8.1e5;
/// moon02 — fourth (invisible in clear weather; ordered for its weather-seed surfacing).
pub(crate) const MOON02_BIAS: f32 = -8.0e5;
/// The cloud dome — last of the sky pass (`0x6d4a71`): clouds blend over a setting sun.
pub(crate) const CLOUDS_BIAS: f32 = -6.0e5;
/// **Far-side model transparents** — the water-plane interleave's early half: the reference splits
/// M2 transparents into an above-water and a below-water list per model (`0x707680`; per
/// **emitter** for particles — each classifies onto exactly ONE list, `0x7084a0`), and `0x483460`
/// draws the list on the eye's FAR side of the water plane
/// *before* the water pass, the near side *after*. A draw classified far-side takes this rung —
/// an effect plus its owner-last rung (capped at 32), a translucent M2 mesh batch plus its batch
/// eps (≤ ~65 × 1e-3, `model_render::BATCH_ORDER_SORT_EPS`), a zfill twin minus its 8 — so it
/// lands under [`WATER_BIAS`] with the owner-last law intact inside the band; near-side draws
/// keep their natural slot above it. The flip on submersion is the interleave inversion
/// (`0x4836d6 cmp eax,0xf`). Sort-only on every lane: the effect stream splits sort from raster
/// by construction, and the mesh lane's far twin zeroes the raster constant back in
/// `WowModelExt::specialize` (`clutter_fade.z` bit 11).
pub(crate) const FAR_SIDE_BIAS: f32 = -4.0e4;
/// **The water surface** — river/ocean/WMO water draw in a fixed frame slot between the two
/// transparent halves (`0x6701d0 → 0x6816d0`: ocean → river → WMO liquid → foam), never view-z
/// sorted against model transparents. One rung below the world band puts every unclassified
/// world transparent after the water (the reference's near-side default) and the far-side
/// effects before it. Magnitude: must clear the far plane (a few ×10³) so no world view-z can
/// reorder a water chunk across it, while staying small as a rasterizer bias (this one rides a
/// `StandardMaterial` base, so it IS also a depth-test bias — ~0.24% depth pull, half of what
/// the nameplates ship at +4e4; the water-noon/name-water capture diffs are the shoreline check).
pub(crate) const WATER_BIAS: f32 = -2.0e4;
/// **The water pass's own foam** — `CWater0Ripple`'s wade wake / standing ring, which the reference
/// draws INSIDE the water group and AFTER every liquid queue has drained (`0x6816d0`: ocean → river
/// → WMO liquid → foam). So foam is unconditionally over every water surface in the frame and still
/// under the near-side world transparents that follow the pass.
///
/// It rode [`WATER_BIAS`]` + 1.0` until B348: `+1` is a *coplanar* tie-break, and the thing it has to
/// beat is not one surface but the whole water band. A water chunk's key is `WATER_BIAS + its own
/// view-z`, so the chunk one lattice over — 33 yd nearer the eye — outranks a foam patch by ~33,
/// thirty times the tie-break, and paints over it: the director's "the wake goes dull, like another
/// layer without the effect is over it". A rung, not an epsilon, is the fix, and the window is
/// arithmetic: above every water key (`WATER_BIAS` + 0) even from the far plane, below every
/// unbiased near-side transparent (view-z ≥ [`WORLD_VIEW_Z_FLOOR`]) even at the eye — i.e.
/// `(WATER_BIAS − WORLD_VIEW_Z_FLOOR, WORLD_VIEW_Z_FLOOR)` = `(−1.7e4, −3e3)`, taken centred with
/// 7e3 of margin at each end. Both ends are asserted below; the blanket 1e4 does not fit a 1.4e4
/// window and is not claimed (the decal band's precedent).
pub(crate) const FOAM_BIAS: f32 = -1.0e4;
/// The sun/moon glare quads — the frame's last render (`0x483740` tail): over the clouds and the
/// rain, under the nameplates; the z-buffer (their pinned far depth, `sky_vertex.wgsl`) is what
/// occludes them. Only ~6× the far plane, not ~10⁵: a rung this far up is also a rasterizer bias
/// (the sign law above), and nothing is gained by making it bigger than the ordering needs.
pub(crate) const GLARE_BIAS: f32 = 2.0e4;

/// The floor of any world draw's view-z: the projection's ~3 km far plane, negated (the sign law
/// above — view-z is negative in front of the eye, more negative the farther away). Two rungs
/// order two draws *unconditionally* only when their gap exceeds this, so it is the unit the
/// assert block measures the ladder's margins in.
pub(crate) const WORLD_VIEW_Z_FLOOR: f32 = -3.0e3;

/// **The world-side rungs** — the biases lanes *outside* this module apply to their own draw, and
/// the reason they are here rather than each next to its lane: they are one ladder. 1163 found the
/// four of them scattered across `blob_shadow`, `ground_fx`, `nameplates` and `target::ring`, each
/// documenting itself by pointing at one of the others ("the same constant, for the same reason,
/// as the ring's"), which is a ladder held together by prose. The compile-time assert below can
/// only check an order it can see — and 1163 left two out of view: the footprint print and the
/// ground-target reticle each kept a private constant in its own lane, copied from a neighbour's
/// number by the same prose 1163 came to end. Both are rungs now (B347).
///
/// Named as one type so a lane reaches the ladder, not a constant: `Rung::GROUND_FX`.
pub struct Rung;

impl Rung {
    /// The **coplanarity** rung — sort bias AND rasterizer depth-bias constant, one number for
    /// both roles, and the home of the rationale the whole decal family shares. Projected decal
    /// vertices are geometrically coplanar with the drawn ground, so without a bias the depth
    /// test dissolves into per-pixel f32 noise (stipple, view-dependent bites); 8192 · 2⁻²³ pushes
    /// their depth ~1e-3·distance toward the camera — far above the coplanar noise floor (an ulp
    /// of Elwynn's 10⁴-yard coords), far below anything visible. Geometric lifts (0.1, 0.02) read
    /// as hovering at grazing angles — director-confirmed. The reference fights the same
    /// coplanarity with polygon offset (trace-verified), the fixed-function twin of this bias.
    ///
    /// It stays a *sort* rung only for **ground-fx spell decals**, which have no reference frame
    /// slot to take: the real client draws those quads as ordinary M2 batches with no ground
    /// conform anywhere in the spell-visual chain (`ground_fx`'s header) — conforming them is our
    /// own improvement, so "wherever that M2 transparent sorts" is the honest answer and the
    /// near-side default is after the water. Everything with a *known* slot moved to the
    /// pre-water band below (B347).
    pub const GROUND_FX: f32 = 8192.0;
    /// **The one rasterizer settle every ground decal shares** — every client of the shared
    /// surface-decal projector (`decal.rs`): the selection ring, the unit blob shadow, spell
    /// ground-fx quads, the footprints and the ground-target reticle. It funds the `GreaterEqual`
    /// tie against the drawn ground (`DECAL_WORLD_CLIP`, 0781).
    ///
    /// **It used to be two constants, and the split was wrong** (1817, superseding B131's
    /// `SHADOW_RASTER`): the ring and reticle rode +8192 on the claim that "the blob shadow is the
    /// one lane that needs more". The binary groups them the other way — the blob shadow reaches
    /// the SAME EGxRs id-`0` arming site as the ring, corpse marker, click marker and sky decal
    /// (`0x6d7920` → `0x6d8047 call 0x6d7330` → `0x6d6fa0` → `0x6d7480`), and it is the
    /// **footprints** that are a lane of their own (`0x69a54a` in `0x69a3e0`), at 10× (measured on
    /// the reference client). And our own measurement agrees with
    /// the grouping: the residual is 0781's bake error — the decal's CPU-baked world verts vs the
    /// receiver's GPU-transformed verts diverge by ~1–3 ulps of the *world coordinate*, millimetres
    /// at city magnitudes — which is a property of the bake, identical on every lane that uses it.
    /// `ground_fx` was a third value again, and not even a raster one: it passed `GROUND_FX as
    /// i32`, a **sort** rung borrowed for a rasterizer field because the numbers coincided — 1806's
    /// conflation, in the one place it had survived.
    /// So one number, and it is the measured one: **the residual is 2.59× the +4096 margin and
    /// therefore 1.30× the +8192 one**, i.e. the ring and the reticle were riding *under* their own
    /// conflict and had simply not been walked on a slope yet. Pinned by
    /// `raised_bias_dominates_the_bake_residual` (particles/render.rs), which now asserts both ends.
    ///
    /// **What is deliberately NOT ported: the reference's magnitudes, or its 1 : 10 : 20 ratio.**
    /// Both are properties of a 24-bit *fixed-point forward-Z* buffer at near 0.1 / far 777 /
    /// depth-range [0, 0.94], where a constant normalized-depth bias is worth `k · d²` yards —
    /// 0.0065 yd at 10 yd on the ring lane, 0.16 yd at 50. Ours is reversed-Z **float**, where the
    /// bias unit is the ULP at the primitive's own depth, so the same constant is worth `z · C ·
    /// 2⁻²³` — **linear**, not quadratic. Neither profile converts into the other, and the quantity
    /// that sets ours is a millimetre-scale bake residual that does not grow with distance at all.
    /// The reference's 10× footprint step has no counterpart here for the same reason: the residual
    /// is the bake's, and the bake is one path.
    pub const DECAL_RASTER: i32 = 32768;
    /// **The pre-water decal band, rung 1: the selection ring** (and, when they are built, the
    /// corpse marker and the click-to-move marker — the same `+0x30` slot and the same call site).
    ///
    /// This rung shipped at **+8192** for months and was moved on a guess's opposite: 1785 left it
    /// alone because the ring's frame slot was *unread*, and it turned out earlier than anything
    /// guessed. `[node+0xb4] = 0x481540` → `0x4815d0` →
    /// `0x48160c call [obj vt+0x38]` → `0x614ada call [obj vt+0x30]` → `0x608e00`: the ring is
    /// emitted **from inside PHASE 1's own M2 node drain** (`0x6812c5 call 0x683dd0`), the same
    /// loop body that then calls `0x6d78f0` for that node's blob shadow — so the family is not
    /// merely before the water, it is before the footprints and before the M2 opaque pass too.
    ///
    /// **And the ring draws UNDER the shadow, not over it.** Per node the `+0x38` tick runs first
    /// (`0x48160c`) and the shadow gate second (`0x683ec3`), so the additive ring goes down and the
    /// modulate shadow darkens it. This ladder asserted the opposite for as long as both rungs have
    /// existed — "where both decals stack the ring draws later deterministically" was a tie-break
    /// invented here, not a fact read anywhere, and it was backwards.
    pub const RING: f32 = -5.4e4;
    /// **The pre-water decal band, rung 2: the unit blob shadow.**
    ///
    /// The reference draws its ground decals *before* the water, and B347 is what it costs not
    /// to. The frame driver `0x483460` emits this one inside PHASE 1's opaque drain row
    /// (`0x6812c5 call 0x683dd0` → `0x6d78f0` → `0x6d7920`) — in the same node-drain loop body
    /// as the ring above and immediately after it, before the footprints, before the M2 opaque
    /// pass (`0x4836a6`), and long before the water surfaces, which draw in phase 3 (`0x6816d0`)
    /// *between* the two M2 transparent passes. Every transparent in the world paints over a
    /// shadow there.
    ///
    /// Here it did the exact opposite. At the old **+4096** the shadow's key (`view-z + bias`)
    /// beat every world transparent — [`WATER_BIAS`], [`FAR_SIDE_BIAS`], and the unbiased
    /// near-side default — making it the one thing in the scene the water did not attenuate: a
    /// crocolisk's blob read through a Stranglethorn river that hid the crocolisk itself, and the
    /// player's read through the Stormwind mage-tower portal (B347, both shots).
    ///
    /// **Where the band fits.** Its keys must clear the world-transparent band below it and the
    /// WMO-skybox band above it, each by more than a world view-z can travel (`WORLD_VIEW_Z_FLOOR`
    /// — two rungs are only separated if their gap exceeds it). The deepest world transparent
    /// sorts at `FAR_SIDE_BIAS − 8 − far ≈ −4.3e4`; the shallowest skybox batch at
    /// `WMO_SKYBOX_BIAS − 0.09 ≈ −6.0e4` — a window of `(−5.7e4, −4.3e4)`, which the four rungs
    /// take 2e3 apart, clearing the skybox by 3e3 below and the world band by ~5e3 above. The
    /// assert block below is the
    /// check, and it is arithmetic on the neighbours rather than a blanket 1e4: the window is only
    /// 1.4e4 wide, so the ladder's usual margin does not fit and pretending it does would be the
    /// lie. **The depth of the rung costs nothing at the rasterizer** — unlike [`WATER_BIAS`] and
    /// the two rungs above, the effect stream carries sort and raster as separate fields
    /// ([`DECAL_RASTER`](Rung::DECAL_RASTER) is this lane's other half).
    pub const SHADOW_SORT: f32 = -5.2e4;
    /// **The pre-water band, rung 3: footprints** — the reference draws them at
    /// `0x483654` (`0x670240` → `0x69a3e0`), one slot AFTER the shadow pass and still before the
    /// M2 opaque pass, so a print paints over a shadow rather than under it. That was inverted
    /// here: the print rode +2048 *below* the shadow's +4096.
    pub const FOOTPRINT: f32 = -5.0e4;
    /// **The pre-water band, rung 4: the ground-target reticle** — the reference's solid-receiver
    /// pass (`0x4836c5`, flags `0x200122`) is the last decal before the water. Its *liquid* pass
    /// (`0x483727`, flags `0xf0000`) is the family's one genuinely post-water draw, and it has no
    /// counterpart here: liquid is not a `GroundDecalSurface` yet, so nothing we project can land
    /// on a water surface at all. When it can, that half wants a rung above [`WATER_BIAS`] — not
    /// this one.
    pub const RETICLE: f32 = -4.8e4;
    /// **The wade-foam decal's rasterizer settle.** `0x68fd0f` arms EGxRs id `0` for the foam draw
    /// (`0x68fae0`), which is the client's third and largest ground-decal bias lane.
    ///
    /// **The shipped value is `D3DRS_DEPTHBIAS = −1.2207217514514923e−04`** =
    /// `−f32(footstepBias(0.125) × [0x810390])` — **−2048.03 ULPs** of the reference's 24-bit
    /// buffer, 20× the ring/shadow lane and 2× the footprints' (measured on the reference client,
    /// four captures at the graphics boundary).
    ///
    /// **1809's `−8.0001` and its `factor = −4.0` were both dead code.** `gxApi` defaults to
    /// `"direct3d"` (`0x63a81d`) and no shipped `WTF` overrides it, so `GxDevCreate` builds
    /// `CGxDeviceD3d` and the OpenGL arm — the only place in the image where the `−4.0` slope
    /// factor exists — never runs. The GL arm also hard-codes a `×65536` *16-bit* ULP conversion,
    /// which is why every number derived from it came out **256× too small**. Measured from a
    /// fully independent state: `factor = 0` on 2485 of 2485 client-originated `glPolygonOffset`
    /// calls across four captures, which WoW's GL arm cannot emit.
    ///
    /// **The signs invert here.** Reference depth runs 0 = near, so its negative offset pulls
    /// toward the eye; ours is reversed-Z (`CompareFunction::GreaterEqual`, near = 1), where toward
    /// the eye is *larger* depth — so a ported term is positive.
    ///
    /// **We spend almost none of it, on purpose.** Our constant is in ULPs of a **float** buffer
    /// (per-primitive — the quantisation 1806 is about), so its world pull is `z · C · 2⁻²³`:
    /// ~1 µm per yard of view distance at `C = 8`. The reference's is `1.29847e−3 · d²` — 0.13 yd
    /// at 10 yd, 0.52 at 20, 3.2 at 50 — and **measured, from framebuffer pixels on two
    /// Northshire wade frames, that it does paint the splash decal onto dry bank**, by ~0.3–1.0 yd
    /// at an 11.9-yd camera. That bleed is the thing the director reported twice (B348), so the
    /// fidelity is available and deliberately not taken; the reference's own bound on it is not
    /// the depth test but the texgen box plus the decal texture's alpha-0 border under CLAMP.
    /// What ours funds instead is a guard against a driver rounding one pipeline's coplanar
    /// arithmetic differently from another's — [`FOAM_RASTER_SLOPE`](Rung::FOAM_RASTER_SLOPE) has
    /// the reason nothing more is needed.
    pub const FOAM_RASTER: i32 = 8;
    /// **The slope half is deliberately NOT armed — the tie it settles does not exist here** (B348
    /// second round, 1811; supersedes 1809 §3's transcription).
    ///
    /// **The reference has no slope term at all** — 1811 argued this from our own geometry and the
    /// bytes then confirmed it outright: `D3DRS_SLOPESCALEDEPTHBIAS` is written nowhere in the
    /// image (`0xaf` never pushed, census of all 24 `call [reg+0xe4]` sites), and the `-4.0` that
    /// 1809 ported lives only in the OpenGL arm the shipped client never constructs (see
    /// [`FOAM_RASTER`](Rung::FOAM_RASTER)). A re-implementation that ports it is transcribing dead
    /// code, and a grazing view of a ground plane is precisely where the two arms diverge most.
    ///
    /// 1811's own reason stands underneath, and is the one that would hold even if the term were
    /// live: our foam patch is not a decal *over* the liquid surface — it **is** the
    /// liquid surface's own triangles: [`water_fx::build_patch`] emits the wet cells straight out
    /// of [`WaterChunkInfo`]'s grid, in the liquid mesh's own winding, through the same
    /// `clip_from_world` (`DECAL_WORLD_CLIP`, 0781) against a mesh whose `Transform` is
    /// `IDENTITY`. Same vertices, same matrix, same arithmetic — so the depths agree exactly and
    /// `GreaterEqual` passes the tie on its own. That holds for the fullbright kinds too, which
    /// are the only liquids that write depth at all (magma/slime are `AlphaMode::Opaque`; water is
    /// `Blend`, depth-write off, so it never competes at all).
    ///
    /// What a slope term *does* buy, measured: a near-horizontal decal at a grazing angle has a
    /// window-depth gradient of `n / (h · f)` per pixel (eye height `h` above the plane, focal
    /// length `f` in pixels), so `factor · m` is a world-space pull of `factor · z² / (h · f)` —
    /// **growing as the square of view distance, and the near plane cancels out**. Whole wet cells
    /// are the unit of liquid geometry, so a shoreline carries a skirt of liquid-lattice triangles
    /// over dry sand (up to 7 yd of it at the reported beach — `benilla-formats --example
    /// water_here`), held back by nothing but the terrain having drawn first. Any pull toward the
    /// eye spends that budget: at `4.0` the wake visibly washed onto the beach, which is what the
    /// director reported twice.
    pub const FOAM_RASTER_SLOPE: f32 = 0.0;
    /// **The underwater drift cloud** — the mote field, at the reference's own frame slot
    /// `0x483731`: after the water surface AND both M2 transparent passes, immediately before the
    /// glare dispatch `0x483740`. The last world content in the frame (decision 1814).
    ///
    /// The window is narrow and this rung is checked against its actual NEIGHBOURS rather than the
    /// blanket 1e4, exactly as the decal band is: it has to clear
    /// [`GROUND_FX`](Rung::GROUND_FX) below and [`GLARE_BIAS`] above, and those are only 1.18e4
    /// apart. Centred, that leaves ~5.8e3 each side — comfortably more than the
    /// [`WORLD_VIEW_Z_FLOOR`] spread that is the most two anchors here can differ by, since the
    /// cloud's anchor is the eye itself and a ground-fx decal's is at most a far plane away.
    ///
    /// One batch for the whole field, so there is no intra-cloud sort — which is right rather than
    /// merely cheap: the reference submits all its survivors as **one indexed triangle list**
    /// (`0x68f3c9`) in slot order, with depth WRITE off and no per-mote ordering anywhere.
    pub const DRIFT_CLOUD: f32 = 1.4e4;
    /// **World text** — above the celestial glare, so a flare never washes a nameplate, and above
    /// every sky rung. The reference draws its world text late in the frame (decision 0519).
    /// Small on purpose: 6× the far plane is all the ordering needs, and this same field doubles
    /// as the rasterizer depth bias on a layer that is depth-TESTED (walls must keep occluding
    /// names). The sign was inverted here once and the water erased the glyphs (decision 0639).
    pub const NAMEPLATE: f32 = 4.0e4;
}

/// The ladder IS the reference order — checked at compile time: monotonic through the sky pass,
/// then the **pre-water decal band** (the reference's own decal slots, B347), then the water-plane
/// interleave, then rain (unbiased view-z, ≈ 0), the glare above it, and the nameplates above the
/// glare so text stays readable through a flare. Rung gaps stay wider than any view-z that could
/// reorder them — over 10⁴ inside the sky (camera-anchored shells spread ±far·0.85 ≈ 2.6e3) and
/// over 10⁴ on the world side (more than the far plane, a few ×10³) — except inside the decal
/// band, which lives in a 1.4e4 window between two bands it must not touch and is therefore
/// checked against its actual neighbours rather than the blanket.
const _: () = {
    assert!(SUN_DISC_BIAS - STARS_BIAS > 1.0e4);
    assert!(WHITE_MOON_BIAS > SUN_DISC_BIAS && MOON02_BIAS > WHITE_MOON_BIAS);
    assert!(CLOUDS_BIAS - MOON02_BIAS > 1.0e4);
    // The water-plane interleave: sky < far-side transparents < water < world transparents (the
    // near-side default). The far-side band is FAR_SIDE_BIAS − 8 (a zfill twin) … + owner rung;
    // owner rungs are capped well under the 1e4 margin (benilla_formats::owner_last_rung's
    // ceiling) and the mesh batch eps under that.
    assert!(FAR_SIDE_BIAS - CLOUDS_BIAS > 1.0e4);
    assert!(WATER_BIAS - FAR_SIDE_BIAS > 1.0e4);
    assert!(-3.0e3 - WATER_BIAS > 1.0e4); // world view-z floor = −far (the ~3 km projection) stays above
                                          // Foam sits in the water pass, over every liquid surface and under the near-side default,
                                          // whatever the two draws' distances are (B348).
    assert!(FOAM_BIAS + WORLD_VIEW_Z_FLOOR - WATER_BIAS > 1.0e3);
    assert!(WORLD_VIEW_Z_FLOOR - FOAM_BIAS > 1.0e3);
    assert!(Rung::GROUND_FX - CLOUDS_BIAS > 1.0e4);
    assert!(GLARE_BIAS - Rung::GROUND_FX > 1.0e4);
    assert!(Rung::NAMEPLATE - GLARE_BIAS > 1.0e4);
    // The drift cloud sits between the ground-fx decals and the glare — the reference's `0x483731`,
    // the frame's last world draw. Its window is 1.18e4 wide, so like the decal band it is measured
    // against its neighbours rather than the blanket: the margin only has to beat the view-z spread
    // two anchors here can differ by, and the cloud's anchor IS the eye.
    assert!(Rung::DRIFT_CLOUD - Rung::GROUND_FX > -WORLD_VIEW_Z_FLOOR);
    assert!(GLARE_BIAS - Rung::DRIFT_CLOUD > -WORLD_VIEW_Z_FLOOR);
    // The raster margins are their own axis (B131) — not comparable to the sort rungs above, only
    // to zero and to each other. One number for every ground decal (1817); the foam is not one of
    // them, and takes far less.
    assert!(Rung::DECAL_RASTER > 0 && Rung::FOAM_RASTER < Rung::DECAL_RASTER);
    // The foam's settle is deliberately the SMALL one: unlike a ground decal its patch is the
    // liquid mesh's own triangles, so it has no bake residual to dominate (1811). It pulls toward
    // the eye under reversed-Z, so it is positive; a negative here would push it INTO its receiver.
    assert!(Rung::FOAM_RASTER > 0);
    // The slope half stays disarmed on this lane (1811): its pull grows as z² and it spends the
    // wet-lattice skirt onto the beach, while the tie it would settle is already won by the patch
    // being the liquid mesh's own triangles. A nonzero value here is a decision, not a tune.
    assert!(Rung::FOAM_RASTER_SLOPE == 0.0);

    // ─── The pre-water decal band (B347, 1785/1789) ─────────────────────────────────────────
    // Internally ordered as the reference's frame emits them — ring then shadow, both from
    // PHASE 1's node drain (`0x6812c5 call 0x683dd0`: the `+0x38` tick at `0x48160c`, then the
    // shadow gate at `0x683ec3`) → footprints (`0x483654`) → the reticle's solid pass
    // (`0x4836c5`) — with a step far wider than the view-z two *stacked* decals can differ by
    // (they share a patch of ground, so their anchors are yards apart, not kilometres).
    assert!(Rung::SHADOW_SORT - Rung::RING > 1.0e3);
    assert!(Rung::FOOTPRINT - Rung::SHADOW_SORT > 1.0e3);
    assert!(Rung::RETICLE - Rung::FOOTPRINT > 1.0e3);
    // And below EVERY world transparent whatever its distance — the deepest being a far-side
    // zfill twin (−8) out at the far plane. This is the assert B347 was missing: at the old
    // positive rungs the band sat above the water and above every M2 transparent, and the shadow
    // was the one thing in the scene the water did not attenuate.
    assert!(FAR_SIDE_BIAS - 8.0 + WORLD_VIEW_Z_FLOOR - Rung::RETICLE > 1.0e3);
    // ...and above the WMO-skybox band whatever the decal's distance, so a building's painted sky
    // can never sort over a decal (it is depth-forced to the far plane as well, but the ladder
    // does not get to lean on a second mechanism to state its own order). Measured at the band's
    // FLOOR, which is the ring's rung.
    assert!(Rung::RING + WORLD_VIEW_Z_FLOOR - WMO_SKYBOX_BIAS > 1.0e3);
};

/// The depth law (module doc) is a property of the **shaders**, so it is checked there: the shared
/// vertex stage must pin the far depth, every sky material must draw through it, and no sky
/// fragment shader may take the depth back (a `frag_depth` write is the same number at the cost of
/// the pipeline's early-Z — decision 2016). Without this, a shell radius silently becomes
/// load-bearing again the moment someone edits one of them — the exact regression 0588 fixed, and
/// one that only shows up at night, on a mountainous horizon, past 2.6 km.
#[test]
fn the_sky_depth_is_pinned_at_the_vertex_and_nowhere_else() {
    use bevy::pbr::Material;
    use bevy::shader::ShaderRef;

    let vertex = include_str!("shaders/sky_vertex.wgsl");
    assert!(
        vertex.contains("const SKY_FAR_CLIP_Z: f32 = 0.0;")
            && vertex.contains("out.position.z = SKY_FAR_CLIP_Z;"),
        "sky_vertex.wgsl no longer pins the far depth — every shell radius is deciding occlusion \
         again (sky_order.rs, \"The depth law\")"
    );
    for (name, src) in [
        ("sky.wgsl", include_str!("shaders/sky.wgsl")),
        ("star.wgsl", include_str!("shaders/star.wgsl")),
        ("cloud.wgsl", include_str!("shaders/cloud.wgsl")),
        ("celestial.wgsl", include_str!("shaders/celestial.wgsl")),
        // The WMO skybox is no longer a shader of its own: it draws on the shared model lane, whose
        // `WOW_SKY_DEPTH` branch obeys the same law and is asserted beside it
        // (`benilla_assets::materials`, `the_sky_lane_pins_the_far_depth_at_the_vertex`).
    ] {
        assert!(
            !src.contains("@builtin(frag_depth)"),
            "{name}: a sky fragment writes its depth again — the pin is the vertex stage's, and a \
             fragment write costs the pipeline its early-Z (sky_order.rs, \"The depth law\")"
        );
    }
    fn shared(name: &str, shader: ShaderRef) {
        match shader {
            ShaderRef::Path(p) => assert_eq!(
                p.to_string(),
                SKY_VERTEX_SHADER,
                "{name}: not drawing through the shared sky vertex stage"
            ),
            _ => panic!("{name}: vertex shader is not a path — not the shared sky vertex stage"),
        }
    }
    shared(
        "SkyMaterial",
        <crate::sky::SkyMaterial as Material>::vertex_shader(),
    );
    shared(
        "CloudMaterial",
        <crate::clouds::CloudMaterial as Material>::vertex_shader(),
    );
    shared(
        "StarMaterial",
        <crate::sun::StarMaterial as Material>::vertex_shader(),
    );
    shared(
        "CelestialMaterial",
        <crate::sun::CelestialMaterial as Material>::vertex_shader(),
    );
}
