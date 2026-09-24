//! The booths' **light rigs** — the two fixed light buffers a booth bake can be lit by, the
//! per-light material-twin cache, and the reference values behind each.
//!
//! A booth model is drawn with the world's own materials, cloned with only their light storage
//! swapped ([`material_variant`]) — so a bake never drifts from what is standing in the world, and
//! never inherits the world's time of day. Which light it swaps to is a *fidelity* question with two
//! different answers ([`BoothLight`]): the round unit-frame portraits keep our neutral
//! [`studio_light`], while the body panes — transcriptions of 1.12 `<PlayerModel>` widgets —
//! carry the reference widget's own light, [`model_pane_light`] (decision 0638).

use benilla_world::lighting::LightBlob;
use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;

/// One booth light: its own copy of the shared-light storage buffer (the canonical
/// [`benilla_world::lighting::LIGHT_HEADER_ROWS`]-row std430 layout, lit lanes packed by the scene's own
/// packer), written ONCE at startup — so a booth reads the same at noon, midnight, or in a fog bank
/// (the world buffer would render a night portrait pitch black). `variants` caches, per world
/// material, its booth twin: an exact clone with only `light_buf` swapped — zero drift from the
/// world-built material (same texture/blend/flags), different light.
#[derive(Default)]
pub(super) struct BoothRig {
    pub(super) buffer: Option<bevy::render::render_resource::Buffer>,
    variants: HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    /// Set by [`Self::variant`] when it could NOT build a real booth twin because the source
    /// material was not resident yet — see [`Self::take_unready`].
    unready: bool,
}

impl BoothRig {
    /// The booth twin of a world-built material: same everything, this rig's light buffer. Cached
    /// per source material so twins dedup exactly like their sources.
    ///
    /// **A caller that is about to COMMIT a bake must consult [`Self::take_unready`] first.** When
    /// the source material is not resident this returns the world material itself, which is bound
    /// to the WORLD light buffer — and per this module's own header that is what "would render a
    /// night portrait pitch black". Latching one into a booth is never right: the bake is stored
    /// in a `MeshMaterial3d` and the booth then sleeps (0540), so a single unlucky frame freezes a
    /// world-lit — or unlit — pane until something else forces a re-bake.
    pub(super) fn variant(
        &mut self,
        world: &Handle<WowModelMaterial>,
        materials: &mut Assets<WowModelMaterial>,
    ) -> Handle<WowModelMaterial> {
        let Some(buffer) = self.buffer.clone() else {
            // No booth buffer at all (headless tests) — there is nothing to wait FOR, so this
            // stays a plain fallback rather than an unready signal.
            if super::booth_log() {
                eprintln!("[booth] variant NO-BUFFER -> world lane (unlit in a booth)");
            }
            return world.clone();
        };
        match material_variant(
            &mut self.variants,
            &buffer,
            world,
            materials,
            VariantLane::World,
        ) {
            Some(twin) => twin,
            None => {
                self.unready = true;
                world.clone()
            }
        }
    }

    /// Take the "a twin could not be built" flag accumulated since the last call. A bake site
    /// checks this after collecting its parts and, if set, **abandons the bake for this frame**
    /// without touching `Booth::baked` — the parts-key compare then re-fires next frame and the
    /// bake retries once the material lands. This is the same law `Booth::pending` already applies
    /// to not-yet-resident textures, moved one level up to the material itself.
    pub(super) fn take_unready(&mut self) -> bool {
        std::mem::take(&mut self.unready)
    }
}

/// The booths' two lights — they are **not** the same law, because their references aren't:
///
/// - [`Self::studio`] lights the round unit-frame **portraits**. The reference bakes those through
///   its own portrait render (`SetPortraitTexture`), not through a UI model widget, so this stays
///   our fixed neutral front-lit studio ([`studio_light`]) — the director-approved look.
/// - [`Self::pane`] lights the **body panes** — the character window's paper doll and the inspect
///   window's twin. Those transcribe a `<PlayerModel>` widget, and the reference gives every one of
///   those exactly one light, from its own constructor ([`model_pane_light`]).
///
/// Two buffers means two variant caches: a material twin points at exactly ONE light buffer, so a
/// twin built for the portraits can never be handed to a body pane.
#[derive(Resource, Default)]
pub(super) struct BoothLight {
    pub(super) studio: BoothRig,
    pub(super) pane: BoothRig,
}

