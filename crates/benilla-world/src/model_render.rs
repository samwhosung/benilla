//! The `WowModelMaterial` build for every M2 and WMO instance: a `StandardMaterial` base for
//! texture, alpha and cull plus the `WowModelExt` shading, deduped so a shared look is one draw.

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

/// Yards of transparent sort bias per authored batch step: a model's coplanar batches tie on one
/// mesh centre, and the reference keeps their authored order inside the instance's sort key
/// (`0x707db1`). Under [`benilla_formats::owner_last_rung`]'s 1-yd floor, over f32 noise at 500 yd.
pub(crate) const BATCH_ORDER_SORT_EPS: f32 = 1e-3;

/// The eps product's ceiling, since `depth_bias as i32` is a pipeline-key axis: under 1.0, and
/// under `FAR_KEY_PULL` for the far twins. Past index 899 batches tie.
pub(crate) const BATCH_ORDER_SORT_CAP: f32 = 0.9;

/// `WOW_NO_ALPHATEST=1`: every cutout batch draws opaque, in the static-gx bake too.
pub(crate) fn alphatest_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_ALPHATEST").is_some())
}

/// `WOW_WMO_BIAS=0`: the WMO batch clip-z nudge off.
fn wmo_bias_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_WMO_BIAS").as_deref(), Ok("0")))
}

/// The material-dedup key: batches that agree on every field share one material.
#[derive(PartialEq, Eq, Hash)]
pub struct MatKey {
    /// The bound light buffer: the world, a portrait booth and char select each bind their own.
    light: bevy::render::render_resource::BufferId,
    texture: Option<AssetId<Image>>,
    blend: ModelBlend,
    two_sided: bool,
    is_wmo: bool,
    is_interior: bool,
    is_emissive: bool,
    is_additive: bool,
    fade_variant: bool,
    /// M2 render flag 0x10 (0x08 for `no_depth_test`), honoured by `specialize`.
    no_depth_write: bool,
    no_depth_test: bool,
    /// The batch's fog colour policy, packed into `clutter_fade.z` bits 4-6.
    fog_policy: benilla_formats::FogPolicy,
    /// The batch samples the generated sphere-map coordinate, not its vertex UVs.
    env_map: bool,
    /// The static terrain-shade selector, fixed per placement.
    shade: ShadeSel,
    /// Authored batch index + 1 (0 = unordered): the transparent sort bias of every batch, and for
    /// WMO the clip-z nudge too (MOBA batches draw in strict file order, `0x6b4f10`/`0x6b5190`).
    batch_order: u16,
    /// The `Arc<UvAnim>` pointer, stable while its model is loaded: one material per UV loop.
    uv_anim: Option<usize>,
    /// The `Arc<RgbAnim>` pointer, keyed like [`Self::uv_anim`]; `None` is a static vertex tint.
    rgb_anim: Option<usize>,
    /// The WMO batch's MOBA section: an interior group's INT and TRANS batches light differently.
    wmo_class: Option<WmoBatchClass>,
    /// The WMO MOMT SIDN night-glow colour, gamma RGB bytes.
    sidn: Option<[u8; 3]>,
    /// The WMO MOMT WINDOW flag: an interior-group batch on the brighter midpoint light.
    window: bool,
    /// A depth-prime twin ([`zfill_material`], the reference's `M2UseZFill` clone at `0x707f7d`).
    zfill: bool,
    /// A WMO skybox batch ([`crate::skybox`]), with its own pipeline and sort rung.
    sky_depth: bool,
    /// The placement owning this material when its animated loop follows the instance's sequence
    /// (the BRM lava bubbles' flipbook); `None` for the shared material.
    instance: Option<Entity>,
}

