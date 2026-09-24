//! `blpx <in.blp> <out.ppm>`: mip 0 as a P6 PPM, alpha composited over magenta so cutouts show; an
//! `<out>` ending in `.rgba` gets raw RGBA8 instead (width×height×4, row-major).

fn main() {
    let mut args = std::env::args().skip(1);
    let (inp, out) = (args.next().expect("in.blp"), args.next().expect("out.ppm"));
    let blp = benilla_blp::decode(&std::fs::read(&inp).expect("read")).expect("decode");
    let mip = &blp.mips[0];
    if out.ends_with(".rgba") {
        std::fs::write(&out, &mip.rgba).expect("write");
        println!("{inp} -> {out} ({}x{} raw RGBA8)", mip.width, mip.height);
        return;
    }
    let mut ppm = format!("P6\n{} {}\n255\n", mip.width, mip.height).into_bytes();
    for px in mip.rgba.as_chunks::<4>().0 {
        let a = px[3] as u32;
        for (c, mag) in px[..3].iter().zip([255u32, 0, 255]) {
            ppm.push(((*c as u32 * a + mag * (255 - a)) / 255) as u8);
        }
    }
    std::fs::write(&out, ppm).expect("write");
    println!("{inp} -> {out} ({}x{})", mip.width, mip.height);
}
