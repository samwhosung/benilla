//! Integration test against the real client at `<repo>/WoW/Data` (gitignored).
//!
//! Skips (passes) when the client isn't present, so CI without assets stays green.
//! Document: place a legally-owned 1.12.1 client's `Data` dir at the repo root `WoW/`.

use benilla_formats::{open_chain, Chain};

#[test]
fn reads_spell_dbc_from_vanilla_chain() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Spell.dbc lives in patch.MPQ; reading by name must resolve through the chain.
    let bytes = chain
        .read_file("DBFilesClient/Spell.dbc")
        .expect("read DBFilesClient/Spell.dbc");

    assert_eq!(&bytes[..4], b"WDBC", "DBC files start with the WDBC magic");
    assert!(
        bytes.len() > 1_000_000,
        "Spell.dbc should be sizable, got {} bytes",
        bytes.len()
    );
}

#[test]
fn resolves_and_reads_across_archive_types() {
    let data = benilla_formats::wow_data_or_skip!();

    // One `Chain` (the `&self` AssetReader backend and the `&mut` streaming loaders are now the same
    // type) must resolve and read across archive kinds — DBC (dbc/patch.MPQ), a UI BLP, and a
    // base-archive M2 with no per-archive listfile (resolved by name hash). Non-empty reads pin
    // cross-archive resolution and the whole-file (no-PTCH) invariant for these files.
    let chain = open_chain(&data).expect("open vanilla patch chain");

    for path in [
        "DBFilesClient/Spell.dbc",
        "DBFilesClient/TaxiNodes.dbc",
        "Interface/Icons/Spell_Holy_ArcaneIntellect.blp",
        // Physical .m2 in a listfile-less base archive (references use .mdx; the .mdx→.m2 remap is a
        // loader concern, so the reader gets the on-disk path) — proves base-archive resolution.
        "Creature\\Kobold\\Kobold.m2",
    ] {
        assert!(chain.contains(path), "chain should resolve {path}");
        let bytes = chain
            .read(path)
            .unwrap_or_else(|e| panic!("chain read {path}: {e:#}"));
        assert!(!bytes.is_empty(), "{path} read empty");
    }
}

#[test]
fn culls_constant_zero_alpha_m2_batches() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // OrgrimmarFloatingEmbers' only render batch is an 84-yd box with a transparency-weight track of
    // 0.0 — a particle-emitter anchor the real client never draws (it culls batches at alpha ≤ 0,
    // verified). It must be culled, leaving the model with no render geometry (its 3 emitters remain).
    let embers = benilla_formats::load_m2_mesh(
        &mut chain,
        "world/kalimdor/orgrimmar/passivedoodads/orgrimmarbonfire/orgrimmarfloatingembers.m2",
    )
    .expect("mesh embers");
    assert!(
        embers.is_empty(),
        "the weight-0 emitter box must be culled, got {} batches",
        embers.len()
    );

    // Control — a normal lamppost keeps every batch (body + glow cards), proving the zero-alpha cull
    // doesn't over-cull visible geometry.
    let lamppost = benilla_formats::load_m2_mesh(
        &mut chain,
        "world/azeroth/elwynn/passivedoodads/lamppost/lamppost.m2",
    )
    .expect("mesh lamppost");
    // Five batches: body + glass + two billboard cards (the spherical additive glow and the cylindrical
    // top-flame) split out from their mixed batch onto their own bones, so each faces the camera about
    // its own pivot. The point here is the alpha-cull leaves every visible batch intact.
    assert_eq!(lamppost.len(), 5, "a normal prop keeps all its batches");
}

#[test]
fn trigger_creature_models_carry_no_render_geometry() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // How an invisible **trigger creature** hides in the real client (bug B13): its
    // model draws nothing. No unit flag, no DBC column — a swept census of all 411 shipped
    // `CreatureModelData` paths found exactly these three, by two different routes:
    //
    //   InvisibleStalker        — `nVertices == 0`: no geometry authored at all (80 bones,
    //                             135 animations, and not one vertex).
    //   InvisibleStalkerNoName  — one 4-vertex batch whose transparency track is a constant
    //                             `0.000`, so the verified zero-alpha cull takes it.
    //   Creature_SpellPortal    — `nVertices == 0`, like the stalker.
    //
    // Between them they back 8 `CreatureDisplayInfo` rows and 154 vmangos creature templates — the
    // Scarab Wall's "Anachronos Quest Trigger Invisible" and Yojamba Isle's "Zandalarian Event
    // Generator" among them. If any of these ever comes back non-empty, the attach path stops
    // being able to tell "this model draws nothing" from "we have no model" and the debug cube
    // returns as a black slab over all 154.
    for path in [
        "creature/invisiblestalker/invisiblestalker.m2",
        "creature/invisiblestalker/invisiblestalkernoname.m2",
        "creature/spells/creature_spellportal.m2",
    ] {
        let mesh = benilla_formats::load_m2_mesh(&mut chain, path)
            .unwrap_or_else(|e| panic!("mesh {path}: {e:#}"));
        assert!(
            mesh.is_empty(),
            "{path} is a trigger creature's model and must build no render geometry, got {} batches",
            mesh.len()
        );
    }

    // Control — an ordinary creature on the same path keeps its geometry, so "empty" is a property
    // of these three models and not of the creature lane.
    let chicken = benilla_formats::load_m2_mesh(&mut chain, "creature/chicken/chicken.m2")
        .expect("mesh chicken");
    assert!(
        !chicken.is_empty(),
        "an ordinary creature model keeps its batches"
    );
}

#[test]
fn the_trigger_creature_still_carries_the_attachments_its_visible_self_rides() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Decision 1656: a trigger creature draws nothing *and is not therefore empty*. The
    // Naxxramas weapon mobs (`Unholy Axe`/`Swords`/`Staff`, creature_template 16194/16215/16216,
    // all display 15294) ARE an `InvisibleStalker` body — the model above, `nVertices == 0` —
    // whose entire visible self hangs off its attachment points: the weapon on HandRight, the
    // name plate on PlayerName. If either of these ever stops resolving, the mobs go back to
    // being a bare name on the floor, and nothing in the render lane would say so.
    let bytes = chain
        .read_file("Creature\\InvisibleStalker\\InvisibleStalker.m2")
        .expect("read InvisibleStalker.m2");
    let attach = benilla_formats::parse_m2_attachments(&bytes).expect("parse attachments");
    let at = |id: u16| attach.iter().find(|a| a.id == id).copied();

    let hand = at(1).expect("HandRight (1) — the drawn mainhand, where the axe hangs");
    assert!(
        hand.position[2] > 0.5,
        "the hand attachment sits on the body, not at the origin: {:?}",
        hand.position
    );
    let name = at(18).expect("PlayerName (18) — `0x608640`'s overhead anchor");
    assert!(
        name.position[2] > 2.0,
        "the name anchor is overhead, not at the feet: {:?}",
        name.position
    );

    // And the bounds really are degenerate, which is why the plate CANNOT come from the fallback
    // (`feet + scale × bbox_z × 1.25` = feet) and must come from the attachment above.
    let bounds = benilla_formats::load_m2_bounds(
        &mut chain,
        "Creature\\InvisibleStalker\\InvisibleStalker.mdx",
    )
    .expect("bounds");
    assert_eq!(
        bounds.bbox_max[2] - bounds.bbox_min[2],
        0.0,
        "a vertex-less model has no vertex box — the overhead fallback is feet-height here"
    );
}

