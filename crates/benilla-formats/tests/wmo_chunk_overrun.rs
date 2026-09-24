//! **The last chunk clamps to EOF.** A WMO's chunk stream is not guaranteed to tile its file
//! exactly, and the reference tolerates the slop: its walk (`0x6c3a60`) "reads chunks while the
//! 8-byte header is in-bounds and clamps the last chunk to EOF (never requires exact tiling)",
//! measured over the whole shipped corpus; the single file in the game that needs the rule is
//! `Undercity_144.wmo`, whose MOGP declares one byte more than the file holds.
//!
//! We used to `break` there and hand the caller nothing. That cost g144 its whole MOGP — flags,
//! portal-ref span, area id, fog, doodad and light refs — and since the portal flood reaches a
//! neighbour only through a group's ref span, the group became a **dead end**: g144 is the short
//! corridor joining g95 and g152, so the rooms past it culled from either side and you saw sky
//! through the doorway (B26, decision 0972).
//!
//! Two oracles, because the bug had two halves. The corpus sweep is the one that generalises: it is
//! the same shape as the reference's own "0 rejected" oracle, and it fails for a *new* file as well
//! as for this one. The byte pins on g144 catch a clamp that silently returns the wrong slice.
//! Skips when the client isn't present.

use benilla_formats::{parse_wmo_root, wmo_group_header, Chain};

/// The file the rule exists for. Its MOGP header must come back whole, with the exact values the
/// raw bytes carry at MOGP+0x08 (flags) and MOGP+0x24/0x26 (the portal-ref span) — the two fields
/// whose loss dead-ended the flood.
#[test]
fn undercity_144_mogp_survives_its_one_byte_overrun() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo")
        .expect("read Undercity_144");

    // The overrun is a property of the shipped file — if this ever stops holding, the test below is
    // no longer exercising the clamp and the rest of this file is decoration.
    let declared =
        u32::from_le_bytes([bytes[0x10], bytes[0x11], bytes[0x12], bytes[0x13]]) as usize;
    assert_eq!(
        0x0c + 8 + declared,
        bytes.len() + 1,
        "Undercity_144's MOGP is supposed to declare exactly one byte past EOF"
    );

    let h = wmo_group_header(&bytes).expect("MOGP must survive the clamp");
    assert_eq!(h.flags, 0xa805, "MOGP+0x08 flags");
    assert_eq!(h.portal_ref_start, 397, "MOGP+0x24 portal-ref start");
    assert_eq!(h.portal_ref_count, 2, "MOGP+0x26 portal-ref count");
}

/// The general oracle, mirroring the reference's own: **every** WMO in the corpus yields the chunk
/// its loader needs — a root parses, a group gives up its MOGP header. This is what makes the rule a
/// contract rather than a one-file patch; `wmo_chunk_census --all` is the same sweep with a readout.
#[test]
fn every_wmo_in_the_corpus_yields_its_loader_chunk() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut reader = Chain::open(&data).expect("open vanilla patch chain");
    let mut paths: Vec<String> = reader
        .list()
        .expect("list the patch chain")
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.to_ascii_lowercase().ends_with(".wmo"))
        .collect();
    paths.sort();
    paths.dedup();
    assert!(
        paths.len() > 5000,
        "expected the full WMO corpus, found {} files",
        paths.len()
    );

    let mut checked = 0u32;
    let mut bad: Vec<String> = Vec::new();
    for p in &paths {
        let Ok(b) = reader.read_file(p) else { continue };
        if b.is_empty() {
            continue; // 0-byte stubs ship in the corpus; the reference accepts them too
        }
        checked += 1;
        // A group file is MVER + MOGP, so its 4CC sits at 0x0c; anything else is a root.
        let ok = if b.len() > 16 && &b[12..16] == b"PGOM" {
            wmo_group_header(&b).is_some()
        } else {
            parse_wmo_root(&b).is_ok()
        };
        if !ok {
            bad.push(p.clone());
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {checked} WMO files lost the chunk their loader needs: {:?}",
        bad.len(),
        &bad[..bad.len().min(10)]
    );
}
