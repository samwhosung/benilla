//! A moving transport's path (type 15, the ships and zeppelins), as the reference builds it from
//! `TaxiPathNode` when the object's record arrives (`0x5f4cc0`) and places the ship on it at a
//! time (`0x5f50a0`), in WoW coordinates.
//!
//! Every node of the path is kept. A leg opens at the first node, at a map change and after a node
//! with flag 0x1; a node with flag 0x2 that is not its leg's first is a stop of its delay × 1000
//! ms. A leg is a Catmull-Rom curve over its nodes ([`curve`]), timed span by span between its
//! stops ([`timing`]), and the legs are laid end to end into the period.

pub(crate) mod curve;
mod timing;

pub use timing::ShipAnim;

use crate::taxi::TaxiPathNode;
use curve::{Curve, SHORT};

/// `0x801360`, the float 0.001 that turns ms into seconds.
const MS: f32 = 0.001;
/// The builder's stack arrays hold 1024 nodes and 1024 stops (`0x5f9530`, `0x5f9610`).
const MAX_NODES: usize = 1024;

/// A transport's pose at one instant of its period, in WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportSample {
    /// The leg the instant falls in, by its place in the path (the tick's `+0x58`).
    pub leg: usize,
    /// The map of the leg the instant falls in, which changes as a transport crosses continents.
    pub map: u32,
    pub pos: [f32; 3],
    /// The facing, radians in `(−π, π]`: against the curve's tangent (`0x5f528e`).
    pub heading: f32,
    /// The phase the ship plays.
    pub anim: ShipAnim,
}

impl TransportSample {
    /// Under way, not standing at a stop.
    pub fn moving(&self) -> bool {
        self.anim != ShipAnim::Stand
    }
}

/// One entry of a leg's stop table (12 bytes at `leg+0x44`): the time the stop is reached in the
/// period, its distance along the leg and its wait, both times in ms. The last entry is the leg's
/// end, with wait 0.
#[derive(Debug, Clone, Copy)]
struct Stop {
    time: u32,
    distance: f32,
    wait: u32,
}

/// One leg (0x4c bytes): its map, curve, start and end in the period, and stop table.
#[derive(Debug, Clone)]
struct Leg {
    map: u32,
    curve: Curve,
    start: u32,
    /// `+0x34`, the leg's span times without its waits.
    travel: u32,
    end: u32,
    stops: Vec<Stop>,
}

/// A moving transport's path (the handler's `+0x20`), built once per `taxiPathID`.
#[derive(Debug, Clone)]
pub struct TransportTimetable {
    /// The period in ms (`+0x1c`): the ship is at `(anchor + elapsed) mod period` in it.
    pub period_ms: u32,
    /// `moveSpeed` (`+0x18`) and `accelRate` (`+0x20`) as floats.
    speed: f32,
    accel: f32,
    legs: Vec<Leg>,
}

impl TransportTimetable {
    /// `0x5f4cc0` over one path's nodes in store order and the template's `moveSpeed` and
    /// `accelRate`, each made a float through `fild` of the zero-extended value. `None` for no
    /// node or a period of 0, which the reference ticks with a modulus of 1 (`0x5f50d5`) and no
    /// leg ending after 0, so it never moves the ship (`0x5f511c`), as a ship with no timetable.
    /// Deviation: `None` too where the reference's build overruns its stack or reads memory it
    /// never wrote (`transports#path-faults`), so the ship stays at its spawn: more than 1024
    /// nodes in a leg (`0x5f9530`), a leg of 3 nodes or fewer (`0x4532e3`) and a stop on a leg's
    /// last node (`0x5f91a1`). No shipped path reaches one.
    pub fn build(nodes: &[TaxiPathNode], move_speed: u32, accel_rate: u32) -> Option<Self> {
        let speed = f64::from(move_speed) as f32;
        let accel = f64::from(accel_rate) as f32;
        let mut legs: Vec<Leg> = Vec::new();
        let mut points: Vec<[f32; 3]> = Vec::new();
        let mut stops: Vec<(usize, u32)> = Vec::new();
        let mut map: Option<u32> = None;
        let mut after_flag1 = false;
        for node in nodes {
            // 0x5f4dda-0x5f4de9: no leg yet, a map change or the previous node's flag 0x1.
            if map != Some(node.map_id) || after_flag1 {
                if let Some(m) = map {
                    legs.push(time_leg(
                        m,
                        std::mem::take(&mut points),
                        &stops,
                        speed,
                        accel,
                    )?);
                }
                map = Some(node.map_id);
                // 0x5f4e32: the stops empty only when a leg opens.
                stops.clear();
            }
            // 0x5f4e37: a stop needs a node before it in its leg; 0x5f4e47 is a 32-bit `imul`.
            if node.flags & 2 != 0 && !points.is_empty() {
                stops.push((points.len(), node.delay.wrapping_mul(1000)));
            }
            points.push(node.pos);
            // 0x5f9530: the appends check no bound, so a 1025th node overruns the stack array.
            if points.len() > MAX_NODES {
                return None;
            }
            after_flag1 = node.flags & 1 != 0;
        }
        legs.push(time_leg(map?, points, &stops, speed, accel)?);

        // 0x5f4eeb-0x5f4f01: every leg's end adds the waits of the last leg's stops, the ones
        // still in the buffer, not its own.
        let waits = stops.iter().fold(0u32, |w, s| w.wrapping_add(s.1));
        let mut total = 0u32;
        for leg in &mut legs {
            leg.start = total;
            total = total.wrapping_add(leg.travel).wrapping_add(waits);
            for stop in &mut leg.stops {
                stop.time = stop.time.wrapping_add(leg.start);
            }
            leg.end = total;
        }
        (total != 0).then_some(Self {
            period_ms: total,
            speed,
            accel,
            legs,
        })
    }