#[test]
fn suppresses_white1_invisible_trap_placeholder() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // The `SpellObject_InvisibleTrap` placeholder (GameObject displayId 1287 — Fire-Festival fury zones,
    // rallying-cry triggers, …): a flat opaque quad textured with the engine utility-white WHITE1.BLP.
    // The real client draws it at per-instance alpha ≈0 (invisible); benilla recognises the placeholder
    // and drops its geometry. It must reduce to zero render batches.
    let trap = benilla_formats::load_m2_mesh(
        &mut chain,
        "world/generic/passivedoodads/traps/spellobject_invisibletrap.m2",
    )
    .expect("mesh trap");
    assert!(
        trap.is_empty(),
        "the WHITE1 invisible-trap placeholder must be suppressed, got {} batches",
        trap.len()
    );

    // Control — a *visible* flat decal that shares the trap's degenerate render box but carries a real
    // texture + AlphaTest (an orc bedroll). It must NOT be suppressed: the fingerprint is opaque+WHITE1,
    // not flatness, so genuine ground decals are kept.
    let mat = benilla_formats::load_m2_mesh(
        &mut chain,
        "world/azeroth/burningsteppes/passivedoodads/orcsleepmats/orcsleepmat01.m2",
    )
    .expect("mesh orc sleep mat");
    assert!(
        !mat.is_empty(),
        "a visible flat decal (real texture, AlphaTest) must be kept, not suppressed"
    );
}

#[test]
fn decodes_a_blp_icon_to_png() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("Interface/Icons/Spell_Holy_ArcaneIntellect.blp")
        .expect("read spell icon BLP");

    // Keyed by pid, like every other temp path in the workspace: several worktrees run
    // `cargo test --workspace` concurrently, and a FIXED shared name means one run's
    // cleanup deletes the file another is about to read. That is not hypothetical — the
    // taxi twin below reddened a land gate exactly that way.
    let out = std::env::temp_dir().join(format!("benilla-fmt-icon-{}.png", std::process::id()));
    let (w, h) = benilla_formats::blp_to_png(&bytes, &out).expect("decode BLP -> PNG");

    assert_eq!((w, h), (64, 64), "spell icons are 64x64");
    let png = std::fs::read(&out).expect("read written PNG");
    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10], "PNG magic");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn dumps_taxinodes_to_csv() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("DBFilesClient/TaxiNodes.dbc")
        .expect("read TaxiNodes.dbc");

    let out = std::env::temp_dir().join(format!("benilla-fmt-taxi-{}.csv", std::process::id()));
    let (records, fields) =
        benilla_formats::dbc_to_csv(&bytes, "TaxiNodes.dbc", &out).expect("dbc->csv");

    assert_eq!((records, fields), (85, 16), "vanilla TaxiNodes.dbc shape");
    let csv = std::fs::read_to_string(&out).expect("read csv");
    assert!(
        csv.lines()
            .next()
            .unwrap()
            .starts_with("ID,MapID,X,Y,Z,Name"),
        "schema-derived header"
    );
    assert!(csv.contains("Stormwind"), "expected a known flight node");
    assert_eq!(csv.lines().count(), 86, "85 records + 1 header row");
    let _ = std::fs::remove_file(&out);
}

#[test]
fn meshes_an_elwynn_terrain_tile() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let (tx, ty) = benilla_formats::find_tile_near(&mut chain, "Azeroth", -8840.56, 489.7)
        .expect("find an Elwynn tile");
    let tile =
        benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty).expect("mesh the tile");

    assert!(!tile.chunks.is_empty(), "tile should have terrain chunks");
    assert!(
        tile.vertex_count() <= 256 * 145,
        "<= 256 chunks * (81 outer + 64 inner) = 145 verts/chunk (center-fan)"
    );
    // Hilltop sanity (B1): every chunk should carry the 64 inner center verts, and at least one
    // tile-wide cell must have its center vertex deviate by > 1 yd from the corner-mean — that's
    // the authored hilltop bump we used to flatten. Catches a regression to the old 81-vert mesher.
    let mut hilltop_dev = 0.0f32;
    for c in &tile.chunks {
        assert_eq!(c.positions.len(), 145, "chunk = 81 outer + 64 inner verts");
        for r in 0..8usize {
            for col in 0..8usize {
                let tl = c.positions[r * 17 + col][2];
                let tr = c.positions[r * 17 + col + 1][2];
                let bl = c.positions[(r + 1) * 17 + col][2];
                let br = c.positions[(r + 1) * 17 + col + 1][2];
                let ctr = c.positions[r * 17 + 9 + col][2];
                let mean = (tl + tr + bl + br) * 0.25;
                hilltop_dev = hilltop_dev.max((ctr - mean).abs());
            }
        }
    }
    assert!(
        hilltop_dev > 1.0,
        "expected ≥1 cell on this Elwynn tile with center−corner deviation > 1 yd \
         (proves we read the authored MCVT inner heights, not just outer); got max {hilltop_dev:.2}",
    );

    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for chunk in &tile.chunks {
        assert_eq!(chunk.indices.len() % 3, 0, "triangle list per chunk");
        assert_eq!(chunk.positions.len(), chunk.uvs.len(), "one UV per vertex");
        assert!(
            chunk
                .indices
                .iter()
                .all(|&i| (i as usize) < chunk.positions.len()),
            "chunk indices reference valid vertices"
        );
        for p in &chunk.positions {
            for i in 0..3 {
                mn[i] = mn[i].min(p[i]);
                mx[i] = mx[i].max(p[i]);
            }
        }
    }

    // Bounds sanity — guards against the MCNK axis-scramble bug.
    // One tile spans ~533 yds in X and Y; tile 31_48's world X is negative; Z holds plausible
    // terrain heights (a leaked horizontal axis would blow the Z span way past this).
    let span = |i: usize| mx[i] - mn[i];
    assert!(
        (500.0..540.0).contains(&span(0)) && (500.0..540.0).contains(&span(1)),
        "X/Y should each span ~one tile (533 yds); got X={:.1}, Y={:.1}",
        span(0),
        span(1)
    );
    assert!(
        mx[0] < 0.0,
        "tile 31_48 world X is negative; got max X={:.1}",
        mx[0]
    );
    assert!(
        span(2) < 600.0,
        "Z should be terrain heights; got span {:.1}",
        span(2)
    );

    // Texturing data: some chunk should reference a base-layer .blp from MTEX.
    assert!(
        tile.chunks.iter().any(|c| c
            .base_texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().ends_with(".blp"))),
        "expected a base-layer .blp texture on some chunk"
    );

    // Multi-layer splat data (Phase 6): Elwynn blends grass/dirt/road, so some chunk should have
    // >1 layer and a non-trivial packed alpha map.
    let multilayer = tile
        .chunks
        .iter()
        .find(|c| c.layer_textures.len() > 1)
        .expect("some chunk should have multiple texture layers");
    let alpha = multilayer
        .alpha_map
        .as_ref()
        .expect("a multi-layer chunk carries a packed alpha map");
    let size = benilla_formats::ALPHA_MAP_SIZE as usize;
    assert_eq!(alpha.len(), size * size * 4, "alpha map is RGBA size²");
    assert!(
        alpha
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0),
        "a multi-layer chunk's alpha map should have some non-zero blend weight"
    );
    assert!(
        multilayer.layer_textures.len() <= 4,
        "at most 4 terrain layers per chunk"
    );
}

