//! Shared world-model material build — the `WowModelMaterial` (a `StandardMaterial` base carrying the
//! texture/alpha/cull + the `WowModelExt` shared-light extension that does the WoW shading) used for
//! every M2/WMO instance: doodads, WMO buildings, creatures, and GameObjects. Deduped per owner by
//! `(texture, blend, sidedness, kind, fade-variant)` so instances sharing a look batch into one draw.

use benilla_formats::{ModelBlend, WmoBatchClass};
use bevy::asset::AssetId;
use bevy::mesh::MeshTag;
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, Face};

use benilla_assets::materials::{WowModelExt, WowModelMaterial, VANILLA_ALPHA_KEY_REF};

mod batch;
pub mod lazy;
pub mod park;
mod visibility;

pub use batch::{BatchVariants, EntityUvLane, M2BatchMaterials, ModelMaterials, SkyboxBatch};

/// Yards of transparent-pass sort bias per authored batch-order step (`MatKey::batch_order` ×
/// this, on `StandardMaterial::depth_bias` — bevy adds that field to the phase item's sort
/// distance; positive = drawn later, the `sky_order` sign law). **Why it exists:** one model's
/// coplanar transparent batches (the Naxx items' Opaque + Mod2x sheen + Blend overlay triples)
/// share one mesh centre, so their sort distances TIE, and a tie is re-resolved every frame by
/// whatever order the queue happens to iterate — in a churning scene (a city) the coplanar layers
/// swap draw order frame to frame and the composite strobes (Mod2x-then-Blend ≠ Blend-then-Mod2x).
/// The reference cannot express the bug: it ties every draw command of one instance to a single
/// sort key and keeps the *authored command order* inside the tie (`0x707db1`; MOBA file order for
/// WMO). This epsilon reproduces that
/// order through bevy's sort. Sized well under [`benilla_formats::owner_last_rung`]'s 1-yd floor
/// (an effect still draws after every batch of its owner) and far over f32 noise on a ~500 yd
/// sort distance (~4e-5).
///
/// It IS a pipeline-key axis when it reaches 1.0: the base `StandardMaterialKey` packs
/// `depth_bias as i32` (only the extension `WowModelKey` excludes it — 0837's comment
/// overclaimed), so the product is capped at [`BATCH_ORDER_SORT_CAP`] and applied to
/// transparent-pass materials only (`model_material`).
pub(crate) const BATCH_ORDER_SORT_EPS: f32 = 1e-3;

/// The eps product's ceiling: strictly under 1.0 so no batch index can mint a pipeline key
/// (`as i32` == 0 for the whole band), and strictly under `FAR_KEY_PULL` so a capped batch's
/// far-side twin still truncates onto the far band's single key integer. Batches past index
/// 899 tie here — see the cap rationale at the one application site.
pub(crate) const BATCH_ORDER_SORT_CAP: f32 = 0.9;

/// `WOW_NO_ALPHATEST=1` — every cutout batch draws opaque (the A/B lever described at the
/// [`model_material`] use site; the static-gx bake keys its cutout runs on the same read).
pub(crate) fn alphatest_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_ALPHATEST").is_some())
}

/// `WOW_WMO_BIAS=0` — the WMO batch's clip-z nudge off (a bisect lever; see its use site). Read
/// once: the use site runs per WMO batch on stream-in, thousands of times through a city's load.
fn wmo_bias_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_WMO_BIAS").as_deref(), Ok("0")))
}

/// Material-dedup key: same texture + blend + sidedness + kind + fade-variant → one shared material.
#[derive(PartialEq, Eq, Hash)]
pub struct MatKey {
    /// **Which light rig this material binds** — the `#[storage(90, …)]` buffer cloned into
    /// `WowModelExt::light_buf`. A key axis because it is the one material input that is not a
    /// property of the batch: the world lane binds the shared sun/sky buffer, a portrait booth
    /// binds its frozen studio buffer, the char-select scene binds its own authored rig. Without
    /// it two lanes building the same batch collide and the second gets the first's *lighting* —
    /// which the booths avoided only by each keeping a private cache, a discipline nothing
    /// enforced and which cost them any distance sweep (they never expired at all).
    light: bevy::render::render_resource::BufferId,
    texture: Option<AssetId<Image>>,
    blend: ModelBlend,
    two_sided: bool,
    is_wmo: bool,
    is_interior: bool,
    is_emissive: bool,
    is_additive: bool,
    fade_variant: bool,
    /// M2 render flag 0x10 / 0x08 — disable depth write / depth test (`specialize` honours them).
    no_depth_write: bool,
    no_depth_test: bool,
    /// The batch's fog COLOUR policy ([`benilla_formats::FogPolicy`]) — packed into `clutter_fade.z`
    /// bits 4-6 (`wow_model.wgsl`'s step-5 fog).
    fog_policy: benilla_formats::FogPolicy,
    /// This batch's texture coordinates are **generated** — the sphere-map environment coordinate,
    /// not the vertex UVs ([`benilla_formats::RenderSubmesh::env_map`]). Packed into
    /// `clutter_fade.z` bit 12; a key axis because two batches sharing a texture but differing here
    /// sample it completely differently. **Not** a `WowModelKey` axis — it swaps which UV the
    /// FRAGMENT stage samples, no pipeline state, so it mints no pipeline (decision 0837).
    env_map: bool,
    /// The batch's static terrain-shade selector ([`ShadeSel`]). Static per placement, so it dedups a
    /// lit / matte / shaded material variant.
    shade: ShadeSel,
    /// Authored batch index + 1 (0 = a legacy/unordered caller). Every ordered batch (M2 and WMO)
    /// folds it into the material's transparent SORT bias ([`BATCH_ORDER_SORT_EPS`]); WMO batches
    /// additionally ride it into the vertex-stage clip-z nudge (the MOBA draw order is strict file
    /// order, no sort — `0x6b4f10`/`0x6b5190`).
    batch_order: u16,
    /// The batch's UV-animation identity (decision 0130 phase 3): the `Arc<UvAnim>` pointer, so
    /// batches scrolling on different loops never share a material (their `sun_scale.zw` offsets
    /// diverge every frame) while every instance of the same model batch does. Sound because the
    /// `Arc` lives in the loaded model asset — one allocation per model per batch, stable while
    /// loaded. `None` = static UVs (the overwhelming majority).
    uv_anim: Option<usize>,
    /// The batch's animated-RGB-tint identity: the `Arc<RgbAnim>` pointer (same soundness argument
    /// as [`Self::uv_anim`]), so batches tinting on different tracks never share a material while
    /// every instance of the same model batch does. `None` = a static tint (the vertex bake).
    rgb_anim: Option<usize>,
    /// The WMO batch's MOBA section (`None` for M2): an interior group's INT and TRANS batches take
    /// different lighting lanes (`tint.w`), so they must never dedupe onto one material.
    wmo_class: Option<WmoBatchClass>,
    /// The WMO MOMT SIDN night-glow colour (RGB gamma bytes; `None` = no SIDN / M2) — part of the
    /// material identity so glass authored with different glow colours never dedupes together.
    sidn: Option<[u8; 3]>,
    /// The WMO MOMT WINDOW flag — an interior-group batch on the brighter midpoint light.
    window: bool,
    /// This material is a **depth-prime twin** ([`zfill_material`] — the reference's `M2UseZFill`
    /// clone command, `0x707f7d`): colour writes masked off, blend off,
    /// z-write on, drawn before its model's colour batches. Its own key axis so a twin can never
    /// dedupe onto a colour material.
    zfill: bool,
    /// This batch is part of a **WMO skybox** ([`crate::skybox`]) — the building-owned painted sky,
    /// drawn on this lane like any other M2 since decision 1264. Its own key axis because the lane
    /// changes pipeline state ([`SKY_DEPTH_MARKER`]: the far depth pinned at the vertex) and sort rung
    /// ([`skybox_sort_bias`]), so a skybox batch must never dedupe onto the identical-looking world
    /// batch — `CavernsOfTimeSky` and Elwynn share no texture today, but nothing enforces that and
    /// the collision would be a world doodad silently drawn at the far plane.
    sky_depth: bool,
    /// **This material belongs to one placement, not to the batch** — `None` for the shared
    /// material every instance of a model reuses, which is every batch but one measured class.
    ///
    /// The animated-material registries ([`crate::doodad_anim::UvAnimMaterials`] and its tint twin)
    /// are keyed by MATERIAL, so a batch whose animated loop depends on **which sequence its
    /// instance is playing** cannot share one and be right: the BRM lava bubbles key their whole
    /// flipbook inside a 50 %-weighted variation of animation id 0, and their 15 placements re-roll
    /// independently every ~3.3 s (decision 0768), so at any instant some are on slot 0 and some on
    /// slot 1. Keying the material by the placement's anim host gives each its own row and its own
    /// sequence (decision 1408); the population is 22 batch-channels across 6 models
    /// (`benilla-extract uvslotscan`), and the clones expire on the same distance evictor as every
    /// other material ([`scope_model_materials`], decision 0785).
    instance: Option<Entity>,
}

