//! Spell chain beams: Chain Lightning's arcs, the Drain Life and Mind Flay beams, Chain Heal. A kit
//! whose `CharProc` decodes to a chain ([`benilla_formats::ChainProc`]) draws a polyline of hops,
//! `caster → t1 → t2 → …`, one ribbon per hop, subdivided, jittered every frame and scrolled: the
//! reference's `LightningObject` (`0x6ec460`) and `CLightning` (`0x7af9b0`/`0x7afcb0`).
//!
//! Hop `i` burns in `[t0 + i × stagger, + life)` (`0x6ec980`), so a 3-hop cast arcs outward, and
//! the beam expires at `t0 + hops × life`. `ChainProc::flag`, the decoded `CharParamTwo`, is the
//! whole cast/channel split: set, both are bypassed and the beam lives until swept by
//! `LightningObject::Stop` (`0x6ece10`), whose one caller is the channel teardown.
//!
//! Endpoints are identities re-resolved from the live units every frame (`0x6ec460`), so a beam
//! tracks moving units, and a hop whose unit does not resolve is hidden, not re-pathed.
//!
//! Render state (`0x7afcb0`, through the `EGxRs` applicator `0x59d350`): additive `SRC_ALPHA/ONE`,
//! emissive white and never tinted, two-sided, depth-write off, fog off, drawn on the shared
//! effect-quad stream ([`benilla_world::particles::buffer::EffectQuads`]).
//!
//! Strands (`CharParamOne`, at most 3, more than one only on Chain Burn) are independently jittered
//! copies of one polyline; the reference builds them from identical arguments and they diverge only
//! through their interleaved draws on one shared PRNG.

use benilla_formats::{ChainEffect, MISSILE_ATTACH_TABLE};
use bevy::prelude::*;

use crate::creature_anim::{ChainProcPlay, FxClass, SpellKitFx, SpellVisuals};
use crate::net::GuidIndex;
use benilla_world::particles::buffer::EffectVertex;
use benilla_world::view::WorldCamera;

use super::OverheadFallback;

/// The caster end's height factor with no `$CSL` marker, `base + modelHeight × modelScale × 0.75`
/// (`0x6ec73e`–`0x6ec771`, the `0.75f` at `0x8012cc`). The height is the authored bbox z-extent
/// that [`OverheadFallback`] carries, standing in for the reference's `obj+0x90`.
const CASTER_HEIGHT_FACTOR: f32 = 0.75;

/// The attachment a non-caster end falls back to when the spell's `SpellVisual` field 9 names
/// none, the reference's literal `0x22` (`0x6ec7b5`–`0x6ec7c7`); below it, the unit's position.
const CHAIN_ATTACH_FALLBACK: u16 = 34;

/// The subdivision floor, the `2.0f` at `0x801628` added at `0x7af716`: even a one-yard hop bends.
const SUBDIVISION_FLOOR: f32 = 2.0;

/// The cross-section normalisation guard (`fcom [0x801360]` at `0x7b01bd`, strictly `>`): a
/// shorter perpendicular stays un-normalized and collapses that segment's width. It measures the
/// screen-plane projection, so it pinches only a hop aimed at the camera, within about 0.023° on a
/// 2.5 yd sub-segment: a guard against NaN, not a visible thinning.
const PERP_EPSILON: f32 = 0.001;

/// The per-frame advection weight on interior points, `main = 0.75·main + 0.25·fresh`
/// (`0x7afc10`/`0x7afc38`), which makes the per-frame re-roll crawl rather than strobe.
const ADVECT_KEEP: f32 = 0.75;

/// Deviation: a cap on one hop's sub-segment count, which the reference lacks, so a vertex run is
/// never sized off table data alone; shipped rows peak at 12 (30 yd at `avgSegLen` 2.78).
const MAX_SUBDIVISIONS: usize = 256;

/// The caster's chain-target hop array, the reference's growable `unit+0xd44`: guids in wire
/// order, the caster's own dropped as it fills (`0x6057bf`/`0x6057c9`). `crate::spell::net` fills
/// it from the `SMSG_SPELL_GO` hit list (`0x6e800d`) and `SMSG_SPELL_UPDATE_CHAIN_TARGETS`
/// (`0x605767`), clearing before each fill, and [`spawn_chain_beams`] consumes it once, as
/// `0x60db72` zeroes the count on every exit. The reference gates the `SMSG_SPELL_GO` fill on
/// `0x6e4870`, a predicate still untraced; here it always fills. Unstreamed targets are left out,
/// as the reference hides a hop it cannot place.
#[derive(Component)]
pub(crate) struct ChainHops(pub(crate) Vec<Entity>);

