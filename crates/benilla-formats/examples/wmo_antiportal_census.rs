//! **Do any 1.12.1 WMOs author an ANTIPORTAL group, and if so which MOGP bit says so?**
//! `cargo run -p benilla-formats --example wmo_antiportal_census`.
//!
//! Later WoW documentation (wowdev.wiki `SMOGroupFlags`) names a high MOGP `flags` bit
//! `ANTIPORTAL`: a group with **no render batches at all**, kept only for its bounding box, which
//! the portal flood uses as an occluder rather than something it ever draws. Nothing confirms that
//! bit for build 5875 — the portal flood
//! ([`benilla_world::wmo_portal`]) only ever branches on `flags & 0x48` (EXTERIOR / EXTERIOR_LIT)
//! and never treats any group as occlusion-only. So the question is purely empirical: does the
//! shipped 5875 data contain such a group at all, and which bit(s) does it carry?
//!
//! **Bit-agnostic by construction.** Rather than assume the wowdev bit number applies unchanged to
//! this build, this walks every group of every root WMO in the install and builds two independent
//! signals, then cross-tabulates them:
//!
//! 1. A histogram of **every** MOGP flag bit (0..31) — how many groups set it, how many distinct
//!    roots contain at least one.
//! 2. The **batch-less set** — every group whose MOGP-owned MOBA chunk has zero render batches,
//!    found without looking at flags at all. This is the wowdev ANTIPORTAL *shape*: no geometry to
//!    draw, kept only for `MOGI`'s bounding box.
//!
//! If 1.12 ships any antiportal-shaped groups, they land in the batch-less set regardless of which
//! bit (if any) names them, and the cross-tab against the flag histogram is what tells us — from
//! the data, not a doc — which bit (if any) 5875 actually uses for it.
//!
//! **The run (2026-09-01, 815/815 roots, 5220/5220 groups, zero read/parse failures): yes, but no
//! single bit names it.** 15 groups across 9 roots ship with zero MOBA render batches — Naxxramas'
//! `frostwyrm_final01.wmo` (7 tiny connector groups), three of AQ40's boss-encounter "enterance"
//! shells, and the three Hyjal `worldtreeroots` collision volumes among them (see the batch-less
//! census below for the full list) — so the wowdev *shape* is real in 5875. But the cross-tab finds
//! **no MOGP bit set by all 15 and rare among the other 5205**: bit 0 (`0x1`) is set on literally
//! every group in the corpus (batch-less or not) and so distinguishes nothing; EXTERIOR (`0x8`)
//! covers 13/15 but also 1052/5205 batch-having groups; every other bit covers fewer than a third of
//! the batch-less set. Whatever these 15 groups are for — several read as pure collision volumes by
//! name/folder (`collidabledoodads\hyjal\worldtreeroots`) rather than deliberate portal occluders —
//! 5875 does not flag them with a dedicated ANTIPORTAL bit the way later clients' `SMOGroupFlags`
//! documentation describes. That is a fact about the *data*, not yet about the *engine*: whether
//! `WoW.exe` 5875's occluder pass (if it has one) singles these 15 out by shape rather than by flag
//! is a question about the binary this census doesn't answer.
//!
//! Only three MOGP bits are named anywhere in benilla's own code today (verified by grep over
//! `benilla-world/src/wmo_portal`, `benilla-assets/src/wmo.rs`, and
//! `benilla-formats/src/models/wmo`, 2026-09-01): `0x8` EXTERIOR, `0x40` EXTERIOR_LIT
//! ([`benilla_world::wmo_portal`]'s flood-defer/lighting-class bits), and `0x40000` SHOW_SKYBOX
//! ([`benilla_formats::WmoGroupInfo::show_skybox`]). The other candidate bits a hypothetical
//! ANTIPORTAL census might reach for — `0x1` (HAS_BSP/MOBN), `0x4` (VERTEXCOLOR), `0x200`
//! (HAS_LIGHTS/MOLR), `0x800` (HAS_LIQUID/MLIQ), `0x2000` (INTERIOR), `0x800000`/`0x1000000`
//! (second MOCV/MOTV) — are *not* named or tested anywhere in our reader: we discover MOLR/MLIQ/a
//! second MOTV by the sub-chunk's presence, never by these flag bits, and INTERIOR is our own
//! `flags & 0x48 == 0` derived test, not a single bit. None of those six get a name in the table
//! below; only a bit our code already states gets one, per the census's own charter.
//!
//! Every field per group comes from the same bytes the runtime reads: [`wmo_group_header`]'s
//! `flags` (MOGP `+0x08`, full 32 bits — **not** the two-bit-derived `interior`/`show_skybox` bools
//! `parse_wmo_root`'s MOGI reader exposes), `benilla_wmo::parse_wmo`'s `render_batches`/
//! `vertex_positions` (MOBA/MOVT), `wmo_group_header`'s `portal_ref_count` (MOPR, sliced from the
//! group's own header span), and the root's [`WmoGroupInfo`] bounding box (MOGI, the same box the
//! portal code treats as a loose AABB of the group's volume).
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use benilla_formats::{parse_wmo_root, wmo_group_header, Chain};
use benilla_wmo::{parse_wmo, ParsedWmo};