/// The per-material static terrain-shade **selector** baked into `sun_scale.x` — NOT an intensity
/// itself. `wow_model.wgsl` thresholds the selector (≥0.85 / ≥0.5) into the byte-true INTENSITY
/// family (decision 0354: 2.5 lit / 1.0 day-night / 0.5 MCSH-shadowed — `[node+0xa4]`) scaling the
/// global SH eval. It is the static half of the verified per-category matrix (`0x69e280`,
/// decision 0173) — dynamic entities
/// (units/players/GameObjects) are always [`ShadeSel::Lit`] here and mix toward the shaded
/// intensity per instance via the `MeshTag` shade byte ([`crate::entity_shade`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShadeSel {
    /// Lit ground, the boosted intensity family (the binary's 2.5): **every entity M2, and only
    /// those** — a unit, a player, a GameObject. The 2.5 and its 3.3333/s chase live on the
    /// WENTITY light node (vtable `0x810810`, hung off `[obj+0xe0]`); an object without one
    /// cannot reach either. ADT map doodads were on this variant until 2050 and are not entities:
    /// they are `CMapDoodadDef`s, whose `[+0xa4]` is only ever {0.0, 0.5, 1.0} and which have no
    /// ramp target field at all (`+0xf8` is `m[2][3]` of their world matrix).
    ///
    /// Entities still select this, but what it *means* for them changed in 0809. The selector is
    /// only the static half — the per-instance `MeshTag` shade byte carries the rest, and
    /// [`crate::entity_shade`] now pins that byte to the day/night ×1.0 for units and players
    /// (`0x672a20`'s null-node fallback commits with no per-node intensity multiply at all), while
    /// GameObjects keep the real 2.5/0.5 chase. So for an entity this variant reads "on the light-node
    /// path"; the node decides the amplitude.
    Lit,
    /// Lit ground, intensity 1.0: **either doodad class** — an exterior WMO MODD prop *and* an ADT
    /// map doodad, byte-verified never to reach the 2.5 site.
    ///
    /// The parenthetical this doc carried until 2050 — "a Stormwind street fountain is NOT
    /// brightened like an Elwynn tree" — was exactly backwards about the tree: both are
    /// `CMapDoodadDef`s and both commit 1.0 lit / 0.5 shadowed. What is brightened is the
    /// *entity* walking past them. Measured over six trace frames: 111 identified MDDF
    /// placements, 356 draws, gains only ∈ {0.5, 1.0}.
    Matte,
    /// The base sits on MCSH-shadowed terrain: the dim intensity (the binary's 0.5).
    Shaded,
    /// Lit by an **authored M2 light rig** (the glue create booth, decision 0429): the lit value is
    /// the order-2 SH probe in slot 0 of the material's *own* light buffer — the scene's ambient +
    /// directional lights folded by `lighting::prop_probe_coeffs`, the same `Model2.bls` curve the
    /// reference's vertex program runs — plus the per-vertex ≤3-nearest point term (the scene's
    /// authored point lights ride the buffer's point table; a rig material is neither WMO nor
    /// interior, so the vertex stage computes it). The world's sun/intensity machinery never
    /// applies: a glue scene has no day/night, no MCSH, no storm band.
    Rig,
}

impl ShadeSel {
    /// The `sun_scale.x` encoding `wow_model.wgsl` thresholds (≥1.5 ⇒ authored-rig probe;
    /// ≥0.85 ⇒ ADT lit; ≥0.5 ⇒ matte; else shaded).
    pub fn selector(self) -> f32 {
        match self {
            ShadeSel::Lit => 1.0,
            ShadeSel::Matte => 0.6,
            ShadeSel::Shaded => 0.2,
            ShadeSel::Rig => 2.0,
        }
    }
}

/// A material-dedup cache. Each model-spawning subsystem (terrain doodads/WMOs, streamed entities)
/// keeps its own so its handles drop with it — and, since decision 0793, so its entries expire by
/// **distance** ([`benilla_assets::SpatialCache`]) instead of living until the next map change. A
/// cache nobody sweeps (the `Local<MaterialCache>`s in the glue booth and the portrait bake) behaves
/// exactly as it did: a plain dedup map.
pub type MaterialCache = benilla_assets::SpatialCache<MatKey, Handle<WowModelMaterial>>;

