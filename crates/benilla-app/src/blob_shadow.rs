//! The **unit blob shadow** — the soft dark oval under every unit (the player, every NPC, every
//! creature), the reference's per-frame shadow pass rebuilt on the shared surface-decal projector
//! ([`benilla_world::decal`]), drawn on the shared effect stream.
//!
//! **The mechanism** (`0x6d7920` IS the unit shadow draw):
//! - **Draw path**: a per-frame pass over registered model nodes (`0x683dd0`, list `[0xc7cb10]`)
//!   → gate `0x6d78f0` (model streamed, master toggle) → `0x6d7920` → the **same decal chain the
//!   selection ring uses** (`0x6d7330 → 0x6d6fa0 → 0x6d7480`), collector flags `0x2f0122` = the
//!   ring's `0x200122` **+ the liquid receivers** (a gap here: liquid isn't in the
//!   [`GroundDecalSurface`] set yet — the shadow lands on terrain + WMO faces only).
//! - **Frame slot**: PHASE 1, among the opaque drains — `0x6812c5 call 0x683dd0`, fifth of the
//!   row `0x6812b1`–`0x6812ca` inside `0x681070`, which the driver `0x483460` calls at
//!   `0x48361d`. `0x683dd0` is the **M2 node drain**, and the same loop body ticks each node's
//!   object first (`0x48160c call [obj vt+0x38]` → the selection ring) and draws its shadow second
//!   (`0x683ec3`) — so per unit the additive ring goes down and this modulate darkens it, never the
//!   other way round. That second step is **not** a gate: both exits of the tick `0x481540` return
//!   1, so `0x683ea5`'s `je` is dead. What the tick DOES do is write the alpha this draw is about
//!   to read, 30 bytes later — which is the whole of §"Appearance" below. So the shadow
//!   lands after terrain and WMO and **before** everything else: the footprint decals
//!   (`0x483654`), the M2 opaque pass (`0x4836a6`), the water surfaces (phase 3, drawn *between*
//!   the two M2 transparent passes) and both of those passes. Every transparent in the world
//!   paints over it — which is what [`Rung::SHADOW_SORT`](benilla_world::sky_order::Rung) now
//!   expresses, and what it did not until B347: at the old positive rung the shadow was the one
//!   thing in the scene a river or a portal effect could not attenuate.
//! - **Texture**: `Textures\ShadowBlob.blp` — a 32×32 grayscale radial blob (flat gray-160 core,
//!   linear rim to white) under a binary alpha disc. The reference multitextures a procedural 64×8
//!   trapezoid ramp on a second stage (`0x6d81a0`/`0x6d82d0`, blend-mode-selected); its combine
//!   wiring is open (an apitrace question) — here the ramp is the vertex-alpha vertical fade below.
//! - **Box law** (`0x711a20` + the `0x6d7920` corner build): a sequence CAaBox, clamped INTO ±5
//!   per axis (a cap, never a floor), scaled by the world matrix, yaw-rotated with the unit's
//!   facing then **axis-aligned-bounded**. Vertical about the model origin: `+1.0·(zExt/2)` up,
//!   `−(5/3)·(zExt/2)` down. A degenerate horizontal box is the reference's no-op exit (no
//!   shadow). **No** `OBJECT_FIELD_SCALE_X` re-read (the transform scale already carries it), no
//!   ring-style `sqrt` compression, no floor. **WHICH sequence — settled at bytes + pixels**:
//! the draw re-reads
//!   `playableAnimationLookup[0]` every frame — **slot 0 = Stand for characters, from the file
//!   image, so the value never changes** (not the playing sequence: the director's gait-stable
//!   observation falsified that first reading, and reference captures confirmed — 1,682 measured
//!   draws, six bit-stable box sizes, HumanMale 0.9134 × 1.0805 yd permanently, Walk/Run extents
//!   never appear). Full extents, no missing half/scale factor — the standing size IS the law.
//! - **Appearance**: multiplicative darken — `GL_DST_COLOR/GL_ZERO` with the fade riding the
//!   combine, which is exactly the lane's `EffectBlend::Multiply` (`dst × lerp(1, src, α)`).
//!   Vertex diffuse is **white** with α = the model's **base** alpha — `CM2Model+0x180`, read
//!   through the accessor `0x710ca0` (`fld [ecx+0x180]`) at `0x6d7fd6`, then clamped to `[0,1]`,
//!   ×255, `__ftol`, packed `(a<<24)|0x00FFFFFF`. VERIFIED, and it is `+0x180` and not `+0x19c`:
//!   over the whole draw every `0x19c` operand is the stack local `[ebp-0x19c]`, and `this` is
//!   touched at exactly three `mov ecx,ebx; call` sites (box `0x711a20`, matrix `0x710600`,
//!   alpha `0x710ca0`), which makes that list the complete field census.
//!
//!   **What rides in, at the bytes** (2026-09-06): the 2 s appear ramp and the despawn
//!   ramp (`obj+0xf4`, `0x613b1e`'s `0x7d0`), the self first-person fade, **and the CharProc-14
//!   aura transition** — stealth's `0.3`, ghost/invisibility's `0.5`. The aura is not a second
//!   channel: `0x60d180` drives the SAME `StartAlphaFade` (`0x614f80`) the appear fade uses, and
//!   `0x614a90` writes `model+0x180 = obj+0x100 · obj+0xf4` unconditionally given a model handle.
//!   A stealthed unit's blob shadow dims with its body, on the same 1000 ms cubic.
//!
//!   **What does not**: the M2 animated colour / texture-weight tracks. The body's per-batch
//!   alpha is `+0x19c · M2Color[i].alpha · M2TextureWeight[j].value` (`0x707aea`–`0x707b33`);
//!   this draw reads `+0x180` raw, so an asset-authored transparency track fades the body and
//!   leaves its shadow at full strength. [`benilla_world::model_fade::UnitRenderAlpha`] is that
//!   same product and stops at the same place.
//!
//!   The darkness lives in the texture RGB. Unlit, no fog, no depth write. The texture loads as the default `WorldArt`
//!   `Rgba8Unorm`, so the modulate multiplies raw bytes in the gamma lane — the reference's own
//!   arithmetic.
//! - **Gating**: the reference's `shadowLOD` cvar {0,1} is the master toggle (default on) — we are
//!   always-on; `shadowBias` (default 0.1) is its depth-bias knob — [`SHADOW_DEPTH_BIAS`] plays
//!   that role here. No dead/mount/kind test exists on the draw path, and **which** objects
//!   register is now settled (2026-09-06):
//!   `[node+0x90]` bit `0x400` is `NOT(arg bit1)` (`0x670e94`), and `0x613e10` takes that arg off
//!   `OBJECT_FIELD_TYPE` — `0xb` GAMEOBJECT, `2` DYNAMICOBJECT, `0` otherwise. **GameObjects and
//!   DynamicObjects never cast a blob shadow; units, players and corpses always do.** Which is
//!   the v1 policy this lane already ran (every Player/Unit with a built animated model), now a
//!   byte-fact rather than a guess.
//!
//!   **One benilla divergence, deliberate.** `0x6d78f0`/`0x6d7920` read no visibility state at
//!   all — the body's draw flag `[CM2Model+0x50]` comes from `ShouldRender` at `0x48174e`, which
//!   is consumed at `0x48161d`, *after* the tick — so the reference can and does draw a shadow
//!   under an undrawn body (a unit whose skin composite is still unbaked, `0x477860`). We gate on
//!   `InheritedVisibility` anyway, because benilla's exterior-scene election is not a mechanism
//!   the reference has and left Tanaris mobs' shadows on the Caverns of Time floor (decision
//!   1277). Keeping that gate is the 1277 call; it is named here so it reads as a choice.
//!
//! One shadow record per unit, its projected triangles rebuilt only when the inputs move
//! ([`ShadowKey`]) and pushed onto the effect stream every shown frame — an idle unit costs a
//! key compare plus one memcpy of its cached slice. (The stream has no per-draw frustum cull;
//! an off-screen shadow's triangles are vertex-clipped GPU-side — dozens of ~50-vert slices,
//! below any ledger line.)

