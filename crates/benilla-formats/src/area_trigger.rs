//! `AreaTrigger.dbc` — the world's invisible trip-wires, and the containment law that fires them.
//!
//! A trigger is a volume the *client* watches: when the player walks into one, the client sends
//! `CMSG_AREATRIGGER` naming its id and the **server** decides what that means — a teleport (every
//! instance entrance, the Darnassus/Rut'theran portals), a quest's "explore here" objective, the
//! inn's rested-XP state, a battleground's entrance list. The client owns only the geometry; it
//! never knows what any trigger does.
//!
//! Layout — 10 × 4-byte columns (record stride `0x28`, which is what the reference's own scan
//! strides by): `ID(0), MapID(1), X(2), Y(3), Z(4), Radius(5), BoxLength(6), BoxWidth(7),
//! BoxHeight(8), BoxYaw(9)`. Positions are WoW world space (the space `bevy_to_wow` produces),
//! yaw in radians.
//!
//! The containment law is [`AreaTriggerRow::contains`] — read from the reference's `0x5e22d0`,
//! which is the only place this math lives.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, u32_at};

const AREA_TRIGGER: &str = "DBFilesClient\\AreaTrigger.dbc";

/// One `AreaTrigger.dbc` row: a sphere (`radius != 0`) or an oriented box (`radius == 0`) sitting
/// on one map, in WoW world space.
#[derive(Clone, Copy, Debug)]
pub struct AreaTriggerRow {
    /// The id that goes on the wire in `CMSG_AREATRIGGER`.
    pub id: u32,
    /// `Map.dbc` id the volume lives on.
    pub map_id: u32,
    /// Centre, WoW world space.
    pub position: [f32; 3],
    /// Sphere radius (yd). `0` ⇒ this row is a box; see [`Self::contains`].
    pub radius: f32,
    /// **Full** box extents along the box's own local X/Y/Z (yd) — halved by the containment test,
    /// which is how the reference reads them.
    pub box_size: [f32; 3],
    /// The box's yaw about world Z (radians, CCW from +X), i.e. where its local +X points.
    pub box_yaw: f32,
}

impl AreaTriggerRow {
    /// Is `p` (WoW world space) inside this volume?
    ///
    /// The reference's containment predicate `0x5e22d0`, an orchestration over gx's matrix ops —
    /// the comparison it owns is what this reproduces:
    ///
    /// - **`radius != 0` ⇒ sphere**, tested in **3D** and **inclusively**: the reference computes
    ///   `Σ(centre − p)²` and returns "inside" on `radius² >= dist²`.
    /// - **`radius == 0` ⇒ oriented box**: the point is carried into the box's own frame (translate
    ///   to the centre, rotate by **−`box_yaw`** about Z) and compared against ±half of each
    ///   `box_size` component, **strictly** on all six faces (`-h < l < h`). The reference does the
    ///   transform through gx (`0x7bdc40` load / `0x7bdd60` rotate-about-Z / `0x7bd700` apply) and
    ///   owns only the comparison; vmangos's `IsPointInAreaTriggerZone` reaches the same law from
    ///   the other side (it rotates the player by `2π − box_orientation`, which is this rotation).
    ///
    /// The server re-runs the same test with **5 yd of slop** before acting
    /// (`MiscHandler.cpp:641`), so a marginally tight answer here still opens the door; a
    /// marginally *loose* one is simply ignored.
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

        // World → box-local: the box's local +X points along `box_yaw`, so the inverse rotation is
        // by −yaw about Z. Z is unrotated (the volume never pitches or rolls).
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

/// Every `AreaTrigger.dbc` row, bucketed by map — the per-map window the reference narrows to
/// before each check (`0x5e2080` walks the map-sorted table for a `[first, end)` index range;
/// bucketing answers the same question without depending on the file staying sorted).
///
/// **File order is preserved inside each bucket**, because the check takes the *first* containing
/// row and volumes do overlap (an instance entrance sits inside its building's larger trigger).
pub struct AreaTriggerCatalog {
    by_map: HashMap<u32, Vec<AreaTriggerRow>>,
    len: usize,
}

impl AreaTriggerCatalog {
    /// The triggers on `map_id`, in file order (empty for a map with none).
    pub fn on_map(&self, map_id: u32) -> &[AreaTriggerRow] {
        self.by_map.get(&map_id).map_or(&[], |v| v.as_slice())
    }

