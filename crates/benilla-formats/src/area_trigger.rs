//! `AreaTrigger.dbc`, the world's trigger volumes. The client owns only the geometry, sending
//! `CMSG_AREATRIGGER` on entering one; the server alone decides what it does. Rows stride `0x28`,
//! as the reference's scan does; positions are WoW world space (`bevy_to_wow`), yaw in radians.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, u32_at};

const AREA_TRIGGER: &str = "DBFilesClient\\AreaTrigger.dbc";

/// One `AreaTrigger.dbc` row: a sphere (`radius != 0`) or an oriented box (`radius == 0`).
#[derive(Clone, Copy, Debug)]
pub struct AreaTriggerRow {
    /// The id that goes on the wire in `CMSG_AREATRIGGER`.
    pub id: u32,
    /// `Map.dbc` id the volume lives on.
    pub map_id: u32,
    /// Centre, WoW world space.
    pub position: [f32; 3],
    /// Sphere radius in yards; `0` makes the row a box.
    pub radius: f32,
    /// Full box extents along its local X/Y/Z in yards; the containment test halves them.
    pub box_size: [f32; 3],
    /// The box's yaw about world Z in radians, CCW from +X: where its local +X points.
    pub box_yaw: f32,
}

impl AreaTriggerRow {
    /// Whether `p` is inside, as the reference's `0x5e22d0` tests it. A sphere is a plain 3D
    /// distance check, inclusive (`radius² >= Σ(centre − p)²`); a box carries `p` into its frame
    /// through gx's matrix ops (`0x7bdc40` load, `0x7bdd60` rotate by `-box_yaw`, `0x7bd700`
    /// apply), strict against half of `box_size` on all six faces. The server re-tests with 5 yd
    /// of slop (`MiscHandler.cpp:641`).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        if self.radius != 0.0 {
            let d = [
                self.position[0] - p[0],
                self.position[1] - p[1],
                self.position[2] - p[2],
            ];
            let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            return self.radius * self.radius >= dist_sq;
        }

        // World to box-local: rotate by -yaw about Z; the volume never pitches or rolls.
        let (sin, cos) = (-self.box_yaw).sin_cos();
        let dx = p[0] - self.position[0];
        let dy = p[1] - self.position[1];
        let local = [
            dx * cos - dy * sin,
            dx * sin + dy * cos,
            p[2] - self.position[2],
        ];
        (0..3).all(|i| {
            let half = self.box_size[i] * 0.5;
            local[i] > -half && local[i] < half
        })
    }
}

/// Every row bucketed by map, the window the reference narrows to (`0x5e2080`). File order holds
/// in a bucket: the check takes the first containing row, and volumes overlap.
pub struct AreaTriggerCatalog {
    by_map: HashMap<u32, Vec<AreaTriggerRow>>,
    len: usize,
}

impl AreaTriggerCatalog {
    /// The triggers on `map_id`, in file order.
    pub fn on_map(&self, map_id: u32) -> &[AreaTriggerRow] {
        self.by_map.get(&map_id).map_or(&[], |v| v.as_slice())
    }

    /// The first trigger on `map_id` containing `p`, where `0x5e2110` stops its scan.
    pub fn first_containing(&self, map_id: u32, p: [f32; 3]) -> Option<&AreaTriggerRow> {
        self.on_map(map_id).iter().find(|t| t.contains(p))
    }

