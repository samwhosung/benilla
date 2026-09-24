//! The celestial layer over the sky dome: the sun and white moon (each a disc and an additive
//! glare), moon02 and the `Stars.m2` stars. The reference's setup `0x6d1ba0` builds the six
//! bodies, the builder `0x6d3b80` places each at `cam + 12·dir`, `CSky::Render` (`0x6d4940`) draws
//! stars, sun, white moon and moon02 in a far depth slice without depth writes, and every disc
//! takes the horizon clip and fade `0x6d1960`. One broadcast celestial diffuse (`[0xce9c2c]`,
//! LightIntBand sub-9, alpha 0xFF) tints every disc and glare but moon02, which has no colour
//! writer; the white moon's teal rim is the dome's night bands (sub-3..6) through its feathered
//! edge. Every body takes the sky's far-depth pin. The bodies are placed in plain world space, so
//! they project through our camera's 45° fovy, where the reference's is 44.1° at 16:9.

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

/// Which sun sprite: the disc or its additive lens-flare glare.
#[derive(Clone, Copy)]
enum SunPart {
    /// The `sunCenter.blp` disc (alpha-blended, band-tinted, horizon clip+fade).
    Disc,
    /// The additive lens flare (`sunGlare.blp`, `0x6cf490`): view-lerped scale 3→20, alpha 0.5→1.
    Glare,
}

#[derive(Component)]
struct SunSprite {
    part: SunPart,
}

/// Which moon sprite: the white moon's disc, its additive glare ring, or moon02.
#[derive(Clone, Copy)]
enum MoonPart {
    /// The `moon.blp` disc (alpha-blended, band-tinted, horizon clip+fade).
    Disc,
    /// The additive `moonglare.blp` ring: scale `2.0 ×` the size curve, view-lerped 0.1→1.
    Glare,
    /// The third disc (`moon02.blp`), drawn every frame on its own phase-precessed bearing
    /// (az 135–165°) at colour alpha 0, as its colour has no writer; the weather seed surfaces it.
    Moon02,
}

#[derive(Component)]
struct MoonSprite {
    part: MoonPart,
}

/// One `Stars.m2` patch or the fallback, faded by the star curve's model-global byte `[stars+0xb]`
/// (`0x6d1b50`).
#[derive(Component)]
struct StarDome {
    /// The patch's authored transparency weight (`Stars.m2` keys 1.0 to 0.25 over its seven
    /// batches, `A = colorAlpha × weight`), under the star-curve alpha; 1.0 for the fallback.
    weight: f32,
}

/// The celestial layer: spawns the sun, moons and stars at startup and places them each frame.
pub(crate) struct SunPlugin;

impl Plugin for SunPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<CelestialMaterial>::default())
            .add_plugins(MaterialPlugin::<StarMaterial>::default())
            .add_systems(Startup, setup_sun.after(AssetSet::Open))
            // After propagation, so the follows place from this frame's camera: a frame of lag is
            // ~1% of the glare's 12-unit distance, a visible swim.
            .add_systems(
                PostUpdate,
                (follow_sun, follow_moons, follow_stars).in_set(crate::billboard::BillboardPlace),
            )
            // After both resolves, so the skybox, the murk and these bodies agree within a frame.
            .add_systems(
                Update,
                apply_celestial_visibility
                    .after(crate::skybox::SkyboxResolve)
                    .after(crate::liquid::SubmersionVerdict),
            );
    }
}

/// Hide the sky pass's elements while a WMO skybox owns the sky or the eye is submerged.
/// `CSky::Render` gates stars, sun disc, white moon, moon02, gradient and cloud dome on one
/// boolean: `0x6d49cd` sets it, a skybox slot whose weight exceeds `[0x808aac]` = 0.99 clears it
/// (`0x6d49fb`/`0x6d4a2e`), and `0x6d4a3b` then skips all six; through the 4 s crossfade they
/// draw under the skybox. A submerged eye skips `CSky::Render` whole (`0x6812a4`), or the discs
/// would draw black under the murk. The glares draw on their own path
/// (`0x483740 → 0x6d48c0 → 0x7e57e0`), which neither gate reaches, and fade underwater
/// themselves; the gradient and cloud domes ([`crate::sky`], [`crate::clouds`]) gate themselves.
#[allow(clippy::type_complexity)]
fn apply_celestial_visibility(
    skybox: Res<crate::skybox::SkyboxWeight>,
    underwater: Res<crate::liquid::Underwater>,
    mut suns: Query<(&SunSprite, &mut Visibility), Without<MoonSprite>>,
    mut moons: Query<(&MoonSprite, &mut Visibility), Without<SunSprite>>,
    mut stars: Query<&mut Visibility, (With<StarDome>, Without<SunSprite>, Without<MoonSprite>)>,
) {
    let suppressed = skybox.replaces_celestial() || underwater.0.any();
    // Lifting the gate restores the spawn default, `Inherited`, never `Visible`.
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