/// The static terrain-shade selector in `sun_scale.x`, which the shader thresholds into the
/// reference's intensities (2.5 lit, 1.0 day/night, 0.5 shadowed; `[node+0xa4]`, `0x69e280`).
/// Entities are always [`ShadeSel::Lit`] and shade per instance by the `MeshTag` byte.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShadeSel {
    /// Entity M2s only: the 2.5 and its 3.3333/s chase live on the WENTITY light node (vtable
    /// `0x810810`, `[obj+0xe0]`), which [`crate::entity_shade`] ramps per instance into the
    /// `MeshTag` byte for units, players and GameObjects alike.
    Lit,
    /// Lit ground at 1.0: exterior WMO MODD props and ADT map doodads, never the 2.5 site; a map
    /// doodad's `CMapDoodadDef` `[+0xa4]` is only ever 0.0, 0.5 or 1.0.
    Matte,
    /// The base sits on MCSH-shadowed terrain: the reference's dim 0.5.
    Shaded,
    /// An authored M2 light rig (the glue create booth): the SH probe in slot 0 of the material's
    /// own light buffer, on the reference's `Model2.bls` curve, plus up to three nearest point
    /// lights; no sun or MCSH.
    Rig,
}

impl ShadeSel {
    /// The `sun_scale.x` value, matching `wow_model.wgsl`'s thresholds (1.5 rig, 0.85 lit,
    /// 0.5 matte, else shaded).
    pub fn selector(self) -> f32 {
        match self {
            ShadeSel::Lit => 1.0,
            ShadeSel::Matte => 0.6,
            ShadeSel::Shaded => 0.2,
            ShadeSel::Rig => 2.0,
        }
    }
}

/// A material-dedup cache, one per model-spawning subsystem; entries expire by distance if swept.
pub type MaterialCache = benilla_assets::SpatialCache<MatKey, Handle<WowModelMaterial>>;

