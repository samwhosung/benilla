//! The celestial layer's startup spawn.

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
    // Discs take `0x6d1960`'s horizon clip and fade, keeping their colour's alpha `a_disc` above
    // the band; glares add gamma bytes (`0x7e5a16`) with no clip. The follow systems rewrite every
    // tint each frame, so `base_color` here is only the first frame's.
    let clip = |base: StandardMaterial, a_disc: f32| CelestialMaterial {
        base,
        extension: CelestialExt {
            fade: Vec4::new(DISC_HORIZON_FADE, 1.0, 0.0, a_disc),
            span: Vec4::new(1.0, 1.0, 0.0, 0.0), // first frame (elevated); the follows write it
        },
    };
    let glare = |base: StandardMaterial| CelestialMaterial {
        base,
        extension: CelestialExt {
            fade: Vec4::new(0.0, 1.0, 1.0, 0.0), // .z = 1: additive glare mode
            span: Vec4::ZERO,                    // unused in glare mode
        },
    };
    // The sun disc, `sunCenter.blp`, or a generated soft disc without assets.
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
            alpha_mode: AlphaMode::Premultiplied, // gamma-correct blend (celestial.wgsl)
            depth_bias: sky_order::SUN_DISC_BIAS, // second sky draw, under the moons and clouds
            ..default()
        },
        1.0, // colour alpha 0xFF, the per-frame diffuse broadcast (0x6d2914)
    ));
    // The sun glare, `sunGlare.blp`: dark texels on a huge quad keep the added light gentle.
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
        depth_bias: sky_order::GLARE_BIAS, // the frame's last render, over clouds and rain
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
    // The white moon, `moon.blp`, opaque but for its feathered texture alpha.
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
            depth_bias: sky_order::WHITE_MOON_BIAS, // third sky draw, over the sun where they cross
            ..default()
        },
        1.0, // colour alpha 0xFF, the per-frame diffuse broadcast (0x6d2914)
    ));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(white_moon),
        Transform::default(),
        MoonSprite {
            part: MoonPart::Disc,
        },
    ));
    // moon02, `moon02.blp`: its colour dword (`[0xce98a4]`) has no writer, so above the horizon
    // band the reference draws it every frame at alpha 0; only the band's ramp and the weather
    // seed ever show it.
    let moon02_tex = world_assets
        .as_mut()
        .and_then(|a| a.sprite_texture("textures\\moon02.blp", &mut images))
        .unwrap_or_else(|| images.add(radial_sprite(128, 0.7, 0.97)));
    let moon02 = disc_mats.add(clip(
        StandardMaterial {
            base_color: Color::BLACK, // the unwritten [0xce98a4], BSS zero
            base_color_texture: Some(moon02_tex),
            unlit: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Premultiplied,
            depth_bias: sky_order::MOON02_BIAS, // fourth sky draw, the last disc
            ..default()
        },
        0.0, // its colour's alpha is unwritten too (`0xce98a4`)
    ));
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(moon02),
        Transform::default(),
        MoonSprite {
            part: MoonPart::Moon02,
        },
    ));
    // The moon glare, the `moonglare.blp` ring, warm under the same band; the teal rim is the
    // dome's night bands through the disc's feathered edge.
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
        depth_bias: sky_order::GLARE_BIAS, // the frame's last render, over clouds and rain
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

    // The stars: `Stars.m2`'s patches on a unit dome, or the procedural field without assets.
    let star_subs = world_assets
        .as_mut()
        .and_then(|a| {
            load_m2_mesh(&mut a.chain.lock_recover(), "Environments\\Stars\\Stars.m2").ok()
        })
        .filter(|s| !s.is_empty());
    if let Some(subs) = star_subs {
        let radius = subs
            .iter()
            .flat_map(|s| s.positions.iter())
            .map(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt())
            .fold(0.0_f32, f32::max)
            .max(1e-3);
        // One material per patch, not per texture: each patch has its own transparency weight.
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
            // The batch's static transparency weight, baked as a loop; sampled at 0, as `Stars.m2`
            // is static.
            let weight = sub
                .alpha_anim
                .as_ref()
                .and_then(|a| a.seq(None).weight.as_ref())
                .map_or(1.0, |w| w.sample(0.0));
            let mat = star_mats.add(StarMaterial {
                base: StandardMaterial {
                    base_color: Color::srgba(1.0, 1.0, 1.0, 0.0), // alpha driven per-frame
                    base_color_texture: tex.clone(),
                    unlit: true,
                    cull_mode: None,
                    alpha_mode: AlphaMode::Premultiplied,
                    // The first sky draw: everything else paints over the stars.
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
        // Without assets, the procedural dot field.
        let star_dot = images.add(radial_sprite(32, 0.2, 1.0));
        let star_mat = star_mats.add(StarMaterial {
            base: StandardMaterial {
                base_color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                base_color_texture: Some(star_dot),
                unlit: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Premultiplied,
                depth_bias: sky_order::STARS_BIAS, // first sky draw, as above
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
