//! Startup spawn of the celestial layer — the sun disc + glare, the white moon disc + its additive glare
//! ring, and the night-sky stars (the real `Stars.m2` patches, or the procedural fallback). One `Startup`
//! system ([`setup_sun`]); per-frame positioning + tinting live in [`super::follow`].

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::LockRecover;
use benilla_assets::WorldAssets;
use benilla_formats::load_m2_mesh;

use super::materials::{CelestialExt, CelestialMaterial, StarExt, StarMaterial, DISC_HORIZON_FADE};
use super::mesh::{quad_mesh, radial_sprite, star_field_mesh};
use super::{MoonPart, MoonSprite, StarDome, SunPart, SunSprite};
use crate::sky_order;

pub(super) fn setup_sun(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut disc_mats: ResMut<Assets<CelestialMaterial>>,
    mut star_mats: ResMut<Assets<StarMaterial>>,
    mut world_assets: Option<ResMut<WorldAssets>>,
) {
    let mesh = meshes.add(quad_mesh());
    // Every body sprite uses `CelestialMaterial` (celestial.wgsl — the gamma lane). The DISCS
    // (sun, white moon, moon02): unlit gamma-correct alpha-blend + the SHARED horizon clip+fade — the
    // real client routes all three through the same `0x6d1960` (clip to the horizon; the fade store
    // `clamp(2.5·height, 0, 1)` is CONDITIONAL on the near-horizon band, above which the disc keeps its
    // colour's own alpha `a_disc`; body skipped entirely below the horizon). The
    // GLARES: additive in GAMMA — the reference's SRC_ALPHA, ONE byte addition
    // (`0x7e5a16`), no horizon clip (their lens-flare envelope gates them). Every RGB
    // tint is the DayNight celestial diffuse band, rewritten per frame by the follow systems —
    // base_color here is only the frame-0 seed.
    let clip = |base: StandardMaterial, a_disc: f32| CelestialMaterial {
        base,
        extension: CelestialExt {
            fade: Vec4::new(DISC_HORIZON_FADE, 1.0, 0.0, a_disc),
            span: Vec4::new(1.0, 1.0, 0.0, 0.0), // frame-0 seed (elevated); per-frame: follow
        },
    };
    let glare = |base: StandardMaterial| CelestialMaterial {
        base,
        extension: CelestialExt {
            fade: Vec4::new(0.0, 1.0, 1.0, 0.0), // .z = 1: additive glare mode
            span: Vec4::ZERO,                    // unused in glare mode
        },
    };
    // Sun disc: the real `sunCenter.blp` (white core → soft yellow edge, feathered alpha), tinted by the
    // celestial band. Assetless dev falls back to a generated soft disc.
    let sun_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("Textures\\sunCenter.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(128, 0.55, 0.95)));
    let disc = disc_mats.add(clip(
        StandardMaterial {
            base_color: Color::WHITE, // per-frame: the celestial diffuse band (follow_sun)
            base_color_texture: Some(sun_tex),
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Premultiplied, // gamma-correct blend (celestial.wgsl) — like the reference
            depth_bias: sky_order::SUN_DISC_BIAS, // second sky draw — over stars, under the moons/clouds
            ..default()
        },
        1.0, // colour alpha 0xFF — the per-frame diffuse broadcast (0x6d2914)
    ));
    // Sun glare: the real `sunGlare.blp` — a big dark radial glow with authored star rays (dark texels ×
    // a huge view-lerped quad = the lens flare; the added light stays gentle per pixel). Gamma-additive
    // (glare mode); the tint (celestial band) and the intensity envelope ride base_color, rewritten per
    // frame by `follow_sun`. Assetless dev falls back to a generated glow.
    let glare_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("Textures\\sunGlare.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(128, 0.0, 1.0)));
    let glow = disc_mats.add(glare(StandardMaterial {
        base_color: Color::WHITE, // per-frame: band tint × the lens-flare envelope (follow_sun)
        base_color_texture: Some(glare_tex),
        unlit: true,
        cull_mode: None,
        alpha_mode: AlphaMode::Add,
        depth_bias: sky_order::GLARE_BIAS, // the frame's last render — over clouds and rain
        ..default()
    }));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(disc),
        Transform::default(),
        SunSprite {
            part: SunPart::Disc,
        },
    ));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(glow),
        Transform::default(),
        SunSprite {
            part: SunPart::Glare,
        },
    ));
    // The WHITE MOON — one disc, the real `moon.blp` (neutral grey-white, feathered alpha edge), FULLY
    // opaque (texture alpha only), tinted per frame by the same celestial band as the sun. The old
    // hand-built composite (a cyan `Moon02.blp` backing + a 0.55-alpha front disc + a hand-cyan glare)
    // is dead: it faked the reference's teal rim while the real mechanism is the band tint + the additive
    // glare ring + the feathered edge over the night sky. Assetless dev falls back to a
    // generated disc.
    let moon_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("textures\\moon.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(128, 0.7, 0.97)));
    let white_moon = disc_mats.add(clip(
        StandardMaterial {
            base_color: Color::WHITE, // per-frame: the celestial diffuse band (follow_moons)
            base_color_texture: Some(moon_tex),
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Premultiplied,
            depth_bias: sky_order::WHITE_MOON_BIAS, // third sky draw — over the sun where they cross
            ..default()
        },
        1.0, // colour alpha 0xFF — the per-frame diffuse broadcast (0x6d2914)
    ));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(white_moon),
        Transform::default(),
        MoonSprite {
            part: MoonPart::Disc,
        },
    ));
    // MOON02 — the engine's third disc: the real `moon02.blp` (a cyan moon texture) whose colour
    // dword (`[0xce98a4]`) has NO writer in the binary — RGB *and* alpha stay BSS-zero. The reference
    // issues its draw every frame, but the quad renders with vertex alpha 0, so the blender paints
    // NOTHING — the second moon is invisible in clear weather (the old "dark smudge that occludes
    // stars" reading is superseded; director-verified against the reference).
    // `a_disc = 0.0` gates the whole quad multiplicatively — including the near-horizon fade band,
    // where our per-fragment lane would otherwise render the reference's soft per-vertex crossing
    // wedge as a hard black bar. Kept spawned for the future weather seed
    // (`255·(1−bcc)` dims all three discs under cloud).
    let moon02_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("textures\\moon02.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(128, 0.7, 0.97)));
    let moon02 = disc_mats.add(clip(
        StandardMaterial {
            base_color: Color::BLACK, // the unwritten [0xce98a4] — BSS zero, forever
            base_color_texture: Some(moon02_tex),
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Premultiplied,
            depth_bias: sky_order::MOON02_BIAS, // fourth sky draw — the last disc
            ..default()
        },
        0.0, // colour ALPHA is unwritten too — the invisible second moon (`0xce98a4`)
    ));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(moon02),
        Transform::default(),
        MoonSprite {
            part: MoonPart::Moon02,
        },
    ));
    // The moon's additive glare — the real `moonglare.blp` ring, co-located with the disc, tinted WARM
    // by the same celestial band as everything else (the old hand-cyan tint faked the teal rim; the real
    // rim is the dome's teal night bands through the disc's feathered edge). Gamma-additive (glare
    // mode); tint × the lens-flare intensity envelope ride base_color, rewritten per frame by
    // `follow_moons`.
    let white_glare_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("textures\\moonglare.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(64, 0.0, 1.0)));
    let white_glare = disc_mats.add(glare(StandardMaterial {
        base_color: Color::WHITE, // per-frame: band tint × the envelope (follow_moons)
        base_color_texture: Some(white_glare_tex),
        unlit: true,
        cull_mode: None,
        alpha_mode: AlphaMode::Add,
        depth_bias: sky_order::GLARE_BIAS, // the frame's last render — over clouds and rain
        ..default()
    }));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(white_glare),
        Transform::default(),
        MoonSprite {
            part: MoonPart::Glare,
        },
    ));

    // Stars — load the **real `Environments\Stars\Stars.m2`** (7 textured patches covering an upper-
    // hemisphere cap, authored star positions/sizes + UVs into `Stars.blp`/`Stars2.blp` — so the dots map
    // at their authored scale, unlike a hand-tiled dome). Built as unlit + alpha-blended meshes, normalised
    // to a unit dome (`follow_stars` scales to the star distance + drives the global alpha). Without `./WoW`
    // (assetless dev) fall back to the procedural dot field.
    let star_subs = world_assets
        .as_mut()
        .and_then(|a| {
            load_m2_mesh(&mut a.chain.lock_recover(), "Environments\\Stars\\Stars.m2").ok()
        })
        .filter(|s| !s.is_empty());
    if let Some(subs) = star_subs {
        // Normalise by the global max radius → a unit dome (so `follow_stars` can scale it like the others).
        let radius = subs
            .iter()
            .flat_map(|s| s.positions.iter())
            .map(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt())
            .fold(0.0_f32, f32::max)
            .max(1e-3);
        // One unlit alpha material per PATCH (not per texture): each batch keys its own authored
        // transparency weight (`Stars.m2` weights 1.0 … 0.25 — dimmer and brighter star groups), which
        // `follow_stars` multiplies under the star-curve global alpha, so materials can't be shared
        // across patches with different weights. Base-colour alpha is driven per-frame.
        let tex_white = world_assets
            .as_mut()
            .and_then(|a| a.texture("Environments\\Stars\\Stars.blp", (true, true), &mut images));
        let tex_blue = world_assets
            .as_mut()
            .and_then(|a| a.texture("Environments\\Stars\\Stars2.blp", (true, true), &mut images));
        for sub in &subs {
            let positions: Vec<[f32; 3]> = sub
                .positions
                .iter()
                .map(|p| (wow_to_bevy(*p) / radius).to_array())
                .collect();
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, sub.uvs.clone());
            mesh.insert_indices(Indices::U32(sub.indices.clone()));
            let is_blue = sub
                .texture
                .as_deref()
                .is_some_and(|t| t.to_lowercase().contains("stars2"));
            let tex = if is_blue { &tex_blue } else { &tex_white };
            // The batch's authored static transparency weight — the alpha-combine bake carries any
            // non-1 constant as a baked loop; sample it at t=0 (Stars.m2 is fully static).
            let weight = sub
                .alpha_anim
                .as_ref()
                .and_then(|a| a.seq(None).weight.as_ref())
                .map_or(1.0, |w| w.sample(0.0));
            // `StarMaterial` (star.wgsl) — gamma-correct premultiplied blend so the soft dots blend
            // into the sky like the reference (our linear-space alpha blend over-brightens them).
            let mat = star_mats.add(StarMaterial {
                base: StandardMaterial {
                    base_color: Color::srgba(1.0, 1.0, 1.0, 0.0), // alpha driven per-frame
                    base_color_texture: tex.clone(),
                    unlit: true,
                    cull_mode: None,
                    alpha_mode: AlphaMode::Premultiplied,
                    // The first sky draw — everything else paints over the stars (the ladder in
                    // `sky_order`; camera-centred domes all sort at ~0 without a bias, leaving
                    // the order to submission luck).
                    depth_bias: sky_order::STARS_BIAS,
                    ..default()
                },
                extension: StarExt {},
            });
            commands.spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat),
                Transform::default(),
                StarDome { weight },
            ));
        }
    } else {
        // Assetless fallback: a procedural scattered dot field (same gamma-correct StarMaterial).
        let star_dot = images.add(radial_sprite(32, 0.2, 1.0));
        let star_mat = star_mats.add(StarMaterial {
            base: StandardMaterial {
                base_color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                base_color_texture: Some(star_dot),
                unlit: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Premultiplied,
                depth_bias: sky_order::STARS_BIAS, // first sky draw — see the asset lane above
                ..default()
            },
            extension: StarExt {},
        });
        commands.spawn((
            Mesh3d(meshes.add(star_field_mesh(350))),
            MeshMaterial3d(star_mat),
            Transform::default(),
            StarDome { weight: 1.0 },
        ));
    }
}
