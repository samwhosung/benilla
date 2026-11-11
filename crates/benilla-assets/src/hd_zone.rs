//! Hero Defense: herni plocha kolem hradu bez objektu mapy a se srovnanym terenem.
//!
//! Upravuje se primo nactena dlazdice ADT, takze zmenu prevezme vykresleni, kolize, dotazy na
//! vysku i trava. Kruh kolem hradu je srovnany na vysku plane, za nim teren plynule prechazi
//! zpet do puvodniho. Objekty mapy (doodady a budovy WMO), jejichz stred lezi v kruhu vcetne
//! prechodu, se nekresli.
//!
//! Server pouziva pro vsechno v kruhu stejnou vysku (vysku hradu), viz `HdArena::LocalToWorld`.

use benilla_formats::TileMesh;

/// Hrad (mapa 1, Kalimdor), stejne jako na serveru (HERO_DEFENSE_PLAN.md, cast Mapa).
const CENTER: [f32; 2] = [-6180.0, -3710.0];
/// Vyska srovnane plochy (rovina Shimmering Flats) a vyska hradu na serveru.
const FLAT_Z: f32 = -58.75;
/// Do teto vzdalenosti je teren rovny: areny konci ve 202 yd, cesta bosse ve 240 yd.
const FLAT_RADIUS: f32 = 245.0;
/// Od `FLAT_RADIUS` do teto vzdalenosti teren plynule prechazi do puvodniho.
const BLEND_RADIUS: f32 = 270.0;

/// Upravi dlazdici, pokud je to Kalimdor a zasahuje do herni plochy.
pub(crate) fn apply(path: &str, tile: &mut TileMesh) {
    if !path.to_ascii_lowercase().replace('\\', "/").contains("kalimdor/kalimdor_") {
        return;
    }

    let dist = |p: [f32; 3]| (p[0] - CENTER[0]).hypot(p[1] - CENTER[1]);

    let (doodads, wmos) = (tile.doodads.len(), tile.wmos.len());
    tile.doodads.retain(|d| dist(d.position) > BLEND_RADIUS);
    tile.wmos.retain(|w| dist(w.position) > BLEND_RADIUS);

    let mut flattened = 0usize;
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
                let len = (blended[0] * blended[0] + blended[1] * blended[1] + blended[2] * blended[2]).sqrt();
                if len > 0.0 {
                    *n = [blended[0] / len, blended[1] / len, blended[2] / len];
                }
            }
            flattened += 1;
        }
    }

    let removed = (doodads - tile.doodads.len(), wmos - tile.wmos.len());
    if flattened > 0 || removed != (0, 0) {
        bevy::log::info!(
            "hero defense: {path}: srovnano {flattened} bodu terenu, schovano {} doodadu a {} budov",
            removed.0,
            removed.1
        );
    }
}
