//! The shared render types every model source (M2 batches, WMO groups) produces: [`RenderSubmesh`]
//! (one per render batch) and the material/animation bits it carries — [`ModelBlend`],
//! [`CharSkinSlot`], [`BillboardKind`]/[`Billboard`], and the billboard glow-card pulse
//! ([`BoneScaleAnim`]). No parsing lives here — just the shapes [`m2_batches`](super::m2_batches) and
//! [`wmo`](super::wmo) both build.

use super::key_anim::SeqLoops;
use super::mat_anim::{AlphaAnim, RgbAnim};
use super::tex_anim::UvAnim;

/// How a submesh blends — maps the model's blend mode to the renderer's alpha handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelBlend {
    /// Fully opaque (trunks, walls, floors).
    Opaque,
    /// Alpha-tested cutout (leaf canopies, fences, windows).
    AlphaTest,
    /// Alpha-blended / additive.
    Blend,
    /// Multiplicative — `out = src·dst` (GL `DST_COLOR/ZERO`; `0x811fe0`: M2 mode 5 → EGxBlend 4;
    /// WMO MOMT mode 4, direct index). Darkens/tints
    /// what's already drawn; the blend equation reads NO alpha, so these batches cannot alpha-fade.
    Mod,
    /// 2× multiplicative — `out = 2·src·dst` (GL `DST_COLOR/SRC_COLOR`; M2 mode 6 → EGxBlend 5; WMO
    /// mode 5, direct index). Neutral at mid-grey, brightens above it: the weapon/armor ARMORREFLECT
    /// sheen layer. Same no-alpha law as [`Self::Mod`].
    Mod2x,
}

/// A character body texture the client supplies at runtime from the player's appearance — the M2
/// texture record carries the *type* but no embedded filename, so the spawn site swaps a per-player
/// material onto the batches that carry the slot.
/// - `Body` (M2 texture type 1) — the composited body atlas: base skin + face/facial-hair/pelvis
///   overlays.
/// - `Hair` (M2 texture type 6) — the hair-mesh texture, a single CharSections `sectionType 3`
///   `TextureName[0]` BLP keyed by hairStyle + hairColor.
/// - `Object` (M2 texture type 2) — the runtime-bound **object skin**: on a weapon/shield M2 this is
///   the item's own texture (`ItemDisplayInfo`'s model-texture column); on a
///   character body model this same type is the cape slot. Which meaning applies depends on the
///   model wearing the batch, not on the type alone — the spawn site resolves it per-consumer.
/// - `SkinExtra` (M2 texture type 8) — the standalone **extra skin** BLP: CharSections `sectionType 0`
///   `TextureName[1]` keyed by skinColor (`…Skin00_NN_Extra.blp`), loaded plain (never composited —
///   the client's dedicated extra-texture loader is a bare TextureCreate). Only fur races author it:
///   the tauren body binds its head/leg fur batches to this slot instead of the body atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharSkinSlot {
    Body,
    Hair,
    Object,
    SkinExtra,
}

/// How a billboarded bone tracks the camera (M2 bone billboard flags). The cylindrical arms keep
/// one authored axis and rebuild the in-plane pair from the camera — WHICH axis is kept is part
/// of the authored look (the frost-armor sheets ride a lock-Z bone so they stay upright while
/// spinning to face the viewer), so the three lock bits stay distinct here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillboardKind {
    /// Spherical (`0x08`) — faces the camera fully (glow cards, coronae).
    Spherical,
    /// Cylindrical lock-X (`0x10`) — keeps the bone's X axis (chains, ropes).
    LockX,
    /// Cylindrical lock-Y (`0x20`).
    LockY,
    /// Cylindrical lock-Z (`0x40`) — keeps model up (the questgiver `?` marker, the frost-armor
    /// sheets): spins about the vertical to face the viewer.
    LockZ,
}

impl BillboardKind {
    /// The kind an M2 bone's flag word authors, `None` for an ordinary bone — THE mapping, shared
    /// by the batch splitter and the skeleton bake. `0x08` wins over a (never-seen) combined lock
    /// bit, matching the palette replacement's own dispatch order.
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits & 0x08 != 0 {
            Some(Self::Spherical)
        } else if bits & 0x10 != 0 {
            Some(Self::LockX)
        } else if bits & 0x20 != 0 {
            Some(Self::LockY)
        } else if bits & 0x40 != 0 {
            Some(Self::LockZ)
        } else {
            None
        }
    }
}

/// How a bone's **effective parent matrix** is rewritten before it composes — M2 bone flag bits
/// `0x1/0x2/0x4`, the standard `ignore parent translate / scale / rotate` trio.
///
/// The reference tests `flags & 7` on every non-root bone (`0x714961`) and, when any
/// of the three is set, takes a ~950-byte arm (`0x71496d`–`0x714d0c`) that rebuilds that bone's
/// parent matrix out of the **model's own root matrix** — pivot-preserved — *and then falls into
/// the billboard selector unchanged*. It is not an escape hatch from anything: it changes the
/// INPUT the billboard law is applied to.
///
/// The three legs are proven from the binary, matching the standard names: `0x1` at `0x714c92`,
/// `0x2` at `0x714bdb`, `0x4` at `0x714a6e` alone / `0x714a18` combined with `0x2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentArm {
    /// `0x1` — the bone's frame is placed at the model root's origin instead of at the pivot its
    /// animated parent carried it to.
    pub ignore_translate: bool,
    /// What `flags & 0x6` does to the parent's basis.
    pub basis: ParentBasis,
}