/// Build (or fetch the deduped) [`WowModelMaterial`] for a model batch: a `StandardMaterial` base
/// carrying the texture/alpha/cull, plus the `WowModelExt` shared-light extension. `fade_variant`
/// marks the `AlphaMode::Blend` twin every feather pass rides — the doodad distance fade AND the
/// entity appear/despawn/aura fades (`entities::display` builds its twins with it too); steady
/// materials pass `false`. `specialize` keys depth-write-ON on this bit for the twin.
///
/// A twin builder passes the **SOURCE batch's blend mode** (never `ModelBlend::Blend`): the twin
/// always blends — the reference's promotion is transparent-pass membership, not a per-mode state
/// (`0x70c1fd`) — but the alpha TEST under promotion keys on the *stored* blend mode (`0x70c237`):
/// AlphaKey keeps its 224/255 cutout while blending, Opaque runs with no alpha test at all. The
/// source blend is what sets [`TWIN_CUTOUT_MARKER`] right (decision 0842: a twin
/// built as `Blend` cut every texel under 224/255 out of a stealthed Opaque batch, which erased
/// Gressil's blade body and left only its high-alpha rune pattern).
pub fn model_material(
    cache: &mut MaterialCache,
    materials: &mut Assets<WowModelMaterial>,
    texture: Option<Handle<Image>>,
    blend: ModelBlend,
    two_sided: bool,
    is_wmo: bool,
    is_interior: bool,
    is_emissive: bool,
    is_additive: bool,
    fade_variant: bool,
    no_depth_write: bool,
    no_depth_test: bool,
    fog_policy: benilla_formats::FogPolicy,
    // This batch's texture coordinates are GENERATED, not authored
    // ([`benilla_formats::RenderSubmesh::env_map`]) — see [`MatKey::env_map`].
    env_map: bool,
    shade: ShadeSel,
    // Authored batch index + 1 (0 = unordered): the transparent-pass ORDER of this batch among its
    // model's other batches — see [`MatKey::batch_order`] and [`BATCH_ORDER_SORT_EPS`].
    batch_order: u16,
    uv_anim: Option<&std::sync::Arc<benilla_formats::UvAnim>>,
    rgb_anim: Option<&std::sync::Arc<benilla_formats::RgbAnim>>,
    // The WMO batch's MOBA section (`None` for M2 batches) — with `is_interior`, it picks the
    // batch's lighting lane (`tint.w`; see `WowModelExt::tint`).
    wmo_class: Option<WmoBatchClass>,
    // The WMO window/glass law (`None`/`false` for M2): the MOMT SIDN night-glow colour and the
    // WINDOW midpoint-light flag (`WowModelExt::sidn`).
    sidn: Option<[u8; 3]>,
    window: bool,
    // This batch belongs to a **WMO skybox** — the sky lane: forced far depth, no rasterizer bias,
    // and the skybox sort rung instead of the ordinary batch-order eps. See [`MatKey::sky_depth`].
    sky_depth: bool,
    light: &Buffer,
    // The ONE placement this material belongs to, or `None` for the shared batch material every
    // instance of the model reuses. `Some` only for a batch whose animated UV/tint loop depends on
    // the sequence its instance is playing (decision 1408): the animated-material registries are
    // keyed by material, so such a batch cannot share one and be right. See `MatKey::instance`.
    instance: Option<Entity>,
) -> Handle<WowModelMaterial> {
    let key = MatKey {
        light: light.id(),
        texture: texture.as_ref().map(Handle::id),
        blend,
        two_sided,
        is_wmo,
        is_interior,
        is_emissive,
        is_additive,
        fade_variant,
        no_depth_write,
        no_depth_test,
        fog_policy,
        env_map,
        shade,
        batch_order,
        uv_anim: uv_anim.map(|a| std::sync::Arc::as_ptr(a) as usize),
        rgb_anim: rgb_anim.map(|a| std::sync::Arc::as_ptr(a) as usize),
        wmo_class,
        sidn,
        window,
        zfill: false,
        sky_depth,
        instance,
    };
    if let Some(h) = cache.fetch(&key) {
        return h;
    }
    // Additive batches (M2 glow cards) ADD their colour to the framebuffer — so the warm glow isn't
    // muted by the (cool, at night) background bleeding through. They go in the transparent pass
    // (`AlphaMode::Blend`); the shader folds the radial alpha into the colour IN GAMMA SPACE
    // (decision 0160) and `specialize` overrides the blend STATE to a pure (ONE, ONE) add. The
    // additive marker is `clutter_fade.z` bit 2 — the shader gates the gamma-premultiply on the
    // SAME bit specialize keys on (a stale "model_flags.w == 2.0" claim here once desynced the
    // two: blend went pure-add while the premultiply never fired — the flat-square regression).
    // `WOW_NO_ALPHATEST=1` draws every cutout batch opaque — the A/B for "is the alpha test
    // itself discarding this surface?", which is otherwise indistinguishable in the pixels
    // from the surface losing a depth test or never being submitted. (B38: the flip
    // survives it unchanged, so the cutout is not what removes the awning.) It suppresses the
    // fade twin's cutout marker too, so the A/B answers the same question mid-fade.
    let source_cutout = blend == ModelBlend::AlphaTest && !alphatest_disabled();
    let alpha_mode = if is_additive || fade_variant {
        // A fade twin blends whatever its source blend is — the reference's promotion is
        // transparent-pass membership (`0x70c1fd`); the source blend the
        // caller passed decides only the cutout marker below.
        AlphaMode::Blend
    } else {
        match blend {
            ModelBlend::Opaque => AlphaMode::Opaque,
            ModelBlend::AlphaTest if source_cutout => AlphaMode::Mask(VANILLA_ALPHA_KEY_REF),
            ModelBlend::AlphaTest => AlphaMode::Opaque,
            // Mod/Mod2x ride the transparent pass (they multiply what's already drawn, so the
            // scene under them must exist); `specialize` swaps the actual blend state to the
            // byte-verified multiply factors via the marker bits below (decision 0528).
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x => AlphaMode::Blend,
        }
    };
    // Single-sided unless the M2's 0x04 flag is set (many canopy planes are one-directional).
    let cull_mode = if two_sided { None } else { Some(Face::Back) };
    // The authored batch order as a transparent sort bias ([`BATCH_ORDER_SORT_EPS`]): one model's
    // coplanar batches draw in file order instead of re-flipping a sort tie every frame.
    //
    // TRANSPARENT-PASS ONLY, and CAPPED below 1.0 (decision 0938's tail — the Stormwind evening
    // log). The bias is also the base `StandardMaterialKey`'s `depth_bias as i32` axis, which
    // 0837's original comment believed excluded (only the EXTENSION key excludes it) — invisible
    // while every product truncated to 0, until a city-scale WMO's root-scoped batch index
    // (`assemble.rs`, u16 — Stormwind roots run past 2000) pushed the product to 1.0+ and each
    // integer became a live-compiled pipeline. Opaque/mask batches take no bias at all: their
    // pass never sorts by it, and the coplanar-stack job belongs to the vertex-stage depth nudge
    // (0837's `sun_scale.y`), so a nonzero value was only a key leak. Transparent batches keep
    // total order through index 899 and tie at the cap beyond it — safe, because the eps is
    // load-bearing only for SAME-CENTRE stacks (M2 item overlays, always low-index); WMO batches
    // sort by their own distinct mesh centres. The cap stays under `FAR_KEY_PULL` so a far twin
    // of a capped batch still lands on the far band's one key integer.
    //
    // The SKY lane takes the same shape one rung down: a skybox is camera-anchored, so its view-z
    // is a few tens of yards and it would sort after the world's transparents rather than behind
    // them ([`crate::sky_order::WMO_SKYBOX_BIAS`]). Its opaque batches still take no bias — the
    // painted cube faces are in the opaque pass, which is exactly where the reference puts them.
    let depth_bias = if !matches!(alpha_mode, AlphaMode::Blend) {
        0.0
    } else if sky_depth {
        skybox_sort_bias(batch_order)
    } else {
        (f32::from(batch_order) * BATCH_ORDER_SORT_EPS).min(BATCH_ORDER_SORT_CAP)
    };
    let base = match texture {
        Some(image) => StandardMaterial {
            base_color_texture: Some(image),
            alpha_mode,
            double_sided: two_sided,
            cull_mode,
            depth_bias,
            ..default()
        },
        // No resolved texture → the reference DISABLES the texture stage and draws the batch in
        // its flat vertex/material colour — i.e. WHITE modulate, byte-verified for the no-source
        // runtime-texture case (`0x59baa0`: slot 0 → glDisable, no default texture, alpha test
        // passes everywhere — the Westfall lamppost's "bulb on" pane). The old muted-brown debug
        // tint was unfaithful. Blend/cull state still
        // honoured — an untextured fade twin must keep its per-instance alpha path.
        None => StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode,
            double_sided: two_sided,
            cull_mode,
            depth_bias,
            ..default()
        },
    };
    let handle = lazy::defer(
        materials,
        ExtendedMaterial {
            base,
            extension: WowModelExt {
                // `clutter_fade.z` is unread by the shader, so it carries the per-batch pipeline markers
                // `specialize` keys on: bit0 = no-depth-write (M2 0x10), bit1 = no-depth-test (0x08),
                // bit2 = additive blend. (`x`/`y`/`w` stay the ground-clutter distance fade — `0` here.)
                clutter_fade: Vec4::new(
                    0.0,
                    0.0,
                    // Bit 3 = OPAQUE-INTENT: an opaque/alpha-key steady batch whose output alpha is
                    // semantically meaningless (opaque/mask pipelines ignore it — only blend pipelines
                    // read it). The shader pins such batches' output alpha to 1.0: a spec-level no-op
                    // that armors against a multi-view pipeline-state mixup observed on macOS/Metal
                    // (opaque WMO/M2 draws intermittently bound with blending enabled when an extra
                    // camera exists, bleeding the garbage BLP alpha channel — the "pale film on
                    // buildings" regression; full measured chain in the fix commit).
                    // Bits 4-6 = the per-batch FOG POLICY (`FogPolicy` discriminant, `0x70baf0`):
                    // 0 = scene so clutter/water materials —
                    // which leave the byte's high bits 0 (see `WorldAssets::model_material`,
                    // `water_fx`) — keep ordinary scene fog; 1/2/3 = the additive/Mod/Mod2x BLACK/
                    // WHITE/GREY fog colours; 4 = fog disabled outright (render flag 0x02).
                    // Bits 7/8 = the MULTIPLY blends (decision 0528): `specialize` swaps the pipeline
                    // to the byte-verified factors — Mod DST_COLOR/ZERO, Mod2x DST_COLOR/SRC_COLOR
                    // (exact on the 0161 gamma lane: the framebuffer holds gamma, like the reference).
                    // Bit 10 = TWIN CUTOUT (decision 0842): this fade twin's SOURCE batch alpha-tests,
                    // so the shader re-applies the hard 224/255 cutout on the unfaded alpha (the
                    // reference's promoted-AlphaKey ALPHAREF = A×224 — the same fixed silhouette). An
                    // Opaque source sets no bit: the reference disables its alpha test outright, steady
                    // and promoted alike (`0x70c237` keys ALPHAREF on the STORED
                    // blend mode, mode 0 → ref 0).
                    f32::from(
                        u16::from(no_depth_write)
                            | (u16::from(no_depth_test) << 1)
                            | (u16::from(is_additive) << 2)
                            | (u16::from(
                                matches!(blend, ModelBlend::Opaque | ModelBlend::AlphaTest)
                                    && !fade_variant
                                    && !is_additive,
                            ) << 3)
                            | (u16::from(fog_policy as u8) << 4)
                            | (u16::from(blend == ModelBlend::Mod) << 7)
                            | (u16::from(blend == ModelBlend::Mod2x) << 8)
                            | (u16::from(fade_variant && source_cutout) * TWIN_CUTOUT_MARKER)
                            | (u16::from(env_map) * ENV_MAP_MARKER)
                            | (u16::from(sky_depth) * SKY_DEPTH_MARKER),
                    ),
                    0.0,
                ),
                // x = WMO (FFP N·L × MOCV, not the M2 SH probe); y = distance-fade blend variant; z = WMO
                // interior group (sun off, baked MOCV carries the room); w = **unlit fullbright** (>0.5 ⇒
                // bypass lighting in wow_model.wgsl): the M2 UNLIT (0x01) flag, or WMO UNLIT on an
                // exterior-group batch (`0x6b4f10`; interior drawer `0x6b5190` ignores it, section
                // law only). Additive is NOT fullbright: the real client *lights* additive batches
                // unless 0x01 is set (`DAT_00811fa8[4] = 1`), so an
                // un-flagged additive (e.g. ArmorReflect shine) is lit. The M2 path DOES generate
                // env-map texcoords — in the vertex program, invisible to a SetRenderState-only
                // sweep (decision 0971). See [`ENV_MAP_MARKER`].
                // M2 Mod/Mod2x ARE fullbright regardless of 0x01 — the lighting table
                // `DAT_00811fa8 = {1,1,1,1,1,0,0}` clears GL_LIGHTING for modes 5/6; WMO lighting
                // stays flag-driven only (decision 0528).
                model_flags: Vec4::new(
                    if is_wmo { 1.0 } else { 0.0 },
                    if fade_variant { 1.0 } else { 0.0 },
                    if is_interior { 1.0 } else { 0.0 },
                    if is_emissive
                        || (!is_wmo && matches!(blend, ModelBlend::Mod | ModelBlend::Mod2x))
                    {
                        1.0
                    } else {
                        0.0
                    },
                ),
                // x = the static MCSH terrain-shade SELECTOR ([`ShadeSel`]: 1.0 ADT-lit / 0.6 matte /
                // 0.2 shaded; shader thresholds at 0.85 and 0.5) that chooses which live sun LEVEL scales the
                // FFP matte's diffuse term. Static per material (a doodad doesn't move), so it dedups the
                // variants; moving entities are Lit here and mix per instance via the `MeshTag` shade byte.
                // y = the WMO authored batch order, read by `wow_model.wgsl`'s VERTEX stage: it scales
                // clip z by (1 + y·2⁻²³) so a later coplanar batch wins the reverse-Z depth test in any
                // draw order — the byte-verified MOBA draw-order determinism. Uniform data by design:
                // as a `WowModelKey` axis driving a fixed-function depth bias it made every batch index
                // its own PIPELINE, and a first city sight compiled ~3000 of them synchronously on the
                // render thread (decision 0837). `WOW_WMO_BIAS=0` (B38's A/B diagnostic) zeroes it here.
                // zw = the batch's live **UV-animation offset** (decision 0130 phase 3, `0x714260`:
                // the real client adds the sampled translation to the stage UVs —
                // translation is un-pivoted, and no placed doodad uses rotation/scaling). Seeded at
                // t = 0 here; `doodad_anim::tick_anim_materials` re-samples it per drawn frame on the
                // shared clock (frozen in captures).
                sun_scale: {
                    let uv0 = uv_anim.map_or([0.0, 0.0], |a| a.sample(0.0));
                    // The clip-z nudge stays WMO-only: M2 coplanar layers pass GreaterEqual at exactly
                    // equal depth (same mesh, same transform, same vertex path), and their ORDER is
                    // the sort bias above — nudging their depth would be an unverified extra.
                    let order = if !is_wmo || wmo_bias_off() {
                        0.0
                    } else {
                        f32::from(batch_order)
                    };
                    Vec4::new(shade.selector(), order, uv0[0], uv0[1])
                },
                // The animated M2Color tint's first key (identity white for static batches — their
                // constant tint rides the vertex colours instead). A lane that never re-samples this
                // shows exactly the old static bake; the effect lane clones + ticks it per instance.
                // `w` = the WMO interior batch-class lane: an interior group's INT batches draw UNLIT
                // (pure tex × MOCV) and its TRANS batches lerp lit↔bake by the MOCV alpha
                // (`0x6b5190`, observed on the abbey at close range). Exterior groups' batches (and
                // every M2) ride 0 = the exterior law.
                tint: {
                    let t0 = rgb_anim.map_or([1.0, 1.0, 1.0], |a| a.sample(0.0));
                    let class_lane = match (is_interior && is_wmo, wmo_class) {
                        (true, Some(WmoBatchClass::Int)) => 1.0,
                        (true, Some(WmoBatchClass::Trans)) => 2.0,
                        _ => 0.0,
                    };
                    Vec4::new(t0[0], t0[1], t0[2], class_lane)
                },
                // The WMO window/glass law (`WowModelExt::sidn`): xyz = the authored SIDN emissive
                // (gamma bytes /255 — the shader ramps it by the live night fraction on lit lanes),
                // w = the WINDOW midpoint-light flag.
                sidn: {
                    let c = sidn.unwrap_or([0, 0, 0]);
                    Vec4::new(
                        f32::from(c[0]) / 255.0,
                        f32::from(c[1]) / 255.0,
                        f32::from(c[2]) / 255.0,
                        if window { 1.0 } else { 0.0 },
                    )
                },
                // Static until a sampler registers this material (decision 1381) — then the
                // table slot is baked in exactly once.
                anim_slots: Vec4::ZERO,
                light_buf: light.clone(),
            },
        },
    );
    cache.insert(key, handle.clone());
    handle
}

