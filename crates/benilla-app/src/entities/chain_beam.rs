//! Spell **chain beams** — Chain Lightning's arcs, Drain Life's rope of soul, Mind Flay's mana
//! beam, Chain Heal's arc, C'Thun's eye beam (slice 2: the renderer).
//!
//! A kit whose `CharProc` decodes to a chain ([`benilla_formats::ChainProc`]) draws a **polyline of
//! hops** — `caster → t1 → t2 → t3` — with one ribbon per hop, each subdivided, jittered per frame
//! and scrolled. The data half (the DBC row, the sentinel fix, the wire) is slice 1; this module is
//! the client's `LightningObject` (`0x6ec460`) + `CLightning` (`0x7af9b0`/`0x7afcb0`) pair.
//!
//! ## What the reference does, and what this transcribes
//!
//! **The hop list is unit state, not packet state.** The reference parks it on the caster (the
//! growable array at `unit+0xd44`) — [`ChainHops`] here — filled by **two** producers, `SMSG_SPELL_GO`'s
//! hit list (`0x6e800d`) and `SMSG_SPELL_UPDATE_CHAIN_TARGETS` (`0x605767`), both dropping the
//! caster's own guid, and **consumed exactly once** by the next chain proc (`0x60db72` zeroes the
//! count on every exit path, including the ones that draw nothing).
//!
//! **Target selection** (`0x60dad4`–`0x60db19`): when the playing kit's spell **is** the unit's
//! `UNIT_CHANNEL_SPELL`, its `UNIT_FIELD_CHANNEL_OBJECT` is set, and the hop count is `<= 1`, the
//! beam runs to that single channel object; otherwise it runs the hop array, and an **empty array
//! draws nothing at all**.
//!
//! **Per hop** (`Bolt[i] = (i, i+1)`, `0x6ec980`): a window `[t0 + i × stagger, + life)` — so a
//! 3-hop Chain Lightning arcs *outward* — and the whole beam expires at `t0 + hops × life`. The
//! **flag** ([`ChainProc::flag`], the decoded `CharParamTwo`) is the entire cast/channel split: with
//! it set, the per-hop window and the expiry are both bypassed and the beam lives until swept
//! (`LightningObject::Stop 0x6ece10`, whose one caller image-wide is the channel teardown).
//!
//! **Endpoints are identities, re-resolved from the live units every frame** (`0x6ec460` runs both
//! through `ClntObjMgr` each tick), so a beam tracks a moving caster and a moving target; a hop
//! whose unit is not resolvable is *hidden*, not re-pathed. The caster's end anchors at its `$CSL`
//! marker, else `base + 0.75 × modelHeight × modelScale` (`0x6ec6f0`); every other endpoint anchors
//! at the M2 attachment named by the spell's own `SpellVisual` field 9 — the same ordinal the
//! missile homes to — with 34 as the fallback and the unit's base below that (`0x6ec780`).
//!
//! **Geometry per hop** (`0x7af6d0`), all VERIFIED:
//! - `n = trunc(len / avgSegLen + 2.0)` sub-segments, `n+1` points, `point[i] = lerp(a, b, i/n)`;
//!   the `+2` is a floor, so even a one-yard hop bends.
//! - each interior point takes an independent 3-vector in `[−1, 1]³` scaled by `len × noiseScale`,
//!   **re-rolled every frame**, and the live polyline advects toward it `0.75` old / `0.25` new.
//! - the ribbon's cross-section is `(−d.y, d.x, 0)` taken **in EYE space**, not world space: the
//!   strip is built from points already run through the world→eye transform (`0x7bca80` against
//!   `0xcf5800 = T(−cameraPos)·VIEW`, assembled at `0x7affb1`–`0x7b00c4`; `VIEW` is the device's
//!   slot-10 matrix, whose producer `0x50ab70` is `LookAt(eye = origin, …)` — **pure rotation, zero
//!   translation row**, which is why the pre-translate is not a double translate). Eye axes are X
//!   right, Y up, **+Z into the screen**, so the formula is the *screen*-plane 90° rotation and the
//!   ribbon is **view-plane aligned**. In world axes it is `normalize(d × camForward)` — the same
//!   vector, and the same magnitude, so the reference's `0.001` guard transfers unchanged. The
//!   transform is an isometry with no projection (`0x7bca80` is row-vector affine, no w-divide) and
//!   both the WORLD and VIEW slots are identity at the submit, so the strip is drawn in eye space
//!   and the width stays **`2 × field2` world yards at any distance**. Decision 1653 corrects 0964,
//!   which read the formula as world-space and drew every beam as a horizontal slab — a hairline
//!   from any camera near the beam's own plane.
//! - an interior vertex sits at `p ± 0.5·(perp₍ᵢ₋₁₎ + perpᵢ)·halfWidth`; **both ends collapse to a
//!   point** with `v = 0.5` — the beam is a spindle, tapered at caster and target.
//! - `u` runs `0 → 1` caster→target, translated by `−(phase / period)` where
//!   `phase = fmod(phase + dt, period)`; a **negative** period reverses the scroll, which is what
//!   flows the four drain textures back toward the caster.
//!
//! **Render state** (`0x7afcb0`, decoded against the `EGxRs` applicator `0x59d350`): additive
//! `SRC_ALPHA/ONE`, emissive white — a beam is **never tinted** — two-sided, depth-write off, fog
//! **off**. It rides the shared effect-quad stream ([`benilla_world::particles::buffer::EffectQuads`]) like
//! every other dynamic effect; `benilla_world::ribbons` is the structural model (same strip-as-quads
//! conversion, same commit).
//!
//! **Named approximations.** (a) The strand count (`CharParamOne`, ≤ 3 — only Chain Burn ships > 1)
//! is modelled as N independently-jittered copies of one polyline: the reference builds them from
//! byte-identical arguments and they diverge *only* through their interleaved draws on the shared
//! PRNG, which is a shared-generator detail we do not reproduce, so ours diverge by construction
//! instead. (b) The `SMSG_SPELL_GO` producer leg is gated in the reference on `0x6e4870`'s return, a
//! predicate that is not settled; we fill unconditionally — the superset, harmless because
//! consumption still requires a chain proc, and because every producer clears before it fills.
//! (c) A channel re-enters the dispatcher every tick (`0x612b18`) where we hold one beam for the
//! channel's life; the observable — a steady beam that ends with the channel — is the same.
//! (d) A beam takes no owner-last draw rung and no water-plane interleave: it belongs to no model.

