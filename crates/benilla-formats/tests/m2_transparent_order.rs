//! The transparent order's file keys (`0x70ae10`) on the models the issues name: a texture unit's
//! priority plane (`+0x02`, `0x707c76`) leads the batch depth inside one instance, and the vertex
//! shader id (`0x70aeed`) leads it inside one plane.

use std::cmp::Ordering;

use benilla_formats::{load_m2_mesh, open_chain, transparent_cmp, BatchOrder, TransparentElement};

/// The batch of file unit `unit`'s keys.
fn unit(subs: &[benilla_formats::RenderSubmesh], unit: u16) -> BatchOrder {
    subs.iter()
        .filter_map(|s| s.order)
        .find(|o| o.unit == unit)
        .unwrap_or_else(|| panic!("unit {unit} draws"))
}

/// One instance under depth fill: the instance key ties, each batch at its own centre's depth.
fn el(order: &BatchOrder, depth: f32) -> TransparentElement<'_> {
    TransparentElement {
        order,
        key_depth: 100.0,
        fill_copy: false,
        batch_depth: depth,
        instance: 1,
    }
}

/// `HangingSkullLight01` (GameObjectDisplayInfo 6417): unit 0 is the additive glow on plane 1,
/// unit 1 the alpha-blended skull on plane 0. The skull draws first and the glow adds over it,
/// from any view.
#[test]
fn hanging_skull_light_draws_its_skull_then_the_glow() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    for (file, glow_unit, skull_unit) in [
        ("HangingSkullLight01.m2", 0, 1),
        ("HangingSkullLight02.m2", 2, 0),
    ] {
        let subs = load_m2_mesh(
            &mut chain,
            &format!("World\\Generic\\PassiveDoodads\\Halloween\\{file}"),
        )
        .expect("load the skull light");
        let (glow, skull) = (unit(&subs, glow_unit), unit(&subs, skull_unit));
        assert_eq!((glow.plane, skull.plane), (1, 0), "{file}: the planes");
        for (g, s) in [(9.0, 8.0), (8.0, 9.0)] {
            assert_eq!(
                transparent_cmp(&el(&skull, s), &el(&glow, g)),
                Ordering::Less,
                "{file}: the skull first, the glow over it, at depths {s} and {g}"
            );
        }
    }
}

/// `Stave_2H_Long_B_04`: units 1 and 2 share one submesh, so one centre. Unit 1 is the
/// environment-mapped sheen (stage digit 2), on layer 1; unit 2 the plain layer 0. The lower
/// vertex shader id draws first, against the file's order.
#[test]
fn a_sheen_on_its_base_orders_by_shader_not_by_file() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let subs = load_m2_mesh(
        &mut chain,
        "Item\\ObjectComponents\\Weapon\\Stave_2H_Long_B_04.m2",
    )
    .expect("load the stave");
    let (sheen, base) = (unit(&subs, 1), unit(&subs, 2));
    assert_eq!(sheen.center, base.center, "one submesh, one centre");
    assert_eq!((sheen.layer, base.layer), (1, 0));
    assert!(base.vertex_shader < sheen.vertex_shader);
    assert_eq!(
        transparent_cmp(&el(&base, 4.0), &el(&sheen, 4.0)),
        Ordering::Less,
        "the base layer first, the sheen over it"
    );
}