/// The `flags & 0x6` leg of [`ParentArm`] — what happens to the parent matrix's three basis
/// vectors. (The reference works in row-major/row-vector form, so its "row K" is our column K:
/// both name the image of model basis vector K, which is what every leg operates on.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentBasis {
    /// `flags & 6 == 0` (so `flags & 7 == 1` alone): the parent's basis is kept as-is.
    Keep,
    /// `flags & 6 == 2` — ignore parent scale: each basis vector is normalized, direction kept.
    UnitNormalize,
    /// `flags & 6 == 4` — ignore parent rotation: the ROOT's direction with the parent's
    /// magnitude (`0x714a6e`'s per-axis `|P_k| / |R_k|` ratio).
    RootDirection,
    /// `flags & 6 == 6` — ignore parent rotation AND scale: the root's basis outright
    /// (`0x714a18`). Every vanilla player mount's rider seat is this or [`Self::RootDirection`],
    /// which is why the saddle translates with the gallop and never rotates.
    RootBasis,
}

impl ParentArm {
    /// The arm an M2 bone's flag word authors, `None` when `flags & 7 == 0` (the ordinary bone,
    /// which goes straight to the billboard selector — `je 0x714d0f`).
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits & 0x7 == 0 {
            return None;
        }
        Some(Self {
            ignore_translate: bits & 0x1 != 0,
            basis: match bits & 0x6 {
                0x2 => ParentBasis::UnitNormalize,
                0x4 => ParentBasis::RootDirection,
                0x6 => ParentBasis::RootBasis,
                _ => ParentBasis::Keep,
            },
        })
    }
}

/// A bone **scale track driven by a global sequence** — the looping "breathe" pulse a glow card rides.
/// VERIFIED mechanism: the Lamppost glow card sits on a spherical-billboard bone whose scale track
/// (`interp=1`, `gseq=0`) oscillates `0.86 … 1.04` over the model's 1333 ms global sequence, so the
/// card grows/shrinks — read as a brightness pulse. The clock is the kernel's global-sequence rule
/// `0x714352`: `gseq_time = elapsed_ms % duration`, then a linear lerp between adjacent keys. Only
/// **global-sequence** tracks are captured here (they loop with zero arming, the static-doodad case);
/// a `gseq == 0xffff` track needs the armed-animation machinery we don't run for static props.
#[derive(Debug, Clone)]
pub struct BoneScaleAnim {
    /// Global-sequence loop length (ms). Sampling wraps at this period.
    pub duration_ms: u32,
    /// Linear interpolation between keys (`interp != 0`); `false` = step.
    pub interp: bool,
    /// `(timestamp_ms, scale_xyz)` keyframes, in file order (ascending time).
    pub keys: Vec<(u32, [f32; 3])>,
}

impl BoneScaleAnim {
    /// Sample the scale at `time_ms` (wrapped into `[0, duration_ms)`), linearly interpolating between
    /// the bracketing keys (or stepping when `interp == false`). Mirrors the key search
    /// `0x713d50` + the linear sampler leg for the static-doodad / global-sequence case.
    pub fn sample(&self, time_ms: u32) -> [f32; 3] {
        let n = self.keys.len();
        if n == 0 {
            return [1.0; 3];
        }
        let t = time_ms % self.duration_ms.max(1);
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        // Last key whose timestamp is ≤ t (keys are time-ascending).
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1; // step, or clamp past the final key
        }
        let (t0, v0) = self.keys[k];
        let (t1, v1) = self.keys[k + 1];
        let frac = if t1 > t0 {
            (t - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        [
            v0[0] + (v1[0] - v0[0]) * frac,
            v0[1] + (v1[1] - v0[1]) * frac,
            v0[2] + (v1[2] - v0[2]) * frac,
        ]
    }
}

/// A bone that **spins rigidly**: a parentless bone whose armed sequence keys ROTATION only, so every
/// vertex weighted wholly to it moves as a rigid body about the pivot — `T(pivot) · R(t) · T(−pivot)`
/// — with no skinning palette and no joint hierarchy involved.
///
/// This is the same "rigidly separable bone ⇒ per-batch transform" reading the billboard card split
/// already makes ([`Billboard`], and `separable_billboard_bones`' two conditions), applied to an
/// authored rotation instead of a camera-facing one. It is a **narrow** claim on purpose, and every
/// clause is load-bearing:
///
/// - **Parentless.** With a parent, the bone's world motion is its parent's chain composed with its
///   own, which is not a single rigid transform unless the whole chain is static. A parented bone is
///   simply not collected — it holds its bind pose, the conservative failure.
/// - **Rotation only.** A translation or scale track on the same bone would still be rigid, but the
///   pivot-conjugated form below would be wrong, so those bones are not collected either.
/// - **Wholly-weighted geometry** is the caller's half: a vertex split between this bone and another
///   is half a rigid body's worth of motion, which no rigid transform can produce.
///
/// The population this exists for: `CavernsOfTimeSky.m2`'s three asteroid belts (bones 1/2/3, each
/// parentless, each keying rotation alone over one 66.667 s loop — 25°, 90° and a full 360°), whose
/// four batches are each `[bone@1.00]` across every vertex (`benilla-extract m2batch`).
#[derive(Debug, Clone, PartialEq)]
pub struct BoneSpin {
    /// The bone's pivot, model space (WoW axes) — the rotation centre.
    pub pivot: [f32; 3],
    /// The armed sequence's duration (**seconds**). Sampling wraps at this period.
    pub duration: f32,
    /// Linear interpolation between keys; `false` = step (the track's `interp_type == 0`). Read from
    /// the rotation track's own header word because [`BoneKeys`] — this collector's source for the
    /// keys themselves — deliberately drops it, and a step track lerped is a real divergence
    /// (`benilla-extract bonescan`'s recorded residual). There is no reason to reproduce that here.
    pub interp: bool,
    /// `(seconds, [x, y, z, w])` keyframes, rebased to the sequence start, time-ascending. The
    /// quaternion is raw WoW model space — conjugate it into the render basis at the use site.
    pub keys: Vec<(f32, [f32; 4])>,
}

impl BoneSpin {
    /// Sample the rotation at `time` seconds (wrapped into `[0, duration)`), **slerping** between the
    /// bracketing keys — the rotation analogue of [`BoneScaleAnim::sample`], with the same search,
    /// the same step leg, and the same clamp past the final key.
    ///
    /// Clamping rather than wrapping around to key 0 is what the track actually says: an M2 track's
    /// keys live inside the sequence band and the client interpolates within it, so a loop whose last
    /// key is not its first snaps at the wrap. That is authored — `CavernsOfTimeSky` bone 1 runs 0° →
    /// 25° and snaps back — and inventing a wrap-around segment would be us animating, not the file.
    pub fn sample(&self, time: f32) -> [f32; 4] {
        const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        let n = self.keys.len();
        if n == 0 {
            return IDENTITY;
        }
        // `rem_euclid` rather than `%`: a negative cursor (a clock read before the anchor) must land
        // inside the loop, not mirror onto its first key.
        let t = if self.duration > 0.0 {
            time.rem_euclid(self.duration)
        } else {
            0.0
        };
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1; // step, or clamp past the final key
        }
        let (t0, q0) = self.keys[k];
        let (t1, q1) = self.keys[k + 1];
        let frac = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        slerp(q0, q1, frac)
    }
}