use benilla_formats::{ChainEffect, MISSILE_ATTACH_TABLE};
use bevy::prelude::*;

use crate::creature_anim::{ChainProcPlay, FxClass, SpellKitFx, SpellVisuals};
use crate::net::GuidIndex;
use benilla_world::particles::buffer::EffectVertex;
use benilla_world::view::WorldCamera;

use super::OverheadFallback;

/// The caster-end anchor's height factor when the model has no `$CSL` marker: the reference's
/// `base + (0, 0, modelHeight × modelScale × 0.75)` (`0x6ec73e`–`0x6ec771`, the `0.75f` at
/// `0x8012cc`). Our stand-in for `obj+0x90` is the model's authored bbox z-extent — the same
/// "model height" the overhead-anchor fallback reads ([`OverheadFallback`]).
const CASTER_HEIGHT_FACTOR: f32 = 0.75;

/// The attachment every non-caster endpoint falls back to when the spell's `SpellVisual` field 9
/// names no usable one — the reference's literal `0x22` (`0x6ec7b5`–`0x6ec7c7`). Below it, the
/// unit's own position (which is what [`attach_world_pos`] already degrades to).
const CHAIN_ATTACH_FALLBACK: u16 = 34;

/// The subdivision floor's constant (`fadd [0x801628] = 2.0f` @ `0x7af716`): `n = trunc(len/avg + 2)`.
const SUBDIVISION_FLOOR: f32 = 2.0;

/// The normalisation guard on the cross-section vector (`fcom [0x801360]` @ `0x7b01bd`, strictly
/// `>`): a perpendicular shorter than this is left **un-normalized**, collapsing that segment's
/// width. The length it measures is the segment's *screen*-plane projection, so what it guards is a
/// hop aimed at the camera — not one aimed at the sky, which is what 0964 believed.
///
/// It is a **numerical guard, not a visible behaviour**: because everything above it is normalized
/// to full width, the pinch cone is `arcsin(0.001 / |d|)` — about `0.023°` for the ~2.5 yd
/// sub-segments a 30 yd link subdivides into, and `noiseScale`'s per-frame re-roll walks the
/// segment out of it. A beam does **not** thin as you turn to look down its length; it holds its
/// width and then, at an angle no player can hold, degenerates instead of producing a NaN.
const PERP_EPSILON: f32 = 0.001;

/// The per-frame advection weight on the live polyline (`0x7afc10`/`0x7afc38`):
/// `main = 0.75·main + 0.25·fresh`, interior points only.
const ADVECT_KEEP: f32 = 0.75;

/// A backstop on one hop's sub-segment count — the reference has none (it trusts the table), but a
/// long hop against a small `avgSegLen` would otherwise size a vertex run off table data. The
/// shipped worst case is far below this: 30 yd at `avgSegLen` 2.78 is 12.
const MAX_SUBDIVISIONS: usize = 256;

/// The caster's chain-target **hop array** — the reference's growable `unit+0xd44`
/// (`{capacity, count, data, quantum}`; the guid list, in wire order, with the caster's own guid
/// dropped as it fills, `0x6057bf`/`0x6057c9`).
///
/// Two producers write it (`crate::spell::net`: the `SMSG_SPELL_GO` hit list and
/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS`), each **clearing before it fills**; [`spawn_chain_beams`]
/// consumes it **once** and removes it, exactly as `0x60db72` zeroes the count on every exit —
/// including the paths that draw nothing. Targets not streamed to us drop out as they resolve: an
/// endpoint we cannot place would be invisible anyway (the reference hides such a hop too).
#[derive(Component)]
pub(crate) struct ChainHops(pub(crate) Vec<Entity>);

/// One hop of a live beam — the reference's 16-byte `Bolt`, whose `idxA`/`idxB` are always
/// `(i, i+1)` into the node list.
struct Bolt {
    /// Absolute scene seconds — `t0 + i × boltStagger` (`0x6ec9d7`–`0x6ec9e8`). Ignored while the
    /// beam is persistent (`0x6ec520` bypasses the window on the flag).
    start: f32,
    /// `start + boltLife` (`0x6ec9eb`). Ignored while persistent.
    end: f32,
    /// The live jittered polyline, one per strand — the reference's per-`CLightning` point array,
    /// advected rather than rebuilt so the jitter reads as motion instead of noise. Empty until the
    /// hop's first drawn frame (and cleared whenever its subdivision count changes).
    strands: Vec<Vec<Vec3>>,
}

/// A live chain beam — the reference's 0x50-byte `LightningObject`.
#[derive(Component)]
pub(crate) struct ChainBeam {
    /// The spell that drew it — the reap's key (`node+0x44`).
    spell_id: u32,
    /// The unit it hangs off. Its loss ends the beam: nothing else would ever reap a persistent one.
    caster: Entity,
    /// `[caster, hop0, hop1, …]` — **identities**, re-resolved every frame (`node+0x10`, the guid
    /// array). The participant set never changes; a beam does not re-target.
    nodes: Vec<Entity>,
    bolts: Vec<Bolt>,
    effect: ChainEffect,
    texture: Handle<Image>,
    /// The M2 attachment every non-caster endpoint anchors at, resolved once from the spell's own
    /// `SpellVisual` field 9 (the reference resolves it per frame; the row cannot change).
    dest_tag: u16,
    /// The decoded `CharParamTwo`: a **channel** beam, which never expires by time (`node+0x48`).
    persistent: bool,
    /// `t0 + hops × boltLife` (`0x6ecd30`). Ignored while [`Self::persistent`].
    expiry: f32,
    /// The texture-scroll accumulator, seconds (`node`-side `+0x60`): `fmod(phase + dt, period)`.
    phase: f32,
    rng: u32,
}

/// `Textures\SpellChainEffects\Lightning.blp` → its `mpq://` load URL (the lowercase/forward-slash
/// form every other BLP load in the client uses).
fn beam_texture_url(raw: &str) -> String {
    format!("mpq://{}", raw.to_ascii_lowercase().replace('\\', "/"))
}

/// One xorshift draw in `[−1, 1)` — our stand-in for the reference's `((rand & 0x7fffff) |
/// 0x3f800000)` mantissa trick re-centred through the same `2.0`. Same distribution, different
/// generator: the beam's look rests on the *amplitude* (`len × noiseScale`) and the 0.75/0.25
/// advection, both exact, never on the reference's draw sequence.
fn jitter(rng: &mut u32) -> f32 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 17;
    *rng ^= *rng << 5;
    // The top 24 bits as [0,1), doubled and re-centred — the reference's own [1,2)→[−1,1) shape.
    (*rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
}

