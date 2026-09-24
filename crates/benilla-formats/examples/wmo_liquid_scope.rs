//! Where liquid **scoping** can still get the wrong answer, read off the shipped files rather than
//! argued: `cargo run -p benilla-formats --example wmo_liquid_scope`.
//!
//! The "swim in air" family (0634 → 0635 → 0696 → 0701) is one defect met at successively finer
//! grain: a liquid footprint has no bound of its own, so each round found a wider set of positions
//! it was still claiming. This is the falsifier that says which bounds are actually load-bearing on
//! shipped content, and where — an argument about WMO liquid is otherwise unfalsifiable from inside
//! the game, where you can only stand in one room at a time.
//!
//! * **STOREY** — a placement holding a pool in one group and a **floor below it** in another.
//!   Placement scoping alone claims the lower room (submerged on dry stone, the B60 shape). This is
//!   what found decision 0701's Undercity site, and it now also reports whether 0701's floor rule
//!   closes each hit. Tested against the pool's real **wet cells**, not its bounding box: a liquid
//!   grid is sparse, so a bbox routinely spans dry ground it never touches.
//! * **OVERHANG** — how far a pool's wet cells reach *outside* their own group's box. This is why
//!   0701 bounds a pool by its group's Z **floor** and not by the reference's whole per-group AABB:
//!   on shipped content a pool overhangs its own room by up to 25 yd in XY, so an XY box test would
//!   newly reject pools that work today.
//! * **DRY-GROUP** — a group whose MOGP `groupLiquid` (@0x34) is not the `0xf` no-liquid sentinel
//!   but that carries **no MLIQ chunk**. The reference's `0x6b9f10` is reported to answer such a
//!   group with an unconditional hit at height `FLT_MAX`; we answer *nothing*, and since 0634/0696
//!   suppress the ADT leg indoors, the room reads DRY. **Still open** — three placed groups.
//!
//! Only placements found in a shipped ADT are reported — an unplaced WMO cannot be stood in — and
//! each hit prints a `.go` for the lower room's floor so it can be looked at.
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

use std::collections::BTreeMap;
use std::io::Cursor;

use benilla_formats::LiquidMesh;

/// MOGP `groupLiquid` @ 0x34: the value meaning "this group holds no liquid".
const NO_LIQUID: u32 = 0xf;

/// Half the 34133⅓-yard map (32 tiles × 533⅓) — the MODF→world origin shift.
const MAP_CENTER: f32 = 17066.666;

/// What one group file says about liquid.
struct Group {
    liquid_type: u32,
    /// The group's MLIQ surface in WMO model space, if it has one.
    liquid: Option<LiquidMesh>,
    /// MOGP bounding box (model space), @0x0c.
    lo: [f32; 3],
    hi: [f32; 3],
}

/// One placement of a model in the world.
struct Site {
    map: String,
    /// Placement origin in WoW world coords.
    pos: [f32; 3],
    rot: [f32; 3],
    unique_id: u32,
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], i: usize) -> Option<f32> {
    u32_at(b, i).map(f32::from_bits)
}

/// Walk top-level chunks for one whose reversed magic matches.
fn chunk<'a>(bytes: &'a [u8], magic: &[u8; 4]) -> Option<&'a [u8]> {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size = u32_at(bytes, off + 4)? as usize;
        let body = bytes.get(off + 8..off + 8 + size)?;
        if &bytes[off..off + 4] == magic {
            return Some(body);
        }
        off += 8 + size;
    }
    None
}

fn read_group(bytes: &[u8]) -> Option<Group> {
    let mogp = chunk(bytes, b"PGOM")?; // MOGP
    if mogp.len() < 0x44 {
        return None;
    }
    let corner = |i: usize| Some([f32_at(mogp, i)?, f32_at(mogp, i + 4)?, f32_at(mogp, i + 8)?]);
    Some(Group {
        liquid_type: u32_at(mogp, 0x34)?,
        liquid: benilla_formats::wmo_group_liquid_mesh(bytes),
        lo: corner(0x0c)?,
        hi: corner(0x18)?,
    })
}

/// Does this group file carry an MLIQ sub-chunk at all? (The MOGP payload is a 68-byte header
/// followed by its own sub-chunks.) Distinct from [`Group::liquid`], which is `None` for an
/// all-holes grid too.
fn has_mliq(bytes: &[u8]) -> bool {
    chunk(bytes, b"PGOM").is_some_and(|m| m.len() > 0x44 && chunk(&m[0x44..], b"QILM").is_some())
}