/// Shortest-arc spherical interpolation of two quaternions, on plain arrays.
///
/// Hand-rolled because `benilla-formats` deliberately carries no math crate — the render basis (and
/// with it `glam`) starts at `benilla-assets`. Two guards earn their place: negating `b` when the dot
/// is negative takes the SHORT way round (without it a 360° track's second half spins backwards), and
/// falling back to a normalised lerp for near-parallel keys avoids `acos`'s vanishing `sin` denominator.
fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = (0..4).map(|i| a[i] * b[i]).sum::<f32>();
    let mut b = b;
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    let (w0, w1) = if dot > 0.9995 {
        (1.0 - t, t) // near-parallel: lerp, then renormalise below
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let mut q = [0.0f32; 4];
    for i in 0..4 {
        q[i] = a[i] * w0 + b[i] * w1;
    }
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 {
        for c in &mut q {
            *c /= len;
        }
    }
    q
}

/// A batch attached to an M2 **billboard bone**: the real client re-orients the bone to the camera
/// every frame (so the card is visible from all sides and tracks the viewer). `pivot` is the bone's
/// pivot in model space (WoW axes) — the point the card rotates about.
#[derive(Debug, Clone)]
pub struct Billboard {
    pub pivot: [f32; 3],
    /// The billboard bone's index in the model's bone table — the joint the card rides on an
    /// animated host (the swinging lamp, the mount's lights). The pivot above is this bone's.
    pub bone: u16,
    pub kind: BillboardKind,
    /// The billboard bone's global-sequence scale animation (the looping glow-card pulse), if any.
    /// `None` for a static billboard bone.
    pub scale_anim: Option<BoneScaleAnim>,
    /// The billboard bone's **translation** loops, one per sequence that keys it: `(anim id, the
    /// keys inside that sequence's band, rebased to it)`. Same keyed-Vec3-loop shape as
    /// [`BoneScaleAnim`] (values are offsets, WoW axes, not scales). The client's one-time load arm
    /// plays anim **0** (the questgiver `?` marker's bob); the marker re-arms anim **190** — the
    /// authored *raised* bob — while the unit shows an overhead name (`0x6076c0`). A sequence whose
    /// band holds ≤1 key gets no entry; a global-sequence track gets none at
    /// all. Only the **marker spawn site arms these** today; placed doodads' cards stay static-pivot
    /// (the billboard half of the 0130 phase-4 bone-ride work owns that arming).
    pub seq_translations: Vec<(u16, BoneScaleAnim)>,
}

/// A WMO group render batch's **MOBA section** — the first `transBatchCount` batches of a group are
/// TRANS, then `intBatchCount` INT, then EXT (MOGP header counts at `+0x28/+0x2a/+0x2c`, right after
/// the byte-verified portal-ref span; cross-checked against NSabbey groups 1/3, whose per-class batch
/// index counts match the reference client's observed draws batch-for-batch). The class picks an
/// interior group's lighting law (observed in a reference capture of the
/// abbey at close range): **INT draws once, UNLIT — pure `tex × MOCV`, no exterior light, no
/// points**; **TRANS draws as a per-vertex MOCV-**alpha** lerp between the day/night-lit surface and
/// that unlit bake** (the reference does it as two passes; one pass computes the same product);
/// **EXT takes the ordinary exterior day/night law**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WmoBatchClass {
    Trans,
    Int,
    Ext,
}

