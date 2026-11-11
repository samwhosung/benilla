//! Hero Defense: herni plocha kolem hradu bez objektu mapy, se srovnanym terenem, vyvysenymi
//! arenami a skalami mezi nimi.
//!
//! Upravuje se primo nactena dlazdice ADT, takze zmenu prevezme vykresleni, kolize, dotazy na
//! vysku i trava. Kruh kolem hradu je srovnany na vysku plane, za nim teren plynule prechazi
//! zpet do puvodniho. Objekty mapy (doodady a budovy WMO), jejichz stred lezi v kruhu vcetne
//! prechodu, se nekresli. Na srovnanou plan se pricte vyskove pole `hd_terrain` (plosiny aren,
//! rampy, skaly), skaly dostanou skalni texturu Thousand Needles a modely skal a jehel.
//!
//! Server pocita vysku plosin a ramp stejne (`hd_defs.h`, `HdArenaGroundOffset`), skaly nezna.

use benilla_formats::{Doodad, TileMesh, ALPHA_MAP_SIZE, CHUNK_SIZE};

use crate::hd_rocks::{ROCKS, ROCK_MODELS};
use crate::hd_terrain;

/// Hrad (mapa 1, Kalimdor), stejne jako na serveru (HERO_DEFENSE_PLAN.md, cast Mapa).
const CENTER: [f32; 2] = [-6180.0, -3710.0];
/// Vyska srovnane plochy (rovina Shimmering Flats) a vyska hradu na serveru.
const FLAT_Z: f32 = -58.75;
/// Do teto vzdalenosti je teren rovny: areny konci ve 202 yd, cesta bosse ve 240 yd.
const FLAT_RADIUS: f32 = 245.0;
/// Od `FLAT_RADIUS` do teto vzdalenosti teren plynule prechazi do puvodniho.
const BLEND_RADIUS: f32 = 270.0;

/// Skalni textury z dat hry (Thousand Needles): hlavni a svetlejsi pruhy.
const ROCK_TEXTURE: &str = "Tileset\\ThousandNeedles\\ThousandNeedlesRock.blp";
const ROCK_HIGHLIGHT_TEXTURE: &str = "Tileset\\ThousandNeedles\\ThousandNeedlesRockHighlight.blp";
/// MCLY effectId bez travy a drobnosti.
const NO_EFFECT: u32 = 0xFFFF_FFFF;
/// uniqueId nasich skal: mimo rozsah originalnich objektu, sdileny mezi dlazdicemi.
const ROCK_UNIQUE_BASE: u32 = 0x7D00_0000;
/// Krok pro vypocet normal ze sklonu (yd), zhruba polovina rozestupu vrcholu terenu.
const NORMAL_EPS: f64 = 1.5;

/// Upravi dlazdici, pokud je to Kalimdor a zasahuje do herni plochy.
pub(crate) fn apply(path: &str, tile: &mut TileMesh) {
    if !path
        .to_ascii_lowercase()
        .replace('\\', "/")
        .contains("kalimdor/kalimdor_")
    {
        return;
    }

    let dist = |p: [f32; 3]| (p[0] - CENTER[0]).hypot(p[1] - CENTER[1]);

    let (doodads, wmos) = (tile.doodads.len(), tile.wmos.len());
    tile.doodads.retain(|d| dist(d.position) > BLEND_RADIUS);
    tile.wmos.retain(|w| dist(w.position) > BLEND_RADIUS);

    let mut flattened = 0usize;
    let mut raised = 0usize;
    let mut painted = 0usize;
    for chunk in &mut tile.chunks {
        for (i, p) in chunk.positions.iter_mut().enumerate() {
            let d = dist(*p);
            if d >= BLEND_RADIUS {
                continue;
            }
            // 0 v rovne casti, 1 na vnejsim okraji prechodu (smoothstep).
            let t = ((d - FLAT_RADIUS) / (BLEND_RADIUS - FLAT_RADIUS)).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            p[2] = FLAT_Z + (p[2] - FLAT_Z) * t;
            if let Some(n) = chunk.normals.get_mut(i) {
                let blended = [n[0] * t, n[1] * t, n[2] * t + (1.0 - t)];
                let len =
                    (blended[0] * blended[0] + blended[1] * blended[1] + blended[2] * blended[2])
                        .sqrt();
                if len > 0.0 {
                    *n = [blended[0] / len, blended[1] / len, blended[2] / len];
                }
            }
            flattened += 1;

            // Plosiny, rampy a skaly.
            let (x, y) = (f64::from(p[0]), f64::from(p[1]));
            if hd_terrain::distance(x, y) > hd_terrain::OUTER_MAX + NORMAL_EPS {
                continue;
            }
            let h = hd_terrain::height(x, y);
            let gx = hd_terrain::height(x + NORMAL_EPS, y) - hd_terrain::height(x - NORMAL_EPS, y);
            let gy = hd_terrain::height(x, y + NORMAL_EPS) - hd_terrain::height(x, y - NORMAL_EPS);
            if h == 0.0 && gx == 0.0 && gy == 0.0 {
                continue;
            }
            p[2] += h as f32;
            if let Some(n) = chunk.normals.get_mut(i) {
                let (nx, ny) = (-gx / (2.0 * NORMAL_EPS), -gy / (2.0 * NORMAL_EPS));
                let len = (nx * nx + ny * ny + 1.0).sqrt();
                *n = [(nx / len) as f32, (ny / len) as f32, (1.0 / len) as f32];
            }
            raised += 1;
        }
        if paint_rock(chunk) {
            painted += 1;
        }
    }

    let added = add_rocks(tile);

    let removed = (doodads - tile.doodads.len() + added, wmos - tile.wmos.len());
    if flattened > 0 || removed != (0, 0) || added > 0 {
        bevy::log::info!(
            "hero defense: {path}: srovnano {flattened} bodu terenu, zvednuto {raised}, skalni textura na {painted} chunkach, \
             schovano {} doodadu a {} budov, pridano {added} skal",
            removed.0,
            removed.1
        );
    }
}