/// The **caster's** end (`0x6ec6f0`): the `$CSL` marker at the live pose, else
/// `base + 0.75 × modelHeight × modelScale`. `None` only when the caster is gone. Every *other*
/// endpoint goes through the missile lane's attachment resolver instead — `0x6ec780` and
/// `0x61ceb0` read the same table the same way.
fn caster_world_pos(
    caster: Entity,
    units: &super::missile::AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    heights: &Query<&OverheadFallback>,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(caster).ok()?;
    let marker = bones.zip(pose).and_then(|(b, p)| {
        let &(bone, offset) = b.markers.get(b"$CSL")?;
        p.posed_point(joints.get(p.joints_root).ok()?, bone, offset)
    });
    Some(marker.unwrap_or_else(|| {
        let (scale, _, translation) = base.to_scale_rotation_translation();
        let height = heights.get(caster).map_or(0.0, |h| h.0);
        translation + Vec3::Y * (scale.y * height * CASTER_HEIGHT_FACTOR)
    }))
}

/// The kit's chain proc → the beam's participants, the reference's selection exactly
/// (`0x60dad4`–`0x60db19`): the channel object when the kit's spell **is** the live channel, that
/// channel names an object, and the hop array holds at most one entry; else the hop array; else
/// nothing at all.
fn select_targets(
    spell_id: u32,
    hops: Option<&ChainHops>,
    store: &crate::net::ObjectStore,
    index: &GuidIndex,
) -> Vec<Entity> {
    let hops = hops.map(|h| h.0.as_slice()).unwrap_or_default();
    let channelling = spell_id != 0 && store.0.unit_channel_spell() == spell_id;
    if channelling && hops.len() <= 1 {
        if let Some(object) = store.0.unit_channel_object().filter(|g| *g != 0) {
            // `count = 1, ptr = descriptor+0x38` — the single-target path. A channel object we
            // haven't streamed leaves the beam with nothing to run to, like a hidden hop.
            return index.0.get(&object).copied().into_iter().collect();
        }
    }
    hops.to_vec()
}

/// Spawn/replace/reap the beam entities: the CharProc dispatcher's beam case, ECS-side.
///
/// Reaps run **before** plays so a GO's own spell-id-keyed reap (which precedes its cast-kit play in
/// the router) never eats the beam that play is about to draw. Only **persistent** beams are
/// reapable: the reference only ever publishes a flagged node to the owner slots that the channel
/// teardown sweeps (`0x6ecdaa` gates the AddRef on the flag), so a one-shot beam always runs its own
/// clock to the end.
pub(super) fn spawn_chain_beams(
    mut commands: Commands,
    time: Res<Time>,
    mut plays: MessageReader<ChainProcPlay>,
    mut kit_fx: MessageReader<SpellKitFx>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    asset_server: Res<AssetServer>,
    index: Res<GuidIndex>,
    stores: Query<(&crate::net::ObjectStore, Option<&ChainHops>)>,
    beams: Query<(Entity, &ChainBeam)>,
    // The removal below is deferred to the next sync point, so a SECOND play on the same caster
    // this frame (a cast kit and an impact kit, an auto-repeat's twin GO) would still see the
    // array through the query. The reference zeroes the count in-place, mid-dispatch — this
    // overlay is that, and it is the same shape as the router's own `pending` hold overlay.
    mut consumed: Local<bevy::ecs::entity::EntityHashSet>,
) {
    consumed.clear();
    for ev in kit_fx.read() {
        let SpellKitFx::Reap {
            entity,
            spell_id,
            class: FxClass::Hold,
        } = ev
        else {
            continue;
        };
        for (e, beam) in &beams {
            if beam.persistent && beam.caster == *entity && beam.spell_id == *spell_id {
                commands.entity(e).despawn();
            }
        }
    }

    let now = time.elapsed_secs();
    for play in plays.read() {
        // The caster may have died inside the same wire drain that produced this play (the router's
        // own law) — and a beam is entirely *about* that unit.
        let Ok((store, hops)) = stores.get(play.entity) else {
            continue;
        };
        // Consume once, on every path — `0x60db72` zeroes the count even when nothing is drawn.
        let hops = hops.filter(|_| !consumed.contains(&play.entity));
        consumed.insert(play.entity);
        commands.entity(play.entity).try_remove::<ChainHops>();
        // `0x6ecbd0`'s own guards: no spell, no strands, no targets ⇒ silent return.
        let targets = select_targets(play.spell_id, hops, store, &index);
        if play.spell_id == 0 || play.proc.beams == 0 || targets.is_empty() {
            continue;
        }
        let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
            continue;
        };
        let Some(effect) = visuals.0.chain_effect(play.proc.effect_id).cloned() else {
            continue; // an id naming no row is the client's own no-op (`0x6ecc2e`)
        };
        // `0x6ec780`: the spell's OWN visual row, field 9's ordinal through the missile table —
        // no ranged fallback (that belongs to the kit resolve, not to this one).
        let dest_tag = spells
            .catalog
            .get(play.spell_id)
            .and_then(|d| visuals.0.stages(d.visual))
            .and_then(|s| MISSILE_ATTACH_TABLE.get(s.missile_attach as usize).copied())
            .unwrap_or(CHAIN_ATTACH_FALLBACK);
        // A replacing persistent beam takes the previous one's place (a channel re-armed on the
        // same caster+spell), matching the effect-model lane's persistent-Begin law.
        if play.proc.flag {
            for (e, beam) in &beams {
                if beam.persistent && beam.caster == play.entity && beam.spell_id == play.spell_id {
                    commands.entity(e).despawn();
                }
            }
        }
        let stagger = f32::from(u16::try_from(effect.bolt_stagger_ms).unwrap_or(u16::MAX)) / 1000.0;
        let life = effect.bolt_life_ms as f32 / 1000.0;
        let bolts: Vec<Bolt> = (0..targets.len())
            .map(|i| {
                let start = now + i as f32 * stagger;
                Bolt {
                    start,
                    end: start + life,
                    strands: vec![Vec::new(); play.proc.beams as usize],
                }
            })
            .collect();
        let texture = asset_server.load::<Image>(beam_texture_url(&effect.texture));
        let nodes = std::iter::once(play.entity).chain(targets).collect();
        commands.spawn((
            Transform::IDENTITY,
            ChainBeam {
                spell_id: play.spell_id,
                caster: play.entity,
                nodes,
                expiry: now + bolts.len() as f32 * life,
                bolts,
                effect,
                texture,
                dest_tag,
                persistent: play.proc.flag,
                phase: 0.0,
                // Per-beam seed: one strand per beam would otherwise draw the same jitter as
                // its neighbour on the same frame (see the module's approximation (a)).
                rng: 0x9E37_79B9 ^ (play.spell_id.wrapping_mul(2654435761)),
            },
        ));
    }
}

