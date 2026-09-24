//! The shipped MCLQ depth byte per liquid kind over wet-cell vertices: its histogram, its slope
//! against the real `surface - terrain` depth, mid-ramp shore pins and what each divisor does to
//! the picture. The reference maps the byte to its depth swatch per kind, river `min(d/42, 1)` and
//! ocean `min(d/255, 1)`. Output is Blizzard data: never commit it.
//! `cargo run --release -p benilla-formats --example liquid_depth_census -- [map ...]`

use std::collections::BTreeMap;

use benilla_formats::{terrain_height_at, LiquidKind, CHUNK_SIZE};

/// MCLQ vertex-grid pitch: the 9×9 grid spans one 8-cell chunk.
const UNIT: f32 = CHUNK_SIZE / 8.0;
const GRID: usize = 9;
const CELLS: usize = 8;
const DRY_NIBBLE: u8 = 0x0f;
/// Verts under no liquid carry FLT_MAX; `is_finite()` is true for it, so gate on magnitude.
const HEIGHT_SENTINEL: f32 = 1.0e9;

/// The per-kind accumulator: the byte histogram plus the paired (byte, yard) samples.
struct Kind {
    hist: Box<[u64; 256]>,
    /// Vertices whose terrain height resolved, for the byte-to-yard fit.
    paired: u64,
    /// Σ byte·yd, Σ byte², Σ yd², Σ byte, Σ yd, for the through-origin slope and r.
    sum_by: f64,
    sum_bb: f64,
    sum_yy: f64,
    sum_b: f64,
    sum_y: f64,
    blocks: u64,
    /// Mid-ramp points `(zone, map, x, y, byte, yards)`, at most one per 2000-yd neighbourhood.
    pins: Vec<(String, String, f32, f32, u8, f32)>,
    /// Σ yards and count per byte: the depth curve a single slope hides where the byte saturates.
    yd_sum: Box<[f64; 256]>,
    yd_n: Box<[u64; 256]>,
}

impl Default for Kind {
    fn default() -> Self {
        Self {
            hist: Box::new([0; 256]),
            paired: 0,
            sum_by: 0.0,
            sum_bb: 0.0,
            sum_yy: 0.0,
            sum_b: 0.0,
            sum_y: 0.0,
            blocks: 0,
            pins: Vec::new(),
            yd_sum: Box::new([0.0; 256]),
            yd_n: Box::new([0; 256]),
        }
    }
}

impl Kind {
    fn push(&mut self, byte: u8, yards: Option<f32>) {
        self.hist[byte as usize] += 1;
        if let Some(y) = yards {
            let (b, y) = (f64::from(byte), f64::from(y));
            self.paired += 1;
            self.yd_sum[byte as usize] += y;
            self.yd_n[byte as usize] += 1;
            self.sum_by += b * y;
            self.sum_bb += b * b;
            self.sum_yy += y * y;
            self.sum_b += b;
            self.sum_y += y;
        }
    }

    fn total(&self) -> u64 {
        self.hist.iter().sum()
    }

    fn percentile(&self, p: f64) -> u8 {
        let target = (self.total() as f64 * p) as u64;
        let mut seen = 0;
        for (b, n) in self.hist.iter().enumerate() {
            seen += n;
            if seen >= target {
                return b as u8;
            }
        }
        255
    }

    /// Least-squares slope of `byte = k · yards` through the origin, and Pearson r of the pair.
    fn slope_and_r(&self) -> Option<(f64, f64)> {
        if self.paired < 32 || self.sum_yy <= 0.0 {
            return None;
        }
        let n = self.paired as f64;
        let slope = self.sum_by / self.sum_yy;
        let cov = self.sum_by / n - (self.sum_b / n) * (self.sum_y / n);
        let vb = self.sum_bb / n - (self.sum_b / n).powi(2);
        let vy = self.sum_yy / n - (self.sum_y / n).powi(2);
        let r = if vb > 0.0 && vy > 0.0 {
            cov / (vb.sqrt() * vy.sqrt())
        } else {
            0.0
        };
        Some((slope, r))
    }

