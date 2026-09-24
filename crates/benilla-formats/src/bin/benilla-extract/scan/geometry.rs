//! Corpus scans over **what geometry a model draws** — the shape of its batches, independent of
//! how they are textured or shaded.
//!
//! Billboard cards and which way they face (`bbscan`, `bbfacescan`), geosets and the untextured /
//! single-triangle strays (`geosetscan`), flat ground-plane quads (`groundscan`), and degenerate
//! authored vertex normals (`normalscan`). What MATERIAL a batch carries is [`super::material`]'s
//! question.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use benilla_formats::Chain;

/// Sweep every `.m2` (under `prefix`, if given) and classify every BILLBOARD batch by which way its
/// geometry faces — see the `Bbfacescan` command doc for why the sign decides visibility.
///
/// A billboard bone puts the model's **+X** toward the viewer (the spherical arm, `0x71547c`),
/// so a batch whose winding normal is +X faces the camera and a −X one faces away. Single-sided
/// (`two_sided` false, i.e. no material `0x04`), the away-facing ones are backface-culled by the
/// reference from every angle — they are authored placeholders the author never saw.
pub fn bbfacescan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut cards) = (0u32, 0u32);
    // The four card populations. `away_single` is the one the renderer's forced-two-sided override
    // changes: those and only those become visible when a card is not allowed to be culled.
    let (mut toward, mut away_single, mut away_two, mut edge_on) = (0u32, 0u32, 0u32, 0u32);
    // …and the batches that are not cards at all. A facing only exists for a batch that IS one
    // plane; a closed solid has faces every way round, backface culling never hides it, and
    // sampling its first triangle answers a question it doesn't have (see `plane_normal`).
    let mut solid = 0u32;
    // …and the other side of the same gate: batches the split REFUSED because their geometry is
    // welded to a billboard bone (`RenderSubmesh::welded_billboard`). These
    // carry no `billboard` at all, so the card loop below never sees them — they are counted from
    // the flag the render lanes read, which is what makes this a cross-check of `bbscan`'s SEAM
    // column (same models, counted from the two ends of the rule) rather than a re-derivation.
    let (mut welded, mut weld_models) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let mut lines = Vec::new();
        let weld = subs.iter().filter(|s| s.welded_billboard).count() as u32;
        welded += weld;
        weld_models += u32::from(weld > 0);
        for (i, s) in subs.iter().enumerate() {
            let Some(bb) = &s.billboard else { continue };
            cards += 1;
            // Not one plane ⇒ not a card. Counted apart rather than classified by a triangle.
            if s.plane_normal().is_none() {
                solid += 1;
                continue;
            }
            // The winding normal's X component: +1 faces the viewer, −1 faces away.
            let Some(fx) = facet_x(s) else {
                edge_on += 1;
                continue;
            };
            if fx > 0.5 {
                toward += 1;
            } else if fx < -0.5 {
                if s.two_sided {
                    away_two += 1;
                } else {
                    away_single += 1;
                    lines.push(format!(
                        "    batch {i:>3}: {:?} {:?} {} verts  facetX {fx:+.2}  tex {}",
                        bb.kind,
                        s.blend,
                        s.positions.len(),
                        s.texture.as_deref().unwrap_or("NONE"),
                    ));
                }
            } else {
                edge_on += 1;
            }
        }
        if !lines.is_empty() {
            println!("{name}");
            for l in lines {
                println!("{l}");
            }
        }
    }
    eprintln!(
        "{scanned} models scanned, {cards} billboard batch(es): {toward} toward, \
         {away_single} away+single-sided (reference culls these), {away_two} away+two-sided, \
         {edge_on} edge-on/degenerate, {solid} NOT PLANAR (3-D geometry — no facing to decide); \
         plus {welded} WELDED batch(es) across {weld_models} model(s) the split refused \
         (skinned, not carded)"
    );
    Ok(())
}