/// The batch's fog COLOUR policy — the per-blend fog table of the M2 batch state setter
/// (`0x70baf0`, table `DAT_811fc4` dispatched at `0x70bddf`): opaque/alpha-key/alpha fog toward
/// the scene day-night colour; Add/AddAlpha toward BLACK (an additive batch FADES with
/// distance/veil instead of adding grey — the storm-veil level-up fix); Mod toward WHITE; Mod2x
/// toward GREY-128. Render flag 0x02 (`0x70bb24`) — or a dead scene fog — disables fog outright.
/// Discriminants are the shader encoding (Scene = 0 so every non-M2 material defaults to the
/// ordinary scene fog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FogPolicy {
    #[default]
    Scene = 0,
    Black = 1,
    White = 2,
    Grey = 3,
    Off = 4,
}

/// One render batch: self-contained geometry + its material.
#[derive(Debug, Clone)]
pub struct RenderSubmesh {
    pub positions: Vec<[f32; 3]>,
    /// Authored per-vertex normals (model space, parallel to `positions`). WoW lights foliage with
    /// these soft outward normals; recomputing flat per-face normals makes crossed canopy planes
    /// catch the sun harshly. Empty only if the source had none (then the renderer recomputes).
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Texture (`.blp`) for this batch, if any. `None` for an unfilled creature skin slot — see
    /// [`Self::skin_slot`], which the spawn site uses to fill it from the display's variation.
    pub texture: Option<String>,
    /// The creature skin variation this batch draws from (`Some(0/1/2)` ⇒ `Monster1/2/3`), so a caller
    /// that loaded the model without skins (the doodad/asset-loader path) can fill the texture at spawn
    /// from `CreatureDisplayInfo`. `None` for a hardcoded/embedded texture (most M2s; all WMOs).
    pub skin_slot: Option<u8>,
    /// The batch's `skinSectionId` (geoset / mesh-part ID) — `group*100 + variant`. The character
    /// compositor selects which geosets are visible by this (hide all but the chosen hair/facial/body
    /// variants); `0` for a model with a single unnamed geoset (most creatures, all doodads/WMO).
    pub geoset_id: u16,
    /// A character body texture the client fills at runtime from the appearance ([`CharSkinSlot`]) — the
    /// M2 record carries the texture *type* but no embedded path, so the spawn site swaps a per-player
    /// material onto this batch. `None` for everything that carries its own texture (most M2s, all WMOs).
    pub char_slot: Option<CharSkinSlot>,
    pub blend: ModelBlend,
    /// Sampler address mode from the M2 texture record's `flags` (`0x1` = repeat U, `0x2` = repeat V;
    /// clear = **clamp to edge**). Not a tiling detail — a *silhouette* one. Content authors a cutout
    /// card's UVs deliberately outside `0..1` so the margin clamps to the texture's transparent border
    /// and the card fades to nothing; sampled with repeat instead, that margin wraps into the opaque
    /// middle of the sheet and draws as solid geometry with a hard seam at the wrap (
    /// bugs B52/B96). `true`/`true` for WMO, which carries its own material flags and keeps today's
    /// behaviour.
    pub wrap_x: bool,
    pub wrap_y: bool,
    /// Render both faces (no backface culling)? From the M2 material's `0x04` flag (WMO: kept `true`
    /// for now). When `false` the batch is single-sided like the real client — visible from one side.
    pub two_sided: bool,
    /// Per-vertex **skeletal skin binding**, parallel to `positions`: the 4 bone
    /// indices each vertex is weighted to. They are **global M2 bone-array indices**, used directly as
    /// joint indices (the skinned-entity path builds one joint entity per bone, in order). **Empty**
    /// for WMO and for the static doodad/GameObject mesh (which never skins); populated by the M2 path.
    pub joints: Vec<[u16; 4]>,
    /// Per-vertex skin **weights** (normalised to sum 1.0), parallel to [`Self::joints`]. Empty when
    /// `joints` is.
    pub weights: Vec<[f32; 4]>,
    /// Per-vertex **MOCV** colour (RGBA, alpha forced 1.0), parallel to `positions`. WMO groups carry
    /// it — the baked shade. On EXTERIOR groups it modulates the lit factor (`tex × MOCV × (ambient +
    /// sun·N·L)`); on INTERIOR groups (see [`Self::interior`]) it *is* the lighting — the reference runs
    /// the FFP with the directional sun off, so the baked MOCV carries the room. **Empty** for M2 (no
    /// MOCV) and for WMO groups lacking it → the renderer treats absent colour as white (no tint).
    pub vertex_colors: Vec<[f32; 4]>,
    /// This batch belongs to a WMO **interior** group. The real 1.12.1 client branches the per-group
    /// render path on `groupFlags & 0x48` (binary-verified at `0x6b3f90`): the EXTERIOR/"exterior-lit"
    /// bits `0x8 | 0x40`. With neither set (`== 0`) the group is a true interior, lit by its baked MOCV
    /// with the directional sun **off**; with either set it takes the exterior (sun N·L) path. The exact
    /// gx-layer combine is not yet byte-pinned (the gx FFP functions aren't decompiled), so the interior
    /// combine + this branch direction are confirmed *visually*, not from the binary, for now. Always
    /// `false` for M2.
    pub interior: bool,
    /// This batch is **unlit** — rendered without scene lighting so it shows at full texture
    /// brightness. For **M2** it's the material `UNLIT (0x01)` flag (lamp glass, window panes, glow
    /// cards). For **WMO** it's MOMT `UNLIT (0x01)` on an **exterior-group** batch only — the
    /// exterior drawer keys lighting per material (`rs 0xE = !(flags & 1)`, unlit ⇒ `tex × white`),
    /// while the interior drawer IGNORES the flag: lit/unlit there is dictated by the batch section
    /// alone ([`Self::wmo_batch`]). Byte law: the interior drawer `0x6b5190`, the exterior
    /// drawer `0x6b4f10`.
    pub emissive: bool,
    /// This batch's texture record is **type 14 — the icon slot**: no file of its own, filled at
    /// runtime by `Model:ReplaceIconTexture` (`0x710ec0` swaps every type-14 handle on the
    /// instance). The shipped user is `Interface\ItemAnimations\ForcedBackpackItem.m2`, the
    /// bag buttons' item-push card, whose one batch is the pushed item's icon.
    /// `false` for every other batch and all of WMO.
    pub icon_slot: bool,
    /// The MOMT **SIDN** (`0x10` — self-illum day/night) authored colour, RGB gamma bytes: the
    /// windows-glow-at-night mechanism. The real client scales it per frame by the night fraction
    /// (1 overnight, 0 all day, ramping 20:30→21:30 and 06:00→07:00) and binds it as the GL material
    /// EMISSION, which the FFP adds **inside** the lit sum before the texture modulate —
    /// `tex × (lit + sidn·night)`. A GL_LIGHTING term, so it contributes on LIT lanes only (dead on
    /// an unlit INT batch, and under `UNLIT` where lighting is off). `None` for M2 and non-SIDN WMO
    /// materials. Byte law: updater `0x6b4090`.
    pub sidn: Option<[u8; 3]>,
    /// MOMT **WINDOW** (`0x20`): in the **interior** drawer this batch swaps GL_LIGHT0 to the
    /// brighter interior pair — ambient AND diffuse = the midpoint of the Direct and Ambient
    /// day/night bands (ambient +16/255, saturating) — install → draw → restore, per batch. The
    /// exterior drawer has no WINDOW machinery, so the flag only acts on interior-group batches.
    /// This is the bright warm pane seen from inside a building (derivation `0x6d37e0`). `false`
    /// for M2.
    pub window: bool,
    /// This batch blends **additively** — its colour is *added* to the framebuffer rather than mixed
    /// with it (M2 blend mode `3` NoAlphaAdd / `4` Add). Glow cards, coronae, magic effects. Rendering
    /// these as ordinary alpha-blend lets the (cool, at night) background bleed *through* and mute the
    /// warm glow; additive preserves the authored hue. `false` for everything else (incl. all WMO for now).
    pub additive: bool,
    /// M2 render-flag **0x10 — disable depth write** for this batch. The real 1.12 client writes depth
    /// for *every* batch (opaque or transparent) **unless** this bit is set
    /// (`0x70c190`); benilla otherwise blanket-disables depth-write for the whole transparent
    /// pass, so a model's own transparent cards bleed through / flicker from some angles. `false` for WMO
    /// (its depth state is the standard opaque/transparent pass).
    pub no_depth_write: bool,
    /// M2 render-flag **0x08 — disable depth test** for this batch (it draws over everything, ignoring
    /// occlusion). Rare; `false` for WMO.
    pub no_depth_test: bool,
    /// The batch's fog COLOUR policy ([`FogPolicy`]) — `Scene` for WMO and every non-M2 batch.
    pub fog_policy: FogPolicy,
    /// Set when this batch sits on an M2 **billboard bone** — the renderer faces it to the camera each
    /// frame (around the bone pivot). `None` for ordinary geometry and all WMO batches.
    pub billboard: Option<Billboard>,
    /// This batch holds geometry bound to a billboard bone the card split **refused** — one whose mesh
    /// is welded to the rest of the model (`separable_billboard_bones`). No rigid
    /// placement of such a batch exists: the reference blends it per vertex, so a lane that wants the
    /// flap to bend must draw the **skinned** form through a joint palette. A lane
    /// that draws it static gets it whole and still. `false` for every ordinary batch — including
    /// every separable card, which is split out and faced rigidly — and for all WMO batches.
    pub welded_billboard: bool,
    /// The batch's **animated material alpha** (decision 0130 phase 2): its time-varying colour-alpha
    /// and/or transparency-weight loops, baked by [`mat_anim`](super::mat_anim). `None` when both
    /// factors are static (baked/culled at build) — the overwhelming majority. The runtime multiplies
    /// the sampled value into the instance's render alpha, per the reference's combine
    /// (`0x707680`).
    pub alpha_anim: Option<AlphaAnim>,
    /// The batch's **UV-animation** loop (decision 0130 phase 3): the texture transform's
    /// translation track baked by [`tex_anim`](super::tex_anim) — the raw `(x, y)` offset over the loop
    /// clock. `None` for the ~98.6% of models with no texture transform, and for all WMO batches.
    pub uv_anim: Option<UvAnim>,
    /// The batch's UV loop **per file sequence slot**, carried ONLY when the slots disagree — i.e.
    /// when [`Self::uv_anim`]'s single loop cannot be right for every instance, because which loop
    /// applies depends on which sequence that instance is playing (bug B98: the BRM
    /// lava bubbles key their whole flipbook inside the 50 %-weighted variation 1, so slot 0 — the
    /// only slot the shared-material registry can read — is a dead hold). `None` for every batch
    /// whose slots agree, which is the shared lane unchanged.
    pub uv_seq: Option<SeqLoops<[f32; 2]>>,
    /// The batch's texture-transform **rotation** loop per file sequence slot — the raw
    /// quaternion keys ([`tex_anim::bake_uv_rot_seqs`](super::tex_anim::bake_uv_rot_seqs),
    /// decision 2019), for the lanes that own a material per instance (the UI model tiles;
    /// the cooldown indicator's sweep is this channel). `None` for a transform that never
    /// rotates — every placed world doodad — and all of WMO.
    pub uv_rot_seq: Option<SeqLoops<[f32; 4]>>,
    /// The batch's texture-transform **scaling** loop per file sequence slot (`(x, y)`), on the
    /// same rule as [`Self::uv_rot_seq`].
    pub uv_scale_seq: Option<SeqLoops<[f32; 2]>>,
    /// The batch's **animated RGB tint** (the M2Color colour track, time-varying only — a spell
    /// effect's white-hot flash cooling to red): baked by [`mat_anim`](super::mat_anim). When
    /// `Some`, the static vertex-colour tint is **skipped** for this batch (the two would
    /// double-apply); the renderer carries the tint on the material instead, seeded at the first
    /// key and animated where the lane runs material animation. `None` for constant/keyless tints
    /// (the static vertex bake) — the overwhelming majority.
    pub rgb_anim: Option<RgbAnim>,
    /// The tint twin of [`Self::uv_seq`], on the same rule and for the same reason.
    pub rgb_seq: Option<SeqLoops<[f32; 3]>>,
    /// The batch's MOBA section for WMO group batches ([`WmoBatchClass`] — TRANS / INT / EXT, the
    /// per-class lighting law of an interior group). `None` for every M2 batch.
    pub wmo_batch: Option<WmoBatchClass>,
    /// The M2 **skin-section index** this batch draws (`SkinBatch::skin_section_index`) — `None`
    /// for every WMO batch, which has no such concept.
    ///
    /// Two batches of one model that name the SAME section rasterize the **same triangles**: a base
    /// layer and the shine/reflect layer authored over it (`ARMORREFLECT3` on the ballista's bolt
    /// heads and shields, `BALISTASHINE02` on the loose bolts — 264 sections across 221 world
    /// doodads, `m2_shared_section`). The reference draws both from one vertex array under
    /// depth-write + LEQUAL, so the later one wins the coplanar tie **exactly**
    /// (`0x70c190`). We only match that while both take the same vertex-transform path,
    /// which is why the consolidators refuse a batch whose section they cannot take whole — see
    /// `terrain_stream::spawn::assemble`.
    pub section: Option<u16>,
    /// This batch's texture coordinates are **GENERATED, not authored** — a sphere-map environment
    /// coordinate derived per frame from the view-space reflection vector, not the vertex UVs
    /// ([`benilla_m2::M2Model::stage_is_env_mapped`]: `texture_unit_lookup[texCoordSet] > 2`).
    ///
    /// The renderer must compute `uv = normalize(P − 2(P·N)N).xy · 0.5 + 0.5` in view space
    /// instead of reading [`Self::uvs`], because on such a batch **there are no UVs to read**: the
    /// artist leaves the whole mesh at a single point (`GnomeSubwayGlass.m2` — all 330 vertices at
    /// exactly `(0,0)`) precisely because the runtime supplies them. Drawing it from the vertex
    /// data paints the entire surface in one corner texel of a reflection sheet — the Deeprun Tram
    /// glass tube's flat yellow (`AKGNOMEREFLECT.BLP` texel 0,0 = `225,221,142`, doubled by its
    /// Mod2x blend). `false` for every WMO batch and for the M2 batches that name a real UV channel.
    pub env_map: bool,
}

