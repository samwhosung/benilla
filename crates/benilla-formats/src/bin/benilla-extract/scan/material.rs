//! Corpus scans over how a batch is textured and blended, and what the effect and entity corpora's
//! animated texture transforms and tints render for a consumer that runs none of them.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use benilla_formats::{Chain, KeyAnim, SeqLoops};

/// List the models whose material table authors blend mode 5 (Mod) or 6 (Mod2x): materials at
/// `0x84`/`0x88`, 4-byte `{u16 flags, u16 blend}` records.
pub fn blendscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut hits) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let at = |o: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?))
        };
        let (Some(n), Some(ofs)) = (at(0x84), at(0x88)) else {
            continue;
        };
        let mats: Vec<(u16, u16)> = (0..n as usize)
            .filter_map(|i| {
                let o = ofs as usize + i * 4;
                let b = bytes.get(o..o + 4)?;
                Some((
                    u16::from_le_bytes([b[0], b[1]]),
                    u16::from_le_bytes([b[2], b[3]]),
                ))
            })
            .collect();
        if mats.iter().any(|&(_, blend)| blend == 5 || blend == 6) {
            hits += 1;
            println!("{mats:?}  {name}");
        }
    }
    eprintln!("{scanned} models scanned, {hits} with Mod/Mod2x materials");
    Ok(())
}

/// List the models whose batch visibility changes between sequences through the alpha combine
/// (`A = colourAlpha × weight`, `A ≤ 0` culls; `0x707680`), which a single-sequence bake misses.
pub fn alphascan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut hits) = (0u32, 0u32);
    let mut by_dir: BTreeMap<String, u32> = BTreeMap::new();
    let mut rows: Vec<(String, usize, usize, usize)> = Vec::new();
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let seq_count = benilla_formats::parse_m2_animations(&bytes).len();
        if seq_count < 2 {
            continue; // a one-sequence model can't disagree with itself
        }
        // Hidden in slot s: the combined factor is 0 at every sample; any flicker above 0 draws.
        let hidden_in = |sub: &benilla_formats::RenderSubmesh, slot: usize| -> bool {
            sub.alpha_anim.as_ref().is_some_and(|a| {
                (0..=16u16).all(|k| a.sample(Some(slot), f32::from(k) * 0.25, 0.0) <= 0.0)
            })
        };
        let (mut first, mut any, mut varies) = (0usize, 0usize, 0usize);
        for sub in &subs {
            let h0 = hidden_in(sub, 0);
            let mut hid_any = h0;
            let mut differs = false;
            for slot in 1..seq_count {
                let h = hidden_in(sub, slot);
                hid_any |= h;
                differs |= h != h0;
            }
            if h0 {
                first += 1;
            }
            if hid_any {
                any += 1;
            }
            if differs {
                varies += 1;
            }
        }
        if varies == 0 {
            continue;
        }
        hits += 1;
        let top = name.split_once('\\').map(|(d, _)| d).unwrap_or("<root>");
        *by_dir.entry(top.to_ascii_lowercase()).or_default() += 1;
        rows.push((name, varies, first, any));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!(
        "model                                                        varies  hid@seq0  hid@any"
    );
    for (name, varies, first, any) in rows.iter().take(60) {
        println!("{name:<60}  {varies:>6}  {first:>8}  {any:>7}");
    }
    if rows.len() > 60 {
        println!("… and {} more", rows.len() - 60);
    }
    println!("\n{hits} of {scanned} models author per-sequence batch visibility");
    println!("by top-level directory:");
    for (dir, n) in &by_dir {
        println!("  {dir:<16} {n:>5}");
    }
    Ok(())
}

/// List the clamp-authored batches (`M2Texture.flags` bit 0 or 1 clear) whose UVs leave `0..1`:
/// clamped, the margin samples the transparent border, where repeat draws the opposite edge with a
/// seam. `over` is how far past the edge the batch reaches, in UV units.
pub fn uvwrapscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut batches, mut hits, mut models) = (0u32, 0u32, 0u32, 0u32);
    let mut cutout_hits = 0u32;
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
        for (i, s) in subs.iter().enumerate() {
            if s.uvs.is_empty() {
                continue;
            }
            batches += 1;
            let ext = |axis: usize| {
                s.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
                    (lo.min(t[axis]), hi.max(t[axis]))
                })
            };
            let (u, v) = (ext(0), ext(1));
            // Only a clamp axis draws wrong under repeat; under 1/512 of a sheet is float slop.
            const SLOP: f32 = 1.0 / 512.0;
            let bad_u = !s.wrap_x && (u.0 < -SLOP || u.1 > 1.0 + SLOP);
            let bad_v = !s.wrap_y && (v.0 < -SLOP || v.1 > 1.0 + SLOP);
            if !bad_u && !bad_v {
                continue;
            }
            hits += 1;
            let cutout = matches!(
                s.blend,
                benilla_formats::ModelBlend::AlphaTest | benilla_formats::ModelBlend::Blend
            );
            if cutout {
                cutout_hits += 1;
            }
            let over = [
                (-u.0).max(0.0),
                (u.1 - 1.0).max(0.0),
                (-v.0).max(0.0),
                (v.1 - 1.0).max(0.0),
            ]
            .into_iter()
            .fold(0.0f32, f32::max);
            lines.push(format!(
                "    batch {i:>3}: {:?} {} verts  u[{:+.3}..{:+.3}] v[{:+.3}..{:+.3}]  \
                 over {over:.3}  {}{}  tex {}",
                s.blend,
                s.positions.len(),
                u.0,
                u.1,
                v.0,
                v.1,
                if bad_u { "U" } else { "-" },
                if bad_v { "V" } else { "-" },
                s.texture.as_deref().unwrap_or("NONE"),
            ));
        }
        if !lines.is_empty() {
            models += 1;
            println!("{name}");
            for l in lines {
                println!("{l}");
            }
        }
    }
    eprintln!(
        "{scanned} models scanned, {batches} textured batch(es): {hits} CLAMP-AUTHORED BATCHES \
         SAMPLING OUTSIDE 0..1 across {models} model(s) — {cutout_hits} of them cutout/blend, \
         where wrapping changes the silhouette rather than just the colour"
    );
    Ok(())
}

/// List the texture paths the corpus asks for in more than one address mode: the mode rides the
/// loaded `Image`, keyed by path, so such a path needs one upload per mode.
pub fn texmodescan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    // texture path -> set of (wrap_x, wrap_y) asked for, as a 4-bit mask
    let mut modes: std::collections::BTreeMap<String, u8> = std::collections::BTreeMap::new();
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
        for s in &subs {
            let Some(tex) = s.texture.as_deref() else {
                continue;
            };
            let bit = 1u8 << ((s.wrap_x as u8) | ((s.wrap_y as u8) << 1));
            *modes.entry(tex.to_ascii_lowercase()).or_default() |= bit;
        }
    }
    let mut conflicted = 0u32;
    for (path, mask) in &modes {
        if mask.count_ones() > 1 {
            conflicted += 1;
            let want = |b: u8, s: &'static str| if mask & (1 << b) != 0 { s } else { "" };
            println!(
                "CONFLICT {path}  asked as: {}{}{}{}",
                want(0, "[clamp,clamp] "),
                want(1, "[repeat,clamp] "),
                want(2, "[clamp,repeat] "),
                want(3, "[repeat,repeat] "),
            );
        }
    }
    eprintln!(
        "{scanned} models scanned, {} distinct texture path(s): {conflicted} asked for MORE THAN \
         ONE address mode (each needs its own upload, or one of its users renders wrong)",
        modes.len()
    );
    Ok(())
}

/// List the batches whose texture coordinates are generated, not authored: the sphere-map
/// environment stages (`texture_unit_lookup[texCoordSet] > 2`, gated at `0x70b8bd`). A
/// `DEGENERATE` batch's authored UVs collapse to one point, so reading them draws one flat texel
/// (`GnomeSubwayGlass.m2`: `AKGNOMEREFLECT.BLP` texel 0,0, doubled by Mod2x).
pub fn envmapscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut batches, mut hits, mut models, mut degenerate) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    // The blend modes and sheets env stages use: Mod2x tints what is behind (glass), Add a sheen.
    let mut by_blend: BTreeMap<String, u32> = BTreeMap::new();
    let mut by_sheet: BTreeMap<String, u32> = BTreeMap::new();
    // `stage_is_env_mapped` reads an out-of-range `texture_unit_lookup` index as env, as the
    // reference's unguarded read does; those hits, and models with no table at all, count apart.
    let (mut from_oob, mut empty_table) = (0u32, 0u32);
    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        scanned += 1;
        if let Ok(fmt) = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes[..])) {
            let model = fmt.model();
            if model.texture_unit_lookup.is_empty() {
                empty_table += 1;
            }
            if let Ok(skin) = model.parse_embedded_skin(&bytes, 0) {
                for b in skin.batches() {
                    let idx = b.texture_coord_combo_index as usize;
                    if model.texture_unit_lookup.get(idx).is_none() {
                        from_oob += 1;
                    }
                }
            }
        }
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        let mut lines = Vec::new();
        for (i, s) in subs.iter().enumerate() {
            batches += 1;
            if !s.env_map {
                continue;
            }
            hits += 1;
            *by_blend.entry(format!("{:?}", s.blend)).or_default() += 1;
            let sheet = s.texture.as_deref().unwrap_or("NONE").to_string();
            *by_sheet.entry(sheet.clone()).or_default() += 1;
            let span = |axis: usize| {
                s.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
                    (lo.min(t[axis]), hi.max(t[axis]))
                })
            };
            let (u, v) = (span(0), span(1));
            let flat = s.uvs.is_empty() || (u.1 - u.0 <= 1e-6 && v.1 - v.0 <= 1e-6);
            if flat {
                degenerate += 1;
            }
            lines.push(format!(
                "    batch {i:>3}: {:?}{}{} {} verts  authored uv u[{:+.3}..{:+.3}] \
                 v[{:+.3}..{:+.3}]  {}  tex {sheet}",
                s.blend,
                if s.additive { " additive" } else { "" },
                if s.emissive { " unlit" } else { "" },
                s.positions.len(),
                u.0,
                u.1,
                v.0,
                v.1,
                if flat { "DEGENERATE" } else { "unused" },
            ));
        }
        if !lines.is_empty() {
            models += 1;
            println!("{name}");
            for l in lines {
                println!("{l}");
            }
        }
    }
    eprintln!("blend modes: {by_blend:?}");
    let mut sheets: Vec<_> = by_sheet.into_iter().collect();
    sheets.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    eprintln!("top reflection sheets:");
    for (sheet, n) in sheets.iter().take(10) {
        eprintln!("   {n:>5} batch(es)  {sheet}");
    }
    eprintln!(
        "gate provenance: {from_eal} of {hits} hit(s) came from a REAL `>= 3` table entry, \
         {from_oob} from the out-of-range fallback ({empty_table} model(s) carry no \
         texture_unit_lookup at all)",
        from_eal = hits.saturating_sub(from_oob),
    );
    eprintln!(
        "{scanned} models scanned, {batches} batch(es): {hits} ENV-MAPPED (generated texcoords) \
         across {models} model(s) — {degenerate} of them DEGENERATE (authored UVs collapse to a \
         point, so a renderer reading the vertex UV draws one flat texel), {} carrying leftover \
         UVs that merely go unused. {} distinct reflection sheet(s).",
        hits - degenerate,
        sheets.len(),
    );
    Ok(())
}

/// The two per-batch channels the bake resolves per file sequence slot (`uv_seq`, `rgb_seq`),
/// both `C3Vector` tracks, so one classifier serves both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// The texture transform's translation (`tex_anim::bake_uv_seqs`): `uv_seq`, or `uv_anim`.
    Uv,
    /// The M2Color RGB tint (`mat_anim::bake_rgb_seqs`): `rgb_seq`, or `rgb_anim`.
    Rgb,
}

impl Channel {
    fn label(self) -> &'static str {
        match self {
            Self::Uv => "UV ",
            Self::Rgb => "RGB",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Uv => "UV (texture-transform translation)",
            Self::Rgb => "RGB (M2Color tint)",
        }
    }

    fn field(self) -> &'static str {
        match self {
            Self::Uv => "uv_seq",
            Self::Rgb => "rgb_seq",
        }
    }

    fn axes(self) -> &'static [&'static str] {
        match self {
            Self::Uv => &["x", "y"],
            Self::Rgb => &["r", "g", "b"],
        }
    }
}

/// How far apart two baked loops may be and still be one authored loop: the noise of a `u32` ms
/// key rebased through an `f32` divide.
const KEY_EPS: f32 = 1e-4;

