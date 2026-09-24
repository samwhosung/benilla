//! Dump an M2's bones and vertices (position, UV, bone binding):
//! `cargo run -p benilla-m2 --example dump_verts <file.m2>`.

use std::io::Cursor;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_verts <file.m2>");
    let data = std::fs::read(&path).expect("read m2");
    let m2 = benilla_m2::parse_m2(&mut Cursor::new(data.as_slice())).expect("parse m2");
    let model = m2.model();
    println!("bones: {}", model.bones.len());
    for (i, b) in model.bones.iter().enumerate() {
        println!(
            "  bone {i}: key {} parent {} flags {:#06x} pivot ({:+.3},{:+.3},{:+.3})",
            b.key_bone,
            b.parent,
            b.flags.bits(),
            b.pivot.x,
            b.pivot.y,
            b.pivot.z
        );
    }
    println!("vertices: {}", model.vertices.len());
    for (i, v) in model.vertices.iter().enumerate() {
        println!(
            "  v {i:3}: pos ({:+.3},{:+.3},{:+.3}) uv ({:+.3},{:+.3}) bones {:?} w {:?}",
            v.position.x,
            v.position.y,
            v.position.z,
            v.tex_coords.x,
            v.tex_coords.y,
            v.bone_indices,
            v.bone_weights
        );
    }
}