/// A render batch that is a single flat **ground-plane quad**: four vertices sharing one authored
/// horizontal plane at or hovering just above z = 0 (model space, WoW axes — the plane the
/// model's owner stands on; hover ≤ [`GROUND_HOVER_MAX`]), forming an axis-aligned rectangle in
/// model XY, every vertex fully weighted to one bone. This is the authoring pattern of
/// ground-ring spell effects — Battle Shout's six crescents at exactly z = 0, Consecration's burn
/// disc hovering at 0.207: flat quads at the owner's feet that bones slide, spin, and scale
/// outward, and that sloped terrain buries per-pixel under a normal depth test (the batches
/// author depth-test ON). The `groundscan` sweep (2026-07-14 z=0, 2026-07-31 hover; full 5875
/// chain) measures the population: 128 of the 139 z=0 flat batches under `Spells\` plus the 108
/// hovering discs are exactly this shape. The entity fx lane re-renders such parts as projected
/// surface decals (`benilla::ground_fx`) so they drape the terrain like the selection ring does.
#[derive(Debug, Clone, Copy)]
pub struct GroundQuad {
    /// The M2 bone (global bone-array index) all four vertices skin to — the quad's animated
    /// slide/spin/scale rides this joint. The consumer must pose corners through the joint's
    /// matrix × the bone's inverse bindpose, exactly the skinned-vertex path.
    pub bone: u16,
    /// The corners in **model space (WoW axes, z = 0)**, in bilinear rect order:
    /// `(min_x, min_y)`, `(max_x, min_y)`, `(min_x, max_y)`, `(max_x, max_y)`.
    pub corners: [[f32; 3]; 4],
    /// The authored UV at each corner, parallel to [`Self::corners`].
    pub uvs: [[f32; 2]; 4],
    /// The batch's **static M2Color tint** — the constant colour-track bake that rides
    /// [`RenderSubmesh::vertex_colors`] on the mesh path — or white when the batch authors none.
    ///
    /// A decal consumer re-renders this quad from the corners and never touches its vertex buffer,
    /// so without carrying the tint here the batch's whole colour is lost. That is load-bearing
    /// exactly where `m2_batches` says it is: spell ground art is authored on the NEUTRAL
    /// `GENERICGLOW*` radials, whose warmth lives entirely in this constant — `Flare_State_Base`'s
    /// two 13.89-yd washes are white sheets tinted `(0.992, 0.467, 0.0)`, and an untinted additive
    /// draw of them is a blown-white pool where the reference lays a dim orange one. A
    /// *time-varying* track instead rides [`RenderSubmesh::rgb_anim`] and clears the vertex bake,
    /// so this is white there and the two never double-apply.
    pub tint: [f32; 3],
}