/// `clutter_fade.z` marker bit 9: this material is a depth-prime twin ([`zfill_material`]).
pub(crate) const ZFILL_MARKER: u16 = 1 << 9;

/// `clutter_fade.z` marker bit 10: this twin's SOURCE batch alpha-tests, so the shader re-applies
/// the hard 224/255 cutout while blending (`wow_model.wgsl`). Set from the source blend mode by
/// [`model_material`] (fade twins) and [`zfill_material`] (depth-prime twins); never on an
/// Opaque-source twin — the reference runs mode 0 with the alpha test disabled, steady and
/// promoted alike (`0x70c237`; decision 0842).
pub(crate) const TWIN_CUTOUT_MARKER: u16 = 1 << 10;

/// `clutter_fade.z` marker bit 12: this batch's texture coordinates are **generated**, so
/// `wow_model.wgsl` derives them from the view-space reflection vector instead of sampling the
/// (meaningless) vertex UVs — the reference's `texture_unit_lookup > 2` env stage. Bit 11 is
/// [`FAR_SIDE_MARKER`]. Deliberately **not** a [`benilla_assets::materials::WowModelKey`] axis: it changes
/// only what the fragment stage samples, so it needs no pipeline of its own (decision 0837).
pub(crate) const ENV_MAP_MARKER: u16 = 1 << 12;

/// `clutter_fade.z` marker bit 13: the **WMO-skybox lane** — this batch is part of a building's
/// painted sky, so `specialize` compiles the shader's `WOW_SKY_DEPTH` branch (the far depth,
/// pinned at the vertex — decision 2016) and drops the rasterizer bias constant. It *is* a
/// [`benilla_assets::materials::WowModelKey`] axis, unlike [`ENV_MAP_MARKER`]: the pin belongs
/// to exactly one camera-anchored model, and every other draw in the frame must keep the
/// pipeline that leaves its depth real.
pub(crate) const SKY_DEPTH_MARKER: u16 = 1 << 13;

/// Sort-bias step per authored batch on the **WMO-skybox lane** — [`BATCH_ORDER_SORT_EPS`]'s job,
/// resized for the rung it has to survive.
///
/// The plain 1e-3 eps cannot be used here: added to [`crate::sky_order::WMO_SKYBOX_BIAS`] (−6e4) it
/// falls *under the f32 ulp at that magnitude* (2¹⁵·2⁻²³ = 0.0039), so every batch rounds onto the
/// same bias and the sort tie the eps exists to break comes back in silence. This step is 4 ulps
/// there — exactly representable, strictly monotone. The tie is not a rare coplanar case on this
/// lane: a skybox is anchored at the eye, so *every* pair of its batches shares a sort distance,
/// every frame.
pub(crate) const SKYBOX_ORDER_EPS: f32 = 0.015625;

/// The skybox band's ceiling. With [`FAR_KEY_PULL`] it keeps the whole band on ONE pipeline-key
/// integer — 0837's law: the base `StandardMaterialKey` packs `depth_bias as i32`, so a band
/// straddling an integer is a live pipeline compile per side. Pulled by just under 1 and capped at
/// 0.9, every batch lands in `(RUNG − 0.99, RUNG − 0.09]`, which truncates toward zero onto the
/// same integer for all of them. The cap ties batches past index 57 (`0.9 / SKYBOX_ORDER_EPS`); the
/// largest skybox the chain ships is 21 batches (`CavernsOfTimeSky.m2`).
pub(crate) const SKYBOX_ORDER_CAP: f32 = 0.9;

/// This batch's slot in the skybox band: the lane rung, pulled onto one pipeline key, plus the
/// authored batch order. See [`SKYBOX_ORDER_EPS`] and [`crate::sky_order::WMO_SKYBOX_BIAS`].
fn skybox_sort_bias(batch_order: u16) -> f32 {
    crate::sky_order::WMO_SKYBOX_BIAS - FAR_KEY_PULL
        + (f32::from(batch_order) * SKYBOX_ORDER_EPS).min(SKYBOX_ORDER_CAP)
}

/// The twin's `Transparent3d` sort bias (yards; **negative = drawn earlier** — the sign law is
/// `sky_order`'s module doc). It must clear the spread of one model's part AABB centres, so **all**
/// of a fading model's twins draw before **any** of its colour parts — the reference achieves the
/// same by tying every command of one instance to a single sort key (`cmd+0x14 = model+0x84`) and
/// putting twins first inside the tie (`0x707db1`, `0x70ae10`). 8 yd covers every
/// humanoid and mount; the residue (a model taller than 8 yd may keep a little self-overlap
/// darkening at its extremities, and transparent scene content sorting within 8 yd behind a fading
/// body draws after the prime and is depth-clipped where the body covers it) is recorded in
/// decision 0831. The same field doubles as the rasterizer depth bias in relative ULPs — at −8 that
/// is ~2⁻²⁰ relative, and its direction (twin marginally farther) only makes the colour pass's
/// GreaterEqual test safer against cross-pipeline ULP noise.
pub(crate) const ZFILL_SORT_BIAS: f32 = -8.0;

/// Build (or fetch) the **depth-prime twin** material for one fadeable entity batch — the
/// reference's `M2UseZFill` clone (`0x707f7d`): while a
/// model draws translucent (`0 < A < 1`), a colour-masked, blend-off, z-writing copy of each of its
/// depth-writing batches draws first, so every colour fragment behind the model's own nearest
/// surface fails the depth test — one blended layer everywhere, no self-overlap darkening.
///
/// `cutout` = **the source batch alpha-tests** (AlphaKey). It mirrors what the part's COLOUR pass
/// discards, and must keep mirroring it: a twin that writes depth where the colour pass discards
/// leaves an invisible depth wall inside the cutout holes. Only an AlphaKey source rides the fade
/// twin's hard 224/255 cutout ([`TWIN_CUTOUT_MARKER`]); Opaque sources never alpha-test
/// (`0x70c237`/`0x707f7d` — the reference's twin keeps mode 1's test, mode 0 has
/// none; decision 0842) and authored-Blend sources discard nothing either.
///
/// Everything that shapes only the *colour* (lighting lane, shade, fog, tint) is canonicalized so
/// twins dedupe maximally — the `WOW_ZFILL` fragment returns before any of it.
pub fn zfill_material(
    cache: &mut MaterialCache,
    materials: &mut Assets<WowModelMaterial>,
    texture: Option<Handle<Image>>,
    two_sided: bool,
    cutout: bool,
    light: &Buffer,
) -> Handle<WowModelMaterial> {
    let key = MatKey {
        light: light.id(),
        texture: texture.as_ref().map(Handle::id),
        blend: if cutout {
            ModelBlend::AlphaTest
        } else {
            ModelBlend::Blend
        },
        two_sided,
        is_wmo: false,
        is_interior: false,
        is_emissive: false,
        is_additive: false,
        fade_variant: cutout,
        no_depth_write: false,
        no_depth_test: false,
        fog_policy: benilla_formats::FogPolicy::Scene,
        // A depth-prime twin writes no colour at all, so which UV it would have sampled cannot
        // reach a pixel — one twin per source batch, whatever its texcoord source.
        env_map: false,
        shade: ShadeSel::Lit,
        batch_order: 0,
        uv_anim: None,
        rgb_anim: None,
        wmo_class: None,
        sidn: None,
        window: false,
        zfill: true,
        // A depth-prime twin exists to write depth for a fading model; the sky writes none, so no
        // skybox batch ever has one (`M2BatchMaterials::skybox` builds no twins at all).
        sky_depth: false,
        // A z-fill twin carries no animated channel at all (no texture, no uv/rgb anim), so it can
        // never be the per-placement lane — it stays the one shared depth prime per batch.
        instance: None,
    };
    if let Some(h) = cache.fetch(&key) {
        return h;
    }
    let cull_mode = if two_sided { None } else { Some(Face::Back) };
    let base = StandardMaterial {
        base_color_texture: texture,
        // Blend keeps the twin in the transparent pass, where the sort bias can place it before
        // the colour parts; `specialize` masks the colour writes off and turns blend off.
        alpha_mode: AlphaMode::Blend,
        depth_bias: ZFILL_SORT_BIAS,
        double_sided: two_sided,
        cull_mode,
        ..default()
    };
    let handle = lazy::defer(
        materials,
        ExtendedMaterial {
            base,
            extension: WowModelExt {
                // Bit 10 (the twin-cutout marker) drives the shader's hard 224/255 cutout — set
                // exactly when the colour pass discards, so depth and colour coverage agree.
                clutter_fade: Vec4::new(
                    0.0,
                    0.0,
                    f32::from(ZFILL_MARKER | if cutout { TWIN_CUTOUT_MARKER } else { 0 }),
                    0.0,
                ),
                // Not a fade twin (`model_flags.y` = 0): the zfill pipeline branch forces its own
                // depth-write, and the cutout rides bit 10 above, so the bit has no work here.
                model_flags: Vec4::ZERO,
                sun_scale: Vec4::new(ShadeSel::Lit.selector(), 0.0, 0.0, 0.0),
                tint: Vec4::new(1.0, 1.0, 1.0, 0.0),
                sidn: Vec4::ZERO,
                anim_slots: Vec4::ZERO,
                light_buf: light.clone(),
            },
        },
    );
    cache.insert(key, handle.clone());
    handle
}

