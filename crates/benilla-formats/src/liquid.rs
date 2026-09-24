//! MCLQ liquid surfaces → per-chunk water meshes (Phase E; Phase 1 = inland water).
//!
//! Vanilla liquid is **MCLQ** — per-MCNK, a 9×9 absolute-height grid + an 8×8 cell-flag grid
//! (no WotLK MH2O), one 804-byte block per set liquid header bit. [`MclqChunk`] decodes it; we
//! validated the output **byte-identical** to a hand-decode of the raw payload against
//! `World\Maps\Azeroth\Azeroth_32_48.adt` (2026-05-31): per-vertex `height`, raw `tile_flags` and
//! `min/max_height` all match (the on-disk `QLCM` magic sits at the offset; the payload — and the
//! crate's parse — start 8 bytes in).
//!
//! Output is in **raw WoW coords** (+X north, +Y west, +Z up, yards), like [`crate::terrain`]; the
//! renderer applies the WoW→Bevy transform and ports the `ocean0_s.bls` shader. We emit a flat
//! surface (normal `(0,0,1)`) of 2 triangles per **wet** cell; dry cells (flag low-nibble `0xf`,
//! whose verts carry the FLT_MAX sentinel height) are skipped.
//!
//! **The cell nibble is the type** (VERIFIED `0x6ba970`/`0x68d9b0`) — the MCNK header bits only say
//! how many blocks a chunk carries, and their bit→type ordering was never byte-proven. Reading the
//! type off the header instead is what kept every open-world lava surface in the game from
//! rendering (Burning Steppes, Searing Gorge, Un'Goro) and what put the ocean texture on 28 coastal
//! river blocks while dropping the sea beside them. `build_liquid_mesh` runs the cells first for
//! exactly that reason.

use benilla_adt::MclqChunk;

use crate::terrain::{snap_to_lattice, UNIT_SIZE};

/// Which animated texture set + render path a liquid surface uses (selects the frame set in
/// `benilla`). The MCLQ (ADT) path emits Still/Ocean/Magma — the reference's three fixed ADT liquid
/// queues; the WMO MLIQ path additionally emits Slime and Rapids (canals, fountains, the Ironforge
/// lava, Undercity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    /// Still water — `XTextures\river\lake_a.*` (30 frames). Lakes, ponds, slow rivers, WMO canals.
    Still,
    /// Rapids — `XTextures\river\fast_a.*` (16 frames). Per-cell nibble `8`.
    ///
    /// **Nothing in the shipped 1.12.1 data selects it**: a sweep of every MCLQ cell on all 12
    /// terrain maps and every one of the 302 MLIQ-bearing WMO groups finds nibbles `{1, 4, 6}` and
    /// `{0, 2, 3, 4, 6, 7}` respectively — never `8`. Kept because the name table `0x86a000` really
    /// does map slot 8 here and the WMO path binds the raw nibble, so authored data *could*; the ADT
    /// path can't reach it either way (its river queue hard-codes texcache type 4).
    Rapids,
    /// Ocean — `XTextures\ocean\ocean_h.*` (30 frames). Coastal / sea tiles.
    Ocean,
    /// Magma / lava — `XTextures\lava\lava.*`. Opaque and **unlit**: the animated texture IS the body,
    /// with no depth LUT to modulate it by — but still **fogged**, like every liquid batch (no
    /// liquid setup overrides the device's fog-on default `0x593d18`; the `0x68d890` vert-fill's
    /// `0x3f800000` is an up normal's Z, not a colour or alpha, and it is byte-identical in the river
    /// and ocean fills).
    ///
    /// Reached from **both** paths: WMO MLIQ (the Ironforge Great Forge) and — since the ADT magma
    /// queue landed — MCLQ, which is all the open-world lava there is: 124 chunks in Burning Steppes
    /// and Searing Gorge, 4 in Un'Goro. Its UVs are authored per-vertex, not generated
    /// ([`benilla_adt::LiquidVertex::texcoords`]).
    Magma,
    /// Slime — `XTextures\slime\slime.*`. Its own texture on the magma render category, and **code-path
    /// identical to magma** on the WMO side: one handler, no type-2-vs-3 branch anywhere
    /// (`0x6b68f0`).
    ///
    /// **WMO liquid only** (nibbles `3`/`7`) — Undercity, the Sludge Fields. Not a deferral: the
    /// reference has no ADT queue for slime at all (VERIFIED — `0x68de40` dispatches only its three
    /// queues, types 4/1/6), and no shipped MCLQ cell carries nibble 3 or 7 to feed one.
    Slime,
}

