//! Corpus scans over the bone tree and what addresses it: bone tracks, attachments, event markers.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use benilla_formats::Chain;

use crate::model_key;

/// List the models where a table scan for an attachment id and the reference's `lookup[id]`
/// (`0x710310`) disagree: `+id` only the lookup reaches, `-id` only the scan reaches (the
/// reference cannot address it), `id:a→b` a different record.
pub fn attachscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut with_points, mut divergent) = (0u32, 0u32, 0u32);
    let mut by_family: BTreeMap<String, u32> = BTreeMap::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Ok(format) = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes)) else {
            continue;
        };
        let model = format.model();
        scanned += 1;
        if model.attachments.is_empty() && model.attach_lookup.is_empty() {
            continue;
        }
        with_points += 1;
        let mut scan: BTreeMap<u16, usize> = BTreeMap::new();
        for (i, a) in model.attachments.iter().enumerate() {
            scan.entry(a.id).or_insert(i);
        }
        let lookup: BTreeMap<u16, usize> = (0..model.attach_lookup.len())
            .filter_map(|id| {
                let idx = *model.attach_lookup.get(id)?;
                (idx != 0xffff && (idx as usize) < model.attachments.len())
                    .then_some((id as u16, idx as usize))
            })
            .collect();
        let mut diffs: Vec<String> = Vec::new();
        for (&id, &idx) in &lookup {
            match scan.get(&id) {
                Some(&s) if s != idx => diffs.push(format!("{id}:{s}→{idx}")),
                Some(_) => {}
                None => diffs.push(format!("+{id}")),
            }
        }
        for &id in scan.keys() {
            if !lookup.contains_key(&id) {
                diffs.push(format!("-{id}"));
            }
        }
        if !diffs.is_empty() {
            divergent += 1;
            *by_family.entry(model_key(&name)).or_default() += 1;
            println!(
                "{:<62} recs {:>2}  lookup {:>2}  {}",
                name,
                model.attachments.len(),
                lookup.len(),
                diffs.join(" ")
            );
        }
    }
    eprintln!(
        "{scanned} models scanned, {with_points} with attachment data, \
         {divergent} where a table scan disagrees with the lookup"
    );
    for (family, n) in by_family {
        eprintln!("  {family}: {n}");
    }
    Ok(())
}

/// One bone `M2Track` read raw, so the check never goes through the parser it tests: v256 stride
/// `0x1c`, interp@0, gseq@2, then `M2Array`s ranges@0x04, timestamps@0x0c, values@0x14.
struct RawBoneTrack {
    interp: u16,
    gseq: u16,
    ranges: Vec<(u32, u32)>,
    ts: Vec<u32>,
    vals: Vec<[f32; 4]>,
    comps: usize,
}

impl RawBoneTrack {
    fn read(b: &[u8], o: usize, comps: usize) -> Option<Self> {
        let u32_at = |p: usize| -> Option<u32> {
            b.get(p..p + 4)
                .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        };
        let u16_at = |p: usize| -> Option<u16> {
            b.get(p..p + 2)
                .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        };
        let f32_at = |p: usize| -> Option<f32> {
            b.get(p..p + 4)
                .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
        };
        let (interp, gseq) = (u16_at(o)?, u16_at(o + 2)?);
        let (rn, ro) = (u32_at(o + 0x04)? as usize, u32_at(o + 0x08)? as usize);
        let (tn, to) = (u32_at(o + 0x0c)? as usize, u32_at(o + 0x10)? as usize);
        let (vn, vo) = (u32_at(o + 0x14)? as usize, u32_at(o + 0x18)? as usize);
        let ranges = (0..rn)
            .map_while(|i| Some((u32_at(ro + i * 8)?, u32_at(ro + i * 8 + 4)?)))
            .collect();
        let stride = comps * 4;
        let n = tn.min(vn);
        let mut ts = Vec::with_capacity(n);
        let mut vals = Vec::with_capacity(n);
        for i in 0..n {
            let Some(t) = u32_at(to + i * 4) else { break };
            let mut v = [0.0f32; 4];
            let mut ok = true;
            for (c, slot) in v.iter_mut().take(comps).enumerate() {
                match f32_at(vo + i * stride + c * 4) {
                    Some(f) => *slot = f,
                    None => ok = false,
                }
            }
            if !ok {
                break;
            }
            ts.push(t);
            vals.push(v);
        }
        Some(Self {
            interp,
            gseq,
            ranges,
            ts,
            vals,
            comps,
        })
    }

