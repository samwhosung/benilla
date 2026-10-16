use super::*;

// The header through the event table at 0x114, as far as `parse_m2` walks.
const HEADER_LEN: usize = 284;

// v256 header offsets of the M2Arrays under test.
const OFS_PLAYABLE_ANIM_LOOKUP: usize = 44;
const OFS_TEX_ANIM: usize = 0x74;
const OFS_TEX_UNIT_LOOKUP: usize = 0x9c;
const OFS_TRANS_LOOKUP: usize = 0xa4;
const OFS_TEX_ANIM_LOOKUP: usize = 0xac;
const OFS_BONES: usize = 52;
const OFS_VERTICES: usize = 68;
const OFS_VIEWS: usize = 76;
const OFS_ATTACHMENTS: usize = 260;
const OFS_EVENTS: usize = 276;

/// A v256 MD20 header with every M2Array empty; tests patch arrays with [`set_arr`].
fn header() -> Vec<u8> {
    let mut b = vec![0u8; HEADER_LEN];
    b[0..4].copy_from_slice(b"MD20");
    b[4..8].copy_from_slice(&256u32.to_le_bytes());
    b
}

fn set_arr(b: &mut [u8], ofs: usize, count: u32, offset: u32) {
    b[ofs..ofs + 4].copy_from_slice(&count.to_le_bytes());
    b[ofs + 4..ofs + 8].copy_from_slice(&offset.to_le_bytes());
}

fn parse(b: &[u8]) -> Result<M2Format> {
    parse_m2(&mut Cursor::new(b))
}

#[test]
fn minimal_header_parses_to_an_empty_model() {
    let b = header();
    let fmt = parse(&b).expect("an all-zero-array header parses");
    let m = fmt.model();
    assert!(m.vertices.is_empty());
    assert!(m.textures.is_empty());
    assert!(m.materials.is_empty());
    assert!(m.bones.is_empty());
    assert!(m.color_alpha_tracks.is_empty());
    assert!(m.color_rgb_tracks.is_empty());
    assert!(m.transparency_tracks.is_empty());
    assert!(m.transparency_lookup.is_empty());
    assert!(m.raw_data.texture_lookup_table.is_empty());
    assert!(m.raw_data.bounding_triangles.is_empty());
    assert!(m.raw_data.bounding_vertices.is_empty());
    assert!(m.attachments.is_empty());
    assert!(m.playable_animation_lookup.is_empty());
    assert!(m.texture_transforms.is_empty());
    assert!(m.texture_transform_lookup.is_empty());
}

/// One v256 M2Track (stride 0x1c): interp@0, gseq@2, timestamps M2Array@0x0c, values M2Array@0x14.
fn m2track(interp: u16, gseq: u16, ts: (u32, u32), vals: (u32, u32)) -> Vec<u8> {
    let mut t = vec![0u8; 0x1c];
    t[0..2].copy_from_slice(&interp.to_le_bytes());
    t[2..4].copy_from_slice(&gseq.to_le_bytes());
    t[0x0c..0x10].copy_from_slice(&ts.0.to_le_bytes());
    t[0x10..0x14].copy_from_slice(&ts.1.to_le_bytes());
    t[0x14..0x18].copy_from_slice(&vals.0.to_le_bytes());
    t[0x18..0x1c].copy_from_slice(&vals.1.to_le_bytes());
    t
}

/// `TanarisTrollGate.m2` swaps its intact and burnt gates with these ±1 keys. Both fix16 records
/// are checked: the M2Color alpha (header `0x54`, track @ `+0x1c`) and the M2TextureWeight.
#[test]
fn a_negative_fix16_key_decodes_signed() {
    let mut b = header();
    let ts_ofs = b.len() as u32;
    b.extend(0u32.to_le_bytes());
    b.extend(333u32.to_le_bytes());
    let val_ofs = b.len() as u32;
    b.extend(0x7fffu16.to_le_bytes()); // +1.0, drawn
    b.extend(0x8001u16.to_le_bytes()); // −1.0, hidden
                                       // One M2Color: a keyless RGB track, then the alpha track.
    let color_ofs = b.len() as u32;
    b.extend(m2track(0, 0xffff, (0, 0), (0, 0)));
    b.extend(m2track(1, 0xffff, (2, ts_ofs), (2, val_ofs)));
    set_arr(&mut b, 0x54, 1, color_ofs);
    // One M2TextureWeight with the same keys.
    let weight_ofs = b.len() as u32;
    b.extend(m2track(1, 0xffff, (2, ts_ofs), (2, val_ofs)));
    set_arr(&mut b, 0x64, 1, weight_ofs);

    let fmt = parse(&b).expect("a colour + weight fixture parses");
    let m = fmt.model();
    let keys = |t: &M2ScalarTrack| t.keys.iter().map(|&(_, v)| v).collect::<Vec<_>>();
    let alpha = keys(&m.color_alpha_tracks[0]);
    assert!(
        (alpha[0] - 1.0).abs() < 1e-4,
        "0x7fff is +1.0 (got {})",
        alpha[0]
    );
    assert!(
        (alpha[1] + 1.0).abs() < 1e-4,
        "0x8001 is −1.0 — read unsigned it would be +1.0 and the batch would draw (got {})",
        alpha[1]
    );
    let weight = keys(&m.transparency_tracks[0]);
    assert!(
        (weight[1] + 1.0).abs() < 1e-4,
        "the transparency weight decodes signed too (got {})",
        weight[1]
    );
}