impl LiquidKind {
    /// The liquid type from a per-tile MLIQ flag low nibble (`flag & 0xf`) — the reference
    /// selection (name table `0x86a000`): the nibble indexes the animated-texture table directly.
    /// `0xf` (and any unmapped value) = hole / no liquid → `None`. `4` is a same-class `lake_a`
    /// variant of `0`; `6`/`7` of `2`/`3`.
    pub fn from_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 0xf {
            0 | 4 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 | 6 => Some(LiquidKind::Magma),
            3 | 7 => Some(LiquidKind::Slime),
            8 => Some(LiquidKind::Rapids),
            _ => None, // 5, 9..=0xf — NULL name-table slots / the 0xf hole sentinel
        }
    }

    /// The kind an **ADT** (MCLQ) cell nibble selects — which is *not* [`Self::from_nibble`].
    ///
    /// The reference's ADT liquid render is **three fixed queues, each hard-coding its texcache
    /// type**: `0x6851b0` → type 4 `lake_a` (`mov ecx,4` @`0x68db9d`), `0x685010` → type 1 `ocean_h`
    /// (@`0x68dabb`), `0x6855a0` → type 6 `lava` (@`0x68dcab`). The cell nibble selects queue
    /// *membership* — via the class `nibble & 3` — not the texture, so an ADT nibble 0 and 8 both
    /// draw `lake_a`, and 2 and 6 both draw `lava`. **Slime has no ADT queue at all**: it renders
    /// only on the WMO path, so class 3 is `None` here.
    ///
    /// The WMO path is the type-faithful one — it binds `0x68aac0(raw nibble)` — and keeps
    /// [`Self::from_nibble`].
    pub fn from_adt_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 3 {
            0 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 => Some(LiquidKind::Magma),
            _ => None, // class 3 — slime, which the reference never dispatches from an ADT
        }
    }

    /// Whether this kind renders as an **opaque, unlit** surface (the animated texture is the body
    /// colour, no water swatch / depth ramp / N·L darkening) — magma and slime. Water/ocean use the
    /// depth-swatch `ocean0_s.bls` path instead.
    ///
    /// "Fullbright" here means **unlit, not unfogged**: every liquid batch in the reference draws with
    /// GL_FOG enabled, magma and slime included (no liquid setup overrides the device's fog-on
    /// default `0x593d18`). Reading it as "no fog" is what made a submerged slime surface a flat
    /// unshaded sheet.
    pub fn is_fullbright(self) -> bool {
        matches!(self, LiquidKind::Magma | LiquidKind::Slime)
    }
}

/// One liquid surface — an MCNK's MCLQ or a WMO group's MLIQ — as a **regular vertex grid** in raw
/// WoW coords (+Z up), plus the triangle list the renderer draws.
///
/// The grid is the primary form: [`Self::grid`] gives its `[cols, rows]` vertex counts,
/// `positions`/`uvs`/`depths` are that grid row-major (`j·cols + i`), and [`Self::wet`] says which
/// of the `(cols−1)·(rows−1)` cells carry liquid. `indices` is the render derivative — 2 triangles
/// per wet cell — so verts under dry cells stay present but unreferenced.
///
/// The queries (is this XY wet, how high is the surface here) read the **grid**, not the triangles:
/// the reference samples the liquid height as a bilinear over the containing cell's four corners
/// (`0x6b7500`), which is a cell lookup, not a triangle search. Drawn two-sided (cull off), so
/// winding is not load-bearing.
#[derive(Debug, Clone)]
pub struct LiquidMesh {
    /// Vertex-grid dimensions `[cols, rows]` — the counts along the two grid axes, `cols` fastest.
    /// MCLQ is always `[9, 9]`; MLIQ is the MLIQ header's `xverts`/`yverts` (Blackrock's magma is
    /// `[55, 82]`). Every per-vertex array below is this grid, row-major.
    pub grid: [u32; 2],
    /// Per-**cell** liquid coverage, row-major over `(cols−1) × (rows−1)`: `true` where the tile
    /// nibble says liquid (and, for MCLQ, its four corner heights are real). This is the source of
    /// truth for containment — a liquid grid is sparse (nibble `0xf` = hole), so its bounding box
    /// routinely spans dry ground it never touches (decision 0635).
    pub wet: Vec<bool>,
    /// Per-**cell**, parallel to [`Self::wet`]: the MLIQ tile flag's **`0x80`** bit — "a neighbouring
    /// group also claims this cell". Authored on **both** claimants, never on a cell only one group
    /// covers (measured over `Stormwind.wmo`: 190 of 2685 wet cells, each claimed by exactly two
    /// groups and flagged in both; zero overlap among the `0x80`-clear cells).
    ///
    /// The reference's strip builder emits a tile iff `(f & 0xf) != 0xf` **and** (`f & 0x80 == 0`
    /// **or** this group won the gate) — a per-frame 2-colouring of the portal graph that lets
    /// exactly ONE of the two claimants draw. Without it both surfaces draw, coplanar, and the
    /// overlap band composites the translucent water twice: the visible line across the Stormwind
    /// canals (B141's water half). Resolved model-wide in `benilla_assets::wmo`, not here — a group
    /// file cannot see its neighbours.
    ///
    /// Always `false` on the ADT path: MCLQ has no such bit and no such overlap.
    pub shared: Vec<bool>,
    /// Vertex positions `[x, y, z]` in WoW yards — `cols·rows` entries, row-major.
    pub positions: Vec<[f32; 3]>,
    /// Texture UVs parallel to `positions`. No scroll for water.
    ///
    /// Generated cell-index UVs (`¼` per cell, tiling) for **water and ocean** — and for WMO liquid,
    /// anchored to model space over the same period. **Magma is different**: its UVs are *authored*
    /// per vertex and read straight off the MCLQ union bytes (`u = s · 3/256`), because the reference's
    /// lava batch is single-stage with a reset texture matrix, so `tc0` is the vertex's own coord
    /// rather than anything the client generates ([`benilla_adt::LiquidVertex::texcoords`]).
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex liquid swatch coord **V** `0..1` from the MCLQ `SVert` depth byte, parallel to
    /// `positions`. River/lake = `clamp(byte/42)` (VERIFIED `c81768`, saturates ~5 yd); ocean =
    /// `clamp(byte/255)` (VERIFIED `c7fcd8`, saturates ~148 yd — its own LUT, on its own authored
    /// byte scale; see [`OCEAN_DEPTH_V_SATURATION`]). The reference uses
    /// this single V to index the depth swatch for BOTH the body-colour band (shallow→deep water rows)
    /// and the opacity ramp (`colorTex.a` = `127+2·row`, deeper = more opaque).
    ///
    /// **Zero on ADT magma, and read by nothing**: those union bytes are texture coords, not a depth
    /// byte, and the reference's magma vert-fill reads no ramp at all (`0x68d890` — no LUT load,
    /// unlike the river/ocean fills). The shader's fullbright branch returns before it ever samples
    /// this, so the zero is a statement, not a value. (The WMO builder fills its own flat
    /// `WMO_WATER_DEPTH_V` for every kind, magma and slime included — equally unread there.)
    pub depths: Vec<f32>,
    /// Triangle indices into `positions` — 6 per wet cell, cells in row-major order. Derived from
    /// [`Self::wet`]; the render form.
    pub indices: Vec<u32>,
    /// The surface's **sound-class nibble** — the majority wet cell's low nibble (terrain), or
    /// the `0x6ba970`-resolved nibble (WMO): `class = nibble & 3`, `FluidSpeed = nibble & 0xc`,
    /// the key the above-water liquid ambient-loop system resolves through `SoundWaterType.dbc`
    /// ([`crate::WaterSoundCatalog`]; `0x462a40`, decision 0506). Carried
    /// beside `kind` because the render kind collapses the river speeds (nibbles 0 and 4 both
    /// draw `lake_a`) that the sound table splits (RiverStill 1111 vs RiverSlow 1112).
    pub sound_nibble: u8,
    /// **WMO only** — the MLIQ header's `materialId`, i.e. this pool's index into the owning root's
    /// MOMT array. The reference's INTERIOR water kernel takes its whole body colour from
    /// `MOMT[materialId].diffColor` (see `benilla_wmo::Material::diff_color`), so this is how a pool
    /// finds its own colour; the group file alone cannot resolve it, because MOMT lives in the root.
    /// `None` on the ADT path, which has no such index and no such lookup.
    pub material_id: Option<u16>,
    /// The texture set / render path this surface uses.
    pub kind: LiquidKind,
}

