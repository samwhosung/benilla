//! Celestial sprites — the **sun** (disc + glare halo), the **white moon** (disc + glare), and the
//! night-sky **stars**, drawn over the sky dome. Byte-pinned end to end (decision 0485): setup
//! `0x6d1ba0` builds six bodies (sun/moon glares, three discs, stars), the
//! builder `0x6d3b80` places each on a camera-centred sphere of radius 12 (`pos = cam + 12·dir`, world
//! space — no local→world rotation), `CSky::Render 0x6d4940` draws stars → sun → white moon → moon02
//! at a far depth slice with depth-write off (terrain occludes), and every disc routes through the
//! shared horizon clip+fade `0x6d1960`. This subsystem's plugin is [`SunPlugin`] (kept that name though
//! it covers every celestial body). Submodules, each its own concern:
//! - [`materials`] — the disc clip+fade [`materials::CelestialMaterial`] and the gamma-correct star material.
//! - [`mesh`] — pure quad / star-field / sprite-texture builders.
//! - [`setup`] — the startup spawn of every celestial entity.
//! - [`follow`] — the per-frame positioning, tinting, and fades.
//!
//! **The bodies** (all textures the real client's, from the patch chain):
//! - **Sun disc** — `sunCenter.blp`, plain alpha blend, a 1.0-unit quad at radius 12 (angular diameter
//!   `2·atan(0.5/12) = 4.77°`), size × the day curve (2× at the dawn/dusk horizon → 1× midday).
//! - **Sun glare** — `sunGlare.blp` (a big dark radial glow with authored star rays), additive,
//!   co-located; a **view-lerped lens flare** (`0x6cf490`): quad scale 3→20 world units as the view
//!   axis swings onto the sun (`f = saturate((cosθ − 0.7)/0.3)`), intensity `lerp(0.5, 1, f)` × the
//!   slewed dnCurve envelope — a DAY flare, full 07:30→19:30, gone by 21:00 (decision 0508).
//! - **White moon** — `moon.blp`, alpha blend, base ×1.75, up at night (azimuth 45°, the sun's bearing).
//! - **Moon glare** — `moonglare.blp` (a soft ring), additive, co-located; quad scale = `2.0 × the
//!   moon size curve` (≈1.14× the disc, both endpoints overwritten per frame), intensity
//!   `lerp(0.1, 1, f)` on the same view lerp × the slewed dnCurve envelope — a DEEP-NIGHT halo,
//!   nothing until 22:45, full only near midnight (decision 0508).
//! - **Stars** — the real `Stars.m2` patches, camera-anchored, global alpha = the star curve, each
//!   patch × its authored transparency weight.
//!
//! **The tint law** (the big 0485 correction): the discs' and glares' RGB is NOT hardcoded — the real
//! client broadcasts one DayNight **celestial diffuse** colour (`[0xce9c2c]` = LightIntBand sub-9,
//! resolved per zone + time; alpha forced 0xFF) into the sun disc, sun glare, white-moon disc, and
//! moon glare every frame. Warm cream at night, orange at dawn/dusk — the follow systems rewrite the
//! material tints per frame. The engine's third disc (`moon02.blp`) has NO colour writer in the
//! binary — it draws vertex-black at azimuth 135–165° on a phase-precessed schedule and can never
//! read as a second moon. The director's Westfall observation (one moon, a TEAL rim) is reproduced
//! NOT by any cool tint — the moon + its glare are warm — but by the dome's teal night bands
//! (sub-3..6) alpha-blending through the moon disc's feathered edge + horizon fade.
//!
//! The DISCS are pinned at their body directions (`WowLighting.{celestial_dir, moon_dir_white}`) just
//! inside the far plane, so terrain occludes them like the reference's far-depth-slice trick. The GLARE
//! quads sit on the reference's own **near sphere** (`cam + 12·dir` — a far-placed quad at the byte-law
//! flare size pierced the sky dome and the depth test cut a giant faceted halo edge; decision 0500);
//! their envelope, not the depth buffer, hides an occluded flare (the [`follow`] terrain/interior
//! visibility gate — the occlusion-query stand-in). Our camera fovy (45°) ≈ the reference's (44.1°), so
//! plain world placement reproduces its projection in both axes.