#[test]
fn texture_transform_translation_track_decodes() {
    let mut b = header();
    let ts_ofs = b.len() as u32;
    b.extend(0u32.to_le_bytes());
    b.extend(1000u32.to_le_bytes());
    let val_ofs = b.len() as u32;
    for v in [[0.0f32, 0.0, 0.0], [0.5, 1.0, 0.0]] {
        for c in v {
            b.extend(c.to_le_bytes());
        }
    }
    // One M2TextureTransform: a 2-key gseq-0 translation, then keyless rotation and scaling.
    let rec_ofs = b.len() as u32;
    b.extend(m2track(1, 0, (2, ts_ofs), (2, val_ofs)));
    b.extend(m2track(0, 0xffff, (0, 0), (0, 0)));
    b.extend(m2track(0, 0xffff, (0, 0), (0, 0)));
    set_arr(&mut b, OFS_TEX_ANIM, 1, rec_ofs);
    let lk_ofs = b.len() as u32;
    b.extend(0u16.to_le_bytes());
    set_arr(&mut b, OFS_TEX_ANIM_LOOKUP, 1, lk_ofs);

    let fmt = parse(&b).expect("header with one texture transform parses");
    let m = fmt.model();
    assert_eq!(m.texture_transform_lookup, vec![0]);
    assert!(m.transparency_lookup.is_empty());
    assert_eq!(m.texture_transforms.len(), 1);
    let t = &m.texture_transforms[0];
    assert_eq!(t.translation.interp, 1);
    assert_eq!(t.translation.gseq, 0);
    assert_eq!(
        t.translation.keys,
        vec![(0, [0.0, 0.0, 0.0]), (1000, [0.5, 1.0, 0.0])]
    );
    assert!(t.translation.constant().is_none());
    assert!(t.rotation.keys.is_empty());
    assert!(t.scaling.keys.is_empty());
}

/// Shaped like StormwindMagePortal01's header: [0] at 0x9c, [0,1,2,3] at 0xa4, [0xffff] at 0xac.
#[test]
fn transparency_lookup_reads_0xa4_not_the_texture_unit_lookup_at_0x9c() {
    let mut b = header();
    let unit_ofs = b.len() as u32;
    b.extend(0u16.to_le_bytes());
    set_arr(&mut b, OFS_TEX_UNIT_LOOKUP, 1, unit_ofs);
    let trans_ofs = b.len() as u32;
    for v in [0u16, 1, 2, 3] {
        b.extend(v.to_le_bytes());
    }
    set_arr(&mut b, OFS_TRANS_LOOKUP, 4, trans_ofs);
    let ta_ofs = b.len() as u32;
    b.extend(0xffffu16.to_le_bytes());
    set_arr(&mut b, OFS_TEX_ANIM_LOOKUP, 1, ta_ofs);

    let fmt = parse(&b).expect("header with all three lookup tables parses");
    let m = fmt.model();
    assert_eq!(m.transparency_lookup, vec![0, 1, 2, 3]);
    assert_eq!(m.texture_transform_lookup, vec![0xffff]);
}

#[test]
fn playable_animation_lookup_decodes_low16_high16() {
    let mut b = header();
    let pal_ofs = b.len() as u32;
    // Row 1 is HumanMale's `playableAnimationLookup[6] = 0x00030001`: id 1, code 3.
    b.extend(0u32.to_le_bytes());
    b.extend(0x0003_0001u32.to_le_bytes());
    set_arr(&mut b, OFS_PLAYABLE_ANIM_LOOKUP, 2, pal_ofs);

    let fmt = parse(&b).expect("header with a playable-animation-lookup table parses");
    let pal = &fmt.model().playable_animation_lookup;
    assert_eq!(pal.len(), 2);
    assert_eq!(pal[0].resolved_id, 0);
    assert_eq!(pal[0].dir_flags, 0);
    assert_eq!(pal[1].resolved_id, 1);
    assert_eq!(pal[1].dir_flags, 3);
}

fn bone_record(pivot: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 108];
    b[96..100].copy_from_slice(&pivot[0].to_le_bytes());
    b[100..104].copy_from_slice(&pivot[1].to_le_bytes());
    b[104..108].copy_from_slice(&pivot[2].to_le_bytes());
    b
}

fn attachment_record(id: u32, bone: u32, position: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 48]; // id, bone, position (20 bytes), the skipped 28-byte track
    b[0..4].copy_from_slice(&id.to_le_bytes());
    b[4..8].copy_from_slice(&bone.to_le_bytes());
    b[8..12].copy_from_slice(&position[0].to_le_bytes());
    b[12..16].copy_from_slice(&position[1].to_le_bytes());
    b[16..20].copy_from_slice(&position[2].to_le_bytes());
    b
}

