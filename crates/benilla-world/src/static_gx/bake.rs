//! The bake half of the retained pass (split from `mod.rs` at the 1,000-line budget): the
//! flush walks dirty-and-quiet cells/regions and [`bake_cell`] turns their items into one
//! recentred mesh + per-item draw list. The f64 position leg's rationale lives on the
//! function.

use benilla_assets::coords::wow_to_bevy;
use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

use super::{
    render, GxItem, StaticGx, ATTRIBUTE_GX_ANCHOR, ATTRIBUTE_GX_WORD, IDLE_FRAMES,
    MAX_DIRTY_FRAMES, REBAKE_FRAMES,
};
use super::{
    WORD_CLASS_INT, WORD_CLASS_TRANS, WORD_FOG_OFF, WORD_HAS_VC, WORD_INTERIOR, WORD_MATTE,
    WORD_SHADE_LIT, WORD_TEXTURED, WORD_UNLIT, WORD_WINDOW, WORD_WMO, WORD_WRAP_X, WORD_WRAP_Y,
};

/// The declined-batch census (`declined`), printed once a streaming burst SETTLES rather than on
/// every count change.
///
/// Two things were wrong with printing on change. It fires from a `PostUpdate` system that runs
/// every frame, so a zone streaming in emitted a near-identical INFO line per frame — a Goldshire
/// login produced ten, climbing 8 → 1630, which reads like a fault rather than a residency count.
/// And the line named only refusals, so it could not be told apart from a dead pass; `accepted` is
/// now beside them (at Goldshire the refusals sit against a five-digit accepted population, which
/// is the fact the line was missing).
///
/// The grain is the honest one for a running total: hold until the counts have not moved for
/// [`CENSUS_SETTLE_FRAMES`], then print the total once — with [`CENSUS_MAX_HOLD_FRAMES`] as the
/// ceiling, so a stream that never goes quiet (a long flight path) still reports instead of
/// trading one silence for another.
fn census_declines(gx: &mut StaticGx, frame: u32) {
    if gx.declined == gx.declined_logged {
        return;
    }
    if gx.declined != gx.declined_seen {
        gx.declined_seen = gx.declined;
        gx.declined_changed = frame;
    }
    let settled = frame.wrapping_sub(gx.declined_changed) >= CENSUS_SETTLE_FRAMES;
    let overdue = frame.wrapping_sub(gx.declined_printed) >= CENSUS_MAX_HOLD_FRAMES;
    if !settled && !overdue {
        return;
    }
    gx.declined_printed = frame;
    let d = gx.declined;
    info!(
        "static-gx: {} batch(es) accepted; declined — env-map {}, depth-flag {}, shade-family {}, \
         prop-fader {} (batches), prop-no-instance {} (props)",
        gx.accepted, d[0], d[1], d[2], d[3], d[4]
    );
    gx.declined_logged = d;
}

/// How long the decline counts must sit still before the census prints — ~1 s at 60 fps, which is
/// shorter than a streaming burst and longer than the gaps inside one.
const CENSUS_SETTLE_FRAMES: u32 = 60;

/// The ceiling on that hold — ~10 s at 60 fps. Counts that never settle (a flight path streaming
/// continuously) report on this instead, so the throttle can never turn the census silent.
const CENSUS_MAX_HOLD_FRAMES: u32 = 600;

