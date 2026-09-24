//! The WMO-interior minimap rasterised offline from the shipped tiles, with the client's placement
//! and 224/255 alpha test, written as `mm_{colour,cover,any,owner,seam}.png` under `MM_OUT`. Black
//! in `cover` (some tile passes the test) but white in `any` (some tile has alpha) is the test
//! cutting art; black in both is a hole in the bake or a spot no placement covers.
use std::collections::BTreeMap;
use std::io::Cursor;

const MAP_CENTER: f32 = 17066.666;
const YD_PER_TEXEL: f32 = 0.5;
const TILE_SPAN_YD: f32 = 128.0;
const ALPHA_REF: u8 = 224;

fn next_pow2(n: u32) -> u32 {
    if n <= 1 {
        1
    } else {
        1u32 << (32 - (n - 1).leading_zeros())
    }
}
/// `benilla_assets::minimap_grid::group_axis_grid`, inlined so this example needs no bevy dep.
fn group_axis_grid(extent: f32) -> (u32, f32) {
    let texels = (extent / YD_PER_TEXEL).max(1.0);
    let px = next_pow2(texels.ceil() as u32).clamp(32, 256);
    (
        ((extent / TILE_SPAN_YD).ceil().max(1.0)) as u32,
        px as f32 * YD_PER_TEXEL,
    )
}

fn model_to_world(rot_deg: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    let (rx, ry, rz) = (
        rot_deg[0].to_radians(),
        rot_deg[1].to_radians(),
        rot_deg[2].to_radians(),
    );
    let rot_x = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c]
    };
    let rot_y = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
    };
    let rot_z = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    };
    let p = rot_x(p, rz - std::f32::consts::FRAC_PI_2);
    let p = rot_z(p, -rx);
    let p = rot_y(p, ry - std::f32::consts::PI);
    rot_x(p, std::f32::consts::FRAC_PI_2)
}
fn world_to_model(rot_deg: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    let (rx, ry, rz) = (
        rot_deg[0].to_radians(),
        rot_deg[1].to_radians(),
        rot_deg[2].to_radians(),
    );
    let rot_x = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c]
    };
    let rot_y = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
    };
    let rot_z = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    };
    let p = rot_x(p, -std::f32::consts::FRAC_PI_2);
    let p = rot_y(p, -(ry - std::f32::consts::PI));
    let p = rot_z(p, rx);
    rot_x(p, -(rz - std::f32::consts::FRAC_PI_2))
}

struct Tile {
    gi: usize,
    x0: f32,
    y0: f32,
    tw: f32,
    th: f32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    midz: f32,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pin = [
        args.first().map_or(-8796.83, |s| s.parse().unwrap()),
        args.get(1).map_or(744.25, |s| s.parse().unwrap()),
        args.get(2).map_or(97.83, |s| s.parse().unwrap()),
    ];
    let half: f32 = args.get(3).map_or(90.0, |s| s.parse().unwrap());
    let out = std::env::var("MM_OUT").unwrap_or_else(|_| ".".into());

    let data = benilla_formats::wow_data().expect("no WoW install");
    let mut chain = benilla_formats::open_chain(&data)?;
    let trs = benilla_formats::load_minimap_translate(&mut chain)?;

