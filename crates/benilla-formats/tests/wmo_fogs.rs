//! WMO MFOG records, and the MOGP fog indices: four bytes at `+0x30`, before the no-liquid word at
//! `+0x34` and `uniqueID` at `+0x38`.

use benilla_formats::{parse_wmo_fogs, wmo_group_header, Chain};

#[test]
fn goldshire_inn_fogs_and_group_indices() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let root = reader
        .read("World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo")
        .expect("read GoldshireInn.wmo");

    // Record 0 is the WMO default, the selector's seed; record 1 is a denser room fog.
    let fogs = parse_wmo_fogs(&root);
    assert_eq!(fogs.len(), 2, "inn should carry 2 MFOG records");
    assert!((fogs[0].fog_end - 194.44444).abs() < 1e-3);
    assert!((fogs[0].fog_start_scalar - 0.25).abs() < 1e-6);
    assert_eq!(fogs[0].color, 0xfffad890, "record 0: warm cream (ARGB)");
    assert_eq!(fogs[1].flags, 0x1);
    assert!((fogs[1].fog_end - 83.333336).abs() < 1e-3);
    assert_eq!(fogs[1].color, 0xfffdcf9e);

    // The tavern rooms point at record 1, the rest at the default; 893.. are the inn's area rows.
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