use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use crate::creature_anim::AnimData;
use crate::net::{Embodied, NetEntity};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::{begin_effect_frame, EffectVertex};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

/// The reference's shadow disc (`Textures\ShadowBlob.blp`, created by `0x6d8070`): grayscale
/// radial blob (gray-160 core → white rim) under a binary alpha disc, multiplied onto the ground.
const SHADOW_TEXTURE: &str = "mpq://textures/shadowblob.blp";
/// The byte clamp on the animation box: each corner component is clamped INTO ±5 yd pre-scale
/// (`0x6992c0` MAX(−5) / `0x699250` MIN(+5) — a cap on huge authored boxes, never a floor).
const BOX_CLAMP: f32 = 5.0;
/// Degenerate-box epsilon (the reference's `[0x8029d4]` = 2.384e-7): a zero horizontal extent is
/// the no-op exit — no shadow.
const DEGENERATE_EPS: f32 = 2.384e-7;

/// One unit's shadow record (a top-level entity — no render components; the cached projection
/// rides the effect stream). Despawned when the owner goes.
#[derive(Component)]
struct BlobShadow {
    owner: Entity,
}

/// Last frame's rebuild inputs — the projection is redone only when one moves. `surfaces` counts
/// the [`GroundDecalSurface`] colliders: a tile streaming in under a *standing* unit changes it,
/// re-arming the rebuild its stillness would otherwise skip.
#[derive(Component, Default)]
struct ShadowKey {
    feet: Vec3,
    rotation: Quat,
    box_min: Vec3,
    box_max: Vec3,
    alpha: f32,
    surfaces: usize,
    shown: bool,
}