    /// A row by id, walking every bucket: for tests and diagnostics, never per frame.
    pub fn get(&self, id: u32) -> Option<&AreaTriggerRow> {
        self.by_map.values().flatten().find(|t| t.id == id)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The 10-column `AreaTrigger.dbc` schema.
pub fn area_trigger_schema() -> Schema {
    let mut s = Schema::new("AreaTrigger");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MapID", FieldType::UInt32));
    s.add_field(SchemaField::new("X", FieldType::Float32));
    s.add_field(SchemaField::new("Y", FieldType::Float32));
    s.add_field(SchemaField::new("Z", FieldType::Float32));
    s.add_field(SchemaField::new("Radius", FieldType::Float32));
    s.add_field(SchemaField::new("BoxLength", FieldType::Float32));
    s.add_field(SchemaField::new("BoxWidth", FieldType::Float32));
    s.add_field(SchemaField::new("BoxHeight", FieldType::Float32));
    s.add_field(SchemaField::new("BoxYaw", FieldType::Float32));
    s.set_key_field("ID");
    s
}

/// Read `AreaTrigger.dbc` off the patch chain into an [`AreaTriggerCatalog`].
pub fn load_area_trigger_catalog(chain: &mut Chain) -> Result<AreaTriggerCatalog> {
    let bytes = chain
        .read_file(AREA_TRIGGER)
        .context("reading AreaTrigger.dbc")?;
    let rs = parse(&bytes, area_trigger_schema(), "AreaTrigger")?;
    let mut by_map: HashMap<u32, Vec<AreaTriggerRow>> = HashMap::new();
    let mut len = 0;
    for r in rs.records() {
        let (Some(id), Some(map_id)) = (u32_at(r, 0), u32_at(r, 1)) else {
            continue;
        };
        let f = |i| f32_at(r, i).unwrap_or(0.0);
        by_map.entry(map_id).or_default().push(AreaTriggerRow {
            id,
            map_id,
            position: [f(2), f(3), f(4)],
            radius: f(5),
            box_size: [f(6), f(7), f(8)],
            box_yaw: f(9),
        });
        len += 1;
    }
    Ok(AreaTriggerCatalog { by_map, len })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(radius: f32) -> AreaTriggerRow {
        AreaTriggerRow {
            id: 1,
            map_id: 0,
            position: [10.0, 20.0, 30.0],
            radius,
            box_size: [0.0; 3],
            box_yaw: 0.0,
        }
    }

    #[test]
    fn sphere_is_3d_and_inclusive() {
        let t = sphere(5.0);
        assert!(t.contains([10.0, 20.0, 30.0]));
        assert!(
            t.contains([15.0, 20.0, 30.0]),
            "exactly on the surface is in"
        );
        assert!(!t.contains([15.1, 20.0, 30.0]));
        // Z counts: straight up out of the sphere is out.
        assert!(!t.contains([10.0, 20.0, 36.0]));
        assert!(t.contains([10.0, 20.0, 34.0]));
    }

    #[test]
    fn box_is_half_extents_strict_and_yawed() {
        let axis_aligned = AreaTriggerRow {
            id: 2,
            map_id: 0,
            position: [0.0, 0.0, 0.0],
            radius: 0.0,
            box_size: [10.0, 4.0, 2.0],
            box_yaw: 0.0,
        };
        assert!(axis_aligned.contains([4.9, 1.9, 0.9]));
        // Half, not full: 6 yd along a 10-yd box is outside.
        assert!(!axis_aligned.contains([6.0, 0.0, 0.0]));
        assert!(!axis_aligned.contains([5.0, 0.0, 0.0]));
        assert!(!axis_aligned.contains([0.0, 0.0, 1.0]));

        // A quarter turn puts the long axis along world +Y.
        let turned = AreaTriggerRow {
            box_yaw: std::f32::consts::FRAC_PI_2,
            ..axis_aligned
        };
        assert!(!turned.contains([4.0, 0.0, 0.0]));
        assert!(turned.contains([0.0, 4.0, 0.0]));
    }

    #[test]
    fn real_area_triggers_load_and_contain_their_own_centres() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_trigger_catalog(&mut chain).expect("load AreaTrigger");
        assert_eq!(cat.len(), 432, "5875 ships 432 area triggers");

        // The Darnassus portal pair, 10-yd spheres on Kalimdor: vmangos's `areatrigger_teleport`
        // sends 527 to (8785.79, 966.98, 30.20), trigger 542's position here, and 542 to 527's.
        let exit = *cat.get(527).expect("trigger 527 (Darnassus - Exit)");
        let entrance = *cat.get(542).expect("trigger 542 (Darnassus - Entrance)");
        assert_eq!((exit.map_id, entrance.map_id), (1, 1));
        assert!(exit.contains([9947.0, 2630.0, 1318.0]), "{exit:?}");
        assert!(entrance.contains([8799.0, 970.0, 30.0]), "{entrance:?}");
        assert!(!entrance.contains([8824.0, 970.0, 30.0]));

        // Every volume contains its own centre, which a column shift breaks.
        for t in cat.on_map(1) {
            assert!(
                t.contains(t.position),
                "trigger {} does not contain its own centre: {t:?}",
                t.id
            );
        }

        let boxes = cat.on_map(0).iter().filter(|t| t.radius == 0.0).count();
        assert!(
            boxes > 0 && boxes < cat.on_map(0).len(),
            "map 0 has both shapes"
        );

        assert!(cat.on_map(1).iter().all(|t| t.map_id == 1));
        assert_eq!((cat.on_map(0).len(), cat.on_map(1).len()), (133, 121));
    }
}