/// Bake dirty-and-quiet cells into retained draw data; publish into [`render::GxWorld`].
pub(super) fn flush_static_gx(mut gx: ResMut<StaticGx>, mut meshes: ResMut<Assets<Mesh>>) {
    let _t = super::gx_perf_guard(0);
    gx.frame = gx.frame.wrapping_add(1);
    let frame = gx.frame;
    census_declines(&mut gx, frame);
    // The reveal gate's "publish what you have" (see [`StaticGx::flush_now`]): consumed here, so
    // one request bakes one flush's worth of dirty regions and the windows resume next frame.
    let now = std::mem::take(&mut gx.flush_now);
    let StaticGx {
        cells,
        wmos,
        props,
        world,
        ..
    } = &mut *gx;
    for (&cell, state) in cells.iter_mut() {
        if !bake_due(state, world.cells.contains_key(&cell), frame, now) {
            continue;
        }
        state.dirty = false;
        if state.items.is_empty() {
            world.cells.remove(&cell);
            continue;
        }
        // Sort for render-side run coalescing: same bucket + same texture ⇒ adjacent, so the
        // node draws one range per run instead of one per item (class-aware baking is a B2
        // refinement — dims/format live render-side only).
        state
            .items
            .sort_by_key(|i| ((u8::from(i.cutout) << 1) | u8::from(i.two_sided), i.texture));
        remap_fader_items(state);
        let baked = bake_cell(&state.items, &mut meshes);
        debug!(
            "static-gx: cell ({},{}) baked — {} item(s), {} vert(s)",
            cell.0,
            cell.1,
            state.items.len(),
            baked.draws.last().map_or(0, |d| d.vertex_range.end),
        );
        world.cells.insert(cell, std::sync::Arc::new(baked));
    }
    for (&instance, state) in wmos.iter_mut() {
        if !bake_due(state, world.wmos.contains_key(&instance), frame, now) {
            continue;
        }
        state.dirty = false;
        if state.items.is_empty() {
            world.wmos.remove(&instance);
            continue;
        }
        // The WMO sort adds the GROUP inside (bucket, texture): a run must be group-
        // homogeneous — the flood selects ranges per group — and same-texture groups still
        // sit adjacent so the coalescer fuses across items within one group.
        state.items.sort_by_key(|i| {
            (
                (u8::from(i.cutout) << 1) | u8::from(i.two_sided),
                i.texture,
                i.wmo.as_ref().map_or(0, |w| w.group),
            )
        });
        let baked = bake_cell(&state.items, &mut meshes);
        debug!(
            "static-gx: wmo {instance} baked — {} item(s), {} group(s), {} vert(s)",
            state.items.len(),
            baked.groups.len(),
            baked.draws.last().map_or(0, |d| d.vertex_range.end),
        );
        world.wmos.insert(instance, std::sync::Arc::new(baked));
    }
    // The prop regions (B4): the WMO loop's shape with the referrer SET as the selection
    // grain — the sort keeps runs set-homogeneous, and the published draw carries the
    // region's set list beside the per-set bounds `bake_cell` accumulated into `groups`.
    for (&instance, state) in props.iter_mut() {
        if !bake_due(state, world.props.contains_key(&instance), frame, now) {
            continue;
        }
        state.dirty = false;
        if state.items.is_empty() {
            world.props.remove(&instance);
            continue;
        }
        state.items.sort_by_key(|i| {
            (
                (u8::from(i.cutout) << 1) | u8::from(i.two_sided),
                i.texture,
                i.prop.as_ref().map_or(0, |p| p.set),
            )
        });
        let mut baked = bake_cell(&state.items, &mut meshes);
        baked.sets = state.sets.clone();
        debug!(
            "static-gx: props {instance} baked — {} item(s), {} set(s), {} vert(s)",
            state.items.len(),
            baked.sets.len(),
            baked.draws.last().map_or(0, |d| d.vertex_range.end),
        );
        world.props.insert(instance, std::sync::Arc::new(baked));
    }
}

/// Is this region's bake due? `now` is the reveal gate's request ([`StaticGx::flush_now`]) and
/// overrides every window — the arrival burst it batches is known to be over.
///
/// Otherwise: a FIRST bake waits only the short window (fast appearance); a
/// re-bake of a published region waits for the LONG quiet window (B3, decision 1432 — the
/// admission trickle arrives spaced wider than the short window, so B2 re-baked cells once
/// per arrival for minutes; 1431's cost map priced it), with the age cap so a never-quiet
/// region still consolidates.
fn bake_due(state: &super::GxCell, published: bool, frame: u32, now: bool) -> bool {
    if !state.dirty {
        return false;
    }
    if now {
        return true;
    }
    let window = if published {
        REBAKE_FRAMES
    } else {
        IDLE_FRAMES
    };
    frame.wrapping_sub(state.last_change) >= window
        || frame.wrapping_sub(state.dirty_since) >= MAX_DIRTY_FRAMES
}

/// B2 (1431): the bake reassigns item indices — remap each fader placement's kill targets
/// to the post-sort order and mark the published bitmap stale. The scan runs right after
/// the flush in the same chain, so the rebuilt bitmap rides the SAME frame's publish: a
/// cell never draws with bits from a previous bake's indices.
fn remap_fader_items(state: &mut super::GxCell) {
    if state.faders.is_empty() {
        return;
    }
    for f in state.faders.values_mut() {
        f.items.clear();
    }
    for (idx, item) in state.items.iter().enumerate() {
        if let Some(uid) = item.fader {
            if let Some(f) = state.faders.get_mut(&uid) {
                f.items
                    .push(u16::try_from(idx).expect("gx cell under u16 items"));
            }
        }
    }
    state.bits_stale = true;
}