/// The cached projection: world-space effect triangles (white × the ramp/fade alpha), pushed
/// onto the stream every shown frame, rebuilt on [`ShadowKey`] change. Empty = hidden.
#[derive(Component, Default)]
struct ShadowVerts(Vec<EffectVertex>);

/// The one shadow texture (kept so the census can report the image's load state — a texture
/// that never arrives withholds every shadow draw at the render-side residency gate, silently).
#[derive(Resource)]
struct ShadowAssets {
    texture: Handle<Image>,
}

pub(crate) struct BlobShadowPlugin;

impl Plugin for BlobShadowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_shadow_assets)
            .add_systems(
                Update,
                (sync_shadows, update_shadows)
                    .chain()
                    // After net motion + input: the decal follows this frame's unit transforms.
                    .after(WorldStage::Input),
            )
            // The stream push: after the frame's stream clear (the caches were rebuilt in
            // `Update`, so this is a pure copy).
            .add_systems(PostUpdate, push_shadows.after(begin_effect_frame));
    }
}

/// Load the shadow disc once.
fn setup_shadow_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    let texture = asset_server.load::<Image>(SHADOW_TEXTURE);
    commands.insert_resource(ShadowAssets { texture });
}

/// Keep one shadow record per eligible unit: spawn for new Player/Unit entities whose model has
/// built (an animated model — [`ModelAnimations`] arrives with it), despawn orphans (owner
/// destroyed / streamed out). The registration *policy* is still open; this is the v1 set (see
/// module docs).
#[allow(clippy::type_complexity)] // the filtered spawn-gate query, commented inline
fn sync_shadows(
    mut commands: Commands,
    // A mount child never gets its own decal: its `Transform` is parent-relative (a shadow
    // keyed on it would project at the world origin) — the mounted composite casts ONE shadow,
    // the unit's, which reads the mount's box while mounted (`update_shadows`).
    units: Query<
        (Entity, &NetEntity),
        (
            With<ModelAnimations>,
            Without<crate::entities::mount::MountBody>,
        ),
    >,
    shadows: Query<(Entity, &BlobShadow)>,
    // The reconciler's inputs move only when a unit's animated model arrives or leaves, or a
    // unit becomes or stops being a mount child (the `units` filter's two terms); a frame with
    // none of those has the same answer it had last frame, and used to pay a set build plus a
    // walk of every unit for it.
    grew: Query<(), Added<ModelAnimations>>,
    mut shrank: RemovedComponents<ModelAnimations>,
    mounted: Query<(), Added<crate::entities::mount::MountBody>>,
    mut unmounted: RemovedComponents<crate::entities::mount::MountBody>,
) {
    if grew.is_empty()
        && mounted.is_empty()
        && shrank.read().next().is_none()
        && unmounted.read().next().is_none()
    {
        return;
    }
    let mut shadowed = EntityHashSet::default();
    for (entity, shadow) in &shadows {
        // Owner gone or no longer eligible (model torn down) → the decal goes with it.
        if units.get(shadow.owner).is_err() {
            commands.entity(entity).despawn();
        } else {
            shadowed.insert(shadow.owner);
        }
    }
    for (owner, net) in &units {
        if !matches!(net.kind, EntityKind::Player | EntityKind::Unit) || shadowed.contains(&owner) {
            continue;
        }
        commands.spawn((
            BlobShadow { owner },
            ShadowKey::default(),
            ShadowVerts::default(),
        ));
    }
}