#[test]
fn terrain_chunks_carry_unit_upward_mcnr_normals() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let (tx, ty) = benilla_formats::find_tile_near(&mut chain, "Azeroth", -8840.56, 489.7)
        .expect("find an Elwynn tile");
    let tile =
        benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty).expect("mesh the tile");

    let (mut checked, mut up) = (0usize, 0usize);
    for chunk in &tile.chunks {
        if chunk.normals.is_empty() {
            continue;
        }
        assert_eq!(
            chunk.normals.len(),
            chunk.positions.len(),
            "one MCNR normal per vertex"
        );
        for n in &chunk.normals {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!(
                (0.9..=1.1).contains(&len),
                "decoded normal should be ~unit, got len {len:.3} for {n:?}"
            );
            checked += 1;
            // WoW Z is up; ground normals point outward/up. If the X/Z/Y decode were wrong, "up"
            // would land in a different component and this would collapse — so it pins the axis.
            if n[2] > 0.5 {
                up += 1;
            }
        }
    }
    assert!(checked > 0, "Elwynn tile should carry MCNR normals");
    assert!(
        up * 100 / checked >= 70,
        "most terrain normals should point up (WoW +Z); only {up}/{checked} did"
    );
}

/// Every terrain triangle must wind **CCW seen from above** in WoW space — the invariant that lets
/// the renderer draw terrain single-sided (backface-culled), which is what the real 1.12.1 client
/// does: its terrain-chunk pass never touches `EGxRs 0x14`, so it inherits the device baseline
/// `CULL_FACE = 1`, and the ground is see-through from underneath. Flip a fan's
/// winding and the world would vanish when viewed from *above* instead — a catastrophic, silent
/// regression no other test here would catch, since winding changes nothing about positions,
/// seams, or heights.
///
/// Two forms of the same law:
///
/// 1. **Exact.** Terrain is a heightfield on a fixed XY lattice, so the sign of a triangle's
///    geometric-normal Z is the sign of its XY-projected signed area — independent of the authored
///    heights. (The reference builds its vertices the same way: `0x6b0e50` writes X and Y from the
///    grid indices and only Z from MCVT.) No shipped triangle may violate this.
/// 2. **Near-total.** A coordinate-system-free statement of the same convention: the emitted
///    winding is the one whose right-hand-rule normal agrees with the vertex's own MCNR normal.
///    Worth keeping because it survives a change of frame — but it is *not* exact on shipped data
///    and must not be asserted as if it were: it holds exactly only against flat ground, where
///    MCNR is exactly `(0,0,1)`, and on this Elwynn tile 4 of 196368 triangle-vertex pairs
///    disagree (worst cos −0.48, on a hillside). Those are authored outliers, not a decode fault
///    — a decode fault would be systematic, not 4-in-200k. A genuine winding flip turns ~every
///    pair negative, which the 0.01% ceiling below still catches loudly.
#[test]
fn terrain_fans_wind_ccw_seen_from_above() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let (tx, ty) = benilla_formats::find_tile_near(&mut chain, "Azeroth", -8840.56, 489.7)
        .expect("find an Elwynn tile");
    let tile =
        benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty).expect("mesh the tile");

    let (mut tris, mut min_nz) = (0usize, f32::MAX);
    let (mut pairs, mut disagreeing) = (0usize, 0usize);
    for chunk in &tile.chunks {
        let shading = (chunk.normals.len() == chunk.positions.len()).then_some(&chunk.normals);
        for t in chunk.indices.as_chunks::<3>().0 {
            let (i, j, k) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (a, b, c) = (chunk.positions[i], chunk.positions[j], chunk.positions[k]);
            let (u, v) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            // Form 1 — Z of u×v: positive ⇔ CCW viewed down the +Z (up) axis.
            min_nz = min_nz.min(u[0] * v[1] - u[1] * v[0]);
            // Form 2 — does u×v agree with each of the three authored MCNR normals?
            if let Some(normals) = shading {
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                for vert in [i, j, k] {
                    let s = normals[vert];
                    pairs += 1;
                    if n[0] * s[0] + n[1] * s[1] + n[2] * s[2] <= 0.0 {
                        disagreeing += 1;
                    }
                }
            }
            tris += 1;
        }
    }
    assert!(tris > 1000, "expected a meshed tile, got {tris} triangles");
    assert!(
        min_nz > 0.0,
        "every terrain triangle must face up (CCW from above in WoW space); \
         worst geometric-normal Z was {min_nz} over {tris} triangles"
    );
    assert!(pairs > 0, "Elwynn tile should carry MCNR normals");
    assert!(
        disagreeing * 10_000 <= pairs,
        "the emitted winding must agree with the authored MCNR normals bar a handful of outliers; \
         {disagreeing}/{pairs} disagreed (ceiling 0.01%) — a flipped fan turns ~all of them"
    );
}

#[test]
fn terrain_is_watertight_at_chunk_seams() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let (tx, ty) = benilla_formats::find_tile_near(&mut chain, "Azeroth", -8840.56, 489.7)
        .expect("find an Elwynn tile");
    let tile =
        benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty).expect("mesh the tile");

    // Bucket every vertex by quantized XY (0.25-yd buckets only group truly-coincident verts, since
    // the grid spacing is ~4.17 yd). A vertex shared by adjacent chunks must come out *bit-identical*
    // in X/Y after lattice snapping — otherwise the fractional misalignment reopens the seam.
    use std::collections::HashMap;
    let mut buckets: HashMap<(i64, i64), Vec<[f32; 3]>> = HashMap::new();
    for c in &tile.chunks {
        for p in &c.positions {
            let key = ((p[0] * 4.0).round() as i64, (p[1] * 4.0).round() as i64);
            buckets.entry(key).or_default().push(*p);
        }
    }
    let mut shared = 0usize;
    for ps in buckets.values() {
        if ps.len() < 2 {
            continue;
        }
        shared += 1;
        for w in ps.windows(2) {
            assert_eq!(
                w[0][0], w[1][0],
                "shared-edge X must be bit-identical (no seam)"
            );
            assert_eq!(
                w[0][1], w[1][1],
                "shared-edge Y must be bit-identical (no seam)"
            );
            assert!(
                (w[0][2] - w[1][2]).abs() < 0.01,
                "shared-edge heights should match: {} vs {}",
                w[0][2],
                w[1][2]
            );
        }
    }
    assert!(
        shared > 0,
        "tile should have shared chunk-edge vertices to check"
    );
}

#[test]
fn loads_a_block_of_elwynn_tiles() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let tiles = benilla_formats::load_tiles_around(&mut chain, "Azeroth", -8840.56, 489.7, 1)
        .expect("load tile block");

    assert!(
        tiles.len() > 1,
        "radius 1 around Stormwind should load several contiguous tiles, got {}",
        tiles.len()
    );
    let mut coords: Vec<_> = tiles.iter().map(|(c, _)| *c).collect();
    coords.sort_unstable();
    coords.dedup();
    assert_eq!(coords.len(), tiles.len(), "tiles must be distinct");
}