/// Why a batch's slots disagree, for every set [`SeqLoops::uniform`] refused. `uniform()` is exact
/// float equality, so `WrapOnly` and `KeysEpsilon` are sets a coarser test would share; `Dead0`
/// and `RealDiffers` need a material per placement.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    /// Slot 0 bakes nothing while a later slot bakes a loop.
    Dead0,
    /// The slots differ only in [`KeyAnim::wrap`]: one band clamps at its tail where another wraps.
    WrapOnly,
    /// The slots agree within [`KEY_EPS`] on every number and flag: float noise only.
    KeysEpsilon,
    /// Different loops (key count, clock law or values), or slot 0 alive beside a dead later slot.
    RealDiffers,
}

impl Why {
    const ALL: [Self; 4] = [
        Self::Dead0,
        Self::WrapOnly,
        Self::KeysEpsilon,
        Self::RealDiffers,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Dead0 => "DEAD-0",
            Self::WrapOnly => "WRAP-ONLY",
            Self::KeysEpsilon => "KEYS-EPSILON",
            Self::RealDiffers => "REAL-DIFFERS",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Dead0 => {
                "slot 0 bakes nothing while a later slot animates — frozen for ever on the \
                 shared lane, and the population the per-placement lane was built for"
            }
            Self::WrapOnly => {
                "the slots bake the same loop and disagree only on the WRAP flag — one band \
                 clamps at its tail where another wraps"
            }
            Self::KeysEpsilon => {
                "the slots agree on every number to within 1e-4 — sub-epsilon noise in the \
                 rebased keys, split by exact float equality"
            }
            Self::RealDiffers => {
                "genuinely different loops (or slot 0 alive against a dead later slot): no single \
                 shared loop can serve every sequence"
            }
        }
    }
}

/// One batch-channel's read of the bake.
struct Verdict {
    why: Why,
    /// The largest disagreement between slot 0 and another slot; 0 means only a flag differs.
    delta: f32,
    detail: String,
}

/// The largest difference between two baked loops in period, key times and values; `None` when
/// their key count or clock law differs.
fn loop_delta<V: AsRef<[f32]>>(a: &KeyAnim<V>, b: &KeyAnim<V>) -> Option<f32> {
    if a.step != b.step || a.gseq != b.gseq || a.keys.len() != b.keys.len() {
        return None;
    }
    let mut d = (a.period - b.period).abs();
    for ((ta, va), (tb, vb)) in a.keys.iter().zip(&b.keys) {
        d = d.max((ta - tb).abs());
        for (x, y) in va.as_ref().iter().zip(vb.as_ref()) {
            d = d.max((x - y).abs());
        }
    }
    Some(d)
}

/// `[.L]`, one character per file sequence slot from slot 0: `L` bakes a loop, `.` moves nothing.
fn slot_cell<V>(slots: &[Option<KeyAnim<V>>]) -> String {
    const HEAD: usize = 28;
    let bits: String = slots
        .iter()
        .map(|s| if s.is_some() { 'L' } else { '.' })
        .collect();
    let live = slots.iter().filter(|s| s.is_some()).count();
    if bits.len() <= HEAD {
        format!("[{bits}]")
    } else {
        format!("[{}…] {live}/{} live", &bits[..HEAD], slots.len())
    }
}

/// `[Wc.]`, each slot's clock law: `W` wrap, `c` clamp, `.` dead.
fn wrap_cell<V>(slots: &[Option<KeyAnim<V>>]) -> String {
    slots
        .iter()
        .map(|s| match s {
            Some(l) if l.wrap => 'W',
            Some(_) => 'c',
            None => '.',
        })
        .collect::<String>()
}

/// One baked loop as `1.500s wrap 28k x[+0.000..+0.938] y[…]`: period, clock, keys, ranges.
fn loop_cell<V: AsRef<[f32]>>(l: &KeyAnim<V>, ch: Channel) -> String {
    let range = ch
        .axes()
        .iter()
        .enumerate()
        .map(|(c, axis)| {
            let (lo, hi) = l
                .keys
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), (_, v)| {
                    let x = v.as_ref()[c];
                    (lo.min(x), hi.max(x))
                });
            format!("{axis}[{lo:+.3}..{hi:+.3}]")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{:>6.3}s {} {:>3}k {range}",
        l.period,
        if l.wrap { "wrap " } else { "clamp" },
        l.keys.len(),
    )
}

/// Why one batch-channel's slots disagree, read through [`SeqLoops::slots`] as the runtime does.
fn classify<V: AsRef<[f32]> + PartialEq>(set: &SeqLoops<V>, ch: Channel) -> Verdict {
    let slots = set.slots();
    let cell = slot_cell(slots);
    // A set exists only when some slot bakes, so a dead slot 0 has a live later one.
    let Some(base) = slots.first().and_then(Option::as_ref) else {
        let (s, live) = slots
            .iter()
            .enumerate()
            .find_map(|(s, l)| Some((s, l.as_ref()?)))
            .expect("a set exists only when some slot bakes a loop");
        return Verdict {
            why: Why::Dead0,
            delta: 0.0,
            detail: format!("{cell} slot 0 dead, slot {s} {}", loop_cell(live, ch)),
        };
    };
    let (mut delta, mut wrap) = (0.0f32, false);
    for (s, slot) in slots.iter().enumerate().skip(1) {
        let Some(l) = slot.as_ref() else {
            return Verdict {
                why: Why::RealDiffers,
                delta,
                detail: format!("{cell} slot 0 {} vs slot {s} dead", loop_cell(base, ch),),
            };
        };
        // Name what diverges: a value range hides a timing difference behind identical brackets.
        let reason = match loop_delta(base, l) {
            Some(d) if d <= KEY_EPS => {
                delta = delta.max(d);
                wrap |= l.wrap != base.wrap;
                continue;
            }
            Some(d) => format!("keys apart by {d:.1e}"),
            None => "a different key count or clock law".to_string(),
        };
        return Verdict {
            why: Why::RealDiffers,
            delta,
            detail: format!(
                "{cell} slot 0 {} vs slot {s} {} — {reason}",
                loop_cell(base, ch),
                loop_cell(l, ch),
            ),
        };
    }
    let why = if wrap {
        Why::WrapOnly
    } else {
        Why::KeysEpsilon
    };
    Verdict {
        why,
        delta,
        detail: format!(
            "{cell} {} clock {} delta {delta:.1e}",
            loop_cell(base, ch),
            wrap_cell(slots),
        ),
    }
}

/// Per-channel corpus counters; a model counts once in each bucket any of its batches lands in.
#[derive(Default)]
struct Tally {
    /// Batches whose slots all bake the same loop, which keep the shared lane.
    shared_b: u32,
    /// Of those, the ones that animate there (`period > 0`).
    shared_live_b: u32,
    shared_live_m: u32,
    /// Batches and models on the per-placement lane, by [`Why`].
    per_b: [u32; 4],
    per_m: [u32; 4],
    per_m_any: u32,
    /// WRAP-ONLY batches bit-identical apart from the flag.
    wrap_exact: u32,
}

/// One model's per-placement batches in one (channel, bucket).
struct Hit {
    name: String,
    ch: Channel,
    why: Why,
    batches: u32,
    slots: usize,
}

const TAIL_ROWS: usize = 25;

/// Census the batches the bake gives a per-placement material because their UV or tint loop
/// differs between file sequence slots, read off the bake's own verdict (`uv_seq`/`rgb_seq`,
/// `Some` when [`SeqLoops::uniform`] refused the shared lane), with the `Why` of each.
pub fn uvslotscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let names = super::m2_names(chain, prefix)?;
    let (mut scanned, mut batches) = (0u32, 0u64);
    let mut tally: [Tally; 2] = Default::default();
    let mut hits: Vec<Hit> = Vec::new();

    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let dir = name.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            continue;
        };
        scanned += 1;
        batches += subs.len() as u64;
        let mut lines: Vec<String> = Vec::new();
        // This model's counts per channel: (shared, shared and live), per bucket, its slot count.
        let mut shared = [(0u32, 0u32); 2];
        let mut per = [[0u32; 4]; 2];
        let mut slots = [0usize; 2];
        for (bi, sub) in subs.iter().enumerate() {
            for ch in [Channel::Uv, Channel::Rgb] {
                let (verdict, live, n) = match ch {
                    Channel::Uv => (
                        sub.uv_seq.as_ref().map(|s| classify(s, ch)),
                        sub.uv_anim.as_ref().is_some_and(|a| a.period > 0.0),
                        sub.uv_seq.as_ref().map_or(0, |s| s.slots().len()),
                    ),
                    Channel::Rgb => (
                        sub.rgb_seq.as_ref().map(|s| classify(s, ch)),
                        sub.rgb_anim.as_ref().is_some_and(|a| a.period > 0.0),
                        sub.rgb_seq.as_ref().map_or(0, |s| s.slots().len()),
                    ),
                };
                let c = ch as usize;
                let Some(v) = verdict else {
                    shared[c].0 += 1;
                    shared[c].1 += u32::from(live);
                    continue;
                };
                per[c][v.why as usize] += 1;
                slots[c] = n;
                if v.why == Why::WrapOnly && v.delta == 0.0 {
                    tally[c].wrap_exact += 1;
                }
                lines.push(format!(
                    "    {} batch {bi:>3}  {:<12}  {}",
                    ch.label(),
                    v.why.label(),
                    v.detail
                ));
            }
        }
        for ch in [Channel::Uv, Channel::Rgb] {
            let c = ch as usize;
            let t = &mut tally[c];
            t.shared_b += shared[c].0;
            t.shared_live_b += shared[c].1;
            t.shared_live_m += u32::from(shared[c].1 > 0);
            t.per_m_any += u32::from(per[c].iter().any(|&n| n > 0));
            for w in Why::ALL {
                let n = per[c][w as usize];
                if n > 0 {
                    t.per_b[w as usize] += n;
                    t.per_m[w as usize] += 1;
                    hits.push(Hit {
                        name: name.clone(),
                        ch,
                        why: w,
                        batches: n,
                        slots: slots[c],
                    });
                }
            }
        }
        if !lines.is_empty() {
            println!(
                "{name}  ({} file sequence slots)",
                slots.iter().copied().max().unwrap_or(0)
            );
            for l in &lines {
                println!("{l}");
            }
        }
    }

    println!();
    println!("=== summary ===  {scanned} model(s) parsed, {batches} render batch(es)");
    for ch in [Channel::Uv, Channel::Rgb] {
        let t = &tally[ch as usize];
        let per_total: u32 = t.per_b.iter().sum();
        println!();
        println!(
            "=== {} ===  {} batch-channel(s)",
            ch.title(),
            u64::from(t.shared_b) + u64::from(per_total)
        );
        println!(
            "  SHARED         {:>7} batch(es)  — `{}` is None: every slot bakes the same loop, so \
             the batch keeps the per-material lane it has always had",
            t.shared_b,
            ch.field()
        );
        println!(
            "    of those     {:>7} batch(es) / {:>4} model(s) carry a LIVE loop there \
             (period > 0); the rest animate nothing at all",
            t.shared_live_b, t.shared_live_m
        );
        println!(
            "  PER-PLACEMENT  {:>7} batch(es) / {:>4} model(s)  — `{}` is Some: `uniform()` \
             refused the shared lane, so the world streamer builds this batch a material per \
             placement",
            per_total,
            t.per_m_any,
            ch.field()
        );
        for w in Why::ALL {
            println!(
                "    {:<13} {:>5} batch(es) / {:>4} model(s)  — {}",
                w.label(),
                t.per_b[w as usize],
                t.per_m[w as usize],
                w.blurb()
            );
        }
        if t.per_b[Why::WrapOnly as usize] > 0 {
            println!(
                "      ({} of the WRAP-ONLY batches are bit-identical apart from the flag; the \
                 rest carry sub-epsilon key noise on top of it)",
                t.wrap_exact
            );
        }
        for w in Why::ALL {
            let rows: Vec<&Hit> = hits
                .iter()
                .filter(|h| h.ch == ch && h.why == w)
                .collect::<Vec<_>>();
            if rows.is_empty() {
                continue;
            }
            println!();
            println!("  --- {} ---  {} model(s)", w.label(), rows.len());
            let mut fams: BTreeMap<String, (u32, u32)> = BTreeMap::new();
            for h in &rows {
                let e = fams.entry(super::family_of(&h.name)).or_default();
                e.0 += 1;
                e.1 += h.batches;
            }
            for (fam, (m, b)) in &fams {
                println!("    {fam:<28} {m:>4} model(s)  {b:>5} batch(es)");
            }
            let mut tail = rows;
            tail.sort_by(|a, b| b.batches.cmp(&a.batches).then_with(|| a.name.cmp(&b.name)));
            for h in tail.iter().take(TAIL_ROWS) {
                println!(
                    "      {:>4} batch(es)  {:>3} slots  {}",
                    h.batches, h.slots, h.name
                );
            }
            if let Some(rest) = tail.len().checked_sub(TAIL_ROWS).filter(|n| *n > 0) {
                println!("      … and {rest} more (top {TAIL_ROWS} shown)");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// fxuvscan, and the per-batch reader down to `uv_batch` that `entityuvscan` shares
// ---------------------------------------------------------------------------

const FX_SPELL_ROWS: usize = 12;

const FX_UNREAD_ROWS: usize = 20;

/// Time samples per slot's loop in the animated sweep; the reported box is their union.
const FX_ANIM_SAMPLES: usize = 64;

/// One texture's mip-0 alpha, plus [`Self::rgb`] for blend 3 `NoAlphaAdd` (`ONE/ONE`), which adds
/// colour whatever the alpha (`BloodSpurtSmall01`'s border: alpha 0, RGB up to 107); `Opaque`,
/// `Mod` and `Mod2x` batches are not judged from the sheet.
struct Sheet {
    w: u32,
    h: u32,
    /// Row-major, one byte per texel.
    alpha: Vec<u8>,
    /// Row-major `max(r, g, b)`.
    rgb: Vec<u8>,
}

impl Sheet {
    /// The inclusive texel ranges a UV span `[lo, hi]` reaches: clamped to the edge texel, or
    /// folded into `0..1` when `wrap`.
    fn axis(lo: f32, hi: f32, n: u32, wrap: bool) -> Vec<(u32, u32)> {
        let last = n.saturating_sub(1);
        let texel = |t: f32| ((t * n as f32) as i64).clamp(0, i64::from(last)) as u32;
        if !wrap {
            return vec![(texel(lo.clamp(0.0, 1.0)), texel(hi.clamp(0.0, 1.0)))];
        }
        if hi - lo >= 1.0 {
            return vec![(0, last)];
        }
        let a = lo - lo.floor();
        let b = a + (hi - lo);
        if b <= 1.0 {
            vec![(texel(a), texel(b))]
        } else {
            vec![(texel(a), last), (0, texel(b - 1.0))]
        }
    }

    /// The most the sheet paints in the UV box `u × v`: its greatest alpha, and with `with_rgb`
    /// its greatest `max(r, g, b)`. The box bounds the triangles, so a 0 is never a false one.
    fn paint(
        &self,
        u: (f32, f32),
        v: (f32, f32),
        wrap_x: bool,
        wrap_y: bool,
        with_rgb: bool,
    ) -> u8 {
        if self.w == 0 || self.h == 0 {
            return 0;
        }
        let mut best = 0u8;
        for &(x0, x1) in &Self::axis(u.0, u.1, self.w, wrap_x) {
            for &(y0, y1) in &Self::axis(v.0, v.1, self.h, wrap_y) {
                for y in y0..=y1 {
                    let row = (y * self.w) as usize;
                    for x in x0..=x1 {
                        let i = row + x as usize;
                        best = best.max(self.alpha[i]);
                        if with_rgb {
                            best = best.max(self.rgb[i]);
                        }
                    }
                }
            }
        }
        best
    }
}

/// One texture's [`Sheet`], memoised by lowercased path; `None`, reported `UNKNOWN`, if it will
/// not load.
fn fx_sheet<'a>(
    cache: &'a mut BTreeMap<String, Option<Sheet>>,
    chain: &mut Chain,
    path: &str,
) -> Option<&'a Sheet> {
    cache
        .entry(path.to_ascii_lowercase())
        .or_insert_with(|| {
            benilla_formats::read_texture_rgba(chain, path)
                .ok()
                .map(|(w, h, rgba)| Sheet {
                    w,
                    h,
                    alpha: rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect(),
                    rgb: rgba
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[0].max(p[1]).max(p[2]))
                        .collect(),
                })
        })
        .as_ref()
}

