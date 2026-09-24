//! **A MOCV one record short is still a bake.** The companion to `wmo_chunk_overrun`: clamping the
//! last chunk to EOF is only half the tolerance, because the chunk that clamps in
//! `Undercity_144.wmo` *is* MOCV — 1159 of its declared 1160 bytes, so the colour buffer parses to
//! 289 entries for 290 vertices.
//!
//! The reader then demanded MOCV be exactly parallel to the positions and returned `None` otherwise,
//! which threw away 289 good colours over one missing byte. An interior batch draws `tex × MOCV`, and
//! absent colour renders as white, so that corridor lit at full brightness and untinted inside a city
//! whose every other interior surface is multiplied by a dark bake — the pale, cold arch in the
//! director's shot, against a reference that shows it warm and lantern-lit.
//!
//! Skips when the client isn't present.

use benilla_formats::{parse_wmo_root, wmo_group_raw_colors, Chain};

/// Walk a group file's MOGP sub-chunks (payload clamped to EOF, as the loader does) and return the
/// declared byte length of `tag`, plus how many of those bytes the file actually holds.
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

/// The file the tolerance exists for: MOCV is its clamped tail, and the bake it carries is the warm
/// orange the reference renders — not the white an absent bake falls back to.
#[test]
fn undercity_144_keeps_its_bake_despite_a_short_mocv() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let g = reader
        .read("World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo")
        .expect("read Undercity_144");

    // The data fact this rests on: MOVT holds 290 vertices; MOCV declares 290 colours and the file
    // is one byte short of them. If the shipped file ever changes, this stops testing the tail.
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
    // BGRA on the wire. The authored bake here is warm orange — the thing that was being replaced by
    // white, and the whole visible symptom.
    let [b, gr, r, _a] = raw[0];
    assert!(
        r > 200 && gr > 100 && gr < 200 && b < 120,
        "g144's bake should be warm orange, got rgb({r},{gr},{b})"
    );
}

/// The tolerance must stay **narrow**. Exactly one group in the corpus has a MOCV that is not a whole
/// number of records parallel to its positions; if that count grows, either the reader regressed or
/// the padding is masking a real parse bug, and either way padding hundreds of vertices from one
/// sample would invent lighting rather than recover it.
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