/// Advance the texture-scroll accumulator and derive the frame's `u` translate — the reference's
/// two halves (`0x7af9c7` Update, `0x7b0544` Render): `phase = fmod(phase + dt, period)` and
/// `u = −(phase / period)`, with a zero period disabling the scroll outright.
///
/// The **sign** rides through untouched: C `fmod` takes its dividend's sign, so a negative period
/// still lands the accumulator in `[0, |period|)` and only `u`'s direction flips — which is exactly
/// why the four drain rows' `−0.5` flows their texture back toward the caster.
fn advance_scroll(phase: f32, dt: f32, period: f32) -> (f32, f32) {
    if period == 0.0 {
        return (0.0, 0.0);
    }
    let phase = (phase + dt) % period;
    (phase, -(phase / period))
}

/// One hop's fresh subdivision, jittered — the reference's `0x7af6d0` rebuild, run every frame.
/// Endpoints are exact; interior points take `len × noiseScale` of independent 3-vector jitter.
fn subdivide(a: Vec3, b: Vec3, effect: &ChainEffect, rng: &mut u32, out: &mut Vec<Vec3>) {
    let len = a.distance(b);
    let n = if effect.avg_seg_len > 0.0 {
        (len / effect.avg_seg_len + SUBDIVISION_FLOOR).trunc() as usize
    } else {
        SUBDIVISION_FLOOR as usize
    }
    .clamp(SUBDIVISION_FLOOR as usize, MAX_SUBDIVISIONS);
    let amp = len * effect.noise_scale;
    out.clear();
    out.reserve(n + 1);
    for i in 0..=n {
        let p = a.lerp(b, i as f32 / n as f32);
        out.push(if i == 0 || i == n {
            p
        } else {
            p + Vec3::new(jitter(rng), jitter(rng), jitter(rng)) * amp
        });
    }
}

/// One strand's polyline, written into the shared stream as a triangle strip expressed in quads
/// (the ribbon lane's conversion: strip triangles `(t₀,b₀,t₁),(b₀,b₁,t₁)` = quad `[b₀,b₁,t₁,t₀]`).
///
/// Cross-section, ends and texcoords are the reference's (`0x7b0196`–`0x7b0541`): the **view-plane
/// aligned** perpendicular, un-normalized below [`PERP_EPSILON`]; an interior vertex at
/// `p ± 0.5·(perp₍ᵢ₋₁₎ + perpᵢ)·halfWidth`; both ends collapsed to a point at `v = 0.5`; `u`
/// running 0→1 caster→target plus the scroll translate. The colour is the beam's constant
/// `0xFFFFFFFF` — emissive white, never tinted.
///
/// `cam_forward` is the world camera's unit forward axis (into the screen) — **one axis for the
/// whole strand**, not a per-point eye ray: the reference's only use of the camera position is the
/// single pre-translate that builds the world→eye matrix (`0x7affb1`), after which every point
/// shares one linear map. So the ribbon lies in the view plane and keeps its authored world width
/// from every angle.
fn push_strand(
    verts: &mut Vec<EffectVertex>,
    pts: &[Vec3],
    half_width: f32,
    u_scroll: f32,
    cam_forward: Vec3,
) {
    let count = pts.len();
    if count < 2 {
        return;
    }
    let seg_perp = |i: usize| -> Vec3 {
        // `(−d.y, d.x, 0)` on the EYE-space delta is `d × camForward` in world axes: same vector,
        // same magnitude — `|d|` projected onto the screen plane — so the reference's 0.001 guard
        // on the raw length transfers unchanged. The order is `d × F`, not `F × d`: the two differ
        // by a sign, which mirrors `v` across the beam's centreline, and this is the one that puts
        // the `v = 0` rail on the same side the reference does (eye axes X right / Y up / +Z into
        // the screen — a beam running rightwards across the screen carries `v = 0` on top).
        let raw = (pts[i + 1] - pts[i]).cross(cam_forward);
        if raw.length() > PERP_EPSILON {
            raw.normalize()
        } else {
            raw
        }
    };
    // (top, bottom, v) for point `i` — the collapsed ends share a vertex and stamp v = 0.5.
    let rail = |i: usize| -> (Vec3, Vec3, f32) {
        if i == 0 || i == count - 1 {
            (pts[i], pts[i], 0.5)
        } else {
            let off = (seg_perp(i - 1) + seg_perp(i)) * 0.5 * half_width;
            (pts[i] + off, pts[i] - off, 0.0)
        }
    };
    let white = [1.0, 1.0, 1.0, 1.0];
    let span = (count - 1) as f32;
    let mut a = rail(0);
    for i in 0..count - 1 {
        let b = rail(i + 1);
        let (ua, ub) = (i as f32 / span + u_scroll, (i + 1) as f32 / span + u_scroll);
        // v: 0 on the top rail, 1 on the bottom — except at a collapsed end, where both are 0.5.
        let (va_t, va_b) = (a.2.min(0.5), if a.2 == 0.5 { 0.5 } else { 1.0 });
        let (vb_t, vb_b) = (b.2.min(0.5), if b.2 == 0.5 { 0.5 } else { 1.0 });
        for (pos, uv) in [
            (a.1, [ua, va_b]),
            (b.1, [ub, vb_b]),
            (b.0, [ub, vb_t]),
            (a.0, [ua, va_t]),
        ] {
            verts.push(EffectVertex {
                pos: pos.to_array(),
                uv,
                color: white,
            });
        }
        a = b;
    }
}