use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;

use benilla_assets::AssetSet;

mod follow;
mod materials;
mod mesh;
mod setup;

use follow::{follow_moons, follow_stars, follow_sun};
pub use materials::{CelestialMaterial, StarMaterial};
use setup::setup_sun;

/// A sun billboard sprite — the disc or its additive lens-flare glare. `SunPart` selects which.
#[derive(Clone, Copy)]
enum SunPart {
    /// The `sunCenter.blp` disc (alpha-blended, band-tinted, horizon clip+fade).
    Disc,
    /// The additive lens flare (`sunGlare.blp`): view-lerped scale 3→20, intensity 0.5→1.
    Glare,
}

#[derive(Component)]
struct SunSprite {
    part: SunPart,
}

/// A moon billboard sprite — the white moon's disc, its additive glare ring, or the black moon02
/// disc. `MoonPart` selects which.
#[derive(Clone, Copy)]
enum MoonPart {
    /// The `moon.blp` disc (alpha-blended, band-tinted, horizon clip+fade).
    Disc,
    /// The additive corona (`moonglare.blp`, a soft warm ring): scale `2.0 ×` the moon size curve,
    /// intensity view-lerped 0.1→1.
    Glare,
    /// The engine's third disc (`moon02.blp`) — drawn every frame, vertex-BLACK (its colour field
    /// has no writer in the binary), on its own phase-precessed bearing (az 135–165°). Never a
    /// visible second moon; it faithfully darkens the stars behind it.
    Moon02,
}

#[derive(Component)]
struct MoonSprite {
    part: MoonPart,
}

/// A night-sky **star** mesh — one tag per `Stars.m2` patch (or the procedural fallback). Camera-anchored
/// over the gradient dome, unlit + alpha-blended; its material's global alpha is driven each frame by the
/// verified star curve (`WowLighting.star_alpha` — the model-global fade `[stars+0xb]/255`, byte =
/// `trunc(curve·254+1)`, draw skipped below 2; `0x6d1b50`, decision 0485). The faithful
/// geometry is the real `Stars.m2` (authored star positions/sizes + UVs into `Stars.blp`/`Stars2.blp`);
/// [`mesh::star_field_mesh`] is the assetless fallback. Built in [`setup::setup_sun`], anchored + faded in
/// [`follow::follow_stars`].
#[derive(Component)]
struct StarDome {
    /// The patch's **authored transparency weight** — `Stars.m2` keys six static weights (1.0 … 0.25)
    /// across its seven batches (`transparency_lookup` two-hop, the verified M2 alpha combine
    /// `A = colorAlpha × weight`), so star groups differ in brightness. Multiplied under the global
    /// star-curve alpha each frame; `1.0` for the procedural fallback.
    weight: f32,
}

/// Sun-sprite subsystem: spawns the disc + glow billboards (+ the two moons + the stars) at startup and pins
/// them to their bodies each frame (camera-facing, at each body's world direction just inside the far plane).
pub(crate) struct SunPlugin;

impl Plugin for SunPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<CelestialMaterial>::default())
            .add_plugins(MaterialPlugin::<StarMaterial>::default())
            .add_systems(Startup, setup_sun.after(AssetSet::Open))
            // Post-propagation camera-anchored placement (the BillboardPlace slot): the follows read
            // the camera's SAME-frame pose and write GlobalTransform directly. In plain Update they
            // read the last-frame camera — one frame of motion is ~1% of the glare quads' 12-unit
            // distance, a visible halo swim/size-pump while moving (decision 0504).
            .add_systems(
                PostUpdate,
                (follow_sun, follow_moons, follow_stars).in_set(crate::billboard::BillboardPlace),
            )
            // The WMO-skybox + submersion suppression, ordered after both resolves for the same
            // reason `crate::sky`'s dome gate is: backdrop and atmosphere must agree WITHIN a
            // frame, or a frame shows the painted sky with our stars still punched through it
            // (or the murk with the sun still up).
            .add_systems(
                Update,
                apply_celestial_visibility
                    .after(crate::skybox::SkyboxResolve)
                    .after(crate::liquid::SubmersionVerdict),
            );
    }
}

