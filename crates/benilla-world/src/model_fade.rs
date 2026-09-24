//! The 1.12 world-doodad distance fade (`0x683f80`, run every frame from the world render
//! `0x681070`). `d` is the horizontal camera distance to the bounding-sphere centre
//! (`rec+0x5c/0x60`) minus the radius (`rec+0x68`), and the radius alone picks the band: a candle
//! fades at 40 to 50 yd, a fence (0.75 to 4.97 yd) or haystack at 100 to 125 or 150 to 200, a tree
//! never (`cargo run -p benilla-formats --example fade_bucket -- <name>` measures a model). The
//! alpha reaches the fragment's `diffuse.a` (`CM2Model+0x180` → `+0x19c`), where the 224/255 alpha
//! test erodes a cutout edge-first; a fading doodad draws blended. Constants: radius cutoffs
//! `0x810188`/`0x81018c`/`0x810190`, band ends (start + range) `0x8101a0`/`a4`/`a8`, ranges
//! `0x810194`/`98`/`9c`, `1.0` at `0x7ff9d8`.

use benilla_assets::materials::WowModelMaterial;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

/// A doodad submesh's distance-fade inputs. `apply_model_visibility` writes the fade into its
/// `MeshTag` and draws `blend` while `0 < fade < 1`, `cutout` otherwise.
#[derive(Component, Clone)]
pub struct DoodadFade {
    /// The scaled bounding-sphere radius (yd), the reference's `rec+0x68` (`0x6952a0`).
    pub(crate) radius: f32,
    /// The model-local bounding-box centre the fade measures to (`0x6952a0`), not the origin.
    pub(crate) local_center: Vec3,
    /// The steady material, in the submesh's authored blend mode.
    pub(crate) cutout: Handle<WowModelMaterial>,
    /// Its `AlphaMode::Blend` twin, drawn only while feathering.
    pub(crate) blend: Handle<WowModelMaterial>,
}

/// Above this bounding radius (yd) a doodad never distance-fades; only the far clip drops it.
pub const NEVER_FADE_RADIUS: f32 = 7.0;

/// `(max_radius, band_start, band_range)` in yd; a doodad takes the first row it does not exceed.
const BUCKETS: [(f32, f32, f32); 3] = [
    (0.5, 40.0, 10.0),
    (2.5, 100.0, 25.0),
    (NEVER_FADE_RADIUS, 150.0, 50.0),
];

/// The distance-fade alpha of a doodad of `radius` yd (placement scale applied) whose centre is
/// `horiz_dist` yd away in the horizontal plane (the reference ignores height). `0.0` means cull.
pub fn doodad_fade_alpha(radius: f32, horiz_dist: f32) -> f32 {
    if radius > NEVER_FADE_RADIUS {
        return 1.0;
    }
    let d = horiz_dist - radius;
    let (_, start, range) = BUCKETS
        .iter()
        .copied()
        .find(|(max_r, _, _)| radius <= *max_r)
        // Unreached: the last row's bound is `NEVER_FADE_RADIUS`.
        .unwrap_or((NEVER_FADE_RADIUS, 150.0, 50.0));
    (1.0 - (d - start) / range).clamp(0.0, 1.0)
}

/// The `(near, far)` centre distances between which [`doodad_fade_alpha`] feathers, from the same
/// `BUCKETS` rows, so the retained static pass that rings its cells on them cannot disagree.
pub fn fade_band(radius: f32) -> Option<(f32, f32)> {
    if radius > NEVER_FADE_RADIUS {
        return None;
    }
    let (_, start, range) = BUCKETS
        .iter()
        .copied()
        .find(|(max_r, _, _)| radius <= *max_r)
        .unwrap_or((NEVER_FADE_RADIUS, 150.0, 50.0));
    Some((start + radius, start + radius + range))
}

/// A render-alpha ramp on one part's `MeshTag`, drawn on the blend twin while `α < 1`
/// ([`apply_render_fade`]). The appear ramp is `FadeTo` (`0x614f80`), cubic over 2 s from first
/// visibility; the teardown is the `SWModelFadeout` pump (`0x672ef0`) on the model an object left
/// behind ([`DespawnFade`]). The interior classifier keeps the light law through either.
#[derive(Component, Clone)]
pub struct RenderFade {
    /// `Time::elapsed_secs` at arming.
    pub started: f32,
    /// Seconds; both reference ramps are 2000 ms (`FadeTo`'s `0x7d0`, the pump's `age > 0x7d0`).
    pub duration: f32,
    /// Appear `0 → 1`; teardown `live α → 0`, `from` being the pump's `startAlpha`.
    pub from: f32,
    pub to: f32,
    /// Which of the reference's two ramps this is.
    pub curve: FadeCurve,
}

/// The reference's two render-alpha ramps, two functions and not one curve: the CGObject's appear
/// ease (`0x614a90`, `t³`) and the fadeout scheduler's teardown (`0x672ef0`, `smoothstep(1 − t)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FadeCurve {
    #[default]
    Cubic,
    Smoothstep,
}

impl RenderFade {
    /// This fade's alpha at fractional age `t`, by its own curve.
    pub fn alpha_at(&self, t: f32) -> f32 {
        match self.curve {
            FadeCurve::Cubic => fade_alpha(self.from, self.to, t),
            FadeCurve::Smoothstep => teardown_fade_alpha(self.from, t),
        }
    }
}

/// The appear fade's length, `FadeTo(1.0, 2000 ms)` on the wall clock (`OsGetAsyncTimeMs`).
pub const APPEAR_FADE_SECS: f32 = 2.0;

impl RenderFade {
    /// A spawn appear-fade armed at `now`: `α = t³` from 0 → 1 over [`APPEAR_FADE_SECS`].
    pub fn appear(now: f32) -> Self {
        Self {
            started: now,
            duration: APPEAR_FADE_SECS,
            from: 0.0,
            to: 1.0,
            curve: FadeCurve::Cubic,
        }
    }
}

/// The appear ramp, `lerp(from, to, clamp(t, 0, 1)³)` at fractional age `t` (`0x614a90`).
pub fn fade_alpha(from: f32, to: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    from + (to - from) * t * t * t
}

/// The teardown ramp, the `SWModelFadeout` pump (`0x672ef0`): `α = (3 − 2t) · t²` with
/// `t = clamp((1 − t_frac) · startAlpha, 0, 1)`, `t_frac` being `age_ms · 0.0005` (`[0x80c698]`).
/// `start_alpha`, the live alpha at the hand-off (`obj+0xf4`), scales the ramp, not the output.
pub fn teardown_fade_alpha(start_alpha: f32, t_frac: f32) -> f32 {
    let t = ((1.0 - t_frac) * start_alpha).clamp(0.0, 1.0);
    (3.0 - 2.0 * t) * t * t
}