/// Vertices further than this from the quad's own plane disqualify a batch as ground-plane flat
/// (the real population authors exact planes; the epsilon only absorbs float noise).
const GROUND_FLAT_EPS: f32 = 0.01;

/// The highest authored **hover** (uniform plane z, model space) a flat quad may sit at and
/// still be the ground-plane shape. z = 0 is the majority authoring (Battle Shout's crescents);
/// a minority hovers the disc just above the owner's ground plane to dodge terrain z-fighting.
/// The `groundscan` hover census (2026-07-31, 5875 chain, `Spells\`) measures the population:
/// 108 quad-shaped batches hover between 0.014 and 0.518 (the paladin aura/seal rings,
/// Consecration's 0.207 disc, Flamestrike's 0.097 burn), then clear air up to 1.389 — the two
/// `GreaterHeal_Low_Base` mid-body glow planes, which must NOT drape to the ground. 1.0 splits
/// the gap.
const GROUND_HOVER_MAX: f32 = 1.0;

/// The empty batch — test scaffolding (a picker or dress test wants *a* `RenderSubmesh`, not a
/// meaningful one). No geometry, `Opaque`, WMO-style repeat sampling; every flag at its
/// nothing-authored value.
impl Default for RenderSubmesh {
    fn default() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            texture: None,
            skin_slot: None,
            geoset_id: 0,
            char_slot: None,
            blend: ModelBlend::Opaque,
            wrap_x: true,
            wrap_y: true,
            two_sided: false,
            joints: Vec::new(),
            weights: Vec::new(),
            vertex_colors: Vec::new(),
            interior: false,
            emissive: false,
            icon_slot: false,
            sidn: None,
            window: false,
            additive: false,
            no_depth_write: false,
            no_depth_test: false,
            fog_policy: FogPolicy::Scene,
            billboard: None,
            welded_billboard: false,
            alpha_anim: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            rgb_anim: None,
            rgb_seq: None,
            wmo_batch: None,
            env_map: false,
            section: None,
        }
    }
}