/// Build or fetch the deduped [`WowModelMaterial`] for one model batch. `fade_variant` marks the
/// blend twin every fade rides, built with the source blend, never `Blend`: the twin always blends
/// (`0x70c1fd`) but its alpha test keys on the stored mode (`0x70c237`), so AlphaKey keeps its
/// 224/255 cutout and Opaque has none.
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
    env_map: bool,
    shade: ShadeSel,
    batch_order: u16,
    uv_anim: Option<&std::sync::Arc<benilla_formats::UvAnim>>,
    rgb_anim: Option<&std::sync::Arc<benilla_formats::RgbAnim>>,
    wmo_class: Option<WmoBatchClass>,
    sidn: Option<[u8; 3]>,
    window: bool,
    sky_depth: bool,
    light: &Buffer,
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
    // An additive batch is a (ONE, ONE) add of gamma-space premultiplied colour: the shader and
    // `specialize` must key on the same `clutter_fade.z` bit 2.
    let source_cutout = blend == ModelBlend::AlphaTest && !alphatest_disabled();
    let alpha_mode = if is_additive || fade_variant {
        // A fade twin always blends (`0x70c1fd`); its source blend only sets the cutout marker.
        AlphaMode::Blend
    } else {
        match blend {
            ModelBlend::Opaque => AlphaMode::Opaque,
            ModelBlend::AlphaTest if source_cutout => AlphaMode::Mask(VANILLA_ALPHA_KEY_REF),
            ModelBlend::AlphaTest => AlphaMode::Opaque,
            // Mod/Mod2x multiply what is drawn, so they need the scene under them; `specialize`
            // sets the multiply factors from the marker bits below.
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x => AlphaMode::Blend,
        }
    };
    // Single-sided unless the M2's 0x04 flag is set.
    let cull_mode = if two_sided { None } else { Some(Face::Back) };
    // Transparent batches only: opaque passes never sort by it. Tying past index 899 is safe:
    // the eps matters only for same-centre stacks (M2 item overlays), and WMO batches sort by
    // their own centres. Skybox transparents take their own rung under the world's; its painted
    // cube faces draw opaque, as in the reference.
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
        // No texture: the reference disables the texture stage and draws the flat vertex colour,
        // a white modulate whose alpha test passes everywhere (`0x59baa0`). Blend and cull still
        // apply, so an untextured fade twin keeps its alpha path.
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
                // x, y, w are the clutter fade (0 here); z packs the markers `specialize` keys on.
                clutter_fade: Vec4::new(
                    0.0,
                    0.0,
                    // Bits: 0 no depth write (M2 0x10), 1 no depth test (0x08), 2 additive,
                    // 3 opaque intent (the shader pins alpha to 1.0: Metal has bound opaque
                    // draws with blending on when an extra camera exists), 4-6 fog policy
                    // (`0x70baf0`), 7 Mod (DST_COLOR, ZERO) and 8 Mod2x (DST_COLOR, SRC_COLOR),
                    // exact because the framebuffer holds gamma as in the reference, 10 twin
                    // cutout (the source alpha-tests: 224/255 on the unfaded alpha; an Opaque
                    // source has no test, `0x70c237`), 12 env map, 13 sky depth.
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
                // x WMO (FFP N·L × MOCV), y fade variant, z WMO interior group (sun off), w unlit:
                // M2 UNLIT 0x01, or WMO UNLIT on an exterior group (`0x6b4f10`; the interior
                // drawer `0x6b5190` ignores it). M2 Mod/Mod2x are always unlit, and additive is
                // lit unless 0x01 is set (`DAT_00811fa8 = {1,1,1,1,1,0,0}`).
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
                // x the shade selector. y the WMO batch order, uniform data, not a pipeline key:
                // the vertex stage scales clip z by (1 + y·2⁻²³), so a later coplanar batch wins
                // in any draw order, as in MOBA order. zw the UV-animation offset at t = 0, the
                // seed the per-frame table row is added to: `0x714260` adds the un-pivoted
                // translation to the stage UVs, and no placed doodad rotates or scales them.
                sun_scale: {
                    let uv0 = uv_anim.map_or([0.0, 0.0], |a| a.sample(0.0));
                    // WMO only: M2 coplanar layers pass GreaterEqual at equal depth, and their
                    // order is the sort bias.
                    let order = if !is_wmo || wmo_bias_off() {
                        0.0
                    } else {
                        f32::from(batch_order)
                    };
                    Vec4::new(shade.selector(), order, uv0[0], uv0[1])
                },
                // xyz the animated M2Color tint's first key (white when static: the vertex colours
                // carry it). w the WMO interior class: INT batches draw unlit tex × MOCV and TRANS
                // batches lerp lit to baked by the MOCV alpha (`0x6b5190`); exterior and M2 are 0.
                tint: {
                    let t0 = rgb_anim.map_or([1.0, 1.0, 1.0], |a| a.sample(0.0));
                    let class_lane = match (is_interior && is_wmo, wmo_class) {
                        (true, Some(WmoBatchClass::Int)) => 1.0,
                        (true, Some(WmoBatchClass::Trans)) => 2.0,
                        _ => 0.0,
                    };
                    Vec4::new(t0[0], t0[1], t0[2], class_lane)
                },
                // xyz the SIDN emissive, ramped by the night fraction; w the WINDOW flag.
                sidn: {
                    let c = sidn.unwrap_or([0, 0, 0]);
                    Vec4::new(
                        f32::from(c[0]) / 255.0,
                        f32::from(c[1]) / 255.0,
                        f32::from(c[2]) / 255.0,
                        if window { 1.0 } else { 0.0 },
                    )
                },
                // Zero until a sampler registers this material and bakes its table slot in once.
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

/// `clutter_fade.z` marker bit 10: the twin's source alpha-tests, so the shader keeps the 224/255
/// cutout while blending. Never on an Opaque source: mode 0 has no alpha test (`0x70c237`).
pub(crate) const TWIN_CUTOUT_MARKER: u16 = 1 << 10;

/// `clutter_fade.z` marker bit 12: texture coordinates from the view-space reflection vector, the
/// reference's `texture_unit_lookup > 2` env stage; fragment-only, so not a `WowModelKey` axis.
pub(crate) const ENV_MAP_MARKER: u16 = 1 << 12;

/// `clutter_fade.z` marker bit 13: a WMO skybox batch, whose pipeline pins the far depth at the
/// vertex with no raster bias; a `WowModelKey` axis, so every other draw keeps its real depth.
pub(crate) const SKY_DEPTH_MARKER: u16 = 1 << 13;

/// [`BATCH_ORDER_SORT_EPS`] for the WMO skybox lane: at the −6e4 rung the f32 ulp is 0.0039, so
/// 1e-3 would round away; this step is 4 ulps. Every skybox batch pair ties, being eye-anchored.
pub(crate) const SKYBOX_ORDER_EPS: f32 = 0.015625;

/// The skybox band's ceiling: with [`FAR_KEY_PULL`] every batch lands on one pipeline-key integer.
/// It ties past index 57; the largest skybox, `CavernsOfTimeSky.m2`, has 21 batches.
pub(crate) const SKYBOX_ORDER_CAP: f32 = 0.9;

/// A batch's slot in the skybox band: the rung, pulled onto one pipeline key, plus its order.
fn skybox_sort_bias(batch_order: u16) -> f32 {
    crate::sky_order::WMO_SKYBOX_BIAS - FAR_KEY_PULL
        + (f32::from(batch_order) * SKYBOX_ORDER_EPS).min(SKYBOX_ORDER_CAP)
}

/// The depth-prime twin's sort bias in yards (negative draws earlier), past the spread of one
/// model's part centres so all its twins draw before any colour part, as the reference's one sort
/// key per instance with twins first does (`cmd+0x14 = model+0x84`, `0x707db1`, `0x70ae10`). 8 yd
/// covers humanoids and mounts; a taller model can keep some self-overlap, and a transparent
/// sorting within 8 yd behind a fading body draws after its prime and clips where the body covers
/// it. As a raster depth bias it is ~2⁻²⁰ relative, away from the eye.
pub(crate) const ZFILL_SORT_BIAS: f32 = -8.0;

/// Build or fetch the depth-prime twin for one fadeable entity batch, the reference's
/// `M2UseZFill` clone (`0x707f7d`): while a model is translucent, a colour-masked, z-writing copy
/// of each depth-writing batch draws first, so only its nearest surface blends. `cutout` marks an
/// AlphaKey source, whose twin keeps its alpha test as the reference's does (mode 0 has none,
/// `0x70c237`); it must match what the colour pass discards, or the holes get depth walls.
/// Colour-only inputs are canonical, so twins dedupe.
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
        env_map: false,
        shade: ShadeSel::Lit,
        batch_order: 0,
        uv_anim: None,
        rgb_anim: None,
        wmo_class: None,
        sidn: None,
        window: false,
        zfill: true,
        // No skybox batch has a twin: the sky writes no depth.
        sky_depth: false,
        instance: None,
    };
    if let Some(h) = cache.fetch(&key) {
        return h;
    }
    let cull_mode = if two_sided { None } else { Some(Face::Back) };
    let base = StandardMaterial {
        base_color_texture: texture,
        // Blend for the transparent pass's sort; `specialize` turns blend and colour writes off.
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
                clutter_fade: Vec4::new(
                    0.0,
                    0.0,
                    f32::from(ZFILL_MARKER | if cutout { TWIN_CUTOUT_MARKER } else { 0 }),
                    0.0,
                ),
                // `y` stays 0: the zfill pipeline forces its own depth write.
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

/// `clutter_fade.z` marker bit 11: a far-side-of-water twin, whose −4e4 sort rung `specialize`
/// keeps out of the raster depth bias (a 0.5% pull that clips coplanar sheen layers).
pub(crate) const FAR_SIDE_MARKER: u16 = 1 << 11;

/// A transparent material's far-side twin, one water rung down so the water paints over it; the
/// app's pipeline warm pass builds through this too, so their keys cannot drift.
pub fn far_twin_of(near: &WowModelMaterial) -> WowModelMaterial {
    let mut far = near.clone();
    far.base.depth_bias = far_sort_bias(far.base.depth_bias);
    far.extension.clutter_fade.z = far_markers(far.extension.clutter_fade.z);
    far
}

/// The pull on every far-side sort slot, so every `near` in `[0, 0.99]` truncates to one
/// pipeline-key integer; a uniform shift, so nothing reorders.
const FAR_KEY_PULL: f32 = 0.99;

/// The twin's sort slot: one water rung under the source's, less [`FAR_KEY_PULL`].
fn far_sort_bias(near: f32) -> f32 {
    near + crate::sky_order::FAR_SIDE_BIAS - FAR_KEY_PULL
}

/// The twin's `clutter_fade.z` word: [`FAR_SIDE_MARKER`] added, every other marker kept.
fn far_markers(z: f32) -> f32 {
    f32::from(z as u16 | FAR_SIDE_MARKER)
}

/// The live far twins, by near id and back. The handles are strong: a pair lives until
/// [`classify_water_side`]'s sweep finds no batch carrying its near identity.
#[derive(Resource, Default)]
pub struct FarSideTwins {
    to_far: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    to_near: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
}

impl FarSideTwins {
    /// The live far twin of `near`; a miss means it is not built yet, and the caller keeps `near`.
    pub(crate) fn far_of(
        &self,
        near: &Handle<WowModelMaterial>,
    ) -> Option<&Handle<WowModelMaterial>> {
        self.to_far.get(&near.id())
    }

    /// The near identity of a far twin, for the mat-anim registry, which keys by near id.
    pub(crate) fn near_of(
        &self,
        id: AssetId<WowModelMaterial>,
    ) -> Option<AssetId<WowModelMaterial>> {
        self.to_near.get(&id).map(bevy::asset::Handle::id)
    }
}

/// The handle a material owner writes: its pick, or the pick's far twin on a [`FarSideOfWater`]
/// entity, so every writer agrees on one handle per frame. An opaque pick has no twin and stands.
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

/// This batch draws on the eye's far side of the water plane; for a `DoodadFade` holder the
/// Visibility authority composes it into its own pick.
#[derive(Component)]
pub struct FarSideOfWater;

/// How long the twin GC may wait for a full walk after a part despawns: it bounds an orphaned
/// pair's life while walking's steady despawns cost ~0.5 full walks/s.
const GC_DEADLINE_SECS: f32 = 2.0;

/// Swap each transparent M2 batch onto or off its far-side twin: the reference draws a model's
/// transparents on the eye's far side before the water pass (`0x707680`), split per model, and
/// every batch carries its model's transform. An instance inside the `±r` band draws on both
/// sides, clipped at the waterline (`M2UseClipPlanes`, [`crate::straddle`]); WMO translucents
/// never classify. Only changed batches re-classify; the eye crossing the surface or surfaces
/// streaming walk them all.
#[allow(clippy::type_complexity)]
pub(crate) fn classify_water_side(
    interleave: crate::particles::WaterInterleave,
    mut twins: ResMut<FarSideTwins>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    mut near_edits: MessageReader<AssetEvent<WowModelMaterial>>,
    mut commands: Commands,
    // A ParamSet: the `Changed` query would conflict with the walk's `&mut`.
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
            // A straddle twin is the classification's output, never its input.
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
    // An instance entering or leaving its water band re-classifies its whole subtree.
    rebanded: Query<Entity, Changed<crate::straddle::ModelWaterBand>>,
    clips: Res<crate::straddle::WaterClips>,
    children: Query<&Children>,
    mut removed: RemovedComponents<MeshMaterial3d<WowModelMaterial>>,
    time: Res<bevy::time::Time>,
    mut eye_was_submerged: Local<Option<bool>>,
    mut gc_due: Local<Option<f32>>,
) {
    // Mirror a near-asset edit into its live twin. Animation needs none (the twin's clone shares
    // the table slot), and twin writes touch only far ids, so this cannot feed back.
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
            // Cannot fail: `to_far` holds the twin's strong handle.
            materials
                .insert(far, far_twin_of(&near))
                .expect("far twin slot is strongly held");
        }
    }
    let eye = interleave.eye_submerged();
    let now = time.elapsed_secs();
    // `last()` drains the reader (`next()` would re-arm next frame), and the earliest despawn
    // holds the deadline, so a steady stream cannot push the GC out forever.
    if removed.read().last().is_some() {
        gc_due.get_or_insert(now + GC_DEADLINE_SECS);
    }
    let full = *eye_was_submerged != Some(eye)
        || interleave.surfaces_changed()
        || gc_due.is_some_and(|due| now >= due);
    *eye_was_submerged = Some(eye);
    if full {
        // Any full walk runs the GC's mark, which satisfies a pending deadline.
        *gc_due = None;
    }

    if full {
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
        // Drop pairs whose near identity no live batch carries; a respawn rebuilds its pair lazily.
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
        // Only the changed set; an entity dirty on two legs classifies twice, the second a no-op.
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
        // A changed room claim or straddle band re-classifies the holder's subtree: both live on
        // the unit root, above its batches, and a claim can change with the unit standing still.
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

/// One candidate batch for [`classify_part`]: entity, transform, layers, handle, whether the
/// Visibility authority owns the handle, marked far, tag (the straddle slot), marked straddling.
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

/// Classify one batch. `used` collects the live near identities for the twin GC's mark; the
/// reactive path passes `None`.
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
    // A booth-layered batch has its own camera and scene, with no world water.
    if layers.is_some_and(|l| !l.intersects(&RenderLayers::default())) {
        return;
    }
    let cur = mat.0.id();
    let (near, swapped) = match twins.to_near.get(&cur) {
        Some(n) => (n.clone(), true),
        None => (mat.0.clone(), false),
    };
    // The current verdict: the marker, or the handle for the batches this system swaps itself.
    let far_now = marked || swapped;
    let qualifies = match materials.get(near.id()) {
        Some(m) => {
            matches!(m.base.alpha_mode, AlphaMode::Blend)
                && m.extension.model_flags.x <= 0.5
                // A WMO skybox sits at the eye and the far depth, on no side of the water plane.
                && (m.extension.clutter_fade.z as u16) & SKY_DEPTH_MARKER == 0
        }
        None => false,
    };
    if !qualifies {
        // A part back on its opaque cutout has no side until it blends again.
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
    // A straddler keeps its near identity, clipped to the eye's half, and `sync_straddle_twins`
    // hangs its far twin beside it; the verdict is the word the clip itself reads.
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
        // Built here, where the store is writable; the twin sync only reads the map.
        far_twin(twins, materials, &near);
    }
    let far = !straddles
        && crate::particles::far_side_of_water(interleave, Some(entity), gt.translation());
    if far == far_now {
        return;
    }
    trace_side(if far { "far-side" } else { "near-side" }, entity, gt);
    if far {
        // Built for an authority-owned batch too: its owner's pick resolves through the map.
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

/// The far twin of `near`, built on first ask; `near` must be live, as `qualifies` proved.
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

/// `WOW_MOVE_TRACE_TAGS=fx`: one line per side transition of one batch.
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

/// Replace the fog-policy bits (4-6) of a packed `clutter_fade.z` word, keeping every other
/// marker; the mask lives beside the packer so no caller masks the word by hand.
pub fn replace_fog_policy(z: f32, policy: benilla_formats::FogPolicy) -> f32 {
    f32::from((z as u16 & !(7 << 4)) | ((policy as u16) << 4))
}

/// The model-render lane's registrations.
pub fn plugin(app: &mut App) {
    // Before `ModelVisSet`, so the authority's pick sees this frame's side, and after the fade
    // resolve. The self feather is unordered: it composes the same marker and can lag one frame
    // as the eye crosses the surface.
    app.init_resource::<FarSideTwins>().add_systems(
        Update,
        classify_water_side
            .after(crate::liquid::SubmersionVerdict)
            .after(crate::model_fade::apply_render_fade)
            .before(crate::model_render::ModelVisSet),
    );
    // The model-`Visibility` authority, the one writer, after the portal PVS it reads.
    app.add_systems(
        Update,
        visibility::apply_model_visibility
            .after(crate::wmo_portal::WmoPvsSet)
            .in_set(ModelVisSet),
    );
    // `WOW_VIS_TRACE=<label>` traces one model's placements.
    if let Some(trace) = visibility::VisTrace::from_env() {
        app.insert_resource(trace);
    }
    app.add_systems(
        Update,
        visibility::trace_model_visibility.after(ModelVisSet),
    );
    // The material dedup cache and its evictors: by distance, and whole on a cross-map transition.
    app.init_resource::<ModelMaterials>()
        .add_systems(Update, (scope_model_materials, evict_model_materials));
    // A material becomes an asset the frame something visible binds it: before the store
    // publishes its queued `Added` event and before `VisibilityPropagate` clears what it reads.
    app.add_systems(
        PostUpdate,
        lazy::realize_bound
            .before(bevy::asset::AssetEventSystems)
            .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
    );
    // A long-hidden streamed part puts its `Mesh3d` down, on last frame's propagated verdict.
    if park::enabled() {
        app.add_systems(Update, park::park_hidden_parts.after(ModelVisSet));
    }
}

/// Expire the material dedup by distance.
fn scope_model_materials(mut scope: crate::art_scope::ArtScope, mut mats: ResMut<ModelMaterials>) {
    scope.apply(&mut mats.0, crate::art_scope::ArtSlot::ModelMats);
}

/// Drop the material dedup whole on a cross-map transition; entries rebuild on demand.
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

/// The index of a [`ModelKind`] in the panel's toggles and per-kind counts.
pub fn kind_index(kind: ModelKind) -> usize {
    match kind {
        ModelKind::Doodad => 0,
        ModelKind::Wmo => 1,
        ModelKind::Creature => 2,
        ModelKind::GameObject => 3,
    }
}

/// The index of a [`ModelBlend`] layer, as [`kind_index`] is for kinds.
pub fn blend_index(b: ModelBlend) -> usize {
    match b {
        ModelBlend::Opaque => 0,
        ModelBlend::AlphaTest => 1,
        ModelBlend::Blend => 2,
        ModelBlend::Mod => 3,
        ModelBlend::Mod2x => 4,
    }
}

/// The world-model subsystem a spawned submesh belongs to, which the panel's toggles scope by.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModelKind {
    Doodad,
    Wmo,
    Creature,
    GameObject,
}

/// A spawned model submesh's subsystem and blend layer, which the panel toggles on.
#[derive(Component, Clone, Copy)]
pub struct ModelPart {
    pub kind: ModelKind,
    pub blend: ModelBlend,
}

/// Why the streamer left this submesh on the entity path, one static label per batch for the
/// census (`VIS_CENSUS … why …`); absent on units, GameObjects and everything the app spawns.
#[derive(Component, Clone, Copy)]
pub struct EntityPathWhy(pub &'static str);

/// The model-`Visibility` authority's set; the first-person self hide runs after it and wins.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelVisSet;

#[cfg(test)]
mod tests {
    use super::replace_fog_policy;
    use benilla_formats::FogPolicy;

    #[test]
    fn replace_fog_policy_preserves_pipeline_markers() {
        // Every marker bit (0-3, 7-8) set, fog policy Grey (3) in bits 4-6.
        let z = f32::from(0b1_1000_1111u16 | (3 << 4));
        let out = replace_fog_policy(z, FogPolicy::Off) as u16;
        assert_eq!(out & 0b1_1000_1111, 0b1_1000_1111, "markers preserved");
        assert_eq!((out >> 4) & 7, FogPolicy::Off as u16, "fog swapped");
        // No stray bits invented.
        let out2 = replace_fog_policy(f32::from(0u16), FogPolicy::Grey) as u16;
        assert_eq!(out2, (FogPolicy::Grey as u16) << 4);
    }

    /// Too small and coplanar layers strobe; too large and effects interleave into their owner.
    #[test]
    fn batch_order_sort_eps_sits_between_f32_noise_and_the_effect_rung() {
        // The capped product is the deepest any batch can bias; WMO roots index past 2000.
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
        // The pipeline key truncates the whole band to 0 for any u16 index.
        assert_eq!(deepest as i32, 0);
        const {
            assert!(super::BATCH_ORDER_SORT_CAP < 1.0);
            // A capped batch's far twin still truncates onto the far band's one key integer.
            assert!(super::BATCH_ORDER_SORT_CAP < super::FAR_KEY_PULL);
        }
        assert_eq!(
            super::far_sort_bias(super::BATCH_ORDER_SORT_CAP) as i32,
            super::far_sort_bias(0.0) as i32,
            "the far band stays one pipeline key across the whole capped eps range"
        );
    }

    #[test]
    fn the_skybox_band_orders_its_batches_on_one_pipeline_key() {
        // `CavernsOfTimeSky.m2` is 21 batches; `order` is the index + 1.
        let band: Vec<f32> = (0..=21).map(super::skybox_sort_bias).collect();

        // 1. Strictly increasing: at −6e4 the ordinary 1e-3 eps would round onto one f32.
        for pair in band.windows(2) {
            assert!(
                pair[1] > pair[0],
                "skybox batch order does not resolve at the rung's magnitude: {pair:?} — \
                 SKYBOX_ORDER_EPS is under the f32 ulp there"
            );
        }

        // 2. One pipeline key for the whole band (`depth_bias as i32` is a key axis), for any
        //    u16 order: the cap holds it.
        let key = band[0] as i32;
        for order in [0u16, 1, 21, 57, 58, 1000, u16::MAX] {
            assert_eq!(
                super::skybox_sort_bias(order) as i32,
                key,
                "batch order {order} mints a second skybox pipeline"
            );
        }

        // 3. Under every world transparent, even with the skybox's shell radius (~94 yd for
        //    Caverns of Time) on its rung and a world draw at the far plane.
        const FAR_PLANE: f32 = 3000.0;
        const SHELL: f32 = 128.0;
        let deepest_world = crate::sky_order::FAR_SIDE_BIAS - FAR_PLANE - super::FAR_KEY_PULL;
        assert!(
            band.last().unwrap() + SHELL < deepest_world,
            "a skybox can sort in front of a world transparent — the painted sky would paint \
             over it (sky_order::WMO_SKYBOX_BIAS)"
        );

        // 4. The rung stays within ten times the world band: margin, not drift.
        const {
            assert!(crate::sky_order::WMO_SKYBOX_BIAS > 10.0 * crate::sky_order::FAR_SIDE_BIAS);
        }
    }

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
        // The whole batch-eps band truncates to one pipeline-key integer; the zfill twin keeps
        // its own one bracket down.
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
        // The marker word stays f32-exact with every bit up to `FAR_SIDE_MARKER` set.
        let all = src | super::FAR_SIDE_MARKER | 0x0f;
        assert_eq!(f32::from(all) as u16, all);
    }
}
