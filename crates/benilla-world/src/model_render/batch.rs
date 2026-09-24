//! [`M2BatchMaterials`]: the materials an authored render batch ([`ModelSubmesh`]) draws with,
//! read off the batch; a spawner picks only the light lane and the variants. A multiply batch
//! cannot feather through alpha, an authored-Blend batch is its own twin, and a batch that writes
//! no depth primes nothing.

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::ModelSubmesh;
use benilla_formats::ModelBlend;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

use super::{model_material, zfill_material, MaterialCache, ShadeSel};
use crate::lighting::SharedLightBuffer;

/// The engine's one `WowModelMaterial` dedup cache, swept by distance and cleared on a map change;
/// an evicted entry costs a rebuild, never a wrong material, as [`super::MatKey`] is complete.
#[derive(Resource, Default)]
pub struct ModelMaterials(pub(crate) MaterialCache);

/// The deduped handles one authored batch renders as; a lane spawns the ones it needs.
pub struct BatchVariants {
    /// The steady exterior material, sky-lit.
    pub steady: Handle<WowModelMaterial>,
    /// Interior matte: day/night at sun ×1.0, the reference's null-node fallback.
    pub interior: Handle<WowModelMaterial>,
    /// Interior bake: the model's SH probe by its `MeshTag` slot, every entity M2's indoor look.
    pub interior_bake: Handle<WowModelMaterial>,
    /// The bake's blend twin, so a fade indoors stays probe-lit.
    pub interior_bake_blend: Handle<WowModelMaterial>,
    /// The `AlphaMode::Blend` twin every feather rides; [`Self::steady`] if that already blends.
    pub fade_blend: Handle<WowModelMaterial>,
    /// The depth-prime twin (`0x707f7d`); `None` for a multiply batch or depth write/test off.
    pub zfill: Option<Handle<WowModelMaterial>>,
}

/// Where [`M2BatchMaterials::entity_variants`] registers a batch's texture transform: the
/// animated-material registry, the delta table and, when it has one, the owning instance.
pub struct EntityUvLane<'a> {
    pub reg: &'a mut crate::doodad_anim::UvAnimMaterials,
    pub table: &'a mut crate::mat_anim_table::MatAnimTable,
    /// `Some` when the materials belong to one instance (a GameObject whose sequence slots bake
    /// different loops); `None` is the shared material every instance of the batch draws through.
    pub instance: Option<Entity>,
}

/// One skybox batch's pair ([`M2BatchMaterials::skybox`]): the authored blend at full weight, and
/// the SRC_ALPHA twin the 4-second crossfade rides.
pub struct SkyboxBatch {
    /// Slot weight 1.0: the authored blend mode.
    pub steady: Handle<WowModelMaterial>,
    /// The blend-promotion twin for `0 < weight < 1` (`0x70c190`).
    pub fade_blend: Handle<WowModelMaterial>,
}

/// Builds the materials an authored render batch draws with. Every method that needs the shared
/// light buffer returns `None` until it exists; a spawner that runs earlier retries.
#[derive(SystemParam)]
pub struct M2BatchMaterials<'w> {
    cache: ResMut<'w, ModelMaterials>,
    materials: ResMut<'w, Assets<WowModelMaterial>>,
    light: Option<Res<'w, SharedLightBuffer>>,
}

