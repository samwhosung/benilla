//! A WMO's last chunk clamps to EOF: the reference's walk (`0x6c3a60`) reads chunks while the
//! 8-byte header is in bounds and never requires exact tiling. The one shipped file that needs it
//! is `Undercity_144.wmo`, whose MOGP declares one byte past the end.

use benilla_formats::{parse_wmo_root, wmo_group_header, Chain};

/// g144 is the corridor joining g95 and g152, and its portal-ref span (MOGP `+0x24`, `+0x26`) is
/// the portal flood's only way through.
#[test]
fn undercity_144_mogp_survives_its_one_byte_overrun() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let bytes = reader
        .read("World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo")
        .expect("read Undercity_144");

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

/// Every WMO in the corpus yields the chunk its loader needs: a root parses, a group gives up its
/// MOGP header.
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