/// The scheduler's skip gate (`0x672df0` @ `0x672e21`, against `[0x8029d0]`): a model handed over
/// below this alpha is unlinked at once (`0x671ac0`), not faded.
pub const TEARDOWN_MIN_ALPHA: f32 = 0.01;

/// A model instance's render alpha, the reference's `CM2Model+0x19c` (`argAlpha · +0x180`, where
/// `+0x180` is `obj+0x100 · obj+0xf4` for a CGObject and the distance fade for a doodad), for the
/// consumers that cannot read a `MeshTag`: particles and ribbons. Missing reads `1.0`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ModelAlpha(pub f32);

/// A fade the game declares on an instance root (an aura's ramp, stealth, a ghost form); the engine
/// folds it into the composed alpha and owns the write. Absent reads `1.0`.
#[derive(Component, Clone, Copy, Debug)]
pub struct ModelFade(pub f32);

/// The model this one is attached to, the reference's `[model+0x1cc]` (`0x712f70`,
/// `CM2Model::attachChild`). A spawn site names only its immediate parent.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct ParentModel(pub Entity);

/// The bound on a [`ParentModel`] walk, a cycle backstop: real chains are 1 to 3 links.
pub const MAX_MODEL_CHAIN: usize = 8;

/// A model instance's composed render alpha, the reference's recursion (`0x714000`, `0x714260`):
/// `child+0x19c = parent+0x19c × child+0x180` up the [`ParentModel`] chain, so an effect passes
/// its own instance root. A link with no published [`ModelAlpha`] (a booth bake) reads its
/// [`ModelFade`]; the published one wins.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ModelAlphas<'w, 's> {
    chain: Query<
        'w,
        's,
        (
            Option<&'static ModelAlpha>,
            Option<&'static ModelFade>,
            Option<&'static ParentModel>,
        ),
    >,
}

impl ModelAlphas<'_, '_> {
    /// `instance`'s composed alpha; a despawned link (its effects freed with it) ends the walk.
    pub fn get(&self, instance: Entity) -> f32 {
        let mut alpha = 1.0;
        let mut at = instance;
        for _ in 0..MAX_MODEL_CHAIN {
            let Ok((published, declared, parent)) = self.chain.get(at) else {
                break;
            };
            // Published wins: it already folds the declared fade in.
            if let Some(a) = published.map(|a| a.0).or(declared.map(|d| d.0)) {
                alpha *= a;
            }
            match parent {
                Some(p) => at = p.0,
                None => break,
            }
        }
        alpha
    }
}

/// One streamed model's render alpha: the product of the appear ramp (0 while pending, hiding its
/// effects too), the teardown, the self zoom feather and the declared fade, as the reference
/// multiplies the transition alpha into `+0x180`; its aura recompute drives the appear fade's own
/// `StartAlphaFade` (`0x60d180` → `0x614f80` → `obj+0xf4` → `+0x180` → `+0x19c` → `emitter+0x1a8`).
pub fn model_render_alpha(
    now: f32,
    appear: Option<UnitAppearFade>,
    despawn_started: Option<f32>,
    self_fade: f32,
    declared: f32,
) -> f32 {
    let appear = match appear {
        None => 1.0,
        Some(UnitAppearFade::Pending { .. }) => 0.0,
        Some(UnitAppearFade::Live { started }) => {
            fade_alpha(0.0, 1.0, (now - started) / APPEAR_FADE_SECS)
        }
    };
    // The mesh channel's teardown smoothstep, so effects thin out with their geometry.
    let despawn = despawn_started.map_or(1.0, |started| {
        teardown_fade_alpha(1.0, (now - started) / APPEAR_FADE_SECS)
    });
    (appear * despawn * self_fade * declared).clamp(0.0, 1.0)
}

/// A streamed unit's render alpha computed from its root, for a consumer that runs before
/// [`publish_model_alpha`] (in `Update`, or on a unit's first frame). Ask the root: a pending
/// unit's parts carry no [`RenderFade`], so a walk over them reads it as opaque.
#[derive(bevy::ecs::system::SystemParam)]
#[allow(clippy::type_complexity)] // one query, the four facets of a unit's presentation
pub struct UnitRenderAlpha<'w, 's> {
    time: Res<'w, Time>,
    viewer: Res<'w, crate::view::Viewer>,
    units: Query<
        'w,
        's,
        (
            Option<&'static UnitAppearFade>,
            Option<&'static DespawnFade>,
            Option<&'static ModelFade>,
            Has<crate::world_unit::ViewerUnit>,
        ),
    >,
}

impl UnitRenderAlpha<'_, '_> {
    /// `unit`'s render alpha, zoom feather included; `1.0` for an entity it cannot read.
    pub fn get(&self, unit: Entity) -> f32 {
        let Ok((appear, despawn, declared, is_self)) = self.units.get(unit) else {
            return 1.0;
        };
        model_render_alpha(
            self.time.elapsed_secs(),
            appear.copied(),
            despawn.and_then(DespawnFade::armed),
            if is_self { self.viewer.self_fade } else { 1.0 },
            declared.map_or(1.0, |d| d.0),
        )
    }
}

#[cfg(test)]
mod unit_render_alpha_tests {
    use super::*;
    use bevy::ecs::system::SystemState;