#[test]
fn reads_and_decodes_a_terrain_texture() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let (tx, ty) = benilla_formats::find_tile_near(&mut chain, "Azeroth", -8840.56, 489.7)
        .expect("find a tile");
    let tile =
        benilla_formats::load_tile_mesh(&mut chain, "Azeroth", tx, ty).expect("mesh the tile");

    let texture = tile
        .chunks
        .iter()
        .find_map(|c| c.base_texture.clone())
        .expect("at least one chunk has a base texture");

    let (w, h, rgba) =
        benilla_formats::read_texture_rgba(&mut chain, &texture).expect("decode terrain texture");
    assert!(w > 0 && h > 0, "non-empty texture, got {w}x{h}");
    assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA8 pixel buffer");
}

#[test]
fn resolves_creature_models_from_dbcs() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let catalog =
        benilla_formats::load_creature_catalog(&mut chain).expect("load creature display catalog");
    assert!(
        catalog.len() > 1000,
        "vanilla has thousands of creature displays, got {}",
        catalog.len()
    );

    // Scan display ids; many should resolve. Prove at least one resolved creature model that ships
    // in this client actually meshes (not every CreatureModelData path is present on disk). Scale 0
    // is valid — many low-id displays are invisible trigger NPCs — so we only require most positive.
    let (mut resolved, mut positive_scale, mut meshed) = (0u32, 0u32, false);
    for id in 1..6000u32 {
        let Some(m) = catalog.model(id) else { continue };
        resolved += 1;
        if m.scale > 0.0 {
            positive_scale += 1;
        }
        let p = m.model_path.to_ascii_lowercase();
        if !meshed && m.scale > 0.0 && p.contains("creature") && p.ends_with(".mdx") {
            if let Ok(subs) =
                benilla_formats::load_m2_mesh_skinned(&mut chain, &m.model_path, &m.textures)
            {
                meshed |= !subs.is_empty();
            }
        }
    }
    assert!(
        resolved > 500,
        "many display ids should resolve, got {resolved}"
    );
    assert!(
        positive_scale * 2 > resolved,
        "most resolved displays should have positive scale, got {positive_scale}/{resolved}"
    );
    assert!(
        meshed,
        "expected at least one resolved creature model to mesh"
    );
}

#[test]
fn resolves_gameobject_models_from_dbc() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let catalog = benilla_formats::load_gameobject_catalog(&mut chain)
        .expect("load gameobject display catalog");
    assert!(
        catalog.len() > 500,
        "vanilla has many GameObject displays, got {}",
        catalog.len()
    );

    // Every resolved path is a model; prove at least one meshes (dispatches .mdx/.mdl/.m2 vs .wmo).
    let mut meshed = false;
    for id in 1..3000u32 {
        let Some(path) = catalog.model_path(id).map(str::to_string) else {
            continue;
        };
        let p = path.to_ascii_lowercase();
        assert!(
            p.ends_with(".mdx") || p.ends_with(".mdl") || p.ends_with(".wmo"),
            "unexpected GameObject model path: {path}"
        );
        if !meshed {
            if let Ok(subs) = benilla_formats::load_object_model(&mut chain, &path) {
                meshed |= !subs.is_empty();
            }
        }
    }
    assert!(meshed, "expected at least one GameObject model to mesh");
}

#[test]
fn loads_classic_models_with_cosmetic_chunks() {
    let data = benilla_formats::wow_data_or_skip!();

    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    // These vanilla M2s aborted `parse_m2` with "failed to fill whole buffer" on stock wow-m2 0.6.4
    // (malformed particle-emitter / light / texture-animation chunks — warcraft-rs#56). Our patched
    // fork degrades those cosmetic chunks to empty, so geometry now loads. They must mesh.
    for path in [
        "Creature\\Kobold\\Kobold.mdx",
        "Creature\\Imp\\Imp.mdx",
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.mdx",
    ] {
        let subs = benilla_formats::load_m2_mesh(&mut chain, path)
            .unwrap_or_else(|e| panic!("load {path}: {e:#}"));
        assert!(!subs.is_empty(), "{path} should produce submeshes");
    }
}

/// Span (max−min) per axis of a point set; `[0;3]` if empty.
fn span3(ps: &[[f32; 3]]) -> [f32; 3] {
    if ps.is_empty() {
        return [0.0; 3];
    }
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in ps {
        for i in 0..3 {
            mn[i] = mn[i].min(p[i]);
            mx[i] = mx[i].max(p[i]);
        }
    }
    [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]]
}

#[test]
fn decodes_m2_collision_hulls() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Golden vectors hand-verified against the raw bytes 2026-06-02. The
    // hull is a coarse TRUNK, far smaller than the render canopy — the "blocked by trunk, walk through
    // leaves" shape. Exact tri/vert counts fire if a wow-m2 bump changes the bounding-array decode.
    let pine = benilla_formats::load_m2_collision_hull(
        &mut chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTree01\\ElwynnPine01.mdx",
    )
    .expect("pine hull");
    assert_eq!(pine.triangle_count(), 6, "ElwynnPine01 hull = 6 tris");
    assert_eq!(pine.positions.len(), 5, "ElwynnPine01 hull = 5 verts");
    let s = span3(&pine.positions);
    assert!(
        (9.0..11.0).contains(&s[2]),
        "pine trunk hull ~10 yd tall, got Z span {:.2}",
        s[2]
    );
    assert!(
        s[0] < 4.0 && s[1] < 4.0,
        "pine trunk hull is thin (≪ canopy), got X={:.2} Y={:.2}",
        s[0],
        s[1]
    );
    assert!(
        pine.indices
            .iter()
            .all(|&i| (i as usize) < pine.positions.len()),
        "hull indices in range"
    );

    let canopy = benilla_formats::load_m2_collision_hull(
        &mut chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTreeCanopy01.mdx",
    )
    .expect("canopy hull");
    assert_eq!(
        canopy.triangle_count(),
        52,
        "ElwynnTreeCanopy01 hull = 52 tris"
    );
    assert_eq!(
        canopy.positions.len(),
        30,
        "ElwynnTreeCanopy01 hull = 30 verts"
    );

    // Sanity: the hull is much smaller than the rendered model's authored bbox (trunk vs full canopy).
    let bounds = benilla_formats::load_m2_bounds(
        &mut chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTreeCanopy01.mdx",
    )
    .expect("canopy bounds");
    let render_x = bounds.bbox_max[0] - bounds.bbox_min[0];
    let hull_x = span3(&canopy.positions)[0];
    assert!(
        hull_x < render_x * 0.6,
        "collision hull (X span {hull_x:.1}) should be far tighter than the render bbox ({render_x:.1})"
    );
}