#[test]
fn attachments_parse_and_skip_out_of_range_records() {
    let mut b = header();
    // Two bones, so a valid bone index is 0 or 1.
    let bones_ofs = b.len() as u32;
    b.extend(bone_record([0.0, 0.0, 0.0]));
    b.extend(bone_record([1.0, 1.0, 1.0]));
    set_arr(&mut b, OFS_BONES, 2, bones_ofs);

    let att_ofs = b.len() as u32;
    b.extend(attachment_record(1, 0, [1.0, 2.0, 3.0])); // valid
    b.extend(attachment_record(70_000, 0, [0.0, 0.0, 0.0])); // id overflows u16 -> skipped
    b.extend(attachment_record(2, 5, [9.0, 9.0, 9.0])); // bone out of range -> skipped
    set_arr(&mut b, OFS_ATTACHMENTS, 3, att_ofs);

    let fmt = parse(&b).expect("header with bones + attachments parses");
    let atts = &fmt.model().attachments;
    assert_eq!(atts.len(), 1, "only the in-range record survives");
    assert_eq!(atts[0].id, 1);
    assert_eq!(atts[0].bone, 0);
    assert_eq!(atts[0].position, [1.0, 2.0, 3.0]);
}

fn event_record(ident: &[u8; 4], bone: u32, position: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 44]; // ident, data, bone, position (24 bytes), the skipped 20-byte track
    b[0..4].copy_from_slice(ident);
    b[8..12].copy_from_slice(&bone.to_le_bytes());
    b[12..16].copy_from_slice(&position[0].to_le_bytes());
    b[16..20].copy_from_slice(&position[1].to_le_bytes());
    b[20..24].copy_from_slice(&position[2].to_le_bytes());
    b
}

#[test]
fn event_markers_parse_and_skip_out_of_range_records() {
    let mut b = header();
    let bones_ofs = b.len() as u32;
    b.extend(bone_record([0.0, 0.0, 0.0]));
    set_arr(&mut b, OFS_BONES, 1, bones_ofs);

    let ev_ofs = b.len() as u32;
    b.extend(event_record(b"$CSL", 0, [1.0, 2.0, 3.0])); // valid
    b.extend(event_record(b"$CSR", 5, [9.0, 9.0, 9.0])); // bone out of range -> skipped
    set_arr(&mut b, OFS_EVENTS, 2, ev_ofs);

    let fmt = parse(&b).expect("header with bones + events parses");
    let markers = &fmt.model().event_markers;
    assert_eq!(markers.len(), 1, "only the in-range record survives");
    assert_eq!(&markers[0].ident, b"$CSL");
    assert_eq!(markers[0].bone, 0);
    assert_eq!(markers[0].position, [1.0, 2.0, 3.0]);
}

#[test]
fn one_vertex_decodes_all_fields() {
    let mut b = header();
    set_arr(&mut b, OFS_VERTICES, 1, HEADER_LEN as u32);
    let mut vert = vec![0u8; 48];
    vert[0..4].copy_from_slice(&1.0f32.to_le_bytes());
    vert[4..8].copy_from_slice(&2.0f32.to_le_bytes());
    vert[8..12].copy_from_slice(&3.0f32.to_le_bytes());
    vert[12..16].copy_from_slice(&[10, 20, 30, 40]);
    vert[16..20].copy_from_slice(&[1, 2, 3, 4]);
    vert[20..24].copy_from_slice(&4.0f32.to_le_bytes());
    vert[24..28].copy_from_slice(&5.0f32.to_le_bytes());
    vert[28..32].copy_from_slice(&6.0f32.to_le_bytes());
    vert[32..36].copy_from_slice(&0.5f32.to_le_bytes());
    vert[36..40].copy_from_slice(&0.25f32.to_le_bytes());
    b.extend(vert);

    let fmt = parse(&b).expect("one full vertex record parses");
    let v = &fmt.model().vertices[0];
    assert_eq!((v.position.x, v.position.y, v.position.z), (1.0, 2.0, 3.0));
    assert_eq!(v.bone_weights, [10, 20, 30, 40]);
    assert_eq!(v.bone_indices, [1, 2, 3, 4]);
    assert_eq!((v.normal.x, v.normal.y, v.normal.z), (4.0, 5.0, 6.0));
    assert_eq!((v.tex_coords.x, v.tex_coords.y), (0.5, 0.25));
}

#[test]
fn hostile_vertex_count_errs_cleanly_not_oom() {
    // `u32::MAX` vertices must fail at the first read, not reserve `u32::MAX * 48` bytes.
    let mut b = header();
    set_arr(&mut b, OFS_VERTICES, u32::MAX, HEADER_LEN as u32);
    assert!(matches!(parse(&b), Err(Error::Truncated)));
}

#[test]
fn hostile_attachment_count_errs_cleanly_not_oom() {
    // `emitted_at` is sized from the count, so a table the file cannot hold is refused first.
    let mut b = header();
    set_arr(&mut b, OFS_ATTACHMENTS, u32::MAX, HEADER_LEN as u32);
    assert!(matches!(parse(&b), Err(Error::Truncated)));
}

#[test]
fn hostile_track_record_counts_walk_only_what_the_file_holds() {
    // The colour (0x54), weight (0x64) and texture-transform (0x74) readers never fail, so the
    // file bounds their loops, not the count.
    let mut b = header();
    set_arr(&mut b, 0x54, u32::MAX, HEADER_LEN as u32);
    set_arr(&mut b, 0x64, u32::MAX, HEADER_LEN as u32);
    set_arr(&mut b, OFS_TEX_ANIM, u32::MAX, HEADER_LEN as u32);
    let fmt = parse(&b).expect("lenient blocks never fail the parse");
    let m = fmt.model();
    assert!(m.color_alpha_tracks.is_empty());
    assert!(m.color_rgb_tracks.is_empty());
    assert!(m.transparency_tracks.is_empty());
    assert!(m.texture_transforms.is_empty());
    // A count that overshoots a partly-present block yields exactly the whole records in it.
    let mut b = header();
    let ofs = b.len() as u32;
    b.extend(m2track(0, 0xffff, (0, 0), (0, 0))); // one whole 0x1c weight record …
    b.extend(vec![0u8; 0x10]); // … and part of a second
    set_arr(&mut b, 0x64, 5, ofs);
    assert_eq!(parse(&b).unwrap().model().transparency_tracks.len(), 1);
}

