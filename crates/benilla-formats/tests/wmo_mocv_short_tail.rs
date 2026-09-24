//! A MOCV one record short is still a bake: in `Undercity_144.wmo` the chunk that clamps to EOF is
//! MOCV, 1159 of its declared 1160 bytes, and the reader pads its 289 colours to the 290 vertices.

use benilla_formats::{parse_wmo_root, wmo_group_raw_colors, Chain};

/// (declared, present) byte lengths of the MOGP sub-chunk `tag`, the payload clamped to EOF.
fn subchunk(group: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let mogp_size =
        u32::from_le_bytes([group[0x10], group[0x11], group[0x12], group[0x13]]) as usize;
    let payload = group.get(0x14..(0x14 + mogp_size).min(group.len()))?;
    let mut off = 0x44usize;
    while off + 8 <= payload.len() {
        let size = u32::from_le_bytes([
            payload[off + 4],
            payload[off + 5],
            payload[off + 6],
            payload[off + 7],
        ]) as usize;
        let start = off + 8;
        let end = start.saturating_add(size);
        if &payload[off..off + 4] == tag {
            return Some((size, end.min(payload.len()).saturating_sub(start)));
        }
        off = end.min(payload.len());
        if off == payload.len() {
            break;
        }
    }
    None
}

/// The bake is the warm orange the reference renders, not the white of an absent bake.
#[test]
fn undercity_144_keeps_its_bake_despite_a_short_mocv() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let g = reader
        .read("World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo")
        .expect("read Undercity_144");

    let (movt_declared, _) = subchunk(&g, b"TVOM").expect("MOVT");
    let (mocv_declared, mocv_present) = subchunk(&g, b"VCOM").expect("MOCV");
    assert_eq!(movt_declared / 12, 290, "MOVT vertex count");
    assert_eq!(
        mocv_declared / 4,
        290,
        "MOCV declares one colour per vertex"
    );
    assert_eq!(
        mocv_present,
        mocv_declared - 1,
        "MOCV is exactly one byte short"
    );

    let raw = wmo_group_raw_colors(&g).expect("a one-record-short MOCV is still a bake");
    assert_eq!(
        raw.len(),
        290,
        "the buffer must come back parallel to the vertices"
    );
    // BGRA on disk.
    let [b, gr, r, _a] = raw[0];
    assert!(
        r > 200 && gr > 100 && gr < 200 && b < 120,
        "g144's bake should be warm orange, got rgb({r},{gr},{b})"
    );
}

/// Exactly one group in the corpus has a MOCV that is not whole and parallel to its positions, so
/// the padding covers that one file.
#[test]
fn only_one_group_in_the_corpus_has_a_short_mocv() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut reader = Chain::open(&data).expect("open vanilla patch chain");
    let mut roots: Vec<String> = reader
        .list()
        .expect("list the patch chain")
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.ends_with(".wmo")
                && !l
                    .trim_end_matches(".wmo")
                    .ends_with(|c: char| c.is_ascii_digit())
        })
        .collect();
    roots.sort();
    roots.dedup();

    let mut short: Vec<String> = Vec::new();
    for rp in &roots {
        let Ok(rb) = reader.read_file(rp) else {
            continue;
        };
        let Ok(root) = parse_wmo_root(&rb) else {
            continue;
        };
        let stem = rp.strip_suffix(".wmo").unwrap_or(rp).to_string();
        for gi in 0..root.group_count() {
            let path = format!("{stem}_{gi:03}.wmo");
            let Ok(g) = reader.read_file(&path) else {
                continue;
            };
            if g.len() < 0x14 {
                continue;
            }
            let (Some((movt, _)), Some((mocv_declared, mocv_present))) =
                (subchunk(&g, b"TVOM"), subchunk(&g, b"VCOM"))
            else {
                continue; // no MOCV at all is normal (every exterior group)
            };
            if mocv_present < mocv_declared || mocv_declared / 4 != movt / 12 {
                short.push(path);
            }
        }
    }
    assert_eq!(
        short,
        vec!["World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo".to_string()],
        "the short-MOCV tolerance is meant to cover exactly one shipped file"
    );
}