/// Reap the booth twins whose world source material died (`AssetEvent::Removed` — e.g. the
/// map-scope teardown, `world_map::MapChange`). A twin is its own asset pinned only by this
/// cache, so without the reap every world material ever baked through a booth would survive
/// the teardown forever (the teleport leak, multiplied per rig). A live bake keeps its
/// twin through its own `MeshMaterial3d` clone; only the dedup entry drops.
pub(super) fn reap_dead_variants(
    mut events: MessageReader<AssetEvent<WowModelMaterial>>,
    mut booths: ResMut<BoothLight>,
) {
    for ev in events.read() {
        if let AssetEvent::Removed { id } = ev {
            booths.studio.variants.remove(id);
            booths.pane.variants.remove(id);
        }
    }
}

/// Which lane a [`material_variant`] twin draws on — the two axes the twin can differ from its
/// world source on, as one closed choice rather than a bool nobody can read at the call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VariantLane {
    /// The world's own shade lane, the batch's authored fog policy kept — the portrait booths,
    /// which render a model exactly as the world would but under a frozen light.
    World,
    /// The scene's authored M2 light rig ([`benilla_world::model_render::ShadeSel::Rig`] — the
    /// probe-slot SH eval + the buffer's point table, decisions 0429/0435) **with fog forced
    /// OFF**: the glue CHARACTER model takes no fog in the reference (its fill callback
    /// `0x470ce0` stages none, its collector's fog stays zeroed), and so does a `<Model>` pane
    /// that never armed any.
    RigUnfogged,
    /// The rig lane with the batch's **authored** fog policy kept — a `<Model>` pane whose Lua
    /// armed fog (decision 2027). The per-material UNFOGGED bit (`0x02`) then does its own work,
    /// which is the reference's own per-batch fork: a fogged pane still draws its UNFOGGED
    /// materials unfogged (`0x70bb24`). The glue background scene takes this lane too.
    RigFogged,
}

/// The twin of a world-built material against `buffer`, cached in `variants` — same
/// texture/blend/flags, only the light storage swapped, plus whatever [`VariantLane`] asks for.
/// [`BoothRig::variant`] is this on [`VariantLane::World`] against one of the two fixed booth
/// buffers; the create scene and the UI model tiles pass their own buffers on a rig lane.
///
/// **`None` means the source material is not resident yet** — there is no twin to hand back, and
/// the world material is NOT an acceptable substitute in a booth (wrong light buffer; see
/// [`BoothRig::variant`]). Callers wait and retry rather than baking something wrong.
pub(crate) fn material_variant(
    variants: &mut HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    buffer: &bevy::render::render_resource::Buffer,
    world: &Handle<WowModelMaterial>,
    materials: &mut Assets<WowModelMaterial>,
    lane: VariantLane,
) -> Option<Handle<WowModelMaterial>> {
    if let Some(twin) = variants.get(&world.id()) {
        return Some(twin.clone());
    }
    // A blend/zfill twin is cloned before the world ever binds it — realize the parked value
    // first (`model_render::lazy`); a no-op for a material already in the store.
    benilla_world::model_render::lazy::realize(materials, world.id());
    let mat = materials.get(world)?;
    let mut twin = mat.clone();
    twin.extension.light_buf = buffer.clone();
    if lane != VariantLane::World {
        twin.extension.sun_scale.x = benilla_world::model_render::ShadeSel::Rig.selector();
    }
    if lane == VariantLane::RigUnfogged {
        // Force fog OFF while preserving every pipeline marker `specialize` keys on (bits 0-3
        // AND the 0528 multiply markers, bits 7-8) — the mask is owned by `model_render`, next
        // to the packer. A hand-rolled `as u8 & 0x0f` here once dropped the multiply markers
        // and alpha-blended the char-select weapon sheen into a white blade.
        twin.extension.clutter_fade.z = benilla_world::model_render::replace_fog_policy(
            twin.extension.clutter_fade.z,
            benilla_formats::FogPolicy::Off,
        );
    }
    let handle = materials.add(twin);
    variants.insert(world.id(), handle.clone());
    Some(handle)
}

