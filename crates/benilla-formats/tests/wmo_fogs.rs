//! WMO MFOG + group fog-index parse — byte check against the Goldshire inn (the interior-fog
//! fold's reference building). Pins the MFOG
//! record decode AND the MOGP fog-index disk offset (`0x30` — an earlier from-memory `0x40`
//! guess read zeros there; the `uniqueID @0x38` / no-liquid `@0x34` neighbours self-validate
//! the layout). Skips when the client isn't present.

use benilla_formats::{parse_wmo_fogs, wmo_group_header, Chain};

#[test]
fn goldshire_inn_fogs_and_group_indices() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let root = reader
        .read("World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo")
        .expect("read GoldshireInn.wmo");

    // Two MFOG records: the WMO default (the selector's seed, record 0) and a denser room fog.
    // Both are the warm tavern cream the storm veil must fade toward indoors.
    let fogs = parse_wmo_fogs(&root);
    assert_eq!(fogs.len(), 2, "inn should carry 2 MFOG records");
    assert!((fogs[0].fog_end - 194.44444).abs() < 1e-3);
    assert!((fogs[0].fog_start_scalar - 0.25).abs() < 1e-6);
    assert_eq!(fogs[0].color, 0xfffad890, "record 0: warm cream (ARGB)");
    assert_eq!(fogs[1].flags, 0x1);
    assert!((fogs[1].fog_end - 83.333336).abs() < 1e-3);
    assert_eq!(fogs[1].color, 0xfffdcf9e);

    // Group fog indices at disk +0x30: the tavern rooms point at record 1, the rest at the
    // default. area_table_id doubles as the layout's self-check (893.. = the inn's rows).
    let group = |gi: u32| {
        let bytes = reader
            .read(&format!(
                "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn_{gi:03}.wmo"
            ))
            .expect("read inn group");
        wmo_group_header(&bytes).expect("group header")
    };
    let g0 = group(0);
    assert_eq!(g0.area_table_id, 893);
    assert_eq!(g0.fog_indices, [1, 0, 0, 0]);
    let g2 = group(2);
    assert_eq!(g2.area_table_id, 895);
    assert_eq!(g2.fog_indices, [0, 0, 0, 0]);
}