/// The model→world rotation for a MODF placement, in the **WoW** frame (Z up) — the `in_wow` half
/// of `benilla_assets::coords::placement_rotation`, as a plain 3×3 so this example needs no math
/// dependency. `Rx(90°)·Ry(ry−180°)·Rz(−rx)·Rx(rz−90°)`, applied to a model-space point.
fn model_to_world(rot_deg: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    let (rx, ry, rz) = (
        rot_deg[0].to_radians(),
        rot_deg[1].to_radians(),
        rot_deg[2].to_radians(),
    );
    let rot_x = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c]
    };
    let rot_y = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
    };
    let rot_z = |p: [f32; 3], a: f32| {
        let (s, c) = a.sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    };
    // Right-to-left, as the quaternion product composes.
    let p = rot_x(p, rz - std::f32::consts::FRAC_PI_2);
    let p = rot_z(p, -rx);
    let p = rot_y(p, ry - std::f32::consts::PI);
    rot_x(p, std::f32::consts::FRAC_PI_2)
}

fn go_at(site: &Site, model_pt: [f32; 3]) -> String {
    let r = model_to_world(site.rot, model_pt);
    format!(
        "{}: .go xyz {:.2} {:.2} {:.2}",
        site.map,
        site.pos[0] + r[0],
        site.pos[1] + r[1],
        site.pos[2] + r[2],
    )
}