impl RenderSubmesh {
    /// Is this a **billboard card authored back-to-front** — one flat plane whose normal sits in the
    /// −X half-space the billboard law points *away* from the camera?
    ///
    /// The billboard arm aims bone-local **+X** at the viewer (`0x71547c`;
    /// M2 bones carry no bind rotation, so bone-local == model space here), and 279 of the corpus's
    /// 4424 billboard batches are wound and normalled the other way — the camera only ever sees the
    /// card's back. The reference skins the normal through the same billboard-replaced palette row
    /// as the position (`0x71a460`) and never flips one per face, so on those cards
    /// its `max(N·L, 0)` runs off a normal pointing away from the viewer and the card's shading
    /// swings with the CAMERA: bare ambient when the sun is behind you, full sun when you look into
    /// it. Consumers light such a card off the side it presents instead.
    ///
    /// The planarity half is load-bearing: 3-D billboard geometry (the questgiver `?`'s 353 verts)
    /// carries normals pointing every way, and flipping just its −X ones would gut its shading.
    /// A card authored edge-on to the camera axis has no facing to correct, matching
    /// `bbfacescan`'s away/edge-on split.
    pub fn billboard_card_faces_away(&self) -> bool {
        self.billboard.is_some() && self.plane_normal().is_some_and(|n| n[0] < -Self::EDGE_ON_X)
    }

    /// A normal whose |x| is under this is edge-on to the billboard law's camera axis: it faces
    /// neither toward the viewer nor away, so there is nothing to decide.
    const EDGE_ON_X: f32 = 1e-3;

