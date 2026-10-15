//! The light pool's face for the character panes' booths ([`crate::portrait`]): a character pane
//! lights its copy with its own `CGLight` and fog through the same pool and the same scene fold
//! ([`TileScene::blob`]) a file pane's tile does.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use bevy::render::renderer::{RenderDevice, RenderQueue};

use benilla_assets::materials::WowModelMaterial;
use benilla_ui::widget::{ModelFog, ModelLight};
use benilla_world::mat_anim_table::MatAnimMirrors;
use benilla_world::rig_palette::RigPaletteMirrors;

use super::{TileRig, TileScene};
use crate::portrait::{material_variant, VariantLane};

/// The pool slot [`super::setup_tiles`] holds for the `CGCharacterModelBase` ctor's scene.
pub(crate) const CHARACTER_SCENE_SLOT: usize = 1;

/// What a booth needs of the pool. Every member is optional: a headless app has no render device,
/// so no pool, and a booth there draws with the world's own materials.
#[derive(SystemParam)]
pub(crate) struct PaneLights<'w> {
    rig: Option<ResMut<'w, TileRig>>,
    device: Option<Res<'w, RenderDevice>>,
    queue: Option<Res<'w, RenderQueue>>,
    mirrors: Option<ResMut<'w, RigPaletteMirrors>>,
    anim_mirrors: Option<ResMut<'w, MatAnimMirrors>>,
}

/// One booth's place in the pool: its slot and the lane its twins draw in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PaneScene {
    pub(crate) slot: usize,
    lane: VariantLane,
}

impl PaneLights<'_> {
    /// The slot for a character pane's scene, grown into the pool if new; past the pool, the
    /// character ctor scene's. `None` with no pool.
    pub(crate) fn scene(&mut self, light: ModelLight, fog: Option<ModelFog>) -> Option<PaneScene> {
        let (Some(rig), Some(device), Some(queue), Some(mirrors), Some(anim_mirrors)) = (
            self.rig.as_deref_mut(),
            self.device.as_deref(),
            self.queue.as_deref(),
            self.mirrors.as_deref_mut(),
            self.anim_mirrors.as_deref_mut(),
        ) else {
            return None;
        };
        let slot = rig.slot_or(
            TileScene::of_character(light, fog),
            CHARACTER_SCENE_SLOT,
            device,
            queue,
            mirrors,
            anim_mirrors,
        );
        // A pane that armed fog draws each batch under its authored fog policy, as a tile does.
        let lane = if rig.lights[slot].scene.fog.is_some() {
            VariantLane::RigFogged
        } else {
            VariantLane::RigUnfogged
        };
        Some(PaneScene { slot, lane })
    }

    /// The scene's light buffer, which the copy's emitters bind.
    pub(crate) fn buffer(&self, scene: Option<PaneScene>) -> Option<Buffer> {
        let rig = self.rig.as_deref()?;
        Some(rig.lights.get(scene?.slot)?.buffer.clone())
    }

    /// The twin of a world-built material under `scene`'s light, cached per slot. `None` while
    /// the source is not resident: the caller retries rather than draw a world-lit pane. With no
    /// pool, the world material itself.
    pub(crate) fn twin(
        &mut self,
        scene: Option<PaneScene>,
        world: &Handle<WowModelMaterial>,
        materials: &mut Assets<WowModelMaterial>,
    ) -> Option<Handle<WowModelMaterial>> {
        let (Some(scene), Some(rig)) = (scene, self.rig.as_deref_mut()) else {
            return Some(world.clone());
        };
        let light = rig.lights.get_mut(scene.slot)?;
        let buffer = light.buffer.clone();
        material_variant(&mut light.variants, &buffer, world, materials, scene.lane)
    }
}
