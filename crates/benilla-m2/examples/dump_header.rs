//! Dump a parsed M2's whole header and record tables for cross-checking against a known-good
//! file: `cargo run -p benilla-m2 --example dump_header <file.m2> [view0.skin]`.

use std::io::Cursor;

use benilla_m2::{parse_m2, M2Layout, M2TextureType};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_header <file.m2> [view0.skin]");
    let data = std::fs::read(&path).expect("read m2");
    let m2 = parse_m2(&mut Cursor::new(data.as_slice())).expect("parse m2");
    let m = m2.model();
    let lay = M2Layout::of(&data).expect("layout");

    println!("version: {} (wrath={})", lay.version, lay.wrath);
    println!(
        "layout strides: bone={} track={} attach={} event={} cam={} pivot={:#x}",
        lay.bone_stride,
        lay.track_stride,
        lay.attachment_stride,
        lay.event_stride,
        lay.camera_stride,
        lay.pivot_ofs
    );

    // The raw (count, offset) of every header array, to check the file's own bookkeeping.
    let arr = |f: usize| -> String {
        let c = data
            .get(f..f + 4)
            .and_then(|s| s.try_into().ok())
            .map(u32::from_le_bytes);
        let o = data
            .get(f + 4..f + 8)
            .and_then(|s| s.try_into().ok())
            .map(u32::from_le_bytes);
        match (c, o) {
            (Some(c), Some(o)) => format!("{c}@{o:#x}"),
            _ => "?".into(),
        }
    };
    println!(
        "arrays: name={} bones={} key_bone={} verts={} skin={} colors={} textures={} transparency={}",
        arr(lay.name),
        arr(lay.bones),
        arr(lay.key_bone),
        arr(lay.vertices),
        arr(lay.skin),
        arr(lay.colors),
        arr(lay.textures),
        arr(lay.transparency)
    );
    println!(
        "  tex_transforms={} idx_by_id={} materials={} bone_combos={} tex_combos={}",
        arr(lay.tex_transforms),
        arr(lay.texture_indices_by_id),
        arr(lay.materials),
        arr(lay.bone_combos),
        arr(lay.texture_combos)
    );
    println!(
        "  tex_coord={} tex_weight={} tex_transf_combos={} attachments={} attach_lookup={} events={}",
        arr(lay.tex_coord_combos),
        arr(lay.tex_weight_combos),
        arr(lay.tex_transform_combos),
        arr(lay.attachments),
        arr(lay.attach_lookup),
        arr(lay.events)
    );
    println!(
        "  cameras={} camera_lookup={} lights={} ribbon={} particle={}",
        arr(lay.cameras),
        arr(lay.camera_lookup),
        arr(lay.lights),
        arr(lay.ribbon),
        arr(lay.particle)
    );

    // Sequences (not carried on the model): dump the raw records for both layouts.
    let (sc, so) = (
        u32::from_le_bytes(data[lay.sequences..lay.sequences + 4].try_into().unwrap()),
        u32::from_le_bytes(
            data[lay.sequences + 4..lay.sequences + 8]
                .try_into()
                .unwrap(),
        ),
    );
    let seq_stride = if lay.wrath { 0x40 } else { 0x44 };
    println!("sequences: {sc}@{so:#x} (stride {seq_stride:#x})");
    for s in 0..sc as usize {
        let rec = so as usize + s * seq_stride;
        let id = u16::from_le_bytes(data[rec..rec + 2].try_into().unwrap());
        let a = u32::from_le_bytes(data[rec + 4..rec + 8].try_into().unwrap());
        if lay.wrath {
            let dur = a;
            let blend_in = u16::from_le_bytes(data[rec + 0x1c..rec + 0x1e].try_into().unwrap());
            let blend_out = u16::from_le_bytes(data[rec + 0x1e..rec + 0x20].try_into().unwrap());
            let flags = u32::from_le_bytes(data[rec + 0x0c..rec + 0x10].try_into().unwrap());
            println!(
                "  seq {s}: id={id} duration={dur} flags={flags:#x} blendIn={blend_in} blendOut={blend_out}"
            );
        } else {
            let start = a;
            let end = u32::from_le_bytes(data[rec + 8..rec + 0xc].try_into().unwrap());
            println!("  seq {s}: id={id} start={start} end={end}");
        }
    }

    println!("bones: {}", m.bones.len());
    for (i, b) in m.bones.iter().enumerate() {
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
    println!(
        "vertices: {}  first pos ({:+.3},{:+.3},{:+.3})",
        m.vertices.len(),
        m.vertices.first().map(|v| v.position.x).unwrap_or(0.0),
        m.vertices.first().map(|v| v.position.y).unwrap_or(0.0),
        m.vertices.first().map(|v| v.position.z).unwrap_or(0.0)
    );
    let tex_type = |t: M2TextureType| -> String {
        match t {
            M2TextureType::Hardcoded => "Hardcoded".into(),
            M2TextureType::Monster1 => "Monster1".into(),
            M2TextureType::Monster2 => "Monster2".into(),
            M2TextureType::Monster3 => "Monster3".into(),
            M2TextureType::Other(v) => format!("Other({v})"),
        }
    };
    println!("textures: {}", m.textures.len());
    for (i, t) in m.textures.iter().enumerate() {
        println!(
            "  tex {i}: {} {} wrap({},{})",
            tex_type(t.texture_type),
            t.filename.string.to_string_lossy(),
            t.wrap_x,
            t.wrap_y
        );
    }
    println!("materials: {}", m.materials.len());
    for (i, mat) in m.materials.iter().enumerate() {
        println!(
            "  mat {i}: flags {:#06x} blend {}",
            mat.flags.bits(),
            mat.blend_mode.bits()
        );
    }
    println!(
        "color tracks: alpha={} rgb={}  transparency tracks={}",
        m.color_alpha_tracks.len(),
        m.color_rgb_tracks.len(),
        m.transparency_tracks.len()
    );
    for (i, t) in m.transparency_tracks.iter().enumerate() {
        println!(
            "  weight {i}: interp={} gseq={:#06x} keys={:?}",
            t.interp, t.gseq, t.keys
        );
    }
    println!(
        "lookups: transparency={:?} tex_unit={:?} tex_transf={:?}",
        m.transparency_lookup, m.texture_unit_lookup, m.texture_transform_lookup
    );
    println!(
        "texture_transforms: {}  global_sequences: {:?}",
        m.texture_transforms.len(),
        m.global_sequences
    );
    println!(
        "attachments: {}  attach_lookup len={}",
        m.attachments.len(),
        m.attach_lookup.len()
    );
    for (i, a) in m.attachments.iter().enumerate() {
        println!(
            "  attach {i}: id={} bone={} pos ({:+.3},{:+.3},{:+.3})",
            a.id, a.bone, a.position[0], a.position[1], a.position[2]
        );
    }
    println!("event_markers: {}", m.event_markers.len());
    for (i, e) in m.event_markers.iter().enumerate() {
        let ident = std::str::from_utf8(&e.ident).unwrap_or("<bin>");
        println!(
            "  event {i}: {ident} bone={} pos ({:+.3},{:+.3},{:+.3})",
            e.bone, e.position[0], e.position[1], e.position[2]
        );
    }
    println!(
        "animation_lookup len={}  playable_animation_lookup len={}  pivot_attach_z={:?}",
        m.animation_lookup.len(),
        m.playable_animation_lookup.len(),
        m.pivot_attach_z
    );
    println!(
        "cameras: {}  camera_lookup len={}",
        m.cameras.len(),
        m.camera_lookup.len()
    );
    let h = &m.header;
    println!(
        "header bounds: box [{:+.2},{:+.2},{:+.2}]..[{:+.2},{:+.2},{:+.2}] r={:.2}",
        h.bounding_box_min[0],
        h.bounding_box_min[1],
        h.bounding_box_min[2],
        h.bounding_box_max[0],
        h.bounding_box_max[1],
        h.bounding_box_max[2],
        h.bounding_sphere_radius
    );
    println!(
        "collision:      box [{:+.2},{:+.2},{:+.2}]..[{:+.2},{:+.2},{:+.2}] r={:.2}",
        h.collision_box_min[0],
        h.collision_box_min[1],
        h.collision_box_min[2],
        h.collision_box_max[0],
        h.collision_box_max[1],
        h.collision_box_max[2],
        h.collision_sphere_radius
    );
    println!(
        "raw_data: tex_lookup={} tri={} verts={}",
        m.raw_data.texture_lookup_table.len(),
        m.raw_data.bounding_triangles.len(),
        m.raw_data.bounding_vertices.len()
    );

    // Optionally parse and dump the external `*.skin` (a Wrath view), cross-checked against the M2.
    if let Some(skin_path) = std::env::args().nth(2) {
        let sdata = std::fs::read(&skin_path).expect("read skin");
        let skin = m.parse_skin_file(&sdata).expect("parse skin");
        println!("\n=== .skin: {skin_path} ===");
        println!(
            "  vertex lookups: {}  triangle corners: {}  submeshes: {}  batches: {}",
            skin.indices().len(),
            skin.triangles().len(),
            skin.submeshes().len(),
            skin.batches().len()
        );
        for (i, s) in skin.submeshes().iter().enumerate() {
            println!(
                "    sub {i}: id={} tri_start={} tri_count={}",
                s.id, s.triangle_start, s.triangle_count
            );
        }
        for (i, b) in skin.batches().iter().enumerate() {
            println!(
                "    bat {i}: section={} mat={} tex_combo={} tex_coord={:#x} color={:#x} weight={} tex_count={}",
                b.skin_section_index,
                b.material_index,
                b.texture_combo_index,
                b.texture_coord_combo_index,
                b.color_index,
                b.weight_combo_index,
                b.texture_count
            );
        }
    }
}
