//! Whether any shipped WMO group has the wowdev `ANTIPORTAL` shape (no MOBA render batches, only a
//! MOGI box) and which MOGP flag bit, if any, marks it: a histogram of every bit, the batch-less
//! groups found without reading flags, and the two cross-tabulated.
//! `cargo run -p benilla-formats --example wmo_antiportal_census`
//!
//! 15 shipped groups are batch-less, and no bit singles them out. The 1.12 client has no antiportal
//! branch on the group flags (the only bits above `0x10000` it tests are `0x40000` at `0x6b42e0`
//! and `0x20000` at `0x6c46a6`), so a batch-less group is invisible geometry, not an occluder.
//! Flags are MOGP's own 32 bits at `+0x08`, in every shipped group a superset of the MOGI copy.
//! Output is Blizzard data: never commit it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use benilla_formats::{parse_wmo_root, wmo_group_header, Chain};
use benilla_wmo::{parse_wmo, ParsedWmo};

/// Below this many groups a bit is rare and gets an example listing.
const RARE_THRESHOLD: u32 = 200;
const MAX_EXAMPLES: usize = 40;
const MAX_BATCHLESS_PRINTED: usize = 200;
const MAX_FAILURE_EXAMPLES: usize = 20;
const MAX_EXTERIOR_RANK: usize = 25;

/// The MOGP EXTERIOR bit.
const EXTERIOR: u32 = 0x8;

/// One group's facts, as the listings print them.
#[derive(Clone)]
struct GroupRow {
    /// The root's chain path; the group file is `{stem}_{NNN}.wmo`.
    root_path: String,
    group_index: u32,
    /// MOGP `flags` at `+0x08`, all 32 bits.
    flags: u32,
    /// MOBA render-batch count.
    batches: usize,
    /// MOVT vertex count.
    vertices: usize,
    /// MOPR portal-ref count, this group's slice from its own MOGP header.
    portal_refs: u16,
    /// MOGI bounding box in model space; `None` if the MOGI table is shorter than the group count.
    bbox: Option<([f32; 3], [f32; 3])>,
}

impl GroupRow {
    fn label(&self) -> String {
        format!("{}#{:03}", self.root_path, self.group_index)
    }

    fn bbox_str(&self) -> String {
        match self.bbox {
            Some((min, max)) => {
                let ext = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
                format!(
                    "{:.1}×{:.1}×{:.1} yd  [{:.0},{:.0},{:.0}]..[{:.0},{:.0},{:.0}]",
                    ext[0], ext[1], ext[2], min[0], min[1], min[2], max[0], max[1], max[2]
                )
            }
            None => "?".to_string(),
        }
    }
}

/// Per-bit tally of groups and roots, batch-less against batch-having, with examples.
#[derive(Default)]
struct BitStat {
    groups: u32,
    roots: BTreeSet<String>,
    batchless_groups: u32,
    other_groups: u32,
    examples: Vec<GroupRow>,
}