    fn unit_at(now: f32, build: impl FnOnce(&mut bevy::ecs::world::EntityWorldMut)) -> f32 {
        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f32(now));
        world.insert_resource(time);
        world.insert_resource(crate::view::Viewer::default());
        let mut unit = world.spawn_empty();
        build(&mut unit);
        let unit = unit.id();
        let mut state = SystemState::<UnitRenderAlpha>::new(&mut world);
        let alpha = state.get(&world).get(unit);
        alpha
    }

    #[test]
    fn a_pending_unit_is_zero_and_a_settled_one_is_opaque() {
        assert_eq!(
            unit_at(10.0, |u| {
                u.insert(UnitAppearFade::Pending { since: 9.0 });
            }),
            0.0,
            "pending: not shown yet, so nothing that rides this number may show either"
        );
        assert_eq!(
            unit_at(11.0, |u| {
                u.insert(UnitAppearFade::Live { started: 10.0 });
            }),
            fade_alpha(0.0, 1.0, 0.5),
            "live: the same cubic the body's parts run"
        );
        assert_eq!(
            unit_at(10.0, |_| {}),
            1.0,
            "nothing in flight: opaque, and no arithmetic paid for it"
        );
    }

    #[test]
    fn an_unarmed_despawn_stamp_is_not_a_fade() {
        assert_eq!(DespawnFade::default().armed(), None);
        assert_eq!(DespawnFade { started: 4.0 }.armed(), Some(4.0));
        assert_eq!(
            unit_at(10.0, |u| {
                u.insert(DespawnFade::default());
            }),
            1.0,
            "marked but not armed: still fully there"
        );
        assert_eq!(
            unit_at(11.0, |u| {
                u.insert(DespawnFade { started: 10.0 });
            }),
            teardown_fade_alpha(1.0, 0.5),
            "armed: the ramp down — the SWModelFadeout smoothstep, not the appear cubic reversed"
        );
    }

    #[test]
    fn the_teardown_curve_is_not_the_appear_curve_reversed() {
        assert_eq!(teardown_fade_alpha(1.0, 0.0), 1.0, "opaque at the hand-off");
        assert_eq!(teardown_fade_alpha(1.0, 1.0), 0.0, "gone at the window end");
        // Mid-window: 0.5, against the reversed cubic's 1 − 0.5³ = 0.875.
        assert!((teardown_fade_alpha(1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((fade_alpha(1.0, 0.0, 0.5) - 0.875).abs() < 1e-6);
        let mut prev = f32::INFINITY;
        for i in 0..=20 {
            let a = teardown_fade_alpha(1.0, i as f32 / 20.0);
            assert!(a <= prev + 1e-6, "monotone at t={i}");
            prev = a;
        }
        assert_eq!(
            teardown_fade_alpha(1.0, 2.0),
            0.0,
            "clamped past the window"
        );

        assert!(
            teardown_fade_alpha(0.25, 0.0) < 0.25,
            "smoothstep(0.25) < 0.25"
        );
        assert_eq!(
            teardown_fade_alpha(0.0, 0.0),
            0.0,
            "handed nothing, shows nothing"
        );
    }

    #[test]
    fn an_unreadable_owner_is_opaque() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(crate::view::Viewer::default());
        let gone = world.spawn_empty().id();
        world.despawn(gone);
        let mut state = SystemState::<UnitRenderAlpha>::new(&mut world);
        assert_eq!(state.get(&world).get(gone), 1.0);
    }
}

/// Publish [`ModelAlpha`] on every streamed object. `PostUpdate`: after every fade writer and the
/// self feather, before the effect sims that read it ([`crate::particles`], [`crate::ribbons`]).
/// An object that never faded gets none.
#[allow(clippy::type_complexity)] // one query, five optional facets of the same entity
pub(crate) fn publish_model_alpha(
    time: Res<Time>,
    viewer: Res<crate::view::Viewer>,
    mut commands: Commands,
    mut units: Query<
        (
            Entity,
            Option<&UnitAppearFade>,
            Option<&DespawnFade>,
            Has<crate::world_unit::ViewerUnit>,
            Option<Ref<ModelFade>>,
            Option<&mut ModelAlpha>,
        ),
        With<crate::world_unit::WorldUnit>,
    >,
    // A declaration that went away is a change the `Ref` cannot see: those units are due.
    mut undeclared: RemovedComponents<ModelFade>,
    // So is the appear ramp's removal: the alpha must land on its final value.
    mut ramp_done: RemovedComponents<UnitAppearFade>,
) {
    let now = time.elapsed_secs();
    let mut undeclared: bevy::platform::collections::HashSet<Entity> = undeclared.read().collect();
    undeclared.extend(ramp_done.read());
    for (entity, appear, despawn, is_self, declared, current) in &mut units {
        // No ramp and not the self body: only a changed declaration can move the alpha.
        if appear.is_none()
            && despawn.is_none()
            && !is_self
            && current.is_some()
            && !declared.as_ref().is_some_and(|d| d.is_changed())
            && !undeclared.contains(&entity)
        {
            continue;
        }
        let declared = declared.as_deref();
        let alpha = model_render_alpha(
            now,
            appear.copied(),
            despawn.and_then(DespawnFade::armed),
            if is_self { viewer.self_fade } else { 1.0 },
            // Ticked in `Update` (`apply_aura_alpha`), so fresh here. `self_fade` is the bare zoom
            // feather, so the self body does not take the aura twice.
            declared.map_or(1.0, |d| d.0),
        );
        match current {
            Some(mut c) => {
                if c.0 != alpha {
                    c.0 = alpha;
                }
            }
            None if alpha >= 1.0 => {}
            None => {
                commands.entity(entity).insert(ModelAlpha(alpha));
            }
        }
    }
}

/// The camera-to-target span (yd) over which the player's own body fades in (`0x8089b0`).
pub const SELF_FADE_WINDOW: f32 = 1.8315;
/// The distance above the near clip at or below which the body hides: first person.
pub const SELF_FADE_HIDE: f32 = 0.00278;

/// The player's own body alpha by camera distance (`0x5b7bb0`): `(1 − cos(π·D/window)) / 2` for
/// `D = dist − nearclip`, 0 at or below [`SELF_FADE_HIDE`]. Pass the live `nearclip`
/// ([`crate::view::ViewDistance::nearclip`]): the fade is measured from the near plane.
pub fn self_model_fade_alpha(dist: f32, nearclip: f32, window: f32) -> f32 {
    let d = dist - nearclip;
    if d <= SELF_FADE_HIDE {
        return 0.0;
    }
    if d >= window {
        return 1.0;
    }
    0.5 * (1.0 - (std::f32::consts::PI * d / window).cos())
}

/// Drive every live [`RenderFade`]: write its alpha into the `MeshTag`, draw the blend twin of the
/// part's live light law ([`crate::interior::InteriorLit`]) while `α < 1`, resolved every frame so
/// a part that classifies mid-ramp follows its room, and remove an appear fade once opaque.
#[allow(clippy::type_complexity)]
pub fn apply_render_fade(
    time: Res<Time>,
    mut commands: Commands,
    // The water-plane axis, composed into this writer's pick (`far_resolved`) so the ramp and the
    // classifier derive the same handle instead of re-swapping.
    far_twins: Res<crate::model_render::FarSideTwins>,
    mut q: Query<(
        Entity,
        &RenderFade,
        &mut MeshTag,
        &mut MeshMaterial3d<WowModelMaterial>,
        Option<&crate::doodad_anim::MatAnim>,
        Option<&FadeMaterials>,
        Option<&crate::interior::InteriorLit>,
        Has<crate::model_render::FarSideOfWater>,
    )>,
) {
    let now = time.elapsed_secs();
    for (entity, fade, mut tag, mut mat, anim, fm, lit, far_side) in &mut q {
        let t = if fade.duration > 0.0 {
            (now - fade.started) / fade.duration
        } else {
            1.0
        };
        // The batch's animated factor multiplies in (`A = instanceAlpha × colourAlpha × weight`,
        // `0x707b0b`, `0x707b33`), or death-only geometry shows through the ramp.
        let alpha = fade.alpha_at(t) * anim.map_or(1.0, |a| a.current);
        // `with_alpha` keeps the ground shade and never writes the untagged, opaque `0`.
        let bits = crate::mesh_tag::with_alpha(tag.0, alpha);
        if tag.0 != bits {
            tag.0 = bits;
        }
        // The cutout ignores `α`; a part without `FadeMaterials` still ramps, never hanging hidden.
        if let Some(fm) = fm {
            let want = crate::model_render::far_resolved(
                fm.material_for(lit, alpha < 1.0),
                far_side,
                &far_twins,
            );
            if mat.0 != *want {
                mat.0 = want.clone();
            }
        }
        // An opaque appear fade hands the material back to the interior classifier.
        if t >= 1.0 && fade.to >= 1.0 {
            commands.entity(entity).try_remove::<RenderFade>();
        }
    }
}

/// An appear fade waiting to arm, the entity hidden meanwhile. The reference arms it on the object
/// becoming visible (`0x4651a0`), not at stream-in, so it never plays behind a loading screen.
#[derive(Component, Clone)]
pub struct PendingAppearFade {
    /// `Time::elapsed_secs` at attach, the backstop timeout's origin.
    pub since: f32,
}

/// Arm a pending fade after this long anyway, so a stuck load cannot leave an entity invisible.
const PENDING_TIMEOUT_SECS: f32 = 8.0;

/// The appear fade's clock on a streamed unit's root. The reference fades a unit and its attached
/// models as one, so a part that resolves later (a held item) joins this ramp; it is dropped when
/// the ramp ends, after which a new part spawns steady.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum UnitAppearFade {
    /// Waiting for the world to be shown.
    Pending { since: f32 },
    /// A joiner copies `started`, which reproduces the same curve.
    Live { started: f32 },
}