/// `clutter_fade.z` marker bit 11: this material is a **far-side-of-water twin** ([`far_twin_of`])
/// — the water-plane interleave's mesh lane ([`crate::sky_order::FAR_SIDE_BIAS`], where the byte
/// story lives). `WowModelExt::specialize` keys on it to keep the huge negative SORT rung out of
/// the rasterizer `DepthBiasState` (at −4e4 that constant is a ~0.5% relative depth pull — enough
/// to clip a blade's coplanar sheen/glow layers behind the blade's own opaque depth).
pub(crate) const FAR_SIDE_MARKER: u16 = 1 << 11;

/// The far-side twin of one transparent model material: the same look, dropped one water rung —
/// [`crate::sky_order::FAR_SIDE_BIAS`] under its authored sort slot — so the water surface paints
/// over it, plus the [`FAR_SIDE_MARKER`] pipeline bit that keeps the rung sort-only. Pure, so the
/// twin's shape is testable without an `Assets` store; the identity holds for every variant the
/// swap can meet (steady blend, fade twin, zfill twin — their own marker bits ride along).
/// `pub(crate)`: the pipeline warm pass ([`crate::pipe_warm`]) builds its far-side warm variants
/// through this same builder, so the twin's key encoding can never drift from the swap's.
pub fn far_twin_of(near: &WowModelMaterial) -> WowModelMaterial {
    let mut far = near.clone();
    far.base.depth_bias = far_sort_bias(far.base.depth_bias);
    far.extension.clutter_fade.z = far_markers(far.extension.clutter_fade.z);
    far
}

/// Fractional pull on every far-side sort slot, so the whole batch-eps band shares ONE pipeline
/// key. Bevy folds `depth_bias as i32` into the material's pipeline key (0837's law): unpulled,
/// a batch-order-0 twin lands exactly on `FAR_SIDE_BIAS` while its eps-nudged siblings land a
/// fraction above — truncating to TWO key integers, i.e. two live pipeline compiles where the
/// descriptors are identical (`specialize` zeroes the raster constant for far twins either way).
/// Pulled by just under 1, every `near ∈ [0, 0.99]` twin truncates to the same integer. Sort-side
/// it is a uniform shift of the whole band, so nothing reorders (the effect lane's far draws sit
/// `rung ≥ 1` above, and the margin only grows).
const FAR_KEY_PULL: f32 = 0.99;

/// The twin's sort slot: one water rung under wherever the source sat (a zfill twin keeps its −8
/// under the far batches it primes; a colour batch keeps its batch eps), less [`FAR_KEY_PULL`]
/// so the eps band collapses onto one pipeline-key integer.
fn far_sort_bias(near: f32) -> f32 {
    near + crate::sky_order::FAR_SIDE_BIAS - FAR_KEY_PULL
}

/// The twin's `clutter_fade.z` marker word: [`FAR_SIDE_MARKER`] added, every other pipeline
/// marker preserved (a fade twin's cutout, a zfill twin's prime, the multiply blends and the fog
/// policy all keep their identity on the far side of the plane).
fn far_markers(z: f32) -> f32 {
    f32::from(z as u16 | FAR_SIDE_MARKER)
}

/// The far twins alive right now: near material id → far twin, and the far id → near handle
/// restore path. Both handles are strong — a twin pair lives exactly as long as some batch entity
/// still carries its near identity (the sweep at the tail of [`classify_water_side`]), the same
/// entity-bound lifetime `FadeMaterials`' handles get from their component.
#[derive(Resource, Default)]
pub struct FarSideTwins {
    to_far: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    to_near: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
}

impl FarSideTwins {
    /// The far twin of `near`, if one is live — the read-only side every material-handle owner
    /// composes with, safe from a parallel walk. A miss means [`classify_water_side`] hasn't
    /// built it yet: the caller keeps the near handle and picks the twin up next frame.
    pub(crate) fn far_of(
        &self,
        near: &Handle<WowModelMaterial>,
    ) -> Option<&Handle<WowModelMaterial>> {
        self.to_far.get(&near.id())
    }

    /// The near identity of a far twin, if `id` is one — the mat-anim draw scan asks, because
    /// its registry keys are always near ids while a far-classified instance carries the twin's
    /// (1375).
    pub(crate) fn near_of(
        &self,
        id: AssetId<WowModelMaterial>,
    ) -> Option<AssetId<WowModelMaterial>> {
        self.to_near.get(&id).map(bevy::asset::Handle::id)
    }
}

/// The far-axis compose every material-handle OWNER applies at its own write site: the handle it
/// wants, dropped one water rung when the entity is marked [`FarSideOfWater`]. This is what lets
/// N legitimate writers (the fade resolve, the aura author, the self feather, the doodad-fade
/// authority) and the classifier all converge on ONE handle per frame — each derives its pick
/// from state and composes the same axis, so change-gates hold and nobody re-swaps what another
/// just wrote (the first cut fought exactly that way, per frame, forever). A cutout/opaque pick
/// composes to itself: far twins exist only for transparent-pass materials, so the lookup misses
/// and the pick stands.
pub fn far_resolved<'a>(
    want: &'a Handle<WowModelMaterial>,
    far: bool,
    twins: &'a FarSideTwins,
) -> &'a Handle<WowModelMaterial> {
    if far {
        twins.far_of(want).unwrap_or(want)
    } else {
        want
    }
}

/// This batch entity's transparent draw is on the eye's FAR side of the water plane — written by
/// [`classify_water_side`], sparse (present only while far). For entities whose material handle
/// the Visibility authority owns (the `DoodadFade` holders it pins to cutout/blend every frame,
/// decision 0025's one-authority law), the marker IS the classification: the authority composes
/// it into its own pick via [`FarSideTwins::far_of`] instead of a second writer fighting it —
/// the same compose-don't-overwrite shape the fade alpha itself takes through that system.
#[derive(Component)]
pub struct FarSideOfWater;

/// Classify every transparent M2 batch against the water plane and swap it onto (or off) its
/// far-side twin — the MESH half of the water-plane interleave (`0x707680`: the reference splits
/// CM2Scene transparents into above/below-water lists per model and draws the eye's far side
/// *before* the water pass; 0911 shipped the effect
/// half and named this one — the sighting it predicted arrived as "the sword reads crisp through
/// the surface": the blade's Mod2x sheen and glow overlays drew after the water, untinted).
///
/// The grain is the reference's: every batch entity of one model instance carries the instance's
/// own transform (the mesh offsets live in vertex space), so classifying each at its
/// `GlobalTransform` IS the per-model split — one liquid hit at the model's placement, all its
/// batches on one list. A slot-bearing instance inside the reference's `±r` band is a
/// **straddler** and takes BOTH lists: its batches stay near and gain a far twin, each clipped at
/// the waterline (the reference's `M2UseClipPlanes`; decision 2188, [`crate::straddle`]).
/// Slot-0 content keeps the one-list sign test. WMO translucents never classify (the reference's
/// lists are CM2Scene's; the WMO leg draws its own), and opaque/cutout batches settle by depth
/// like any world geometry.
///
/// **Two lanes, one classification.** Entities whose handle the Visibility authority owns — the
/// `DoodadFade` holders `debug_panel::apply_model_visibility` pins to cutout/blend every frame —
/// get the [`FarSideOfWater`] marker and a pre-built twin; the authority composes both into its
/// own pick (decision 0025's one-authority law — the first cut swapped their handles from here
/// and the two writers re-swapped the same 550 coastal doodads every frame, forever). Everything
/// else (equipment, spell-fx attachments, entity fade twins) is swapped here directly, after the
/// fade resolve, deriving near-vs-far from the CURRENT handle so an upstream overwrite is
/// re-classified the same frame instead of fought over.
///
/// **Reactive, not per-frame** (decision 0930; 0922 named the lever). A batch's verdict is a
/// function of its transform, its handle, its layers, its ancestors' room claim, the loaded
/// surfaces, and the eye's side — so a *clean* batch is skipped, and only the changed set
/// re-classifies (~2.6k of ~40k parts on a measured Stormwind frame; the full sweep cost 2.2 ms).
/// The frame promotes to the full walk — byte-identical to the old per-frame behaviour — whenever
/// a global term that changes VERDICTS moves: the eye crosses the surface (every verdict
/// inverts) or surfaces stream in/out. A part *despawn* changes no live verdict — it only
/// obligates the twin GC's exact mark eventually — so it schedules a full walk behind
/// [`GC_DEADLINE_SECS`] instead of promoting the same frame (decision 1462: walking streams
/// parts out on 12–22 frames/s, and each promotion was a full re-classify of every part —
/// lane B of 1461's walking tax; between full frames a pair orphaned by a handle overwrite —
/// a fade settling to its cutout — lingers harmlessly, kept fresh by the mirror above, until
/// the next edge sweeps it, and a GC-due twin lingers the same way). The room-claim trigger
/// fans DOWN: the claim lives on the unit root, the batches are its descendants, and a claim
/// can change with the unit standing still (a building streaming in under it re-claims at the
/// same spot).
/// How long a despawn-obligated twin GC may wait for its full walk (1462). Bounds how long an
/// orphaned twin pair can outlive its last user — a handful of small material clones — while a
/// walking regime's continuous despawn stream costs ~0.5 full walks/s instead of 12–22.
const GC_DEADLINE_SECS: f32 = 2.0;