    /// The single plane every authored vertex normal of this batch shares — `None` when they do
    /// not, i.e. the batch is **3-D geometry**, not a card.
    ///
    /// This is the gate that separates the two things a "billboard batch" can be, and getting it
    /// wrong is expensive in both directions. A flat card has one facing, so which way it points
    /// decides whether the reference ever shows it and which side to light.
    /// A closed solid — the questgiver `?`'s 353 verts, a pauldron's little
    /// spike — has normals pointing every way: no single facing exists, backface culling can never
    /// hide it, and a per-batch normal flip would gut its shading. Sampling *one* triangle of such
    /// a batch and reporting the answer as the batch's facing is how decision 0836 concluded the
    /// reference culls a shoulder flap it in fact draws.
    pub fn plane_normal(&self) -> Option<[f32; 3]> {
        /// Cosine floor for "these two normals are the same plane" — one authored card, allowing
        /// for unnormalised//soft-averaged authoring.
        const SAME_PLANE_COS: f32 = 0.999;
        let unit = |n: &[f32; 3]| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (l > 1e-6).then(|| [n[0] / l, n[1] / l, n[2] / l])
        };
        let n0 = self.normals.first().and_then(unit)?;
        self.normals
            .iter()
            .all(|n| {
                unit(n).is_some_and(|n| n[0] * n0[0] + n[1] * n0[1] + n[2] * n0[2] > SAME_PLANE_COS)
            })
            .then_some(n0)
    }

    /// Detect the flat **ground-plane quad** shape ([`GroundQuad`]) — `None` for everything that
    /// isn't exactly it: more than four vertices, vertices off one shared horizontal plane, a
    /// plane hovering above [`GROUND_HOVER_MAX`], a multi-bone or partial-weight skin, or corners
    /// that don't form an axis-aligned XY rectangle. Billboard batches never match (their
    /// geometry is camera-facing by construction, not ground-lying). Deliberately strict: the 11
    /// `OTHER-FLAT` spell batches the population sweep found (Fist of Justice, IceNuke impact)
    /// stay on the ordinary render path rather than stretch this shape.
    pub fn ground_quad(&self) -> Option<GroundQuad> {
        self.ground_quad_hover(GROUND_HOVER_MAX).map(|(q, _)| q)
    }

    /// [`Self::ground_quad`] with the hover ceiling as a parameter, returning the plane's
    /// authored z alongside the quad — the census entry point (`groundscan` passes `INFINITY` to
    /// measure the whole hover population with the renderer's own shape test, so the instrument
    /// cannot drift from the mechanism).
    pub fn ground_quad_hover(&self, max_hover: f32) -> Option<(GroundQuad, f32)> {
        if self.billboard.is_some()
            || self.positions.len() != 4
            || self.joints.len() != 4
            || self.weights.len() != 4
            || self.uvs.len() != 4
        {
            return None;
        }
        // One shared horizontal plane, at or hovering just above the owner's ground level. The
        // corners keep their authored z: the decal projection drapes them onto the receiving
        // surface regardless, exactly as it flattens the z = 0 majority.
        let hover = self.positions.iter().map(|p| p[2]).sum::<f32>() / 4.0;
        if !(-GROUND_FLAT_EPS..=max_hover).contains(&hover)
            || self
                .positions
                .iter()
                .any(|p| (p[2] - hover).abs() > GROUND_FLAT_EPS)
        {
            return None;
        }
        // One bone, full weight, across all four vertices.
        let bone = self.joints[0][0];
        if self
            .joints
            .iter()
            .zip(&self.weights)
            .any(|(j, w)| j[0] != bone || w[0] < 0.999)
        {
            return None;
        }
        // Axis-aligned rectangle: every vertex sits on a corner of the XY bounding box.
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &self.positions {
            for a in 0..2 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let eps = [
            (max[0] - min[0]) * 1e-3 + 1e-6,
            (max[1] - min[1]) * 1e-3 + 1e-6,
        ];
        if max[0] - min[0] <= eps[0] || max[1] - min[1] <= eps[1] {
            return None; // degenerate (zero-area) quad
        }
        // Slot each vertex into its bilinear rect position; every slot must fill exactly once.
        let mut corners = [[0.0_f32; 3]; 4];
        let mut uvs = [[0.0_f32; 2]; 4];
        let mut filled = [false; 4];
        for (p, uv) in self.positions.iter().zip(&self.uvs) {
            let sx = if (p[0] - min[0]).abs() <= eps[0] {
                0
            } else if (p[0] - max[0]).abs() <= eps[0] {
                1
            } else {
                return None; // an x between the extremes — not a rectangle corner
            };
            let sy = if (p[1] - min[1]).abs() <= eps[1] {
                0
            } else if (p[1] - max[1]).abs() <= eps[1] {
                1
            } else {
                return None;
            };
            let slot = sy * 2 + sx;
            if filled[slot] {
                return None; // two vertices on one corner — degenerate
            }
            filled[slot] = true;
            corners[slot] = *p;
            uvs[slot] = *uv;
        }
        // The batch's constant M2Color, straight off the vertex bake it would have drawn with
        // (all four are the same colour — see [`GroundQuad::tint`]); white when it authors none.
        let tint = self
            .vertex_colors
            .first()
            .map_or([1.0; 3], |c| [c[0], c[1], c[2]]);
        Some((
            GroundQuad {
                bone,
                corners,
                uvs,
                tint,
            },
            hover,
        ))
    }
}