/// Hide the sky pass's own elements while a WMO skybox owns the sky ([`crate::skybox`]) — or while
/// the eye is **submerged**.
///
/// `CSky::Render` gates its six draws on **one shared boolean** — `0x6d49cd` sets it, a filled skybox
/// slot clears it (`0x6d49fb`/`0x6d4a2e`, once a slot's weight exceeds `[0x808aac]` = 0.99), and
/// `0x6d4a3b test edi,edi; je` then skips **stars, sun disc, white moon, moon02, gradient band and
/// cloud dome together**. There is no per-element gating, which is why this is one system and not
/// five. Live-confirmed: standing in King's Square the reference's whole `[0.975, 0.98]` slice is
/// three `count=12` draws — the skybox cube — and nothing else.
///
/// The gate is the slot's **weight**, never the resolve alone: through the body of the 4-second
/// crossfade (weight ≤ 0.99) the six elements keep drawing and the skybox alpha-blends over them
/// ([`crate::skybox::SkyboxWeight`]).
///
/// The **submerged eye** suppresses the same set one level up: the scene driver's `0x6812a4`
/// submerged test skips the whole `CSky::Render` call ("the surface is drawn identically from
/// below; what changes underwater is scene-wide atmosphere only"). Without it the discs keep
/// drawing under the murk, tinted by the underwater LightParams' celestial band — which is black
/// there, so the sun read as a black ball
/// from under water (director report, 2026-08-03).
///
/// **The GLARES are deliberately exempt.** They are not in this pass: the reference renders them
/// last, on their own path (`0x483740 → 0x6d48c0 → 0x7e57e0`), which the boolean never reaches — so
/// a sun flare still blooms over the painted sky. (Whether the glare path carries its own submersion
/// gate is not yet known; until it is, the flare keeps its envelope behaviour.) The
/// gradient band is [`crate::sky`]'s dome and the cloud dome is [`crate::clouds`]'s; each keeps its
/// own authority (decision 0025) reading these same resources.
#[allow(clippy::type_complexity)]
fn apply_celestial_visibility(
    skybox: Res<crate::skybox::SkyboxWeight>,
    underwater: Res<crate::liquid::Underwater>,
    mut suns: Query<(&SunSprite, &mut Visibility), Without<MoonSprite>>,
    mut moons: Query<(&MoonSprite, &mut Visibility), Without<SunSprite>>,
    mut stars: Query<&mut Visibility, (With<StarDome>, Without<SunSprite>, Without<MoonSprite>)>,
) {
    let suppressed = skybox.replaces_celestial() || underwater.0.any();
    // `Inherited`, not `Visible`: these sprites hang off the celestial rig and must keep deferring to
    // it when the skybox stands down, exactly as the cloud dome's gate does.
    let want = |in_sky_pass: bool| {
        if in_sky_pass && suppressed {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        }
    };
    let set = |vis: &mut Visibility, target: Visibility| {
        if *vis != target {
            *vis = target;
        }
    };
    for (sprite, mut vis) in &mut suns {
        set(&mut vis, want(matches!(sprite.part, SunPart::Disc)));
    }
    for (sprite, mut vis) in &mut moons {
        set(
            &mut vis,
            want(matches!(sprite.part, MoonPart::Disc | MoonPart::Moon02)),
        );
    }
    for mut vis in &mut stars {
        set(&mut vis, want(true));
    }
}