/// Prida chunku skalni textury tam, kde je teren skala. Vrati, jestli se chunk zmenil.
///
/// Vrstvy se v shaderu michaji postupne (`mix` po vrstvach 1 az 3), takze skala jako posledni
/// vrstva prekryje vse pod ni. Chunk ma nejvys 4 vrstvy: pri 4 se nahradi posledni.
fn paint_rock(chunk: &mut benilla_formats::ChunkMesh) -> bool {
    let Some(nw) = chunk.positions.first().copied() else {
        return false;
    };
    let size = f64::from(CHUNK_SIZE);
    let (nx, ny) = (f64::from(nw[0]), f64::from(nw[1]));
    // Nejblizsi bod chunku ke hradu; chunk mimo skaly se preskoci.
    let (cx, cy) = (f64::from(CENTER[0]), f64::from(CENTER[1]));
    let qx = cx.clamp(nx - size, nx);
    let qy = cy.clamp(ny - size, ny);
    if (qx - cx).hypot(qy - cy) > hd_terrain::OUTER_MAX {
        return false;
    }

    let n = ALPHA_MAP_SIZE as usize;
    let mut rock = vec![0u8; n * n];
    let mut light = vec![0u8; n * n];
    let mut any = false;
    for ty in 0..n {
        for tx in 0..n {
            // Texel (tx, ty): tx na vychod (sloupce chunku), ty na jih (radky chunku).
            let x = nx - (ty as f64 + 0.5) / n as f64 * size;
            let y = ny - (tx as f64 + 0.5) / n as f64 * size;
            let w = hd_terrain::rockiness(x, y);
            if w <= 0.0 {
                continue;
            }
            any = true;
            rock[ty * n + tx] = (w * 255.0).round() as u8;
            light[ty * n + tx] = (w * hd_terrain::highlight(x, y) * 255.0).round() as u8;
        }
    }
    if !any {
        return false;
    }

    let mut alpha = chunk.alpha_map.take().unwrap_or_else(|| {
        let mut map = vec![0u8; n * n * 4];
        for px in map.chunks_exact_mut(4) {
            px[3] = 255;
        }
        map
    });
    // Skala do prvni volne vrstvy (pri plnem chunku do posledni), pruhy za ni, kdyz je misto.
    let rock_slot = chunk.layer_textures.len().clamp(1, 3);
    set_layer(chunk, rock_slot, ROCK_TEXTURE);
    for i in 0..n * n {
        alpha[i * 4 + rock_slot - 1] = rock[i];
    }
    if rock_slot < 3 {
        set_layer(chunk, rock_slot + 1, ROCK_HIGHLIGHT_TEXTURE);
        for i in 0..n * n {
            alpha[i * 4 + rock_slot] = light[i];
        }
    }
    chunk.alpha_map = Some(alpha);
    true
}

/// Nastavi vrstve `slot` texturu (vrstvu prida, kdyz chybi), bez travy a drobnosti.
fn set_layer(chunk: &mut benilla_formats::ChunkMesh, slot: usize, texture: &str) {
    if chunk.layer_textures.is_empty() {
        // Chunk bez vrstev: zakladni vrstva zustane jako pomocna, skala jde nad ni.
        chunk.layer_textures.push(texture.to_string());
        chunk.layer_effect_ids.push(NO_EFFECT);
    }
    while chunk.layer_textures.len() <= slot {
        chunk.layer_textures.push(texture.to_string());
        chunk.layer_effect_ids.push(NO_EFFECT);
    }
    chunk.layer_textures[slot] = texture.to_string();
    if let Some(e) = chunk.layer_effect_ids.get_mut(slot) {
        *e = NO_EFFECT;
    }
}

/// Prida modely skal, jejichz stred lezi v teto dlazdici. Vrati jejich pocet.
fn add_rocks(tile: &mut TileMesh) -> usize {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for p in tile.chunks.iter().flat_map(|c| c.positions.iter()) {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }
    let mut added = 0;
    for (i, &(model, x, y, heading, tilt_x, tilt_z, scale, sink)) in ROCKS.iter().enumerate() {
        // Kazda skala patri prave jedne dlazdici (polootevrene hranice).
        if !(x >= min_x && x < max_x && y >= min_y && y < max_y) {
            continue;
        }
        let z = FLAT_Z + hd_terrain::height(f64::from(x), f64::from(y)) as f32 - sink;
        tile.doodads.push(Doodad {
            model: ROCK_MODELS[model as usize].to_string(),
            position: [x, y, z],
            rotation: [tilt_x, heading, tilt_z],
            scale,
            unique_id: ROCK_UNIQUE_BASE + i as u32,
        });
        added += 1;
    }
    added
}