/// A bit counts as "rare" — and earns an example listing — below this many groups. Chosen well
/// above the size of any plausible antiportal set (a handful of buildings at most) and well below
/// EXTERIOR/EXTERIOR_LIT, which sit in the thousands.
const RARE_THRESHOLD: u32 = 200;
/// Example rows kept per rare bit.
const MAX_EXAMPLES: usize = 40;
/// Batch-less rows printed in full before the report falls back to "N more, elided".
const MAX_BATCHLESS_PRINTED: usize = 200;
/// Root-failure examples kept for the audit trail.
const MAX_FAILURE_EXAMPLES: usize = 20;
/// Rows in the "most EXTERIOR groups" table (see its section for why that ranking is worth having).
const MAX_EXTERIOR_RANK: usize = 25;

/// MOGP EXTERIOR — the bit the deferred-window worklist and its Pass-2 replay key on.
const EXTERIOR: u32 = 0x8;

/// One group's full fact row — what every example listing and the batch-less census print.
#[derive(Clone)]
struct GroupRow {
    /// The ROOT's chain path (e.g. `world\wmo\...\foo.wmo`); the group file is `{stem}_{NNN}.wmo`.
    root_path: String,
    group_index: u32,
    /// MOGP `flags` @ `+0x08`, all 32 bits, unfiltered.
    flags: u32,
    /// MOBA render-batch count.
    batches: usize,
    /// MOVT vertex count.
    vertices: usize,
    /// MOPR portal-ref count (this group's slice, from its own MOGP header span).
    portal_refs: u16,
    /// MOGI bounding box, WMO model space (WoW axes) — `None` if the root's MOGI table is shorter
    /// than its declared group count (never observed, but not asserted away).
    bbox: Option<([f32; 3], [f32; 3])>,
}

impl GroupRow {
    /// `{root}#{group}` for compact example rows.
    fn label(&self) -> String {
        format!("{}#{:03}", self.root_path, self.group_index)
    }

    /// Bounding-box extent (max − min) per axis, or `?` if MOGI didn't carry one.
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

/// Per-bit tally: how many groups/roots set it, split by batch-less vs. batch-having, plus a
/// bounded example list for the rare-bit report.
#[derive(Default)]
struct BitStat {
    groups: u32,
    roots: BTreeSet<String>,
    batchless_groups: u32,
    other_groups: u32,
    examples: Vec<GroupRow>,
}

/// Normalize a chain path the way the MPQ hash compares them (matches `wmo_ownerless_pools`).
fn key(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Bench a known-name label for a MOGP bit — filled in ONLY for bits benilla's own code already
/// names as a flag test (verified 2026-09-01; see the module doc for what was checked and ruled
/// out). Everything else prints blank: this census must not invent names the data alone can't back.
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

    // Every ROOT `.wmo` the chain lists, across every mounted archive — a group file's stem ends
    // `_NNN`, a root's does not (same heuristic as `wmo_ownerless_pools`). The listfile is not a
    // provably complete index of every archive (`Chain::list`'s own doc), so a root reachable ONLY
    // by name and absent from every listfile would be missed here; nothing in the corpus is known
    // to do that, but the failure counts below exist so a gap like it would show up as a number
    // rather than silence.
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
            // Two independent reads of the same bytes, deliberately: `wmo_group_header` for the raw
            // MOGP `flags`/MOPR span (the load-bearing 32 bits this census exists to histogram), and
            // `benilla_wmo::parse_wmo` for MOBA/MOVT counts — the same split `wmo_group_submeshes`
            // itself makes.
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

    // ==== which buildings own MANY exterior groups ================================================
    //
    // Not an antiportal question — a *consumer* one, and the reason it sits in this census rather
    // than in a throwaway script. The reference draws a building's EXTERIOR groups only through the
    // deferred portal windows the interior flood leaves behind, each group tested against its own
    // window's sub-frustum (`0x6b3c73`–`0x6b3d6f`; benilla decision 1826).
    // A building with ONE exterior group cannot show that law at work: a single whole-envelope shell
    // has a box wide enough to intersect any window, so it draws from everywhere and looks identical
    // either way — which is exactly why Stormwind (306 groups, one `0x8`) is the wrong subject to
    // eyeball the behaviour on. The buildings below, whose shells are split into many groups, are
    // where per-window culling is visible at all, and so where a retest belongs.
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
    // For every bit, how it splits across the batch-less/batch-having populations — the
    // flag-agnostic and flag-based signals collide here. A bit set by (close to) ALL batch-less
    // groups and by (close to) none of the batch-having ones is the data's own answer to "which bit
    // means antiportal in 5875"; the reverse (never set on a batch-less group) rules a candidate out
    // regardless of what later documentation calls it.
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