    // The WMO placement in this ADT whose origin is nearest the pin.
    let (tc, tr) = benilla_formats::world_to_tile(pin[0], pin[1]);
    let adt_name = format!("World\\Maps\\Azeroth\\Azeroth_{tc}_{tr}.adt");
    let bytes = chain.read_file(&adt_name)?;
    let benilla_adt::ParsedAdt::Root(adt) = benilla_adt::parse_adt(&mut Cursor::new(&*bytes))?;
    let mut best: Option<(String, [f32; 3], [f32; 3], f32)> = None;
    for p in &adt.wmo_placements {
        let Some(model) = adt.wmos.get(p.name_id as usize) else {
            continue;
        };
        let o = [
            MAP_CENTER - p.position[2],
            MAP_CENTER - p.position[0],
            p.position[1],
        ];
        let d = (o[0] - pin[0]).hypot(o[1] - pin[1]);
        if best.as_ref().is_none_or(|b| d < b.3) {
            best = Some((model.replace('/', "\\"), o, p.rotation, d));
        }
    }
    let (model, origin, rot, _) = best.expect("no WMO placement in this ADT");
    // `MM_MODEL="x,y[,z]"` pins in the WMO's model space instead, as a reference capture states it.
    let pm = match std::env::var("MM_MODEL") {
        Ok(v) => {
            let n: Vec<f32> = v.split(',').map(|t| t.trim().parse().unwrap()).collect();
            [n[0], n[1], *n.get(2).unwrap_or(&0.0)]
        }
        Err(_) => world_to_model(
            rot,
            [pin[0] - origin[0], pin[1] - origin[1], pin[2] - origin[2]],
        ),
    };
    let back = model_to_world(rot, pm);
    eprintln!("model {model}\n  origin {origin:?} rot {rot:?}\n  pin in model space: {pm:?}\n  round-trip: {back:?} vs {:?}",
        [pin[0] - origin[0], pin[1] - origin[1], pin[2] - origin[2]]);

