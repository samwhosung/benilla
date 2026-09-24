//! WMO MLIQ liquid surfaces: an `xverts × yverts` grid with one tile fewer each way, and a per-tile
//! type nibble where `0xf` is a hole and 4 is lake_a.

use benilla_formats::{wmo_group_liquid_mesh, Chain, LiquidKind};

#[test]
fn stormwind_canal_group_builds_still_water() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    // A canal segment: 12×9 vertices, 11×8 tiles, 52 of them wet and 36 holes.
    let g099 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo")
        .expect("read Stormwind_099.wmo");
    let mesh = wmo_group_liquid_mesh(&g099).expect("group 099 carries water");
    assert_eq!(mesh.kind, LiquidKind::Still, "canal water is lake_a/still");
    assert_eq!(mesh.positions.len(), 12 * 9, "full 12×9 grid emitted");
    assert_eq!(mesh.uvs.len(), 12 * 9);
    assert_eq!(mesh.depths.len(), 12 * 9);
    assert_eq!(
        mesh.indices.len(),
        52 * 6,
        "52 wet tiles → 2 tris each; the 36 hole tiles are skipped"
    );
    assert!(mesh
        .indices
        .iter()
        .all(|&i| (i as usize) < mesh.positions.len()));
    for p in &mesh.positions {
        assert!(
            p[2].is_finite() && p[2].abs() < 10_000.0,
            "sane height {p:?}"
        );
    }

    let g000 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_000.wmo")
        .expect("read Stormwind_000.wmo");
    assert!(
        wmo_group_liquid_mesh(&g000).is_none(),
        "a masonry group carries no liquid"
    );
}

/// MLIQ carries a height for every vertex, wet or not, and the shipped files leave hole interiors
/// at `0.0`; the builder gives every undrawn vertex the lowest drawn height, so the mesh bounds
/// stay on the sheet (186 of 868 such vertices in Blackfathom's pool, 36 of 108 in the canal).
#[test]
fn hole_corner_heights_never_reach_the_mesh_bounds() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    for (path, surface_z) in [
        (
            "World\\wmo\\dungeon\\kl_blackfathom\\blackfathom_instance_007.wmo",
            -58.283_04_f32,
        ),
        (
            "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo",
            -6.482_947_f32,
        ),
    ] {
        let group = reader
            .read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mesh = wmo_group_liquid_mesh(&group).unwrap_or_else(|| panic!("{path} carries water"));

        // Both pools are flat, so every vertex, drawn or not, sits on the sheet.
        let (lo, hi) = mesh
            .positions
            .iter()
            .map(|p| p[2])
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), z| {
                (lo.min(z), hi.max(z))
            });
        assert!(
            (lo - surface_z).abs() < 1e-3 && (hi - surface_z).abs() < 1e-3,
            "{path}: mesh z spans [{lo}..{hi}], expected the flat sheet at {surface_z}"
        );

        for &i in &mesh.indices {
            let z = mesh.positions[i as usize][2];
            assert!(
                (z - surface_z).abs() < 1e-3,
                "{path}: drawn vertex {i} at z {z}, expected {surface_z}"
            );
        }
    }
}

/// `0x6b62e0`'s category 0 splits on the group's `MOGP.flags & 0x48`: the exterior arm binds
/// `MapObjExtWater0.bls` and lights a vertex normal; the interior arm is fixed-function, unlit, and
/// takes its body colour from `MOMT[materialId].diffColor`. Each pool carries a per-vertex opacity.
#[test]
fn the_two_water_arms_carry_their_own_inputs() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");

    // (group file, is the group interior?, the pool's MLIQ materialId)
    for (path, interior, material_id) in [
        (
            "World\\wmo\\dungeon\\kl_blackfathom\\blackfathom_instance_007.wmo",
            true,
            27_u16,
        ),
        (
            "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo",
            false,
            115_u16,
        ),
    ] {
        let bytes = reader
            .read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let header = benilla_formats::wmo_group_header(&bytes)
            .unwrap_or_else(|| panic!("{path} parses a MOGP header"));
        assert_eq!(
            header.flags & 0x48 == 0,
            interior,
            "{path}: MOGP flags {:#010x} put it on the wrong water arm",
            header.flags
        );

        let mesh = wmo_group_liquid_mesh(&bytes).unwrap_or_else(|| panic!("{path} carries water"));
        assert_eq!(
            mesh.material_id,
            Some(material_id),
            "{path}: the pool must name its own MOMT slot — an interior arm reads its body colour \
             from exactly this index"
        );

        // Blackfathom's opacity spans 202 values, the canal's is 91% one: only the range is shared.
        let (lo, hi) = mesh
            .depths
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(
            (0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi),
            "{path}: opacity V out of range [{lo}..{hi}]"
        );
        assert!(
            hi > 0.0,
            "{path}: every vertex opacity is zero — the byte is not being read"
        );
    }
}

/// WMO water repeats its texture once per 4.167-yd grid cell: `0x6b6630` writes `u = i, v = j`
/// from loop counters that start at 0.
#[test]
fn wmo_water_repeats_once_per_grid_cell() {
    let data = benilla_formats::wow_data_or_skip!();
    let reader = Chain::open(&data).expect("open vanilla patch chain");
    let g099 = reader
        .read("World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind_099.wmo")
        .expect("read Stormwind_099.wmo");
    let mesh = wmo_group_liquid_mesh(&g099).expect("group 099 carries water");
    // The grid is 12 vertices wide, so index 12 starts the next row.
    let du = mesh.uvs[1][0] - mesh.uvs[0][0];
    assert!(
        (du - 1.0).abs() < 1e-4,
        "one repeat per cell: adjacent vertices differ by {du} in u, expected 1.0"
    );
    let dv = mesh.uvs[12][1] - mesh.uvs[0][1];
    assert!(
        (dv - 1.0).abs() < 1e-4,
        "one repeat per cell: adjacent rows differ by {dv} in v, expected 1.0"
    );
}
