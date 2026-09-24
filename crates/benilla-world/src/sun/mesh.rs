//! The celestial layer's billboard quad, procedural star field and soft round sprite.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// A unit quad in the XY plane (UV 0..1), the sprite billboard.
pub(super) fn quad_mesh() -> Mesh {
    let positions = vec![
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let indices = vec![0u32, 1, 2, 0, 2, 3];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    m.insert_indices(Indices::U32(indices));
    m
}

/// A pseudo-random `[0, 1)` from an index, stable across frames so the stars never flicker.
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = (x ^ (x >> 16)).wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x & 0x00ff_ffff) as f32 / 0x0100_0000 as f32
}

/// The assetless stand-in for `Stars.m2`: `count` small quads facing the centre, over the upper
/// hemisphere at unit radius, hashed by index and uniform by area, with some size variation.
pub(super) fn star_field_mesh(count: u32) -> Mesh {
    const SIZE: f32 = 0.0022; // half-size on the unit sphere, ~0.13°
    let mut positions = Vec::with_capacity(count as usize * 4);
    let mut uvs = Vec::with_capacity(count as usize * 4);
    let mut normals = Vec::with_capacity(count as usize * 4);
    let mut indices = Vec::with_capacity(count as usize * 6);
    for i in 0..count {
        let az = hash01(i * 3) * std::f32::consts::TAU;
        let y = 0.04 + 0.95 * hash01(i * 3 + 1); // up-component (just off the horizon → zenith)
        let s = SIZE * (0.6 + 0.9 * hash01(i * 3 + 2)); // per-star size variation
        let h = (1.0 - y * y).max(0.0).sqrt();
        let dir = Vec3::new(h * az.cos(), y, h * az.sin());
        // A tangent frame perpendicular to `dir` (robust near the zenith where dir ≈ +Y).
        let mut right = dir.cross(Vec3::Y);
        if right.length_squared() < 1e-6 {
            right = dir.cross(Vec3::X);
        }
        let right = right.normalize();
        let upq = dir.cross(right).normalize();
        let base = positions.len() as u32;
        for (dx, dy, uv) in [
            (-1.0, -1.0, [0.0, 1.0]),
            (1.0, -1.0, [1.0, 1.0]),
            (1.0, 1.0, [1.0, 0.0]),
            (-1.0, 1.0, [0.0, 0.0]),
        ] {
            positions.push((dir + (right * dx + upq * dy) * s).to_array());
            normals.push((-dir).to_array());
            uvs.push(uv);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    m.insert_indices(Indices::U32(indices));
    m
}

/// A white sprite whose alpha is opaque inside `core` and 0 beyond `edge` (0 = centre, 1 = the
/// square's edge midpoint), with a smoothstep between.
pub(super) fn radial_sprite(size: u32, core: f32, edge: f32) -> Image {
    let mut data = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) * 0.5;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let r = (dx * dx + dy * dy).sqrt(); // 0 centre → 1 edge-mid → ~1.41 corner
            let t = ((r - core) / (edge - core)).clamp(0.0, 1.0);
            let a = 1.0 - (t * t * (3.0 - 2.0 * t));
            let i = ((y * size + x) * 4) as usize;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = (a * 255.0) as u8;
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}