#[test]
fn filters_wmo_collidable_triangles_by_mopy() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Golden vector: the binary-verified client filter (collide iff `!(flags & 0x04 DETAIL)`,
    // `0x6a37b0` -> `0x6bc700`) selects exactly 5114 collidable tris from the Goldshire Inn. (On
    // this fixture that equals the old `COLLISION||(RENDER&&!DETAIL)` count — every non-DETAIL face here
    // is also COLLISION-or-RENDER; the rules diverge only on flags-0x00/0x40-only faces, absent here.)
    // Pins both the filter clause and the wow-wmo group decode.
    let inn = benilla_formats::load_wmo_collision_tris(
        &mut chain,
        "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo",
    )
    .expect("goldshire inn collision");
    assert_eq!(inn.triangle_count(), 5114, "Goldshire Inn collidable tris");
    assert!(
        inn.indices
            .iter()
            .all(|&i| (i as usize) < inn.positions.len()),
        "collidable indices in range"
    );
    let s = span3(&inn.positions);
    assert!(
        (40.0..80.0).contains(&s[0])
            && (20.0..50.0).contains(&s[1])
            && (20.0..45.0).contains(&s[2]),
        "inn collidable bbox should be building-sized, got {s:?}"
    );

    // A bridge is ~entirely collision surface (you walk across the deck): elwynnwidebridge = 100%.
    let bridge = benilla_formats::load_wmo_collision_tris(
        &mut chain,
        "World\\wmo\\Azeroth\\Collidable Doodads\\Elwynn\\WideBridge\\ElwynnWideBridge.wmo",
    )
    .expect("elwynn wide bridge collision");
    assert!(
        bridge.triangle_count() > 0,
        "a bridge must carry collidable triangles (the 'walk under bridges' fix)"
    );
}

#[test]
fn derives_the_wmo_window_glass_law_from_momt() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Golden vectors for the three MOMT window/glass mechanisms (the exterior UNLIT drawer
    // `0x6b4f10`, the interior WINDOW drawer `0x6b5190`, the SIDN night-ramp `0x6b4090`). The
    // Goldshire Inn authors both window kinds:
    //   mat 5  MM_ELWYNN_WND_EXT__01, flags 0x11 (UNLIT|SIDN), exterior groups only → the unlit
    //          fullbright pane (SIDN inert under lighting-off), sidn colour still carried;
    //   mat 21 MM_ELWYNN_WND_INT__01, flags 0x28 (WINDOW), interior groups' EXT section → lit by
    //          the interior midpoint light, never fullbright, no SIDN colour.
    let inn = benilla_formats::load_wmo(
        &mut chain,
        "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo",
    )
    .expect("goldshire inn render submeshes");
    let tex_is = |sub: &benilla_formats::RenderSubmesh, name: &str| {
        sub.texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_uppercase().ends_with(name))
    };
    let ext_panes: Vec<_> = inn
        .iter()
        .filter(|s| tex_is(s, "MM_ELWYNN_WND_EXT__01.BLP"))
        .collect();
    assert!(!ext_panes.is_empty(), "inn exterior window panes present");
    for s in &ext_panes {
        assert!(!s.interior, "the EXT pane sits in an exterior group");
        assert!(s.emissive, "UNLIT on an exterior group ⇒ unlit fullbright");
        assert_eq!(s.sidn, Some([203, 203, 203]), "the authored SIDN colour");
        assert!(!s.window, "no WINDOW bit on the exterior pane");
    }
    let int_panes: Vec<_> = inn
        .iter()
        .filter(|s| tex_is(s, "MM_ELWYNN_WND_INT__01.BLP"))
        .collect();
    assert!(!int_panes.is_empty(), "inn interior window panes present");
    for s in &int_panes {
        assert!(s.interior, "the INT pane sits in interior groups");
        assert!(
            !s.emissive,
            "the interior drawer ignores UNLIT — never fullbright"
        );
        assert!(s.window, "WINDOW (0x20) ⇒ the interior midpoint light");
        assert_eq!(s.sidn, None, "no SIDN flag on the interior pane");
        assert_eq!(
            s.wmo_batch,
            Some(benilla_formats::WmoBatchClass::Ext),
            "the inn's interior panes land in the EXT-in-group (lit) section"
        );
    }

    // The SIDN colour is a CImVector — BGRA on disk. Shadowfang's stained glass authors an
    // ASYMMETRIC colour (disk bytes 216,231,250,255), so this pins the BGRA→RGB decode: a warm
    // cream (250,231,216), not a cool blue — a swizzle bug can't pass.
    let sfk = benilla_formats::load_wmo(
        &mut chain,
        "World\\wmo\\Dungeon\\LD_ShadowFang\\LD_ShadowFang.wmo",
    )
    .expect("shadowfang render submeshes");
    assert!(
        sfk.iter().any(|s| s.sidn == Some([250, 231, 216])),
        "Shadowfang authors the warm-cream SIDN glass (BGRA 216,231,250 → RGB 250,231,216)"
    );
}

#[test]
fn reads_authentic_atmosphere_from_light_dbc() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::LightCatalog::load(&mut chain).expect("load Light DBCs");

    // Northshire (Human start) on Azeroth resolves to the continent's global daytime light.
    let a = cat.sample_noon(0, [-8949.95, -132.49, 83.5]);

    // Fog bands are RAW (no /36 — that scale is positions-only, per the reference):
    // the client pushes `min(raw, farclip)`, so these huge land values resolve to the view distance.
    // Must have read real bands, not fallen back to the default.
    assert_ne!(
        a.fog_end,
        benilla_formats::Atmosphere::DEFAULT.fog_end,
        "got the fallback atmosphere — Light bands didn't resolve"
    );

    // Exact verified ground truth (2026-05-25): Northshire → global light #1 → clear LightParams 12.
    // Decoded independently from the raw DBC bytes and cross-checked vs wowdev.wiki; locking these
    // pins the band index formula, the row→meaning map, the color decode, AND the light selection.
    let to255 = |c: [f32; 3]| {
        [
            (c[0] * 255.0).round() as i32,
            (c[1] * 255.0).round() as i32,
            (c[2] * 255.0).round() as i32,
        ]
    };
    assert!(
        (a.fog_end - 500.0).abs() < 1.0,
        "clear fog_end should be 500 yd (raw 18000/36, decision 0324), got {}",
        a.fog_end
    );
    assert!(
        (a.fog_start_frac - 0.25).abs() < 0.001,
        "clear fog start fraction should be +0.25, got {}",
        a.fog_start_frac
    );
    assert_eq!(to255(a.sky[0]), [0, 31, 73], "SkyTop/zenith (int row 2)");
    assert_eq!(to255(a.sky[1]), [58, 162, 207], "SkyMiddle (int row 3)");
    assert_eq!(to255(a.sky[2]), [153, 220, 245], "SkyBand1 (int row 4)");
    assert_eq!(to255(a.sky[3]), [175, 218, 224], "SkyBand2 (int row 5)");
    assert_eq!(
        to255(a.sky[4]),
        [180, 180, 180],
        "SkySmog/horizon (int row 6)"
    );
    assert_eq!(to255(a.sun_color), [255, 247, 222], "sun (int row 9)");
    assert_eq!(to255(a.ambient), [104, 130, 154], "ambient (int row 1)");
    assert_eq!(to255(a.fog_color), [77, 120, 143], "fog color (int row 7)");
    for c in [a.fog_color, a.sun_color, a.ambient] {
        for ch in c {
            assert!(
                (0.0..=1.0).contains(&ch),
                "color channel out of range: {ch}"
            );
        }
    }
    assert!(
        a.fog_color.iter().sum::<f32>() > 0.2 && a.sun_color.iter().sum::<f32>() > 0.5,
        "daytime colors implausibly dark: fog {:?} sun {:?}",
        a.fog_color,
        a.sun_color
    );
}