/// What a part spawning onto a unit does about the unit's appear fade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinedFade {
    /// No unit fade in flight: spawn steady.
    Steady,
    /// Arm with the unit once the world is shown.
    Pending { since: f32 },
    /// Join the running ramp at its current position.
    Live { started: f32 },
}

/// The join decision for a part spawning onto a unit whose root carries `unit`.
pub fn join_unit_appear_fade(unit: Option<UnitAppearFade>) -> JoinedFade {
    match unit {
        None => JoinedFade::Steady,
        Some(UnitAppearFade::Pending { since }) => JoinedFade::Pending { since },
        Some(UnitAppearFade::Live { started }) => JoinedFade::Live { started },
    }
}

/// Arm each [`PendingAppearFade`] once the loading screen stops covering the world, or after
/// [`PENDING_TIMEOUT_SECS`], and advance each root's [`UnitAppearFade`] on the same instant.
pub(crate) fn arm_appear_fade(
    time: Res<Time>,
    viewer: Res<crate::view::Viewer>,
    mut commands: Commands,
    q: Query<(
        Entity,
        &PendingAppearFade,
        Has<crate::billboard::BillboardCard>,
    )>,
    mut units: Query<&mut UnitAppearFade>,
) {
    let now = time.elapsed_secs();
    let shown = !viewer.world_covered;
    let mut armed = 0usize;
    // Counted apart: a world-root billboard card arms only through its own spawn.
    let mut armed_cards = 0usize;
    for (entity, pending, is_card) in &q {
        if shown || now - pending.since > PENDING_TIMEOUT_SECS {
            armed += 1;
            armed_cards += usize::from(is_card);
            // `try_*`, like every fade command here: the entities are wire-owned, and a net destroy
            // can land between this query and its commands.
            commands
                .entity(entity)
                .try_insert(RenderFade::appear(now))
                .try_remove::<PendingAppearFade>();
        }
    }
    for mut unit_fade in &mut units {
        if let UnitAppearFade::Pending { since } = *unit_fade {
            if shown || now - since > PENDING_TIMEOUT_SECS {
                *unit_fade = UnitAppearFade::Live { started: now };
            }
        }
    }
    // `WOW_INTERIOR_LOG=1`: the arming instant the `[interior]` lines are read against.
    if armed > 0 && std::env::var_os("WOW_INTERIOR_LOG").is_some() {
        eprintln!(
            "[fade-arm] t {now:.2} armed {armed} parts ({armed_cards} billboard cards) \
             (world shown: {shown})"
        );
    }
}

/// Drop a [`UnitAppearFade::Live`] once its ramp has run, so a later part spawns steady.
pub(crate) fn retire_unit_appear_fade(
    time: Res<Time>,
    mut commands: Commands,
    q: Query<(Entity, &UnitAppearFade)>,
) {
    let now = time.elapsed_secs();
    for (entity, fade) in &q {
        if let UnitAppearFade::Live { started } = fade {
            if now - started >= APPEAR_FADE_SECS {
                // `try_remove`: the entity is wire-owned (`arm_appear_fade`).
                commands.entity(entity).try_remove::<UnitAppearFade>();
            }
        }
    }
}

/// A fadeable part's materials, which every fade resolves per frame ([`Self::material_for`]):
/// `cutout` steady, `blend` its `AlphaMode::Blend` twin.
#[derive(Component, Clone)]
pub struct FadeMaterials {
    pub cutout: Handle<WowModelMaterial>,
    pub blend: Handle<WowModelMaterial>,
    /// The interior bake variant's blend twin, keeping a bake-lit part's room light as it feathers.
    pub bake_blend: Option<Handle<WowModelMaterial>>,
    /// The depth-prime twin, the reference's `M2UseZFill` clone (`0x707f7d`): while the alpha is in
    /// `(0, 1)` a z-writing, colour-masked child draws first, so the model blends as one layer.
    /// `None` where the material disables z-write or z-test, the reference's own gate.
    pub zfill: Option<Handle<WowModelMaterial>>,
}

impl FadeMaterials {
    /// The law's blend twin while `feathering`, its steady material otherwise; the one place the
    /// two compose, so every fade writer agrees whichever runs last.
    pub fn material_for<'a>(
        &'a self,
        lit: Option<&'a crate::interior::InteriorLit>,
        feathering: bool,
    ) -> &'a Handle<WowModelMaterial> {
        match (feathering, lit) {
            (true, Some(l)) if l.is_bake() => self.bake_blend.as_ref().unwrap_or(&self.blend),
            (true, _) => &self.blend,
            (false, Some(l)) => l.steady_material(),
            (false, None) => &self.cutout,
        }
    }
}

/// The materials a spawning batch fades through. `blend: None` means it cannot feather (a
/// WMO-display batch), the one reason to spawn opaque during the unit's ramp. A multiply batch
/// (Mod, Mod2x) passes its steady self: its blend reads no alpha, so `wow_model.wgsl` lerps its
/// colour toward the blend identity by the tag alpha, as the reference's preset 5 does.
pub struct FadeSet<'a> {
    pub steady: &'a Handle<WowModelMaterial>,
    pub blend: Option<&'a Handle<WowModelMaterial>>,
    pub bake_blend: Option<&'a Handle<WowModelMaterial>>,
    pub zfill: Option<&'a Handle<WowModelMaterial>>,
}