/// Per frame: age the beam, re-resolve every hop's live endpoints, re-jitter and advect its
/// polyline, and write the ribbons into the shared effect-quad stream — the reference's
/// `LightningObject::Update` (`0x6ec460`) and `CLightning::Render` (`0x7afcb0`) in one pass.
pub(crate) fn simulate_chain_beams(
    time: Res<Time>,
    mut commands: Commands,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
    images: Res<Assets<Image>>,
    world_cam: Query<(Entity, &GlobalTransform), With<WorldCamera>>,
    units: super::missile::AttachPosQuery,
    joints: Query<&GlobalTransform>,
    heights: Query<&OverheadFallback>,
    mut beams: Query<(Entity, &mut ChainBeam)>,
    mut scratch: Local<Vec<Vec3>>,
) {
    let Ok((cam, cam_xf)) = world_cam.single() else {
        return;
    };
    // The reference's world→eye map is rebuilt from the live camera every `CLightning::Render`
    // (`0x7aff82`–`0x7b00c4`), so the cross-section follows the camera frame by frame; one axis
    // serves the whole beam.
    let cam_forward = *cam_xf.forward();
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    for (entity, mut beam) in &mut beams {
        // Dead ⇔ `flag == 0 && now >= expiry` (`0x6ec6b9`); and a beam whose caster is gone has
        // nothing left to hang off — the one reap a persistent beam would otherwise never get.
        if (!beam.persistent && now >= beam.expiry) || !units.contains(beam.caster) {
            commands.entity(entity).despawn();
            continue;
        }
        let (phase, u_scroll) = advance_scroll(beam.phase, dt, beam.effect.scroll_period_s);
        beam.phase = phase;
        if !images.contains(&beam.texture) {
            continue; // not resident yet — nothing to draw, nothing to advect against
        }
        let (dest_tag, half_width, persistent, texture) = (
            beam.dest_tag,
            beam.effect.half_width,
            beam.persistent,
            beam.texture.id(),
        );
        let ChainBeam {
            nodes,
            bolts,
            effect,
            rng,
            ..
        } = &mut *beam;

        let mut batch = draw.batch(cam, texture);
        let (mut anchor_sum, mut anchor_n) = (Vec3::ZERO, 0.0f32);
        for (i, bolt) in bolts.iter_mut().enumerate() {
            // The per-hop window — bypassed entirely while the flag is set (`0x6ec520`).
            if !persistent && !(bolt.start <= now && now < bolt.end) {
                bolt.strands.iter_mut().for_each(Vec::clear);
                continue;
            }
            // Both endpoints re-resolved from the live units, every frame. Either missing hides
            // this hop (`SetVisible(handle, 0)`) without touching the rest of the chain.
            let dest = [dest_tag, CHAIN_ATTACH_FALLBACK];
            let from = if i == 0 {
                caster_world_pos(nodes[0], &units, &joints, &heights)
            } else {
                super::missile::attach_world_pos(nodes[i], dest, &units, &joints)
            };
            let (Some(from), Some(to)) = (
                from,
                super::missile::attach_world_pos(nodes[i + 1], dest, &units, &joints),
            ) else {
                bolt.strands.iter_mut().for_each(Vec::clear);
                continue;
            };
            anchor_sum += (from + to) * 0.5;
            anchor_n += 1.0;
            for pts in bolt.strands.iter_mut() {
                subdivide(from, to, effect, rng, &mut scratch);
                if pts.len() == scratch.len() {
                    // Advect the live polyline toward the fresh roll — interior only, endpoints
                    // exact (`0x7afbe7`–`0x7afc81`). This is what turns a per-frame re-roll into
                    // a crawl rather than a strobe.
                    let last = pts.len() - 1;
                    pts[0] = scratch[0];
                    pts[last] = scratch[last];
                    for k in 1..last {
                        pts[k] = pts[k] * ADVECT_KEEP + scratch[k] * (1.0 - ADVECT_KEEP);
                    }
                } else {
                    // First frame, or the subdivision count changed under a moving endpoint:
                    // take the fresh polyline whole (the reference's dirty-bit rebuild).
                    pts.clear();
                    pts.extend_from_slice(&scratch);
                }
                push_strand(batch.verts_mut(), pts, half_width, u_scroll, cam_forward);
            }
        }
        if anchor_n == 0.0 {
            continue; // every hop hidden — `commit_quads` over an empty range is a no-op anyway
        }
        // `EGxRs 0x07 = 3` → `glBlendFunc(SRC_ALPHA, ONE)`, and `0x0f = 0` → GL_FOG off (the
        // lane's default). Depth-write off and two-sided come with the additive pipeline. No
        // owner rung: a beam is not a model's emitter (module docs, approximation (d)).
        batch
            .additive()
            .anchored(anchor_sum / anchor_n)
            .owner(entity)
            .quads();
    }
}