    /// The reference's sample at absolute `t_ms` for file slot `slot`: `0x713d50`'s key search,
    /// then the lerp or step of `0x713ea0`/`0x71af20`.
    fn reference(&self, slot: usize, t_ms: u32) -> Option<[f32; 4]> {
        let last = self.ts.len().checked_sub(1)?;
        // `0x713d59`: the window is `ranges[slot]` when present, else the whole key list.
        let (lo, hi) = match self.ranges.get(slot) {
            Some(&(lo, hi)) => (lo as usize, hi as usize),
            None => (0, last),
        };
        let (lo, hi) = (lo.min(last), hi.min(last));
        // `0x713d7b`: a collapsed window resolves to `keys[lo]` outright.
        if lo >= hi {
            return Some(self.vals[lo]);
        }
        let mut k0 = lo;
        for (k, &ts) in self.ts.iter().enumerate().take(hi + 1).skip(lo) {
            if ts <= t_ms {
                k0 = k;
            } else {
                break;
            }
        }
        // `0x713e45`: `k1 = k0 + 1`, bounded by the total key count, not the window's `hi`.
        if self.interp == 0 || k0 + 1 > last {
            return Some(self.vals[k0]);
        }
        let (t0, t1) = (self.ts[k0], self.ts[k0 + 1]);
        if t1 <= t0 {
            return Some(self.vals[k0]);
        }
        let f = (t_ms as f32 - t0 as f32) / (t1 as f32 - t0 as f32);
        let (a, b) = (self.vals[k0], self.vals[k0 + 1]);
        let mut out = [0.0f32; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = a[i] + (b[i] - a[i]) * f;
        }
        Some(out)
    }

    /// A plain hold, the answer without the `ranges` window: the band's first and last own keys,
    /// or the nearest out-of-band key when it has none, as `(start, end, band_was_empty)`.
    fn benilla_today(&self, start: u32, end: u32) -> Option<([f32; 4], [f32; 4], bool)> {
        let inb: Vec<usize> = (0..self.ts.len())
            .filter(|&k| self.ts[k] >= start && self.ts[k] <= end)
            .collect();
        if let (Some(&f), Some(&l)) = (inb.first(), inb.last()) {
            // Bevy holds the first key before it and the last key after it.
            return Some((self.vals[f], self.vals[l], false));
        }
        let mut best: Option<(u32, usize)> = None;
        for (k, &ts) in self.ts.iter().enumerate() {
            let d = if ts < start { start - ts } else { ts - end };
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, k));
            }
        }
        let (_, k) = best?;
        Some((self.vals[k], self.vals[k], true))
    }

    /// Distance in the channel's units: degrees for a quaternion, by the chord form since
    /// `acos(dot)` floors near 0.04° in f32; model units otherwise.
    fn delta(&self, a: [f32; 4], b: [f32; 4]) -> f32 {
        if self.comps == 4 {
            let norm = |q: [f32; 4]| {
                let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                if n > 0.0 {
                    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
                } else {
                    q
                }
            };
            let (a, mut b) = (norm(a), norm(b));
            if a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                b = [-b[0], -b[1], -b[2], -b[3]];
            }
            let d = a
                .iter()
                .zip(b)
                .map(|(x, y)| (x - y) * (x - y))
                .sum::<f32>()
                .sqrt();
            2.0 * (d / 2.0).min(1.0).asin().to_degrees()
        } else {
            (0..3)
                .map(|i| (a[i] - b[i]) * (a[i] - b[i]))
                .sum::<f32>()
                .sqrt()
        }
    }
}

