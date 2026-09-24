//! A BLP2 with a stale `alpha_type` still decodes: `particles/dust1.blp` is palettized with
//! `alpha_bits == 0` and `alpha_type == 2`, and `alpha_bits` alone governs its alpha.

use benilla_formats::{blp_to_rgba, open_chain};

#[test]
fn decodes_blp2_unknown_alpha_type() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("particles\\dust1.blp")
        .expect("read particles/dust1.blp");

    let (w, h, rgba) = blp_to_rgba(&bytes).expect("dust1.blp must decode despite alpha_type == 2");
    assert!(w > 0 && h > 0, "decoded dimensions {w}x{h}");
    assert_eq!(
        rgba.len(),
        (w * h * 4) as usize,
        "RGBA8 buffer matches dimensions"
    );
}