    /// The through-origin slope over a byte window only: the ocean pins 83% of its vertices at
    /// 255, and a whole-range fit hides the ramp that paints the shore.
    fn slope_in(&self, lo: u8, hi: u8) -> Option<(f64, u64)> {
        let (mut sby, mut syy, mut n) = (0.0, 0.0, 0u64);
        for b in lo..=hi {
            let cnt = self.yd_n[b as usize];
            if cnt == 0 {
                continue;
            }
            // Bucket mean stands for its members: Σ b·y and Σ y² over the bucket, using the mean y.
            let ybar = self.yd_sum[b as usize] / cnt as f64;
            sby += f64::from(b) * ybar * cnt as f64;
            syy += ybar * ybar * cnt as f64;
            n += cnt;
        }
        (n >= 32 && syy > 0.0).then(|| (sby / syy, n))
    }

    /// What a divisor does to the picture: (share on the deep row, share on the shallow row, mean V).
    fn under(&self, div: f64) -> (f64, f64, f64) {
        let total = self.total().max(1) as f64;
        let (mut deep, mut shallow, mut sum) = (0.0, 0.0, 0.0);
        for (b, &n) in self.hist.iter().enumerate() {
            let v = (b as f64 / div).clamp(0.0, 1.0);
            let n = n as f64;
            if v >= 0.95 {
                deep += n;
            }
            if v <= 0.05 {
                shallow += n;
            }
            sum += v * n;
        }
        (deep / total, shallow / total, sum / total)
    }
}