/// One hop of a live beam, the reference's 16-byte `Bolt`, whose `idxA`/`idxB` are always
/// `(i, i+1)` into the node list.
struct Bolt {
    /// `t0 + i × boltStagger` in scene seconds (`0x6ec9d7`–`0x6ec9e8`).
    start: f32,
    /// `start + boltLife` (`0x6ec9eb`).
    end: f32,
    /// The live jittered polyline per strand, advected rather than rebuilt; empty while the hop is
    /// not drawn.
    strands: Vec<Vec<Vec3>>,
}

/// A live chain beam, the reference's 0x50-byte `LightningObject`.
#[derive(Component)]
pub(crate) struct ChainBeam {
    /// The reap key (`node+0x44`).
    spell_id: u32,
    /// Its loss ends the beam; nothing else would reap a persistent one.
    caster: Entity,
    /// `[caster, hop0, hop1, …]` as identities re-resolved every frame (`node+0x10`); a beam never
    /// re-targets.
    nodes: Vec<Entity>,
    bolts: Vec<Bolt>,
    effect: ChainEffect,
    texture: Handle<Image>,
    /// The attachment every non-caster endpoint anchors at, from the spell's `SpellVisual` field 9,
    /// resolved once where the reference re-reads the unchanging row every frame.
    dest_tag: u16,
    /// The decoded `CharParamTwo`: a channel beam, which never expires by time (`node+0x48`).
    persistent: bool,
    /// `t0 + hops × boltLife` (`0x6ecd30`); ignored while [`Self::persistent`].
    expiry: f32,
    /// The texture-scroll accumulator in seconds (`+0x60`).
    phase: f32,
    rng: u32,
}

/// The DBC's backslash path as the lowercase, forward-slash `mpq://` URL every BLP load uses.
fn beam_texture_url(raw: &str) -> String {
    format!("mpq://{}", raw.to_ascii_lowercase().replace('\\', "/"))
}

/// One xorshift draw in `[−1, 1)`. Deviation: not the reference's generator
/// (`((rand & 0x7fffff) | 0x3f800000)` re-centred through `2.0`), because the look rests on the
/// amplitude and the advection, both exact, never on the draw sequence.
fn jitter(rng: &mut u32) -> f32 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 17;
    *rng ^= *rng << 5;
    (*rng >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
}

/// The caster's end (`0x6ec6f0`): the `$CSL` marker at the live pose, else
/// `base + 0.75 × modelHeight × modelScale`. Other ends anchor at the spell's field-9 attachment
/// through the missile's resolver, as `0x6ec780` and `0x61ceb0` read the same table the same way.
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

/// The beam's participants (`0x60dad4`–`0x60db19`): the channel object when the kit's spell is the
/// live channel, it names an object and the hop array holds at most one entry; else the hop array;
/// else nothing.
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
            // The single-target path (`count = 1, ptr = descriptor+0x38`); an unstreamed channel
            // object leaves nothing to run to, like a hidden hop.
            return index.0.get(&object).copied().into_iter().collect();
        }
    }
    hops.to_vec()
}

/// The CharProc dispatcher's beam case: spawns, replaces and reaps beams. Reaps run before plays,
/// so a GO's spell-keyed reap, which precedes its cast-kit play in the router, never eats the beam
/// that play draws. Only a persistent beam is reapable: the reference publishes only a flagged node
/// to the owner slots the channel teardown sweeps (`0x6ecdaa`), so a one-shot runs its own clock.
/// The reference re-enters the dispatcher every channel tick (`0x612b18`); one held beam shows the
/// same steady beam, ending with the channel.
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
    // The removal below lands at the next sync point, so a second play on the same caster this
    // frame would still see the array; this set is the reference's in-place zeroing.
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
        // The caster may be gone within the same wire drain that produced this play.
        let Ok((store, hops)) = stores.get(play.entity) else {
            continue;
        };
        // Consume once, on every path: `0x60db72` zeroes the count even when nothing is drawn.
        let hops = hops.filter(|_| !consumed.contains(&play.entity));
        consumed.insert(play.entity);
        commands.entity(play.entity).try_remove::<ChainHops>();
        // `0x6ecbd0`'s guards: no spell, no strands or no targets is a silent return.
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
        // `0x6ec780`: the spell's own visual row, field 9 through the missile table, with no
        // ranged fallback.
        let dest_tag = spells
            .catalog
            .get(play.spell_id)
            .and_then(|d| visuals.0.stages(d.visual))
            .and_then(|s| MISSILE_ATTACH_TABLE.get(s.missile_attach as usize).copied())
            .unwrap_or(CHAIN_ATTACH_FALLBACK);
        // A persistent beam replaces the one before it on the same caster and spell.
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
                // Per-beam seed, so neighbouring beams never draw the same jitter on one frame.
                rng: 0x9E37_79B9 ^ (play.spell_id.wrapping_mul(2654435761)),
            },
        ));
    }
}

