//! The route a transport transfer's loading screen draws over the world map
//! (`loadingscreen/route` in benilla-re): the per-path leg table the client builds from
//! `TaxiPathNode` at start-up (`0x408000`), the leg a transfer shows (`0x4072c0`), and the route's
//! vertices (`0x4075a0`): two marker quads and a ribbon of ship quads along a Catmull-Rom curve
//! over the leg's end stop, its `LoadingScreenTaxiSplines` waypoints and the next leg's first stop.
//! Positions are in the screen's unit box, y up.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::taxi::{TaxiPathNode, TaxiPathNodes};
use crate::transports::curve::Curve;
use crate::Chain;

const LOADING_SCREEN_TAXI_SPLINES: &str = "DBFilesClient\\LoadingScreenTaxiSplines.dbc";

/// The marker quads' half extents, `0x7ffdac` and `0x7ffda8`.
const MARKER_HALF_W: f32 = f32::from_bits(0x3ca3_d70a);
const MARKER_HALF_H: f32 = f32::from_bits(0x3cd9_1687);
/// `0x7ffd80`: ribbon steps per unit of route length.
const STEPS_PER_UNIT: f32 = f32::from_bits(0x4285_5556);
/// The ship quad's corner offsets at `0x8828b0`, (∓0.01, ∓0.015, 0) in strip order.
const SHIP_OFFSETS: [[f32; 2]; 4] = [
    [-0.01, -0.015],
    [0.01, -0.015],
    [-0.01, 0.015],
    [0.01, 0.015],
];
/// The ship quad's texture coordinates at `0x882bac`, the lower left quarter of `DynamicElements`.
const SHIP_UVS: [[f32; 2]; 4] = [[0.0, 1.0], [0.5, 1.0], [0.0, 0.5], [0.5, 0.5]];

/// One vertex of the route, `DYNAMICELEMENTVERT` less its colour: the unit-box position and the
/// texture coordinate, `v` down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
}

/// Four vertices in strip order: lower left, lower right, upper left, upper right.
pub type RouteQuad = [RouteVertex; 4];

/// A built route (`0x8828e0`): the markers at the leg's end stop and the next leg's first stop,
/// then the ribbon, one ship quad per step along the curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub markers: [RouteQuad; 2],
    pub ribbon: Vec<RouteQuad>,
}

impl Route {
    /// The ribbon quads `0x406fb0` draws at the load fraction `fraction`: quad `k` while
    /// `k / (n − 1)` is not above it, `n − 1` stored as a float, so the first always draws.
    pub fn revealed(&self, fraction: f32) -> usize {
        let last = f64::from(self.ribbon.len().saturating_sub(1) as f32);
        (0..self.ribbon.len())
            .take_while(|&k| {
                let ratio = k as f64 / last;
                // `test ah,0x41; je` leaves only for a ratio greater, ordered.
                ratio <= f64::from(fraction) || fraction.is_nan()
            })
            .count()
    }
}

/// One `LoadingScreenTaxiSplines.dbc` row: a path's leg and the route's waypoints, in the unit box.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingScreenTaxiSpline {
    pub path: u32,
    pub leg: u32,
    pub x: [f32; 8],
    pub y: [f32; 8],
}

/// The `LoadingScreenTaxiSplines.dbc` rows in file order.
pub struct LoadingScreenTaxiSplines {
    rows: Vec<LoadingScreenTaxiSpline>,
}

impl LoadingScreenTaxiSplines {
    pub fn from_rows(rows: Vec<LoadingScreenTaxiSpline>) -> Self {
        Self { rows }
    }

    /// `0x407da0`: the first row with the path and leg.
    fn find(&self, path: u32, leg: u32) -> Option<&LoadingScreenTaxiSpline> {
        self.rows.iter().find(|r| r.path == path && r.leg == leg)
    }
}