/// 9×9 vertex grid per MCNK.
const GRID: usize = 9;
/// 8×8 cell grid per MCNK.
const CELLS: usize = 8;
/// Cell-flag low nibble meaning "dry / do not render".
const DRY_NIBBLE: u8 = 0x0f;
/// The scale the reference applies to a magma vertex's authored `u16` texture coords:
/// `u = (s as i32 as f64 · TEXC) as f32`, `TEXC = 0x3c400000 = 3/256`, bit-exact at
/// `WoW.exe 0x68d890`. Authored steps run ≈42 units per 4.167-yd cell, so one lava repeat spans
/// ≈8.5 yd — twice the density of the ¼-per-cell water field.
const MAGMA_TEXCOORD_SCALE: f32 = f32::from_bits(0x3c40_0000);
/// Verts under no liquid carry FLT_MAX (`0x7F7FFFFF`) — `is_finite()` is TRUE for it, so gate on
/// magnitude instead.
const HEIGHT_SENTINEL: f32 = 1.0e9;
/// River/lake depth-byte at which the swatch coord `V` saturates to 1.0: `V = clamp(byte/42)`.
/// VERIFIED from `WoW.exe` — the in-game river/lake water list (`FUN_0068d790`) reads the **steep**
/// `DAT_00c81768` LUT, built by `FUN_0068c4c0` as `clamp((d/9)/4.6667) = clamp(d/42)` (constants
/// `0x81028c=1/9`, `0x810380=1/4.6667` static-verified), and the live VBO confirms `V` is multiples
/// of 1/42 saturating at 1.0. (The gentler `byte/255` = the `DAT_00c7fcd8` LUT on a DIFFERENT list
/// `FUN_0068d690` that the from-above river path does not use — hence the river middle never reached
/// the deep/teal swatch row until this. The river depth ramp is ≈8.5 byte/yd, so V saturates at ≈5 yd.)
const RIVER_DEPTH_V_SATURATION: f32 = 42.0;
/// Ocean depth-byte at which the swatch coord `V` saturates: `V = clamp(byte/255)`.
/// VERIFIED — the ocean has its **own** LUT, not a missing one. `FUN_0068c4c0` builds
/// `DAT_00c7fcd8[i] = min(i/255, 1.0)` (256 f32 entries, `fstp` @`0x68c57c`) beside the river's
/// `c81768`, and the ocean vert-fill reads it at `0x68d718 fld [depth*4 + 0xc7fcd8]` exactly as the
/// river fill reads its own at `0x68d818` — same shape, same `tc0 = (0.5, ramp[depthByte])`, only a
/// different divisor.
///
/// **The two divisors are not comparable, because the two depth bytes are not on one scale** — which
/// is what made `/255` look 6× too slow beside the river's `/42`. Measured over every MCLQ block in
/// Azeroth + Kalimdor (`examples/liquid_depth_census`, 17.4 M ocean and 0.55 M river wet vertices,
/// each byte paired against its own geometric depth `surface − terrain`):
///
/// | | authored ramp | byte 255 is | V saturates at |
/// |---|---|---|---|
/// | river/lake | 8.96 byte/yd | 28 yd | byte 42 ≈ **4.7 yd** |
/// | ocean | 1.72 byte/yd | 148 yd | byte 255 ≈ **148 yd** |
///
/// The artists authored the sea's byte **5.2× gentler per yard**, so the 6× gentler LUT lands the two
/// ramps within ~15 % of each other in *yards of depth per unit of V*. `/42` on the ocean would be
/// the correction applied twice. It is also not a subtle difference: **83.4 % of all ocean wet
/// vertices are pinned at byte 255** — the open sea is authored fully deep, and the whole ramp is a
/// shore band. (The river control reproduces its own independently-verified ≈8.5 byte/yd, which is
/// what says the census measures what it claims to.)
const OCEAN_DEPTH_V_SATURATION: f32 = 255.0;