/// One batch's appear-fade membership, which every batch spawn (billboard cards included) resolves
/// and dresses through: the reference draws every batch of a model through one instance alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PartFade {
    /// No ramp to join, or the batch cannot feather: open opaque.
    Steady,
    Pending(f32),
    Live(f32),
}

impl PartFade {
    /// Fold the unit's join decision with the batch's own fade-capability.
    pub fn resolve(joined: JoinedFade, set: &FadeSet<'_>) -> Self {
        match (joined, set.blend) {
            (_, None) | (JoinedFade::Steady, _) => Self::Steady,
            (JoinedFade::Pending { since }, _) => Self::Pending(since),
            (JoinedFade::Live { started }, _) => Self::Live(started),
        }
    }

    /// The material and `MeshTag` alpha the spawn opens on, so no part flashes before its first
    /// [`apply_render_fade`].
    pub fn seed(self, set: &FadeSet<'_>, now: f32) -> (Handle<WowModelMaterial>, f32) {
        match self {
            Self::Steady => (set.steady.clone(), 1.0),
            Self::Pending(_) => (set.blend.cloned().unwrap_or_default(), 0.0),
            Self::Live(started) => (
                set.blend.cloned().unwrap_or_default(),
                fade_alpha(0.0, 1.0, (now - started) / APPEAR_FADE_SECS),
            ),
        }
    }

    /// Insert [`FadeMaterials`] on any batch that can feather, whether or not it joins a ramp (the
    /// teardown and zoom feather use it too), and arm the join; returns whether it armed.
    pub fn dress(self, child: &mut bevy::ecs::system::EntityCommands, set: &FadeSet<'_>) -> bool {
        if let Some(blend) = set.blend {
            child.insert(FadeMaterials {
                cutout: set.steady.clone(),
                blend: blend.clone(),
                bake_blend: set.bake_blend.cloned(),
                zfill: set.zfill.cloned(),
            });
        }
        match self {
            Self::Steady => false,
            Self::Pending(since) => {
                child.insert(PendingAppearFade { since });
                true
            }
            Self::Live(started) => {
                child.insert(RenderFade {
                    started,
                    duration: APPEAR_FADE_SECS,
                    from: 0.0,
                    to: 1.0,
                    curve: FadeCurve::Cubic,
                });
                true
            }
        }
    }
}

/// A streamed object gone away (out of range, destroyed, or released from its despawn pin), which
/// [`apply_despawn_fade`] fades out and then despawns; `started < 0` is not yet armed. In the
/// reference `SMSG_DESTROY_OBJECT` (`0x4674a0`) and `SMSG_UPDATE_OBJECT`'s out-of-range destroy
/// (`0x465ec0`, run off the leading-block check `0x4651e1`, calling at `0x465f4f`/`0x465fa6`; the
/// type-4/5 block arm `0x465fd0` only discards its guids) reach the object destroy `0x464920`,
/// whose vtable slot 1 (base `0x6145e0`) hands the model to the `SWModelFadeout` scheduler
/// (`0x672df0`): the object is freed at once and its orphaned model fades out.
#[derive(Component)]
pub struct DespawnFade {
    pub(crate) started: f32,
}

impl Default for DespawnFade {
    fn default() -> Self {
        Self { started: -1.0 }
    }
}

impl DespawnFade {
    /// The instant this fade-out armed, or `None` while `started` is still the negative sentinel.
    pub fn armed(&self) -> Option<f32> {
        (self.started >= 0.0).then_some(self.started)
    }
}

/// Arm a teardown [`RenderFade`] on every fadeable part in the tree (held items hang under joints,
/// [`crate::entities::BoneAttach`]) and on the billboard cards following it, then despawn after
/// [`APPEAR_FADE_SECS`]; with nothing fadeable, despawn at once.
pub(crate) fn apply_despawn_fade(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut DespawnFade)>,
    children_of: Query<&Children>,
    // The tag holds the alpha the teardown starts from. `Option`: an untagged part is opaque, not
    // unfadeable.
    fm: Query<Option<&MeshTag>, With<FadeMaterials>>,
    cards: Query<(Entity, &crate::billboard::BillboardCard, Option<&MeshTag>), With<FadeMaterials>>,
) {
    let now = time.elapsed_secs();
    for (parent, mut df) in &mut q {
        if df.started < 0.0 {
            let mut any = false;
            let mut walked = bevy::ecs::entity::EntityHashSet::default();
            arm_despawn_descendants(
                parent,
                now,
                &mut commands,
                &children_of,
                &fm,
                &mut any,
                &mut walked,
            );
            for (card, follow, tag) in &cards {
                if follow.follows().is_some_and(|a| walked.contains(&a)) {
                    let start = start_alpha(tag);
                    if start >= TEARDOWN_MIN_ALPHA {
                        any = true;
                        arm_fade_out(card, now, start, &mut commands);
                    }
                }
            }
            if any {
                df.started = now;
            } else {
                commands.entity(parent).try_despawn();
            }
        } else if now - df.started >= APPEAR_FADE_SECS {
            commands.entity(parent).try_despawn();
        }
    }
}

/// Arm one part's teardown from the alpha it shows, as the scheduler takes the live `obj+0xf4`: a
/// model torn down mid-appear ramps down from where it stood.
fn arm_fade_out(entity: Entity, now: f32, start_alpha: f32, commands: &mut Commands) {
    commands
        .entity(entity)
        .try_insert(RenderFade {
            started: now,
            duration: APPEAR_FADE_SECS,
            from: start_alpha,
            to: 0.0,
            curve: FadeCurve::Smoothstep,
        })
        .try_remove::<PendingAppearFade>();
}

/// The alpha a part shows, the teardown's `startAlpha`; no tag is opaque, as in the shader.
fn start_alpha(tag: Option<&MeshTag>) -> f32 {
    tag.map_or(1.0, |t| crate::mesh_tag::alpha_of(t.0))
}

/// Arm every [`FadeMaterials`] part at or under `entity`, recording each visited entity in
/// `walked` for the caller's billboard-card match.
fn arm_despawn_descendants(
    entity: Entity,
    now: f32,
    commands: &mut Commands,
    children_of: &Query<&Children>,
    fm: &Query<Option<&MeshTag>, With<FadeMaterials>>,
    any: &mut bool,
    walked: &mut bevy::ecs::entity::EntityHashSet,
) {
    walked.insert(entity);
    // The scheduler's skip gate (`0x672e21`): a part below it gets no ramp, and an object with no
    // part above it is unlinked at once (`0x671ac0`).
    if let Ok(tag) = fm.get(entity) {
        let start = start_alpha(tag);
        if start >= TEARDOWN_MIN_ALPHA {
            *any = true;
            arm_fade_out(entity, now, start, commands);
        }
    }
    if let Ok(children) = children_of.get(entity) {
        for &child in children {
            arm_despawn_descendants(child, now, commands, children_of, fm, any, walked);
        }
    }
}