/// The texture scroll (`0x7af9c7` Update, `0x7b0544` Render): `phase = fmod(phase + dt, period)`
/// and `u = −(phase / period)`; a zero period disables it. `fmod` keeps the dividend's sign, so a
/// negative period only flips `u`'s direction: the drains' `−0.5` flows back toward the caster.
fn advance_scroll(phase: f32, dt: f32, period: f32) -> (f32, f32) {
    if period == 0.0 {
        return (0.0, 0.0);
    }
    let phase = (phase + dt) % period;
    (phase, -(phase / period))
}

/// One hop's fresh subdivision (`0x7af6d0`, rebuilt every frame): `n = trunc(len / avgSegLen + 2)`
/// segments, exact endpoints, and each interior point offset by an independent `[−1, 1]³` vector
/// scaled by `len × noiseScale`.
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

/// One strand's polyline into the shared stream, strip triangles `(t₀,b₀,t₁),(b₀,b₁,t₁)` written
/// as the quad `[b₀,b₁,t₁,t₀]`. Cross-section, ends and texcoords are the reference's
/// (`0x7b0196`–`0x7b0541`): an interior vertex at `p ± 0.5·(perp₍ᵢ₋₁₎ + perpᵢ)·halfWidth`, both
/// ends collapsed to a point at `v = 0.5` (a spindle), `u` 0→1 caster→target plus the scroll, and
/// the constant colour `0xFFFFFFFF`.
///
/// The perpendicular is `(−d.y, d.x, 0)` in eye space: the reference runs the points through
/// `0x7bca80` against `0xcf5800 = T(−cameraPos)·VIEW` (built at `0x7affb1`–`0x7b00c4`; `VIEW` is
/// the pure rotation `0x50ab70` builds), an isometry with no projection, and submits with identity
/// WORLD and VIEW. Eye axes are X right, Y up, +Z into the screen, so the ribbon lies in the view
/// plane and spans `2 × halfWidth` world yards at any distance; in world axes the perpendicular is
/// `d × cam_forward`, one axis for the whole strand.
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
        // The eye-space `(−d.y, d.x, 0)` in magnitude too, so the 0.001 guard transfers. `d × F`,
        // not `F × d`: the other order mirrors `v` across the centreline, and this one puts
        // `v = 0` on top of a rightward beam, as the reference does.
        let raw = (pts[i + 1] - pts[i]).cross(cam_forward);
        if raw.length() > PERP_EPSILON {
            raw.normalize()
        } else {
            raw
        }
    };
    // (top, bottom, v) for point `i`; the collapsed ends share a vertex and stamp v = 0.5.
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
        // v is 0 on the top rail and 1 on the bottom, but 0.5 on both at a collapsed end.
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

/// Per frame: age each beam, re-resolve its endpoints, re-jitter and advect its polylines and write
/// the ribbons, the reference's `LightningObject::Update` (`0x6ec460`) and `CLightning::Render`
/// (`0x7afcb0`) in one pass.
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
    // The reference rebuilds its world→eye map every render (`0x7aff82`–`0x7b00c4`).
    let cam_forward = *cam_xf.forward();
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    for (entity, mut beam) in &mut beams {
        // Dead when `flag == 0 && now >= expiry` (`0x6ec6b9`), or when the caster is gone, the one
        // end a persistent beam would otherwise never get.
        if (!beam.persistent && now >= beam.expiry) || !units.contains(beam.caster) {
            commands.entity(entity).despawn();
            continue;
        }
        let (phase, u_scroll) = advance_scroll(beam.phase, dt, beam.effect.scroll_period_s);
        beam.phase = phase;
        if !images.contains(&beam.texture) {
            continue; // texture not resident yet
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
            // The per-hop window, bypassed while the flag is set (`0x6ec520`).
            if !persistent && !(bolt.start <= now && now < bolt.end) {
                bolt.strands.iter_mut().for_each(Vec::clear);
                continue;
            }
            // A missing endpoint hides only this hop (`SetVisible(handle, 0)`).
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
                    // Advect toward the fresh roll, interior only (`0x7afbe7`–`0x7afc81`).
                    let last = pts.len() - 1;
                    pts[0] = scratch[0];
                    pts[last] = scratch[last];
                    for k in 1..last {
                        pts[k] = pts[k] * ADVECT_KEEP + scratch[k] * (1.0 - ADVECT_KEEP);
                    }
                } else {
                    // First frame, or a new subdivision count: take the fresh polyline whole (the
                    // reference's dirty-bit rebuild).
                    pts.clear();
                    pts.extend_from_slice(&scratch);
                }
                push_strand(batch.verts_mut(), pts, half_width, u_scroll, cam_forward);
            }
        }
        if anchor_n == 0.0 {
            continue; // every hop hidden
        }
        // `EGxRs 0x07 = 3` is `glBlendFunc(SRC_ALPHA, ONE)` and `0x0f = 0` is fog off; depth-write
        // off and two-sided come with the additive pipeline. No owner draw rung and no water-plane
        // interleave: a beam belongs to no model.
        batch
            .additive()
            .anchored(anchor_sum / anchor_n)
            .owner(entity)
            .quads();
    }
}