/// Re-project each shadow whose inputs moved; clear it when the box degenerates, the fade
/// reaches zero, or no receiving surface is in the box (the reference's no-ground gate).
#[allow(clippy::type_complexity)]
fn update_shadows(
    time: Res<Time>,
    catalog: Option<Res<AnimData>>,
    shadow_assets: Option<Res<ShadowAssets>>,
    images: Res<Assets<Image>>,
    decals: WorldDecal,
    // The unit's render alpha, asked of the unit — see the alpha block below.
    unit_alpha: benilla_world::model_fade::UnitRenderAlpha,
    owners: Query<
        (
            &Transform,
            &ModelAnimations,
            Has<Embodied>,
            Option<&crate::entities::mount::MountChild>,
            // …and whether the owner is drawn at all. A body the exterior-scene election sent to
            // pass 2 is not in the reference's scene, so it casts nothing. The
            // election writes the ROOT's `Visibility`; this is that verdict after propagation.
            Option<&InheritedVisibility>,
        ),
        Without<BlobShadow>,
    >,
    // The mounted box source: the composite's one shadow reads the MOUNT's Stand box at the
    // mount's rendered scale while a mount model is attached (the mount IS the footprint on the
    // ground; the rider's box would undersize it). The mount-vs-body source of the client's own
    // shadow box is untraced — this is the named approximation of decision 0441's P2, carried
    // until that source is known.
    mount_anims: Query<(&NetEntity, &ModelAnimations), With<crate::entities::mount::MountBody>>,
    mut shadows: Query<(&BlobShadow, &mut ShadowKey, &mut ShadowVerts)>,
    // Once-a-second census at debug level (`RUST_LOG=benilla_app::blob_shadow=debug` — the lib
    // target is `benilla_app`; a `benilla::` filter silently matches nothing): how many
    // shadows exist and why the hidden ones hid — the first question of any "no shadow under X"
    // report, answerable from a log instead of a debugger.
    mut census_at: Local<f32>,
) {
    let now = time.elapsed_secs();
    let census = now >= *census_at;
    if census {
        *census_at = now + 1.0;
    }
    let (mut n_total, mut n_shown, mut n_no_owner, mut n_no_clip, mut n_degen, mut n_no_ground) =
        (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    // Counted apart from `n_no_owner`: a body the exterior election sent to pass 2 HAS an owner,
    // and folding the two would make the census answer "why did it hide" with a lie. This is the
    // instrument's whole job.
    let mut n_undrawn = 0u32;
    let surface_count = decals.receiver_count();
    for (shadow, mut key, mut verts) in &mut shadows {
        n_total += 1;
        let Ok((unit, anims, is_self, mount_child, drawn)) = owners.get(shadow.owner) else {
            // sync_shadows despawns next frame; keep it cleared meanwhile.
            hide(&mut key, &mut verts);
            n_no_owner += 1;
            continue;
        };
        // An owner that is not drawn casts nothing. The director's report from inside Caverns of
        // Time: the exterior election had correctly stopped drawing the Tanaris
        // mobs overhead, and their shadows carried on being projected onto the cavern floor,
        // because this lane keys off the unit's `Transform` and never asked whether the unit was
        // in the scene. The census counts it separately, so "why is there a shadow with no
        // creature" is answerable from the log rather than from a debugger.
        if !drawn.is_none_or(|v| v.get()) {
            hide(&mut key, &mut verts);
            n_undrawn += 1;
            continue;
        }
        // Mounted: the box and the extra scale column come from the mount child (the
        // `mount_anims` doc above); until its model lands, the rider's own box carries the frame.
        let (anims, extra_scale) = match mount_child.and_then(|mc| mount_anims.get(mc.0).ok()) {
            Some((mnet, manims)) => (manims, mnet.scale),
            None => (anims, 1.0),
        };
        // The byte+pixel law: the box is playableAnimationLookup[0]'s
        // sequence — Stand, permanently (the reference re-reads it per frame from the file image;
        // the value can't change). resolve(0) walks the same baked table, so Stand-less models
        // land on their substitute exactly like the binary's row-0 fast path.
        let stand = catalog.as_deref().map_or(0, |c| anims.resolve(0, &c.0).id);
        let clip = anims.find(stand);
        let Some(clip) = clip else {
            hide(&mut key, &mut verts);
            n_no_clip += 1;
            continue;
        };
        // The box law (see module docs): clamp INTO ±5 pre-scale, scale, yaw-rotate + AA-bound.
        let s = (unit.scale.x * extra_scale).max(0.0);
        let bmin = clip
            .bounds_min
            .clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP))
            * s;
        let bmax = clip
            .bounds_max
            .clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP))
            * s;
        if bmax.x - bmin.x <= DEGENERATE_EPS || bmax.z - bmin.z <= DEGENERATE_EPS {
            // The reference's degenerate-box no-op exit (`0x61e9c0`): no shadow.
            hide(&mut key, &mut verts);
            n_degen += 1;
            continue;
        }
        // **The unit's render alpha, asked of the unit** — the shadow rides the model's fade slot
        // (module docs), so it takes the engine's answer rather than reconstructing one.
        //
        // It used to gather the live `RenderFade`s off this unit's *part* entities and read "no
        // part is fading" as `1.0`. That is true of a settled unit and exactly backwards for the
        // state a unit spends its whole arrival in: a streamed unit holds `PendingAppearFade` —
        // no `RenderFade` anywhere yet — until the world is actually shown, so its parts are
        // deliberately invisible while this walk found nothing to attribute and drew the shadow
        // **fully opaque**. A dark oval on the ground under no creature for the length of the
        // load (2.9 s on a probe login), snapping to zero the instant the ramp armed, then easing
        // back in with the body. `UnitRenderAlpha` asks the root, where `UnitAppearFade` tells
        // pending from settled — the distinction a part-side walk cannot make.
        //
        // The self first-person fade comes with it (the reference rides the same slot), which is
        // why there is no `is_self` term here any more.
        let alpha = unit_alpha.get(shadow.owner);
        if alpha <= 0.0 {
            hide(&mut key, &mut verts);
            continue;
        }
        let next = ShadowKey {
            feet: unit.translation,
            rotation: unit.rotation,
            box_min: bmin,
            box_max: bmax,
            alpha,
            surfaces: surface_count,
            shown: true,
        };
        if key.shown && !key_changed(&key, &next) {
            n_shown += 1;
            continue;
        }
        // Horizontal: the model box's 4 rect corners through the unit's rotation with the
        // vertical column dead (`rot × (x, 0, z)`, XZ taken — the byte build zeroes the z-terms),
        // then axis-aligned-bounded.
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
        for (x, z) in [
            (bmin.x, bmin.z),
            (bmin.x, bmax.z),
            (bmax.x, bmin.z),
            (bmax.x, bmax.z),
        ] {
            let w = unit.rotation * Vec3::new(x, 0.0, z);
            (min_x, max_x) = (min_x.min(w.x), max_x.max(w.x));
            (min_z, max_z) = (min_z.min(w.z), max_z.max(w.z));
        }
        // Vertical about the model origin: `+1.0·(zExt/2)` up, `−(5/3)·(zExt/2)` down (the byte
        // constants `[0xcea60c]`/`[0xcea610]`).
        let half_v = (bmax.y - bmin.y) * 0.5;
        let frame = DecalFrame {
            center: unit.translation,
            sin: 0.0,
            cos: 1.0,
            min_x,
            max_x,
            min_z,
            max_z,
            min_y: -half_v * (5.0 / 3.0),
            max_y: half_v,
        };
        let span_v = frame.max_y - frame.min_y;
        verts.0.clear();
        let projected = span_v > 0.0
            && decals.project(
                &mut verts.0,
                &frame,
                |p| {
                    // The trapezoid ramp over the vertical span (the reference's second texture
                    // stage, `0x6d81a0`: rise x<2, flat, fall x≥10 over x = 12·u). *Interim
                    // seat*: that the ramp runs vertically is inferred from the box's asymmetric
                    // vertical reach; the combine wiring is the flagged apitrace item.
                    alpha * shadow_ramp((p.y - frame.min_y) / span_v)
                },
                |x, z| frame.rect_uv(x, z),
            );
        *key = next;
        key.shown = projected;
        if projected {
            n_shown += 1;
        } else {
            verts.0.clear();
            n_no_ground += 1;
        }
        if census && is_self {
            // The census's self row: where the OWN shadow's projection actually went — the
            // "shadow missing under me" report's second question (the first is the gate line
            // below). Y-extents vs feet split "landed on the surface I stand on" from "fell
            // through to a receiver below" in one read.
            let (mut y_min, mut y_max) = (f32::MAX, f32::MIN);
            let (mut x_min, mut x_max) = (f32::MAX, f32::MIN);
            let (mut z_min, mut z_max) = (f32::MAX, f32::MIN);
            for v in &verts.0 {
                y_min = y_min.min(v.pos[1]);
                y_max = y_max.max(v.pos[1]);
                x_min = x_min.min(v.pos[0]);
                x_max = x_max.max(v.pos[0]);
                z_min = z_min.min(v.pos[2]);
                z_max = z_max.max(v.pos[2]);
            }
            // World-XZ extents of what was actually emitted vs the frame rect: if these disagree,
            // the projector inflated/displaced the box (the -8844,669 hunt).
            debug!(
                "self shadow world span: x [{:.3}, {:.3}] ({:.3}), z [{:.3}, {:.3}] ({:.3}); \
                 feet ({:.3}, {:.3}), yaw {:.1} deg",
                x_min,
                x_max,
                x_max - x_min,
                z_min,
                z_max,
                z_max - z_min,
                unit.translation.x,
                unit.translation.z,
                unit.rotation
                    .to_euler(bevy::math::EulerRot::YXZ)
                    .0
                    .to_degrees(),
            );
            debug!(
                "self shadow: feet y {:.3}, box y [{:.3}, {:.3}], {} verts, vert y [{:.3}, \
                 {:.3}]",
                unit.translation.y,
                unit.translation.y + frame.min_y,
                unit.translation.y + frame.max_y,
                verts.0.len(),
                y_min,
                y_max
            );
            // The -8844,669 hunt's uv probe: a span check alone can't see a DEGENERATE mapping
            // (all uv.y at the rim still spans 0..1) — print the actual per-vert uv pairs.
            let uvs: Vec<String> = verts
                .0
                .iter()
                .map(|v| format!("({:.3},{:.3})", v.uv[0], v.uv[1]))
                .collect();
            debug!("self shadow uvs: {}", uvs.join(" "));
            // And the box/rect numbers: measured on the reference client, HumanFemale's
            // footprint is 0.77x0.74 yd nearly centred; a bigger or offset rect indicts the box
            // math, not the projector.
            debug!(
                "self shadow box: bmin {:?} bmax {:?} rect x [{:.3}, {:.3}] z [{:.3}, {:.3}] \
                 (extent {:.3}x{:.3}, centre offset ({:.3}, {:.3}))",
                bmin,
                bmax,
                min_x,
                max_x,
                min_z,
                max_z,
                max_x - min_x,
                max_z - min_z,
                (min_x + max_x) * 0.5,
                (min_z + max_z) * 0.5,
            );
        }
    }
    if census && n_total > 0 {
        // Not just "loaded": the CONTENT. A white-decoded blob multiplies to a no-op — an
        // invisible shadow whose every draw-side reading looks healthy (the -8844,669 hunt).
        let tex = shadow_assets.map_or("no-resource".into(), |a| {
            images.get(&a.texture).map_or("MISSING".into(), |img| {
                let (w, h) = (img.width(), img.height());
                let center = img
                    .data
                    .as_ref()
                    .and_then(|d| {
                        let i = ((h / 2) * w + w / 2) as usize * 4;
                        d.get(i..i + 4).map(|p| format!("{p:?}"))
                    })
                    .unwrap_or_else(|| "no-data".into());
                // The -8844,669 hunt: the whole centre ROW, not one texel — the ink radius and
                // alpha reach decide how much of the box the disc visibly fills.
                if let Some(d) = img.data.as_ref() {
                    let row: Vec<String> = (0..w as usize)
                        .map(|x| {
                            let i = ((h / 2) as usize * w as usize + x) * 4;
                            d.get(i..i + 4)
                                .map(|p| format!("{}/{}", p[0], p[3]))
                                .unwrap_or_default()
                        })
                        .collect();
                    debug!("blob row {}: {}", h / 2, row.join(" "));
                }
                format!("loaded {w}x{h} center {center} id {:?}", a.texture.id())
            })
        });
        debug!(
            "blob shadows: {n_total} ({n_shown} shown, {n_no_owner} ownerless, {n_undrawn} \
             undrawn, {n_no_clip} no-clip, {n_degen} degenerate, {n_no_ground} no-ground; \
             {surface_count} surfaces; texture {tex})"
        );
    }
}