/// Build one cell's (or WMO region's) mesh (recentred — 0974's precision split; the node
/// pushes the origin) and its per-item draw list, plus per-GROUP bounds for a WMO region
/// (the cull's per-group admission tests them; empty for cells).
fn bake_cell(items: &[GxItem], meshes: &mut Assets<Mesh>) -> render::GxCellDraw {
    use bevy::math::DVec3;
    // World positions accumulate in f64 and round to f32 only AFTER recentring. Baking
    // `transform_point` in f32 quantized every vertex to the ULP of its ±9,000-yd world
    // coordinate (~0.0005 yd) BEFORE the recentre could save it — 0974's exact defect,
    // reintroduced at bake time. Indoors a pixel spans ~0.001 yd, so that was a half-pixel
    // shift of every interpolant: the inn A/B's ±1/255 film over 800k pixels, while the
    // tram — a map whose placements sit near the origin — matched at exactly 0. In f64 the
    // 9,000-magnitude intermediate is exact to ~1e-12 and the subtraction hands the f32
    // vertex only its SMALL recentred value; what remains against the entity path is the
    // GPU's own rotate-at-local-magnitude rounding, which no bake can undercut.
    let mut positions64: Vec<DVec3> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut words: Vec<u32> = Vec::new();
    let mut anchors: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut draws: Vec<render::GxItemDraw> = Vec::new();
    let (mut mn, mut mx) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    // Per-group world bounds (WMO regions): group index → (min, max).
    let mut group_bounds: Vec<(u16, DVec3, DVec3)> = Vec::new();
    for (item_idx, item) in items.iter().enumerate() {
        let sub = &item.geometry;
        let base = u32::try_from(positions64.len()).expect("gx cell under u32 vertices");
        // Low 16 bits: the ITEM index — the render side's record table resolves it to a
        // texture-array layer (+ the WMO per-item record; dims/format are render-side
        // knowledge — see render.rs).
        let has_vc = sub.vertex_colors.len() == sub.positions.len();
        let word_flags = u32::try_from(item_idx).expect("gx cell under u16 items")
            | (u32::from(item.wrap_x) * WORD_WRAP_X)
            | (u32::from(item.wrap_y) * WORD_WRAP_Y)
            | (u32::from(item.unlit) * WORD_UNLIT)
            | (u32::from(item.fog_off) * WORD_FOG_OFF)
            | (u32::from(item.shade_lit) * WORD_SHADE_LIT)
            | (u32::from(item.matte) * WORD_MATTE)
            | (u32::from(item.texture.is_some()) * WORD_TEXTURED)
            | (u32::from(has_vc) * WORD_HAS_VC)
            // An interior PROP is WORD_INTERIOR with WORD_WMO clear (B4) — the entity
            // shader's own `interior_prop = flags.z && !flags.x` split. A slot-less prop
            // (exterior, or probe-table overflow) keeps the exterior law, like the entity
            // path's fallback.
            | item.prop.as_ref().map_or(0, |p| {
                u32::from(p.slot.is_some()) * WORD_INTERIOR
            })
            | item.wmo.as_ref().map_or(0, |w| {
                WORD_WMO
                    | (u32::from(w.interior) * WORD_INTERIOR)
                    | (u32::from(w.class_lane == 1) * WORD_CLASS_INT)
                    | (u32::from(w.class_lane == 2) * WORD_CLASS_TRANS)
                    | (u32::from(w.window) * WORD_WINDOW)
            });
        let anchor = item.transform.translation;
        let rot = item.transform.rotation;
        // Position/normal baking mirrors the placement transform's algebra (scale, rotate,
        // translate) with the position leg in f64 (see the header note); the normal rides
        // rotation alone (uniform placement scale preserves direction), authored zero
        // normals kept zero for the shader's `wow_normalize` DC collapse (1268).
        let rot64 = rot.as_dquat();
        let scale64 = item.transform.scale.as_dvec3();
        let t64 = item.transform.translation.as_dvec3();
        let has_normals = sub.normals.len() == sub.positions.len();
        let (mut gmn, mut gmx) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
        for (vi, p) in sub.positions.iter().enumerate() {
            let w = rot64 * (wow_to_bevy(*p).as_dvec3() * scale64) + t64;
            mn = mn.min(w);
            mx = mx.max(w);
            gmn = gmn.min(w);
            gmx = gmx.max(w);
            positions64.push(w);
            let n = if has_normals {
                (rot * wow_to_bevy(sub.normals[vi])).normalize_or_zero()
            } else {
                Vec3::Y
            };
            normals.push(n.to_array());
            uvs.push(*sub.uvs.get(vi).unwrap_or(&[0.0, 0.0]));
            // MOCV / the baked constant tint, exactly as the entity mesh carries
            // ATTRIBUTE_COLOR (`model.rs` inserts `vertex_colors` raw); white where the batch
            // authors none — bit-identical through every lane by the WORD_HAS_VC contract.
            colors.push(if has_vc {
                sub.vertex_colors[vi]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            });
            words.push(word_flags); // layer bits 0..16 resolve render-side (dims live there)
            anchors.push(anchor.to_array());
        }
        // The selection grain: a WMO item's GROUP, a prop item's referrer SET (B4) — one
        // field, because the node's range selection is the same mechanism for both (the
        // per-frame bit vector just means "group visible" on one map and "set admitted" on
        // the other).
        let sel_group = item
            .wmo
            .as_ref()
            .map(|w| w.group)
            .or(item.prop.as_ref().map(|p| p.set));
        if let Some(g) = sel_group {
            match group_bounds.iter_mut().find(|(gb, _, _)| *gb == g) {
                Some((_, bmn, bmx)) => {
                    *bmn = bmn.min(gmn);
                    *bmx = bmx.max(gmx);
                }
                None => group_bounds.push((g, gmn, gmx)),
            }
        }
        let start = u32::try_from(indices.len()).expect("gx cell under u32 indices");
        indices.extend(sub.indices.iter().map(|i| base + i));
        draws.push(render::GxItemDraw {
            index_range: start..u32::try_from(indices.len()).unwrap(),
            texture: item.texture,
            cutout: item.cutout,
            two_sided: item.two_sided,
            vertex_range: base..u32::try_from(positions64.len()).unwrap(),
            group: sel_group,
            order: item.wmo.as_ref().map_or(0, |w| w.order),
            sidn: item.wmo.as_ref().map_or([0; 3], |w| w.sidn),
            slot: item.prop.as_ref().and_then(|p| p.slot).unwrap_or(0),
        });
    }
    // Recentre for clip-space precision: the shader reconstructs world = v + origin.
    // The origin is the f32 ROUNDING of the f64 centre, and the subtraction runs in f64
    // against that exact value — the vertex absorbs every bit the origin's rounding lost, so
    // the only f32 quantization anywhere is of the SMALL recentred coordinate.
    let center = (mn + mx) * 0.5;
    let origin = center.as_vec3();
    let origin64 = origin.as_dvec3();
    let positions: Vec<[f32; 3]> = positions64
        .iter()
        .map(|w| (*w - origin64).as_vec3().to_array())
        .collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_attribute(ATTRIBUTE_GX_WORD, words);
    mesh.insert_attribute(ATTRIBUTE_GX_ANCHOR, anchors);
    mesh.insert_indices(Indices::U32(indices));
    // The kill bitmap opens all-zero (nothing killed); a cell with faders is marked
    // `bits_stale` by the flush, so the scan overwrites this before the same frame's publish.
    let killed = vec![0u64; draws.len().div_ceil(64)];
    render::GxCellDraw {
        mesh: meshes.add(mesh),
        origin,
        aabb: Aabb::from_min_max((mn - origin64).as_vec3(), (mx - origin64).as_vec3()),
        draws,
        groups: group_bounds
            .into_iter()
            .map(|(g, bmn, bmx)| {
                (
                    g,
                    Aabb::from_min_max((bmn - origin64).as_vec3(), (bmx - origin64).as_vec3()),
                )
            })
            .collect(),
        sets: Vec::new(), // prop regions: the flush fills it from the collector's dedup list
        killed,
        killed_rev: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{batch, batch_of, object, tri};
    use super::super::GxWmoBatch;
    use super::*;
    use crate::model_render::ShadeSel;
    use benilla_formats::{ModelBlend, RenderSubmesh, WmoBatchClass};
    use std::sync::Arc;

    /// **The census reports once per burst, and cannot be starved into silence.** It used to
    /// print on any count change from a system that runs every frame, so a Goldshire login
    /// emitted ten near-identical INFO lines climbing 8 → 1630 — a residency count that read as
    /// a fault. Three things are pinned here: a moving count stays quiet, a settled one prints
    /// exactly once, and a count that never settles still reports on the ceiling.
    #[test]
    fn the_decline_census_prints_once_a_burst_settles() {
        let mut gx = StaticGx::default();
        let mut frame = 0u32;
        let tick = |gx: &mut StaticGx, frame: &mut u32| {
            *frame += 1;
            census_declines(gx, *frame);
        };
        // A burst: the count moves every frame, and nothing is reported while it does.
        for _ in 0..10 {
            gx.declined[3] += 1;
            tick(&mut gx, &mut frame);
        }
        assert_eq!(gx.declined_logged, [0; 5], "a moving count stays quiet");
        // It settles: after the window, the total lands once and stays landed.
        for _ in 0..CENSUS_SETTLE_FRAMES {
            tick(&mut gx, &mut frame);
        }
        assert_eq!(gx.declined_logged[3], 10, "the settled total reports");
        let logged = gx.declined_logged;
        for _ in 0..CENSUS_SETTLE_FRAMES * 2 {
            tick(&mut gx, &mut frame);
        }
        assert_eq!(gx.declined_logged, logged, "…once, not every frame after");
        // A stream that never goes quiet reports on the ceiling instead of never.
        for _ in 0..CENSUS_MAX_HOLD_FRAMES + 1 {
            gx.declined[3] += 1;
            tick(&mut gx, &mut frame);
        }
        assert!(
            gx.declined_logged[3] > 10,
            "a count that never settles still reports"
        );
    }

    /// The census counts THIS world's population: a map change resets it, or the line reports the
    /// sum of two worlds and reads as a leak (`static_merge::reset`'s own rationale).
    #[test]
    fn a_map_change_resets_the_census() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        assert!(gx.divert(batch(&g, Vec3::ZERO, None, ModelBlend::Opaque)));
        gx.tally_prop_declined(false);
        assert_eq!((gx.accepted, gx.declined[3]), (1, 1));
        gx.clear();
        assert_eq!((gx.accepted, gx.declined[3]), (0, 0), "a fresh world");
    }

    /// The bake cadence (B3): first bakes wait only the short window, re-bakes
    /// of a published region wait for the long one, and the age cap consolidates a
    /// never-quiet region regardless.
    #[test]
    fn the_bake_waits_short_first_and_long_after() {
        let mut state = super::super::GxCell::default();
        assert!(
            !bake_due(&state, false, 100, false),
            "clean cells are never due"
        );
        state.dirty = true;
        state.dirty_since = 0;
        state.last_change = 0;
        assert!(!bake_due(&state, false, IDLE_FRAMES - 1, false));
        assert!(
            bake_due(&state, false, IDLE_FRAMES, false),
            "first bake: short window"
        );
        assert!(
            !bake_due(&state, true, IDLE_FRAMES, false),
            "published: short is not enough"
        );
        assert!(!bake_due(&state, true, REBAKE_FRAMES - 1, false));
        assert!(
            bake_due(&state, true, REBAKE_FRAMES, false),
            "published: long window"
        );
        // The reveal gate's request overrides every window: a load that has finished arriving
        // publishes what it holds instead of waiting out the quiet timer.
        state.last_change = IDLE_FRAMES;
        assert!(
            !bake_due(&state, false, IDLE_FRAMES, false),
            "still quiet-timing"
        );
        assert!(
            bake_due(&state, false, IDLE_FRAMES, true),
            "flush_now: due now"
        );
        assert!(bake_due(&state, true, IDLE_FRAMES, true), "…published too");
        state.dirty = false;
        assert!(
            !bake_due(&state, false, IDLE_FRAMES, true),
            "flush_now still bakes nothing that is not dirty"
        );
        state.dirty = true;
        state.last_change = 0;
        // A cell that keeps changing (never quiet) still consolidates at the age cap.
        state.last_change = MAX_DIRTY_FRAMES;
        assert!(bake_due(&state, true, MAX_DIRTY_FRAMES, false));
    }

    /// A quiet cell bakes: one recentred mesh whose draws sort by (bucket, texture), each
    /// item's word carrying its index + flags, index ranges contiguous over one index buffer.
    #[test]
    fn a_quiet_cell_bakes_sorted_contiguous_draws() {
        let mut gx = StaticGx::default();
        let g = tri([10.0, 0.0, 10.0]);
        // A cutout item pushed FIRST must sort after the two opaque items.
        let mut cut = batch(&g, Vec3::new(5.0, 0.0, 5.0), None, ModelBlend::AlphaTest);
        cut.unlit = true;
        assert!(gx.divert(cut));
        assert!(gx.divert(batch(
            &g,
            Vec3::new(6.0, 0.0, 6.0),
            None,
            ModelBlend::Opaque
        )));
        assert!(gx.divert(batch(
            &g,
            Vec3::new(7.0, 0.0, 7.0),
            None,
            ModelBlend::Opaque
        )));
        let mut meshes = Assets::<Mesh>::default();
        let state = gx.cells.get_mut(&(0, 0)).unwrap();
        state
            .items
            .sort_by_key(|i| ((u8::from(i.cutout) << 1) | u8::from(i.two_sided), i.texture));
        let baked = bake_cell(&state.items, &mut meshes);
        assert_eq!(baked.draws.len(), 3);
        assert!(!baked.draws[0].cutout && !baked.draws[1].cutout);
        assert!(baked.draws[2].cutout, "the cutout item sorted last");
        // Contiguity: each draw's range starts where the previous ended.
        assert_eq!(baked.draws[0].index_range, 0..3);
        assert_eq!(baked.draws[1].index_range, 3..6);
        assert_eq!(baked.draws[2].index_range, 6..9);
        let mesh = meshes.get(&baked.mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Uint32(words)) =
            mesh.attribute(ATTRIBUTE_GX_WORD)
        else {
            panic!("gx word attribute missing")
        };
        // Item indices ride the low bits in bake order; the cutout item (baked last, index 2)
        // carries its UNLIT flag.
        assert_eq!(words[0] & 0xffff, 0);
        assert_eq!(words[3] & 0xffff, 1);
        assert_eq!(words[6] & 0xffff, 2);
        assert_ne!(words[6] & WORD_UNLIT, 0);
        assert_eq!(words[0] & WORD_UNLIT, 0);
        // Recentring (0974's split): the mesh-local bound centres exactly on zero by
        // construction, and the origin carries the world offset (nonzero here — the items
        // stand away from the world origin).
        assert!(Vec3::from(baked.aabb.center).length() < 1e-4);
        assert!(baked.origin.length() > 1.0);
    }

    /// B2: after the bake's sort scatters a placement's batches through the item order, the
    /// remap hands each fader exactly its own post-sort indices — the kill bits land on the
    /// placement that crossed the band, never a neighbour.
    #[test]
    fn the_remap_names_each_faders_post_sort_items() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let seed = || super::super::GxFadeSeed {
            radius: 0.4,
            local_center: Vec3::ZERO,
            stat_mesh: Handle::default(),
            aabb: None,
            cutout: Handle::default(),
            blend: Handle::default(),
        };
        // The exile unit is the PLACEMENT — its identity, shared by every batch (1534).
        let (p1, p2) = (object(1), object(2));
        // Placement 1: a cutout batch (sorts LAST) + an opaque batch (sorts first)…
        let mut b = batch_of(
            &p1,
            &g,
            Vec3::new(1.0, 0.0, 1.0),
            None,
            ModelBlend::AlphaTest,
        );
        b.fade = Some(seed());
        assert!(gx.divert(b));
        let mut b = batch_of(&p1, &g, Vec3::new(1.0, 0.0, 1.0), None, ModelBlend::Opaque);
        b.fade = Some(seed());
        assert!(gx.divert(b));
        // …a never-fade opaque batch between them, and placement 2's opaque batch.
        assert!(gx.divert(batch(
            &g,
            Vec3::new(2.0, 0.0, 2.0),
            None,
            ModelBlend::Opaque
        )));
        let mut b = batch_of(&p2, &g, Vec3::new(3.0, 0.0, 3.0), None, ModelBlend::Opaque);
        b.fade = Some(seed());
        assert!(gx.divert(b));
        let state = gx.cells.get_mut(&(0, 0)).unwrap();
        state
            .items
            .sort_by_key(|i| ((u8::from(i.cutout) << 1) | u8::from(i.two_sided), i.texture));
        remap_fader_items(state);
        assert!(state.bits_stale, "the same-frame scan rebuilds the bitmap");
        // The cutout batch sorted last (index 3); placement 1 owns one opaque slot + it.
        let f1 = &state.faders[&1].items;
        let f2 = &state.faders[&2].items;
        assert_eq!(f1.len(), 2);
        assert!(f1.contains(&3), "the cutout batch sorted to the tail");
        assert_eq!(f2.len(), 1);
        assert!(!f2.iter().any(|i| f1.contains(i)), "no shared kill targets");
        // The never-fade item belongs to nobody.
        let claimed: usize = f1.len() + f2.len();
        assert_eq!(state.items.len() - claimed, 1);
    }

    /// A WMO batch diverts into a region keyed by its INSTANCE entity (never a cell), the
    /// shade refusal does not apply to it (the entity path passes `Matte` for every WMO
    /// batch), and the baked region carries the slice-2 facts: group-homogeneous draws in
    /// (bucket, texture, group) order, per-group bounds, the WMO word bits, and the per-item
    /// order/SIDN records.
    #[test]
    fn a_wmo_batch_diverts_by_instance_and_bakes_group_ranges() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let instance = Entity::PLACEHOLDER;
        let wmo = |group: u16, class: Option<WmoBatchClass>, interior: bool| GxWmoBatch {
            instance,
            group,
            interior,
            class,
            sidn: Some([10, 20, 30]),
            window: true,
            batch_order: group + 1,
        };
        // An INT batch of group 2, pushed FIRST — Matte shade must NOT refuse it…
        let mut b = batch(&g, Vec3::new(1.0, 0.0, 1.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.wmo = Some(wmo(2, Some(WmoBatchClass::Int), true));
        assert!(gx.divert(b));
        // …a TRANS batch of group 1 (same bucket/texture — the sort must bring it first)…
        let mut b = batch(&g, Vec3::new(2.0, 0.0, 2.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.wmo = Some(wmo(1, Some(WmoBatchClass::Trans), true));
        assert!(gx.divert(b));
        // …and an exterior-law batch of group 2 again (fuses with the first after the sort).
        let mut b = batch(&g, Vec3::new(3.0, 0.0, 3.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.wmo = Some(wmo(2, None, false));
        assert!(gx.divert(b));
        assert!(gx.cells.is_empty(), "WMO items never land in cells");
        let state = gx.wmos.get_mut(&instance).expect("the instance's region");
        state.items.sort_by_key(|i| {
            (
                (u8::from(i.cutout) << 1) | u8::from(i.two_sided),
                i.texture,
                i.wmo.as_ref().map_or(0, |w| w.group),
            )
        });
        let mut meshes = Assets::<Mesh>::default();
        let baked = bake_cell(&state.items, &mut meshes);
        // Group 1 sorted ahead of group 2; ranges contiguous over one index buffer.
        assert_eq!(
            baked.draws.iter().map(|d| d.group).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(2)]
        );
        assert_eq!(baked.draws[0].index_range, 0..3);
        assert_eq!(baked.draws[2].index_range, 6..9);
        // Per-group bounds for the cull's admission walk.
        let mut groups: Vec<u16> = baked.groups.iter().map(|(g, _)| *g).collect();
        groups.sort_unstable();
        assert_eq!(groups, vec![1, 2]);
        // The per-item records: authored order + SIDN ride the draw.
        assert_eq!(baked.draws[0].order, 2); // group 1's batch_order = group + 1
        assert_eq!(baked.draws[0].sidn, [10, 20, 30]);
        // The word bits: WMO everywhere; TRANS/INT class lanes; WINDOW; no vertex colours.
        let mesh = meshes.get(&baked.mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Uint32(words)) =
            mesh.attribute(ATTRIBUTE_GX_WORD)
        else {
            panic!("gx word attribute missing")
        };
        let w_trans = words[baked.draws[0].vertex_range.start as usize];
        let w_int = words[baked.draws[1].vertex_range.start as usize];
        let w_ext = words[baked.draws[2].vertex_range.start as usize];
        for w in [w_trans, w_int, w_ext] {
            assert_ne!(w & WORD_WMO, 0);
            assert_ne!(w & WORD_WINDOW, 0);
            assert_eq!(w & WORD_HAS_VC, 0, "the fixture authors no colours");
        }
        assert_ne!(w_trans & WORD_CLASS_TRANS, 0);
        assert_ne!(w_int & WORD_CLASS_INT, 0);
        assert_eq!(w_ext & (WORD_CLASS_INT | WORD_CLASS_TRANS), 0);
        assert_ne!(w_trans & WORD_INTERIOR, 0);
        assert_eq!(w_ext & WORD_INTERIOR, 0);
        // White default colours where none are authored (the bit-identity contract).
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("gx colour attribute missing")
        };
        assert_eq!(colors[0], [1.0, 1.0, 1.0, 1.0]);
        // clear() empties the WMO side too.
        gx.clear();
        assert!(gx.wmos.is_empty());
    }

    /// A prop region bakes with the referrer SET as the selection grain (B4): draws sort
    /// (bucket, texture, set) and carry the set index in `group`, per-set bounds land in
    /// `groups`, the interior slot rides the draw, and the word carries
    /// INTERIOR-without-WMO for slotted items and MATTE for the exterior family.
    #[test]
    fn a_prop_region_bakes_set_ranges_and_slots() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let instance = Entity::PLACEHOLDER;
        let rooms_a: Arc<[u16]> = Arc::from([3u16].as_slice());
        let rooms_b: Arc<[u16]> = Arc::from([5u16, 8].as_slice());
        let mk = |groups: &Arc<[u16]>, slot| super::super::GxPropBatch {
            instance,
            groups: Arc::clone(groups),
            slot,
        };
        // Set B pushed FIRST — the sort must bring set A (index 1? no: dedup order is
        // arrival order, so B=0, A=1) ranges together and set-homogeneous.
        let mut b = batch(&g, Vec3::new(1.0, 0.0, 1.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.prop = Some(mk(&rooms_b, Some(42)));
        assert!(gx.divert(b));
        let mut b = batch(&g, Vec3::new(2.0, 0.0, 2.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.prop = Some(mk(&rooms_a, None));
        assert!(gx.divert(b));
        let mut b = batch(&g, Vec3::new(3.0, 0.0, 3.0), None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.prop = Some(mk(&rooms_b, Some(43)));
        assert!(gx.divert(b));
        let state = gx.props.get_mut(&instance).expect("the instance's region");
        state.items.sort_by_key(|i| {
            (
                (u8::from(i.cutout) << 1) | u8::from(i.two_sided),
                i.texture,
                i.prop.as_ref().map_or(0, |p| p.set),
            )
        });
        let mut meshes = Assets::<Mesh>::default();
        let baked = bake_cell(&state.items, &mut meshes);
        // Set 0 (rooms_b)'s two items sit adjacent; set 1 (rooms_a) follows.
        assert_eq!(
            baked.draws.iter().map(|d| d.group).collect::<Vec<_>>(),
            vec![Some(0), Some(0), Some(1)]
        );
        assert_eq!(
            baked.draws.iter().map(|d| d.slot).collect::<Vec<_>>(),
            vec![42, 43, 0]
        );
        // Per-set bounds for the admission walk.
        let mut sets: Vec<u16> = baked.groups.iter().map(|(s, _)| *s).collect();
        sets.sort_unstable();
        assert_eq!(sets, vec![0, 1]);
        // The word: a slotted item is INTERIOR without WMO; a slot-less Matte prop keeps
        // the exterior law with the MATTE family bit.
        let mesh = meshes.get(&baked.mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Uint32(words)) =
            mesh.attribute(ATTRIBUTE_GX_WORD)
        else {
            panic!("gx word attribute missing")
        };
        let w_int = words[baked.draws[0].vertex_range.start as usize];
        let w_ext = words[baked.draws[2].vertex_range.start as usize];
        assert_ne!(w_int & WORD_INTERIOR, 0);
        assert_eq!(w_int & WORD_WMO, 0, "a prop item never takes the WMO lane");
        assert_eq!(w_ext & WORD_INTERIOR, 0);
        assert_ne!(w_ext & WORD_MATTE, 0);
    }

    /// Authored vertex colours bake RAW into ATTRIBUTE_COLOR and set the HAS_VC bit — the
    /// entity mesh's exact carriage (`model.rs` inserts `vertex_colors` untransformed), which
    /// is both MOCV's lane and the fix for the slice-1 gap where a doodad's baked constant
    /// tint was silently dropped.
    #[test]
    fn authored_vertex_colours_bake_raw_and_set_the_bit() {
        let mut gx = StaticGx::default();
        let g = Arc::new(RenderSubmesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
            vertex_colors: vec![[0.25, 0.5, 0.75, 0.5]; 3],
            ..Default::default()
        });
        assert!(gx.divert(batch(&g, Vec3::ZERO, None, ModelBlend::Opaque)));
        let mut meshes = Assets::<Mesh>::default();
        let baked = bake_cell(&gx.cells[&(0, 0)].items, &mut meshes);
        let mesh = meshes.get(&baked.mesh).unwrap();
        let Some(bevy::mesh::VertexAttributeValues::Uint32(words)) =
            mesh.attribute(ATTRIBUTE_GX_WORD)
        else {
            panic!("gx word attribute missing")
        };
        assert_ne!(words[0] & WORD_HAS_VC, 0);
        assert_eq!(words[0] & WORD_WMO, 0, "a cell item takes no WMO lane");
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("gx colour attribute missing")
        };
        assert_eq!(colors[1], [0.25, 0.5, 0.75, 0.5]);
    }
}
