//! `LightIntBand`/`LightFloatBand` decoding and time-of-day interpolation, wrapping across
//! midnight.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, u32_at};
use crate::Chain;

/// Half-minutes in a game day (band time axis); `1440` = noon.
pub(super) const DAY: u32 = 2880;

/// One band row: parallel (time, value) pairs across the day, in file order (not always ascending).
pub(super) struct Band<T> {
    times: Vec<u32>,
    values: Vec<T>,
}

/// The shared band record: `ID, num, time[16], value[16]`, 34 fields, 136 bytes.
pub(super) fn band_schema(name: &str, value: FieldType) -> Schema {
    let mut s = Schema::new(name);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("num", FieldType::UInt32));
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("time{i}"), FieldType::UInt32));
    }
    for i in 0..16 {
        s.add_field(SchemaField::new(format!("value{i}"), value));
    }
    s
}

/// Decode a packed LightIntBand color (stored `0x00RRGGBB`, blue in the low byte) to sRGB 0..1.
fn decode_color(v: u32) -> [f32; 3] {
    [
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    ]
}

/// The `(i0, i1, frac)` segment bracketing `t`, walking `times` as a cycle in file order; `times`
/// must be non-empty.
fn segment(times: &[u32], t: u32) -> (usize, usize, f32) {
    let n = times.len();
    if n == 1 {
        return (0, 0, 0.0);
    }
    // 0x6d63e0: walk the keys as a cycle in file order; the first pair bracketing `t` wins, and
    // a pair that does not ascend wraps through midnight.
    let day = i64::from(DAY);
    let t = i64::from(t);
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b) = (i64::from(times[i]), i64::from(times[j]));
        let (b, t) = if b > a {
            if !(a <= t && t <= b) {
                continue;
            }
            (b, t)
        } else {
            if !(t >= a || t <= b) {
                continue;
            }
            (b + day, if t < a { t + day } else { t })
        };
        return (i, j, (t - a) as f32 / (b - a) as f32);
    }
    (n - 1, n - 1, 0.0)
}

pub(super) fn sample_float(b: &Band<f32>, t: u32) -> Option<f32> {
    if b.values.is_empty() {
        return None;
    }
    let (i0, i1, f) = segment(&b.times, t);
    Some(b.values[i0] + (b.values[i1] - b.values[i0]) * f)
}

pub(super) fn sample_color(b: &Band<u32>, t: u32) -> Option<[f32; 3]> {
    if b.values.is_empty() {
        return None;
    }
    let (i0, i1, f) = segment(&b.times, t);
    let (c0, c1) = (decode_color(b.values[i0]), decode_color(b.values[i1]));
    Some([
        c0[0] + (c1[0] - c0[0]) * f,
        c0[1] + (c1[1] - c0[1]) * f,
        c0[2] + (c1[2] - c0[2]) * f,
    ])
}

pub(super) fn load_bands(
    chain: &mut Chain,
    path: &str,
    schema: Schema,
) -> Result<HashMap<u32, Band<u32>>> {
    let bytes = chain
        .read_file(path)
        .with_context(|| format!("reading {path}"))?;
    let rs = parse(&bytes, schema, path)?;
    let mut m = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(num)) = (u32_at(r, 0), u32_at(r, 1)) else {
            continue;
        };
        let num = (num as usize).min(16);
        let times = (0..num).filter_map(|i| u32_at(r, 2 + i)).collect();
        let values = (0..num).filter_map(|i| u32_at(r, 18 + i)).collect();
        m.insert(id, Band { times, values });
    }
    Ok(m)
}

pub(super) fn load_float_bands(
    chain: &mut Chain,
    path: &str,
    schema: Schema,
) -> Result<HashMap<u32, Band<f32>>> {
    let bytes = chain
        .read_file(path)
        .with_context(|| format!("reading {path}"))?;
    let rs = parse(&bytes, schema, path)?;
    let mut m = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(num)) = (u32_at(r, 0), u32_at(r, 1)) else {
            continue;
        };
        let num = (num as usize).min(16);
        let times = (0..num).filter_map(|i| u32_at(r, 2 + i)).collect();
        let values = (0..num).filter_map(|i| f32_at(r, 18 + i)).collect();
        m.insert(id, Band { times, values });
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_walks_unordered_keys_as_a_cycle() {
        // LightParams 8's rows store [720, 1440, 0]: 09:00 sits between 06:00 and noon.
        assert_eq!(segment(&[720, 1440, 0], 1080), (0, 1, 0.5));
        // 18:00 lies on the noon-to-midnight pair.
        assert_eq!(segment(&[720, 1440, 0], 2160), (1, 2, 0.5));
        // [2640, 2520, 0, 1440]: the first pair wraps across most of the day and wins.
        let (i0, i1, f) = segment(&[2640, 2520, 0, 1440], 1440);
        assert_eq!((i0, i1), (0, 1));
        assert!((f - (1440.0 + 2880.0 - 2640.0) / 2760.0).abs() < 1e-6);
    }

    #[test]
    fn segment_wraps_across_midnight() {
        // Keys at 06:00 and 18:00 in half-minutes: 03:00 wraps from the last key, not clamps.
        let times = [720u32, 2160];
        let (i0, i1, f) = segment(&times, 360);
        assert_eq!((i0, i1), (1, 0));
        let span = 720 + DAY - 2160; // 1440
        assert!((f - (360 + DAY - 2160) as f32 / span as f32).abs() < 1e-6);
    }
}