    let stem = model
        .trim_end_matches(".wmo")
        .trim_end_matches(".WMO")
        .to_string();
    let key_stem = stem
        .to_ascii_lowercase()
        .split_once("world\\")
        .map(|(_, r)| r.to_string())
        .unwrap();
    let root = benilla_formats::parse_wmo_root(&chain.read_file(&format!("{stem}.wmo"))?)?;
    let infos = root.group_infos().to_vec();
    let (mut glo, mut ghi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for i in &infos {
        for k in 0..3 {
            glo[k] = glo[k].min(i.bbox_min[k]);
            ghi[k] = ghi[k].max(i.bbox_max[k]);
        }
    }
    eprintln!("{} groups, model bbox {glo:?} .. {ghi:?}", infos.len());

    // `MM_AUDIT`: our per-group tile grid against the one authored in the translate table.
    if std::env::var("MM_AUDIT").is_ok() {
        let (mut bad, mut tiles_ours, mut tiles_theirs) = (0usize, 0usize, 0usize);
        for (gi, info) in infos.iter().enumerate() {
            let (mut mc, mut mr, mut present) = (-1i32, -1i32, 0usize);
            for c in 0..16u32 {
                for r in 0..16u32 {
                    if trs
                        .get(&format!("{key_stem}_{gi:03}_{c:02}_{r:02}.blp"))
                        .is_some()
                    {
                        mc = mc.max(c as i32);
                        mr = mr.max(r as i32);
                        present += 1;
                    }
                }
            }
            if present == 0 {
                continue;
            }
            let (nx, twx) = group_axis_grid(info.bbox_max[0] - info.bbox_min[0]);
            let (ny, twy) = group_axis_grid(info.bbox_max[1] - info.bbox_min[1]);
            tiles_ours += (nx * ny) as usize;
            tiles_theirs += present;
            if nx as i32 != mc + 1 || ny as i32 != mr + 1 {
                bad += 1;
                if bad <= 20 {
                    println!(
                        "  g{gi:3}: ours {nx}x{ny} (tile {twx}x{twy} yd), authored {}x{} ({present} tiles)  extent {:.1} x {:.1}",
                        mc + 1, mr + 1,
                        info.bbox_max[0] - info.bbox_min[0], info.bbox_max[1] - info.bbox_min[1],
                    );
                }
            }
        }
        println!("AUDIT: {bad} groups with a grid mismatch; we ask for {tiles_ours} tiles, {tiles_theirs} are authored");
        return Ok(());
    }

    // `MM_FIT`: each group's real yards per texel, from its authored tile size and its extent.
    if std::env::var("MM_FIT").is_ok() {
        println!("  gi  authored px   grid   extent yd        yd/texel (x, y)");
        let mut ratios: Vec<f32> = Vec::new();
        for (gi, info) in infos.iter().enumerate() {
            let (mut mc, mut mr) = (-1i32, -1i32);
            let mut dim = (0u32, 0u32);
            for c in 0..16u32 {
                for r in 0..16u32 {
                    let key = format!("{key_stem}_{gi:03}_{c:02}_{r:02}.blp");
                    let Some(hashed) = trs.get(&key) else {
                        continue;
                    };
                    mc = mc.max(c as i32);
                    mr = mr.max(r as i32);
                    if c == 0 && r == 0 {
                        if let Ok(b) = chain.read_file(&format!("textures\\Minimap\\{hashed}")) {
                            if let Ok(d) = benilla_blp::decode(&b) {
                                dim = (d.width, d.height)
                            }
                        }
                    }
                }
            }
            if mc < 0 || dim.0 == 0 {
                continue;
            }
            let (ex, ey) = (
                info.bbox_max[0] - info.bbox_min[0],
                info.bbox_max[1] - info.bbox_min[1],
            );
            // This read pairs model X with image height and rows: the layout below, transposed.
            let (nx, ny) = ((mc + 1) as f32, (mr + 1) as f32);
            let ypt_x = ex / (dim.1 as f32 * ny);
            let ypt_y = ey / (dim.0 as f32 * nx);
            ratios.push(ypt_x);
            ratios.push(ypt_y);
            if gi < 24 {
                println!(
                    "  g{gi:3}  {:3}x{:<3}  {}x{}   {ex:7.1} x {ey:7.1}   {ypt_x:.4}  {ypt_y:.4}",
                    dim.0,
                    dim.1,
                    mc + 1,
                    mr + 1
                );
            }
        }
        ratios.sort_by(f32::total_cmp);
        let mean = ratios.iter().sum::<f32>() / ratios.len() as f32;
        println!(
            "yd/texel over {} axes: min {:.4} median {:.4} max {:.4} mean {mean:.4}",
            ratios.len(),
            ratios[0],
            ratios[ratios.len() / 2],
            ratios[ratios.len() - 1]
        );
        return Ok(());
    }

    // `MM_ANCHOR`: where a single-tile group's opaque art sits in its power-of-two texture.
    if std::env::var("MM_ANCHOR").is_ok() {
        println!("  gi   tex     opaque box (u0,u1)x(v0,v1)   extent/0.5 texels   fill");
        for (gi, info) in infos.iter().enumerate().take(40) {
            let key = format!("{key_stem}_{gi:03}_00_00.blp");
            let Some(hashed) = trs.get(&key) else {
                continue;
            };
            if trs.get(&format!("{key_stem}_{gi:03}_01_00.blp")).is_some()
                || trs.get(&format!("{key_stem}_{gi:03}_00_01.blp")).is_some()
            {
                continue;
            }
            let Ok(b) = chain.read_file(&format!("textures\\Minimap\\{hashed}")) else {
                continue;
            };
            let Ok(d) = benilla_blp::decode(&b) else {
                continue;
            };
            let m = &d.mips[0];
            let (mut u0, mut u1, mut v0, mut v1) = (u32::MAX, 0u32, u32::MAX, 0u32);
            for v in 0..m.height {
                for u in 0..m.width {
                    if m.rgba[((v * m.width + u) * 4 + 3) as usize] != 0 {
                        u0 = u0.min(u);
                        u1 = u1.max(u);
                        v0 = v0.min(v);
                        v1 = v1.max(v);
                    }
                }
            }
            if u0 == u32::MAX {
                continue;
            }
            let (ex, ey) = (
                info.bbox_max[0] - info.bbox_min[0],
                info.bbox_max[1] - info.bbox_min[1],
            );
            println!("  g{gi:3}  {:3}x{:<3}  u[{u0:3},{u1:3}] v[{v0:3},{v1:3}]   X {:.1} tx, Y {:.1} tx   {:.0}x{:.0}",
                m.width, m.height, ex / YD_PER_TEXEL, ey / YD_PER_TEXEL, u1 - u0 + 1, v1 - v0 + 1);
        }
        return Ok(());
    }

    // `MM_MULTI`: the same on multi-tile groups, for which name index runs along which axis.
    if std::env::var("MM_MULTI").is_ok() {
        let mut shown = 0;
        for (gi, info) in infos.iter().enumerate() {
            let mut cells = Vec::new();
            for c in 0..8u32 {
                for r in 0..8u32 {
                    if let Some(h) = trs.get(&format!("{key_stem}_{gi:03}_{c:02}_{r:02}.blp")) {
                        cells.push((c, r, h.to_string()));
                    }
                }
            }
            if cells.len() < 2 {
                continue;
            }
            let (ex, ey) = (
                info.bbox_max[0] - info.bbox_min[0],
                info.bbox_max[1] - info.bbox_min[1],
            );
            println!(
                "g{gi} extent X {ex:.1} ({:.1} tx) Y {ey:.1} ({:.1} tx), {} tiles",
                ex / 0.5,
                ey / 0.5,
                cells.len()
            );
            for (c, r, h) in &cells {
                let Ok(b) = chain.read_file(&format!("textures\\Minimap\\{h}")) else {
                    continue;
                };
                let Ok(d) = benilla_blp::decode(&b) else {
                    continue;
                };
                let m = &d.mips[0];
                let (mut u0, mut u1, mut v0, mut v1) = (u32::MAX, 0u32, u32::MAX, 0u32);
                for v in 0..m.height {
                    for u in 0..m.width {
                        if m.rgba[((v * m.width + u) * 4 + 3) as usize] != 0 {
                            u0 = u0.min(u);
                            u1 = u1.max(u);
                            v0 = v0.min(v);
                            v1 = v1.max(v);
                        }
                    }
                }
                println!(
                    "   {c}_{r}: {:3}x{:<3}  opaque u[{u0:3},{u1:3}] v[{v0:3},{v1:3}]",
                    m.width, m.height
                );
            }
            shown += 1;
            if shown >= 6 {
                break;
            }
        }
        return Ok(());
    }

    // `MM_BLP`: each tile's declared header and alpha histogram, as the file holds them.
    if std::env::var("MM_BLP").is_ok() {
        for gi in 0..infos.len() {
            for c in 0..4u32 {
                for r in 0..4u32 {
                    let key = format!("{key_stem}_{gi:03}_{c:02}_{r:02}.blp");
                    let Some(hashed) = trs.get(&key) else {
                        continue;
                    };
                    let Ok(b) = chain.read_file(&format!("textures\\Minimap\\{hashed}")) else {
                        continue;
                    };
                    let hdr = (
                        u32::from_le_bytes(b[8..12].try_into().unwrap()), // compression
                        b[12],
                        b[13],
                        b[14],
                        b[15], // alphaDepth, alphaType, hasMips, ?
                    );
                    let Ok(d) = benilla_blp::decode(&b) else {
                        continue;
                    };
                    let m = &d.mips[0];
                    let zero = m.rgba.chunks(4).filter(|p| p[3] == 0).count();
                    let full = m.rgba.chunks(4).filter(|p| p[3] == 255).count();
                    let n = m.rgba.len() / 4;
                    println!("g{gi:3} {c}_{r} {}x{} hdr(comp={} aDepth={} aType={} mips={}) alpha0 {:.1}% alpha255 {:.1}% other {}",
                        m.width, m.height, hdr.0, hdr.1, hdr.2, hdr.3,
                        100.0 * zero as f32 / n as f32, 100.0 * full as f32 / n as f32, n - zero - full);
                    if gi > 60 {
                        return Ok(());
                    }
                }
            }
        }
        return Ok(());
    }

    let (lo, hi) = ([pm[0] - half, pm[1] - half], [pm[0] + half, pm[1] + half]);
    let mut tiles: Vec<Tile> = Vec::new();
    let mut cache: BTreeMap<String, Option<(u32, u32, Vec<u8>)>> = BTreeMap::new();
    for (gi, info) in infos.iter().enumerate() {
        let (nx, twx) = group_axis_grid(info.bbox_max[0] - info.bbox_min[0]);
        let (ny, twy) = group_axis_grid(info.bbox_max[1] - info.bbox_min[1]);
        for col in 0..nx {
            for row in 0..ny {
                // Measured off the shipped art: image u runs along model +X from the bbox X-min,
                // columns advancing with +X; v runs along model Y inverted. Cells stride `tw` and
                // share interior edges; a boundary cell grows 1.0 yd on its outer side
                // (`0x6a5270`), and `MM_NOBLEED` drops that, so neighbouring groups only abut.
                let bleed = if std::env::var("MM_NOBLEED").is_ok() {
                    0.0
                } else {
                    1.0
                };
                let x0 = info.bbox_min[0] + col as f32 * twx - if col == 0 { bleed } else { 0.0 };
                let x1 = info.bbox_min[0]
                    + (col + 1) as f32 * twx
                    + if col + 1 == nx { bleed } else { 0.0 };
                let y0 = info.bbox_min[1] + row as f32 * twy - if row == 0 { bleed } else { 0.0 };
                let y1 = info.bbox_min[1]
                    + (row + 1) as f32 * twy
                    + if row + 1 == ny { bleed } else { 0.0 };
                if x0 > hi[0] || x1 < lo[0] || y0 > hi[1] || y1 < lo[1] {
                    continue;
                }
                let key = format!("{key_stem}_{gi:03}_{col:02}_{row:02}.blp");
                let dec = cache.entry(key.clone()).or_insert_with(|| {
                    let hashed = trs.get(&key)?;
                    let b = chain
                        .read_file(&format!("textures\\Minimap\\{hashed}"))
                        .ok()?;
                    let d = benilla_blp::decode(&b).ok()?;
                    let m = d.mips.first()?;
                    Some((m.width, m.height, m.rgba.clone()))
                });
                let Some((w, h, rgba)) = dec.clone() else {
                    continue;
                };
                tiles.push(Tile {
                    gi,
                    x0,
                    y0,
                    tw: x1 - x0,
                    th: y1 - y0,
                    w,
                    h,
                    rgba,
                    midz: 0.5 * (info.bbox_min[2] + info.bbox_max[2]),
                });
            }
        }
    }
    let hits = cache.values().filter(|v| v.is_some()).count();
    eprintln!(
        "{} tiles overlap the window ({} keys probed, {hits} resolved)",
        tiles.len(),
        cache.len()
    );
    for k in cache.iter().filter(|(_, v)| v.is_none()).take(4) {
        eprintln!("  MISS {}", k.0);
    }
    // Farthest group Z-mid first. The reference paints ascending by signed `Zmid - playerZ`, the
    // player's own group last (`0x4ebeb0`).
    tiles.sort_by(|a, b| {
        (a.midz - pm[2])
            .abs()
            .total_cmp(&(b.midz - pm[2]).abs())
            .reverse()
    });

    let px = (half * 2.0 / YD_PER_TEXEL) as u32;
    let mut colour = vec![0u8; (px * px * 3) as usize];
    let mut cover = vec![0u8; (px * px) as usize];
    let mut anyv = vec![0u8; (px * px) as usize];
    let mut owner = vec![0u8; (px * px * 3) as usize];
    let bilinear = std::env::var("MM_BILINEAR").is_ok();
    // `MM_TERRAIN`: an outdoor ADT tile underlay; the reference's interior composite has none.
    if std::env::var("MM_TERRAIN").is_ok() {
        let mut adt_cache: BTreeMap<(u32, u32), Option<Vec<u8>>> = BTreeMap::new();
        for iy in 0..px {
            for ix in 0..px {
                let my = pm[1] + half - (ix as f32 + 0.5) * YD_PER_TEXEL;
                let mx = pm[0] + half - (iy as f32 + 0.5) * YD_PER_TEXEL;
                let w = model_to_world(rot, [mx, my, pm[2]]);
                let (wx, wy) = (origin[0] + w[0], origin[1] + w[1]);
                let (tx, ty) = benilla_formats::world_to_tile(wx, wy);
                let e = adt_cache.entry((tx, ty)).or_insert_with(|| {
                    let key = format!("Azeroth\\map{tx}_{ty}.blp");
                    let hashed = trs.get(&key)?;
                    let b = chain
                        .read_file(&format!("textures\\Minimap\\{hashed}"))
                        .ok()?;
                    let d = benilla_blp::decode(&b).ok()?;
                    Some(d.mips.first()?.rgba.clone())
                });
                let Some(rgba) = e else { continue };
                let (ox, oy) = benilla_formats::tile_to_world(tx, ty);
                let u = (((oy - wy) / 533.3333) * 256.0) as u32;
                let v = (((ox - wx) / 533.3333) * 256.0) as u32;
                let s = ((v.min(255) * 256 + u.min(255)) * 4) as usize;
                let o = (iy * px + ix) as usize;
                cover[o] = 255;
                colour[o * 3..o * 3 + 3].copy_from_slice(&rgba[s..s + 3]);
            }
        }
    }
    for t in &tiles {
        let gcol = [
            (t.gi * 97 % 251) as u8,
            (t.gi * 57 % 241) as u8,
            (t.gi * 151 % 233) as u8,
        ];
        for iy in 0..px {
            for ix in 0..px {
                // Screen: +x is model −y, +y (down) is model −x (the app's `to_target`).
                let my = pm[1] + half - (ix as f32 + 0.5) * YD_PER_TEXEL;
                let mx = pm[0] + half - (iy as f32 + 0.5) * YD_PER_TEXEL;
                if mx < t.x0 || mx >= t.x0 + t.tw || my < t.y0 || my >= t.y0 + t.th {
                    continue;
                }
                // `MM_BILINEAR` is the reference's LINEAR, CLAMP_TO_EDGE sampler. The client
                // samples `[0.5/W, 1-0.5/W]` across the drawn rect, so its W texel centres span it.
                let fu = (mx - t.x0) / (t.tw / (t.w as f32 - 1.0));
                let fv = (t.y0 + t.th - my) / (t.th / (t.h as f32 - 1.0));
                let (u, v) = (
                    (fu + 0.5).max(0.0).min(t.w as f32 - 1.0) as u32,
                    (fv + 0.5).max(0.0).min(t.h as f32 - 1.0) as u32,
                );
                let s = ((v * t.w + u) * 4) as usize;
                let a = if bilinear {
                    let (i0, j0) = (fu.floor(), fv.floor());
                    let (wx, wy) = (fu - i0, fv - j0);
                    let at = |i: f32, j: f32| -> f32 {
                        let i = (i.max(0.0) as u32).min(t.w - 1);
                        let j = (j.max(0.0) as u32).min(t.h - 1);
                        f32::from(t.rgba[((j * t.w + i) * 4 + 3) as usize])
                    };
                    let top = at(i0, j0) * (1.0 - wx) + at(i0 + 1.0, j0) * wx;
                    let bot = at(i0, j0 + 1.0) * (1.0 - wx) + at(i0 + 1.0, j0 + 1.0) * wx;
                    (top * (1.0 - wy) + bot * wy).round() as u8
                } else {
                    t.rgba[s + 3]
                };
                let o = (iy * px + ix) as usize;
                if a > 0 {
                    anyv[o] = 255
                }
                if a >= ALPHA_REF {
                    cover[o] = 255;
                    colour[o * 3..o * 3 + 3].copy_from_slice(&t.rgba[s..s + 3]);
                    owner[o * 3..o * 3 + 3].copy_from_slice(&gcol);
                }
            }
        }
    }
    // A hairline is an uncovered run of at most 4 px with coverage on both sides: a tile seam.
    let mut seam = colour.clone();
    let mut hair = 0usize;
    let mut runs: BTreeMap<usize, usize> = BTreeMap::new();
    for axis in 0..2 {
        for a in 0..px {
            let mut i = 0u32;
            while i < px {
                let at = |k: u32| -> usize {
                    if axis == 0 {
                        (a * px + k) as usize
                    } else {
                        (k * px + a) as usize
                    }
                };
                if cover[at(i)] == 0 {
                    let mut j = i;
                    while j < px && cover[at(j)] == 0 {
                        j += 1
                    }
                    let len = (j - i) as usize;
                    if len <= 4 && i > 0 && j < px {
                        hair += len;
                        *runs.entry(len).or_default() += 1;
                        for k in i..j {
                            let o = at(k) * 3;
                            seam[o] = 255;
                            seam[o + 1] = 0;
                            seam[o + 2] = 255;
                        }
                    }
                    i = j;
                } else {
                    i += 1
                }
            }
        }
    }
    println!("hairline px (uncovered run <=4 with coverage both sides): {hair}  run-length histogram {runs:?}");
    image::save_buffer(
        format!("{out}/mm_seam.png"),
        &seam,
        px,
        px,
        image::ColorType::Rgb8,
    )?;
    // `MM_DISC`: the painted share of the visible disc of that radius in yards; the reference
    // leaves 4.16% of it clear in its Stormwind capture.
    if let Ok(v) = std::env::var("MM_DISC") {
        let rd: f32 = v.parse().unwrap();
        let c0 = px as f32 * 0.5;
        let (mut inside, mut painted) = (0usize, 0usize);
        for iy in 0..px {
            for ix in 0..px {
                let d = ((ix as f32 + 0.5 - c0).powi(2) + (iy as f32 + 0.5 - c0).powi(2)).sqrt()
                    * YD_PER_TEXEL;
                if d > rd {
                    continue;
                }
                inside += 1;
                if cover[(iy * px + ix) as usize] != 0 {
                    painted += 1
                }
            }
        }
        println!(
            "DISC r={rd} yd: {:.2}% painted, {:.2}% on the clear",
            100.0 * painted as f32 / inside as f32,
            100.0 * (inside - painted) as f32 / inside as f32
        );
    }

    let holes = cover.iter().filter(|&&c| c == 0).count();
    let recoverable = cover
        .iter()
        .zip(&anyv)
        .filter(|(&c, &a)| c == 0 && a > 0)
        .count();
    println!("{px}x{px} px @ {YD_PER_TEXEL} yd:  {holes} uncovered ({:.2}%), of which {recoverable} have art the alpha test cut ({:.1}%)",
        100.0 * holes as f32 / (px * px) as f32, 100.0 * recoverable as f32 / holes.max(1) as f32);
    image::save_buffer(
        format!("{out}/mm_colour.png"),
        &colour,
        px,
        px,
        image::ColorType::Rgb8,
    )?;
    image::save_buffer(
        format!("{out}/mm_cover.png"),
        &cover,
        px,
        px,
        image::ColorType::L8,
    )?;
    image::save_buffer(
        format!("{out}/mm_any.png"),
        &anyv,
        px,
        px,
        image::ColorType::L8,
    )?;
    image::save_buffer(
        format!("{out}/mm_owner.png"),
        &owner,
        px,
        px,
        image::ColorType::Rgb8,
    )?;
    eprintln!("wrote {out}/mm_{{colour,cover,any,owner}}.png");
    Ok(())
}