#[test]
fn truncated_vertex_record_errs_cleanly() {
    let mut b = header();
    set_arr(&mut b, OFS_VERTICES, 1, HEADER_LEN as u32);
    b.extend(vec![0u8; 40]); // one vertex declared, but only 40 of its 48 bytes are present
    assert!(matches!(parse(&b), Err(Error::Truncated)));
}

#[test]
fn hostile_shapes_do_not_panic() {
    assert!(matches!(parse(&[]), Err(Error::NotMd20)));
    assert!(matches!(parse(&[0u8; 7]), Err(Error::NotMd20)));
    let mut b = vec![0u8; HEADER_LEN];
    b[0..4].copy_from_slice(b"MD20");
    b[4..8].copy_from_slice(&999u32.to_le_bytes());
    assert!(matches!(parse(&b), Err(Error::UnsupportedVersion(999))));
    assert!(matches!(
        parse(&header()[..HEADER_LEN - 4]),
        Err(Error::Truncated)
    ));
}

/// An MD20 file with one embedded skin profile: three indices, three triangle indices, one
/// submesh, one batch.
fn model_with_one_skin() -> Vec<u8> {
    let mut b = header();
    let vp = HEADER_LEN as u32; // M2View header right after the fixed header
    set_arr(&mut b, OFS_VIEWS, 1, vp);
    b.extend(vec![0u8; 44]); // M2View header, patched below by absolute offset
    let idx_ofs = b.len() as u32;
    b.extend([1u16, 2, 3].iter().flat_map(|v| v.to_le_bytes()));
    let tri_ofs = b.len() as u32;
    b.extend([4u16, 5, 6].iter().flat_map(|v| v.to_le_bytes()));
    let sub_ofs = b.len() as u32;
    let mut section = vec![0u8; 32]; // v256 (<260) M2SkinSection stride
    section[0..2].copy_from_slice(&7u16.to_le_bytes()); // skinSectionId
    section[8..10].copy_from_slice(&0u16.to_le_bytes()); // triangle_start
    section[10..12].copy_from_slice(&3u16.to_le_bytes()); // triangle_count
    b.extend(section);
    let bat_ofs = b.len() as u32;
    let mut batch = vec![0u8; 24];
    batch[4..6].copy_from_slice(&0u16.to_le_bytes()); // skin_section_index
    batch[8..10].copy_from_slice(&11u16.to_le_bytes()); // color_index
    batch[0x0a..0x0c].copy_from_slice(&2u16.to_le_bytes()); // material_index
    batch[0x0e..0x10].copy_from_slice(&1u16.to_le_bytes()); // texture_count
    batch[0x10..0x12].copy_from_slice(&5u16.to_le_bytes()); // texture_combo_index
    batch[0x14..0x16].copy_from_slice(&9u16.to_le_bytes()); // weight_combo_index
    b.extend(batch);

    set_arr(&mut b, vp as usize, 3, idx_ofs);
    set_arr(&mut b, vp as usize + 8, 3, tri_ofs);
    set_arr(&mut b, vp as usize + 24, 1, sub_ofs);
    set_arr(&mut b, vp as usize + 32, 1, bat_ofs);
    b
}

#[test]
fn embedded_skin_decodes_geometry_and_batches() {
    let b = model_with_one_skin();
    let fmt = parse(&b).expect("synthetic one-skin model parses");
    let skin = fmt
        .model()
        .parse_embedded_skin(&b, 0)
        .expect("skin profile 0 decodes");
    assert_eq!(*skin.indices(), vec![1, 2, 3]);
    assert_eq!(*skin.triangles(), vec![4, 5, 6]);
    assert_eq!(skin.submeshes().len(), 1);
    assert_eq!(skin.submeshes()[0].id, 7);
    assert_eq!(skin.submeshes()[0].triangle_count, 3);
    assert_eq!(skin.batches().len(), 1);
    assert_eq!(skin.batches()[0].material_index, 2);
    assert_eq!(skin.batches()[0].color_index, 11);
    assert_eq!(skin.batches()[0].texture_combo_index, 5);
    assert_eq!(skin.batches()[0].weight_combo_index, 9);
}

#[test]
fn embedded_skin_out_of_range_index_errs() {
    let b = model_with_one_skin();
    let fmt = parse(&b).expect("synthetic one-skin model parses");
    assert!(matches!(
        fmt.model().parse_embedded_skin(&b, 1),
        Err(Error::Truncated)
    ));
}

#[test]
fn hostile_submesh_count_errs_cleanly_not_oom() {
    // The vertex case, in `parse_embedded_skin`'s M2View arrays.
    let mut b = model_with_one_skin();
    let vp = HEADER_LEN; // M2View header offset (see `model_with_one_skin`)
    let end = b.len() as u32;
    set_arr(&mut b, vp + 24, u32::MAX, end); // submeshes M2Array, offset = end of file
    let fmt = parse(&b).expect("header + view arrays are still well-formed");
    assert!(matches!(
        fmt.model().parse_embedded_skin(&b, 0),
        Err(Error::Truncated)
    ));
}