/// Read `LoadingScreenTaxiSplines.dbc` off the patch chain.
pub fn load_loading_screen_taxi_splines(chain: &mut Chain) -> Result<LoadingScreenTaxiSplines> {
    let bytes = chain
        .read_file(LOADING_SCREEN_TAXI_SPLINES)
        .context("reading LoadingScreenTaxiSplines.dbc")?;
    let mut s = Schema::new("LoadingScreenTaxiSplines");
    for name in ["ID", "PathID"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for i in 0..16 {
        let name = if i < 8 { "LocX" } else { "LocY" };
        s.add_field(SchemaField::new(
            format!("{name}{}", i % 8),
            FieldType::Float32,
        ));
    }
    s.add_field(SchemaField::new("LegIndex", FieldType::UInt32));
    let rs = parse(&bytes, s, "LoadingScreenTaxiSplines")?;
    let rows = rs
        .records()
        .iter()
        .filter_map(|r| {
            let mut x = [0.0f32; 8];
            let mut y = [0.0f32; 8];
            for i in 0..8 {
                x[i] = f32_at(r, 2 + i)?;
                y[i] = f32_at(r, 10 + i)?;
            }
            Some(LoadingScreenTaxiSpline {
                path: u32_at(r, 1)?,
                leg: u32_at(r, 18)?,
                x,
                y,
            })
        })
        .collect();
    Ok(LoadingScreenTaxiSplines::from_rows(rows))
}

/// A leg's stops (`0x882b60`, 8 bytes): the first and the last stop of its run of nodes. `None` is
/// a pair whose first stop id is 0, a run with no stop.
type StopPair = Option<(TaxiPathNode, TaxiPathNode)>;

/// The per-path leg lists `0x408000` builds at start-up (`0x882b50`).
pub struct TaxiLegTable {
    legs: HashMap<u32, Vec<StopPair>>,
}

impl TaxiLegTable {
    /// `0x408000` over the store in file order. A flag 2 node is the run's last stop, and its first
    /// when the run has none. A flag 1 node, or a path's first node on a new continent, closes the
    /// run without being read as a stop; a change of path closes a run with a first stop, in the
    /// path before it. The last stop is never cleared, and a run still open at the end is dropped.
    pub fn build(nodes: &TaxiPathNodes) -> Self {
        let mut legs: HashMap<u32, Vec<StopPair>> = HashMap::new();
        let mut first: Option<TaxiPathNode> = None;
        let mut last: Option<TaxiPathNode> = None;
        let mut prev: Option<&TaxiPathNode> = None;
        // A pair of a first stop id 0 is a run with no stop; a last stop without a first is only
        // ever a run's first stop being 0.
        let close = |first: Option<TaxiPathNode>, last: Option<TaxiPathNode>| {
            first.zip(last).filter(|(f, l)| f.id > 0 && l.id > 0)
        };
        for node in nodes.in_file_order() {
            // 0x4080cb: a change of path closes a run with a first stop (`0x4080d0`).
            if let Some(p) = prev.filter(|p| p.path_id != node.path_id) {
                if first.is_some_and(|f| f.id > 0) {
                    legs.entry(p.path_id).or_default().push(close(first, last));
                }
            }
            if prev.is_none_or(|p| p.path_id != node.path_id) {
                first = None;
            }
            let same_path = prev.filter(|p| p.path_id == node.path_id);
            if node.flags & 1 != 0 {
                // 0x408122: appends with no test of the first stop.
                legs.entry(node.path_id)
                    .or_default()
                    .push(close(first, last));
                first = None;
            } else if same_path.is_some_and(|p| p.map_id != node.map_id) {
                // 0x408163, 0x40816b: the path's first node on another continent.
                legs.entry(node.path_id)
                    .or_default()
                    .push(close(first, last));
                first = None;
            } else if node.flags & 2 != 0 {
                // 0x4081a2: the last stop, and the first when the run has none.
                last = Some(*node);
                if first.is_none() {
                    first = Some(*node);
                }
            }
            prev = Some(node);
        }
        Self { legs }
    }

    /// `0x4072c0`'s search: the first of the path's legs whose first stop is on `from_map`. The
    /// walk ends at a leg with no stop.
    pub fn leg_from(&self, path: u32, from_map: u32) -> Option<usize> {
        let legs = self.legs.get(&path)?;
        for (i, pair) in legs.iter().enumerate() {
            let (first, _) = (*pair)?;
            if first.map_id == from_map {
                return Some(i);
            }
        }
        None
    }

    /// `0x4075a0(path, leg)`: the route from the leg's last stop A to the next leg's first stop B
    /// (`(leg + 1) mod n`), each placed by `project` (`0x4a72b0`, `None` without the map's
    /// `WorldMapContinent` row), through the row's waypoints, those with an x or a y above 0 up to
    /// the first with neither. The curve runs over A, A, the waypoints, B, B; its length `L` makes
    /// `max(trunc(L · 66.66667), 1)` steps, and the ribbon puts a ship quad at the point of each of
    /// the `steps + 1` parameters `k / steps`, accumulated as a float and clamped to `[0, 1]`.
    pub fn route(
        &self,
        path: u32,
        leg: usize,
        splines: &LoadingScreenTaxiSplines,
        project: impl Fn(u32, f32, f32) -> Option<[f32; 2]>,
    ) -> Option<Route> {
        let row = splines.find(path, u32::try_from(leg).ok()?)?;
        let legs = self.legs.get(&path).filter(|l| !l.is_empty())?;
        let (_, a) = (*legs.get(leg)?)?;
        let (b, _) = legs[(leg + 1) % legs.len()]?;
        let a = project(a.map_id, a.pos[0], a.pos[1])?;
        let b = project(b.map_id, b.pos[0], b.pos[1])?;
        let mut points = vec![[a[0], a[1], 0.0]; 2];
        // 0x407750-0x407798: `jp` stores a waypoint whose x is above 0 or NaN, else one whose y is.
        let above = |v: f32| v > 0.0 || v.is_nan();
        points.extend(
            row.x
                .iter()
                .zip(&row.y)
                .take_while(|(&x, &y)| above(x) || above(y))
                .map(|(&x, &y)| [x, y, 0.0]),
        );
        points.extend([[b[0], b[1], 0.0]; 2]);
        let curve = Curve::new(points);
        // 0x4077f4-0x407814: `fmul` then `__ftol`, at least 1.
        let steps = ftol(f64::from(curve.length) * f64::from(STEPS_PER_UNIT)).max(1);
        Some(Route {
            markers: [marker(a, 0.5, 1.0), marker(b, 0.0, 0.5)],
            ribbon: ribbon(&curve, steps as u32),
        })
    }
}

/// `__ftol` as a caller reads `eax`.
fn ftol(v: f64) -> i32 {
    const TWO_63: f64 = (1u64 << 63) as f64;
    let t = v.trunc();
    (if (-TWO_63..TWO_63).contains(&t) {
        t as i64
    } else {
        i64::MIN
    }) as i32
}

/// `0x407900`: a marker quad around `c`, ±0.02 by ±0.0265, on the top half of `DynamicElements`
/// between `u0` and `u1`; A takes the right quarter and B the left.
fn marker(c: [f32; 2], u0: f32, u1: f32) -> RouteQuad {
    let at = |dx: f32, dy: f32| {
        [
            (f64::from(c[0]) + f64::from(dx)) as f32,
            (f64::from(c[1]) + f64::from(dy)) as f32,
        ]
    };
    let (w, h) = (MARKER_HALF_W, MARKER_HALF_H);
    [
        RouteVertex {
            pos: at(-w, -h),
            uv: [u0, 0.5],
        },
        RouteVertex {
            pos: at(w, -h),
            uv: [u1, 0.5],
        },
        RouteVertex {
            pos: at(-w, h),
            uv: [u0, 0.0],
        },
        RouteVertex {
            pos: at(w, h),
            uv: [u1, 0.0],
        },
    ]
}

/// `0x407c80(curve, steps)`: a ship quad at the curve's point for each of `steps + 1` parameters
/// from 0, each the last plus `1/steps` stored as a float, clamped to 0 below and to 1 at 1 and
/// above or NaN.
fn ribbon(curve: &Curve, steps: u32) -> Vec<RouteQuad> {
    let step = (1.0 / f64::from(steps)) as f32;
    let mut t = 0.0f32;
    (0..=steps)
        .map(|_| {
            t = if 0.0 > t {
                0.0
            } else if t >= 1.0 || t.is_nan() {
                1.0
            } else {
                t
            };
            let p = curve.point(t);
            let quad = [0, 1, 2, 3].map(|i| RouteVertex {
                pos: [
                    (f64::from(p[0]) + f64::from(SHIP_OFFSETS[i][0])) as f32,
                    (f64::from(p[1]) + f64::from(SHIP_OFFSETS[i][1])) as f32,
                ],
                uv: SHIP_UVS[i],
            });
            t = (f64::from(step) + f64::from(t)) as f32;
            quad
        })
        .collect()
}

#[cfg(test)]
mod tests;