/// The M2 render-alpha systems, registered apart from the entity streamer so a world with no game
/// in it (the world viewer) runs them too.
pub fn plugin(app: &mut App) {
    // No ordering against the interior classifier or the doodad fade: a live fade owns the channel.
    app.add_systems(
        Update,
        (
            arm_appear_fade,
            apply_despawn_fade,
            apply_render_fade,
            retire_unit_appear_fade,
        )
            .chain(),
    )
    // After every `Update` fade writer and the self feather, before the effect sims that read it.
    .add_systems(
        PostUpdate,
        publish_model_alpha.before(crate::billboard::BillboardPlace),
    );
}

/// The composition walk over a tree with no published alpha, such as a booth bake.
#[cfg(test)]
mod chain_tests {
    use super::*;

    fn composed(app: &mut App, at: Entity) -> f32 {
        let mut sys = bevy::ecs::system::SystemState::<ModelAlphas>::new(app.world_mut());
        let alphas = sys.get(app.world());
        alphas.get(at)
    }

    #[test]
    fn an_uncomposed_tree_reads_its_declared_fade_and_children_compose_onto_it() {
        let mut app = App::new();
        let parent = app.world_mut().spawn(ModelFade(0.5)).id();
        let child = app
            .world_mut()
            .spawn((ModelFade(0.5), ParentModel(parent)))
            .id();
        let plain = app.world_mut().spawn(ParentModel(parent)).id();
        assert_eq!(composed(&mut app, parent), 0.5);
        assert_eq!(composed(&mut app, child), 0.25, "0.5 × 0.5, the recursion");
        assert_eq!(
            composed(&mut app, plain),
            0.5,
            "a child declaring nothing is its parent's"
        );
    }

    /// Reading both would apply the declared fade twice.
    #[test]
    fn a_published_alpha_beats_the_declaration_it_was_computed_from() {
        let mut app = App::new();
        let unit = app
            .world_mut()
            .spawn((ModelFade(0.5), ModelAlpha(0.2)))
            .id();
        assert_eq!(composed(&mut app, unit), 0.2);
    }