/// Every wet cell of a liquid mesh as `(x0, y0, x1, y1, height)` in model space.
fn wet_cells(lq: &LiquidMesh) -> Vec<([f32; 4], f32)> {
    let (cols, rows) = (lq.grid[0] as usize, lq.grid[1] as usize);
    let mut out = Vec::new();
    for j in 0..rows.saturating_sub(1) {
        for i in 0..cols.saturating_sub(1) {
            if !lq.wet.get(j * (cols - 1) + i).copied().unwrap_or(false) {
                continue;
            }
            let c = [
                lq.positions[j * cols + i],
                lq.positions[j * cols + i + 1],
                lq.positions[(j + 1) * cols + i],
                lq.positions[(j + 1) * cols + i + 1],
            ];
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            let mut h = f32::MIN;
            for v in c {
                for a in 0..2 {
                    lo[a] = lo[a].min(v[a]);
                    hi[a] = hi[a].max(v[a]);
                }
                h = h.max(v[2]);
            }
            out.push(([lo[0], lo[1], hi[0], hi[1]], h));
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    // ---- pass 1: every WMO actually placed in the world, and where. ------------------------
    let mut placed: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    let adts: Vec<String> = chain
        .list()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.to_lowercase().ends_with(".adt"))
        .collect();
    eprintln!("scanning {} ADTs…", adts.len());
    for name in &adts {
        let Ok(bytes) = chain.read_file(name) else {
            continue;
        };
        let Ok(benilla_adt::ParsedAdt::Root(adt)) =
            benilla_adt::parse_adt(&mut Cursor::new(&*bytes))
        else {
            continue;
        };
        let lower = name.to_lowercase();
        let map = lower.split(['\\', '/']).nth(2).unwrap_or("?").to_string();
        for p in &adt.wmo_placements {
            let Some(model) = adt.wmos.get(p.name_id as usize) else {
                continue;
            };
            let e = placed
                .entry(model.to_lowercase().replace('/', "\\"))
                .or_default();
            if !e.iter().any(|s| s.map == map) {
                e.push(Site {
                    map: map.clone(),
                    pos: [
                        MAP_CENTER - p.position[2],
                        MAP_CENTER - p.position[0],
                        p.position[1],
                    ],
                    rot: p.rotation,
                    unique_id: p.unique_id,
                });
            }
        }
    }
    // A WMO-only map has no ADT at all — the whole world is the WDT's single `MODF` (20 of the 43
    // shipped maps). Scanning only ADTs therefore declared the Deeprun Tram, the
    // jails and every instance-shaped dungeon *unplaced*, which is the one thing this census uses
    // to decide a group is unreachable. The tram's own flooded sections are among the sites this
    // check was written for, so the blind spot was hiding exactly the sites asked about.
    let wdts: Vec<String> = chain
        .list()?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.to_lowercase().ends_with(".wdt"))
        .collect();
    eprintln!("scanning {} WDTs for WMO-only maps…", wdts.len());
    for name in &wdts {
        let Ok(bytes) = chain.read_file(name) else {
            continue;
        };
        let Ok(wdt) =
            benilla_wdt::WdtReader::new(Cursor::new(&*bytes), benilla_wdt::WowVersion::Classic)
                .read()
        else {
            continue;
        };
        let Some(g) = wdt.global_wmo() else { continue };
        let lower = name.to_lowercase();
        let map = lower.split(['\\', '/']).nth(2).unwrap_or("?").to_string();
        // No `MAP_CENTER − v` remap here: a global WMO's MODF is authored in world coords already
        // (falsified against all 26 of the server's entry points). Every shipped one is
        // (0,0,0), so a `.go` printed for one of these is the model-space offset itself.
        placed
            .entry(g.model.to_lowercase().replace('/', "\\"))
            .or_default()
            .push(Site {
                map,
                pos: g.position,
                rot: g.rotation,
                unique_id: 0,
            });
    }
    eprintln!("{} distinct WMO models placed in the world", placed.len());

    // ---- pass 2: read every placed model's groups. -----------------------------------------
    let mut dry_groups: Vec<String> = Vec::new();
    let mut storey: Vec<(f32, String)> = Vec::new();
    let mut overhang: Vec<(f32, String)> = Vec::new();
    let mut floor_misses = 0usize;
    // groupLiquid value -> (groups with an MLIQ chunk, groups without) — the sentinel sanity check:
    // `0xf` is only the "no liquid" marker if it is what the overwhelming majority of dry groups say.
    let mut type_census: BTreeMap<u32, (usize, usize)> = BTreeMap::new();

    for (model, sites) in &placed {
        let stem = model.strip_suffix(".wmo").unwrap_or(model);
        let mut groups: Vec<Group> = Vec::new();
        for gi in 0.. {
            let Ok(gb) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                break;
            };
            let Some(g) = read_group(&gb) else { break };
            let e = type_census.entry(g.liquid_type).or_default();
            if has_mliq(&gb) {
                e.0 += 1;
            } else {
                e.1 += 1;
                if g.liquid_type != NO_LIQUID {
                    // The ROOM, not the building origin: a placement origin routinely sits
                    // inside rock, so a `.go` to it is unusable for an eyeball check. Aim at the
                    // group box's own centre, just off its floor.
                    let mid = |a: usize| (g.lo[a] + g.hi[a]) * 0.5;
                    dry_groups.push(format!(
                        "  {model} group {gi:03} liquidType {}  box z [{:.1}..{:.1}]\n      room  {}\n      origin{}",
                        g.liquid_type,
                        g.lo[2],
                        g.hi[2],
                        go_at(&sites[0], [mid(0), mid(1), g.lo[2] + 1.5]),
                        go_at(&sites[0], [0.0, 0.0, 0.0]),
                    ));
                }
            }
            groups.push(g);
        }

        // STOREY: for each pool, the lower groups its WET CELLS actually hang over.
        for (ai, a) in groups.iter().enumerate() {
            let Some(lq) = a.liquid.as_ref() else {
                continue;
            };
            let cells = wet_cells(lq);
            // OVERHANG: the farthest a wet cell reaches past its own group's XY box.
            let out = cells.iter().fold(0.0f32, |m, (xy, _)| {
                m.max(a.lo[0] - xy[0])
                    .max(xy[2] - a.hi[0])
                    .max(a.lo[1] - xy[1])
                    .max(xy[3] - a.hi[1])
            });
            if out > 0.01 {
                overhang.push((out, format!("{model}  pool g{ai:03}")));
            }
            for (bi, b) in groups.iter().enumerate() {
                if ai == bi {
                    continue;
                }
                // The claim only fires where the query would: a point in B under the pool's height
                // and inside a wet cell's XY. Take B's own floor as the standing surface.
                let stand = b.lo[2] + 1.5;
                let mut best: Option<([f32; 4], f32)> = None;
                for (xy, h) in &cells {
                    if *h <= stand {
                        continue; // the pool is at or below this floor — no false submersion
                    }
                    let overlap =
                        xy[0] < b.hi[0] && xy[2] > b.lo[0] && xy[1] < b.hi[1] && xy[3] > b.lo[1];
                    // …and B must not simply BE the pool's own room: skip when B's box contains the
                    // liquid height, i.e. the pool is inside B and the submersion is real.
                    if overlap && *h > b.hi[2] && best.is_none_or(|(_, bh)| *h > bh) {
                        best = Some((*xy, *h));
                    }
                }
                if let Some((xy, h)) = best {
                    let pt = [(xy[0] + xy[2]) * 0.5, (xy[1] + xy[3]) * 0.5, stand];
                    // Would a FLOOR under the pool — "a pool cannot claim anything below its own
                    // group's box" — close this one? The candidate fix, checked per hit rather
                    // than assumed to generalize from the site that motivated it.
                    let floored = stand < a.lo[2];
                    if !floored {
                        floor_misses += 1;
                    }
                    storey.push((
                        h - stand,
                        format!(
                            "  {:>7.1} yd  {}  {model}  pool g{ai:03} over g{bi:03} (floor {:.1}, pool {:.1}, pool-group floor {:.1})\n      {}  [uid {}]",
                            h - stand,
                            if floored { "FLOOR-FIXES" } else { "floor-MISSES" },
                            b.lo[2],
                            h,
                            a.lo[2],
                            go_at(&sites[0], pt),
                            sites[0].unique_id,
                        ),
                    ));
                }
            }
        }
    }

    overhang.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "\n=== OVERHANG: how far a pool's WET CELLS reach outside its OWN group's box, in XY ===\n\
         (the risk in narrowing the scope from placement to GROUP: where a pool hangs over the\n\
          neighbouring group, a subject standing under it is in group B while the water is group A's,\n\
          and a strict group match would stop it swimming. 0 = the pool never leaves its own room.)"
    );
    for (d, line) in overhang.iter().take(15) {
        println!("  {d:>7.2} yd  {line}");
    }

    // ---- the ARCHIVE-WIDE census. -----------------------------------------------------------
    // The placed-model census below answers "what does shipped, reachable content use". This one
    // answers the prior question: is `groupLiquid` a field the 1.12 authors USED at all? If every
    // MLIQ-carrying group in the whole archive says `0xf`, then the type-override branch never
    // fires on real data and a stray non-`0xf` value on a group with no liquid is content noise,
    // not a feature we are failing to implement.
    let mut all_census: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    let mut odd: Vec<String> = Vec::new();
    for name in chain.list()?.into_iter().map(|e| e.name) {
        let lower = name.to_lowercase();
        // group files are `<stem>_NNN.wmo`
        if !lower.ends_with(".wmo") {
            continue;
        }
        let Some(stem) = lower.strip_suffix(".wmo") else {
            continue;
        };
        if stem.len() < 4 || !stem[stem.len() - 3..].bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Some(g) = read_group(&bytes) else {
            continue;
        };
        let mliq = has_mliq(&bytes);
        let e = all_census.entry(g.liquid_type).or_default();
        if mliq {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
        if g.liquid_type != NO_LIQUID {
            odd.push(format!(
                "  {lower}  liquidType {}  {}",
                g.liquid_type,
                if mliq { "HAS MLIQ" } else { "no MLIQ" }
            ));
        }
    }
    println!("\n=== groupLiquid census over the WHOLE ARCHIVE: value -> (with MLIQ, without) ===");
    for (ty, (with, without)) in &all_census {
        println!("  {ty:>5} -> {with:>5} with MLIQ, {without:>5} without");
    }
    println!("\n  every non-0xf group in the archive ({}):", odd.len());
    for line in &odd {
        println!("{line}");
    }

    println!("\n=== groupLiquid census over placed models: value -> (with MLIQ, without) ===");
    for (ty, (with, without)) in &type_census {
        println!("  {ty:>5} -> {with:>5} with MLIQ, {without:>5} without");
    }

    println!(
        "\n=== DRY-GROUP: groupLiquid != 0xf but no MLIQ chunk ({}) ===",
        dry_groups.len()
    );
    for line in &dry_groups {
        println!("{line}");
    }

    storey.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "\n=== STOREY: a walkable floor under another group's WET CELLS ({}) ===",
        storey.len()
    );
    println!("  the FLOOR rule (a pool never claims below its own group box) misses {floor_misses} of {}", storey.len());
    for (_, line) in &storey {
        println!("{line}");
    }
    Ok(())
}
