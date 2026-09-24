//! The WMO liquid around a pin: the groups holding water there, their MOGP flags, liquid type and
//! MLIQ heights, and how many surfaces claim each point.
//! `cargo run -p benilla-formats --example mm_water_pin -- x y z [r]`
use std::io::Cursor;

const MAP_CENTER: f32 = 17066.666;

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

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pin = [
        a.first().map_or(-8796.83, |s| s.parse().unwrap()),
        a.get(1).map_or(744.25, |s| s.parse().unwrap()),
        a.get(2).map_or(97.83, |s| s.parse().unwrap()),
    ];
    let r: f32 = a.get(3).map_or(60.0, |s| s.parse().unwrap());

    let data = benilla_formats::wow_data().unwrap();
    let mut chain = benilla_formats::open_chain(&data)?;
    let (tc, tr) = benilla_formats::world_to_tile(pin[0], pin[1]);
    let bytes = chain.read_file(&format!("World\\Maps\\Azeroth\\Azeroth_{tc}_{tr}.adt"))?;
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
    let (model, origin, rot, _) = best.unwrap();
    let pm = world_to_model(
        rot,
        [pin[0] - origin[0], pin[1] - origin[1], pin[2] - origin[2]],
    );
    println!("{model}  pin in model space {pm:?}");
    let stem = model.trim_end_matches(".wmo").to_string();
    let root = benilla_formats::parse_wmo_root(&chain.read_file(&format!("{stem}.wmo"))?)?;

    let mut surfaces: Vec<(usize, benilla_formats::LiquidMesh, u32)> = Vec::new();
    for (gi, info) in root.group_infos().iter().enumerate() {
        if info.bbox_min[0] > pm[0] + r
            || info.bbox_max[0] < pm[0] - r
            || info.bbox_min[1] > pm[1] + r
            || info.bbox_max[1] < pm[1] - r
        {
            continue;
        }
        let Ok(gb) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
            continue;
        };
        let Some(h) = benilla_formats::wmo_group_header(&gb) else {
            continue;
        };
        let liq = benilla_formats::wmo_group_liquid_mesh(&gb);
        let Some(m) = liq else { continue };
        let wet = m.wet.iter().filter(|w| **w).count();
        if wet == 0 {
            continue;
        }
        let (mut zlo, mut zhi) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut xlo, mut xhi, mut ylo, mut yhi) = (
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        );
        let (cols, rows) = (m.grid[0] as usize, m.grid[1] as usize);
        for j in 0..rows.saturating_sub(1) {
            for i in 0..cols.saturating_sub(1) {
                if !m.wet[j * (cols - 1) + i] {
                    continue;
                }
                for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = m.positions[(j + dj) * cols + i + di];
                    zlo = zlo.min(p[2]);
                    zhi = zhi.max(p[2]);
                    xlo = xlo.min(p[0]);
                    xhi = xhi.max(p[0]);
                    ylo = ylo.min(p[1]);
                    yhi = yhi.max(p[1]);
                }
            }
        }
        surfaces.push((gi, m.clone(), h.flags));
        let w = model_to_world(rot, [0.5 * (xlo + xhi), 0.5 * (ylo + yhi), zhi]);
        println!(
            "  g{gi:3} flags 0x{:x}{}  liq {:?} groupLiquid 0x{:x}  grid {}x{} wet {wet}\n         model x[{xlo:.1},{xhi:.1}] y[{ylo:.1},{yhi:.1}] z[{zlo:.2},{zhi:.2}]  .go xyz {:.2} {:.2} {:.2}",
            h.flags,
            if h.flags & 0x8 != 0 { " EXTERIOR" } else if h.flags & 0x40 != 0 { " EXT_LIT" } else { " interior" },
            m.kind, h.group_liquid, cols, rows,
            origin[0] + w[0], origin[1] + w[1], origin[2] + w[2] + 2.0,
        );
    }

    // Groups claiming each point as wet; a point claimed twice draws darker and z-fights.
    let step = 0.25f32;
    let n = (2.0 * r / step) as i32;
    let mut hist = std::collections::BTreeMap::<usize, usize>::new();
    let mut gated_hist = std::collections::BTreeMap::<usize, usize>::new();
    let mut worst: Option<(usize, [f32; 3])> = None;
    for j in 0..n {
        for i in 0..n {
            let mx = pm[0] - r + (i as f32 + 0.5) * step;
            let my = pm[1] - r + (j as f32 + 0.5) * step;
            let mut k = 0;
            let mut gated = 0;
            let mut owner: Option<usize> = None;
            for (gi, m, _) in &surfaces {
                let Some(c) = cell_at(m, mx, my) else {
                    continue;
                };
                if !m.wet[c] {
                    continue;
                }
                k += 1;
                // The gate: an unshared cell always draws; a `0x80` cell only for the first
                // (lowest-index) claimant.
                if !m.shared[c] {
                    gated += 1;
                } else if owner.is_none_or(|o| o == *gi) {
                    owner = Some(*gi);
                    gated += 1;
                }
            }
            *hist.entry(k).or_default() += 1;
            *gated_hist.entry(gated).or_default() += 1;
            if k >= 2 && worst.as_ref().is_none_or(|w| k > w.0) {
                worst = Some((k, model_to_world(rot, [mx, my, -6.48])));
            }
        }
    }
    println!(
        "\nwet-surface overlap over a {}x{} lattice at {step} yd: {hist:?}",
        n, n
    );
    println!("  with the MLIQ 0x80 shared-tile gate (lowest claimant wins): {gated_hist:?}");
    if let Some((k, w)) = worst {
        println!(
            "  deepest overlap {k} surfaces at .go xyz {:.2} {:.2} {:.2}",
            origin[0] + w[0],
            origin[1] + w[1],
            origin[2] + w[2] + 2.0
        );
    }
    Ok(())
}

fn cell_at(m: &benilla_formats::LiquidMesh, mx: f32, my: f32) -> Option<usize> {
    let (cols, rows) = (m.grid[0] as usize, m.grid[1] as usize);
    if cols < 2 || rows < 2 {
        return None;
    }
    let o = m.positions[0];
    let sx = (m.positions[1][0] - o[0]).abs().max(1e-6);
    let sy = (m.positions[cols][1] - o[1]).abs().max(1e-6);
    let (x0, y0) = (
        o[0].min(m.positions[cols * rows - 1][0]),
        o[1].min(m.positions[cols * rows - 1][1]),
    );
    let i = ((mx - x0) / sx).floor();
    let j = ((my - y0) / sy).floor();
    if i < 0.0 || j < 0.0 || i as usize >= cols - 1 || j as usize >= rows - 1 {
        return None;
    }
    Some(j as usize * (cols - 1) + i as usize)
}