/// The **model pane's** light rows — the reference's own, for the `<PlayerModel>` panes (the
/// character window's paper doll, the inspect window's twin). Byte-VERIFIED from the 1.12 client
/// this session (decision 0638), because nothing in FrameXML sets it and the widget default decides:
///
/// - `"PlayerModel"` registers factory `0x495bd0` (widget-type table, `0x49597a`), which allocates
///   `CharacterModelBase` (`0x3f8` B, the source-path string at `0x84351c`) and runs the ctor
///   `0x505680`. `DressUpModel`/`TabardModel` are subclasses of the same base — same light.
/// - That ctor configures the widget's embedded `CGLight` at `+0x324`: **directional**
///   (`0x71b620(light, 0)` @`0x5056c9`), **direction `(0, 1, 0)` — the direction the light
///   PROPAGATES** (`0x71b6a0` @`0x505761` writes `CGLight+0x24`, which `0x71bce0` negates — three
///   `fchs` at `71be7c`/`71be81`/`71be86` — before building the SH basis; 0638 read the field as
///   a to-light vector, 2034 corrects it — it overwrites an earlier `(0, −0.7071, −0.7071)` set at
///   `0x5056e9`, dead code), **diffuse
///   `(0.8, 0.8, 0.64)`** (`+0x3c`, @`0x505702`), **ambient `(0.7, 0.7, 0.7)`** (`+0x30`,
///   @`0x50572e`), then **enables** it (`0x71b780(light, 1)` @`0x50576a`).
/// - It is the widget's ONLY light: the per-frame fill callback `0x76d680` (registered on the
///   model at `[model+0x3bc]` by `0x76cd30`) stages exactly this one, gated on its enable flag; the
///   widget's private scene container holds no others (a paper doll has no background scene M2
///   authoring lights, unlike the glue screens — create's `SetCharCustomizeBackground 0x4707b0`
///   loads one), and `CSimpleModel::SetLight` (`0x76cf30`) has exactly one caller in the whole
///   binary — the Lua `Model:SetLight` binding, which no 1.12 FrameXML file ever calls.
/// - No `×2.5` exterior-intensity node on this path (widgets carry no lighting node, and the ×2.5
///   node-apply `0x6a7300` is world-only), so row 19's dial stays at the studio's `0.4` =
///   intensity 1.0.
///
/// **Why this reads so much darker than the studio light, and why that is the point.** The
/// reference's light is a pure SIDE light: it travels toward WoW `(0, 1, 0)`, the model's own left
/// — Bevy `(−1, 0, 0)` under [`wow_to_bevy`] — so it falls on the figure from its right, square
/// across the camera axis, since the body rig puts the eye on `−Z` looking at the model's front
/// ([`framing::body_frame`]). Everything the pane actually shows the viewer is therefore lit by
/// **ambient alone** (`0.7`), with the diffuse only grazing the figure's screen-left side (its own
/// right; 0638 had it grazing the screen-right side, the same grazing light from the opposite
/// side, which is why the A/B that approved 0638's look could not tell the two apart — 2034). The
/// studio light aims its diffuse from the camera's own
/// side, so the same surfaces got `0.58 + 0.85·(N·L≈1) ≈ 1.4` — roughly twice the reference, which
/// is exactly the "too brightly lit" the director reported.
pub(super) fn model_pane_light() -> LightBlob {
    // The builder takes the light's PROPAGATION direction (it negates it back into a to-light
    // vector for the SH fold) — and the ctor's `(0,1,0)` IS the propagation direction
    // (`CGLight+0x24`, negated by `0x71bce0` before the SH basis), so it goes in as written.
    let sun_dir = benilla_assets::coords::wow_to_bevy([0.0, 1.0, 0.0]);
    LightBlob::model(
        [0.7, 0.7, 0.7],  // CharacterModelBase ctor, CGLight+0x30
        [0.8, 0.8, 0.64], // CharacterModelBase ctor, CGLight+0x3c
        sun_dir,
    )
    .dial(0.4) // STALE (see `studio_light` — no shader reads 19.w)
}