    /// The pose `0x5f50a0` gives the ship at `cycle_ms` into its period: the first leg ending
    /// after it, then the stop whose wait holds it, or else the span it falls in, run on the speed
    /// curve from the stop the span leaves. The distance over the leg's length, clamped to
    /// `[0, 1]` with NaN as 1, places the curve's frame; the facing is the `atan2` of its tangent
    /// with x and y negated and normalised, from a frame whose tangent was `(1, 0, 0)`
    /// (`0x5f8d60`).
    pub fn sample(&self, cycle_ms: u32) -> TransportSample {
        let t = cycle_ms % self.period_ms;
        let (index, leg) = self
            .legs
            .iter()
            .enumerate()
            .find(|(_, l)| t < l.end)
            .expect("the last leg ends at the period");
        let last = leg.stops.len() - 1;
        let mut span_start = leg.start;
        let mut base = 0.0f32;
        let mut at_stop = None;
        let mut j = 0;
        while j < last {
            let stop = leg.stops[j];
            if t < stop.time {
                break;
            }
            if t - stop.time < stop.wait {
                at_stop = Some(stop.distance);
                break;
            }
            span_start = stop.time.wrapping_add(stop.wait);
            base = stop.distance;
            j += 1;
        }
        let (distance, anim) = match at_stop {
            Some(d) => (f64::from(d), ShipAnim::Stand),
            None => {
                // 0x5f51b7-0x5f51ee: the zero-extended ms times 0.001, stored.
                let seconds = |ms: u32| (f64::from(ms) * f64::from(MS)) as f32;
                let span = seconds(leg.stops[j].time.wrapping_sub(span_start));
                let into = seconds(t.wrapping_sub(span_start));
                let (d, anim) =
                    timing::speed_curve(self.speed, self.accel, into, span, j != 0, j != last);
                (d + f64::from(base), anim)
            }
        };
        let x = distance / f64::from(leg.curve.length);
        let fraction = if x < 0.0 {
            0.0
        } else if x < 1.0 {
            x as f32
        } else {
            1.0
        };
        let (pos, tangent) = leg.curve.frame(fraction, [1.0, 0.0, 0.0]);
        let h = normalize2([-tangent[0], -tangent[1]]);
        TransportSample {
            leg: index,
            map: leg.map,
            pos,
            heading: f64::from(h[1]).atan2(f64::from(h[0])) as f32,
            anim,
        }
    }

    /// Whether any leg is on `map_id`; a worldport keeps such a transport across the switch.
    pub fn touches_map(&self, map_id: u32) -> bool {
        self.legs.iter().any(|l| l.map == map_id)
    }
}

/// `0x5f9120(points, stops, leg)`: the curve over the leg's nodes, then its stop table. Stop `j` at
/// node `k` lies at the curve's distance to node `k`; its time is the leg's travel to it, each span
/// rounded to ms on its own, plus the waits before it. The end entry holds the whole travel and
/// waits, the length and wait 0.
fn time_leg(
    map: u32,
    points: Vec<[f32; 3]>,
    stops: &[(usize, u32)],
    v: f32,
    a: f32,
) -> Option<Leg> {
    let n = points.len();
    // 0x4532e3: 3 nodes or fewer write no segment length, and 0x5f9427-0x5f9450 never write the
    // leg's +8; 0x5f91a1: a stop on the last node leaves the table's last entry unwritten.
    if n <= 3 || stops.iter().any(|&(k, _)| k + 1 >= n) {
        return None;
    }
    let curve = Curve::new(points);
    let mut table = Vec::with_capacity(stops.len() + 1);
    let mut travel = 0u32;
    let mut waited = 0u32;
    let mut last = 0.0f32;
    for (j, &(k, wait)) in stops.iter().enumerate() {
        let at = curve.distance_to(k);
        travel = travel.wrapping_add(timing::span_to_stop_ms(at, last, v, a, j == 0));
        last = at as f32;
        table.push(Stop {
            time: travel.wrapping_add(waited),
            distance: last,
            wait,
        });
        waited = waited.wrapping_add(wait);
    }
    travel = travel.wrapping_add(timing::span_to_end_ms(
        curve.length,
        last,
        v,
        a,
        stops.is_empty(),
    ));
    table.push(Stop {
        time: travel.wrapping_add(waited),
        distance: curve.length,
        wait: 0,
    });
    Some(Leg {
        map,
        curve,
        start: 0,
        travel,
        end: 0,
        stops: table,
    })
}

/// `0x5f8cb0`: a vector of two over its length `√(x² + y²)`, unless that is below 2^-22.
fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let (x, y) = (f64::from(v[0]), f64::from(v[1]));
    let len = (x * x + y * y).sqrt();
    if len.abs() < SHORT {
        return v;
    }
    let r = 1.0 / len;
    [(r * x) as f32, (r * y) as f32]
}

#[cfg(test)]
mod tests;