impl M2BatchMaterials<'_> {
    /// Whether the shared light buffer is resident.
    pub fn ready(&self) -> bool {
        self.light.is_some()
    }

    /// The material store this param holds: a second `ResMut<Assets<WowModelMaterial>>` beside it
    /// would be a Bevy system-param conflict.
    pub fn materials(&mut self) -> &mut Assets<WowModelMaterial> {
        &mut self.materials
    }

    /// One steady, sky-lit material for a batch drawn in the world. `order` is the authored batch
    /// index + 1 (`0` = unordered), the sort bias that keeps a model's coplanar layers in file
    /// order ([`super::BATCH_ORDER_SORT_EPS`]).
    pub fn steady(
        &mut self,
        sub: &ModelSubmesh,
        texture: Option<Handle<Image>>,
        order: u16,
    ) -> Option<Handle<WowModelMaterial>> {
        let light = self.light.as_ref()?.0.clone();
        Some(self.build(
            sub,
            texture,
            order,
            shade_for(sub),
            false,
            false,
            false,
            &light,
        ))
    }

    /// The pair for a batch of a WMO skybox ([`crate::skybox`]), an ordinary M2 but for its depth:
    /// it draws at the reverse-Z far value ([`crate::sky_order`]), never writes depth, and always
    /// tests it, overriding M2 flag `0x08`, which the reference uses only to order the sky inside
    /// its own depth slice `[0.975, 0.98]` (ported as [`super::skybox_sort_bias`]). The slot weight
    /// (`[CM2Model+0x180]`, from the interior crossfade `[0xce9bdc]`) scales every batch's alpha,
    /// and `0 < A < 1` promotes a batch to SRC_ALPHA blending (`0x70c190`): `fade_blend` is that
    /// twin, or `steady` when the batch already blends.
    pub fn skybox(
        &mut self,
        sub: &benilla_formats::RenderSubmesh,
        texture: Option<Handle<Image>>,
        order: u16,
    ) -> Option<SkyboxBatch> {
        let light = self.light.as_ref()?.0.clone();
        let mut mk = |fade: bool| {
            model_material(
                &mut self.cache.0,
                &mut self.materials,
                texture.clone(),
                sub.blend,
                sub.two_sided,
                false, // an M2, whichever building names it
                false,
                sub.emissive,
                sub.additive,
                fade,
                true,  // never writes depth
                false, // …and never skips the test
                sub.fog_policy,
                sub.env_map,
                ShadeSel::Lit, // unread: every shipped skybox batch is UNLIT (0x01)
                order,
                // No texture-transform or colour loop: neither shipped skybox authors one. Wiring
                // one needs the `M2Model` lane's `Arc`, which the material key identifies it by.
                None,
                None,
                None, // M2: no MOBA class, no SIDN, no WINDOW
                None,
                false,
                true, // the sky lane
                &light,
                None, // one shared sky material, no per-sequence channel to key on
            )
        };
        let steady = mk(false);
        let fade_blend = if matches!(
            sub.blend,
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
        ) {
            steady.clone()
        } else {
            mk(true)
        };
        Some(SkyboxBatch { steady, fade_blend })
    }

    /// One steady material lit by a render target's own light buffer (a portrait booth, the model
    /// pane, char-select), which [`super::MatKey::light`] keys apart from the world. `rig` lights
    /// by the scene's authored M2 rig ([`ShadeSel::Rig`]) and plays the UV animation. `order` must
    /// be the batch index + 1: a glue scene's batches share one root, so equal biases flicker.
    pub fn off_world(
        &mut self,
        sub: &ModelSubmesh,
        texture: Option<Handle<Image>>,
        order: u16,
        light: &Buffer,
        rig: bool,
    ) -> Handle<WowModelMaterial> {
        let shade = if rig { ShadeSel::Rig } else { ShadeSel::Lit };
        self.build(sub, texture, order, shade, false, false, rig, light)
    }

    /// The variant set of any entity part, all lit with one indoor pair: the reference gives every
    /// entity M2 the same entity-node fill (`0x672a20`). Registers each variant's UV loop on `uv`
    /// in the same step; the depth-prime twin, drawn only mid-feather, is not registered.
    pub fn entity_variants(
        &mut self,
        sub: &ModelSubmesh,
        texture: Option<Handle<Image>>,
        order: u16,
        uv: &mut EntityUvLane<'_>,
    ) -> Option<BatchVariants> {
        let light = self.light.as_ref()?.0.clone();
        let steady = self.build(
            sub,
            texture.clone(),
            order,
            ShadeSel::Lit,
            false,
            false,
            true,
            &light,
        );
        let interior = self.build(
            sub,
            texture.clone(),
            order,
            ShadeSel::Matte,
            false,
            false,
            true,
            &light,
        );
        let interior_bake = self.build(
            sub,
            texture.clone(),
            order,
            ShadeSel::Matte,
            true,
            false,
            true,
            &light,
        );
        // A multiply batch fades by the shader's identity lerp (the reference's preset-5 shape)
        // and a Blend batch already blends: both are their own twin. The rest twin from the
        // source blend, so the 224/255 cutout matches the colour pass.
        let own_twin = matches!(
            sub.blend,
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
        );
        let fade_blend = if own_twin {
            steady.clone()
        } else {
            self.build(
                sub,
                texture.clone(),
                order,
                ShadeSel::Lit,
                false,
                true,
                true,
                &light,
            )
        };
        let interior_bake_blend = if own_twin {
            interior_bake.clone()
        } else {
            self.build(
                sub,
                texture.clone(),
                order,
                ShadeSel::Matte,
                true,
                true,
                true,
                &light,
            )
        };
        // `register_entity_uv` is idempotent per material, so collapsed variants cost nothing.
        let loops = crate::doodad_anim::UvLoops::of(sub);
        if loops.animates() {
            for id in [
                &steady,
                &interior,
                &interior_bake,
                &interior_bake_blend,
                &fade_blend,
            ] {
                crate::doodad_anim::register_entity_uv(
                    uv.reg,
                    uv.table,
                    &mut self.materials,
                    id.id(),
                    &loops,
                    uv.instance,
                );
            }
        }
        Some(BatchVariants {
            steady,
            interior,
            interior_bake,
            interior_bake_blend,
            fade_blend,
            zfill: self.zfill_for(sub, texture, &light),
        })
    }

    /// The variant set for a character composite (a runtime body/hair atlas) with the caller's
    /// blend and sidedness; never a multiply batch, so every twin is real.
    pub fn char_variants(
        &mut self,
        texture: Handle<Image>,
        blend: ModelBlend,
        two_sided: bool,
    ) -> Option<BatchVariants> {
        let light = self.light.as_ref()?.0.clone();
        let mut mk = |shade: ShadeSel, probe: bool, fade: bool| {
            model_material(
                &mut self.cache.0,
                &mut self.materials,
                Some(texture.clone()),
                blend,
                two_sided,
                false, // a composite sheet is never a WMO batch
                probe, // interior-PROP mode: the shader reads the MeshTag as an SH-probe slot
                false, // …never UNLIT
                false, // …nor additive
                fade,
                false, // …nor depth-write/test disabled: body and hair are ordinary geometry
                false,
                // Body and hair are opaque or alpha-cut, which the byte table fogs to the scene.
                benilla_formats::FogPolicy::Scene,
                // No env map: ArmorReflect reaches a character through its held items' batches.
                false,
                shade,
                0,     // a composite is one sheet, not a batch in an authored order
                None,  // …with no texture transform
                None,  // …and no animated M2Color tint
                None,  // worn/held part: light selection anchors at the instance origin
                None,  // M2 carries no MOMT SIDN colour
                false, // …nor the WINDOW flag
                false, // a character composite is never a skybox
                &light,
                None, // a composite sheet carries no animated UV/tint channel at all
            )
        };
        Some(BatchVariants {
            steady: mk(ShadeSel::Lit, false, false),
            interior: mk(ShadeSel::Matte, false, false),
            interior_bake: mk(ShadeSel::Matte, true, false),
            interior_bake_blend: mk(ShadeSel::Matte, true, true),
            fade_blend: mk(ShadeSel::Lit, false, true),
            // Character parts always write depth, so every one twins; as in the colour twin, only
            // an alpha-key source alpha-tests (224/255) while fading.
            zfill: Some(zfill_material(
                &mut self.cache.0,
                &mut self.materials,
                Some(texture),
                two_sided,
                blend == ModelBlend::AlphaTest,
                &light,
            )),
        })
    }

    /// The cache, store and light buffer for the placed-instance spawner, whose indoor look is set
    /// once per placement.
    pub fn pieces(
        &mut self,
    ) -> Option<(&mut MaterialCache, &mut Assets<WowModelMaterial>, Buffer)> {
        let light = self.light.as_ref()?.0.clone();
        Some((&mut self.cache.0, &mut self.materials, light))
    }

    /// The depth-prime twin; a multiply batch never feathers, so it primes nothing.
    fn zfill_for(
        &mut self,
        sub: &ModelSubmesh,
        texture: Option<Handle<Image>>,
        light: &Buffer,
    ) -> Option<Handle<WowModelMaterial>> {
        if sub.no_depth_write || sub.no_depth_test {
            return None;
        }
        match sub.blend {
            ModelBlend::Mod | ModelBlend::Mod2x => None,
            b => Some(zfill_material(
                &mut self.cache.0,
                &mut self.materials,
                texture,
                sub.two_sided,
                b == ModelBlend::AlphaTest,
                light,
            )),
        }
    }

    /// The builder call: all but the shade, probe, fade and UV-play choices come off the batch.
    fn build(
        &mut self,
        sub: &ModelSubmesh,
        texture: Option<Handle<Image>>,
        order: u16,
        shade: ShadeSel,
        probe: bool,
        fade: bool,
        play_uv: bool,
        light: &Buffer,
    ) -> Handle<WowModelMaterial> {
        model_material(
            &mut self.cache.0,
            &mut self.materials,
            texture,
            sub.blend,
            // A billboard card culls by the material's `0x04` flag, like any other batch.
            sub.two_sided,
            sub.wmo_batch.is_some(),
            // SH-probe mode when a variant asks; otherwise the authored interior bit (WMO only).
            probe || sub.interior,
            sub.emissive,
            sub.additive,
            fade,
            sub.no_depth_write,
            sub.no_depth_test,
            sub.fog_policy,
            sub.env_map,
            shade,
            order,
            play_uv.then_some(sub.uv_anim.as_ref()).flatten(),
            sub.rgb_anim.as_ref(),
            sub.wmo_batch,
            sub.sidn,
            sub.window,
            // The sky lane overrides the batch's depth state, so only `Self::skybox` builds it.
            false,
            light,
            // Entity batches share one material per batch: an entity resolves its sequence
            // through `MatAnim::host`, so only the world streamer keys one per placement.
            None,
        )
    }
}

/// A WMO group batch shades through the FFP N·L path, so its `sun_scale` is unread. Every other
/// batch is lit on the entity 2.5/0.5 lane (`0x69e280`); the dynamic half, the MCSH sample at the
/// entity's feet, rides the per-instance `MeshTag` shade byte.
fn shade_for(sub: &ModelSubmesh) -> ShadeSel {
    if sub.wmo_batch.is_some() {
        ShadeSel::Matte
    } else {
        ShadeSel::Lit
    }
}