/// Measure, per bone track and sequence band, how far a plain key hold sits from the reference's
/// sampler (`0x713d50`): an empty band samples the `ranges[slot]` window, past a band's last key
/// the reference keeps lerping toward `k0 + 1` (bounded by the total key count, not the window),
/// and a step track (`interp == 0`) copies `keys[k0]` where the emitted clips glide. It also
/// checks that no band's keys fall outside its own window, which makes reading the window safe.
pub fn bonescan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    // Two orders above the angle formula's f32 noise (about 1e-4°), far below what shows.
    const ROT_EPS: f32 = 0.01; // degrees
    const VEC_EPS: f32 = 1e-4; // model units
    let (mut scanned, mut models_step, mut models_empty_differ, mut models_edge_differ) =
        (0u32, 0u32, 0u32, 0u32);
    let (mut slots_keyed, mut slots_empty) = (0u64, 0u64);
    let (mut empty_differ, mut edge_differ, mut ranges_absent, mut window_violation) =
        (0u64, 0u64, 0u64, 0u64);
    let (mut step_tracks, mut total_tracks, mut step_bands) = (0u64, 0u64, 0u64);
    let (mut gseq_tracks, mut gseq_multi, mut gseq_orphan) = (0u64, 0u64, 0u64);
    let (mut gseq_ranges_absent, mut gseq_ranges_restrict) = (0u64, 0u64);
    let mut gseq_orphan_models: BTreeSet<String> = BTreeSet::new();
    let mut gseq_restrict_models: BTreeSet<String> = BTreeSet::new();
    let mut emitted_extra = 0u64;
    // (empty-band peak, keyed-edge peak) per channel kind.
    let (mut peak_rot, mut peak_vec) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    let mut emitted_models: BTreeSet<String> = BTreeSet::new();
    // Worst offender per class: (delta, model, bone, channel, slot).
    let mut worst_empty: Vec<(f32, String, usize, &'static str, usize)> = Vec::new();
    let mut worst_edge: Vec<(f32, String, usize, &'static str, usize)> = Vec::new();
    let mut step_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut worst_step: Vec<(f32, String, usize, &'static str, usize, usize)> = Vec::new();
    for name in names {
        let Ok(b) = chain.read_file(&name) else {
            continue;
        };
        if b.len() < 0x40 || &b[0..4] != b"MD20" {
            continue;
        }
        scanned += 1;
        let u32_at = |o: usize| -> usize {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize)
                .unwrap_or(0)
        };
        // Sequences in file order (count@0x1c, ofs@0x20, stride 0x44), the order `ranges` indexes.
        let (sn, so) = (u32_at(0x1c), u32_at(0x20));
        let seqs: Vec<(u32, u32)> = (0..sn)
            .map_while(|i| {
                let e = so + i * 0x44;
                (e + 0x44 <= b.len()).then(|| (u32_at(e + 4) as u32, u32_at(e + 8) as u32))
            })
            .collect();
        // globalSequences @0x14/0x18: a duration per entry, 0 for no period.
        let (gn, go) = (u32_at(0x14), u32_at(0x18));
        let gseq_period = |g: u16| -> u32 {
            let i = g as usize;
            if i >= gn {
                0
            } else {
                u32_at(go + i * 4) as u32
            }
        };
        let (bn, bo) = (u32_at(0x34), u32_at(0x38));
        let (mut m_step, mut m_empty, mut m_edge) = (0u64, 0u64, 0u64);
        for bi in 0..bn {
            let brec = bo + bi * 0x6c;
            if brec + 0x6c > b.len() {
                break;
            }
            for (off, comps, ch) in [(0x0c, 3, "trans"), (0x28, 4, "rot"), (0x44, 3, "scale")] {
                let Some(tr) = RawBoneTrack::read(&b, brec + off, comps) else {
                    continue;
                };
                if tr.ts.is_empty() {
                    continue;
                }
                total_tracks += 1;
                if tr.interp == 0 {
                    step_tracks += 1;
                    m_step += 1;
                    // A step shows only where a band holds two keys: the reference snaps there.
                    for (slot, &(start, end)) in seqs.iter().enumerate() {
                        if end <= start {
                            continue;
                        }
                        let inb: Vec<usize> = (0..tr.ts.len())
                            .filter(|&k| tr.ts[k] >= start && tr.ts[k] <= end)
                            .collect();
                        if inb.len() < 2 {
                            continue;
                        }
                        step_bands += 1;
                        let jump = inb
                            .windows(2)
                            .map(|w| tr.delta(tr.vals[w[0]], tr.vals[w[1]]))
                            .fold(0.0f32, f32::max);
                        if jump > if comps == 4 { ROT_EPS } else { VEC_EPS } {
                            worst_step.push((jump, name.clone(), bi, ch, slot, inb.len()));
                        }
                    }
                }
                // A global-sequence track runs on its own clock: one key is a constant in every
                // clip, more go to `parse_m2_global_sequence_bones`, which needs a non-zero
                // period, so a multi-key track on a zero period plays nowhere. `0x713d50` picks
                // the `ranges` window before the gseq clock, so a narrow window clips the loop.
                if tr.gseq != 0xffff {
                    gseq_tracks += 1;
                    if tr.ts.len() > 1 {
                        gseq_multi += 1;
                        if gseq_period(tr.gseq) == 0 {
                            gseq_orphan += 1;
                            gseq_orphan_models.insert(name.clone());
                        }
                        match tr.ranges.len() {
                            0 => gseq_ranges_absent += 1,
                            _ => {
                                if tr
                                    .ranges
                                    .iter()
                                    .any(|&(lo, hi)| lo != 0 || hi as usize != tr.ts.len() - 1)
                                {
                                    gseq_ranges_restrict += 1;
                                    gseq_restrict_models.insert(name.clone());
                                }
                            }
                        }
                    }
                    continue;
                }
                let eps = if comps == 4 { ROT_EPS } else { VEC_EPS };
                for (slot, &(start, end)) in seqs.iter().enumerate() {
                    if end <= start {
                        continue;
                    }
                    if tr.ranges.get(slot).is_none() {
                        ranges_absent += 1;
                    }
                    let Some((mine_a, mine_b, was_empty)) = tr.benilla_today(start, end) else {
                        continue;
                    };
                    let (Some(ref_a), Some(ref_b)) =
                        (tr.reference(slot, start), tr.reference(slot, end))
                    else {
                        continue;
                    };
                    let d = tr.delta(mine_a, ref_a).max(tr.delta(mine_b, ref_b));
                    let peak = if comps == 4 {
                        &mut peak_rot
                    } else {
                        &mut peak_vec
                    };
                    if was_empty {
                        peak.0 = peak.0.max(d);
                        slots_empty += 1;
                        if d > eps {
                            empty_differ += 1;
                            m_empty += 1;
                            worst_empty.push((d, name.clone(), bi, ch, slot));
                        }
                    } else {
                        peak.1 = peak.1.max(d);
                        slots_keyed += 1;
                        // Reading the window is safe only if the band's own keys sit inside it.
                        if let Some(&(lo, hi)) = tr.ranges.get(slot) {
                            let inb: Vec<usize> = (0..tr.ts.len())
                                .filter(|&k| tr.ts[k] >= start && tr.ts[k] <= end)
                                .collect();
                            if inb.first().is_some_and(|&f| (f as u32) < lo)
                                || inb.last().is_some_and(|&l| (l as u32) > hi)
                            {
                                window_violation += 1;
                            }
                        }
                        if d > eps {
                            edge_differ += 1;
                            m_edge += 1;
                            worst_edge.push((d, name.clone(), bi, ch, slot));
                        }
                    }
                }
            }
        }
        // What the parse emits: a band-slot with more keys than its own in-band count carries a
        // head or tail sample from the window.
        for a in benilla_formats::parse_m2_animations(&b) {
            for bk in &a.bones {
                for (off, comps, emitted) in [
                    (0x0c, 3, bk.translation.len()),
                    (0x28, 4, bk.rotation.len()),
                    (0x44, 3, bk.scale.len()),
                ] {
                    let brec = bo + bk.bone as usize * 0x6c;
                    let Some(tr) = RawBoneTrack::read(&b, brec + off, comps) else {
                        continue;
                    };
                    if tr.gseq != 0xffff {
                        continue;
                    }
                    let inb = tr
                        .ts
                        .iter()
                        .filter(|&&ts| ts >= a.start_ms && ts <= a.end_ms)
                        .count();
                    if emitted > inb.max(1) {
                        emitted_extra += 1;
                        emitted_models.insert(name.clone());
                    }
                }
            }
        }
        if m_step > 0 {
            models_step += 1;
            step_models.insert(name.clone(), m_step);
        }
        if m_empty > 0 {
            models_empty_differ += 1;
        }
        if m_edge > 0 {
            models_edge_differ += 1;
        }
        for w in [&mut worst_empty, &mut worst_edge] {
            if w.len() > 4096 {
                w.sort_by(|a, b| b.0.total_cmp(&a.0));
                w.truncate(512);
            }
        }
        if worst_step.len() > 4096 {
            worst_step.sort_by(|a, b| b.0.total_cmp(&a.0));
            worst_step.truncate(512);
        }
    }
    worst_empty.sort_by(|a, b| b.0.total_cmp(&a.0));
    worst_edge.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("scanned {scanned} models · {total_tracks} keyed bone tracks");
    println!("\nbone×channel×sequence slots: {slots_keyed} keyed, {slots_empty} EMPTY");
    println!("  ranges array absent for the slot: {ranges_absent}");
    println!("  band keys OUTSIDE their own ranges window: {window_violation}  (must be 0 to adopt the window)");
    for (label, n, models, worst) in [
        (
            "EMPTY band  (nearest-key vs the file's window)",
            empty_differ,
            models_empty_differ,
            &worst_empty,
        ),
        (
            "HELD edge   (hold vs the reference's ongoing lerp)",
            edge_differ,
            models_edge_differ,
            &worst_edge,
        ),
    ] {
        println!("\n{label}: {n} slots differ, across {models} models");
        for (d, m, bi, ch, slot) in worst.iter().take(15) {
            let unit = if *ch == "rot" { "deg" } else { "u" };
            println!("  {d:9.3}{unit:<4} {m:<52} bone {bi:<4} {ch:<6} slot {slot}");
        }
    }
    worst_step.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "\nSTEP bone tracks (interp == 0, the reference copies keys[k0]): {step_tracks} of \
         {total_tracks}, in {models_step} models"
    );
    println!(
        "  bands where a step track holds 2+ keys (we glide, the reference snaps): {step_bands}; \
         {} of them snap by more than the noise floor. Worst:",
        worst_step.len()
    );
    for (d, m, bi, ch, slot, n) in worst_step.iter().take(20) {
        let unit = if *ch == "rot" { "deg" } else { "u" };
        println!("  {d:9.3}{unit:<4} {m:<52} bone {bi:<4} {ch:<6} slot {slot:<4} ({n} keys)");
    }
    println!(
        "\nGLOBAL-SEQUENCE bone tracks: {gseq_tracks} total, {gseq_multi} multi-key (the \
         `GlobalSeqDrive` lane's input; single-key ones are constants folded into every clip)"
    );
    println!(
        "  multi-key on a ZERO-period global sequence — sampled by NEITHER lane: {gseq_orphan}, \
         in {} models",
        gseq_orphan_models.len()
    );
    for m in gseq_orphan_models.iter().take(10) {
        println!("      {m}");
    }
    println!(
        "  their `ranges` window: absent (whole key list) {gseq_ranges_absent}, RESTRICTIVE \
         (not [0, last] — would clip the loop) {gseq_ranges_restrict} in {} models",
        gseq_restrict_models.len()
    );
    for m in gseq_restrict_models.iter().take(10) {
        println!("      {m}");
    }
    println!(
        "\nworst disagreement anywhere in the corpus (a BOUND, not a threshold count):\n  \
         rotation: empty band {:.5} deg, keyed edge {:.5} deg\n  \
         translation/scale: empty band {:.6} u, keyed edge {:.6} u",
        peak_rot.0, peak_rot.1, peak_vec.0, peak_vec.1
    );
    println!(
        "\nEMITTED clips: {emitted_extra} band-slots carry a head/tail sample beyond their own \
         in-band keys, in {} models",
        emitted_models.len()
    );
    for m in emitted_models.iter().take(10) {
        println!("      {m}");
    }
    println!("  models with any step track:");
    for (m, n) in step_models.iter().take(10) {
        println!("  {n:>6}  {m}");
    }
    if step_models.len() > 10 {
        println!("  … and {} more models", step_models.len() - 10);
    }
    Ok(())
}