/// `WOW_OCEAN_DEPTH_DIV=<divisor>` — override [`OCEAN_DEPTH_V_SATURATION`] for one run.
///
/// The ocean depth ramp is 97 % of the water in the world and its whole visible extent is the shore
/// band, so "is this the right curve" is a question about a beach, and a **look** question — the
/// director's to call, not ours to grade from a screenshot. This is the A/B: `WOW_OCEAN_DEPTH_DIV=42`
/// puts the river's ramp on the sea (deep by ~24 yd of depth instead of ~148), so the two can be run
/// side by side against the reference client at the same shoreline. Read once, at first use.
fn ocean_depth_v_divisor() -> f32 {
    static DIV: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *DIV.get_or_init(|| {
        std::env::var("WOW_OCEAN_DEPTH_DIV")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|d| *d > 0.0)
            .unwrap_or(OCEAN_DEPTH_V_SATURATION)
    })
}

/// Build a flat liquid surface mesh from one parsed MCLQ **block** at MCNK origin `position` (the
/// chunk header's raw WoW `[x, y, z]`). A chunk can hold more than one block — see [`MclqChunk`].
///
/// `None` when the block has no wet cell, or when its cells are slime, which the reference has no
/// ADT queue for ([`LiquidKind::from_adt_nibble`]).
pub(crate) fn build_liquid_mesh(mclq: &MclqChunk, position: [f32; 3]) -> Option<LiquidMesh> {
    if mclq.vertices.len() < GRID * GRID || mclq.tile_flags.len() < CELLS * CELLS {
        return None;
    }

    // ── Cells first, because the CELLS decide the kind. ──
    // 2 tris per wet cell. Skip dry cells (low nibble 0xf) and any cell with a sentinel-height
    // corner (belt-and-suspenders).
    let mut indices = Vec::with_capacity(CELLS * CELLS * 6);
    let mut wet = vec![false; CELLS * CELLS];
    // Wet-cell nibble tally → the block's majority nibble, which is BOTH its render kind and its
    // sound class (`LiquidMesh::sound_nibble`). Counted over genuinely-wet cells only, so it
    // describes what is actually drawn.
    let mut nibble_counts = [0u32; 16];
    for row in 0..CELLS {
        for col in 0..CELLS {
            let flag = mclq.tile_flags[row * CELLS + col] & 0x0f;
            if flag == DRY_NIBBLE {
                continue;
            }
            let tl = (row * GRID + col) as u32;
            let tr = tl + 1;
            let bl = ((row + 1) * GRID + col) as u32;
            let br = bl + 1;
            // Guard against a wet cell whose corner is the sentinel/NaN (data anomaly) — check the
            // ORIGINAL heights, not the sanitized `positions`. Wet cells normally carry real heights.
            if [tl, tr, bl, br].iter().any(|&i| {
                let raw = mclq.vertices[i as usize].height;
                !raw.is_finite() || raw.abs() >= HEIGHT_SENTINEL
            }) {
                continue;
            }
            nibble_counts[flag as usize] += 1;
            // The cell is liquid AND its corners are real — the same gate the triangles pass, so
            // the grid the queries read and the mesh the renderer draws can never disagree.
            wet[row * CELLS + col] = true;
            indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
        }
    }
    if indices.is_empty() {
        return None;
    }

    // The kind comes from the CELL NIBBLE — the VERIFIED type source (`0x6ba970`/`0x68d9b0`) — not
    // from the MCNK header bits, whose bit→type ordering was never byte-proven and which mislabelled
    // every two-block river-mouth chunk as ocean. In shipped data a block's wet cells are of one
    // class throughout, so the majority is the block; the tally is what breaks a tie.
    let sound_nibble = nibble_counts
        .iter()
        .enumerate()
        .max_by_key(|(_, &c)| c)
        .map(|(n, _)| n as u8)
        .unwrap_or(0);
    let kind = LiquidKind::from_adt_nibble(sound_nibble)?;

    // Depth-byte → swatch coord `V` divisor. BOTH are VERIFIED, and they are two LUTs built side by
    // side in `FUN_0068c4c0`, not one law and one stand-in: river/lake `c81768` = `min(d/42, 1)` read
    // by the river fill `FUN_0068d790`, ocean `c7fcd8` = `min(d/255, 1)` read by the ocean fill
    // `FUN_0068d690`. They differ because the two authored depth bytes are on different scales — see
    // [`OCEAN_DEPTH_V_SATURATION`] for the census that measures both.
    let depth_v_div = match kind {
        LiquidKind::Ocean => ocean_depth_v_divisor(),
        _ => RIVER_DEPTH_V_SATURATION,
    };
    // Magma reads its UVs off the vertex and its depth off nothing at all (see `LiquidMesh::uvs` /
    // `::depths`); every other kind generates cell-index UVs and ramps by the depth byte.
    let authored_uvs = kind == LiquidKind::Magma;
    let [wx, wy, _wz] = position;

    // 81 grid verts in raw WoW coords. Linear index `n`: row = n/9 (steps south, −X), col = n%9
    // (steps east, −Y) — the same orientation as the MCVT outer grid, so the water lattice lines
    // up with the terrain it sits in. Z is the MCLQ per-vertex absolute world height. Snap X/Y to
    // the global lattice exactly as terrain does.
    let mut positions = Vec::with_capacity(GRID * GRID);
    let mut uvs = Vec::with_capacity(GRID * GRID);
    let mut depths = Vec::with_capacity(GRID * GRID);
    for n in 0..GRID * GRID {
        let row = (n / GRID) as f32;
        let col = (n % GRID) as f32;
        // Dry/no-liquid verts carry the FLT_MAX (0x7F7FFFFF) sentinel (and `is_finite()` is TRUE for
        // it). They're never indexed (only wet cells emit tris), but they stay in `positions`, so a
        // raw sentinel would blow the mesh AABB out to ~1e38 — which makes Bevy's visibility culling
        // drop the whole chunk (the "partial water chunks vanish, fully-wet ocean is fine" bug, since
        // only mixed-wet/dry chunks contain sentinels). Substitute the chunk's `min_height` for any
        // non-finite/sentinel height so the AABB stays tight; the value is invisible (unreferenced).
        let raw = mclq.vertices[n].height;
        let h = if raw.is_finite() && raw.abs() < HEIGHT_SENTINEL {
            raw
        } else {
            mclq.min_height
        };
        positions.push([
            snap_to_lattice(wx - row * UNIT_SIZE),
            snap_to_lattice(wy - col * UNIT_SIZE),
            h,
        ]);
        if authored_uvs {
            // `u = (s as i32 as f64 · 3/256) as f32` — VERIFIED bit-exact `0x68d890`. Unsigned: the
            // reference widens the u16 to i32, so 0xffff is 65535, never −1.
            let [s, t] = mclq.vertices[n].texcoords();
            let scale = f64::from(MAGMA_TEXCOORD_SCALE);
            uvs.push([
                ((f64::from(s)) * scale) as f32,
                ((f64::from(t)) * scale) as f32,
            ]);
            depths.push(0.0);
        } else {
            uvs.push([col * 0.25, row * 0.25]);
            // SVert depth byte (water/ocean: union_data[0]) → swatch coord V (0..1). River/lake saturate
            // at byte 42 (`clamp(byte/42)`, VERIFIED `c81768`) so the channel middle hits the deep/teal
            // swatch row; ocean at /255 (see `depth_v_div`). The SAME V drives both the body-colour depth
            // band and the opacity ramp on the shader side (one swatch row → colour + alpha).
            depths.push((mclq.vertices[n].depth_byte() as f32 / depth_v_div).clamp(0.0, 1.0));
        }
    }

    Some(LiquidMesh {
        grid: [GRID as u32, GRID as u32],
        shared: vec![false; wet.len()],
        wet,
        positions,
        uvs,
        depths,
        indices,
        sound_nibble,
        material_id: None, // ADT liquid has no MOMT to index
        kind,
    })
}