#[allow(clippy::type_complexity)]
pub(crate) fn classify_water_side(
    interleave: crate::particles::WaterInterleave,
    mut twins: ResMut<FarSideTwins>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    mut near_edits: MessageReader<AssetEvent<WowModelMaterial>>,
    mut commands: Commands,
    // The material-handle leg needs a ParamSet: `Changed<MeshMaterial3d>` reads the same ticks
    // the walk's `&mut` writes (Bevy B0001), so the dirty-entity list is collected from `p0`
    // before `p1` is borrowed. The transform/layers legs read ticks the walk only reads, so they
    // stay plain sibling queries.
    mut set: ParamSet<(
        Query<
            Entity,
            (
                With<GlobalTransform>,
                Changed<MeshMaterial3d<WowModelMaterial>>,
            ),
        >,
        Query<
            (
                Entity,
                &GlobalTransform,
                Option<&bevy::camera::visibility::RenderLayers>,
                &mut MeshMaterial3d<WowModelMaterial>,
                Has<crate::model_fade::DoodadFade>,
                Has<FarSideOfWater>,
                Option<&MeshTag>,
                Has<crate::straddle::StraddlesWater>,
            ),
            // A straddle twin is the classification's OUTPUT, never its input.
            Without<crate::straddle::StraddleTwinOf>,
        >,
    )>,
    moved: Query<
        Entity,
        (
            With<MeshMaterial3d<WowModelMaterial>>,
            Or<(
                Changed<GlobalTransform>,
                Changed<bevy::camera::visibility::RenderLayers>,
            )>,
        ),
    >,
    reclaimed: Query<Entity, Changed<crate::wmo_portal::UnitWmoRoom>>,
    // The straddle verdict's edge (decision 2188): an instance entering or leaving its water band
    // re-classifies its whole subtree — the same fan-down as the room claim.
    rebanded: Query<Entity, Changed<crate::straddle::ModelWaterBand>>,
    clips: Res<crate::straddle::WaterClips>,
    children: Query<&Children>,
    mut removed: RemovedComponents<MeshMaterial3d<WowModelMaterial>>,
    time: Res<bevy::time::Time>,
    mut eye_was_submerged: Local<Option<bool>>,
    mut gc_due: Local<Option<f32>>,
) {
    // Mirror any near-asset edit into the live twin. Since decision 1381 the texanim/tint lane
    // animates through the shared table (the twin's clone carries the same slot and seed, so it
    // scrolls in phase with no mirror needed), which leaves this listener idle in the steady
    // state — it now catches only genuine one-off near edits (a debug repaint, a future lane).
    // Our own twin writes touch only far ids (never `to_far` keys), so this can't feed back.
    let edited: Vec<AssetId<WowModelMaterial>> = near_edits
        .read()
        .filter_map(|ev| match ev {
            AssetEvent::Modified { id } if twins.to_far.contains_key(id) => Some(*id),
            _ => None,
        })
        .collect();
    for id in edited {
        if let (Some(near), Some(far)) = (materials.get(id).cloned(), twins.to_far.get(&id)) {
            let far = far.id();
            // Can't fail: `to_far` holds the twin's strong handle, so its slot is live.
            materials
                .insert(far, far_twin_of(&near))
                .expect("far twin slot is strongly held");
        }
    }
    let eye = interleave.eye_submerged();
    let now = time.elapsed_secs();
    // `last()` drains the reader — `next()` would leave the tail readable and re-arm NEXT frame
    // as a spurious second deadline. The earliest despawn holds the deadline (`get_or_insert`):
    // a steady despawn stream must not push the GC out forever.
    if removed.read().last().is_some() {
        gc_due.get_or_insert(now + GC_DEADLINE_SECS);
    }
    let full = *eye_was_submerged != Some(eye)
        || interleave.surfaces_changed()
        || gc_due.is_some_and(|due| now >= due);
    *eye_was_submerged = Some(eye);
    if full {
        // Any full walk runs the GC's exact mark, whatever triggered it — a pending deadline
        // is satisfied, not merely postponed.
        *gc_due = None;
    }

    if full {
        // The full walk — every part, plus the twin GC's exact mark-and-sweep.
        let mut used: std::collections::HashSet<AssetId<WowModelMaterial>> =
            std::collections::HashSet::new();
        for item in set.p1().iter_mut() {
            classify_part(
                &interleave,
                &clips,
                &mut twins,
                &mut materials,
                &mut commands,
                Some(&mut used),
                item,
            );
        }
        // Drop twin pairs whose near identity no live batch carries any more — the twin dies
        // with its users. A respawned model refetches its near handle from its spawner's cache
        // and the pair rebuilds lazily on the next far classification.
        if !twins.to_far.is_empty() {
            let stale: Vec<AssetId<WowModelMaterial>> = twins
                .to_far
                .keys()
                .filter(|k| !used.contains(*k))
                .copied()
                .collect();
            for k in stale {
                if let Some(f) = twins.to_far.remove(&k) {
                    twins.to_near.remove(&f.id());
                }
            }
        }
    } else {
        // The reactive frame: only the changed set. `p0` collects first — the ParamSet frees it
        // before `p1` — then the transform/layers leg and the room-claim fan-out come straight
        // off their filters. An entity dirty on two legs classifies twice; the second pass
        // no-ops (`far == far_now`).
        let handle_dirty: Vec<Entity> = set.p0().iter().collect();
        let mut parts = set.p1();
        for e in moved.iter().chain(handle_dirty) {
            if let Ok(item) = parts.get_mut(e) {
                classify_part(
                    &interleave,
                    &clips,
                    &mut twins,
                    &mut materials,
                    &mut commands,
                    None,
                    item,
                );
            }
        }
        // A changed claim — or a changed straddle band (2188) — re-classifies the holder's whole
        // subtree: both live on the unit root, the batches are its descendants (a nested holder
        // shadows for its own subtree; re-classifying it from here anyway is idempotent).
        let mut stack: Vec<Entity> = reclaimed.iter().chain(rebanded.iter()).collect();
        while let Some(e) = stack.pop() {
            if let Ok(ch) = children.get(e) {
                stack.extend(ch.iter());
            }
            if let Ok(item) = parts.get_mut(e) {
                classify_part(
                    &interleave,
                    &clips,
                    &mut twins,
                    &mut materials,
                    &mut commands,
                    None,
                    item,
                );
            }
        }
    }
}

/// One transparent-candidate batch as [`classify_part`] sees it: the entity, its transform, its
/// layers, its handle (the channel this lane may swap), whether the Visibility authority owns
/// that handle, whether it is marked far, its tag (the instance slot the straddle verdict is
/// keyed on), and whether it is marked straddling.
type PartItem<'a> = (
    Entity,
    &'a GlobalTransform,
    Option<&'a bevy::camera::visibility::RenderLayers>,
    Mut<'a, MeshMaterial3d<WowModelMaterial>>,
    bool,
    bool,
    Option<&'a MeshTag>,
    bool,
);