/// Elwynn's STORM param (k2 = LightParams 10) at midday — decoded off the real DBCs and
/// byte-traced through the blend. The load-bearing pin is the **negative fog-start fraction**:
/// −0.5, UNCLAMPED (`0x6cee6d`) — the mechanism behind the reference's constant ~33% near veil
/// under rain. A clamp regression here silently kills the veil.
#[test]
fn reads_elwynn_storm_fog_endpoints() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::LightCatalog::load(&mut chain).expect("load Light DBCs");

    // Northshire, noon, storm slot (Elwynn global light #1 → storm LightParams 10, single-stop bands).
    let s = cat.sample(0, [-8949.95, -132.49, 83.5], 1440, true);
    assert!(
        (s.fog_end - 277.8).abs() < 1.0,
        "storm fog_end should be ~278 yd (raw 10000/36, decision 0324), got {}",
        s.fog_end
    );
    assert!(
        (s.fog_start_frac - -0.5).abs() < 0.001,
        "storm fog start fraction should be −0.5 (negative ⇒ the near veil), got {}",
        s.fog_start_frac
    );
    let to255 = |c: [f32; 3]| {
        [
            (c[0] * 255.0).round() as i32,
            (c[1] * 255.0).round() as i32,
            (c[2] * 255.0).round() as i32,
        ]
    };
    assert_eq!(to255(s.fog_color), [82, 84, 82], "storm fog color (grey)");
    assert_eq!(
        to255(s.sun_diffuse),
        [101, 101, 101],
        "storm diffuse (flat grey)"
    );
    assert_eq!(to255(s.ambient), [78, 78, 95], "storm ambient (grey-blue)");
}

#[test]
fn reads_map_directories_from_map_dbc() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::load_map_catalog(&mut chain).expect("load Map.dbc");

    // 5875 has ~44 maps (2 continents, ~12 dungeons, BGs, dev maps).
    assert!(
        cat.len() >= 40,
        "expected ≥40 Map.dbc rows, got {}",
        cat.len()
    );
    // Canonical IDs — pin the schema so a field-count drift can't silently break the directory
    // column (which would silently make every cross-map teleport pick the wrong terrain).
    assert_eq!(cat.directory(0), Some("Azeroth"));
    assert_eq!(cat.directory(1), Some("Kalimdor"));
    // Deadmines instance — a dungeon, proves dungeons resolve too.
    assert_eq!(cat.directory(36), Some("DeadminesInstance"));
}

#[test]
fn decodes_azeroth_wdl_against_apitrace_ground_truth() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    // Eastern Kingdoms distant-terrain heightmap.
    let wdl = benilla_formats::WdlFile::load(&mut chain, "Azeroth").expect("load Azeroth.wdl");

    // VERIFIED by walking the real file (2026-05-30): MVER=18, 687 present tiles.
    assert_eq!(wdl.present_count(), 687, "Azeroth.wdl present-tile count");

    // Golden vector tied to apitrace WoW.8: the 2nd WDL tile drawn that frame spanned world
    // X[-9066.7,-8533.3] Y[-1600,-1066.7] (= tile_x 34, tile_y 48) with vertex heights 54..349.
    assert!(
        wdl.is_present(34, 48),
        "apitrace tile (34,48) must be present"
    );
    let mesh = wdl.tile_mesh(34, 48).expect("mesh for (34,48)");
    assert_eq!(mesh.positions.len(), 545, "545 verts (17x17 + 16x16)");
    assert_eq!(
        mesh.indices.len(),
        3072,
        "3072 indices (16x16 cells x 4 tris)"
    );

    // Heights (world Z) — exact int16 from the file; must match the captured VB range.
    let zs: Vec<f32> = mesh.positions.iter().map(|p| p[2]).collect();
    let zmin = zs.iter().copied().fold(f32::INFINITY, f32::min);
    let zmax = zs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(zmin, 54.0, "tile (34,48) min height (apitrace VB)");
    assert_eq!(zmax, 349.0, "tile (34,48) max height (apitrace VB)");

    // Planar extent — one ADT tile (533.33 yd) on each axis, matching the captured VB span.
    let xs: Vec<f32> = mesh.positions.iter().map(|p| p[0]).collect();
    let ys: Vec<f32> = mesh.positions.iter().map(|p| p[1]).collect();
    let span = |v: &[f32]| {
        v.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - v.iter().copied().fold(f32::INFINITY, f32::min)
    };
    assert!(
        (span(&xs) - 533.333).abs() < 0.5,
        "X span ~533.33, got {}",
        span(&xs)
    );
    assert!(
        (span(&ys) - 533.333).abs() < 0.5,
        "Y span ~533.33, got {}",
        span(&ys)
    );
    // Max-X/-Y corner = tile origin (matches wow_wdt::tile_to_world(34,48)).
    let xmax = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let ymax = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!((xmax - (-8533.3)).abs() < 0.5, "tile origin X, got {xmax}");
    assert!((ymax - (-1066.7)).abs() < 0.5, "tile origin Y, got {ymax}");
}