fn main() -> anyhow::Result<()> {
    let maps: Vec<String> = {
        let a: Vec<String> = std::env::args().skip(1).collect();
        if a.is_empty() {
            vec!["Azeroth".into(), "Kalimdor".into()]
        } else {
            a
        }
    };

    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    let areas = benilla_formats::load_area_table_catalog(&mut chain).ok();

    let mut kinds: BTreeMap<String, Kind> = BTreeMap::new();
    for map in &maps {
        let tiles = benilla_formats::MapTiles::load(&mut chain, map)?;
        // Tile (32, 32) is world (0, 0); radius 32 covers the whole 64×64 grid.
        let coords = tiles.existing_in_radius(0.0, 0.0, 32);
        eprintln!("{map}: {} tiles", coords.len());
        for (tx, ty) in coords {
            let path = format!("World\\Maps\\{map}\\{map}_{tx}_{ty}.adt");
            let Ok(bytes) = chain.read_file(&path) else {
                continue;
            };
            // The mesh gives only the terrain; the depth bytes come off the raw parse.
            let Ok(mesh) = benilla_formats::adt_to_tile_mesh(&bytes) else {
                continue;
            };
            let mut cur = std::io::Cursor::new(bytes.as_slice());
            let Ok(benilla_adt::ParsedAdt::Root(root)) = benilla_adt::parse_adt(&mut cur) else {
                continue;
            };
            for mcnk in &root.mcnk_chunks {
                let [wx, wy, _] = mcnk.header.position;
                for block in &mcnk.liquids {
                    if block.vertices.len() < GRID * GRID {
                        continue;
                    }
                    // Kind from the majority wet-cell nibble, the same rule as `build_liquid_mesh`.
                    let mut counts = [0u32; 16];
                    let mut wet = [false; CELLS * CELLS];
                    for (c, w) in wet.iter_mut().enumerate() {
                        let nib = block.tile_flags[c] & 0x0f;
                        if nib == DRY_NIBBLE {
                            continue;
                        }
                        counts[nib as usize] += 1;
                        *w = true;
                    }
                    let Some((maj, _)) = counts.iter().enumerate().max_by_key(|(_, &c)| c) else {
                        continue;
                    };
                    if counts[maj] == 0 {
                        continue;
                    }
                    let Some(kind) = LiquidKind::from_adt_nibble(maj as u8) else {
                        continue;
                    };
                    // Magma spends the same union bytes on texture coords, not a depth.
                    if kind == LiquidKind::Magma {
                        continue;
                    }
                    let acc = kinds.entry(format!("{kind:?}")).or_default();
                    acc.blocks += 1;
                    for n in 0..GRID * GRID {
                        let (row, col) = (n / GRID, n % GRID);
                        // Only verts that corner a wet cell carry a real height and a real byte.
                        let touches = [
                            (row.wrapping_sub(1), col.wrapping_sub(1)),
                            (row.wrapping_sub(1), col),
                            (row, col.wrapping_sub(1)),
                            (row, col),
                        ]
                        .into_iter()
                        .any(|(r, c)| r < CELLS && c < CELLS && wet[r * CELLS + c]);
                        if !touches {
                            continue;
                        }
                        let h = block.vertices[n].height;
                        if !h.is_finite() || h.abs() >= HEIGHT_SENTINEL {
                            continue;
                        }
                        let x = wx - row as f32 * UNIT;
                        let y = wy - col as f32 * UNIT;
                        // Nudge off the lattice so the terrain lookup lands inside a triangle.
                        let ground = terrain_height_at(&mesh.chunks, [x - 0.01, y - 0.01, 0.0]);
                        let yards = ground.map(|g| h - g).filter(|d| *d > 0.0 && *d < 500.0);
                        let byte = block.vertices[n].depth_byte();
                        // Mid-ramp pins, the only band where the divisor shows on screen.
                        if (40..=200).contains(&byte)
                            && acc.pins.len() < 64
                            && !acc.pins.iter().any(|p| {
                                p.1 == *map && (p.2 - x).abs() < 2000.0 && (p.3 - y).abs() < 2000.0
                            })
                        {
                            let zone = benilla_formats::area_id_at(&mesh.chunks, [x, y, 0.0])
                                .and_then(|id| areas.as_ref()?.name(id).map(str::to_string))
                                .unwrap_or_else(|| "?".into());
                            acc.pins.push((
                                zone,
                                map.clone(),
                                x,
                                y,
                                byte,
                                yards.unwrap_or(f32::NAN),
                            ));
                        }
                        acc.push(byte, yards);
                    }
                }
            }
        }
    }

    println!(
        "\n=== MCLQ depth byte, over wet-cell vertices ({}) ===",
        maps.join(" + ")
    );
    for (name, k) in &kinds {
        let total = k.total();
        println!(
            "\n{name}: {} blocks, {total} wet verts ({} paired with terrain)",
            k.blocks, k.paired
        );
        print!("  byte percentiles:");
        for p in [0.01, 0.10, 0.25, 0.50, 0.75, 0.90, 0.99] {
            print!(" p{:02}={:3}", (p * 100.0) as u32, k.percentile(p));
        }
        println!(" max={}", k.hist.iter().rposition(|&n| n > 0).unwrap_or(0));
        match k.slope_and_r() {
            Some((slope, r)) => println!(
                "  byte -> yards: {slope:.2} byte/yd (r={r:.3}) => byte 42 = {:.1} yd, byte 255 = {:.1} yd",
                42.0 / slope,
                255.0 / slope
            ),
            None => println!("  byte -> yards: too few paired samples"),
        }
        for div in [42.0, 255.0] {
            let (deep, shallow, mean) = k.under(div);
            println!(
                "  V = byte/{div:<5.0} -> mean {mean:.3}, {:.1}% on the DEEP row (V>=.95), {:.1}% on the SHALLOW row (V<=.05)",
                deep * 100.0,
                shallow * 100.0
            );
        }
        if let Some((slope, n)) = k.slope_in(1, 250) {
            println!(
                "  UNSATURATED band (byte 1..250, {n} verts): {slope:.2} byte/yd => byte 42 = {:.2} yd, byte 255 = {:.2} yd",
                42.0 / slope,
                255.0 / slope
            );
        }
        print!("  byte -> mean geometric depth (surface-terrain), yd:");
        for b in [1u8, 8, 16, 32, 42, 64, 96, 128, 160, 192, 224, 254, 255] {
            let n = k.yd_n[b as usize];
            if n >= 8 {
                print!(" {b}={:.1}", k.yd_sum[b as usize] / n as f64);
            } else {
                print!(" {b}=-");
            }
        }
        println!();
        if !k.pins.is_empty() {
            println!("  shore-band pins (byte 40..200) — where the divisor is visible:");
            for (zone, map, x, y, b, yd) in k.pins.iter().take(12) {
                println!("    {map} {x:9.1} {y:9.1}  byte {b:3}  depth {yd:5.1} yd  {zone}");
            }
        }
        print!("  histogram (16 buckets of 16):");
        for b in 0..16 {
            let n: u64 = k.hist[b * 16..(b + 1) * 16].iter().sum();
            print!(" {:.1}%", n as f64 / total.max(1) as f64 * 100.0);
        }
        println!();
    }
    Ok(())
}