/// Push every shown shadow's cached triangles onto the stream — one Multiply draw per unit at
/// the shadow rung (the pre-water decal band's floor, the module header's frame slot), fog off:
/// the reference's shadow pass state.
fn push_shadows(
    assets: Option<Res<ShadowAssets>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
    shadows: Query<(Entity, &ShadowKey, &ShadowVerts)>,
) {
    let Some(assets) = assets else { return };
    let Ok(cam) = cam.single() else { return };
    for (entity, key, verts) in &shadows {
        if verts.0.is_empty() {
            continue;
        }
        // Multiply: the modulate decal is its own darkening — the scene light is already in the
        // ground it multiplies, which is why the lane's unlit default is right here.
        let mut batch = draw
            .batch(cam, assets.texture.id())
            .multiply()
            .anchored(key.feet)
            .rung(
                benilla_world::sky_order::Rung::SHADOW_SORT,
                benilla_world::sky_order::Rung::DECAL_RASTER,
            )
            .owner(entity);
        batch.vertices(&verts.0);
        batch.tris();
    }
}

/// Clear the record and drop the cache key so the next eligible frame rebuilds from scratch.
fn hide(key: &mut ShadowKey, verts: &mut ShadowVerts) {
    key.shown = false;
    verts.0.clear();
}