/// The X component of a batch's first-triangle winding normal, in WoW model space — `None` when the
/// triangle is degenerate or the normal lies in the YZ plane (nothing to decide a facing from).
fn facet_x(s: &benilla_formats::RenderSubmesh) -> Option<f32> {
    let tri = s.indices.get(..3)?;
    let p = |i: u32| s.positions.get(i as usize).copied();
    let (a, b, c) = (p(tri[0])?, p(tri[1])?, p(tri[2])?);
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (len > 1e-9).then(|| n[0] / len)
}

/// The widest max/min axis spread a scale track ever holds — `1.0` for a uniform track (the
/// overwhelming majority: a glow card that only pulses in size), `> 1` for one that stretches its
/// bone along an axis. A key with an axis at or below zero contributes `1.0`: a zero axis is the
/// reference's own "hide this" (the eyelid blink's retracted lid), not a stretch, and dividing by
/// it would report infinity.
fn scale_spread<T>(keys: &[(T, [f32; 3])]) -> f32 {
    keys.iter()
        .map(|(_, s)| {
            let (lo, hi) = s
                .iter()
                .fold((f32::MAX, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            if lo > 1e-4 {
                hi / lo
            } else {
                1.0
            }
        })
        .fold(1.0f32, f32::max)
}

/// Sweep every `.m2` (under `prefix`, if given) and classify its billboard usage — see the
/// `Bbscan` command doc. Output per model: the authored arms and how many vertices ride each
/// DIRECTLY (primary bone is the billboard bone — the card path) vs INHERITED (primary bone
/// descends from one — the joint-palette path), then the same question for the
/// model's **particle emitters and ribbons** (`fx[…]`) — the population behind decision 0813: an
/// emitter on (or under) a billboard bone has a camera-dependent origin, because the reference
/// folds the record position through the *replaced* palette matrix (`0x7190a9`–`0x71910c`).
///
/// The `NONUNIF[…]` column is the third population: billboard bones whose SCALE is animated
/// **non-uniformly** (in any sequence band, or on a global-sequence loop), listed as
/// `bone:arm×ratio` where the ratio is the widest max/min axis spread the bone ever holds. The
/// billboard law preserves the bone's scale under the substituted camera basis (`T·R_cam·S`), so
/// such a bone stretches its card along one model axis — the Lightwell's lock-Z shaft
/// (`World\Goober\G_HolyLightWell.m2` bone 0, `×4.26`) is a 0.12 yd card pulled into a 4.5 yd
/// column of light. Any consumer that reduces the scale to one scalar renders these squat and
/// blown-out instead (bug B169), and a `d` in the direct column is where that lands on a **card**,
/// whose transform is rebuilt from the joint rather than skinned from it.
pub fn bbscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let arm = |k: benilla_formats::BillboardKind| match k {
        benilla_formats::BillboardKind::Spherical => "S",
        benilla_formats::BillboardKind::LockX => "X",
        benilla_formats::BillboardKind::LockY => "Y",
        benilla_formats::BillboardKind::LockZ => "Z",
    };
    let (mut scanned, mut hits) = (0u32, 0u32);
    // Corpus totals: models exercising each arm, split by how the geometry rides it.
    let mut direct_models: HashMap<&'static str, u32> = HashMap::new();
    let mut inherited_models: HashMap<&'static str, u32> = HashMap::new();
    // …and the effect riders (particles/ribbons on a billboard chain).
    let mut fx_models = 0u32;
    let mut fx_total: HashMap<String, u32> = HashMap::new();
    // …and the seam population (see the classification below).
    let (mut seam_models, mut seam_bones) = (0u32, 0u32);
    // …and the non-uniform-scale population: billboard bones that stretch their card along one
    // model axis, split by whether geometry rides them DIRECTLY (the card path, where a scalar
    // scale loses the stretch outright) or only by inheritance (the palette path).
    let (mut nonunif_models, mut nonunif_bones, mut nonunif_direct) = (0u32, 0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let Ok(fmt) = benilla_m2::parse_m2(&mut std::io::Cursor::new(bytes.as_slice())) else {
            continue;
        };
        let m = fmt.model();
        let kinds: Vec<Option<benilla_formats::BillboardKind>> = m
            .bones
            .iter()
            .map(|b| benilla_formats::BillboardKind::from_bone_flags(b.flags.bits()))
            .collect();
        if kinds.iter().all(|k| k.is_none()) {
            continue;
        }
        hits += 1;
        // Nearest billboard ancestor (self included) per bone — the arm whose palette
        // replacement a vertex on this bone inherits. Bounded walk (M2 parents precede
        // children; the bound is just a malformed-file guard).
        let ancestor_arm = |mut i: usize| -> Option<(bool, benilla_formats::BillboardKind)> {
            let mut hops = 0;
            loop {
                if let Some(k) = kinds.get(i).copied().flatten() {
                    return Some((hops == 0, k));
                }
                let p = usize::try_from(*m.bones.get(i).map(|b| &b.parent)?).ok()?;
                hops += 1;
                if p >= m.bones.len() || hops > m.bones.len() {
                    return None;
                }
                i = p;
            }
        };
        let (mut direct, mut inherited): (HashMap<&str, u32>, HashMap<&str, u32>) =
            (HashMap::new(), HashMap::new());
        for v in &m.vertices {
            match ancestor_arm(v.bone_indices[0] as usize) {
                Some((true, k)) => *direct.entry(arm(k)).or_default() += 1,
                Some((false, k)) => *inherited.entry(arm(k)).or_default() += 1,
                None => {}
            }
        }
        // SEAM bones: billboard bones whose geometry is welded to the rest of the model — by a
        // partial vertex weight, or by a triangle it shares with a static neighbour. The reference
        // skins per vertex, so such geometry BENDS (a flap's root stays on the body while its tip
        // swings to the camera); a rigid card cannot express that at all, and moving the group
        // rigidly tears the flap in two. These are the bones the split declines,
        // read through the renderer's own predicate so this census cannot drift from it.
        let seam = benilla_formats::non_separable_billboard_bones(&bytes);
        let fmt_counts = |m: &HashMap<&str, u32>| {
            let mut v: Vec<String> = m.iter().map(|(k, n)| format!("{k}:{n}")).collect();
            v.sort();
            v.join(" ")
        };
        // The EFFECT riders: a particle emitter / ribbon whose bone chain reaches a billboard
        // bone has a camera-dependent live frame (its record position rides the replaced palette
        // matrix), so a consumer that places it at the rest pose puts it in the wrong place.
        // `d` = the effect's own bone is the billboard bone, `i` = it descends from one.
        let mut fx: HashMap<String, u32> = HashMap::new();
        let mut tally = |tag: &str, bone: u16| {
            if let Some((direct, k)) = ancestor_arm(bone as usize) {
                let key = format!("{tag}{}{}", if direct { "d" } else { "i" }, arm(k));
                *fx.entry(key).or_default() += 1;
            }
        };
        for e in benilla_formats::parse_m2_particle_emitters(&bytes)
            .unwrap_or_default()
            .iter()
        {
            tally("p", e.bone);
        }
        for r in benilla_formats::parse_m2_ribbon_emitters(&bytes)
            .unwrap_or_default()
            .iter()
        {
            tally("r", r.bone);
        }
        let fx_counts = {
            let mut v: Vec<String> = fx.iter().map(|(k, n)| format!("{k}:{n}")).collect();
            v.sort();
            v.join(" ")
        };
        let bones: String = kinds.iter().flatten().map(|&k| arm(k)).collect();
        // NON-UNIFORM billboard scale (see the `NONUNIF` column in the doc above).
        let seqs = benilla_formats::parse_m2_animations(&bytes);
        let gseq = benilla_formats::parse_m2_global_sequence_bones(&bytes);
        let mut nonunif: BTreeMap<usize, f32> = BTreeMap::new();
        for bk in seqs.iter().flat_map(|a| &a.bones) {
            let r = scale_spread(&bk.scale);
            if r > 1.01 {
                let e = nonunif.entry(bk.bone as usize).or_insert(1.0);
                *e = e.max(r);
            }
        }
        for gb in &gseq {
            let Some(ch) = &gb.scale else { continue };
            let r = scale_spread(&ch.keys);
            if r > 1.01 {
                let e = nonunif.entry(gb.bone as usize).or_insert(1.0);
                *e = e.max(r);
            }
        }
        // Only the BILLBOARD bones matter — an ordinary bone's non-uniform scale rides the joint
        // palette like any other transform and nothing reduces it.
        nonunif.retain(|&b, _| kinds.get(b).copied().flatten().is_some());
        let nonunif_col = if nonunif.is_empty() {
            String::new()
        } else {
            nonunif_models += 1;
            nonunif_bones += nonunif.len() as u32;
            // Does geometry ride this bone DIRECTLY (primary bone == it, and separable)? That is
            // the card path — where the stretch has to survive a transform rebuild.
            let direct_bone = |b: usize| {
                !seam.contains(&(b as u16))
                    && m.vertices.iter().any(|v| v.bone_indices[0] as usize == b)
            };
            nonunif_direct += nonunif.keys().filter(|&&b| direct_bone(b)).count() as u32;
            format!(
                "  NONUNIF[{}]",
                nonunif
                    .iter()
                    .map(|(&b, r)| format!(
                        "{b}:{}{}\u{00d7}{r:.2}",
                        kinds.get(b).copied().flatten().map_or("?", arm),
                        if direct_bone(b) { "d" } else { "i" }
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let seam_col = if seam.is_empty() {
            String::new()
        } else {
            seam_models += 1;
            seam_bones += seam.len() as u32;
            format!(
                "  SEAM[{}]",
                seam.iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        println!(
            "{bones:>4}  direct[{}]  inherited[{}]  fx[{fx_counts}]{seam_col}{nonunif_col}  {name}",
            fmt_counts(&direct),
            fmt_counts(&inherited)
        );
        if !fx.is_empty() {
            fx_models += 1;
            for (k, n) in &fx {
                *fx_total.entry(k.clone()).or_default() += n;
            }
        }
        for k in direct.keys() {
            *direct_models
                .entry(match *k {
                    "S" => "S",
                    "X" => "X",
                    "Y" => "Y",
                    _ => "Z",
                })
                .or_default() += 1;
        }
        for k in inherited.keys() {
            *inherited_models
                .entry(match *k {
                    "S" => "S",
                    "X" => "X",
                    "Y" => "Y",
                    _ => "Z",
                })
                .or_default() += 1;
        }
    }
    let tot = |m: &HashMap<&'static str, u32>| {
        let mut v: Vec<String> = m.iter().map(|(k, n)| format!("{k}:{n}")).collect();
        v.sort();
        v.join(" ")
    };
    let fx_tot = {
        let mut v: Vec<String> = fx_total.iter().map(|(k, n)| format!("{k}:{n}")).collect();
        v.sort();
        v.join(" ")
    };
    eprintln!(
        "{scanned} models scanned, {hits} with billboard bones; models by arm — direct(card) [{}]  inherited(palette) [{}]; {fx_models} with EFFECTS on a billboard chain [{fx_tot}]; {seam_models} with SEAM billboard bones ({seam_bones} total) — geometry welded to the model, which a rigid card tears; {nonunif_models} with NON-UNIFORM billboard scale ({nonunif_bones} bones, {nonunif_direct} of them ridden DIRECTLY = a card whose stretch a scalar scale would lose)",
        tot(&direct_models),
        tot(&inherited_models)
    );
    Ok(())
}

/// Sweep every `.m2` (under `prefix`, if given) and census the **geometry a non-character model
/// draws that the reference may not** — the population instrument behind the bug channel's
/// "stray untextured primitive" family. Three independent signals per model, all read through the
/// renderer's own batch resolution (`m2batch`'s), so the report can't drift from the mechanism:
///
/// - **MULTI-GEOSET** — more than one distinct `skinSectionId`. The character compositor selects
///   among these; every other spawn path draws **all** of them, so this is exactly the population
///   an unfiltered creature/doodad/effect draw over-renders (`Creature\Banshee\Banshee.m2` is the
///   pinned case: `0`×17 + `402`×9).
/// - **UNTEX** — batches with no texture *and* no runtime slot that fills one: neither a character
///   composite slot ([`benilla_formats::RenderSubmesh::char_slot`]) nor a creature skin variation
///   ([`benilla_formats::RenderSubmesh::skin_slot`], filled at spawn from `CreatureDisplayInfo`).
///   Both fills are ordinary, so counting them would drown the signal — 324 of 420 `Creature\`
///   models carry a skin slot. What is left is geometry nothing can texture.
/// - **TINY** — batches of at most 2 faces: the literal single-triangle/quad primitives.
///
/// A model is listed when it trips any of the three. `m2batch` then explains one model in full.
pub fn geosetscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut hits) = (0u32, 0u32);
    let (mut multi_models, mut untex_models, mut tiny_models) = (0u32, 0u32, 0u32);
    // Top-level directory → multi-geoset model count, so the report says *where* the population
    // lives (Creature/, Spells/, World/…) rather than only how big it is.
    let mut by_dir: BTreeMap<String, u32> = BTreeMap::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let mut geosets: BTreeMap<u16, u32> = BTreeMap::new();
        let (mut untex, mut tiny) = (0u32, 0u32);
        for s in &subs {
            *geosets.entry(s.geoset_id).or_default() += 1;
            if s.texture.is_none() && s.char_slot.is_none() && s.skin_slot.is_none() {
                untex += 1;
            }
            if !s.indices.is_empty() && s.indices.len() <= 6 {
                tiny += 1;
            }
        }
        let multi = geosets.len() > 1;
        if !multi && untex == 0 && tiny == 0 {
            continue;
        }
        hits += 1;
        if multi {
            multi_models += 1;
            let top = name.split_once('\\').map(|(d, _)| d).unwrap_or("<root>");
            *by_dir.entry(top.to_ascii_lowercase()).or_default() += 1;
        }
        if untex > 0 {
            untex_models += 1;
        }
        if tiny > 0 {
            tiny_models += 1;
        }
        let hist = geosets
            .iter()
            .map(|(id, n)| format!("{id}×{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut marks = Vec::new();
        if multi {
            marks.push(format!("MULTI-GEOSET({})", geosets.len()));
        }
        if untex > 0 {
            marks.push(format!("UNTEX({untex})"));
        }
        if tiny > 0 {
            marks.push(format!("TINY({tiny})"));
        }
        println!(
            "{:>3} batches  [{hist}]  {}  {name}",
            subs.len(),
            marks.join(" ")
        );
    }
    let dirs = by_dir
        .iter()
        .map(|(d, n)| format!("{d}={n}"))
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "{scanned} models scanned, {hits} listed — {multi_models} MULTI-GEOSET, \
         {untex_models} with UNTEX batches, {tiny_models} with TINY batches; \
         multi-geoset by top dir: [{dirs}]"
    );
    Ok(())
}

/// Sweep every `.m2` (under `prefix`, if given) and report models that author flat ground-plane
/// render geometry — the population instrument for the class of spell effects that lie in the
/// model-space XY plane at z≈0 (WoW axes, Z up) and get buried by sloped terrain (Battle Shout's
/// crescents are the canonical case: 6 batches, each a 4-vert quad, every vertex exactly z=0, each
/// quad skinned 100% to a single bone). Per batch (same batch/geoset resolution `m2batch` uses):
/// FLAT if every vertex has `|z| <= 0.01` in model space; flat batches sub-classify QUAD-1BONE
/// (the crescent shape — [`benilla_formats::RenderSubmesh::ground_quad`], the ground-fx decal
/// lane's own detector) vs OTHER-FLAT (flat but not that shape, staying on the ordinary render
/// path) — which decides how general the renderer mechanism has to be.
pub fn groundscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut hits, mut all_flat, mut mixed) = (0u32, 0u32, 0u32, 0u32);
    let (mut quad_total, mut other_total) = (0u32, 0u32);
    // The hover census: every ground-quad-SHAPED batch whose uniform plane sits ABOVE z = 0
    // (any height — the shape test is the renderer's own, the ceiling is lifted), the population
    // that decides where the decal lane's hover ceiling (`GROUND_HOVER_MAX`) can sit.
    let mut hovers: Vec<(f32, String)> = Vec::new();
    // The TINT census: ground quads whose whole colour is a CONSTANT M2Color
    // (`GroundQuad::tint`) — the population a decal consumer draws white unless it carries the
    // vertex-colour bake across (the Flare's two washes on the neutral `GENERICGLOW*` radials).
    let (mut tinted_total, mut tinted_models) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let total_batches = subs.len();
        let (mut flat_count, mut quad_count, mut other_count) = (0u32, 0u32, 0u32);
        let mut blend_modes: Vec<String> = Vec::new();
        let mut model_hovers: Vec<f32> = Vec::new();
        let mut tints: Vec<[f32; 3]> = Vec::new();
        for s in &subs {
            if s.positions.is_empty() {
                continue;
            }
            if let Some((_, hover)) = s.ground_quad_hover(f32::INFINITY) {
                if hover > 0.01 {
                    hovers.push((hover, name.clone()));
                    model_hovers.push(hover);
                }
            }
            if !s.positions.iter().all(|v| v[2].abs() <= 0.01) {
                continue;
            }
            flat_count += 1;
            let bm = format!("{:?}", s.blend);
            if !blend_modes.contains(&bm) {
                blend_modes.push(bm);
            }
            // The RENDERER's own detector, so this report is exactly what the ground-fx
            // decal lane will do with each batch — the instrument can't drift from the
            // mechanism it measures.
            if let Some(q) = s.ground_quad() {
                quad_count += 1;
                if q.tint != [1.0; 3] && !tints.contains(&q.tint) {
                    tints.push(q.tint);
                }
            } else {
                other_count += 1;
            }
        }
        if flat_count == 0 && model_hovers.is_empty() {
            continue;
        }
        hits += 1;
        quad_total += quad_count;
        other_total += other_count;
        if flat_count as usize == total_batches {
            all_flat += 1;
        } else {
            mixed += 1;
        }
        tinted_total += tints.len() as u32;
        tinted_models += u32::from(!tints.is_empty());
        blend_modes.sort();
        let hover_note = if model_hovers.is_empty() {
            String::new()
        } else {
            format!(
                "  hover[{}]",
                model_hovers
                    .iter()
                    .map(|h| format!("{h:.2}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        let tint_note = if tints.is_empty() {
            String::new()
        } else {
            format!(
                "  TINT[{}]",
                tints
                    .iter()
                    .map(|t| format!("{:.2},{:.2},{:.2}", t[0], t[1], t[2]))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        println!(
            "{total_batches:>3} batches  {flat_count:>3} flat ({quad_count:>2} quad-1bone, {other_count:>2} other-flat){hover_note}{tint_note}  blend[{}]  {name}",
            blend_modes.join(" ")
        );
    }
    eprintln!(
        "{scanned} models scanned, {hits} with flat batches ({all_flat} all-flat, {mixed} mixed); flat batches: {quad_total} QUAD-1BONE, {other_total} OTHER-FLAT"
    );
    eprintln!(
        "static M2Color TINT on ground quads: {tinted_models} models carry {tinted_total} distinct non-white constants — the colour a decal lane loses unless it carries the vertex bake"
    );
    if !hovers.is_empty() {
        hovers.sort_by(|a, b| a.0.total_cmp(&b.0));
        eprintln!(
            "{} hovering quad-shaped batches (uniform plane z > 0.01), sorted:",
            hovers.len()
        );
        for (h, name) in &hovers {
            eprintln!("  z {h:>7.3}  {name}");
        }
    }
    Ok(())
}

/// Sweep every `.m2` (under `prefix`, if given) and measure how far its authored header bounding
/// box — the model's **all-animation** vertex extent, and the box the reference derives its doodad
/// cull sphere from — reaches past its **bind-pose** vertex extent.
///
/// The population instrument for. A placed model's submesh entity keeps its transform
/// at the placement origin while the joint palette moves its vertices, so a bind-pose mesh bound
/// stops describing what is drawn the moment the model animates: cull with it and the object blinks
/// out while its geometry is still on screen. `SLACK` is how many yards the authored box reaches
/// past the bind-pose box on its worst face — for the ambient critters (birds, bats, butterflies,
/// wasps) that is tens of yards against a sub-yard body, which is the whole bug.
///
/// `SHORT` is the same measure with the signs reversed: yards of bind-pose geometry sticking out of
/// the *authored* box. It is not zero in the shipped corpus, which is why the fix unions the two
/// boxes instead of swapping one for the other — the reference never trips over it because it tests
/// the box's circumsphere, which swallows the overhang.
///
/// `ANIM` marks the models the widened bound actually applies to: any bone track with more than one
/// key, or any global-sequence bone channel. A static model keeps its tighter per-batch bound.
pub fn animboundscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    // (slack, short, animates, bind-pose half-diagonal, name)
    let mut rows: Vec<(f32, f32, bool, f32, String)> = Vec::new();
    let mut scanned = 0u32;
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Ok(bounds) = benilla_formats::parse_m2_bounds(&bytes) else {
            continue;
        };
        let Ok(fmt) = benilla_m2::parse_m2(&mut std::io::Cursor::new(bytes.as_slice())) else {
            continue;
        };
        let verts = &fmt.model().vertices;
        if verts.is_empty() {
            continue;
        }
        scanned += 1;
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in verts {
            for (a, c) in [v.position.x, v.position.y, v.position.z]
                .into_iter()
                .enumerate()
            {
                lo[a] = lo[a].min(c);
                hi[a] = hi[a].max(c);
            }
        }
        let (mut slack, mut short) = (0.0f32, 0.0f32);
        for a in 0..3 {
            slack = slack
                .max(lo[a] - bounds.bbox_min[a])
                .max(bounds.bbox_max[a] - hi[a]);
            short = short
                .max(bounds.bbox_min[a] - lo[a])
                .max(hi[a] - bounds.bbox_max[a]);
        }
        let half_diag =
            ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt()
                * 0.5;
        // "Animates" the way `doodad_anim::classify` means it: geometry that MOVES relative to the
        // placement transform — a keyed bone track, or a free-running global-sequence channel.
        let keyed = benilla_formats::parse_m2_animations(&bytes)
            .iter()
            .any(|a| {
                a.bones
                    .iter()
                    .any(|b| b.translation.len() > 1 || b.rotation.len() > 1 || b.scale.len() > 1)
            });
        let gseq = !benilla_formats::parse_m2_global_sequence_bones(&bytes).is_empty();
        rows.push((slack, short, keyed || gseq, half_diag, name));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let over = |t: f32| rows.iter().filter(|r| r.0 > t && r.2).count();
    println!("{scanned} models with geometry");
    println!(
        "  animated:                        {}",
        rows.iter().filter(|r| r.2).count()
    );
    for t in [1.0f32, 5.0, 10.0, 20.0] {
        println!("  animated with SLACK > {t:>5.1} yd:  {}", over(t));
    }
    println!(
        "  models whose authored box does NOT contain their bind pose: {}",
        rows.iter().filter(|r| r.1 > 0.01).count()
    );
    println!();
    println!(
        "{:>9}  {:>8}  {:>9}  {:>4}  MODEL",
        "SLACK", "SHORT", "BIND-R", "ANIM"
    );
    // The class the bug is about: the authored box dwarfs the body, so the animation carries the
    // model clean out of any bind-pose bound. Ranked by that ratio, not by raw slack — a 200 yd
    // waterfall with 200 yd of slack is never culled either way.
    let mut ranked: Vec<&(f32, f32, bool, f32, String)> =
        rows.iter().filter(|r| r.2 && r.0 > 1.0).collect();
    ranked.sort_by(|a, b| (b.0 / b.3.max(0.05)).total_cmp(&(a.0 / a.3.max(0.05))));
    for (slack, short, _, half_diag, name) in ranked.iter().take(40) {
        println!(
            "{slack:9.2}  {short:8.2}  {half_diag:9.2}  {:>4}  {name}",
            "yes"
        );
    }
    Ok(())
}

/// `normalscan` — census the batches carrying **degenerate authored vertex normals** (`(0,0,0)`).
///
/// The shipped corpus authors them, and the reference draws those surfaces lit: its `Model2.bls`
/// vertex program consumes the normal as the zero vector, so the order-2 SH quadratic form
/// collapses to its DC term. A renderer that
/// `normalize()`s the same datum gets NaN, `clamp(NaN, 0, 1)` floors the lighting factor to 0, and
/// the batch renders **pure black over its correct texture** — bug B134's Qiraji Brainwasher
/// sleeves and Ironaya skirt, and the reason the shader's normalize is guarded.
///
/// This is the population instrument for that class: how many models are on it, how much of each
/// batch is degenerate, and which models are worst hit. `ALL` marks a batch with no usable normal
/// at all — the one that renders as a solid black shape rather than a shaded gradient.
pub fn normalscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    // (degenerate verts, total verts, batches touched, all-degenerate batches, model)
    let mut rows: Vec<(usize, usize, usize, usize, String)> = Vec::new();
    let mut scanned = 0u32;
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let (mut bad, mut total, mut touched, mut all_bad) = (0usize, 0usize, 0usize, 0usize);
        for s in &subs {
            let n = s
                .normals
                .iter()
                .filter(|v| v[0] * v[0] + v[1] * v[1] + v[2] * v[2] <= 1e-8)
                .count();
            total += s.normals.len();
            if n > 0 {
                bad += n;
                touched += 1;
                all_bad += usize::from(n == s.normals.len());
            }
        }
        if bad > 0 {
            rows.push((bad, total, touched, all_bad, name));
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let batches: usize = rows.iter().map(|r| r.2).sum();
    let all_batches: usize = rows.iter().map(|r| r.3).sum();
    let verts: usize = rows.iter().map(|r| r.0).sum();
    println!("normalscan — degenerate (0,0,0) authored vertex normals");
    println!("  models scanned:                  {scanned}");
    println!("  models with any:                 {}", rows.len());
    println!("  batches touched:                 {batches}");
    println!("  batches with NO usable normal:   {all_batches}");
    println!("  degenerate vertices:             {verts}");
    println!();
    println!(
        "{:>7}  {:>7}  {:>7}  {:>4}  MODEL",
        "BAD", "OF", "BATCHES", "ALL"
    );
    for (bad, total, touched, all_bad, name) in rows.iter().take(60) {
        println!("{bad:7}  {total:7}  {touched:7}  {all_bad:4}  {name}");
    }
    Ok(())
}