    #[test]
    fn an_entity_with_neither_is_opaque() {
        let mut app = App::new();
        let bare = app.world_mut().spawn_empty().id();
        assert_eq!(composed(&mut app, bare), 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A weapon glow is two links from its wearer: glow, item, wearer.
    #[test]
    fn an_attached_models_alpha_composes_through_the_whole_chain() {
        use bevy::ecs::system::SystemState;

        let mut world = World::new();
        let wearer = world.spawn(ModelAlpha(0.25)).id();
        let item = world.spawn(ParentModel(wearer)).id();
        let glow = world.spawn(ParentModel(item)).id();
        let dimmed = world.spawn((ModelAlpha(0.5), ParentModel(wearer))).id();
        let loose = world.spawn_empty().id();
        let gone = world.spawn(ParentModel(wearer)).id();
        world.despawn(gone);

        let mut state: SystemState<ModelAlphas> = SystemState::new(&mut world);
        let alphas = state.get(&world);
        assert_eq!(alphas.get(wearer), 0.25, "the unit's own");
        assert_eq!(alphas.get(item), 0.25, "an item takes its wearer's");
        assert_eq!(alphas.get(glow), 0.25, "…and its glow takes the item's");
        assert_eq!(alphas.get(dimmed), 0.125, "own × parent's — a product");
        assert_eq!(alphas.get(loose), 1.0, "chained to nothing ⇒ opaque");
        assert_eq!(
            alphas.get(gone),
            1.0,
            "a despawned instance never gates a draw"
        );
    }

    #[test]
    fn a_looping_chain_terminates() {
        use bevy::ecs::system::SystemState;

        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn(ParentModel(a)).id();
        world.entity_mut(a).insert(ParentModel(b));

        let mut state: SystemState<ModelAlphas> = SystemState::new(&mut world);
        assert_eq!(state.get(&world).get(a), 1.0);
    }

    /// `SystemState::apply` after a manual despawn recreates a wire destroy landing between the
    /// system's query and its commands.
    #[test]
    #[allow(clippy::type_complexity)] // the SystemState tuple is the system's own signature
    fn arm_appear_fade_tolerates_a_same_frame_wire_despawn() {
        use bevy::ecs::system::SystemState;

        let mut world = World::new();
        world.init_resource::<Time>();
        // The default viewer covers nothing, so every pending fade arms this frame.
        world.init_resource::<crate::view::Viewer>();
        let doomed = world.spawn(PendingAppearFade { since: 0.0 }).id();

        let mut state: SystemState<(
            Res<Time>,
            Res<crate::view::Viewer>,
            Commands,
            Query<(
                Entity,
                &PendingAppearFade,
                Has<crate::billboard::BillboardCard>,
            )>,
            Query<&mut UnitAppearFade>,
        )> = SystemState::new(&mut world);
        let (time, viewer, commands, q, units) = state.get_mut(&mut world);
        arm_appear_fade(time, viewer, commands, q, units);

        world.despawn(doomed);
        state.apply(&mut world); // would panic without the `try_*` contract
        assert!(world.get_entity(doomed).is_err(), "stays despawned");
    }

    #[test]
    fn the_material_rule_follows_the_law_not_the_arm_instant() {
        use crate::interior::{InteriorKind, InteriorLit};

        let cutout: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("c0000000-0000-4000-8000-000000000001");
        let blend: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("b1000000-0000-4000-8000-000000000002");
        let bake: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("ba000000-0000-4000-8000-000000000003");
        let bake_blend: Handle<WowModelMaterial> =
            bevy::asset::uuid_handle!("bb000000-0000-4000-8000-000000000004");
        let fm = FadeMaterials {
            cutout: cutout.clone(),
            blend: blend.clone(),
            bake_blend: Some(bake_blend.clone()),
            zfill: None,
        };

        // Unclassified (a WMO-display part): the exterior pair.
        assert_eq!(*fm.material_for(None, true), blend);
        assert_eq!(*fm.material_for(None, false), cutout);

        // Bake-capable but not yet resolved indoors, as a part spawns: the exterior pair.
        let kind = InteriorKind::Bake {
            material: bake.clone(),
            center: Vec3::ZERO,
        };
        let unresolved = InteriorLit::new(kind.clone(), cutout.clone());
        assert_eq!(*fm.material_for(Some(&unresolved), true), blend);
        assert_eq!(*fm.material_for(Some(&unresolved), false), cutout);

        // The law flips to the room's bake mid-ramp: feathered and settled on the bake variant.
        let lit = InteriorLit::applied_bake_for_test(kind, cutout.clone());
        assert_eq!(*fm.material_for(Some(&lit), true), bake_blend);
        assert_eq!(*fm.material_for(Some(&lit), false), bake);

        // No bake blend twin: the exterior blend, rather than no feather.
        let no_twin = FadeMaterials {
            bake_blend: None,
            zfill: None,
            ..fm.clone()
        };
        assert_eq!(*no_twin.material_for(Some(&lit), true), blend);
    }

    #[test]
    fn trees_and_buildings_never_fade() {
        assert_eq!(doodad_fade_alpha(7.01, 0.0), 1.0);
        assert_eq!(doodad_fade_alpha(20.0, 500.0), 1.0);
        assert_eq!(doodad_fade_alpha(7.0001, 195.0), 1.0);
    }

    /// A yard past each edge (the retained pass's hysteresis) the alpha is exactly 1 or 0.
    #[test]
    fn fade_band_edges_agree_with_the_alpha_they_summarize() {
        for radius in [0.0, 0.3, 0.5, 1.7, 2.5, 4.0, 7.0] {
            let (near, far) = fade_band(radius).expect("faders carry a band");
            assert_eq!(doodad_fade_alpha(radius, near - 1.0), 1.0, "r={radius}");
            assert_eq!(doodad_fade_alpha(radius, far + 1.0), 0.0, "r={radius}");
            let mid = doodad_fade_alpha(radius, (near + far) / 2.0);
            assert!(mid > 0.0 && mid < 1.0, "r={radius} mid={mid}");
        }
        assert_eq!(fade_band(7.01), None, "never-faders have no band");
    }

    #[test]
    fn small_props_fade_40_to_50() {
        // Radius 0, so `d` is the centre distance.
        let r = 0.0;
        assert_eq!(doodad_fade_alpha(r, 39.0), 1.0);
        assert_eq!(doodad_fade_alpha(r, 40.0), 1.0);
        assert!((doodad_fade_alpha(r, 45.0) - 0.5).abs() < 1e-6);
        assert_eq!(doodad_fade_alpha(r, 50.0), 0.0);
        assert_eq!(doodad_fade_alpha(r, 60.0), 0.0);
    }

    #[test]
    fn radius_offsets_the_band() {
        // A 0.5-yd prop reaches the band start (`d` = 40) at centre distance 40.5.
        assert_eq!(doodad_fade_alpha(0.5, 40.5), 1.0);
        assert!((doodad_fade_alpha(0.5, 45.5) - 0.5).abs() < 1e-6);
        assert_eq!(doodad_fade_alpha(0.5, 50.5), 0.0);
    }

    #[test]
    fn mid_bucket_100_to_125() {
        let r = 2.5;
        assert_eq!(doodad_fade_alpha(r, 100.0 + r), 1.0);
        assert!((doodad_fade_alpha(r, 112.5 + r) - 0.5).abs() < 1e-6);
        assert_eq!(doodad_fade_alpha(r, 125.0 + r), 0.0);
    }

    #[test]
    fn large_bucket_150_to_200() {
        let r = 5.0;
        assert_eq!(doodad_fade_alpha(r, 150.0 + r), 1.0);
        assert!((doodad_fade_alpha(r, 175.0 + r) - 0.5).abs() < 1e-6);
        assert_eq!(doodad_fade_alpha(r, 200.0 + r), 0.0);
    }

    #[test]
    fn monotonic_bigger_fades_farther() {
        // At 120 yd the small band (40 to 50) is long past and the large (150 to 200) not reached.
        let small = doodad_fade_alpha(0.3, 120.0);
        let large = doodad_fade_alpha(6.0, 120.0);
        assert_eq!(small, 0.0);
        assert_eq!(large, 1.0);
        assert!(large >= small);
    }

    #[test]
    fn appear_fade_is_cubic_0_to_1() {
        assert_eq!(fade_alpha(0.0, 1.0, 0.0), 0.0);
        assert!((fade_alpha(0.0, 1.0, 0.5) - 0.125).abs() < 1e-6); // t³
        assert_eq!(fade_alpha(0.0, 1.0, 1.0), 1.0);
        assert_eq!(fade_alpha(0.0, 1.0, -1.0), 0.0);
        assert_eq!(fade_alpha(0.0, 1.0, 2.0), 1.0);
    }

    #[test]
    fn despawn_fade_eases_to_0() {
        // The cubic run downward, `1 − t³`; the despawn itself runs `teardown_fade_alpha`.
        assert_eq!(fade_alpha(1.0, 0.0, 0.0), 1.0);
        assert!((fade_alpha(1.0, 0.0, 0.5) - 0.875).abs() < 1e-6);
        assert_eq!(fade_alpha(1.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn self_fade_hidden_at_first_person() {
        let nc = 1.0;
        assert_eq!(self_model_fade_alpha(nc, nc, SELF_FADE_WINDOW), 0.0);
        assert_eq!(self_model_fade_alpha(0.0, nc, SELF_FADE_WINDOW), 0.0);
        assert_eq!(
            self_model_fade_alpha(nc + SELF_FADE_HIDE, nc, SELF_FADE_WINDOW),
            0.0
        );
    }

    #[test]
    fn self_fade_opaque_when_zoomed_out() {
        let nc = 1.0;
        assert_eq!(
            self_model_fade_alpha(nc + SELF_FADE_WINDOW, nc, SELF_FADE_WINDOW),
            1.0
        );
        assert_eq!(self_model_fade_alpha(100.0, nc, SELF_FADE_WINDOW), 1.0);
    }

    #[test]
    fn no_unit_marker_joins_steady() {
        assert_eq!(join_unit_appear_fade(None), JoinedFade::Steady);
    }

    #[test]
    fn pending_unit_joins_at_the_same_since() {
        let since = 3.25;
        assert_eq!(
            join_unit_appear_fade(Some(UnitAppearFade::Pending { since })),
            JoinedFade::Pending { since }
        );
    }

    #[test]
    fn live_unit_joins_at_the_same_started() {
        let started = 12.5;
        assert_eq!(
            join_unit_appear_fade(Some(UnitAppearFade::Live { started })),
            JoinedFade::Live { started }
        );
    }

    #[test]
    fn joining_mid_ramp_reproduces_the_original_curve() {
        let started = 1.4;
        let joined = join_unit_appear_fade(Some(UnitAppearFade::Live { started }));
        assert_eq!(joined, JoinedFade::Live { started });
        for now in [1.4f32, 1.7, 2.0, 2.9, 3.4] {
            let body_alpha = fade_alpha(0.0, 1.0, (now - started) / APPEAR_FADE_SECS);
            let JoinedFade::Live {
                started: joined_started,
            } = joined
            else {
                unreachable!()
            };
            let item_alpha = fade_alpha(0.0, 1.0, (now - joined_started) / APPEAR_FADE_SECS);
            assert_eq!(body_alpha, item_alpha);
        }
    }

    #[test]
    fn a_units_render_alpha_is_zero_until_it_is_shown_then_rides_the_same_cubic() {
        assert_eq!(
            model_render_alpha(
                10.0,
                Some(UnitAppearFade::Pending { since: 9.0 }),
                None,
                1.0,
                1.0
            ),
            0.0,
            "pending: the unit is not on screen, and neither are its effects"
        );
        let live = |now| {
            model_render_alpha(
                now,
                Some(UnitAppearFade::Live { started: 10.0 }),
                None,
                1.0,
                1.0,
            )
        };
        assert_eq!(live(10.0), 0.0, "the ramp starts at nothing");
        assert!(
            (live(11.0) - fade_alpha(0.0, 1.0, 0.5)).abs() < 1e-6,
            "mid-ramp is the mesh parts' own cubic, not a second curve"
        );
        assert_eq!(live(12.0), 1.0, "…and latches opaque at the duration");
        assert_eq!(
            model_render_alpha(10.0, None, None, 1.0, 1.0),
            1.0,
            "no fade: opaque"
        );
    }

    #[test]
    fn the_render_alpha_multiplies_its_writers() {
        // First person: opaque by the appear ramp, but the feather is 0.
        assert_eq!(model_render_alpha(20.0, None, None, 0.0, 1.0), 0.0);
        assert!((model_render_alpha(20.0, None, None, 0.5, 1.0) - 0.5).abs() < 1e-6);
        // Streaming out: the teardown takes the same channel to 0.
        assert_eq!(model_render_alpha(10.0, None, Some(10.0), 1.0, 1.0), 1.0);
        assert_eq!(model_render_alpha(12.0, None, Some(10.0), 1.0, 1.0), 0.0);
        let both = model_render_alpha(
            11.0,
            Some(UnitAppearFade::Live { started: 10.0 }),
            Some(10.0),
            0.5,
            1.0,
        );
        assert!((0.0..=1.0).contains(&both) && both > 0.0);
    }

    /// Stealth's aura alpha, 0.3.
    #[test]
    fn the_aura_alpha_is_a_factor_of_the_effect_render_alpha() {
        assert!((model_render_alpha(20.0, None, None, 1.0, 0.3) - 0.3).abs() < 1e-6);
        assert!((model_render_alpha(20.0, None, None, 0.5, 0.3) - 0.15).abs() < 1e-6);
        assert_eq!(model_render_alpha(20.0, None, Some(20.0), 1.0, 0.3), 0.3);
    }

    #[test]
    fn unit_fade_retires_after_the_appear_duration() {
        // `retire_unit_appear_fade`'s condition, a pure time check.
        let started = 5.0;
        let almost_done = started + APPEAR_FADE_SECS - 0.001;
        let done = started + APPEAR_FADE_SECS;
        assert!(almost_done - started < APPEAR_FADE_SECS);
        assert!(done - started >= APPEAR_FADE_SECS);
    }

    #[test]
    fn self_fade_cosine_ramp_midpoint() {
        // Mid-window the cosine smoothstep is 0.5 (cos(π/2) = 0).
        let nc = 1.0;
        let mid = nc + SELF_FADE_WINDOW / 2.0;
        assert!((self_model_fade_alpha(mid, nc, SELF_FADE_WINDOW) - 0.5).abs() < 1e-6);
        let near = self_model_fade_alpha(nc + 0.4, nc, SELF_FADE_WINDOW);
        let far = self_model_fade_alpha(nc + 1.4, nc, SELF_FADE_WINDOW);
        assert!(near < far);
        assert!((0.0..=1.0).contains(&near) && (0.0..=1.0).contains(&far));
    }

    /// A billboard card is a world root matched by its follow-anchor; a stranger's is left alone.
    #[test]
    fn a_despawn_fade_reaches_the_models_billboard_cards() {
        let fm = || FadeMaterials {
            cutout: Handle::default(),
            blend: Handle::default(),
            bake_blend: None,
            zfill: None,
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn((Transform::default(), fm())).id();
        // The held-item shape: a joint, an attach root under it, a card following that root.
        let joint = app.world_mut().spawn(Transform::default()).id();
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().entity_mut(unit).add_child(joint);
        app.world_mut().entity_mut(joint).add_child(root);
        let info = benilla_assets::BillboardInfo {
            pivot: Vec3::ZERO,
            bone: 1,
            kind: benilla_formats::BillboardKind::Spherical,
            scale_anim: None,
            seq_translations: Vec::new(),
        };
        let mine = app
            .world_mut()
            .spawn((
                crate::billboard::BillboardCard::following(&info, root),
                fm(),
            ))
            .id();
        let stranger = app.world_mut().spawn(Transform::default()).id();
        let theirs = app
            .world_mut()
            .spawn((
                crate::billboard::BillboardCard::following(&info, stranger),
                fm(),
            ))
            .id();
        app.world_mut()
            .entity_mut(unit)
            .insert(DespawnFade::default());
        app.add_systems(Update, apply_despawn_fade);
        app.update();

        let out = |e: Entity| {
            app.world()
                .entity(e)
                .get::<RenderFade>()
                .map(|f| (f.from, f.to))
        };
        assert_eq!(
            out(unit),
            Some((1.0, 0.0)),
            "the body arms, as it always did"
        );
        assert_eq!(out(mine), Some((1.0, 0.0)), "…and so does its own card");
        assert_eq!(out(theirs), None, "a stranger's card is left alone");
    }
}

#[cfg(test)]
mod inversion_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn a_declared_fade_and_the_self_feather_both_reach_the_composed_alpha() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(crate::view::Viewer {
            self_fade: 0.5,
            ..default()
        });

        let plain = app
            .world_mut()
            .spawn((world_unit_for_test(), ModelFade(0.25)))
            .id();
        let me = app
            .world_mut()
            .spawn((world_unit_for_test(), crate::world_unit::ViewerUnit))
            .id();
        let both = app
            .world_mut()
            .spawn((
                world_unit_for_test(),
                crate::world_unit::ViewerUnit,
                ModelFade(0.5),
            ))
            .id();

        app.world_mut()
            .run_system_once(publish_model_alpha)
            .expect("publisher runs");

        let w = app.world();
        let got = |e| w.get::<ModelAlpha>(e).expect("published").0;
        assert!(
            (got(plain) - 0.25).abs() < 1e-6,
            "a declared fade reaches the alpha on any body"
        );
        assert!(
            (got(me) - 0.5).abs() < 1e-6,
            "the self feather applies to the viewer's body and nothing else"
        );
        assert!(
            (got(both) - 0.25).abs() < 1e-6,
            "…and the two multiply (0.5 feather x 0.5 declared) rather than one winning"
        );
    }

    fn world_unit_for_test() -> crate::world_unit::WorldUnit {
        crate::world_unit::WorldUnit {
            wades: true,
            scale: 1.0,
            height: 2.0,
            bound: None,
        }
    }
}