    /// The **first** trigger on `map_id` containing `p` — the reference's own pick
    /// (`0x5e2110` breaks its scan on the first hit).
    pub fn first_containing(&self, map_id: u32, p: [f32; 3]) -> Option<&AreaTriggerRow> {
        self.on_map(map_id).iter().find(|t| t.contains(p))
    }

    /// A row by id (linear over its map's bucket is not available here — ids are unique table-wide,
    /// so this walks every bucket; used by tests and diagnostics, never per frame).
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

/// 10 u32/f32-wide columns (see module doc).
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

    /// The sphere leg: 3D, and inclusive exactly on the surface (the reference's `radius² >= d²`).
    #[test]
    fn sphere_is_3d_and_inclusive() {
        let t = sphere(5.0);
        assert!(t.contains([10.0, 20.0, 30.0]));
        assert!(
            t.contains([15.0, 20.0, 30.0]),
            "exactly on the surface is in"
        );
        assert!(!t.contains([15.1, 20.0, 30.0]));
        // Z counts: straight up out of the sphere is out, which a 2D test would miss.
        assert!(!t.contains([10.0, 20.0, 36.0]));
        assert!(t.contains([10.0, 20.0, 34.0]));
    }

    /// The box leg: half-extents, strict faces, and the yaw actually rotating the volume.
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
        // Strict on the face itself.
        assert!(!axis_aligned.contains([5.0, 0.0, 0.0]));
        assert!(!axis_aligned.contains([0.0, 0.0, 1.0]));

        // Rotate the same box a quarter turn: the long axis now runs along world +Y, so a point
        // that was inside along X is outside, and its mirror along Y is inside.
        let turned = AreaTriggerRow {
            box_yaw: std::f32::consts::FRAC_PI_2,
            ..axis_aligned
        };
        assert!(!turned.contains([4.0, 0.0, 0.0]));
        assert!(turned.contains([0.0, 4.0, 0.0]));
    }

    /// The real 5875 table: the Darnassus portal pair the reports name, a box row and a
    /// sphere row both present, and the per-map bucketing. Skips without client data.
    #[test]
    fn real_area_triggers_load_and_contain_their_own_centres() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_area_trigger_catalog(&mut chain).expect("load AreaTrigger");
        assert_eq!(cat.len(), 432, "5875 ships 432 area triggers");

        // The two the Darnassus report rides on, both on Kalimdor, both 10-yd spheres. They
        // pin the coordinate columns from *outside* the client: vmangos's `areatrigger_teleport`
        // sends 527 ("Darnassus - Exit") to (8785.79, 966.98, 30.20) — which is trigger **542**'s
        // own position here, and vice versa. Two independent sources, agreeing to a couple of yards,
        // on a pair of doorways that teleport to each other.
        let exit = *cat.get(527).expect("trigger 527 (Darnassus - Exit)");
        let entrance = *cat.get(542).expect("trigger 542 (Darnassus - Entrance)");
        assert_eq!((exit.map_id, entrance.map_id), (1, 1));
        assert!(exit.contains([9947.0, 2630.0, 1318.0]), "{exit:?}");
        assert!(entrance.contains([8799.0, 970.0, 30.0]), "{entrance:?}");
        // …and 25 yd away is outside a 10-yd sphere.
        assert!(!entrance.contains([8824.0, 970.0, 30.0]));

        // Every volume contains its own centre — the one invariant that catches a column shift
        // (a size read as a coordinate, or a radius read from the wrong slot, breaks it).
        for t in cat.on_map(1) {
            assert!(
                t.contains(t.position),
                "trigger {} does not contain its own centre: {t:?}",
                t.id
            );
        }

        // Both shapes are represented in the shipped table.
        // Both shapes ship: 80 boxes, the rest spheres.
        let boxes = cat.on_map(0).iter().filter(|t| t.radius == 0.0).count();
        assert!(
            boxes > 0 && boxes < cat.on_map(0).len(),
            "map 0 has both shapes"
        );

        // Buckets are per map, and the two continents carry most of the table.
        assert!(cat.on_map(1).iter().all(|t| t.map_id == 1));
        assert_eq!((cat.on_map(0).len(), cat.on_map(1).len()), (133, 121));
    }
}