// --- cameras (header 0x124/0x128) + the cubic samplers ---------------------------------------

/// A header long enough for `cameras`@`0x124` and `cameraLookup`@`0x12c`, past [`HEADER_LEN`].
const CAM_HEADER_LEN: usize = 0x134;
const OFS_CAMERAS: usize = 0x124;
const OFS_CAMERA_LOOKUP: usize = 0x12c;

fn cam_header() -> Vec<u8> {
    let mut b = header();
    b.resize(CAM_HEADER_LEN, 0);
    b
}

/// One fixture key: `(ms, value, in_tan, out_tan)`.
type SplineKeyFixture = (u32, [f32; 3], [f32; 3], [f32; 3]);

/// Append a cubic `C3Vector` track's `{value, inTan, outTan}` keys and return the track's 28 bytes.
fn spline_vec3_track(b: &mut Vec<u8>, interp: u16, keys: &[SplineKeyFixture]) -> Vec<u8> {
    let times_ofs = b.len() as u32;
    for (ms, ..) in keys {
        b.extend_from_slice(&ms.to_le_bytes());
    }
    let vals_ofs = b.len() as u32;
    for (_, v, i, o) in keys {
        for triple in [v, i, o] {
            for c in triple {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    let mut t = Vec::new();
    t.extend_from_slice(&interp.to_le_bytes());
    t.extend_from_slice(&0xffffu16.to_le_bytes()); // gseq: an ordinary sequence-timeline track
    t.extend_from_slice(&0u32.to_le_bytes()); // ranges count
    t.extend_from_slice(&0u32.to_le_bytes()); // ranges offset
    t.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    t.extend_from_slice(&times_ofs.to_le_bytes());
    t.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    t.extend_from_slice(&vals_ofs.to_le_bytes());
    t
}

/// A one-camera model whose position track runs X from 0 to 3 over 1000 ms, with
/// `outTan[k0] = 1` and `inTan[k1] = 2`.
fn model_with_one_camera(interp: u16) -> Vec<u8> {
    let mut b = cam_header();
    let rec = b.len() as u32;
    b.resize(rec as usize + 0x7c, 0);
    // fov / far / near, then the two bases.
    b[rec as usize + 0x04..rec as usize + 0x08]
        .copy_from_slice(&std::f32::consts::FRAC_PI_4.to_le_bytes());
    b[rec as usize + 0x08..rec as usize + 0x0c].copy_from_slice(&27.777779f32.to_le_bytes());
    b[rec as usize + 0x0c..rec as usize + 0x10].copy_from_slice(&0.22222222f32.to_le_bytes());
    for (ofs, base) in [(0x2c, [10.0f32, 20.0, 30.0]), (0x54, [40.0f32, 50.0, 60.0])] {
        for (i, c) in base.iter().enumerate() {
            let at = rec as usize + ofs + i * 4;
            b[at..at + 4].copy_from_slice(&c.to_le_bytes());
        }
    }
    let track = spline_vec3_track(
        &mut b,
        interp,
        &[
            (0, [0.0; 3], [0.0; 3], [1.0, 0.0, 0.0]),
            (1000, [3.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0; 3]),
        ],
    );
    let at = rec as usize + 0x10;
    b[at..at + 0x1c].copy_from_slice(&track);
    // The lookup: one entry, pointing at camera 0.
    let lk = b.len() as u32;
    b.extend_from_slice(&0u16.to_le_bytes());
    set_arr(&mut b, OFS_CAMERAS, 1, rec);
    set_arr(&mut b, OFS_CAMERA_LOOKUP, 1, lk);
    b
}

#[test]
fn camera_record_fields_and_lookup_parse() {
    let b = model_with_one_camera(2);
    let cams = parse_cameras(&b);
    assert_eq!(cams.len(), 1);
    let c = &cams[0];
    assert!((c.fov - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    assert!((c.far_clip - 27.777779).abs() < 1e-4);
    assert!((c.near_clip - 0.22222222).abs() < 1e-7);
    assert_eq!(c.position_base, [10.0, 20.0, 30.0]);
    assert_eq!(c.target_base, [40.0, 50.0, 60.0]);
    assert_eq!(c.positions.keys.len(), 2);
    assert_eq!(parse_camera_lookup(&b), vec![0]);
    // …and the whole-model parse carries the same records.
    let fmt = parse(&b).expect("fixture parses");
    assert_eq!(fmt.model().cameras.len(), 1);
    assert_eq!(fmt.model().camera_lookup, vec![0]);
}

#[test]
fn cubic_sampler_takes_all_four_interp_legs() {
    // At the midpoint, hand-computed from the bases in `M2Track::sample_ms`:
    //   step   → value[k0]                                              = 0
    //   linear → 0 + (3−0)·0.5                                          = 1.5
    //   Bézier → 0.125·0 + 0.375·1 + 0.375·2 + 0.125·3                  = 1.5
    //   Hermite→ 0.5·0 + 0.125·1 + 0.5·3 + (−0.125)·2                   = 1.375
    for (interp, want) in [(0u16, 0.0f32), (1, 1.5), (2, 1.5), (3, 1.375)] {
        let b = model_with_one_camera(interp);
        let cam = parse_cameras(&b).remove(0);
        let got = cam.positions.sample_ms(500).expect("keyed track samples")[0];
        assert!(
            (got - want).abs() < 1e-5,
            "interp {interp}: got {got}, want {want}"
        );
        // End-clamped at both ends, on every leg.
        assert_eq!(cam.positions.sample_ms(0).unwrap()[0], 0.0);
        assert_eq!(cam.positions.sample_ms(9_999).unwrap()[0], 3.0);
    }
}

#[test]
fn a_truncated_camera_record_is_dropped_not_half_read() {
    let mut b = model_with_one_camera(2);
    // Claim two records where only one fits: the second must not appear with default tracks.
    let (rec, _) = (
        u32::from_le_bytes(b[OFS_CAMERAS + 4..OFS_CAMERAS + 8].try_into().unwrap()),
        0,
    );
    set_arr(&mut b, OFS_CAMERAS, 2, rec);
    assert_eq!(parse_cameras(&b).len(), 1);
}

// --- Wrath (264) fixtures: the narrower header, 88-byte bones, nested tracks ------------------
// A v264 file is 20 bytes narrower than v256: no playable-animation lookup, a 4-byte skin count,
// no texture flipbooks. Offsets below are the Wrath (not v256) ones.

const WRATH_HEADER_LEN: usize = 0x110;
const WRATH_SEQS: usize = 0x1c;
const WRATH_BONES: usize = 0x2c;
const WRATH_TRANSPARENCY: usize = 0x58;
const WRATH_ATTACHMENTS: usize = 0xf0;
const WRATH_EVENTS: usize = 0x100;

fn wrath_header() -> Vec<u8> {
    let mut b = vec![0u8; WRATH_HEADER_LEN];
    b[0..4].copy_from_slice(b"MD20");
    b[4..8].copy_from_slice(&264u32.to_le_bytes());
    b
}

/// A Wrath bone: 88 bytes, the pivot `C3` at `0x4c` (a name CRC and 20-byte tracks shift it in).
fn wrath_bone_record(pivot: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 88];
    b[0x4c..0x50].copy_from_slice(&pivot[0].to_le_bytes());
    b[0x50..0x54].copy_from_slice(&pivot[1].to_le_bytes());
    b[0x54..0x58].copy_from_slice(&pivot[2].to_le_bytes());
    b
}

/// A Wrath nested fix16 `M2Track` (20-byte header, then one outer/inner array pair per sequence)
/// for a single sequence. Returns the track's file offset.
fn wrath_fix16_track(b: &mut Vec<u8>, interp: u16, gseq: u16, keys: &[(u32, i16)]) -> u32 {
    let head = b.len() as u32;
    let n = keys.len() as u32;
    let ts_out = head + 20;
    let vs_out = head + 28;
    let ts_in = head + 36;
    let vs_in = ts_in + n * 4;
    let mut h = vec![0u8; 20];
    h[0..2].copy_from_slice(&interp.to_le_bytes());
    h[2..4].copy_from_slice(&gseq.to_le_bytes());
    h[4..8].copy_from_slice(&1u32.to_le_bytes());
    h[8..12].copy_from_slice(&ts_out.to_le_bytes());
    h[12..16].copy_from_slice(&1u32.to_le_bytes());
    h[16..20].copy_from_slice(&vs_out.to_le_bytes());
    b.extend_from_slice(&h);
    b.extend_from_slice(&n.to_le_bytes());
    b.extend_from_slice(&ts_in.to_le_bytes());
    b.extend_from_slice(&n.to_le_bytes());
    b.extend_from_slice(&vs_in.to_le_bytes());
    for (ms, _) in keys {
        b.extend_from_slice(&ms.to_le_bytes());
    }
    for (_, v) in keys {
        b.extend_from_slice(&(*v as u16).to_le_bytes());
    }
    head
}

#[test]
fn wrath_minimal_header_parses_empty_model() {
    let b = wrath_header();
    let fmt = parse(&b).expect("an all-zero Wrath header parses");
    let m = fmt.model();
    assert!(m.bones.is_empty());
    assert!(m.vertices.is_empty());
    assert!(
        m.playable_animation_lookup.is_empty(),
        "a Wrath file has no playable-animation lookup"
    );
}

#[test]
fn wrath_bone_parses_88byte_record_pivot_at_0x4c() {
    let mut b = wrath_header();
    let bones_ofs = b.len() as u32;
    b.extend(wrath_bone_record([1.5, -2.5, 3.5]));
    set_arr(&mut b, WRATH_BONES, 1, bones_ofs);
    let fmt = parse(&b).expect("a Wrath bone parses");
    let m = fmt.model();
    assert_eq!(m.bones.len(), 1);
    assert_eq!(
        m.bones[0].pivot,
        C3 {
            x: 1.5,
            y: -2.5,
            z: 3.5
        }
    );
}

#[test]
fn wrath_nested_fix16_track_flattens() {
    let mut b = wrath_header();
    let rec_ofs = wrath_fix16_track(&mut b, 1, 0xffff, &[(0, 0), (1000, 0x7fff)]);
    set_arr(&mut b, WRATH_TRANSPARENCY, 1, rec_ofs);
    let fmt = parse(&b).expect("a Wrath weight track parses");
    let m = fmt.model();
    assert_eq!(m.transparency_tracks.len(), 1);
    let t = &m.transparency_tracks[0];
    assert_eq!(t.interp, 1);
    assert_eq!(t.keys.len(), 2);
    let keys: Vec<f32> = t.keys.iter().map(|&(_, v)| v).collect();
    assert!(
        (keys[0] - 0.0).abs() < 1e-6,
        "fix16 0 decodes to 0.0 (got {})",
        keys[0]
    );
    assert!(
        (keys[1] - 1.0).abs() < 1e-6,
        "fix16 0x7fff decodes to 1.0 (got {})",
        keys[1]
    );
}

/// A two-sequence track: each sequence's relative keys are rebased onto the shared timeline by
/// the accumulated band start, so sequence 1's key at relative 0 lands at absolute 1000.
#[test]
fn wrath_sequence_track_rebases_by_band_start() {
    let mut b = wrath_header();
    // Two sequences: durations 1000 and 500, so band_starts = [0, 1000].
    let seq_ofs = b.len() as u32;
    for dur in [1000u32, 500] {
        let mut rec = vec![0u8; 0x40];
        rec[4..8].copy_from_slice(&dur.to_le_bytes());
        b.extend(rec);
    }
    set_arr(&mut b, WRATH_SEQS, 2, seq_ofs);
    // One weight track, one clip per sequence, each a single fix16 key at relative ms 0.
    let head = b.len() as u32;
    let (outer_ts, outer_vs, inner_ts) = (head + 20, head + 36, head + 52);
    let (ts0, ts1) = (inner_ts, inner_ts + 4);
    let (vs0, vs1) = (inner_ts + 8, inner_ts + 10);
    let mut h = vec![0u8; 20];
    h[0..2].copy_from_slice(&1u16.to_le_bytes());
    h[2..4].copy_from_slice(&0xffffu16.to_le_bytes());
    h[4..8].copy_from_slice(&2u32.to_le_bytes());
    h[8..12].copy_from_slice(&outer_ts.to_le_bytes());
    h[12..16].copy_from_slice(&2u32.to_le_bytes());
    h[16..20].copy_from_slice(&outer_vs.to_le_bytes());
    b.extend_from_slice(&h);
    for (cnt, off) in [(1u32, ts0), (1, ts1), (1, vs0), (1, vs1)] {
        b.extend_from_slice(&cnt.to_le_bytes());
        b.extend_from_slice(&off.to_le_bytes());
    }
    b.extend_from_slice(&0u32.to_le_bytes()); // inner timestamps: seq0 0, seq1 0
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&0x7fffu16.to_le_bytes()); // inner values: 1.0, 1.0
    b.extend_from_slice(&0x7fffu16.to_le_bytes());
    set_arr(&mut b, WRATH_TRANSPARENCY, 1, head);

    let fmt = parse(&b).expect("a Wrath multi-sequence track parses");
    let m = fmt.model();
    let t = &m.transparency_tracks[0];
    assert_eq!(t.keys, vec![(0u32, 1.0f32), (1000u32, 1.0f32)]);
    assert_eq!(t.ranges, vec![(0u32, 1u32), (1u32, 2u32)]);
}

#[test]
fn wrath_attachment_40_and_event_56_strides() {
    let mut b = wrath_header();
    let bones_ofs = b.len() as u32;
    b.extend(wrath_bone_record([0.0; 3]));
    set_arr(&mut b, WRATH_BONES, 1, bones_ofs);
    // One attachment: 40 bytes, id@0, bone@4, position C3 @8.
    let att_ofs = b.len() as u32;
    let mut att = vec![0u8; 40];
    att[0..4].copy_from_slice(&1u32.to_le_bytes());
    att[4..8].copy_from_slice(&0u32.to_le_bytes());
    for (i, c) in [1.0f32, 2.0, 3.0].iter().enumerate() {
        att[8 + i * 4..12 + i * 4].copy_from_slice(&c.to_le_bytes());
    }
    b.extend(att);
    set_arr(&mut b, WRATH_ATTACHMENTS, 1, att_ofs);
    // One event: 56 bytes, ident@0, bone@8, position C3 @12.
    let ev_ofs = b.len() as u32;
    let mut ev = vec![0u8; 56];
    ev[0..4].copy_from_slice(b"$CSL");
    ev[8..12].copy_from_slice(&0u32.to_le_bytes());
    for (i, c) in [4.0f32, 5.0, 6.0].iter().enumerate() {
        ev[12 + i * 4..16 + i * 4].copy_from_slice(&c.to_le_bytes());
    }
    b.extend(ev);
    set_arr(&mut b, WRATH_EVENTS, 1, ev_ofs);

    let fmt = parse(&b).expect("a Wrath attachment + event parse");
    let m = fmt.model();
    assert_eq!(m.attachments.len(), 1, "the in-range attachment survives");
    assert_eq!(m.attachments[0].id, 1);
    assert_eq!(m.attachments[0].position, [1.0, 2.0, 3.0]);
    assert_eq!(m.event_markers.len(), 1, "the in-range event survives");
    assert_eq!(&m.event_markers[0].ident, b"$CSL");
    assert_eq!(m.event_markers[0].position, [4.0, 5.0, 6.0]);
}

// --- Wrath external `*.skin` profiles ----------------------------------------------------------
// A Wrath model keeps its view geometry in an external `.skin` file: a `SKIN` header, then the
// vertex lookup (a `u16` into the base vertices), the triangle corners, the per-vertex bone
// indices, the submeshes (48-byte `ssSubmeshes`) and the batches (24-byte `ssTextureUnits`).

const WRATH_VERTICES: usize = 0x3c;

/// A v264 model with `n` base vertices (zeroed), for a `.skin` to look into.
fn wrath_model_with_vertices(n: usize) -> Vec<u8> {
    let mut b = wrath_header();
    let verts_ofs = b.len() as u32;
    for _ in 0..n {
        b.extend(vec![0u8; 48]);
    }
    set_arr(&mut b, WRATH_VERTICES, n as u32, verts_ofs);
    b
}

/// A Wrath `.skin` file: an identity `u16` vertex lookup of `n_vert`, `n_idx` triangle corners,
/// `n_vert` bone records, one submesh (id 5, the whole corner range) and one batch.
fn skin_file(n_vert: usize, n_idx: usize) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"SKIN");
    b.resize(0x30, 0); // the rest of the header (48 bytes), patched below
    let (o_vert, o_idx, o_bone) = (0x30usize, 0x30 + n_vert * 2, 0x30 + n_vert * 2 + n_idx * 2);
    let (o_sub, o_bat) = (o_bone + n_vert * 4, o_bone + n_vert * 4 + 48);
    let mut arr = |ofs: usize, count: u32, offset: u32| {
        b[ofs..ofs + 4].copy_from_slice(&count.to_le_bytes());
        b[ofs + 4..ofs + 8].copy_from_slice(&offset.to_le_bytes());
    };
    arr(4, n_vert as u32, o_vert as u32);
    arr(0x0c, n_idx as u32, o_idx as u32);
    arr(0x14, n_vert as u32, o_bone as u32); // bone indices (present, not read by benilla)
    arr(0x1c, 1, o_sub as u32);
    arr(0x24, 1, o_bat as u32);
    b[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes()); // boneCountMax
    for i in 0..n_vert {
        b.extend_from_slice(&(i as u16).to_le_bytes()); // vertices: identity into the base list
    }
    for i in 0..n_idx {
        b.extend_from_slice(&(i as u16).to_le_bytes()); // triangle corners: 0..n_idx
    }
    for _ in 0..n_vert {
        b.extend_from_slice(&[0u8; 4]); // bone indices: every corner on bone 0
    }
    let mut sub = vec![0u8; 48];
    sub[0..2].copy_from_slice(&5u16.to_le_bytes()); // skinSectionId
    sub[8..10].copy_from_slice(&0u16.to_le_bytes()); // index_start
    sub[10..12].copy_from_slice(&(n_idx as u16).to_le_bytes()); // index_count
    b.extend_from_slice(&sub);
    let mut bat = vec![0u8; 24];
    bat[4..6].copy_from_slice(&0u16.to_le_bytes()); // skin_section_index
    bat[0x0a..0x0c].copy_from_slice(&2u16.to_le_bytes()); // material_index
    bat[0x10..0x12].copy_from_slice(&5u16.to_le_bytes()); // texture_combo_index
    b.extend_from_slice(&bat);
    b
}

#[test]
fn wrath_skin_file_decodes_geometry_and_batches() {
    let m2 = wrath_model_with_vertices(4);
    let fmt = parse(&m2).expect("the Wrath model parses");
    let skin = fmt
        .model()
        .parse_skin_file(&skin_file(4, 3))
        .expect("the .skin decodes");
    assert_eq!(*skin.indices(), vec![0, 1, 2, 3]);
    assert_eq!(*skin.triangles(), vec![0, 1, 2]);
    assert_eq!(skin.submeshes().len(), 1);
    assert_eq!(skin.submeshes()[0].id, 5);
    assert_eq!(skin.submeshes()[0].triangle_start, 0);
    assert_eq!(skin.submeshes()[0].triangle_count, 3);
    assert_eq!(skin.batches().len(), 1);
    assert_eq!(skin.batches()[0].skin_section_index, 0);
    assert_eq!(skin.batches()[0].material_index, 2);
    assert_eq!(skin.batches()[0].texture_combo_index, 5);
}

#[test]
fn wrath_skin_bad_magic_errs() {
    let m2 = wrath_model_with_vertices(1);
    let fmt = parse(&m2).expect("the Wrath model parses");
    let mut bad = skin_file(1, 3);
    bad[0] = b'X'; // not "SKIN"
    assert!(matches!(
        fmt.model().parse_skin_file(&bad),
        Err(Error::NotSkin)
    ));
}

#[test]
fn wrath_skin_out_of_range_vertex_lookup_errs() {
    // The model has two base vertices, but the profile's lookup names four (0..3): 2 and 3 are
    // out of range and must be refused, not read past the end of the base list.
    let m2 = wrath_model_with_vertices(2);
    let fmt = parse(&m2).expect("the Wrath model parses");
    assert!(matches!(
        fmt.model().parse_skin_file(&skin_file(4, 3)),
        Err(Error::Truncated)
    ));
}

#[test]
fn wrath_skin_truncated_file_errs() {
    let m2 = wrath_model_with_vertices(4);
    let fmt = parse(&m2).expect("the Wrath model parses");
    let full = skin_file(4, 3);
    // Cut the file inside the submesh/batch block: the record must fail, not read past the end.
    assert!(matches!(
        fmt.model().parse_skin_file(&full[..full.len() - 8]),
        Err(Error::Truncated)
    ));
}