/// The two **fixed global `LightParams` rows** magma and slime submersion read (`light::PARAM_MAGMA`
/// / `PARAM_SLIME`, byte-VERIFIED `0x6d2371`), pinned against the shipped client.
///
/// This is the cross-check that earned the finding rather than merely repeating it: nothing in
/// `Light.dbc` references rows 6 or 7, so no position-keyed sample can reach them and no zone can
/// vouch for them. What vouches for them is that they decode to *exactly* the vanilla lava and slime
/// view — dense fiery orange and dense pure green — which a wrong pair of row numbers would not.
#[test]
fn magma_and_slime_submersion_read_the_fixed_global_light_params() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::LightCatalog::load(&mut chain).expect("load Light DBCs");
    let to255 = |c: [f32; 3]| {
        [
            (c[0] * 255.0).round() as i32,
            (c[1] * 255.0).round() as i32,
            (c[2] * 255.0).round() as i32,
        ]
    };

    // Two positions on different continents whose CLEAR atmospheres differ — the fixed rows must be
    // identical at both, because they are zone-independent. This is the property that separates "a
    // fixed global row" from "a zone underwater slot that happens to look right at one pin".
    let pins = [
        (0u32, [-7531.21f32, -1123.64, 172.58]), // Blackrock Mountain (the B24/B68 lava pin)
        (1u32, [-5075.53f32, -2063.09, -50.10]), // Thousand Needles (the B68 water pin)
    ];
    for kind in [
        benilla_formats::Submersion::Magma,
        benilla_formats::Submersion::Slime,
    ] {
        let sampled: Vec<_> = pins
            .iter()
            .map(|(map, pos)| cat.sample_blended(*map, *pos, 1440, false, kind, false))
            .collect();
        assert_eq!(
            to255(sampled[0].fog_color),
            to255(sampled[1].fog_color),
            "{kind:?} is zone-INDEPENDENT: the fixed row must not vary by position"
        );
        assert_eq!(
            sampled[0].fog_end, sampled[1].fog_end,
            "{kind:?} fog end must not vary by position"
        );
    }

    // Magma → row 7. Dense fiery orange, and the shortest fog in the client: 972/36 = 27 yd, with a
    // −2.0 start fraction putting `1 − 27/(27+54)` = 67 % of it at the eye itself.
    let magma = cat.sample_blended(
        0,
        pins[0].1,
        1440,
        false,
        benilla_formats::Submersion::Magma,
        false,
    );
    assert_eq!(
        to255(magma.fog_color),
        [200, 52, 0],
        "magma fog (row 7 int 7)"
    );
    assert_eq!(to255(magma.ambient), [255, 55, 0], "magma ambient");
    assert!(
        (magma.fog_end - 27.0).abs() < 0.01,
        "magma fog end 972/36 = 27 yd, got {}",
        magma.fog_end
    );
    assert!(
        (magma.fog_start_frac + 2.0).abs() < 1e-6,
        "magma start fraction −2.0, got {}",
        magma.fog_start_frac
    );

    // Slime → row 6. Pure green at 1800/36 = 50 yd, −1.0 start = 50 % at the eye.
    let slime = cat.sample_blended(
        0,
        pins[0].1,
        1440,
        false,
        benilla_formats::Submersion::Slime,
        false,
    );
    assert_eq!(
        to255(slime.fog_color),
        [0, 255, 0],
        "slime fog (row 6 int 7)"
    );
    assert_eq!(to255(slime.ambient), [0, 60, 0], "slime ambient");
    assert!(
        (slime.fog_end - 50.0).abs() < 0.01,
        "slime fog end 1800/36 = 50 yd, got {}",
        slime.fog_end
    );
    assert!(
        (slime.fog_start_frac + 1.0).abs() < 1e-6,
        "slime start fraction −1.0, got {}",
        slime.fog_start_frac
    );

    // And the whole point: neither is the zone's own underwater atmosphere. At the Thousand Needles
    // pin the water murk is the olive-brown LP 203 — being in lava there must not show it.
    let water = cat.sample_blended(
        1,
        pins[1].1,
        1440,
        false,
        benilla_formats::Submersion::Water,
        false,
    );
    assert_ne!(
        to255(water.fog_color),
        to255(magma.fog_color),
        "lava must not inherit the zone's water murk"
    );
    assert_ne!(
        to255(water.fog_color),
        to255(slime.fog_color),
        "slime must not inherit the zone's water murk"
    );
    // Dry is different again — the guard against "everything resolved to one fallback".
    let dry = cat.sample_blended(
        1,
        pins[1].1,
        1440,
        false,
        benilla_formats::Submersion::Dry,
        false,
    );
    assert_ne!(
        to255(dry.fog_color),
        to255(water.fog_color),
        "the water pin's underwater slot must differ from its clear slot"
    );
}

/// The far band's tile window **contains the camera's own tile**. Dropping it was
/// invisible at the default view distance — a 533 yd tile sits inside a 777 yd wall, so it would
/// have been discarded anyway — and a hole above the horizon at any lower one, where the own tile is
/// the only thing that draws the near horizon. The director found it at view distance 320 in
/// Weazel's Crater; this pins the window at the coordinates they reported.
#[test]
fn the_wdl_window_contains_the_cameras_own_tile() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let wdl = benilla_formats::WdlFile::load(&mut chain, "Kalimdor").expect("load Kalimdor.wdl");

    // Weazel's Crater, Thousand Needles — the report's spot. The client's debug panel read
    // `tile 39,42` standing there, which is what `world_to_tile` must agree on.
    let (x, y) = (-5841.9_f32, -3802.4);
    let own = benilla_wdt::world_to_tile(x, y);
    assert_eq!(own, (39, 42), "the reported tile for Weazel's Crater");
    assert!(wdl.is_present(own.0, own.1), "the own tile is authored");

    let window = wdl.tiles_in_ring(x, y, 5);
    assert!(
        window.contains(&own),
        "the camera's own WDL tile must be in the window — without it the near horizon is a hole"
    );
    // And it is a full (2r+1)² window, not a ring: every present tile within the radius.
    let expected = (-5i32..=5)
        .flat_map(|dy| (-5i32..=5).map(move |dx| (own.0 as i32 + dx, own.1 as i32 + dy)))
        .filter(|&(tx, ty)| (0..64).contains(&tx) && (0..64).contains(&ty))
        .filter(|&(tx, ty)| wdl.is_present(tx as u32, ty as u32))
        .count();
    assert_eq!(window.len(), expected, "every present tile in the window");
}

/// A **tombstoned path must never fall through to the live copy a base archive still holds.**
///
/// `patch.MPQ` deletes 26 paths that `model.MPQ` and friends still carry in full — cut content the
/// 1.12.1 client does not load. The failure this pins is silent and points the
/// wrong way: a chain that resolved *past* the delete-marker would serve `OgreMage.m2`'s 386 KB as
/// if it were current, looking entirely healthy while rendering a model the reference never draws.
/// Until now the property was held by a comment in `Chain::read` and nothing else.
///
/// This chain already filters them out — the point of this test is that it stays that way.
///
/// Non-vacuous by construction: each name is first read straight out of the base archive that still
/// holds it, so the test fails loudly if the pairing ever stops being a real tombstone-over-content
/// case rather than passing on an absent file.
#[test]
fn tombstoned_paths_never_fall_through_to_the_base_copy() {
    let data = benilla_formats::wow_data_or_skip!();

    // Tombstoned by `patch.MPQ`; still present, in full, in `model.MPQ` (measured: 386416 / 437456).
    const DELETED: [&str; 2] = [
        "Creature\\OgreMage\\OgreMage.m2",
        "Creature\\OgreWarlord\\OgreWarlord.m2",
    ];

    let base = Chain::open(&data.join("model.MPQ")).expect("open model.MPQ alone");
    let chain = Chain::open(&data).expect("open the vanilla patch chain");
    let listed: std::collections::HashSet<String> = chain
        .list()
        .expect("list the chain")
        .into_iter()
        .map(|e| e.name.replace('/', "\\").to_ascii_lowercase())
        .collect();

    for name in DELETED {
        // The positive control: the content really is still there to be served by mistake.
        let live = base
            .read(name)
            .unwrap_or_else(|e| panic!("{name} must still exist in model.MPQ: {e}"));
        assert!(
            live.len() > 100_000,
            "{name} is a real model in the base archive, got {} bytes",
            live.len()
        );

        // And the composite refuses it on all three paths.
        assert!(!chain.contains(name), "{name} is deleted from the chain");
        let err = chain
            .read(name)
            .expect_err("reading a tombstoned path must fail, not return the base copy")
            .to_string();
        assert!(
            err.contains("deleted from patch chain"),
            "the error must name the tombstone, not a generic miss: {err}"
        );
        assert!(
            !listed.contains(&name.to_ascii_lowercase()),
            "{name} must not be enumerated"
        );
    }
}