/// The beam's arithmetic against the reference's constants, and the hop array's consumption.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use benilla_formats::{
        char_proc_type, ChainProc, SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualStages,
    };

    use super::*;
    use crate::net::ObjectStore;

    /// Chain Lightning (`benilla-extract <Data> spellvis 421`): visual 36, chain effect id 1.
    const CHAIN_SPELL: u32 = 421;
    const CHAIN_VISUAL: u32 = 36;

    /// Chain Lightning's `SpellChainEffects` row as shipped (`benilla-extract <Data> chaincensus`).
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
                        // Field 9's ordinal: missile-table index 1 is attachment 34, also the
                        // chain's fallback.
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

    /// A cast-stage Chain Lightning play: chain id 1, one strand, flag clear.
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

    /// `n = trunc(len / avgSegLen + 2.0)` (`0x7af713`): 30 yd gives 12 segments, 13 points.
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

    /// Exact endpoints, and interior offsets bounded by `len × noiseScale` per axis (`0x7af748`).
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
        assert_eq!(verts.len(), 8);
        // Quad corner order is [b₀, b₁, t₁, t₀]: the first quad's b₀/t₀ are the collapsed caster
        // end, and the second's b₁/t₁ the collapsed target end.
        assert_eq!(verts[0].pos, pts[0].to_array());
        assert_eq!(verts[3].pos, pts[0].to_array());
        assert_eq!(verts[5].pos, pts[2].to_array());
        assert_eq!(verts[6].pos, pts[2].to_array());
        let (mid_bottom, mid_top) = (Vec3::from(verts[1].pos), Vec3::from(verts[2].pos));
        assert!(
            (mid_top.distance(mid_bottom) - 1.0).abs() < 1e-5,
            "2 × 0.5 yd"
        );
        let across = (mid_top - mid_bottom).normalize();
        assert!(across.dot(cam_forward).abs() < 1e-6, "square to the camera");
        assert!(across.x.abs() < 1e-6 && across.z.abs() < 1e-6, "±Y here");
        assert_eq!(verts[0].uv[1], 0.5);
        assert_eq!(verts[3].uv[1], 0.5);
    }

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

    /// `0x7af9d7`/`0x7b055f`: the accumulator wraps on `|period|`; a negative one reverses `u`.
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
        assert_eq!(advance_scroll(0.4, 0.25, 0.0), (0.0, 0.0));
    }

    #[test]
    fn jitter_draws_stay_in_range() {
        let mut rng = 0x9E37_79B9u32;
        for _ in 0..10_000 {
            let v = jitter(&mut rng);
            assert!((-1.0..1.0).contains(&v), "{v} out of [−1, 1)");
        }
    }

    /// The single-target path (`0x60dae5`) takes the channel object only with at most one hop.
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

        assert!(app.world().entity(caster).get::<ChainHops>().is_none());
        app.world_mut().write_message(play(caster, CHAIN_SPELL));
        app.update();
        assert_eq!(
            beams(&mut app).len(),
            1,
            "the second play has nothing to run"
        );
    }

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

    #[test]
    fn the_texture_path_becomes_an_mpq_url() {
        assert_eq!(
            beam_texture_url("Textures\\SpellChainEffects\\Lightning.blp"),
            "mpq://textures/spellchaineffects/lightning.blp"
        );
    }
}
