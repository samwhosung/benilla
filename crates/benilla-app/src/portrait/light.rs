//! The round portraits' light rig: a fixed light buffer, its material-twin cache, and the values
//! behind it. The character panes light through the UI model tiles' pool instead
//! (`crate::ui_models::PaneLights`). A booth draws with the world's own materials, cloned with only
//! the light storage swapped ([`material_variant`]), so it never inherits the time of day.

use benilla_world::lighting::LightBlob;
use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;

/// One booth light: its own copy of the shared-light storage buffer, written once at startup so a
/// booth reads the same at noon, midnight or in fog. `variants` caches each world material's twin,
/// an exact clone with only `light_buf` swapped.
#[derive(Default)]
pub(super) struct BoothRig {
    pub(super) buffer: Option<bevy::render::render_resource::Buffer>,
    variants: HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    /// Set by [`Self::variant`] when the source material was not resident yet.
    unready: bool,
}

impl BoothRig {
    /// The booth twin of a world-built material, cached per source. When the source is not
    /// resident this returns the world material itself, on the world light buffer: a caller about
    /// to commit a bake must check [`Self::take_unready`] first, since a sleeping booth would keep
    /// that wrongly lit pane until something forces a re-bake.
    pub(super) fn variant(
        &mut self,
        world: &Handle<WowModelMaterial>,
        materials: &mut Assets<WowModelMaterial>,
    ) -> Handle<WowModelMaterial> {
        let Some(buffer) = self.buffer.clone() else {
            // No booth buffer (headless tests): nothing to wait for, so a plain fallback.
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

    /// Take the "a twin could not be built" flag since the last call. When set, the bake site
    /// abandons this frame's bake without touching `Booth::baked`, so it retries once the
    /// material lands.
    pub(super) fn take_unready(&mut self) -> bool {
        std::mem::take(&mut self.unready)
    }
}

/// The booths' light, with its own variant cache since a twin points at one buffer.
/// [`Self::studio`] lights the round unit-frame portraits. Deviation: a fixed neutral front-lit
/// studio ([`studio_light`]) replaces the portrait bake's hard-coded rig (`0x525b10`: ambient 0.45
/// grey, white directional), because the studio is the look chosen for them.
#[derive(Resource, Default)]
pub(super) struct BoothLight {
    pub(super) studio: BoothRig,
}

/// Reap the booth twins whose world source died (`AssetEvent::Removed`, e.g. a map teardown); a
/// twin is pinned only by this cache, so it would otherwise outlive the teardown.
pub(super) fn reap_dead_variants(
    mut events: MessageReader<AssetEvent<WowModelMaterial>>,
    mut booths: ResMut<BoothLight>,
) {
    for ev in events.read() {
        if let AssetEvent::Removed { id } = ev {
            booths.studio.variants.remove(id);
        }
    }
}

/// Which shade lane and fog policy a [`material_variant`] twin draws with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VariantLane {
    /// The world's shade lane and fog policy, under a frozen light: the portrait booths.
    World,
    /// The M2 light rig ([`benilla_world::model_render::ShadeSel::Rig`]) with fog forced off: the
    /// glue character model takes no fog (its fill callback `0x470ce0` stages none), nor does a
    /// `<Model>` pane that never armed any.
    RigUnfogged,
    /// The rig lane with the authored fog policy: a `<Model>` pane whose Lua armed fog, and the
    /// glue background scene. Unfogged materials (bit `0x02`) still draw unfogged (`0x70bb24`).
    RigFogged,
}

/// The twin of a world-built material against `buffer`, cached in `variants`: only the light
/// storage swapped, plus what `lane` asks for. `None` means the source is not resident yet; the
/// world material is no substitute (wrong light buffer), so callers retry.
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
    // A blend/zfill twin can be cloned before the world binds it: realize the parked value first.
    benilla_world::model_render::lazy::realize(materials, world.id());
    let mat = materials.get(world)?;
    let mut twin = mat.clone();
    twin.extension.light_buf = buffer.clone();
    if lane != VariantLane::World {
        twin.extension.sun_scale.x = benilla_world::model_render::ShadeSel::Rig.selector();
    }
    if lane == VariantLane::RigUnfogged {
        // Fog off, keeping every pipeline marker `specialize` keys on (bits 0-3 and the multiply
        // markers, bits 7-8); the mask belongs to `model_render`, next to the packer.
        twin.extension.clutter_fade.z = benilla_world::model_render::replace_fog_policy(
            twin.extension.clutter_fade.z,
            benilla_formats::FogPolicy::Off,
        );
    }
    let handle = materials.add(twin);
    variants.insert(world.id(), handle.clone());
    Some(handle)
}

/// The round portraits' studio light: neutral warm-white ambient and diffuse, the sun from the
/// camera's three-quarter side so the face shown is lit, fog and point lights off. The layout comes
/// from the scene's own packer ([`LightBlob`]); only the values live here.
pub(super) fn studio_light() -> LightBlob {
    // −sun_dir is the to-light vector: toward the camera side (−Z, a bit of −X from the yaw, up).
    let sun_dir = Vec3::new(0.25, -0.45, 0.85).normalize();
    // Fog and the SIDN night lane stay at the builder's off-world defaults; no shader reads 19.w.
    LightBlob::model(
        [0.58, 0.56, 0.54], // studio ambient, neutral warm-white
        [0.85, 0.82, 0.78], // studio diffuse
        sun_dir,
    )
    .dial(0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The studio light keys from the camera's side.
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