/// One batch entity's classification — the shared body of [`classify_water_side`]'s full and
/// reactive paths. `used` collects the near identities live batches carry (the twin GC's mark);
/// the reactive path passes `None` and leaves the sweep to the next full frame.
fn classify_part(
    interleave: &crate::particles::WaterInterleave,
    clips: &crate::straddle::WaterClips,
    twins: &mut FarSideTwins,
    materials: &mut Assets<WowModelMaterial>,
    commands: &mut Commands,
    used: Option<&mut std::collections::HashSet<AssetId<WowModelMaterial>>>,
    (entity, gt, layers, mut mat, authority_owned, marked, tag, straddling): PartItem<'_>,
) {
    use bevy::camera::visibility::RenderLayers;
    // A booth-layered batch belongs to its own camera and scene — no world water applies
    // (the effect lane's own booth guard, `particles::sim`).
    if layers.is_some_and(|l| !l.intersects(&RenderLayers::default())) {
        return;
    }
    let cur = mat.0.id();
    let (near, swapped) = match twins.to_near.get(&cur) {
        Some(n) => (n.clone(), true),
        None => (mat.0.clone(), false),
    };
    // This lane's current verdict: the marker (what every composing owner reads); the raw
    // handle state seconds it for the lanes this system swaps itself.
    let far_now = marked || swapped;
    let qualifies = match materials.get(near.id()) {
        Some(m) => {
            matches!(m.base.alpha_mode, AlphaMode::Blend)
                && m.extension.model_flags.x <= 0.5
                // A WMO skybox is anchored at the eye and forces the far depth: it has no side of
                // the water plane to be on, and the far twin's −4e4 would sink its own sort rung
                // into the world band. The sky is never part of the interleave.
                && (m.extension.clutter_fade.z as u16) & SKY_DEPTH_MARKER == 0
        }
        None => false,
    };
    if !qualifies {
        // A marked part that settled back onto its opaque cutout (fade over, aura released)
        // stops being a transparent draw at all — its side is nobody's business until it
        // feathers again, and a doubled one stops being doubled.
        if marked {
            commands.entity(entity).remove::<FarSideOfWater>();
        }
        if straddling {
            commands
                .entity(entity)
                .remove::<crate::straddle::StraddlesWater>();
        }
        return;
    }
    if let Some(used) = used {
        used.insert(near.id());
    }
    // **The straddle split** (decision 2188, [`crate::straddle`]): an instance inside its water
    // band draws on BOTH lists. This batch keeps its NEAR identity — the fragment clips it to the
    // eye's half — and `sync_straddle_twins` hangs its far twin beside it, clipped to the other.
    // The verdict is the per-slot word the clip itself reads, so a batch is doubled exactly when
    // its fragments are halved.
    let straddles = tag.is_some_and(|t| clips.straddles(crate::mesh_tag::rig_of(t.0)));
    if straddles != straddling {
        trace_side(
            if straddles { "straddle" } else { "unstraddle" },
            entity,
            gt,
        );
        if straddles {
            commands
                .entity(entity)
                .insert(crate::straddle::StraddlesWater);
        } else {
            commands
                .entity(entity)
                .remove::<crate::straddle::StraddlesWater>();
        }
    }
    if straddles {
        // Built here, where the store is writable — the twin sync only reads the map.
        far_twin(twins, materials, &near);
    }
    let far = !straddles
        && crate::particles::far_side_of_water(interleave, Some(entity), gt.translation());
    if far == far_now {
        return;
    }
    // The swap's own trace (`WOW_MOVE_TRACE_TAGS=fx`): which batches crossed the plane this
    // frame and which way — the numeric read for a sort question no pixel can answer, and
    // naturally sparse (transitions only, never per-frame spam).
    trace_side(if far { "far-side" } else { "near-side" }, entity, gt);
    if far {
        // Build (or fetch) the twin either way — every composing owner needs it live in the
        // map before its own pick can resolve it.
        let far_h = far_twin(twins, materials, &near);
        commands.entity(entity).insert(FarSideOfWater);
        if !authority_owned {
            mat.0 = far_h;
        }
    } else {
        commands.entity(entity).remove::<FarSideOfWater>();
        if !authority_owned {
            mat.0 = near;
        }
    }
}

/// The far twin of `near`, built on first ask. `near` must be a live asset — every caller has
/// just proved it through the classifier's `qualifies`.
fn far_twin(
    twins: &mut FarSideTwins,
    materials: &mut Assets<WowModelMaterial>,
    near: &Handle<WowModelMaterial>,
) -> Handle<WowModelMaterial> {
    if let Some(h) = twins.to_far.get(&near.id()) {
        return h.clone();
    }
    let twin = far_twin_of(materials.get(near.id()).unwrap());
    let h = materials.add(twin);
    twins.to_far.insert(near.id(), h.clone());
    twins.to_near.insert(h.id(), near.clone());
    h
}

/// `WOW_MOVE_TRACE_TAGS=fx`: one line per side transition of one batch — `far-side`/`near-side`
/// for the one-list law, `straddle`/`unstraddle` for the two-list band.
fn trace_side(what: &str, entity: Entity, gt: &GlobalTransform) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    let p = gt.translation();
    benilla_assets::trace::line(
        "fx",
        &format!(
            "{what} mesh e={entity} at=[{:.1},{:.1},{:.1}]",
            p.x, p.y, p.z
        ),
    );
}

/// Replace the fog-policy bits (4-6) inside a packed `clutter_fade.z` marker word, preserving
/// every other pipeline marker — bits 0-3 AND the Mod/Mod2x multiply markers in bits 7-8
/// (decision 0528). The mask lives here, beside the packer above: the portrait booth's rig twin
/// once truncated the word to `u8` and hand-masked `& 0x0f`, silently dropping the multiply
/// markers — the char-select white-blade regression.
pub fn replace_fog_policy(z: f32, policy: benilla_formats::FogPolicy) -> f32 {
    f32::from((z as u16 & !(7 << 4)) | ((policy as u16) << 4))
}

/// The model-render lane's own registrations (decision 1163, stage zero).
///
/// `FarSideTwins` is engine state in an engine module, and it was initialised by `EntitiesPlugin`
/// purely because `model_render` had no plugin to put it in — which is how the world viewer's very
/// first survey run found the model-`Visibility` authority reading a resource that did not exist.
pub fn plugin(app: &mut App) {
    // The water-plane interleave's MESH half (the sibling of the effect half in `particles::sim`):
    // every transparent M2 batch classifies against the water plane and takes its far-side twin, so
    // the surface paints over a submerged model's sheen/glow layers. Two lanes: entities whose
    // handle the Visibility authority owns (the `DoodadFade` holders it pins every frame, decision
    // 0025) get a MARKER the authority composes into its own pick — so classify runs before
    // `ModelVisSet`, and the pick sees this frame's side; everything else (equipment, spell-fx,
    // fade twins) is swapped here directly, after the fade resolve whose choice it re-derives from
    // the current handle. The self feather stays deliberately unordered: it composes the same
    // marker through `far_resolved`, so the two writers can only disagree on a frame the marker
    // itself flips — a one-frame stale side on the player's own parts at the instant the eye
    // crosses the surface, inside the whole-screen atmosphere swap that crossing already is (the
    // same accepted class as the interior law's one-frame lag, 0919).
    app.init_resource::<FarSideTwins>().add_systems(
        Update,
        classify_water_side
            .after(crate::liquid::SubmersionVerdict)
            .after(crate::model_fade::apply_render_fade)
            .before(crate::model_render::ModelVisSet),
    );
    // **The model-`Visibility` authority** (decision 0025's one-writer law), which lived in
    // `debug_panel` because the panel's toggles were its first input and now lives with the lane
    // it is the authority *for*. `apply_model_visibility` reads the WMO portal PVS, so it runs
    // after the compute that fills it.
    app.add_systems(
        Update,
        visibility::apply_model_visibility
            .after(crate::wmo_portal::WmoPvsSet)
            .in_set(ModelVisSet),
    );
    // `WOW_VIS_TRACE=<label>`: watch one model's placements decide (see `visibility::VisTrace`).
    // The resource is absent unless the env var names something, and the system's first line is
    // the `None` check — so an ordinary run pays one `Option` read per frame.
    if let Some(trace) = visibility::VisTrace::from_env() {
        app.insert_resource(trace);
    }
    app.add_systems(
        Update,
        visibility::trace_model_visibility.after(ModelVisSet),
    );
    // The material dedup cache and its two evictors — the soft one (distance, decision 0785) and
    // the hard one (a cross-map transition, decision 0729). Both used to live in `entities`, which
    // owned the cache because the entity spawner was the first lane to want one; the glue booth
    // and the portrait bake each kept a `Local` that nothing ever swept, which is exactly the
    // unbounded residency `art_scope` was written to end.
    app.init_resource::<ModelMaterials>()
        .add_systems(Update, (scope_model_materials, evict_model_materials));
    // Deferred realization (`lazy`): a built material becomes an asset the frame something
    // visible binds it. The top of `PostUpdate` — `Assets::insert` only QUEUES its
    // `AssetEvent::Added`, so the sweep has to land before the store publishes
    // (`AssetEventSystems`) or the bound entity loses a frame of drawing, and before
    // `VisibilityPropagate` clears the view-visibility bits it reads. Both bounds and why they
    // are the only two available: `lazy`'s module doc.
    app.add_systems(
        PostUpdate,
        lazy::realize_bound
            .before(bevy::asset::AssetEventSystems)
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
    );
    // Parking (`park`): a long-hidden streamed part puts its `Mesh3d` down. `Update`, after the
    // authority, on last frame's propagated verdict — the order the module doc explains.
    if park::enabled() {
        app.add_systems(Update, park::park_hidden_parts.after(ModelVisSet));
    }
}

/// Expire the material dedup by **distance** (decision 0785).
fn scope_model_materials(mut scope: crate::art_scope::ArtScope, mut mats: ResMut<ModelMaterials>) {
    scope.apply(&mut mats.0, crate::art_scope::ArtSlot::ModelMats);
}

/// …and drop it whole on a cross-map transition (decision 0729 — see `MapChange` for why a clear
/// is always safe mid-session). Every entry is get-or-insert at its use site, so a cleared one
/// rebuilds on the next spawn that wants it.
fn evict_model_materials(
    mut changes: MessageReader<crate::world_map::MapChange>,
    mut mats: ResMut<ModelMaterials>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    info!("model materials evicted: {}", mats.0.len());
    mats.0.clear();
}

/// Column order for the four [`ModelKind`]s wherever one is used as an index — the panel's
/// toggles, its per-kind counts. `ModelKind`'s own index is private; this is the shared one.
pub fn kind_index(kind: ModelKind) -> usize {
    match kind {
        ModelKind::Doodad => 0,
        ModelKind::Wmo => 1,
        ModelKind::Creature => 2,
        ModelKind::GameObject => 3,
    }
}

/// Column order for the five [`ModelBlend`] layers — same role as [`kind_index`].
pub fn blend_index(b: ModelBlend) -> usize {
    match b {
        ModelBlend::Opaque => 0,
        ModelBlend::AlphaTest => 1,
        ModelBlend::Blend => 2,
        ModelBlend::Mod => 3,
        ModelBlend::Mod2x => 4,
    }
}