/// The fixed studio-light rows (the shared-light std430 layout): neutral warm-white ambient +
/// diffuse, the sun travelling from the camera's three-quarter side INTO the scene (so the face the
/// portrait shows is the lit one), fog OFF (row 4 w=0), point lights off (row 12.w).
///
/// This lights the round unit-frame **portraits** only. The body panes have a reference law of
/// their own — [`model_pane_light`].
///
/// The lit-lane rows (0-2, the SH block, the sun DC) come from the SAME packer the scene light
/// uses ([`LightBlob`]) — this function used to hand-copy the layout
/// and rendered black portraits the day 0354 moved the lit lanes onto rows it never wrote. Only
/// the studio *values* live here; the layout lives in one place.
pub(super) fn studio_light() -> LightBlob {
    // −sun_dir is the to-light vector: toward the camera side (−Z, a bit of −X from the yaw, up).
    let sun_dir = Vec3::new(0.25, -0.45, 0.85).normalize();
    // Fog stays off and the SIDN night lane stays 0 — the builder's off-world defaults.
    // 19.w — STALE: this was the retired exterior-intensity A/B dial ("0.4 brings the untagged
    // booth parts from the lit 2.5 rung to intensity 1.0"), but NO shader reads 19.w today. Under
    // the 0753 trace law the exterior lane commits the sun at ×1 regardless, which is the very
    // level this dial once aimed for — the drift resolved itself; stated until a portrait-light
    // pass confirms the booth look.
    LightBlob::model(
        [0.58, 0.56, 0.54], // studio ambient — neutral warm-white
        [0.85, 0.82, 0.78], // studio diffuse
        sun_dir,
    )
    .dial(0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference `<PlayerModel>` light, pinned at the values the 1.12 `CharacterModelBase`
    /// constructor writes (decision 0638) — and, more importantly, pinned at the *consequence*:
    /// its light is square across the body rig's view axis, so what the pane shows the viewer is
    /// ambient-lit only. A swapped axis here is the bug 0638 replaced (a frontal studio key, ~2×
    /// the reference on every surface the director can see); a flipped sign is 0638's own reading
    /// of `CGLight+0x24` as a to-light vector, which 2034 corrected to the propagation direction.
    #[test]
    fn model_pane_light_is_the_reference_widget_light_across_the_view_axis() {
        let blob = model_pane_light();
        let rows = blob.header_rows();
        assert_eq!(rows[0][..3], [0.7, 0.7, 0.7], "ambient (CGLight+0x30)");
        assert_eq!(rows[1][..3], [0.8, 0.8, 0.64], "diffuse (CGLight+0x3c)");

        // Row 2 is the light's PROPAGATION direction, and the ctor's (0,1,0) IS that direction
        // (`0x71bce0` negates `CGLight+0x24` before the SH basis — 2034); the to-light is its
        // negation.
        let sun = Vec3::from_slice(&rows[2][..3]);
        assert!(
            sun.abs_diff_eq(benilla_assets::coords::wow_to_bevy([0.0, 1.0, 0.0]), 1e-6),
            "propagation = wow_to_bevy(0,1,0): {sun:?}"
        );
        let to_light = -sun;
        assert!(
            to_light.abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-6),
            "to-light = -wow_to_bevy(0,1,0): {to_light:?}"
        );
        // The body rig puts the eye on −Z looking at +Z (`framing::body_frame`), so a surface facing
        // the camera has normal −Z: N·L = 0, i.e. NO diffuse on anything the pane shows front-on.
        let facing_camera = Vec3::NEG_Z;
        assert!(
            facing_camera.dot(to_light).abs() < 1e-6,
            "the reference light grazes the pane's visible face, it does not key it"
        );
        // …while the figure's screen-LEFT side (the camera's right is −X in world, so +X is its
        // left) is the one that catches it — the model's own right, the side the light travels
        // away from.
        assert!(Vec3::X.dot(to_light) > 0.99, "lit from the viewer's left");
    }

    /// The studio light — the round portraits' — is the OPPOSITE arrangement on purpose: its key
    /// comes from the camera's own side, which is why the two rigs must not share a buffer.
    #[test]
    fn studio_light_keys_from_the_camera_side() {
        let blob = studio_light();
        let rows = blob.header_rows();
        let to_light = -Vec3::from_slice(&rows[2][..3]);
        assert!(
            to_light.z < -0.5,
            "the studio key points back toward the camera (−Z): {to_light:?}"
        );
    }
}