/// Did any rebuild input move beyond noise? Position/box at a millimetre, rotation at ~0.05°,
/// alpha at under a colour step.
fn key_changed(a: &ShadowKey, b: &ShadowKey) -> bool {
    const POS_EPS: f32 = 1e-3;
    a.feet.distance_squared(b.feet) > POS_EPS * POS_EPS
        || a.rotation.angle_between(b.rotation) > 1e-3
        || (a.box_min - b.box_min).abs().max_element() > POS_EPS
        || (a.box_max - b.box_max).abs().max_element() > POS_EPS
        || (a.alpha - b.alpha).abs() > 1.0 / 255.0
        || a.surfaces != b.surfaces
}

/// The reference's trapezoid alpha ramp (`0x6d81a0`/`0x6d82d0`: `x = 12·u` — rise `x<2 → x/2`,
/// flat `2≤x<10 → 1`, fall `x≥10 → (12−x)/2`, clamped at 0).
fn shadow_ramp(u: f32) -> f32 {
    let x = 12.0 * u.clamp(0.0, 1.0);
    if x < 2.0 {
        0.5 * x
    } else if x < 10.0 {
        1.0
    } else {
        (0.5 * (12.0 - x)).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramp's byte-verified shape: rise to 1 at x=2 (u=1/6), flat through x=10 (u=5/6), fall
    /// to 0 at x=12 (u=1).
    #[test]
    fn ramp_matches_reference_trapezoid() {
        assert_eq!(shadow_ramp(0.0), 0.0);
        assert!((shadow_ramp(1.0 / 12.0) - 0.5).abs() < 1e-6); // x=1 → 0.5
        assert!((shadow_ramp(1.0 / 6.0) - 1.0).abs() < 1e-6); // x=2 → 1.0
        assert_eq!(shadow_ramp(0.5), 1.0);
        assert!((shadow_ramp(5.0 / 6.0) - 1.0).abs() < 1e-6); // x=10 → 1.0
        assert!((shadow_ramp(11.0 / 12.0) - 0.5).abs() < 1e-6); // x=11 → 0.5
        assert_eq!(shadow_ramp(1.0), 0.0);
        // Out-of-range clamps, never negative.
        assert_eq!(shadow_ramp(-1.0), 0.0);
        assert_eq!(shadow_ramp(2.0), 0.0);
    }

    /// The box law: clamp INTO ±5 pre-scale (a cap, not a floor), then scale.
    #[test]
    fn box_clamp_caps_pre_scale() {
        let raw = Vec3::new(-7.0, 0.0, 3.0);
        let clamped = raw.clamp(Vec3::splat(-BOX_CLAMP), Vec3::splat(BOX_CLAMP));
        assert_eq!(clamped, Vec3::new(-5.0, 0.0, 3.0));
        // A scale-2 unit's clamped box still doubles — the cap is pre-scale.
        assert_eq!(clamped * 2.0, Vec3::new(-10.0, 0.0, 6.0));
    }
}