/// The beam's arithmetic, against the numbers read out of the binary — the subdivision floor,
/// the spindle taper, the view-plane cross-section and its degenerate regime, and the signed
/// scroll. Each of these is a value that would go straight into pixels, and two of the four columns
/// they read are mis-named in the community schemas.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use benilla_formats::{
        char_proc_type, ChainProc, SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualStages,
    };

    use super::*;
    use crate::net::ObjectStore;

    /// Chain Lightning's real chain (5875 `spellvis 421`): visual 36, `SpellChainEffects` id 1.
    const CHAIN_SPELL: u32 = 421;
    const CHAIN_VISUAL: u32 = 36;

    /// `SpellChainEffects` id 1 — Chain Lightning's row, verbatim from the shipped table
    /// (`benilla-extract <Data> chaincensus`).
    fn lightning() -> ChainEffect {
        ChainEffect {
            avg_seg_len: 2.78,
            half_width: 0.5,
            noise_scale: 0.04,
            scroll_period_s: 1.0,
            bolt_life_ms: 1000,
            bolt_stagger_ms: 300,
            texture: "Textures\\SpellChainEffects\\Lightning.blp".into(),
        }
    }

    /// A minimal app running only [`spawn_chain_beams`] over Chain Lightning's synthetic chain.
    fn beam_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<GuidIndex>();
        app.add_message::<ChainProcPlay>()
            .add_message::<SpellKitFx>();
        app.insert_resource(SpellVisuals(
            SpellVisualCatalog::from_tables(
                HashMap::from([(
                    CHAIN_VISUAL,
                    VisualStages {
                        // Field 9's ordinal — index 1 of the missile table is attachment 34,
                        // which is also the chain's own fallback.
                        missile_attach: 1,
                        ..Default::default()
                    },
                )]),
                HashMap::new(),
            )
            .with_chain_effect(1, lightning()),
        ));
        app.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([(
                CHAIN_SPELL,
                SpellDisplay {
                    visual: CHAIN_VISUAL,
                    ..Default::default()
                },
            )])),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        app.add_systems(Update, spawn_chain_beams);
        app
    }

    /// A streamed unit: an empty descriptor store is all the spawner reads off a target.
    fn unit(app: &mut App) -> Entity {
        app.world_mut()
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])))
            .id()
    }

    /// A cast-stage Chain Lightning play: chain id 1, one strand, flag clear. The params go through
    /// the client's small-int decode, which round-trips an integer written as a float.
    fn play(entity: Entity, spell_id: u32) -> ChainProcPlay {
        ChainProcPlay {
            entity,
            spell_id,
            proc: ChainProc {
                effect_id: 1,
                beams: 1,
                flag: false,
                ty: char_proc_type::CHAIN_CAST,
            },
        }
    }

    /// Every live beam as `(nodes, [(hop start, hop end)], expiry, persistent)`.
    #[allow(clippy::type_complexity)]
    fn beams(app: &mut App) -> Vec<(Vec<Entity>, Vec<(f32, f32)>, f32, bool)> {
        app.world_mut()
            .query::<&ChainBeam>()
            .iter(app.world())
            .map(|b| {
                (
                    b.nodes.clone(),
                    b.bolts.iter().map(|x| (x.start, x.end)).collect(),
                    b.expiry,
                    b.persistent,
                )
            })
            .collect()
    }

    /// `n = trunc(len / avgSegLen + 2.0)`, `n+1` points — a **floor of two**, so even a hop shorter
    /// than one segment still bends (`0x7af713`, the `2.0f` at `0x801628`). The 30-yard case:
    /// 30 / 2.78 + 2 → 12 segments, 13 points.
    #[test]
    fn subdivision_carries_the_plus_two_floor() {
        let (e, mut rng, mut out) = (lightning(), 1u32, Vec::new());
        for (len, want_points) in [(0.0, 3), (1.0, 3), (30.0, 13), (2.78, 4)] {
            subdivide(Vec3::ZERO, Vec3::X * len, &e, &mut rng, &mut out);
            assert_eq!(out.len(), want_points, "{len} yd");
        }
        // A row whose segment length is 0 (id 15, which no kit reaches) must not divide by zero.
        let degenerate = ChainEffect {
            avg_seg_len: 0.0,
            ..lightning()
        };
        subdivide(Vec3::ZERO, Vec3::X * 30.0, &degenerate, &mut rng, &mut out);
        assert_eq!(out.len(), 3);
    }

    /// Endpoints are **exact** — the jitter is interior-only — and the interior offset is bounded by
    /// `len × noiseScale` per axis (`amp = len × field3`, `0x7af748`).
    #[test]
    fn jitter_is_interior_only_and_scales_with_hop_length() {
        let (e, mut rng, mut out) = (lightning(), 0x1234_5678u32, Vec::new());
        let (a, b) = (Vec3::new(1.0, 2.0, 3.0), Vec3::new(31.0, 2.0, 3.0));
        subdivide(a, b, &e, &mut rng, &mut out);
        assert_eq!(
            (out[0], *out.last().unwrap()),
            (a, b),
            "endpoints are exact"
        );
        let amp = 30.0 * e.noise_scale;
        let mut moved = 0;
        for (i, p) in out.iter().enumerate().take(out.len() - 1).skip(1) {
            let straight = a.lerp(b, i as f32 / (out.len() - 1) as f32);
            let d = *p - straight;
            assert!(
                d.x.abs() <= amp && d.y.abs() <= amp && d.z.abs() <= amp,
                "point {i} strayed {d} beyond ±{amp}"
            );
            moved += usize::from(d.length() > 1e-6);
        }
        assert_eq!(moved, out.len() - 2, "every interior point takes a draw");
    }

    /// The ribbon spans `2 × halfWidth` across its cross-section, that cross-section is square to
    /// the **camera** (perpendicular to the view axis, not to world up), and **both ends collapse
    /// to a point** — the beam is a spindle, not a slab (`0x7b0196`–`0x7b0541`).
    #[test]
    fn the_ribbon_is_a_spindle_two_half_widths_across() {
        let pts = [
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, -10.0),
        ];
        // A camera off to the side, looking along −X at a hop that runs along −Z.
        let cam_forward = -Vec3::X;
        let mut verts = Vec::new();
        push_strand(&mut verts, &pts, 0.5, 0.0, cam_forward);
        // 2 segments × one quad each.
        assert_eq!(verts.len(), 8);
        // Quad corner order is [b₀, b₁, t₁, t₀]: the first quad's b₀/t₀ are the collapsed caster
        // end, and the second's b₁/t₁ the collapsed target end.
        assert_eq!(verts[0].pos, pts[0].to_array());
        assert_eq!(verts[3].pos, pts[0].to_array());
        assert_eq!(verts[5].pos, pts[2].to_array());
        assert_eq!(verts[6].pos, pts[2].to_array());
        // The middle point carries the full width…
        let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
        assert!(
            (mid_top.distance(mid_bottom) - 1.0).abs() < 1e-5,
            "2 × 0.5 yd"
        );
        // …spread square to the view axis, which for this camera means straight up/down.
        let across = (mid_top - mid_bottom).normalize();
        assert!(across.dot(cam_forward).abs() < 1e-6, "square to the camera");
        assert!(across.x.abs() < 1e-6 && across.z.abs() < 1e-6, "±Y here");
        // …and the collapsed ends stamp v = 0.5 on both rails.
        assert_eq!(verts[0].uv[1], 0.5);
        assert_eq!(verts[3].uv[1], 0.5);
    }

    /// **The defect decision 1653 fixes.** A long, level hop watched from a camera in its own
    /// plane — the Razorgore-room beam, and Chain Lightning's mob-to-mob jumps — kept its full
    /// authored width in the reference and collapsed to a hairline for us, because 0964 spread the
    /// ribbon across *world* horizontal instead of the *screen* plane. Any camera axis must give
    /// the same `2 × halfWidth` span, and it must never lie along the view direction.
    #[test]
    fn a_level_hop_keeps_its_width_from_a_level_camera() {
        let pts = [Vec3::ZERO, Vec3::X * 15.0, Vec3::X * 30.0];
        for cam_forward in [
            Vec3::NEG_Z,                  // level, square on
            Vec3::new(0.0, -0.2, -1.0),   // the player's slightly-above-eye camera
            Vec3::new(-0.3, -0.05, -1.0), // level and off to one side
            Vec3::NEG_Y,                  // straight down
        ] {
            let cam_forward = cam_forward.normalize();
            let mut verts = Vec::new();
            push_strand(&mut verts, &pts, 0.5, 0.0, cam_forward);
            let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
            let across = mid_top - mid_bottom;
            assert!(
                (across.length() - 1.0).abs() < 1e-5,
                "{cam_forward}: spans 2 × 0.5 yd, not {}",
                across.length()
            );
            assert!(
                across.normalize().dot(cam_forward).abs() < 1e-5,
                "{cam_forward}: the width never runs into the screen"
            );
        }
    }

    /// The `v = 0` rail sits where the reference puts it. Eye axes are X right, Y up, `+Z` into
    /// the screen, and the perp is `(−d.y, d.x, 0)`, so a beam running **rightwards** across the
    /// screen carries `v = 0` along its **top** edge. In world axes that is `d × camForward`; the
    /// opposite order is the same line mirrored, which would flip the texture across the beam's
    /// centreline.
    #[test]
    fn the_v_zero_rail_is_the_top_edge_of_a_rightward_beam() {
        // Camera at the origin looking along −Z with +Y up: a hop along +X runs left→right.
        let pts = [Vec3::ZERO, Vec3::X * 5.0, Vec3::X * 10.0];
        let mut verts = Vec::new();
        push_strand(&mut verts, &pts, 0.5, 0.0, Vec3::NEG_Z);
        // Quad corner order is [b₀, b₁, t₁, t₀]; index 2 is the mid point's `t` rail.
        let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
        assert_eq!(verts[2].uv[1], 0.0, "the t rail is v = 0");
        assert!(
            mid_top.y > mid_bottom.y,
            "…and it is the upper edge on screen"
        );
    }

    /// A hop pointing **at the camera** has no screen-plane extent, and the reference skips the
    /// normalisation below `0.001` (`0x7b01bd`), so the segment degenerates instead of producing a
    /// NaN. This is a **numerical guard, not a behaviour you can see**: the cone is
    /// `arcsin(0.001/|d|)` ≈ `0.023°` on a real sub-segment, and every angle outside it is
    /// normalized to full width. A hop pointing straight *up* is not in the regime at all — that
    /// was 0964's world-space misreading, and it drew a vertical beam as nothing.
    #[test]
    fn a_hop_aimed_at_the_camera_collapses_instead_of_exploding() {
        let pts = [Vec3::ZERO, Vec3::Y * 5.0, Vec3::Y * 10.0];
        let mut verts = Vec::new();
        push_strand(&mut verts, &pts, 0.5, 0.0, Vec3::NEG_Y);
        let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
        assert!(
            mid_top.distance(mid_bottom) < 1e-6,
            "a hop seen end-on has no width, and certainly no NaN"
        );
        assert!(verts.iter().all(|v| v.pos.iter().all(|c| c.is_finite())));

        // …and the same hop, from a camera beside it, is at full width.
        verts.clear();
        push_strand(&mut verts, &pts, 0.5, 0.0, Vec3::NEG_Z);
        let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
        assert!(
            (mid_top.distance(mid_bottom) - 1.0).abs() < 1e-5,
            "2 × 0.5 yd"
        );
    }

    /// `u` runs 0 → 1 caster→target (`0x7afb20`), translated by the scroll (`0x7b057e`).
    #[test]
    fn u_runs_caster_to_target_and_the_scroll_translates_it() {
        let pts = [Vec3::ZERO, Vec3::X * 5.0, Vec3::X * 10.0];
        let mut verts = Vec::new();
        push_strand(&mut verts, &pts, 0.5, 0.0, Vec3::NEG_Z);
        assert_eq!(verts[0].uv[0], 0.0, "caster end");
        assert_eq!(verts[5].uv[0], 1.0, "target end");
        verts.clear();
        push_strand(&mut verts, &pts, 0.5, -0.25, Vec3::NEG_Z);
        assert_eq!(
            verts[0].uv[0], -0.25,
            "the whole run slides with the scroll"
        );
        assert_eq!(verts[5].uv[0], 0.75);
    }

    /// The scroll accumulator wraps on `|period|` and a **negative** period reverses `u`'s
    /// direction with its magnitude unchanged — the four drains' `−0.5` is why their texture flows
    /// back toward the caster (`0x7af9d7` / `0x7b055f`).
    #[test]
    fn the_scroll_period_sign_reverses_the_direction() {
        // One second of a 1 s period sweeps u from 0 to −1 and wraps.
        let (mut phase, mut u) = (0.0, 0.0);
        for _ in 0..4 {
            (phase, u) = advance_scroll(phase, 0.25, 1.0);
        }
        assert!(phase.abs() < 1e-6, "wrapped back to 0 after a full period");
        assert!(u.abs() < 1e-6);
        let (_, quarter) = advance_scroll(0.0, 0.25, 1.0);
        assert!(
            (quarter + 0.25).abs() < 1e-6,
            "positive period ⇒ u goes negative"
        );
        // The drains: a −0.5 s period keeps the accumulator in [0, 0.5) and flips u's sign, so it
        // sweeps 0 → +1 at 2 tiles/s where a +0.5 would sweep 0 → −1.
        let (phase, u) = advance_scroll(0.0, 0.25, -0.5);
        assert!((phase - 0.25).abs() < 1e-6, "accumulator stays positive");
        assert!((u - 0.5).abs() < 1e-6, "…and u runs the other way");
        // A zero period disables the scroll outright rather than dividing by it.
        assert_eq!(advance_scroll(0.4, 0.25, 0.0), (0.0, 0.0));
    }

    /// The jitter draw stays in `[−1, 1)` — the amplitude bound the test above rests on.
    #[test]
    fn jitter_draws_stay_in_range() {
        let mut rng = 0x9E37_79B9u32;
        for _ in 0..10_000 {
            let v = jitter(&mut rng);
            assert!((-1.0..1.0).contains(&v), "{v} out of [−1, 1)");
        }
    }

    /// A hop array holding at most one entry, on a unit channelling THIS spell at an object,
    /// selects that **channel object** — the reference's single-target path (`0x60dae5`). Anything
    /// else runs the array, and an empty array draws nothing at all.
    #[test]
    fn target_selection_prefers_the_channel_object_only_on_its_own_narrow_path() {
        use benilla_protocol::ObjectFields;
        const CHANNEL_SPELL: u32 = 689; // Drain Life
        const OBJECT_GUID: u64 = 0xABC;
        let victim = Entity::from_raw_u32(7).expect("a test entity id");
        let hop = Entity::from_raw_u32(8).expect("a test entity id");
        let mut index = GuidIndex::default();
        index.0.insert(OBJECT_GUID, victim);
        // Field 20 = UNIT_FIELD_CHANNEL_OBJECT (a guid pair), 144 = UNIT_CHANNEL_SPELL.
        let channelling = ObjectStore(ObjectFields::from_pairs(&[
            (20, OBJECT_GUID as u32),
            (21, (OBJECT_GUID >> 32) as u32),
            (144, CHANNEL_SPELL),
        ]));
        let idle = ObjectStore(ObjectFields::from_pairs(&[]));

        assert_eq!(
            select_targets(CHANNEL_SPELL, None, &channelling, &index),
            vec![victim],
            "no hops + a live channel object ⇒ the channel object"
        );
        assert_eq!(
            select_targets(
                CHANNEL_SPELL,
                Some(&ChainHops(vec![hop])),
                &channelling,
                &index
            ),
            vec![victim],
            "…and `<= 1` is unsigned `jbe`: ONE hop still takes the channel path"
        );
        assert_eq!(
            select_targets(
                CHANNEL_SPELL,
                Some(&ChainHops(vec![hop, victim])),
                &channelling,
                &index
            ),
            vec![hop, victim],
            "two hops outgrow the single-target path and the array wins"
        );
        assert_eq!(
            select_targets(421, Some(&ChainHops(vec![hop])), &channelling, &index),
            vec![hop],
            "a DIFFERENT spell's kit never takes the channel object"
        );
        assert!(
            select_targets(CHANNEL_SPELL, None, &idle, &index).is_empty(),
            "no channel, no hops ⇒ nothing is drawn"
        );
    }

    /// The end-to-end spawn: a play + a filled hop array becomes a beam whose nodes are
    /// `caster → t1 → t2`, whose hops carry the reference's staggered windows, and whose expiry is
    /// `hops × boltLife`. And the array is **consumed** — a second play the same frame draws nothing.
    #[test]
    fn a_play_over_a_filled_hop_array_builds_the_staggered_polyline_once() {
        let mut app = beam_app();
        let (caster, t1, t2) = (unit(&mut app), unit(&mut app), unit(&mut app));
        app.world_mut()
            .entity_mut(caster)
            .insert(ChainHops(vec![t1, t2]));
        app.world_mut().write_message(play(caster, CHAIN_SPELL));
        app.update();

        let live = beams(&mut app);
        assert_eq!(live.len(), 1, "one beam, not one per hop");
        let (nodes, bolts, expiry, persistent) = &live[0];
        assert_eq!(
            nodes,
            &vec![caster, t1, t2],
            "caster → t1 → t2, in wire order"
        );
        assert_eq!(bolts.len(), 2, "two hops for two targets");
        // id 1's row: 1000 ms of life per hop, 300 ms of stagger between them.
        assert!(
            (bolts[1].0 - bolts[0].0 - 0.3).abs() < 1e-5,
            "the hop stagger"
        );
        assert!(
            (bolts[0].1 - bolts[0].0 - 1.0).abs() < 1e-5,
            "one second per hop"
        );
        assert!((expiry - bolts[0].0 - 2.0).abs() < 1e-5, "hops × boltLife");
        assert!(!persistent, "a cast-stage proc (flag 0) self-terminates");

        // The array is gone — the same law as `0x60db72`, which zeroes the count on every path.
        assert!(app.world().entity(caster).get::<ChainHops>().is_none());
        app.world_mut().write_message(play(caster, CHAIN_SPELL));
        app.update();
        assert_eq!(
            beams(&mut app).len(),
            1,
            "the second play has nothing to run"
        );
    }

    /// The consume is immediate, not deferred: two plays in ONE frame must not both see the array.
    #[test]
    fn two_plays_in_one_frame_share_no_hops() {
        let mut app = beam_app();
        let (caster, t1) = (unit(&mut app), unit(&mut app));
        app.world_mut()
            .entity_mut(caster)
            .insert(ChainHops(vec![t1]));
        app.world_mut().write_message(play(caster, CHAIN_SPELL));
        app.world_mut().write_message(play(caster, CHAIN_SPELL));
        app.update();
        assert_eq!(beams(&mut app).len(), 1, "only the first play consumes");
    }

    /// The flag is the whole cast/channel split: a **persistent** beam never expires by time and is
    /// ended only by the kit reap (the channel teardown's `LightningObject::Stop`), while a one-shot
    /// ignores that reap entirely and runs its own clock — the reference only ever publishes a
    /// FLAGGED node to the slots the teardown sweeps.
    #[test]
    fn only_a_persistent_beam_answers_the_kit_reap() {
        for persistent in [false, true] {
            let mut app = beam_app();
            let (caster, t1) = (unit(&mut app), unit(&mut app));
            app.world_mut()
                .entity_mut(caster)
                .insert(ChainHops(vec![t1]));
            let mut p = play(caster, CHAIN_SPELL);
            p.proc.flag = persistent;
            app.world_mut().write_message(p);
            app.update();
            assert_eq!(beams(&mut app).len(), 1);
            assert_eq!(beams(&mut app)[0].3, persistent);

            app.world_mut().write_message(SpellKitFx::Reap {
                entity: caster,
                spell_id: CHAIN_SPELL,
                class: FxClass::Hold,
            });
            app.update();
            assert_eq!(
                beams(&mut app).len(),
                usize::from(!persistent),
                "persistent={persistent}: the reap takes the channel beam and spares the cast one"
            );
        }
    }

    /// The client's own guards, each a silent return in `0x6ecbd0`: a bare kit push (spell id 0),
    /// a zero strand count, and a chain id naming no table row all build nothing — while still
    /// consuming the array on the way out.
    #[test]
    fn the_constructors_guards_build_nothing_but_still_consume() {
        for (spell_id, beams_param, effect_id) in [
            (0u32, 1u32, 1u32),
            (CHAIN_SPELL, 0, 1),
            (CHAIN_SPELL, 1, 99),
        ] {
            let mut app = beam_app();
            let (caster, t1) = (unit(&mut app), unit(&mut app));
            app.world_mut()
                .entity_mut(caster)
                .insert(ChainHops(vec![t1]));
            let mut p = play(caster, spell_id);
            p.proc.effect_id = effect_id;
            p.proc.beams = beams_param;
            app.world_mut().write_message(p);
            app.update();
            assert!(
                beams(&mut app).is_empty(),
                "spell {spell_id} / beams {beams_param} / chain {effect_id} draws nothing"
            );
            assert!(
                app.world().entity(caster).get::<ChainHops>().is_none(),
                "…and the array is consumed anyway"
            );
        }
    }

    /// The DBC's backslash path becomes the lane's `mpq://` URL (lowercased, forward slashes) —
    /// the same shape every other BLP load in the client uses.
    #[test]
    fn the_texture_path_becomes_an_mpq_url() {
        assert_eq!(
            beam_texture_url("Textures\\SpellChainEffects\\Lightning.blp"),
            "mpq://textures/spellchaineffects/lightning.blp"
        );
    }
}
