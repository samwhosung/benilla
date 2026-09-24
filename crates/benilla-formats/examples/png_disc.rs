//! TEMP: what fraction of a captured minimap disc is the composite's black clear.
//! `cargo run -p benilla-formats --example png_disc -- <png> [cx cy r]` — defaults to the whole
//! image's inscribed circle, which is what the `WOW_MM_PROBE` crop is.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let img = image::open(&a[0])?.to_rgb8();
    let (w, h) = img.dimensions();
    let cx = a.get(1).map_or(w as f32 * 0.5, |s| s.parse().unwrap());
    let cy = a.get(2).map_or(h as f32 * 0.5, |s| s.parse().unwrap());
    let r = a
        .get(3)
        .map_or(w.min(h) as f32 * 0.5 - 2.0, |s| s.parse().unwrap());
    let (mut inside, mut black) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() > r {
                continue;
            }
            inside += 1;
            let p = img.get_pixel(x, y).0;
            if p[0] < 16 && p[1] < 16 && p[2] < 16 {
                black += 1;
            }
        }
    }
    println!(
        "{}: {:.2}% of the disc is clear-black ({black}/{inside})",
        a[0],
        100.0 * black as f32 / inside as f32
    );
    Ok(())
}