/// Census each 4CC's `M2Event` records by `bone` and `position`. The reference hands an event's
/// arm its own world point, the model-space rest `position` moved by its bone's live matrix and the
/// placement (`0x5f3e20`'s `[ebp+0x10]`), which differs from the model root only for a record off
/// the origin or on a bone whose chain any sequence keys.
pub fn eventmarkerscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    /// Yards from the model origin past which a marker counts as off it: a presence threshold.
    const OFF_ORIGIN: f32 = 0.01;
    let names = super::m2_names(chain, prefix)?;
    // 4CC → (records, off-origin, on a keyed bone, max offset, the model + bone at that max)
    let mut per_tag: BTreeMap<String, (u32, u32, u32, f32, String)> = BTreeMap::new();
    let (mut scanned, mut carriers) = (0u32, 0u32);
    // Models that author a 4CC more than once, and those whose copies sit apart: the reference
    // fires a specific record and takes its point, so a first-match lookup by tag is wrong there.
    let (mut dup_models, mut dup_split) = (0u32, 0u32);
    let mut dup_examples: Vec<String> = Vec::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Ok(format) = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes)) else {
            continue;
        };
        let model = format.model();
        scanned += 1;
        if model.event_markers.is_empty() {
            continue;
        }
        carriers += 1;
        // The bones any sequence keys; a marker moves if any bone up its parent chain is keyed.
        let keyed: BTreeSet<u16> = benilla_formats::parse_m2_animations(&bytes)
            .iter()
            .flat_map(|a| a.bones.iter().map(|b| b.bone))
            .collect();
        let moves = |bone: u16| {
            let mut b = bone as i32;
            // Bounded by the bone count: a malformed parent cycle must not spin here.
            for _ in 0..model.bones.len() + 1 {
                let Ok(idx) = usize::try_from(b) else {
                    return false;
                };
                if keyed.contains(&(idx as u16)) {
                    return true;
                }
                let Some(rec) = model.bones.get(idx) else {
                    return false;
                };
                b = rec.parent as i32;
            }
            false
        };
        let mut by_ident: BTreeMap<[u8; 4], Vec<(u16, [f32; 3])>> = BTreeMap::new();
        for m in &model.event_markers {
            by_ident
                .entry(m.ident)
                .or_default()
                .push((m.bone, m.position));
        }
        let dups: Vec<_> = by_ident.iter().filter(|(_, v)| v.len() > 1).collect();
        if !dups.is_empty() {
            dup_models += 1;
            let split: Vec<_> = dups
                .iter()
                .filter(|(_, v)| v.iter().any(|x| *x != v[0]))
                .collect();
            if !split.is_empty() {
                dup_split += 1;
                if dup_examples.len() < 12 {
                    let tags: Vec<String> = split
                        .iter()
                        .map(|(i, v)| format!("{}×{}", String::from_utf8_lossy(&i[..]), v.len()))
                        .collect();
                    dup_examples.push(format!("  {name}  {}", tags.join(" ")));
                }
            }
        }
        for m in &model.event_markers {
            let d = (m.position[0].powi(2) + m.position[1].powi(2) + m.position[2].powi(2)).sqrt();
            let e = per_tag
                .entry(String::from_utf8_lossy(&m.ident).into_owned())
                .or_insert((0, 0, 0, 0.0, String::new()));
            e.0 += 1;
            e.1 += u32::from(d > OFF_ORIGIN);
            e.2 += u32::from(moves(m.bone));
            if d > e.3 {
                e.3 = d;
                e.4 = format!("{name} bone {}", m.bone);
            }
        }
    }
    println!(
        "=== M2 event markers: how far each 4CC's records sit from the model origin ===\n  \
         tag    records  off-origin  on a MOVING bone   max offset (yd)  where"
    );
    for (tag, (n, off, keyed, max, where_)) in &per_tag {
        println!("  {tag}  {n:>7}  {off:>10}  {keyed:>16}  {max:>16.3}  {where_}");
    }
    println!(
        "\n=== same-4CC duplicate records ===\n  {dup_models} model(s) author some 4CC more than \
         once; {dup_split} of them place the duplicates at DIFFERENT (bone, position) pairs — the \
         population where resolving a fired event by a first-match table scan gives the wrong \
         point, and the reason a fired event has to carry its own record's."
    );
    for e in &dup_examples {
        println!("{e}");
    }
    eprintln!(
        "{scanned} models scanned, {carriers} carry an event table. `position` is model space, so \
         a record at 0.000 with no keyed bone renders the model root and the event's own point the \
         SAME world position — the transform earns its code only on the rows above it."
    );
    Ok(())
}