/// An effect model's `SpellVisualEffectName` rows and the spells that reach it, by stage.
#[derive(Default)]
struct FxReach {
    /// The table's own spelling of the path (`.mdx`), for display.
    path: String,
    effects: std::collections::BTreeSet<u32>,
    /// spell id → (name, the stages that reach this model).
    spells: BTreeMap<u32, (String, std::collections::BTreeSet<&'static str>)>,
}

/// Fold one `SpellVisualEffectName` id's model into the corpus, noting `spell`'s stage when given;
/// an id with no row or path adds nothing.
fn fx_record(
    out: &mut BTreeMap<String, FxReach>,
    visuals: &benilla_formats::SpellVisualCatalog,
    effect: u32,
    spell: Option<(u32, &str, &'static str)>,
) {
    let Some(path) = visuals.effect_path(effect) else {
        return;
    };
    // `SpellVisualEffectName` 1180 spells its path with a leading separator; the chain holds it
    // without one, so the key drops it and the display path keeps it.
    let e = out
        .entry(crate::model_key(path.trim_start_matches(['\\', '/'])))
        .or_default();
    if e.path.is_empty() {
        e.path = path.to_string();
    }
    e.effects.insert(effect);
    if let Some((id, name, stage)) = spell {
        e.spells
            .entry(id)
            .or_insert_with(|| (name.to_string(), std::collections::BTreeSet::new()))
            .1
            .insert(stage);
    }
}

/// Every model the spell-visual tables reach, keyed by [`crate::model_key`], then joined to the
/// spells: every kit's ten effect slots, and each `SpellVisual` row's missile model (field 7) and
/// dest-anchored model (field 12). Stage `area` covers field 12 and the area kit (field 13).
fn fx_corpus(
    visuals: &benilla_formats::SpellVisualCatalog,
    spells: &benilla_formats::SpellCatalog,
) -> BTreeMap<String, FxReach> {
    let mut out: BTreeMap<String, FxReach> = BTreeMap::new();
    for kit_id in visuals.kit_ids() {
        let Some(kit) = visuals.kit(kit_id) else {
            continue;
        };
        for (_, effect) in kit.effects() {
            fx_record(&mut out, visuals, effect, None);
        }
    }
    for (_, st) in visuals.visuals() {
        fx_record(&mut out, visuals, st.missile_model, None);
        fx_record(&mut out, visuals, st.area_effect, None);
    }
    for (id, sp) in spells.iter() {
        let Some(st) = visuals.stages(sp.visual).copied() else {
            continue;
        };
        for (stage, kit_id) in [
            ("precast", st.precast),
            ("cast", st.cast),
            ("impact", st.impact),
            ("state", st.state),
            ("channel", st.channel),
            ("area", st.area_kit),
        ] {
            let Some(kit) = visuals.kit(kit_id) else {
                continue;
            };
            for (_, effect) in kit.effects() {
                fx_record(&mut out, visuals, effect, Some((id, &sp.name, stage)));
            }
        }
        fx_record(
            &mut out,
            visuals,
            st.missile_model,
            Some((id, &sp.name, "missile")),
        );
        fx_record(
            &mut out,
            visuals,
            st.area_effect,
            Some((id, &sp.name, "area")),
        );
    }
    out
}

/// What a consumer that runs no texture-transform animation renders for a keyed batch.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FxClass {
    /// Frozen, the batch samples no painted texel while the animation does: it draws nothing.
    Invisible,
    /// It paints nothing at any point of the loop, so the frozen transform is not its fault.
    Never,
    /// It draws, but statically.
    Frozen,
    /// A constant non-identity offset (`period == 0`), which a lane seeding none draws misplaced.
    Held,
    /// No call from the sheet: a `Mod`/`Mod2x` batch, or a texture that will not load.
    Unknown,
}

impl FxClass {
    const ALL: [Self; 5] = [
        Self::Invisible,
        Self::Never,
        Self::Frozen,
        Self::Held,
        Self::Unknown,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Invisible => "INVISIBLE",
            Self::Never => "NEVER",
            Self::Frozen => "FROZEN",
            Self::Held => "HELD",
            Self::Unknown => "UNKNOWN",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Invisible => {
                "frozen, the batch samples only transparent texels while the scroll reaches painted \
                 ones — it renders NOTHING"
            }
            Self::Never => {
                "nothing painted at any point of the loop — the missing scroll is not this batch's \
                 problem (investigate)"
            }
            Self::Frozen => "it draws, statically: the scroll is the motion it loses",
            Self::Held => {
                "keyed to a constant non-identity offset — a lane that seeds none draws it \
                 mis-registered, not still"
            }
            Self::Unknown => {
                "no alpha lane to judge from (Mod/Mod2x), or the texture would not decode"
            }
        }
    }
}

/// The texture-transform triple [`benilla_formats::uv_transform`] composes at `(slot, t)`: the
/// per-slot set when the bake built one, else the shared loop.
fn fx_state(
    sub: &benilla_formats::RenderSubmesh,
    slot: usize,
    t: f32,
) -> ([f32; 2], [f32; 4], [f32; 2]) {
    let trans = match (&sub.uv_seq, &sub.uv_anim) {
        (Some(s), _) => s.seq(Some(slot)).map_or([0.0, 0.0], |l| l.sample(t)),
        (None, Some(a)) => a.sample(t),
        (None, None) => [0.0, 0.0],
    };
    let rot = sub
        .uv_rot_seq
        .as_ref()
        .and_then(|r| r.seq(Some(slot)))
        .map_or([0.0, 0.0, 0.0, 1.0], |l| l.sample(t));
    let scale = sub
        .uv_scale_seq
        .as_ref()
        .and_then(|r| r.seq(Some(slot)))
        .map_or([1.0, 1.0], |l| l.sample(t));
    (trans, rot, scale)
}

/// How many file sequence slots the batch's transform was baked across, at least 1.
fn fx_slots(sub: &benilla_formats::RenderSubmesh) -> usize {
    let n = |v: Option<usize>| v.unwrap_or(0);
    n(sub.uv_seq.as_ref().map(|s| s.slots().len()))
        .max(n(sub.uv_rot_seq.as_ref().map(|s| s.slots().len())))
        .max(n(sub.uv_scale_seq.as_ref().map(|s| s.slots().len())))
        .max(1)
}

/// The longest transform period in slot `slot`; 0 when every channel holds.
fn fx_period(sub: &benilla_formats::RenderSubmesh, slot: usize) -> f32 {
    let t = match (&sub.uv_seq, &sub.uv_anim) {
        (Some(s), _) => s.seq(Some(slot)).map_or(0.0, |l| l.period),
        (None, Some(a)) => a.period,
        (None, None) => 0.0,
    };
    let r = sub
        .uv_rot_seq
        .as_ref()
        .and_then(|s| s.seq(Some(slot)))
        .map_or(0.0, |l| l.period);
    let s = sub
        .uv_scale_seq
        .as_ref()
        .and_then(|s| s.seq(Some(slot)))
        .map_or(0.0, |l| l.period);
    t.max(r).max(s)
}

/// The UV bounding box through [`benilla_formats::uv_transform`], all four corners.
fn fx_box(
    u: (f32, f32),
    v: (f32, f32),
    t: [f32; 2],
    q: [f32; 4],
    s: [f32; 2],
) -> ((f32, f32), (f32, f32)) {
    let mut out = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
    for uv in [[u.0, v.0], [u.1, v.0], [u.0, v.1], [u.1, v.1]] {
        let p = benilla_formats::uv_transform(uv, t, q, s);
        out.0 .0 = out.0 .0.min(p[0]);
        out.0 .1 = out.0 .1.max(p[0]);
        out.1 .0 = out.1 .0.min(p[1]);
        out.1 .1 = out.1 .1.max(p[1]);
    }
    out
}

/// Whether the batch's texture transform is keyed: the shared loop or any per-sequence set.
fn uv_keyed(sub: &benilla_formats::RenderSubmesh) -> bool {
    sub.uv_anim.is_some()
        || sub.uv_seq.is_some()
        || sub.uv_rot_seq.is_some()
        || sub.uv_scale_seq.is_some()
}

/// Whether the material table (read as `blendscan` does) authors blend 3 `NoAlphaAdd`: the bake
/// folds modes 3 and 4 into `additive`, so such a model's additive batches are judged on RGB too.
fn authors_no_alpha_add(bytes: &[u8]) -> bool {
    let at = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?))
    };
    match (at(0x84), at(0x88)) {
        (Some(n), Some(ofs)) => (0..n as usize).any(|i| {
            let o = ofs as usize + i * 4;
            bytes
                .get(o + 2..o + 4)
                .is_some_and(|b| u16::from_le_bytes([b[0], b[1]]) == 3)
        }),
        _ => false,
    }
}