/// Which world-model subsystem a spawned submesh belongs to — lets the panel scope toggles (e.g.
/// hide doodads without touching NPCs).
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModelKind {
    Doodad,
    Wmo,
    Creature,
    GameObject,
}

/// Tags every spawned model submesh with the metadata the panel toggles on: its subsystem and its
/// blend mode (the "layer" — opaque trunk vs alpha-cut canopy).
#[derive(Component, Clone, Copy)]
pub struct ModelPart {
    pub kind: ModelKind,
    pub blend: ModelBlend,
}

/// **Why this submesh is an entity at all** — the world streamer's answer, one static label per
/// batch, for the census (`VIS_CENSUS … why …`). A city keeps thousands of hidden model
/// submeshes resident on the entity path while the retained pass draws the world (5.6k ADT
/// doodad submeshes at the Stormwind auction house, 1945), and every one of them rides every
/// per-`Mesh3d` sweep; which divert declined it is the first question about pricing them.
/// Absent on units, GameObjects and everything the app lane spawns.
#[derive(Component, Clone, Copy)]
pub struct EntityPathWhy(pub &'static str);

/// Ordering handle so the one system allowed to *override* the model-`Visibility` authority — the
/// self-avatar first-person hide ([`crate::player`]) — can run **after** it and win the frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelVisSet;

#[cfg(test)]
mod tests {
    use super::replace_fog_policy;
    use benilla_formats::FogPolicy;

    /// Swapping the fog policy must preserve EVERY pipeline marker bit — 0-3 and the 0528
    /// multiply markers in 7-8. The portrait booth's old hand-rolled `as u8 & 0x0f` dropped
    /// bits 7-8 (the char-select white-blade regression this pins against).
    #[test]
    fn replace_fog_policy_preserves_pipeline_markers() {
        // All marker bits set (0-3, 7-8) + fog policy Grey (3) in bits 4-6.
        let z = f32::from(0b1_1000_1111u16 | (3 << 4));
        let out = replace_fog_policy(z, FogPolicy::Off) as u16;
        assert_eq!(out & 0b1_1000_1111, 0b1_1000_1111, "markers preserved");
        assert_eq!((out >> 4) & 7, FogPolicy::Off as u16, "fog swapped");
        // And the reverse direction: no stray bits invented.
        let out2 = replace_fog_policy(f32::from(0u16), FogPolicy::Grey) as u16;
        assert_eq!(out2, (FogPolicy::Grey as u16) << 4);
    }

    /// The authored-batch-order sort epsilon sits in its working band: any realistic model's
    /// last batch still biases UNDER the effect lane's owner-last rung floor (an item's glow
    /// particles must draw after every batch of the item — 0719/0721), and one step is far above
    /// f32 noise on a far-scene sort distance (~4e-5 at 500 yd), so the tie actually breaks.
    /// Pins the constant against a careless resize in either direction — either end failing is
    /// the Naxx-item strobe (coplanar layers re-flipping a sort tie every frame) or its dual
    /// (effects interleaving into their owner's batches).
    #[test]
    fn batch_order_sort_eps_sits_between_f32_noise_and_the_effect_rung() {
        // The CAPPED product is the deepest any batch can bias — "512 is past any shipped
        // model" was this test's original premise, and Stormwind's root batch tables disproved
        // it (indices past 2000; the 0938-tail live compiles at biases 1 and 2).
        let deepest =
            (super::BATCH_ORDER_SORT_EPS * f32::from(u16::MAX)).min(super::BATCH_ORDER_SORT_CAP);
        assert!(
            deepest < benilla_formats::owner_last_rung(0.0),
            "a model's last batch must still sort before its own effects' rung"
        );
        let far_noise = 500.0_f32 * f32::EPSILON;
        assert!(
            super::BATCH_ORDER_SORT_EPS > 8.0 * far_noise,
            "one order step must dominate f32 noise on a far sort distance"
        );
        // Bevy's pipeline key truncates the WHOLE band to 0 — no batch index may mint a
        // pipeline (0837, re-learned in 0938's tail): the cap holds for any u16.
        assert_eq!(deepest as i32, 0);
        const {
            assert!(super::BATCH_ORDER_SORT_CAP < 1.0);
            // A capped batch's far twin still truncates onto the far band's ONE key integer.
            assert!(super::BATCH_ORDER_SORT_CAP < super::FAR_KEY_PULL);
        }
        assert_eq!(
            super::far_sort_bias(super::BATCH_ORDER_SORT_CAP) as i32,
            super::far_sort_bias(0.0) as i32,
            "the far band stays one pipeline key across the whole capped eps range"
        );
    }

    /// The **WMO-skybox band**: the four properties the lane's whole ordering rests on, none of
    /// which survives being reasoned about in prose (decision 1264). The band is arithmetic at a
    /// magnitude where f32 stops being able to represent the steps, which is exactly how the
    /// ordinary eps would have failed here in silence.
    #[test]
    fn the_skybox_band_orders_its_batches_on_one_pipeline_key() {
        // `CavernsOfTimeSky.m2` is 21 batches; `order` is the index + 1.
        let band: Vec<f32> = (0..=21).map(super::skybox_sort_bias).collect();

        // 1. STRICTLY INCREASING — the whole point. At −6e4 the ordinary 1e-3 eps rounds every
        //    one of these onto the same f32 and the sort tie the eps exists to break comes back.
        for pair in band.windows(2) {
            assert!(
                pair[1] > pair[0],
                "skybox batch order does not resolve at the rung's magnitude: {pair:?} — \
                 SKYBOX_ORDER_EPS is under the f32 ulp there"
            );
        }

        // 2. ONE PIPELINE KEY for the whole band (0837's law: `StandardMaterialKey` packs
        //    `depth_bias as i32`, so a band straddling an integer is a live compile per side).
        //    Checked for ANY u16 order, not just the 21 real ones — the cap is what holds it.
        let key = band[0] as i32;
        for order in [0u16, 1, 21, 57, 58, 1000, u16::MAX] {
            assert_eq!(
                super::skybox_sort_bias(order) as i32,
                key,
                "batch order {order} mints a second skybox pipeline"
            );
        }

        // 3. UNDER EVERY WORLD TRANSPARENT. The deepest a world draw can sort is the far-side
        //    water rung plus a far-plane view-z; the skybox's own camera-anchored view-z (its
        //    shell radius, ~94 yd for Caverns of Time) rides on top of the rung and must not
        //    close the gap.
        const FAR_PLANE: f32 = 3000.0;
        const SHELL: f32 = 128.0;
        let deepest_world = crate::sky_order::FAR_SIDE_BIAS - FAR_PLANE - super::FAR_KEY_PULL;
        assert!(
            band.last().unwrap() + SHELL < deepest_world,
            "a skybox can sort in front of a world transparent — the painted sky would paint \
             over it (sky_order::WMO_SKYBOX_BIAS)"
        );

        // 4. The rung is the SMALLEST job it can be, per `sky_order`'s "as small as its ordering
        //    job allows": a magnitude an order over the world band is margin, two is drift.
        const {
            assert!(crate::sky_order::WMO_SKYBOX_BIAS > 10.0 * crate::sky_order::FAR_SIDE_BIAS);
        }
    }

    /// The far-side twin is its source shifted exactly one water rung with the marker bit added —
    /// and every OTHER marker bit rides along untouched (a fade twin's cutout, a zfill twin's
    /// prime, the multiply blends and the fog policy all keep their pipeline identity on the far
    /// side of the plane). The band stays coherent: a far zfill twin still primes before the far
    /// colour batches it serves, and the deepest batch eps still clears the water rung's margin.
    #[test]
    fn far_twin_keeps_the_source_identity_one_rung_down() {
        assert_eq!(
            super::far_sort_bias(super::ZFILL_SORT_BIAS),
            crate::sky_order::FAR_SIDE_BIAS + super::ZFILL_SORT_BIAS - super::FAR_KEY_PULL,
            "one rung down from wherever the source sat (less the key-collapse pull)"
        );
        assert!(
            super::far_sort_bias(super::ZFILL_SORT_BIAS)
                < super::far_sort_bias(super::BATCH_ORDER_SORT_EPS * 512.0),
            "a far zfill twin still primes before its model's far colour batches"
        );
        // The pull's whole point: the entire batch-eps band truncates to ONE pipeline-key
        // integer (0837's law — `depth_bias as i32` is a key axis), and the zfill twin keeps
        // its own single key one bracket down.
        assert_eq!(
            super::far_sort_bias(0.0) as i32,
            super::far_sort_bias(super::BATCH_ORDER_SORT_EPS * 512.0) as i32,
            "batch-order far twins may never mint a second pipeline"
        );
        const {
            assert!(super::FAR_KEY_PULL > super::BATCH_ORDER_SORT_EPS * 512.0);
        }
        assert_eq!(
            super::far_sort_bias(super::ZFILL_SORT_BIAS) as i32,
            crate::sky_order::FAR_SIDE_BIAS as i32 + super::ZFILL_SORT_BIAS as i32,
            "the zfill far key stays put — the pull is sub-integer"
        );
        let src = super::ZFILL_MARKER | super::TWIN_CUTOUT_MARKER | (3 << 4);
        let z = super::far_markers(f32::from(src)) as u16;
        assert_ne!(z & super::FAR_SIDE_MARKER, 0, "the sort-only pipeline bit");
        assert_eq!(
            z & !super::FAR_SIDE_MARKER,
            src,
            "every other marker preserved"
        );
        // The marker word stays f32-exact with every bit up to FAR_SIDE set (integers ≤ 2^12
        // are exact in f32 — the packing's soundness bound).
        let all = src | super::FAR_SIDE_MARKER | 0x0f;
        assert_eq!(f32::from(all) as u16, all);
    }
}