/// The WMO **skybox** pair, against the shipped Stratholme files (`crate::models::wmo`,
/// `benilla::skybox`): a root's MOSB model and the per-group `0x40000` gate that asks for it.
///
/// The sibling roots are the whole point of the fixture. `Stratholme_B` is the burning **city** — it
/// names `StratholmeSkybox.m2` *and* 61 of its 83 groups set the bit, which is why the reference's
/// sky in King's Square is a painted red sky and not map 329's khaki `Light.dbc` gradient.
/// `Stratholme.wmo` is the **dungeon** next door: no MOSB, and not one of its 92 groups sets the bit.
/// A regression that drops either half (the chunk parse, or the flag) shows up here as one of the two
/// halves going quiet, and the two together are what keeps the gate off the chunk alone — four 1.12
/// roots name a skybox no group ever asks for (`benilla-extract skyboxscan`).
#[test]
fn reads_the_skybox_and_its_per_group_gate() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");

    let city = chain
        .read_file("World\\wmo\\Dungeon\\LD_Stratholme\\Stratholme_B.wmo")
        .expect("read the Stratholme city WMO root");
    let city = benilla_formats::parse_wmo_root(&city).expect("parse the city root");
    assert_eq!(
        city.skybox(),
        Some("environments\\stars\\stratholmeskybox.m2"),
        "the city root's MOSB names the painted sky (normalized .mdx -> .m2)"
    );
    let asking = city.group_infos().iter().filter(|g| g.show_skybox).count();
    assert_eq!(
        (asking, city.group_infos().len()),
        (61, 83),
        "the city's open streets ask for the skybox; its enclosed rooms don't"
    );

    let dungeon = chain
        .read_file("World\\wmo\\Dungeon\\LD_Stratholme\\Stratholme.wmo")
        .expect("read the Stratholme dungeon WMO root");
    let dungeon = benilla_formats::parse_wmo_root(&dungeon).expect("parse the dungeon root");
    assert_eq!(
        dungeon.skybox(),
        None,
        "the dungeon root's MOSB is the empty string — no skybox"
    );
    assert_eq!(
        dungeon
            .group_infos()
            .iter()
            .filter(|g| g.show_skybox)
            .count(),
        0,
        "and not one of its groups asks for one"
    );
}

/// **The footstep chain's WMO leg, on the shipped bytes** (bug B236's sequel).
///
/// `MOMT+0x20` is a `TerrainType.dbc` id, and this is the evidence for that claim rather than a
/// format doc: across every root WMO in the archive the dword only ever takes ids the table
/// actually has, and `10 "None"` — the unauthored default — dominates. The Kharanos inn is the
/// reported case: it authors `None` on all 20 materials, so the reference gives a walker its
/// generic dry kit and NO footprint, where reading the ADT beneath the floor gives snow and snow
/// prints.
#[test]
fn wmo_ground_type_is_a_terrain_type_id() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::load_footstep_catalog(&mut chain).expect("footstep catalog");

    // The Kharanos inn ("Thunderbrew Distillery" is the AREA name; the model is snow_Inn).
    let inn = chain
        .read_file("World\\wmo\\KhazModan\\Buildings\\Dwarven_Inn\\snow_Inn\\Snow_Inn.wmo")
        .expect("the Kharanos inn root");
    let inn = benilla_formats::parse_wmo_root(&inn).expect("inn root parses");
    let ground = inn.material_ground_types();
    assert_eq!(ground.len(), 20, "the inn's material count");
    assert!(
        ground.iter().all(|&g| g == 10),
        "every Kharanos inn material is the unauthored `None`: {ground:?}"
    );

    // `None` is not silence — it is a real row, and it is the quiet generic step.
    assert_eq!(
        cat.sound_class_of(10),
        Some(0),
        "TerrainType 10 -> SoundID 0"
    );
    assert!(
        !cat.terrain_leaves_footprints(10),
        "a WMO floor takes no prints"
    );
    assert!(
        cat.terrain_leaves_footprints(3) && cat.terrain_leaves_footprints(7),
        "Snow and Sand are the print surfaces"
    );
    assert_eq!(
        cat.resolve_terrain(7, 10).map(|(dry, _)| dry),
        Some(560),
        "class 7 indoors: CharacterMediumLargeDirt"
    );
    assert_eq!(
        cat.resolve_terrain(7, 3).map(|(dry, _)| dry),
        Some(563),
        "class 7 on snow: CharacterMediumLargeSnow — what the ADT-only chain wrongly played inside"
    );

    // The value-domain argument, over every root WMO in the archive: nothing outside the table.
    let roots: Vec<String> = chain
        .list()
        .expect("chain listing")
        .into_iter()
        .map(|e| e.name)
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".wmo")
                && !l
                    .strip_suffix(".wmo")
                    .and_then(|s| s.rsplit('_').next())
                    .is_some_and(|t| t.len() == 3 && t.chars().all(|c| c.is_ascii_digit()))
        })
        .collect();
    assert_eq!(roots.len(), 815, "root WMOs in the 5875 archive");
    let (mut total, mut none, mut multi) = (0usize, 0usize, 0usize);
    for name in &roots {
        let Ok(bytes) = chain.read_file(name) else {
            continue;
        };
        let Ok(root) = benilla_formats::parse_wmo_root(&bytes) else {
            continue;
        };
        let g = root.material_ground_types();
        for &v in &g {
            assert!(
                cat.sound_class_of(v).is_some(),
                "{name}: ground_type {v} is not a TerrainType id"
            );
            total += 1;
            none += usize::from(v == 10);
        }
        multi += usize::from(g.iter().collect::<std::collections::HashSet<_>>().len() > 1);
    }
    assert_eq!(
        (total, none),
        (10_299, 10_075),
        "the 5875 ground_type census"
    );
    assert_eq!(multi, 121, "roots authoring more than one surface");
}

/// The **ghost sky** — `LightParams.lightSkyboxID` → `LightSkybox.dbc` → a model the chain can
/// actually read. Pins all three joints of a path that was parsed and then dropped on the floor:
/// the field was named in the schema and had zero consumers, so nothing caught that the table it
/// indexes was never loaded at all.
///
/// The reference reaches this table only through the ghost override (`0x6d26cb`, gated on
/// `[0xce9bb0] != -1`), and the shipped data matches that gate exactly: 5 of 426 `LightParams`
/// rows carry a non-zero id, all of them 3 = `DeathClouds.mdx`, reached from param slot **4** of
/// every one of the 374 `Light` rows.
/// So the answer is the same everywhere, which is what the spread of positions below asserts —
/// including Deeprun Tram, the shipped map with no `Light` row of its own (it must still find the
/// sky through the zero-match fallback row, not come back bare).
#[test]
fn resolves_the_ghost_skybox_everywhere() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let cat = benilla_formats::LightCatalog::load(&mut chain).expect("load Light DBCs");

    const DEATH_CLOUDS: &str = "environments\\stars\\deathclouds.m2";
    for (map, pos, what) in [
        (0u32, [-8949.95f32, -132.49, 83.5], "Northshire, Azeroth"),
        (1, [1629.0, -4373.0, 31.0], "Orgrimmar, Kalimdor"),
        (
            369,
            [0.0, 0.0, 0.0],
            "Deeprun Tram — no Light row of its own",
        ),
    ] {
        assert_eq!(
            cat.ghost_skybox(map, pos),
            Some(DEATH_CLOUDS),
            "the ghost sky must resolve at {what}"
        );
    }

    // The path is normalised to what the chain actually holds: the DBC authors `.mdx`, the archive
    // ships `.m2`. Reading it here is the half that a name-only assert would miss — the render lane
    // hands this exact string to `load_m2_mesh`.
    let bytes = chain
        .read_file(DEATH_CLOUDS)
        .expect("the ghost sky model the DBC names must be readable from the chain");
    assert_eq!(&bytes[..4], b"MD20", "DeathClouds is an M2");
}