/// One keyed batch's texture-transform read, shared by [`fxuvscan`] and [`entityuvscan`]. The
/// caller supplies the sheet: a creature batch's `Monster1/2/3` slot is filled per display from
/// `CreatureDisplayInfo`, so it has one sheet per skin.
struct UvBatch {
    /// The verdict at the identity, the offset a material built with `play_uv = false` seeds.
    class: FxClass,
    /// The verdict at the first key, where a lane that seeds the loop and never ticks it sits.
    key_class: FxClass,
    /// `uv_anim.sample(0.0)`, `[0, 0]` without a shared loop.
    seed: [f32; 2],
    lines: Vec<String>,
}

/// Read one batch's texture transform; the caller has checked [`uv_keyed`] and that it has UVs.
fn uv_batch(
    sheets: &mut BTreeMap<String, Option<Sheet>>,
    chain: &mut Chain,
    sub: &benilla_formats::RenderSubmesh,
    bi: usize,
    tex: Option<&str>,
    no_alpha_add: bool,
) -> UvBatch {
    let ext = |axis: usize| {
        sub.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
            (lo.min(t[axis]), hi.max(t[axis]))
        })
    };
    let (au, av) = (ext(0), ext(1));
    let slots = fx_slots(sub);
    let moves = (0..slots).any(|s| fx_period(sub, s) > 0.0);

    // Two frozen states: the identity, which a material built with `play_uv = false` seeds
    // (`sun_scale.zw = (0, 0)`, the authored UVs), and the first key, which `model_material`
    // seeds when handed the loop. Rotation and scale stay at the identity in both: a shared
    // material carries only the translation seed.
    let seed = sub.uv_anim.as_ref().map_or([0.0, 0.0], |a| a.sample(0.0));
    let (fu, fv) = fx_box(au, av, [0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0]);
    let (ku, kv) = fx_box(au, av, seed, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0]);

    // The animated reach: the union over every slot's sampled loop, an over-approximation.
    let mut ru = (f32::MAX, f32::MIN);
    let mut rv = (f32::MAX, f32::MIN);
    for slot in 0..slots {
        let period = fx_period(sub, slot);
        for i in 0..FX_ANIM_SAMPLES {
            let t = period * i as f32 / (FX_ANIM_SAMPLES - 1) as f32;
            let (tr, q, sc) = fx_state(sub, slot, t);
            let (bu, bv) = fx_box(au, av, tr, q, sc);
            ru = (ru.0.min(bu.0), ru.1.max(bu.1));
            rv = (rv.0.min(bv.0), rv.1.max(bv.1));
        }
    }

    let sheet = tex.and_then(|p| fx_sheet(sheets, chain, p));
    let reads_alpha = matches!(
        sub.blend,
        benilla_formats::ModelBlend::Blend | benilla_formats::ModelBlend::AlphaTest
    );
    let opaque = sub.blend == benilla_formats::ModelBlend::Opaque;
    let with_rgb = sub.additive && no_alpha_add;
    let (frozen_a, key_a, anim_a, dims) = match sheet {
        Some(s) => (
            s.paint(fu, fv, sub.wrap_x, sub.wrap_y, with_rgb),
            s.paint(ku, kv, sub.wrap_x, sub.wrap_y, with_rgb),
            s.paint(ru, rv, sub.wrap_x, sub.wrap_y, with_rgb),
            format!("{}x{}", s.w, s.h),
        ),
        None => (0, 0, 0, "—".to_string()),
    };
    let verdict = |painted: u8| {
        if !moves {
            FxClass::Held
        } else if opaque {
            FxClass::Frozen
        } else if !reads_alpha || sheet.is_none() {
            FxClass::Unknown
        } else if painted > 0 {
            FxClass::Frozen
        } else if anim_a > 0 {
            FxClass::Invisible
        } else {
            FxClass::Never
        }
    };
    let class = verdict(frozen_a);
    let key_class = verdict(key_a);

    let chans = |sub: &benilla_formats::RenderSubmesh| {
        let n = |on: bool, s: &'static str| if on { s } else { "—" };
        format!(
            "{}{}{}",
            n(sub.uv_anim.is_some() || sub.uv_seq.is_some(), "T"),
            n(sub.uv_rot_seq.is_some(), "R"),
            n(sub.uv_scale_seq.is_some(), "S"),
        )
    };
    // Per axis, in texels: a clamp span past an edge samples the edge texel, so `[+0.951..+1.950]`
    // over 16 texels is column 15 alone.
    let axis = |wrap: bool, a: (f32, f32), f: (f32, f32), n: u32| {
        let (texels, verdict) = match n {
            0 => ("—".to_string(), "no sheet"),
            n => {
                let r = Sheet::axis(f.0, f.1, n, wrap);
                let edge_only = !wrap
                    && r.len() == 1
                    && r[0].0 == r[0].1
                    && (r[0].0 == 0 || r[0].0 == n - 1)
                    && (f.0 < 0.0 || f.1 > 1.0);
                (
                    r.iter()
                        .map(|&(x, y)| format!("{x}..{y}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    if wrap {
                        "wraps"
                    } else if f.1 <= 0.0 || f.0 >= 1.0 {
                        "WHOLLY OUTSIDE 0..1 — the border texel, alone"
                    } else if edge_only {
                        "CLAMPED TO ONE BORDER TEXEL for its whole length"
                    } else {
                        "reaches the sheet"
                    },
                )
            }
        };
        format!(
            "{:<6} authored [{:+.3}..{:+.3}]  frozen [{:+.3}..{:+.3}] -> texels {:<7} {}",
            if wrap { "REPEAT" } else { "CLAMP" },
            a.0,
            a.1,
            f.0,
            f.1,
            texels,
            verdict,
        )
    };
    let (tw, th) = sheet.map_or((0, 0), |s| (s.w, s.h));
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "  batch {bi:>3}  {:?}{}  {} verts  chans {}  {} slot(s)  tex {} {dims}{}",
        sub.blend,
        if sub.additive { " additive" } else { "" },
        sub.positions.len(),
        chans(sub),
        slots,
        tex.unwrap_or("NONE"),
        if with_rgb {
            "  [model authors NoAlphaAdd — judged on alpha OR rgb]"
        } else {
            ""
        },
    ));
    lines.push(format!("      u  {}", axis(sub.wrap_x, au, fu, tw)));
    lines.push(format!("      v  {}", axis(sub.wrap_y, av, fv, th)));
    lines.push(format!(
        "      frozen@identity   alpha max {frozen_a:>3}/255  => {}   \
         [a lane built with `play_uv = false`: the AUTHORED UVs, untransformed]",
        class.label(),
    ));
    lines.push(format!(
        "      frozen@first-key  alpha max {key_a:>3}/255  => {}   \
         seed [{:+.4},{:+.4}] u[{:+.3}..{:+.3}] v[{:+.3}..{:+.3}]",
        key_class.label(),
        seed[0],
        seed[1],
        ku.0,
        ku.1,
        kv.0,
        kv.1,
    ));
    lines.push(format!(
        "      animated          alpha max {anim_a:>3}/255        \
         u[{:+.3}..{:+.3}] v[{:+.3}..{:+.3}] over {} sample(s) x {slots} slot(s)",
        ru.0, ru.1, rv.0, rv.1, FX_ANIM_SAMPLES,
    ));
    UvBatch {
        class,
        key_class,
        seed,
        lines,
    }
}

struct FxHit {
    /// The corpus key (lowercased `.m2`) the reach map joins on.
    key: String,
    batch: usize,
    /// The verdict at the identity seed.
    class: FxClass,
    /// The verdict at the first-key seed.
    key_class: FxClass,
}

