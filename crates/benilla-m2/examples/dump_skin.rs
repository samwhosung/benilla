//! Dump an M2's textures, materials and skin-0 sections and batches:
//! `cargo run -p benilla-m2 --example dump_skin <file.m2>`, extracted first with `mpqx`
//! (benilla-formats).

use std::io::Cursor;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_skin <file.m2>");
    let data = std::fs::read(&path).expect("read m2");
    let m2 = benilla_m2::parse_m2(&mut Cursor::new(data.as_slice())).expect("parse m2");
    let model = m2.model();
    println!("textures: {}", model.textures.len());
    for (i, t) in model.textures.iter().enumerate() {
        use benilla_m2::M2TextureType as T;
        let ty = match t.texture_type {
            T::Hardcoded => "hardcoded".to_string(),
            T::Monster1 => "monster1".to_string(),
            T::Monster2 => "monster2".to_string(),
            T::Monster3 => "monster3".to_string(),
            T::Other(v) => format!("other({v})"),
        };
        println!(
            "  tex {i}: type {ty} file {:?}",
            t.filename.string.to_string_lossy()
        );
    }
    println!("materials: {}", model.materials.len());
    for (i, m) in model.materials.iter().enumerate() {
        println!(
            "  mat {i}: flags {:#06x} blend {}",
            m.flags.bits(),
            m.blend_mode.bits()
        );
    }
    println!(
        "colors: {} transparency tracks: {} (lookup {:?})",
        model.color_alpha_tracks.len(),
        model.transparency_tracks.len(),
        model.transparency_lookup
    );
    println!(
        "texture_lookup_table: {:?}",
        model.raw_data.texture_lookup_table
    );
    let skin = model.parse_embedded_skin(&data, 0).expect("skin 0");
    println!(
        "skin 0: {} sections, {} batches",
        skin.submeshes().len(),
        skin.batches().len()
    );
    for (i, s) in skin.submeshes().iter().enumerate() {
        println!(
            "  section {i}: id {} tri_start {} tri_count {}",
            s.id, s.triangle_start, s.triangle_count
        );
    }
    for (i, b) in skin.batches().iter().enumerate() {
        println!(
            "  batch {i}: section {} tex_combo {} mat {} color {} weight_combo {} tex_count {} uvanim {}",
            b.skin_section_index,
            b.texture_combo_index,
            b.material_index,
            b.color_index,
            b.weight_combo_index,
            b.texture_count,
            b.texture_transform_combo_index
        );
    }
}