impl LiquidMesh {
    /// Drop the cells `keep` rejects (indexed like [`Self::wet`]), rebuilding [`Self::indices`].
    /// The vertex arrays are left alone — a vertex no triangle references costs nothing to keep and
    /// the grid stays addressable by cell index.
    ///
    /// This is how the MLIQ [`Self::shared`] gate is applied once the owning model can see every
    /// group at once.
    pub fn retain_cells(&mut self, keep: impl Fn(usize) -> bool) {
        let (cols, rows) = (self.grid[0] as usize, self.grid[1] as usize);
        let (xt, yt) = (cols.saturating_sub(1), rows.saturating_sub(1));
        let mut dropped = false;
        for c in 0..self.wet.len() {
            if self.wet[c] && !keep(c) {
                self.wet[c] = false;
                dropped = true;
            }
        }
        if !dropped {
            return;
        }
        self.indices.clear();
        for ty in 0..yt {
            for tx in 0..xt {
                if !self.wet[ty * xt + tx] {
                    continue;
                }
                let tl = (ty * cols + tx) as u32;
                let (tr, bl) = (tl + 1, ((ty + 1) * cols + tx) as u32);
                let br = bl + 1;
                self.indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use benilla_adt::{parse_adt, ParsedAdt};

    use super::*;

    /// Every liquid mesh a real tile builds, with the MCNK origin each came from.
    fn tile_liquids(chain: &mut crate::Chain, map: &str, tx: u32, ty: u32) -> Vec<LiquidMesh> {
        let bytes = chain
            .read_file(&format!("World\\Maps\\{map}\\{map}_{tx}_{ty}.adt"))
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");
        root.mcnk_chunks
            .iter()
            .flat_map(|m| {
                m.liquids
                    .iter()
                    .filter_map(|lq| build_liquid_mesh(lq, m.header.position))
            })
            .collect()
    }

    /// Whether a mesh's **wet** footprint covers a raw WoW `(x, y)`, and at what surface height —
    /// the containment the swim/submersion queries do, done here off the grid directly.
    fn wet_at(m: &LiquidMesh, x: f32, y: f32) -> Option<f32> {
        let cols = m.grid[0] as usize;
        for (c, _) in m.wet.iter().enumerate().filter(|(_, &w)| w) {
            let (row, col) = (c / (cols - 1), c % (cols - 1));
            let corners = [
                row * cols + col,
                row * cols + col + 1,
                (row + 1) * cols + col,
                (row + 1) * cols + col + 1,
            ];
            let xs: Vec<f32> = corners.iter().map(|&i| m.positions[i][0]).collect();
            let ys: Vec<f32> = corners.iter().map(|&i| m.positions[i][1]).collect();
            let (x0, x1) = (
                xs.iter().cloned().fold(f32::MAX, f32::min),
                xs.iter().cloned().fold(f32::MIN, f32::max),
            );
            let (y0, y1) = (
                ys.iter().cloned().fold(f32::MAX, f32::min),
                ys.iter().cloned().fold(f32::MIN, f32::max),
            );
            if (x0..=x1).contains(&x) && (y0..=y1).contains(&y) {
                return Some(corners.iter().map(|&i| m.positions[i][2]).sum::<f32>() / 4.0);
            }
        }
        None
    }

    /// **B21 — "there should be lava here", `.go xyz -7845.99 -1065.50 123.60 0`.**
    ///
    /// Open-world lava is MCLQ magma, and the ADT mesher used to return `None` for it, so every
    /// outdoor lava surface in the game was silently absent while Blackrock's (WMO MLIQ) rendered
    /// fine — exactly the shape of the report. The pin is Burning Steppes, tile 33_46.
    ///
    /// This is the whole of it: a sweep of all 12 shipped terrain maps finds MCLQ magma on five
    /// tiles only — 124 chunks across Burning Steppes/Searing Gorge and 4 in Un'Goro — and no MCLQ
    /// slime anywhere at all.
    #[test]
    fn b21_burning_steppes_lava_builds_a_magma_surface_at_the_reported_pin() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let meshes = tile_liquids(&mut chain, "Azeroth", 33, 46);

        let magma: Vec<&LiquidMesh> = meshes
            .iter()
            .filter(|m| m.kind == LiquidKind::Magma)
            .collect();
        assert_eq!(magma.len(), 64, "Burning Steppes tile 33_46 magma chunks");

        // The reported spot itself is wet, with lava — not merely "somewhere on this tile".
        let (px, py, pz) = (-7845.99f32, -1065.50, 123.60);
        let hit = magma
            .iter()
            .find_map(|m| wet_at(m, px, py))
            .expect("the reported pin stands on a magma surface");
        assert!(
            (hit - pz).abs() < 6.0,
            "lava surface {hit} sits at the reported eye height {pz}"
        );

        // Fullbright + opaque, and carrying the lava-flow sound class (nibble 6: class 2 = magma,
        // FluidSpeed 4 → LavaFlowLoop) — the whole magma contract, not just a mesh that exists.
        assert!(magma.iter().all(|m| m.kind.is_fullbright()));
        assert!(
            magma.iter().all(|m| m.sound_nibble == 6),
            "shipped ADT magma is nibble 6 throughout"
        );
        // No ADT slime exists to build, and the reference has no queue for it if it did.
        assert!(
            !meshes.iter().any(|m| m.kind == LiquidKind::Slime),
            "slime never renders from an ADT"
        );
    }

    /// Lava's UVs are **authored per vertex**, not generated — and authored *world-continuously*, so
    /// a chunk's east edge repeats its neighbour's west edge exactly. That continuity is the check
    /// that the `u16 → ·3/256` read (VERIFIED `0x68d890`) is being applied to the right bytes in the
    /// right order: get the scale wrong and it still tiles, get the *field* wrong and the seam shows.
    #[test]
    fn magma_uvs_come_from_the_vertex_and_run_continuous_across_chunks() {
        let data = crate::wow_data_or_skip!();
        if !data.is_dir() {
            return;
        }
        let mut chain = crate::open_chain(&data).expect("open chain");
        let magma: Vec<LiquidMesh> = tile_liquids(&mut chain, "Azeroth", 33, 46)
            .into_iter()
            .filter(|m| m.kind == LiquidKind::Magma)
            .collect();

        // Not the generated ¼-per-cell field: authored steps are ≈42 units ≈ 0.49 UV per cell.
        let stepped = magma.iter().any(|m| {
            let du = (m.uvs[1][0] - m.uvs[0][0]).abs();
            du > 0.001 && (du - 0.25).abs() > 0.05
        });
        assert!(
            stepped,
            "magma UVs are authored, not the water cell-index UVs"
        );

        // Two chunks adjacent along −Y: the west one's col-8 vertex column IS the east one's col-0.
        let mut shared = 0;
        for a in &magma {
            for b in &magma {
                let (pa, pb) = (a.positions[8], b.positions[0]);
                if (pa[0] - pb[0]).abs() < 0.01 && (pa[1] - pb[1]).abs() < 0.01 {
                    for row in 0..9 {
                        let (ua, ub) = (a.uvs[row * 9 + 8], b.uvs[row * 9]);
                        // Sentinel-height (dry) verts carry no authored coord — skip those.
                        if ua == [0.0, 0.0] || ub == [0.0, 0.0] {
                            continue;
                        }
                        assert!(
                            (ua[0] - ub[0]).abs() < 1e-4 && (ua[1] - ub[1]).abs() < 1e-4,
                            "shared lava edge vertex must carry one UV: {ua:?} vs {ub:?}"
                        );
                        shared += 1;
                    }
                }
            }
        }
        assert!(shared > 0, "no shared lava chunk edge found to check");
    }

    /// **The sweep's second find: an MCNK can carry more than one MCLQ block**, one per set liquid
    /// header bit (the MCLQ walk `0x6af7a3`). 28 shipped Azeroth chunks are river mouths that
    /// carry two — the stream *and* the sea beneath it — and reading only the first dropped the sea
    /// while the old header-bit priority ("ocean wins") labelled the river block ocean.
    ///
    /// Hillsbrad's tile 33_33 is one. Both blocks must survive, each with its own kind and height.
    #[test]
    fn a_river_mouth_chunk_builds_both_its_river_and_its_sea() {
        let data = crate::wow_data_or_skip!();
        if !data.is_dir() {
            return;
        }
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Maps\\Azeroth\\Azeroth_33_33.adt")
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");

        let two_block: Vec<&benilla_adt::McnkChunk> = root
            .mcnk_chunks
            .iter()
            .filter(|m| m.liquids.len() > 1)
            .collect();
        assert!(
            !two_block.is_empty(),
            "Azeroth_33_33 carries river-mouth chunks with two liquid blocks"
        );

        for mcnk in two_block {
            assert_eq!(mcnk.liquids.len(), 2, "two set liquid bits ⇒ two blocks");
            let built: Vec<LiquidMesh> = mcnk
                .liquids
                .iter()
                .filter_map(|lq| build_liquid_mesh(lq, mcnk.header.position))
                .collect();
            assert_eq!(built.len(), 2, "both blocks mesh");
            // Disk order is header-bit order: bit 2 (river) then bit 3 (ocean) — and the KIND comes
            // from each block's own cell nibble, so the river no longer reads as ocean.
            assert_eq!(built[0].kind, LiquidKind::Still, "block 0 is the stream");
            assert_eq!(built[1].kind, LiquidKind::Ocean, "block 1 is the sea");
            let sea = built[1].positions[built[1].indices[0] as usize][2];
            let river = built[0].positions[built[0].indices[0] as usize][2];
            assert!(
                river > sea + 1.0,
                "the stream ({river}) sits above sea level ({sea})"
            );
            // **The invariant that makes spawning both safe at a river mouth.**
            // `liquid::liquid_at` resolves overlapping footprints by taking the LOWEST surface
            // (decision 0634/0701 — the volume you are actually in when two are stacked). If a
            // river cell and an ocean cell covered the same XY, the sea at z≈0 would win under a
            // stream 5 yd above it and the player would read as dry while standing in the water.
            // At every real river mouth the two blocks are authored **disjoint** — each cell
            // belongs to exactly one — so they never compete for a position.
            //
            // Swept game-wide: 28 two-block chunks, and the only 4 whose cells overlap are in
            // Kalimdor tile 26_54 (a flat −23.47 sheet under a full-chunk sea at 0.0, in the dead
            // water off the south-west corner). That patch read the same way *before* multi-block
            // parsing — the first block was all there was — so it is a named residual of the
            // lowest-wins rule, not a regression. See the decision record.
            for (cell, (&r, &o)) in built[0].wet.iter().zip(&built[1].wet).enumerate() {
                assert!(
                    !(r && o),
                    "cell {cell} is wet in BOTH blocks — the stacked-surface case liquid_at \
                     resolves lowest-wins, which would read this stream as dry"
                );
            }
        }
    }

    /// Golden test against a real Elwynn tile: Crystal Lake area `Azeroth_32_48.adt` has 42 MCLQ
    /// water chunks (all river/still, surface ≈ 143.99 yd). Skips when the client isn't present.
    /// Every liquid mesh a real tile builds, **paired with the raw MCLQ block it came from** — so a
    /// depth assertion can be made against the bytes on disk rather than against the mesh's own
    /// arithmetic run backwards.
    fn tile_liquids_with_source(
        chain: &mut crate::Chain,
        map: &str,
        tx: u32,
        ty: u32,
    ) -> Vec<(benilla_adt::MclqChunk, LiquidMesh)> {
        let bytes = chain
            .read_file(&format!("World\\Maps\\{map}\\{map}_{tx}_{ty}.adt"))
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");
        root.mcnk_chunks
            .iter()
            .flat_map(|m| {
                m.liquids.iter().filter_map(|lq| {
                    build_liquid_mesh(lq, m.header.position).map(|mesh| (lq.clone(), mesh))
                })
            })
            .collect()
    }

    /// **The two depth divisors are two verified LUTs, not one law and one stand-in.** The ocean's
    /// `/255` spent months labelled a placeholder on a misattributed address (`0x68d890` — the
    /// *magma* vert-fill), so this pins both against real shipped data: an ocean tile off Baradin
    /// Bay whose depth bytes run the full ramp, and Elwynn's rivers as the control.
    ///
    /// The V assertions read the **raw MCLQ depth byte**, never a byte recovered from the mesh's own
    /// V (that would assert the arithmetic against itself and pass under any divisor). The shore-band
    /// assertion is the one with teeth: the sea's byte is authored ~5× gentler per yard than a
    /// river's, so putting the river's `/42` on the ocean saturates a shore band that should still
    /// be ramping — which is exactly the look change this ramp is not.
    #[test]
    fn each_liquid_kind_rides_its_own_verified_depth_divisor() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // Baradin Bay: shelf water whose bytes span the ocean ramp rather than pinning at 255.
        let ocean: Vec<(benilla_adt::MclqChunk, LiquidMesh)> =
            tile_liquids_with_source(&mut chain, "Azeroth", 41, 41)
                .into_iter()
                .filter(|(_, m)| m.kind == LiquidKind::Ocean)
                .collect();
        assert!(!ocean.is_empty(), "Azeroth_41_41 carries ocean");

        let mut raw: Vec<u8> = Vec::new();
        for (src, mesh) in &ocean {
            for (n, v) in mesh.depths.iter().enumerate() {
                let byte = src.vertices[n].depth_byte();
                raw.push(byte);
                assert!(
                    (*v - f32::from(byte) / OCEAN_DEPTH_V_SATURATION).abs() < 1e-4,
                    "ocean V is the raw byte over 255 (VERIFIED `c7fcd8`); byte {byte} gave V={v}"
                );
            }
        }

        // The shore band exists and is NOT saturated — the whole point of the gentler divisor. Under
        // `/42` every one of these mid-band verts would already read the deep row.
        let mid = raw.iter().filter(|&&b| (42..=200).contains(&b)).count();
        let pinned = raw.iter().filter(|&&b| b == u8::MAX).count();
        assert!(
            mid > 100,
            "this tile must carry a real shore ramp to test, saw {mid} mid-band verts"
        );
        assert!(
            mid * 4 > pinned,
            "the `/42` divisor would flatten a shore band this tile spends {mid} verts ramping \
             through (against {pinned} genuinely pinned-deep) — the look change the ocean ramp is not"
        );

        // Control: rivers/lakes DO ride `/42`, and Elwynn's channel middles reach the deep row.
        let river: Vec<(benilla_adt::MclqChunk, LiquidMesh)> =
            tile_liquids_with_source(&mut chain, "Azeroth", 32, 48)
                .into_iter()
                .filter(|(_, m)| m.kind == LiquidKind::Still)
                .collect();
        assert!(!river.is_empty(), "Azeroth_32_48 carries river/lake water");
        let mut saturated_below_255 = false;
        for (src, mesh) in &river {
            for (n, v) in mesh.depths.iter().enumerate() {
                let byte = src.vertices[n].depth_byte();
                let want = (f32::from(byte) / RIVER_DEPTH_V_SATURATION).clamp(0.0, 1.0);
                assert!(
                    (*v - want).abs() < 1e-4,
                    "river V is clamp(byte/42) (VERIFIED `c81768`); byte {byte} gave V={v}"
                );
                // The teeth on the river side: a byte well under 255 already reads fully deep,
                // which `/255` could never produce.
                saturated_below_255 |= (42..200).contains(&byte) && *v >= 0.999;
            }
        }
        assert!(
            saturated_below_255,
            "a river channel middle saturates to the deep row by byte 42, not byte 255"
        );
    }

    #[test]
    fn parses_elwynn_lake_tile() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Maps\\Azeroth\\Azeroth_32_48.adt")
            .expect("read Azeroth_32_48.adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");

        let mut meshes = Vec::new();
        for mcnk in &root.mcnk_chunks {
            for mclq in &mcnk.liquids {
                if let Some(m) = build_liquid_mesh(mclq, mcnk.header.position) {
                    meshes.push(m);
                }
            }
        }

        // 42 water chunks on this tile, every one still river water.
        assert_eq!(
            meshes.len(),
            42,
            "expected 42 liquid chunks on Azeroth_32_48"
        );
        assert!(
            meshes.iter().all(|m| m.kind == LiquidKind::Still),
            "Crystal Lake is all still water"
        );

        for m in &meshes {
            assert_eq!(m.positions.len(), 81, "9×9 grid");
            assert_eq!(m.uvs.len(), 81, "uv per vertex");
            assert_eq!(m.depths.len(), 81, "depth per vertex");
            // EVERY vertex (even unreferenced dry-cell verts) must be finite + sane, or the mesh AABB
            // blows up (dry verts carry the FLT_MAX sentinel) and Bevy frustum-culls the whole chunk —
            // the "partial water chunks vanish, fully-wet ocean is fine" bug.
            for p in &m.positions {
                assert!(
                    p[2].is_finite() && p[2].abs() < 10_000.0,
                    "all liquid verts sane for a clean AABB, got z={}",
                    p[2]
                );
            }
            assert!(!m.indices.is_empty(), "at least one wet cell");
            assert_eq!(m.indices.len() % 6, 0, "2 tris per wet cell");
            // Indices in range, and every referenced vertex has a finite, sane height — no
            // FLT_MAX sentinel leaked in (the dry-cell gate worked). Elwynn water bodies sit at
            // various elevations (Crystal Lake ≈ 144 yd, streams lower), all in a plausible band.
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            for &i in &m.indices {
                let z = m.positions[i as usize][2];
                assert!(
                    z.is_finite() && z.abs() < 10_000.0,
                    "referenced height {z} sane"
                );
                assert!(
                    (50.0..300.0).contains(&z),
                    "Elwynn water elevation {z} plausible"
                );
            }
            // Within one chunk a water surface is near-planar (a lake; a gentle stream slopes a
            // little across the 33-yd chunk but not wildly).
            let zs: Vec<f32> = m
                .indices
                .iter()
                .map(|&i| m.positions[i as usize][2])
                .collect();
            let (zmin, zmax) = zs
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &z| (lo.min(z), hi.max(z)));
            assert!(
                zmax - zmin < 30.0,
                "per-chunk surface roughly flat ({zmin}..{zmax})"
            );
        }
    }
}