/// Classify every keyed texture-transform batch the spell-visual chain reaches by what a consumer
/// that runs none of it renders.
pub fn fxuvscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let visuals = benilla_formats::load_spell_visual_catalog(chain)?;
    let spells = benilla_formats::load_spell_catalog(chain)?;
    let corpus = fx_corpus(&visuals, &spells);
    let pfx = prefix.map(|p| p.to_ascii_lowercase().replace('/', "\\"));

    let mut sheets: BTreeMap<String, Option<Sheet>> = BTreeMap::new();
    let (mut listed, mut read, mut missing, mut batches) = (0u32, 0u32, 0u32, 0u64);
    let (mut anim_models, mut anim_batches) = (0u32, 0u32);
    let mut per_class: BTreeMap<FxClass, u32> = BTreeMap::new();
    let mut per_key_class: BTreeMap<FxClass, u32> = BTreeMap::new();
    let mut hits: Vec<FxHit> = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    let mut seed_disagrees = 0u32;
    // Batches that draw frozen at UVs the loop never opens at.
    let mut misregistered = 0u32;

    for (key, reach) in &corpus {
        if pfx.as_deref().is_some_and(|p| !key.starts_with(p)) {
            continue;
        }
        listed += 1;
        let Ok(bytes) = chain.read_file(key) else {
            missing += 1;
            unread.push(format!("{} (not in the chain)", reach.path));
            continue;
        };
        read += 1;
        let dir = key.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]) else {
            missing += 1;
            unread.push(format!("{} (will not parse)", reach.path));
            continue;
        };
        batches += subs.len() as u64;
        let no_alpha_add = authors_no_alpha_add(&bytes);

        let mut lines: Vec<String> = Vec::new();
        for (bi, sub) in subs.iter().enumerate() {
            if !uv_keyed(sub) || sub.uvs.is_empty() {
                continue;
            }
            anim_batches += 1;
            let read = uv_batch(
                &mut sheets,
                chain,
                sub,
                bi,
                sub.texture.as_deref(),
                no_alpha_add,
            );
            if read.class != read.key_class {
                seed_disagrees += 1;
            }
            if read.class == FxClass::Frozen && (read.seed[0] != 0.0 || read.seed[1] != 0.0) {
                misregistered += 1;
            }
            *per_class.entry(read.class).or_default() += 1;
            *per_key_class.entry(read.key_class).or_default() += 1;
            hits.push(FxHit {
                key: key.clone(),
                batch: bi,
                class: read.class,
                key_class: read.key_class,
            });
            lines.extend(read.lines);
        }
        if !lines.is_empty() {
            anim_models += 1;
            println!(
                "{}  effect {}  ({} spell(s) reach it)",
                reach.path,
                reach
                    .effects
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("·"),
                reach.spells.len(),
            );
            for l in &lines {
                println!("{l}");
            }
        }
    }

    println!();
    println!(
        "=== summary ===  {listed} model(s) swept of the {} the spell-visual chain reaches, {read} \
         read ({missing} unreadable), {batches} render batch(es)",
        corpus.len()
    );
    println!(
        "  {anim_models} model(s) / {anim_batches} batch(es) carry a keyed TEXTURE TRANSFORM — \
         the channel the spell-effect lane and the entity lane both run"
    );
    println!("    {:<10} {:>8} {:>9}", "", "identity", "first-key");
    for c in FxClass::ALL {
        println!(
            "    {:<10} {:>8} {:>9}  — {}",
            c.label(),
            per_class.get(&c).copied().unwrap_or(0),
            per_key_class.get(&c).copied().unwrap_or(0),
            c.blurb(),
        );
    }
    println!(
        "  of the {} batch(es) FROZEN@identity, {misregistered} are frozen at UVs the loop never \
         opens at (a non-zero first key), so what draws is neither the animation nor its opening \
         frame — a static mis-registration on top of the missing motion",
        per_class.get(&FxClass::Frozen).copied().unwrap_or(0),
    );
    println!(
        "  the two columns are the two frozen states a non-running lane can be in: `identity` \
         is a material built with `play_uv = false` (the AUTHORED UVs, drawn \
         untransformed), `first-key` what the same \
         batch becomes on a lane that seeds `uv_anim.sample(0.0)` and never ticks it. \
         {seed_disagrees} batch(es) are classified differently by the two — read the per-batch \
         lines for which."
    );
    if !unread.is_empty() {
        println!();
        // On 1.12 each is a dead `SpellVisualEffectName` row, mostly `Particles\*.mdl` names the
        // archives never shipped.
        let mdl = unread
            .iter()
            .filter(|u| u.to_ascii_lowercase().contains(".mdl "))
            .count();
        println!(
            "--- unreadable ---  {} model(s) the chain names but could not be swept ({mdl} `.mdl` \
             + {} `.mdx`, every one absent from the archives — dead table rows, not a gap)",
            unread.len(),
            unread.len() - mdl,
        );
        for u in unread.iter().take(FX_UNREAD_ROWS) {
            println!("  {u}");
        }
        if let Some(rest) = unread.len().checked_sub(FX_UNREAD_ROWS).filter(|n| *n > 0) {
            println!("  … and {rest} more (top {FX_UNREAD_ROWS} shown)");
        }
    }

    // Every model carrying a batch of class `c` under `pick`'s seed, with the spells that reach it.
    let listing = |title: &str, note: &str, pick: fn(&FxHit) -> FxClass, c: FxClass| {
        let rows: Vec<&FxHit> = hits.iter().filter(|h| pick(h) == c).collect();
        if rows.is_empty() {
            return;
        }
        println!();
        println!("--- {title} ---  {} batch(es){note}", rows.len());
        let mut models: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for h in &rows {
            models.entry(h.key.as_str()).or_default().push(h.batch);
        }
        for (key, bs) in &models {
            let Some(reach) = corpus.get(*key) else {
                continue;
            };
            println!(
                "  {}  batch(es) {}",
                reach.path,
                bs.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            if reach.spells.is_empty() {
                println!("      (no shipped spell reaches it — kit-table content only)");
                continue;
            }
            for (id, (name, stages)) in reach.spells.iter().take(FX_SPELL_ROWS) {
                println!(
                    "      spell {id:<6} {name:<34} {}",
                    stages.iter().copied().collect::<Vec<_>>().join(","),
                );
            }
            if let Some(rest) = reach
                .spells
                .len()
                .checked_sub(FX_SPELL_ROWS)
                .filter(|n| *n > 0)
            {
                println!("      … and {rest} more spell(s) (top {FX_SPELL_ROWS} shown)");
            }
        }
    };
    for c in [FxClass::Invisible, FxClass::Never, FxClass::Unknown] {
        listing(
            c.label(),
            "  (frozen@identity — what the effect lane renders)",
            |h| h.class,
            c,
        );
    }
    // Batches invisible only at the first-key seed, where a lane that never ticks the loop sits.
    let key_only: Vec<&FxHit> = hits
        .iter()
        .filter(|h| h.key_class == FxClass::Invisible && h.class != FxClass::Invisible)
        .collect();
    if !key_only.is_empty() {
        println!();
        println!(
            "--- INVISIBLE under the FIRST-KEY seed only ---  {} batch(es)\n    (these draw today \
             and would go dark on a lane that seeds `uv_anim.sample(0.0)` without ticking it)",
            key_only.len()
        );
        let mut models: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for h in &key_only {
            models.entry(h.key.as_str()).or_default().push(h.batch);
        }
        for (key, bs) in &models {
            println!(
                "  {}  batch(es) {}",
                corpus.get(*key).map_or(*key, |r| r.path.as_str()),
                bs.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }

    // The spells that reach an invisible batch; `+` marks one reached only at the first-key seed.
    let mut reached: BTreeMap<u32, (String, std::collections::BTreeSet<&'static str>, bool)> =
        BTreeMap::new();
    for h in hits
        .iter()
        .filter(|h| h.class == FxClass::Invisible || h.key_class == FxClass::Invisible)
    {
        let Some(reach) = corpus.get(&h.key) else {
            continue;
        };
        let today = h.class == FxClass::Invisible;
        for (id, (name, stages)) in &reach.spells {
            let e = reached
                .entry(*id)
                .or_insert_with(|| (name.clone(), std::collections::BTreeSet::new(), false));
            e.1.extend(stages.iter().copied());
            e.2 |= today;
        }
    }
    let today = reached.values().filter(|v| v.2).count();
    println!();
    println!(
        "=== spells reaching an INVISIBLE batch ===  {today} spell(s) today, {} counting the \
         first-key seed's extra batches (marked `+`)",
        reached.len()
    );
    for (id, (name, stages, today)) in &reached {
        println!(
            "  {} {id:<6} {name:<40} {}",
            if *today { " " } else { "+" },
            stages.iter().copied().collect::<Vec<_>>().join(","),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// entityuvscan: the unit, GameObject and held-item corpus
// ---------------------------------------------------------------------------

const ENTITY_ID_ROWS: usize = 12;

const ENTITY_UNREAD_ROWS: usize = 20;

/// The `Item\ObjectComponents\<dir>\` folders an `ItemDisplayInfo` left model can live in. The
/// table has no folder column (the worn slot picks it), so the census asks the archives.
const ITEM_DIRS_L: [&str; 4] = ["Weapon", "Shield", "Shoulder", "Quiver"];

/// The folders a right model can live in: the right pauldron, or an arrow or bullet.
const ITEM_DIRS_R: [&str; 2] = ["Shoulder", "Ammo"];

/// Helm files are per race and sex, `<stem>_<Ra><S>.m2`: this prefix by race id, then `M` or `F`,
/// as `ensure_item_model` spells them.
const HELM_RACE_PREFIX: [&str; 8] = ["Hu", "Or", "Dw", "Ni", "Sc", "Ta", "Gn", "Tr"];

/// Every table row that can put an entity-lane model on screen.
#[derive(Default)]
struct EntityReach {
    /// The table's own spelling of the path (`.mdx`), for display.
    path: String,
    /// The `CreatureModelData` row ids naming it (normally one).
    creature_models: std::collections::BTreeSet<u32>,
    /// The `CreatureDisplayInfo` rows that reach it, the population measure.
    creature_displays: Vec<u32>,
    /// The playable `(race, sex)` bodies among those displays (`ChrRaces` columns 4 and 5).
    player_bodies: Vec<String>,
    go_displays: Vec<u32>,
    /// `ItemDisplayInfo` rows naming it, and through which `Item\ObjectComponents\` join.
    item_displays: BTreeMap<u32, std::collections::BTreeSet<&'static str>>,
    /// The corpse lane's bone piles (`<Race><Sex>DeathSkeleton`), which no display table names.
    bones: Vec<String>,
    /// Also reached by the spell-visual chain, so also swept by `fxuvscan`.
    also_fx: bool,
}

impl EntityReach {
    /// Table rows that can put this model on screen, a rough population measure.
    fn rows(&self) -> usize {
        self.creature_displays.len()
            + self.go_displays.len()
            + self.item_displays.len()
            + self.bones.len()
    }

    /// The one-line join-back cell printed beside the model path.
    fn cell(&self) -> String {
        let ids = |v: &[u32]| {
            let head: Vec<String> = v.iter().take(ENTITY_ID_ROWS).map(u32::to_string).collect();
            match v.len().checked_sub(ENTITY_ID_ROWS).filter(|n| *n > 0) {
                Some(rest) => format!("{} … +{rest}", head.join(",")),
                None => head.join(","),
            }
        };
        let mut parts: Vec<String> = Vec::new();
        if !self.creature_displays.is_empty() {
            parts.push(format!(
                "creature model {} · {} display(s) {}",
                self.creature_models
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("·"),
                self.creature_displays.len(),
                ids(&self.creature_displays),
            ));
        }
        if !self.player_bodies.is_empty() {
            parts.push(format!("PLAYER body {}", self.player_bodies.join(",")));
        }
        if !self.go_displays.is_empty() {
            parts.push(format!(
                "gameobject {} display(s) {}",
                self.go_displays.len(),
                ids(&self.go_displays),
            ));
        }
        if !self.item_displays.is_empty() {
            let kinds: std::collections::BTreeSet<&str> = self
                .item_displays
                .values()
                .flat_map(|k| k.iter().copied())
                .collect();
            parts.push(format!(
                "item {} display(s) [{}] {}",
                self.item_displays.len(),
                kinds.into_iter().collect::<Vec<_>>().join("/"),
                ids(&self.item_displays.keys().copied().collect::<Vec<_>>()),
            ));
        }
        if !self.bones.is_empty() {
            parts.push(format!("bone pile {}", self.bones.join(",")));
        }
        if self.also_fx {
            parts.push("ALSO a spell-visual model (whose lane runs its transform)".to_string());
        }
        parts.join("  ·  ")
    }
}

/// The corpus, plus the counters that say what the sweep could not see.
#[derive(Default)]
struct EntityCorpus {
    models: BTreeMap<String, EntityReach>,
    /// `CreatureDisplayInfo` rows walked, and `CreatureModelData` rows no display reaches.
    creature_rows: usize,
    creature_orphan_models: usize,
    /// `GameObjectDisplayInfo` rows walked, and those naming a `.wmo` (no M2 batches to animate).
    go_rows: usize,
    go_wmo: usize,
    /// `ItemDisplayInfo` model columns walked; `item_other_dir` names a file under a folder its
    /// column never joins (thrown weapons, swept through the left column), `item_absent` a file
    /// nothing ships.
    item_rows: usize,
    item_other_dir: usize,
    item_absent: usize,
}

/// Fold one model path into the corpus under [`crate::model_key`], keeping the table's spelling.
fn entity_record<'a>(
    out: &'a mut BTreeMap<String, EntityReach>,
    path: &str,
) -> &'a mut EntityReach {
    let e = out
        .entry(crate::model_key(path.trim_start_matches(['\\', '/'])))
        .or_default();
    if e.path.is_empty() {
        e.path = path.to_string();
    }
    e
}

/// Every model the entity lane can render, walked from the tables: `CreatureDisplayInfo` →
/// `CreatureModelData` (player bodies included), `GameObjectDisplayInfo`, `ItemDisplayInfo`'s two
/// model columns joined to the folders that hold them, and the corpse lane's bone piles.
fn entity_corpus(chain: &mut Chain) -> Result<EntityCorpus> {
    let mut out = EntityCorpus::default();

    // --- creatures, players, mounts, critters ---------------------------------------------
    let creatures = benilla_formats::load_creature_catalog(chain).context("CreatureDisplayInfo")?;
    let model_paths: BTreeMap<u32, String> = creatures
        .sized_models()
        .map(|(id, path, _)| (id, path.to_string()))
        .collect();
    let mut reached_models: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut display_model: BTreeMap<u32, u32> = BTreeMap::new();
    for (display, model) in creatures.display_models() {
        display_model.insert(display, model);
        let Some(path) = model_paths.get(&model) else {
            continue;
        };
        out.creature_rows += 1;
        reached_models.insert(model);
        let e = entity_record(&mut out.models, path);
        e.creature_models.insert(model);
        e.creature_displays.push(display);
    }
    out.creature_orphan_models = model_paths.len() - reached_models.len();

    // --- the playable bodies, named as such, and the bone piles they leave ------------------
    if let Ok(cc) = benilla_formats::CharCreateCatalog::load(chain) {
        for race in 1..=u8::try_from(HELM_RACE_PREFIX.len()).unwrap_or(8) {
            let Some(file) = cc.race_file(race).map(str::to_string) else {
                continue;
            };
            for (sex, sex_name) in [(0u8, "Male"), (1, "Female")] {
                if let Some(path) = cc
                    .body_display(race, sex)
                    .and_then(|d| display_model.get(&d))
                    .and_then(|m| model_paths.get(m))
                {
                    entity_record(&mut out.models, &path.clone())
                        .player_bodies
                        .push(format!("{file}{sex_name}"));
                }
                // `0x5d673c`'s format string: the corpse lane's own model, named by no table.
                let bones = format!(
                    "World\\Generic\\PassiveDoodads\\DeathSkeletons\\{file}{sex_name}DeathSkeleton.mdx"
                );
                if chain.read_file(&crate::model_key(&bones)).is_ok() {
                    entity_record(&mut out.models, &bones)
                        .bones
                        .push(format!("{file}{sex_name}"));
                }
            }
        }
    }

    // --- GameObjects -----------------------------------------------------------------------
    let gos = benilla_formats::load_gameobject_catalog(chain).context("GameObjectDisplayInfo")?;
    let go_rows: Vec<(u32, String)> = gos.iter().map(|(id, p)| (id, p.to_string())).collect();
    for (id, path) in go_rows {
        out.go_rows += 1;
        if !crate::model_key(&path).ends_with(".m2") {
            out.go_wmo += 1;
            continue;
        }
        entity_record(&mut out.models, &path).go_displays.push(id);
    }

    // --- held / worn item geometry ----------------------------------------------------------
    let item_files: Vec<String> = super::m2_names(chain, Some("item\\objectcomponents"))?
        .into_iter()
        .map(|n| n.to_ascii_lowercase())
        .collect();
    let files: std::collections::HashSet<&str> = item_files.iter().map(String::as_str).collect();
    // The same listing by basename, to tell `item_other_dir` from `item_absent`.
    let by_base: std::collections::HashSet<&str> = item_files
        .iter()
        .filter_map(|f| f.rsplit_once('\\').map(|(_, b)| b))
        .collect();
    let items = benilla_formats::load_item_display_catalog(chain).context("ItemDisplayInfo")?;
    let item_rows: Vec<(u32, [Option<String>; 2])> =
        items.iter().map(|(id, d)| (id, d.model.clone())).collect();
    for (id, model) in item_rows {
        for (col, dirs) in [(0usize, &ITEM_DIRS_L[..]), (1, &ITEM_DIRS_R[..])] {
            let Some(name) = model[col].as_deref() else {
                continue;
            };
            out.item_rows += 1;
            let mut placed = false;
            let put = |out: &mut EntityCorpus, path: String, kind: &'static str| {
                entity_record(&mut out.models, &path)
                    .item_displays
                    .entry(id)
                    .or_default()
                    .insert(kind);
            };
            for dir in dirs {
                let path = format!("Item\\ObjectComponents\\{dir}\\{name}");
                if files.contains(path.to_ascii_lowercase().as_str()) {
                    put(&mut out, path, dir);
                    placed = true;
                }
            }
            // A helm row names only the stem; the files are per race and sex.
            if col == 0 {
                let stem = name.strip_suffix(".m2").unwrap_or(name);
                for prefix in HELM_RACE_PREFIX {
                    for letter in ['M', 'F'] {
                        let path =
                            format!("Item\\ObjectComponents\\Head\\{stem}_{prefix}{letter}.m2");
                        if files.contains(path.to_ascii_lowercase().as_str()) {
                            put(&mut out, path, "Head");
                            placed = true;
                        }
                    }
                }
            }
            if !placed {
                if by_base.contains(name.to_ascii_lowercase().as_str()) {
                    out.item_other_dir += 1;
                } else {
                    out.item_absent += 1;
                }
            }
        }
    }

    // The id lists come from HashMap walks: sort them so the report diffs run to run.
    for e in out.models.values_mut() {
        e.creature_displays.sort_unstable();
        e.go_displays.sort_unstable();
        e.player_bodies.sort();
        e.bones.sort();
    }

    // --- the models the spell-visual chain also reaches -------------------------------------
    if let (Ok(visuals), Ok(spells)) = (
        benilla_formats::load_spell_visual_catalog(chain),
        benilla_formats::load_spell_catalog(chain),
    ) {
        for key in fx_corpus(&visuals, &spells).keys() {
            if let Some(e) = out.models.get_mut(key) {
                e.also_fx = true;
            }
        }
    }
    Ok(out)
}

/// Which clock a batch's keyed transform runs on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UvClock {
    /// Every live loop runs on a global sequence, a free-running clock the reference anchors once
    /// per instance at attach (`0x70eae1`, `CM2Model+0x68`), so one shared uniform serves it.
    Gseq,
    /// Every live loop runs on its sequence band, the host's play head, which no shared uniform
    /// serves.
    Band,
    /// The batch's three channels disagree: some global, some band.
    Mixed,
    /// A constant hold (`period == 0`): a seed, no clock.
    Hold,
}

impl UvClock {
    const ALL: [Self; 4] = [Self::Gseq, Self::Band, Self::Mixed, Self::Hold];

    fn label(self) -> &'static str {
        match self {
            Self::Gseq => "GSEQ",
            Self::Band => "BAND",
            Self::Mixed => "MIXED",
            Self::Hold => "HOLD",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Gseq => {
                "every live loop rides a GLOBAL SEQUENCE — a free-running clock, so one shared \
                 material uniform is faithful"
            }
            Self::Band => {
                "every live loop rides its SEQUENCE BAND — the instance's own play head, which a \
                 shared uniform cannot serve (a per-instance lane can)"
            }
            Self::Mixed => "the batch's channels disagree: some global-sequence, some band",
            Self::Hold => {
                "keyed but never moving — a constant offset that needs a seed, not a clock"
            }
        }
    }
}

/// One transform channel's read of the clock question.
#[derive(Default)]
struct ChannelClock {
    /// `[L.L]` per file slot; `None` without a per-slot set.
    cell: Option<String>,
    /// File slots whose loop is live (`period > 0`).
    live: Vec<usize>,
    /// `true` for each live loop clocked on a global sequence.
    gseq: Vec<bool>,
    /// The leading live loop as the report prints it: `(period, wrap, gseq)`.
    lead: Option<(f32, bool, bool)>,
    /// The channel's one loop is the shared slot-0 bake.
    uniform: bool,
}

impl ChannelClock {
    /// `gseq 2.000s wrap`, `band 1.500s clamp`, `held`, or a dash.
    fn cell_text(&self) -> String {
        match self.lead {
            None if self.cell.is_none() => "—".to_string(),
            None => "held".to_string(),
            Some((period, wrap, gseq)) => format!(
                "{} {period:.3}s {}",
                if gseq { "gseq" } else { "band" },
                if wrap { "wrap" } else { "clamp" },
            ),
        }
    }
}

/// Read one channel's per-slot set, or the translation channel's shared slot-0 loop.
fn channel_clock<V: AsRef<[f32]> + PartialEq>(
    set: Option<&SeqLoops<V>>,
    shared: Option<&KeyAnim<V>>,
) -> ChannelClock {
    let mut out = ChannelClock::default();
    match set {
        Some(s) => {
            out.cell = Some(slot_cell(s.slots()));
            for (i, l) in s.slots().iter().enumerate() {
                let Some(l) = l.as_ref().filter(|l| l.period > 0.0) else {
                    continue;
                };
                out.live.push(i);
                out.gseq.push(l.gseq);
                out.lead.get_or_insert((l.period, l.wrap, l.gseq));
            }
        }
        None => {
            if let Some(l) = shared {
                out.uniform = true;
                if l.period > 0.0 {
                    out.gseq.push(l.gseq);
                    out.lead = Some((l.period, l.wrap, l.gseq));
                }
            }
        }
    }
    out
}

struct EntityHit {
    key: String,
    batch: usize,
    class: FxClass,
    key_class: FxClass,
    clock: UvClock,
    /// `T`/`R`/`S`, in that order, for the channels the bake keyed.
    chans: [bool; 3],
    /// The translation has a per-sequence set (`uv_seq`), `uvslotscan`'s per-placement population.
    per_seq_trans: bool,
    /// The shared translation loop is live, `uvslotscan`'s live shared count.
    live_shared_trans: bool,
}

/// Classify the keyed texture-transform and tint batches the unit, GameObject and item tables
/// reach.
pub fn entityuvscan(chain: &mut Chain, prefix: Option<&str>) -> Result<()> {
    let corpus = entity_corpus(chain)?;
    let pfx = prefix.map(|p| p.to_ascii_lowercase().replace('/', "\\"));
    let anim_names = benilla_formats::load_anim_data_catalog(chain).ok();
    let creatures = benilla_formats::load_creature_catalog(chain).unwrap_or_default();

    let mut sheets: BTreeMap<String, Option<Sheet>> = BTreeMap::new();
    let (mut listed, mut read, mut missing, mut batches) = (0u32, 0u32, 0u32, 0u64);
    let (mut anim_models, mut anim_batches) = (0u32, 0u32);
    let mut no_uvs = 0u32;
    let mut per_class: BTreeMap<FxClass, u32> = BTreeMap::new();
    let mut per_key_class: BTreeMap<FxClass, u32> = BTreeMap::new();
    let mut per_clock: BTreeMap<UvClock, u32> = BTreeMap::new();
    let mut per_chans: BTreeMap<String, u32> = BTreeMap::new();
    let mut hits: Vec<EntityHit> = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    let mut seed_disagrees = 0u32;
    let mut misregistered = 0u32;
    // The `uvslotscan` cross-check: this corpus's share of the two whole-corpus translation counts.
    let (mut live_shared, mut per_seq) = (0u32, 0u32);
    // Batches whose sheet is filled at runtime (a creature skin, a character composite), and
    // those where two of a model's skins disagree about what a frozen batch paints.
    let (mut skin_filled, mut skin_disagrees) = (0u32, 0u32);

    // --- the tint half of the same sweep ---
    let (mut tint_models, mut tint_batches) = (0u32, 0u32);
    let mut tint_class: BTreeMap<TintClass, u32> = BTreeMap::new();
    let mut tint_clock: BTreeMap<UvClock, u32> = BTreeMap::new();
    let mut tint_hits: Vec<TintHit> = Vec::new();
    let mut tint_blocks: Vec<(String, Vec<String>)> = Vec::new();
    let (mut tint_live_shared, mut tint_per_seq) = (0u32, 0u32);
    let (mut alpha_models, mut alpha_batches, mut alpha_live) = (0u32, 0u32, 0u32);

    let keys: Vec<String> = corpus.models.keys().cloned().collect();
    for key in keys {
        if pfx.as_deref().is_some_and(|p| !key.starts_with(p)) {
            continue;
        }
        let path = corpus.models[&key].path.clone();
        listed += 1;
        let Ok(bytes) = chain.read_file(&key) else {
            missing += 1;
            unread.push(format!("{path} (not in the chain)"));
            continue;
        };
        read += 1;
        let dir = key
            .rsplit_once('\\')
            .map(|(d, _)| d)
            .unwrap_or("")
            .to_string();
        let Ok(subs) = benilla_formats::parse_m2_render_submeshes(&bytes, &dir, &[]) else {
            missing += 1;
            unread.push(format!("{path} (will not parse)"));
            continue;
        };
        batches += subs.len() as u64;
        // Any of the three channels keeps the model: a batch can animate its tint and not its UVs.
        if !subs
            .iter()
            .any(|sub| uv_keyed(sub) || tint_keyed(sub) || sub.alpha_anim.is_some())
        {
            continue;
        }
        let no_alpha_add = authors_no_alpha_add(&bytes);
        // File slot -> `AnimationData.dbc` id; a missing slot is a zero-duration sequence, which
        // the parse drops and the bake keeps.
        let slot_anim: BTreeMap<usize, u16> = benilla_formats::parse_m2_animations(&bytes)
            .iter()
            .map(|a| (a.seq_index, a.anim_id))
            .collect();
        // The creature skins per `Monster1/2/3` slot, from each display's `textureVariation`.
        let mut skins: [std::collections::BTreeSet<String>; 3] = Default::default();
        for d in &corpus.models[&key].creature_displays {
            let Some(m) = creatures.model(*d) else {
                continue;
            };
            for (i, t) in m.textures.iter().enumerate() {
                if let Some(name) = t {
                    skins[i].insert(format!("{dir}\\{name}.blp"));
                }
            }
        }

        // Which bands carry the live loops, named by animation.
        let slot_line = |c: &ChannelClock| -> String {
            match (&c.cell, c.uniform) {
                (Some(cell), _) => {
                    let ids: Vec<String> = c
                        .live
                        .iter()
                        .take(ENTITY_ID_ROWS)
                        .map(|s| match slot_anim.get(s) {
                            Some(id) => match anim_names.as_ref().and_then(|a| a.name(*id)) {
                                Some(n) => format!("{s}:{id} {n}"),
                                None => format!("{s}:{id}"),
                            },
                            None => format!("{s}:—"),
                        })
                        .collect();
                    format!("{cell} live {}", ids.join(" "))
                }
                (None, true) => "SHARED (one loop, every sequence)".to_string(),
                (None, false) => "—".to_string(),
            }
        };

        // --- the tint and alpha pass over the same batches ---------------------------------
        let mut tlines: Vec<String> = Vec::new();
        let mut model_has_alpha = false;
        for (bi, sub) in subs.iter().enumerate() {
            if let Some(a) = sub.alpha_anim.as_ref() {
                alpha_batches += 1;
                model_has_alpha = true;
                if a.slots().iter().any(|q| {
                    [q.color.as_ref(), q.weight.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|l| l.period > 0.0)
                }) {
                    alpha_live += 1;
                }
            }
            if !tint_keyed(sub) {
                continue;
            }
            tint_batches += 1;
            let read = tint_batch(sub, bi);
            let c = channel_clock(sub.rgb_seq.as_ref(), sub.rgb_anim.as_ref());
            let clock = if c.gseq.is_empty() {
                UvClock::Hold
            } else if c.gseq.iter().all(|g| *g) {
                UvClock::Gseq
            } else if c.gseq.iter().all(|g| !*g) {
                UvClock::Band
            } else {
                UvClock::Mixed
            };
            tint_live_shared += u32::from(read.live_shared);
            tint_per_seq += u32::from(read.per_seq);
            *tint_class.entry(read.class).or_default() += 1;
            *tint_clock.entry(clock).or_default() += 1;
            tint_hits.push(TintHit {
                key: key.clone(),
                batch: bi,
                class: read.class,
                clock,
                seed: read.seed,
                rest_wrong: c.live.contains(&0) || (c.uniform && c.lead.is_some()),
                seed_is_white: read.seed_is_white,
                delta: read.delta,
                per_seq: read.per_seq,
                live_shared: read.live_shared,
            });
            tlines.extend(read.lines);
            tlines.push(format!(
                "      clock  {:<5}      {}",
                clock.label(),
                c.cell_text(),
            ));
            tlines.push(format!("      slots  {}", slot_line(&c)));
            if let Some(set) = sub.rgb_seq.as_ref() {
                let v = classify(set, Channel::Rgb);
                tlines.push(format!("      rgb_seq {:<12} {}", v.why.label(), v.detail));
            }
        }
        if model_has_alpha {
            alpha_models += 1;
        }
        if !tlines.is_empty() {
            tint_models += 1;
            tint_blocks.push((path.clone(), tlines));
        }

        let mut lines: Vec<String> = Vec::new();
        for (bi, sub) in subs.iter().enumerate() {
            if !uv_keyed(sub) {
                continue;
            }
            if sub.uvs.is_empty() {
                no_uvs += 1;
                continue;
            }
            anim_batches += 1;

            // The sheet: the authored path, else each of the display's skins, which can differ.
            let candidates: Vec<String> = match (&sub.texture, sub.skin_slot, sub.char_slot) {
                (Some(t), _, _) => vec![t.clone()],
                (None, Some(slot), _) => {
                    skin_filled += 1;
                    skins
                        .get(slot as usize)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default()
                }
                (None, None, Some(_)) => {
                    // A runtime character composite has no sheet to judge: UNKNOWN.
                    skin_filled += 1;
                    Vec::new()
                }
                (None, None, None) => Vec::new(),
            };
            let mut reads: Vec<(Option<String>, UvBatch)> = Vec::new();
            if candidates.is_empty() {
                let r = uv_batch(&mut sheets, chain, sub, bi, None, no_alpha_add);
                reads.push((None, r));
            } else {
                for c in &candidates {
                    let r = uv_batch(&mut sheets, chain, sub, bi, Some(c), no_alpha_add);
                    reads.push((Some(c.clone()), r));
                }
            }
            if reads.iter().any(|(_, r)| r.class != reads[0].1.class) {
                skin_disagrees += 1;
            }
            let read = &reads[0].1;

            let t = channel_clock(sub.uv_seq.as_ref(), sub.uv_anim.as_ref());
            let r = channel_clock::<[f32; 4]>(sub.uv_rot_seq.as_ref(), None);
            let s = channel_clock::<[f32; 2]>(sub.uv_scale_seq.as_ref(), None);
            let flags: Vec<bool> = [&t, &r, &s].iter().flat_map(|c| c.gseq.clone()).collect();
            let clock = if flags.is_empty() {
                UvClock::Hold
            } else if flags.iter().all(|g| *g) {
                UvClock::Gseq
            } else if flags.iter().all(|g| !*g) {
                UvClock::Band
            } else {
                UvClock::Mixed
            };
            let chans = [
                sub.uv_anim.is_some() || sub.uv_seq.is_some(),
                sub.uv_rot_seq.is_some(),
                sub.uv_scale_seq.is_some(),
            ];
            let per_seq_trans = sub.uv_seq.is_some();
            let live_shared_trans =
                !per_seq_trans && sub.uv_anim.as_ref().is_some_and(|a| a.period > 0.0);
            live_shared += u32::from(live_shared_trans);
            per_seq += u32::from(per_seq_trans);

            if read.class != read.key_class {
                seed_disagrees += 1;
            }
            if read.class == FxClass::Frozen && (read.seed[0] != 0.0 || read.seed[1] != 0.0) {
                misregistered += 1;
            }
            *per_class.entry(read.class).or_default() += 1;
            *per_key_class.entry(read.key_class).or_default() += 1;
            *per_clock.entry(clock).or_default() += 1;
            let chan_key: String = ["T", "R", "S"]
                .iter()
                .zip(chans)
                .filter_map(|(n, on)| on.then_some(*n))
                .collect();
            *per_chans.entry(chan_key).or_default() += 1;
            hits.push(EntityHit {
                key: key.clone(),
                batch: bi,
                class: read.class,
                key_class: read.key_class,
                clock,
                chans,
                per_seq_trans,
                live_shared_trans,
            });

            lines.extend(reads[0].1.lines.clone());
            for (sheet, r) in reads.iter().skip(1) {
                lines.push(format!(
                    "      skin variant      {} => {} / {}",
                    sheet.as_deref().unwrap_or("NONE"),
                    r.class.label(),
                    r.key_class.label(),
                ));
            }
            lines.push(format!(
                "      clock  {:<5}      T {:<22} R {:<22} S {}",
                clock.label(),
                t.cell_text(),
                r.cell_text(),
                s.cell_text(),
            ));
            lines.push(format!("      slots  T {}", slot_line(&t)));
            if r.cell.is_some() {
                lines.push(format!("      slots  R {}", slot_line(&r)));
            }
            if s.cell.is_some() {
                lines.push(format!("      slots  S {}", slot_line(&s)));
            }
            // Why the bake refused the shared lane, as `uvslotscan` reports it.
            if let Some(set) = sub.uv_seq.as_ref() {
                let v = classify(set, Channel::Uv);
                lines.push(format!("      uv_seq {:<12} {}", v.why.label(), v.detail));
            }
        }
        if !lines.is_empty() {
            anim_models += 1;
            println!("{path}  [{}]", corpus.models[&key].cell());
            for l in &lines {
                println!("{l}");
            }
        }
    }

    println!();
    println!(
        "=== summary ===  {listed} model(s) swept of the {} the entity tables reach, {read} read \
         ({missing} unreadable), {batches} render batch(es)",
        corpus.models.len()
    );
    println!(
        "  sources: {} CreatureDisplayInfo row(s) ({} CreatureModelData row(s) no display reaches, \
         unswept) · {} GameObjectDisplayInfo row(s) ({} name a .wmo) · {} ItemDisplayInfo model \
         column(s) ({} name a file that ships under a folder this column is never joined to — \
         thrown weapons named in both columns, already swept through the left one — and {} name a \
         file nothing ships) · the bone piles",
        corpus.creature_rows,
        corpus.creature_orphan_models,
        corpus.go_rows,
        corpus.go_wmo,
        corpus.item_rows,
        corpus.item_other_dir,
        corpus.item_absent,
    );
    println!(
        "  {anim_models} model(s) / {anim_batches} batch(es) carry a keyed TEXTURE TRANSFORM — the \
         channel `entity_variants` seeds and registers ({no_uvs} more are keyed but \
         carry no UVs to transform)"
    );
    println!(
        "    channels  {}   (T translation · R rotation · S scaling — a sweep that read only T \
         would miss a scale-only or rotate-only batch entirely)",
        per_chans
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(" · "),
    );
    println!("    {:<10} {:>8} {:>9}", "", "identity", "first-key");
    for c in FxClass::ALL {
        println!(
            "    {:<10} {:>8} {:>9}  — {}",
            c.label(),
            per_class.get(&c).copied().unwrap_or(0),
            per_key_class.get(&c).copied().unwrap_or(0),
            c.blurb(),
        );
    }
    println!(
        "  of the {} batch(es) FROZEN@identity, {misregistered} are frozen at UVs the loop never \
         opens at (a non-zero first key); {seed_disagrees} batch(es) are classified differently by \
         the two seeds",
        per_class.get(&FxClass::Frozen).copied().unwrap_or(0),
    );
    println!();
    println!("  --- the CLOCK the fix has to serve ---");
    for c in UvClock::ALL {
        println!(
            "    {:<6} {:>5} batch(es)  — {}",
            c.label(),
            per_clock.get(&c).copied().unwrap_or(0),
            c.blurb(),
        );
    }
    println!();
    println!(
        "  --- cross-check against `uvslotscan` (whole corpus) ---\n    {live_shared} of this \
         corpus's batch(es) carry a LIVE SHARED translation loop and {per_seq} carry a \
         per-sequence translation set (`uv_seq`). Both must be <= `uvslotscan`'s whole-corpus \
         counts for the same two lanes (its `of those … carry a LIVE loop there` and \
         `PER-PLACEMENT` lines) — this corpus is a subset of that one by construction."
    );
    // The two translation lanes by model, to check against `uvslotscan`'s listings.
    let mut shared_models: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut per_seq_models: BTreeMap<&str, u32> = BTreeMap::new();
    for h in &hits {
        if h.live_shared_trans {
            shared_models.insert(h.key.as_str());
        }
        if h.per_seq_trans {
            *per_seq_models.entry(h.key.as_str()).or_default() += 1;
        }
    }
    println!(
        "    the LIVE SHARED loop lands on {} model(s); the per-sequence set on these, which must \
         each appear in `uvslotscan`'s PER-PLACEMENT listing:",
        shared_models.len()
    );
    for (key, n) in &per_seq_models {
        println!(
            "      {n} batch(es)  {}",
            corpus.models.get(*key).map_or(*key, |r| r.path.as_str())
        );
    }

    println!(
        "    {skin_filled} affected batch(es) take their sheet at RUNTIME (a creature skin \
         variation or a character composite), so the visibility verdict is asked once per skin; \
         {skin_disagrees} of them are classified differently by two skins of the same model."
    );
    if !unread.is_empty() {
        println!();
        println!(
            "--- unreadable ---  {} model(s) the tables name but could not be swept",
            unread.len()
        );
        for u in unread.iter().take(ENTITY_UNREAD_ROWS) {
            println!("  {u}");
        }
        if let Some(rest) = unread
            .len()
            .checked_sub(ENTITY_UNREAD_ROWS)
            .filter(|n| *n > 0)
        {
            println!("  … and {rest} more (top {ENTITY_UNREAD_ROWS} shown)");
        }
    }

    // Every model with a batch of class `c`, and the table rows that reach it.
    let listing = |title: &str, note: &str, pick: fn(&EntityHit) -> FxClass, c: FxClass| {
        let rows: Vec<&EntityHit> = hits.iter().filter(|h| pick(h) == c).collect();
        if rows.is_empty() {
            return;
        }
        println!();
        println!("--- {title} ---  {} batch(es){note}", rows.len());
        let mut models: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for h in &rows {
            models.entry(h.key.as_str()).or_default().push(h.batch);
        }
        for (key, bs) in &models {
            let Some(reach) = corpus.models.get(*key) else {
                continue;
            };
            println!(
                "  {}  batch(es) {}",
                reach.path,
                bs.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            println!("      {}", reach.cell());
        }
    };
    for c in [
        FxClass::Invisible,
        FxClass::Never,
        FxClass::Unknown,
        FxClass::Held,
    ] {
        listing(
            c.label(),
            "  (frozen@identity — what the entity lane renders)",
            |h| h.class,
            c,
        );
    }
    // Batches invisible only at the first-key seed, where a lane that never ticks the loop sits.
    let key_only: Vec<&EntityHit> = hits
        .iter()
        .filter(|h| h.key_class == FxClass::Invisible && h.class != FxClass::Invisible)
        .collect();
    if !key_only.is_empty() {
        println!();
        println!(
            "--- INVISIBLE under the FIRST-KEY seed only ---  {} batch(es)\n    (these draw today \
             and would go dark on a lane that seeds `uv_anim.sample(0.0)` without ticking it)",
            key_only.len()
        );
        let mut models: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for h in &key_only {
            models.entry(h.key.as_str()).or_default().push(h.batch);
        }
        for (key, bs) in &models {
            println!(
                "  {}  batch(es) {}",
                corpus.models.get(*key).map_or(*key, |r| r.path.as_str()),
                bs.iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }

    let mut models: BTreeMap<&str, (Vec<&EntityHit>, usize)> = BTreeMap::new();
    for h in &hits {
        let e = models.entry(h.key.as_str()).or_default();
        e.0.push(h);
        e.1 = corpus.models.get(&h.key).map_or(0, EntityReach::rows);
    }
    let mut tail: Vec<(&str, (Vec<&EntityHit>, usize))> = models.into_iter().collect();
    tail.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(b.0)));
    println!();
    println!(
        "=== affected models, by population ===  {} model(s), {} batch(es)",
        tail.len(),
        hits.len()
    );
    for (key, (hs, rows)) in &tail {
        let Some(reach) = corpus.models.get(*key) else {
            continue;
        };
        let classes: Vec<String> = hs
            .iter()
            .map(|h| {
                format!(
                    "{}:{}{}{}/{}/{}",
                    h.batch,
                    if h.chans[0] { "T" } else { "" },
                    if h.chans[1] { "R" } else { "" },
                    if h.chans[2] { "S" } else { "" },
                    h.class.label(),
                    h.clock.label(),
                )
            })
            .collect();
        println!(
            "  {rows:>5} row(s)  {}\n           batches {}\n           {}",
            reach.path,
            classes.join("  "),
            reach.cell(),
        );
    }

    // =====================================================================================
    // The M2Color tint half: a tint multiplies, where a UV offset looks up.
    // =====================================================================================
    println!();
    println!(
        "============================== M2COLOR TINT ==============================\n\
         The same corpus asked about the OTHER channel, the keyed M2Color tint. A batch is\n\
         seeded at the loop's first key (`model_material`'s `tint.xyz`) and re-sampled only\n\
         where `doodad_anim::register_tint` registers it: placed doodads\n\
         (`terrain_stream/spawn/assemble.rs`), dressed entity parts (`entities/attach/dress.rs`)\n\
         and the glue booth (`portrait/glue_booth.rs`)."
    );
    for (path, block) in &tint_blocks {
        println!(
            "{path}  [{}]",
            corpus
                .models
                .get(&crate::model_key(path))
                .map_or(String::new(), EntityReach::cell)
        );
        for l in block {
            println!("{l}");
        }
    }
    println!();
    println!(
        "=== tint summary ===  {tint_models} model(s) / {tint_batches} batch(es) of the same \
         {read} read model(s) carry a keyed M2Color TINT"
    );
    for c in TintClass::ALL {
        println!(
            "    {:<11} {:>5} batch(es)  — {}",
            c.label(),
            tint_class.get(&c).copied().unwrap_or(0),
            c.blurb(),
        );
    }
    println!();
    println!("  --- the CLOCK each tint rides ---");
    for c in UvClock::ALL {
        println!(
            "    {:<6} {:>5} batch(es)  — {}",
            c.label(),
            tint_clock.get(&c).copied().unwrap_or(0),
            c.blurb(),
        );
    }
    let white = tint_hits.iter().filter(|h| h.seed_is_white).count();
    let rest = tint_hits.iter().filter(|h| h.rest_wrong).count();
    println!();
    println!(
        "  {rest} of {} batch(es) are wrong AT REST — slot 0, the band a resting instance plays, \
         carries a live loop. The other {} key their tint only in a later slot, so the seed is the \
         right colour until that animation plays and the error is confined to it (the per-batch \
         `slots` line names which). Reporting those as flatly broken would overstate the hole.",
        tint_hits.len(),
        tint_hits.len() - rest,
    );
    println!(
        "  {white} batch(es) seed WHITE — no slot-0 bake, so the shared material carries the \
         IDENTITY tint while a later sequence slot colours the batch. The tint twin of the UV \
         lane's DEAD-0, and the one shape a slot-0 shared loop could never fix."
    );
    println!();
    println!(
        "  --- cross-check against `uvslotscan`'s RGB section (whole corpus) ---\n    \
         {tint_live_shared} of this corpus's batch(es) carry a LIVE SHARED tint loop and \
         {tint_per_seq} carry a per-sequence set (`rgb_seq`). Both must be <= that report's \
         `of those … carry a LIVE loop there` and `PER-PLACEMENT` counts for the RGB channel."
    );
    let mut tint_shared_models: std::collections::BTreeSet<&str> =
        std::collections::BTreeSet::new();
    let mut tint_seq_models: BTreeMap<&str, u32> = BTreeMap::new();
    for h in &tint_hits {
        if h.live_shared {
            tint_shared_models.insert(h.key.as_str());
        }
        if h.per_seq {
            *tint_seq_models.entry(h.key.as_str()).or_default() += 1;
        }
    }
    println!(
        "    the LIVE SHARED tint lands on {} model(s); the per-sequence set on these, which must \
         each appear in `uvslotscan`'s RGB PER-PLACEMENT listing:",
        tint_shared_models.len()
    );
    for (key, n) in &tint_seq_models {
        println!(
            "      {n} batch(es)  {}",
            corpus.models.get(*key).map_or(*key, |r| r.path.as_str())
        );
    }
    println!();
    println!(
        "  --- the ALPHA channel, for contrast: this lane DOES serve it ---\n    \
         {alpha_models} model(s) / {alpha_batches} batch(es) carry a baked `alpha_anim`, \
         {alpha_live} of them with a live loop — and every one of them is sampled per instance: \
         `entities::attach::dress::spawn_part` inserts `MatAnim::following(anim, unit)` on each \
         part and card (and `equipment::spawn` on each held item), which \
         `doodad_anim::sample_mat_anim` ticks every frame off the instance's own AnimationPlayer. \
         Counted here so the report cannot be read as `the entity lane runs no material \
         animation`: it runs exactly one of the three channels, and that one is per-instance \
         already."
    );

    let tlisting = |title: &str, c: TintClass| {
        let rows: Vec<&TintHit> = tint_hits.iter().filter(|h| h.class == c).collect();
        if rows.is_empty() {
            return;
        }
        println!();
        println!("--- {title} ---  {} batch(es)", rows.len());
        let mut models: BTreeMap<&str, Vec<&TintHit>> = BTreeMap::new();
        for h in &rows {
            models.entry(h.key.as_str()).or_default().push(h);
        }
        for (key, bs) in &models {
            let Some(reach) = corpus.models.get(*key) else {
                continue;
            };
            println!(
                "  {}  batch(es) {}",
                reach.path,
                bs.iter()
                    .map(|h| {
                        format!(
                            "{} frozen [{:.2},{:.2},{:.2}] Δ{:.2}{}",
                            h.batch,
                            h.seed[0],
                            h.seed[1],
                            h.seed[2],
                            h.delta,
                            if h.seed_is_white { " (WHITE seed)" } else { "" },
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
            println!("      {}", reach.cell());
        }
    };
    for c in [TintClass::Black, TintClass::Strong, TintClass::Slight] {
        tlisting(c.label(), c);
    }

    let mut tmodels: BTreeMap<&str, (Vec<&TintHit>, usize)> = BTreeMap::new();
    for h in &tint_hits {
        let e = tmodels.entry(h.key.as_str()).or_default();
        e.0.push(h);
        e.1 = corpus.models.get(&h.key).map_or(0, EntityReach::rows);
    }
    let mut ttail: Vec<(&str, (Vec<&TintHit>, usize))> = tmodels.into_iter().collect();
    ttail.sort_by(|a, b| b.1 .1.cmp(&a.1 .1).then_with(|| a.0.cmp(b.0)));
    println!();
    println!(
        "=== tint-affected models, by population ===  {} model(s), {} batch(es)",
        ttail.len(),
        tint_hits.len()
    );
    for (key, (hs, rows)) in &ttail {
        let Some(reach) = corpus.models.get(*key) else {
            continue;
        };
        println!(
            "  {rows:>5} row(s)  {}\n           batches {}\n           {}",
            reach.path,
            hs.iter()
                .map(|h| format!("{}:{}/{}", h.batch, h.class.label(), h.clock.label()))
                .collect::<Vec<_>>()
                .join("  "),
            reach.cell(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The M2Color tint half of the entity census
// ---------------------------------------------------------------------------

/// Whether the batch's tint is keyed: the shared slot-0 loop, or the per-sequence set, which
/// alone carries a batch whose slot 0 is dead.
fn tint_keyed(sub: &benilla_formats::RenderSubmesh) -> bool {
    sub.rgb_anim.is_some() || sub.rgb_seq.is_some()
}

/// How many file sequence slots the tint was baked across, at least 1.
fn tint_slots(sub: &benilla_formats::RenderSubmesh) -> usize {
    sub.rgb_seq.as_ref().map_or(1, |s| s.slots().len()).max(1)
}

/// The tint loop's period in slot `slot`; `0.0` where it is a constant hold.
fn tint_period(sub: &benilla_formats::RenderSubmesh, slot: usize) -> f32 {
    match (&sub.rgb_seq, &sub.rgb_anim) {
        (Some(s), _) => s.seq(Some(slot)).map_or(0.0, |l| l.period),
        (None, Some(a)) => a.period,
        (None, None) => 0.0,
    }
}

/// The tint at `(slot, t)`: the per-slot set, else the shared loop, else white.
fn tint_state(sub: &benilla_formats::RenderSubmesh, slot: usize, t: f32) -> [f32; 3] {
    match (&sub.rgb_seq, &sub.rgb_anim) {
        (Some(s), _) => s.seq(Some(slot)).map_or([1.0; 3], |l| l.sample(t)),
        (None, Some(a)) => a.sample(t),
        (None, None) => [1.0; 3],
    }
}

/// Rec.709 relative luminance, a reading aid beside the per-channel deltas.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// What a consumer that seeds the tint and never re-samples it renders, banded by the worst
/// per-channel distance between the seed and anything the loop reaches (0..1 tint units).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TintClass {
    /// The seed is near black while the loop lights up: it draws black, or nothing if additive.
    Black,
    /// 0.25 or more away.
    Strong,
    /// Between 0.05 and 0.25 away: visible side by side, easy to miss alone.
    Slight,
    /// Under 0.05 away: the loop barely moves.
    Negligible,
}

impl TintClass {
    const ALL: [Self; 4] = [Self::Black, Self::Strong, Self::Slight, Self::Negligible];

    fn label(self) -> &'static str {
        match self {
            Self::Black => "BLACK",
            Self::Strong => "STRONG",
            Self::Slight => "SLIGHT",
            Self::Negligible => "NEGLIGIBLE",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Black => {
                "the frozen tint is ~black while the loop lights up — the multiply kills the batch \
                 (an additive one disappears entirely)"
            }
            Self::Strong => {
                "frozen >= 0.25 away from what the loop reaches — an obvious wrong colour"
            }
            Self::Slight => {
                "frozen 0.05..0.25 away — visible against the real thing, easy to miss alone"
            }
            Self::Negligible => "frozen < 0.05 away — the loop barely moves; this is not a bug",
        }
    }
}

struct TintBatch {
    class: TintClass,
    /// What `model_material` seeds into `tint.xyz`: the shared loop's first key, else white.
    seed: [f32; 3],
    /// The seed is white: there is no shared loop, and the tint lives in a later slot.
    seed_is_white: bool,
    /// The worst per-channel distance between the seed and anything the loop reaches.
    delta: f32,
    per_seq: bool,
    live_shared: bool,
    lines: Vec<String>,
}

/// Read one batch's keyed tint; the caller has checked [`tint_keyed`].
fn tint_batch(sub: &benilla_formats::RenderSubmesh, bi: usize) -> TintBatch {
    let slots = tint_slots(sub);
    // `model_material` seeds `tint.xyz` with `rgb_anim`'s first key, or white without one, and
    // the builder always hands it the loop, so an unticked tint holds its opening colour.
    let seed = sub.rgb_anim.as_ref().map_or([1.0; 3], |a| a.sample(0.0));
    let seed_is_white = sub.rgb_anim.is_none();

    // Everything the loop reaches: the same sampled union `uv_batch` takes.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut delta = 0.0f32;
    for slot in 0..slots {
        let period = tint_period(sub, slot);
        for i in 0..FX_ANIM_SAMPLES {
            let t = period * i as f32 / (FX_ANIM_SAMPLES - 1) as f32;
            let c = tint_state(sub, slot, t);
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
                delta = delta.max((c[k] - seed[k]).abs());
            }
        }
    }
    let (ls, llo, lhi) = (luma(seed), luma(lo), luma(hi));
    let class = if ls < 0.02 && lhi >= 0.05 {
        TintClass::Black
    } else if delta >= 0.25 {
        TintClass::Strong
    } else if delta >= 0.05 {
        TintClass::Slight
    } else {
        TintClass::Negligible
    };
    let live_shared =
        !sub.rgb_seq.is_some() && sub.rgb_anim.as_ref().is_some_and(|a| a.period > 0.0);
    let mut lines = Vec::new();
    lines.push(format!(
        "  batch {bi:>3}  {:?}{}  {} verts  {slots} slot(s)",
        sub.blend,
        if sub.additive { " additive" } else { "" },
        sub.positions.len(),
    ));
    lines.push(format!(
        "      seed   [{:.3},{:.3},{:.3}] luma {ls:.3}   {}",
        seed[0],
        seed[1],
        seed[2],
        if seed_is_white {
            "WHITE — no slot-0 bake, so the shared material seeds the IDENTITY while a later slot tints"
        } else {
            "the shared loop's first key (`model_material`'s `tint.xyz`)"
        },
    ));
    lines.push(format!(
        "      loop   r[{:+.3}..{:+.3}] g[{:+.3}..{:+.3}] b[{:+.3}..{:+.3}]  luma [{llo:.3}..{lhi:.3}]  \
         over {} sample(s) x {slots} slot(s)",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2], FX_ANIM_SAMPLES,
    ));
    lines.push(format!(
        "      frozen worst per-channel delta {delta:.3}  => {}",
        class.label(),
    ));
    TintBatch {
        class,
        seed,
        seed_is_white,
        delta,
        per_seq: sub.rgb_seq.is_some(),
        live_shared,
        lines,
    }
}

struct TintHit {
    key: String,
    batch: usize,
    class: TintClass,
    clock: UvClock,
    /// The colour the material is seeded with.
    seed: [f32; 3],
    /// Slot 0, the band a resting instance plays, carries a live loop; otherwise the seed holds
    /// until a later slot's animation plays.
    rest_wrong: bool,
    seed_is_white: bool,
    delta: f32,
    per_seq: bool,
    live_shared: bool,
}