/// A chain path normalised as the MPQ hash compares it.
fn key(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// A name only for the MOGP bits benilla's own code tests as flags; the rest print blank.
fn known_name(bit: u32) -> &'static str {
    match bit {
        3 => "EXTERIOR (0x8)",
        6 => "EXTERIOR_LIT (0x40)",
        18 => "SHOW_SKYBOX (0x40000)",
        _ => "",
    }
}

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data()
        .ok_or_else(|| anyhow::anyhow!("no 1.12.1 install found (set $WOW_DATA)"))?;
    let chain = Chain::open(&data)?;

    // Every root `.wmo` the chain lists (a group file's stem ends `_NNN`, a root's does not); a
    // root absent from every listfile is missed.
    let roots: BTreeSet<String> = chain
        .list()?
        .into_iter()
        .map(|e| key(&e.name))
        .filter(|k| {
            let Some(stem) = k.strip_suffix(".wmo") else {
                return false;
            };
            !stem
                .rsplit('_')
                .next()
                .is_some_and(|t| t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    eprintln!("scanning {} listed WMO roots…", roots.len());

    let mut bits: Vec<BitStat> = (0..32).map(|_| BitStat::default()).collect();
    let mut batchless: Vec<GroupRow> = Vec::new();
    let mut exterior_per_root: BTreeMap<String, u32> = BTreeMap::new();
    let mut groups_scanned = 0u32;
    let mut roots_scanned = 0u32;
    let (mut root_read_fail, mut root_parse_fail) = (0u32, 0u32);
    let (mut group_read_fail, mut group_parse_fail) = (0u32, 0u32);
    let mut failure_examples: Vec<String> = Vec::new();

    for root_path in &roots {
        let bytes = match chain.read(root_path) {
            Ok(b) => b,
            Err(e) => {
                root_read_fail += 1;
                if failure_examples.len() < MAX_FAILURE_EXAMPLES {
                    failure_examples.push(format!("{root_path}: root unreadable ({e})"));
                }
                continue;
            }
        };
        let root = match parse_wmo_root(&bytes) {
            Ok(r) => r,
            Err(e) => {
                root_parse_fail += 1;
                if failure_examples.len() < MAX_FAILURE_EXAMPLES {
                    failure_examples.push(format!("{root_path}: root did not parse ({e})"));
                }
                continue;
            }
        };
        roots_scanned += 1;
        let stem = root_path.strip_suffix(".wmo").unwrap_or(root_path);
        let infos = root.group_infos();

        for gi in 0..root.group_count() {
            let group_path = format!("{stem}_{gi:03}.wmo");
            let gbytes = match chain.read(&group_path) {
                Ok(b) => b,
                Err(_) => {
                    group_read_fail += 1;
                    continue;
                }
            };
            // `wmo_group_header` for the raw MOGP flags and MOPR span, `parse_wmo` for the MOBA and
            // MOVT counts, the split `wmo_group_submeshes` makes.
            let Some(header) = wmo_group_header(&gbytes) else {
                group_parse_fail += 1;
                continue;
            };
            let Ok(ParsedWmo::Group(group)) = parse_wmo(&mut Cursor::new(gbytes.as_slice())) else {
                group_parse_fail += 1;
                continue;
            };
            groups_scanned += 1;

            let bbox = infos.get(gi as usize).map(|i| (i.bbox_min, i.bbox_max));
            let row = GroupRow {
                root_path: root_path.clone(),
                group_index: gi,
                flags: header.flags,
                batches: group.render_batches.len(),
                vertices: group.vertex_positions.len(),
                portal_refs: header.portal_ref_count,
                bbox,
            };
            let is_batchless = row.batches == 0;

            for (bit, stat) in bits.iter_mut().enumerate() {
                if header.flags & (1u32 << bit) == 0 {
                    continue;
                }
                stat.groups += 1;
                stat.roots.insert(root_path.clone());
                if is_batchless {
                    stat.batchless_groups += 1;
                } else {
                    stat.other_groups += 1;
                }
                if stat.examples.len() < MAX_EXAMPLES {
                    stat.examples.push(row.clone());
                }
            }

            if header.flags & EXTERIOR != 0 {
                *exterior_per_root.entry(root_path.clone()).or_default() += 1u32;
            }

            if is_batchless {
                batchless.push(row);
            }
        }
    }

    // ==== headline ================================================================================
    println!("==== WMO ANTIPORTAL census (1.12.1 / build 5875) ====\n");
    println!(
        "roots: {roots_scanned} parsed / {} listed  (read failures {root_read_fail}, parse failures {root_parse_fail})",
        roots.len()
    );
    println!(
        "groups: {groups_scanned} parsed  (read failures {group_read_fail}, parse failures {group_parse_fail})\n"
    );

    let batchless_roots: BTreeSet<&str> = batchless.iter().map(|r| r.root_path.as_str()).collect();
    println!(
        "batch-less groups (zero MOBA render batches — the wowdev ANTIPORTAL shape): {} across {} roots",
        batchless.len(),
        batchless_roots.len()
    );
    if batchless.is_empty() {
        println!(
            "VERDICT: ZERO — no shipped 1.12.1 WMO authors an antiportal-shaped group. Every group \
in the corpus that carries a MOGI bounding box also carries at least one MOBA render batch."
        );
    } else {
        println!(
            "VERDICT: {} antiportal-shaped group(s) shipped — see the batch-less census and the \
cross-tab below for which flag bit(s) the data implicates.",
            batchless.len()
        );
    }
    if !failure_examples.is_empty() {
        println!("\nroot failures (up to {MAX_FAILURE_EXAMPLES}):");
        for f in &failure_examples {
            println!("  {f}");
        }
    }

    // ==== which buildings own many exterior groups ============================================
    // The reference draws exterior groups only through the deferred portal windows the interior
    // flood leaves, each against its own window's sub-frustum (`0x6b3c73`-`0x6b3d6f`); only a
    // building whose shell is split into many groups shows that culling.
    println!("\n---- roots by EXTERIOR (0x8) group count, top {MAX_EXTERIOR_RANK} ----");
    let mut ranked: Vec<(&String, &u32)> = exterior_per_root.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let multi = ranked.iter().filter(|(_, &n)| n > 1).count();
    println!(
        "{multi} of {} roots with any exterior group have MORE THAN ONE",
        ranked.len()
    );
    for (path, n) in ranked.iter().take(MAX_EXTERIOR_RANK) {
        println!("  {n:4}  {path}");
    }

    // ==== the full 32-bit histogram ===============================================================
    println!("\n---- MOGP flags histogram (every bit 0..31) ----");
    println!(
        "{:>3} {:>10}  {:>8} {:>8}  known name",
        "bit", "mask", "groups", "roots"
    );
    for (bit, stat) in bits.iter().enumerate() {
        if stat.groups == 0 {
            continue;
        }
        println!(
            "{:>3} {:>#10x}  {:>8} {:>8}  {}",
            bit,
            1u32 << bit,
            stat.groups,
            stat.roots.len(),
            known_name(bit as u32)
        );
    }
    let unset_bits: Vec<u32> = bits
        .iter()
        .enumerate()
        .filter(|(_, s)| s.groups == 0)
        .map(|(b, _)| b as u32)
        .collect();
    if !unset_bits.is_empty() {
        println!(
            "\nbits never set by any shipped group: {}",
            unset_bits
                .iter()
                .map(|b| format!("{b}(0x{:x})", 1u32 << b))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // ==== rare-bit examples ========================================================================
    println!(
        "\n---- rare bits (< {RARE_THRESHOLD} groups): up to {MAX_EXAMPLES} example rows each ----"
    );
    let mut any_rare = false;
    for (bit, stat) in bits.iter().enumerate() {
        if stat.groups == 0 || stat.groups >= RARE_THRESHOLD {
            continue;
        }
        any_rare = true;
        println!(
            "\nbit {bit} (0x{:x}) — {} groups / {} roots{}",
            1u32 << bit,
            stat.groups,
            stat.roots.len(),
            if known_name(bit as u32).is_empty() {
                String::new()
            } else {
                format!("  [{}]", known_name(bit as u32))
            }
        );
        for r in &stat.examples {
            println!(
                "    {:<48} flags {:#010x}  batches {:>3} verts {:>5} portal-refs {:>3}  bbox {}",
                r.label(),
                r.flags,
                r.batches,
                r.vertices,
                r.portal_refs,
                r.bbox_str()
            );
        }
        if stat.groups as usize > stat.examples.len() {
            println!(
                "    … {} more not shown",
                stat.groups as usize - stat.examples.len()
            );
        }
    }
    if !any_rare {
        println!("(none — every set bit is carried by >= {RARE_THRESHOLD} groups)");
    }

    // ==== batch-less census ========================================================================
    println!(
        "\n---- batch-less groups (zero MOBA — flag-agnostic antiportal shape): {} ----",
        batchless.len()
    );
    for r in batchless.iter().take(MAX_BATCHLESS_PRINTED) {
        println!(
            "  {:<48} flags {:#010x}  verts {:>5} portal-refs {:>3}  bbox {}",
            r.label(),
            r.flags,
            r.vertices,
            r.portal_refs,
            r.bbox_str()
        );
    }
    if batchless.len() > MAX_BATCHLESS_PRINTED {
        println!(
            "  … {} more not shown (true count {})",
            batchless.len() - MAX_BATCHLESS_PRINTED,
            batchless.len()
        );
    }

    // ==== cross-tab: which bit(s) does the batch-less set actually carry? =========================
    // A bit set by every batch-less group and rare among the rest would mark antiportals.
    let other_total = groups_scanned - batchless.len() as u32;
    println!(
        "\n---- cross-tab: bit occurrence, batch-less ({} groups) vs. batch-having ({} groups) ----",
        batchless.len(),
        other_total
    );
    if batchless.is_empty() {
        println!(
            "(the batch-less set is empty, so there is nothing to cross-tabulate — no bit can be \
implicated as ANTIPORTAL from this corpus)"
        );
    } else {
        println!(
            "{:>3} {:>10}  {:>10} {:>10}  known name",
            "bit", "mask", "batch-less", "batch-have"
        );
        for (bit, stat) in bits.iter().enumerate() {
            if stat.batchless_groups == 0 && stat.other_groups == 0 {
                continue;
            }
            println!(
                "{:>3} {:>#10x}  {:>6}/{:<3} {:>6}/{:<3}  {}",
                bit,
                1u32 << bit,
                stat.batchless_groups,
                batchless.len(),
                stat.other_groups,
                other_total,
                known_name(bit as u32)
            );
        }
        let implicated: Vec<u32> = bits
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.batchless_groups == batchless.len() as u32 && s.other_groups < other_total / 10
            })
            .map(|(b, _)| b as u32)
            .collect();
        if implicated.is_empty() {
            println!(
                "\nno bit is set by EVERY batch-less group while staying rare among batch-having \
groups — the batch-less set carries no single common flag distinguishing it from the rest of the \
corpus."
            );
        } else {
            println!(
                "\nimplicated bit(s) — set by every batch-less group, and rare (< 10%) among \
batch-having groups: {}",
                implicated
                    .iter()
                    .map(|b| format!("bit {b} (0x{:x})", 1u32 << b))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    Ok(())
}
