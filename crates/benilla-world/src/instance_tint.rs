//! The per-instance body **TINT** — a unit's whole model multiplied by one colour (decision 0812):
//! the render channel for the aura state kit's CharProc 1 ([`crate::aura_visual`]), which had the
//! nodes modelled and nowhere to put them.
//!
//! **What it is in the reference.** A CM2 model instance carries two per-instance colour slots
//! beside its fade alpha (`model+0x180`): a **modulate** tint at `model+0x184/188/18c` (setter
//! `0x710cf0`, default `1,1,1`) and an **additive** highlight emissive at `+0x190/194/198`
//! (`0x710d40`, default `0,0,0`). benilla already carries the additive one — that is
//! [`crate::mesh_tag::HIGHLIGHT_BIT`], the hover/target brighten — because its colour is a single
//! config constant and fits in a flag bit. This module is its modulate sibling, which needs a real
//! per-instance RGB.
//!
//! **Where it lands in the draw.** The animate kernel
//! `0x714260` derives `[ebx+0x1a0..0x1a8] = tint × inputColorA` per frame (per-channel `fmul` at
//! `0x7142cc`/`0x7142de`/`0x7142ed`); the batcher `0x70c3a9`–`0x70c4d3` clamps it to [0,1],
//! byte-packs `0xAARRGGBB` and issues gx **SetState(1, modulatedColor)** at `0x70c8ad`, which drives
//! the device-init `GL_COLOR_MATERIAL` / `GL_AMBIENT_AND_DIFFUSE` current-colour path. So the tint
//! is the **material ambient+diffuse colour**: it multiplies the light sum *inside* the [0,1] clamp
//! and NOT the emission terms — structurally identical to what a WMO batch's MOCV already does in
//! `wow_model.wgsl`, which is where the shader multiplies it. An UNLIT batch's program adds the
//! tint·M2Color term and the highlight inside one clamp (`c28 + c29`), so the fullbright path takes
//! the tint and the highlight both.
//!
//! **The channel: no new `MeshTag` bits.** `MeshTag` bits 19..=29 already carry the instance's rig
//! slot ([`crate::rig_palette`]) — allocated per unit, written into every skinned part's tag at
//! spawn, and preserved by every runtime tag writer by construction. The vertex stage reads it to
//! index the palette region; the fragment stage now reads the *same* slot to index this table. That
//! is what makes the tint free of the bit budget the alpha and probe payloads fight over (5 spare
//! bits outdoors, **zero** indoors): it needs none.
//!
//! The rig field is safe to read on an unskinned instance because `WOW_RIG_SKIN` is keyed on the
//! mesh's vertex layout (`WowModelExt::specialize` tests `ATTRIBUTE_WOW_JOINT_INDEX`), never on the
//! tag — so a static mesh carrying a nonzero slot is not skinned by it, and the field is a plain
//! per-instance index. Slot **0** is the world's no-rig sentinel (terrain, WMO, doodads, clutter,
//! every unskinned part), so it is never written and always reads identity.
//!
//! **`0` means identity, and that is the client's own packing.** `0x60d840` builds the node value as
//! `round(param) | 0xff000000` (`0x60d8cc`) — an authored colour always carries a full alpha byte,
//! so a `0` word cannot be a real tint. A zeroed region is therefore a no-op by construction: the
//! portrait booths' studio buffers, slot 0, and every frame before anything is tinted all cost
//! nothing and change no pixel. The one shipped kit that authors a **black** body tint (spell 27200
//! Defile, kit 6531, `param0 = 0`) survives it, because we store `0xFF000000` for it, not `0`.
//!
//! **Not eased.** The reference applies the tint head node on *change* (`unit+0xd04`
//! change-detection → `×1/255` → `0x710cf0`), with none of the 1000 ms cubic ramp its sibling alpha
//! gets (`0x614f80`). A tint appears and disappears instantly, and that is faithful.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::mesh_tag::MAX_RIG_SLOTS;

/// Bytes per slot in the tint region: one packed `0xFFRRGGBB` word.
const SLOT_BYTES: u64 = 4;

/// The "no tint" word — see the module doc. Zero is identity, so a zeroed buffer is inert.
pub const IDENTITY: u32 = 0;

/// Byte offset of the instance-tint region inside a `wow_light`-layout buffer: between the rig slot
/// table and the palette rows, mirroring `wow_model.wgsl`'s struct order (the palette array is
/// runtime-sized, so it has to stay last).
pub(crate) fn region_offset() -> u64 {
    crate::rig_palette::rig_table_region_offset() + MAX_RIG_SLOTS as u64 * SLOT_BYTES
}

/// Bytes this region adds to every `wow_light`-layout buffer (8 KB at 2048 slots).
pub(crate) fn region_bytes() -> u64 {
    MAX_RIG_SLOTS as u64 * SLOT_BYTES
}

/// Pack an RGB triple the way the reference does (`0x60d840`: `param | 0xff000000`) — so the word is
/// never [`IDENTITY`], even for an authored black.
pub fn pack(rgb: [u8; 3]) -> u32 {
    0xff00_0000 | (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])
}

/// Off-world `wow_light`-layout buffers that also carry the tint region — registered by key, the
/// same shape as [`crate::rig_palette::RigPaletteMirrors`] and deliberately NOT the same list.
///
/// **Why this is opt-in, and why the portrait booths are not on it.** The reference's per-instance
/// tint lives on ONE CM2 instance (`[inst+0x184..0x18c]`, setter `0x710cf0`). A unit-frame portrait
/// does not render that instance: the bake `0x524f60` builds a **fresh** CM2 from it whose ctor sets
/// colour `(1,1,1)` and alpha `1.0` (`0x70ea60`-`0x70ea89`/`0x70eaca`), and `0x525261 call 0x47a230`
/// then sets those very two fields *again* — so a ghost's portrait shows the pre-death, untinted,
/// fully-opaque face, and "a client that tints the portrait when the ghost flags are set diverges
/// from 1.12.1" (benilla report B49, decision 1481).
/// Our round portraits mirror the world entity's own children, so they carry the WORLD unit's rig
/// slot — pushing this region into their buffer would tint them with it. Their buffer keeps the
/// zero-initialised (identity) region instead, which is the reference's fresh-instance behaviour
/// reproduced by construction.
///
/// The **glue scene** is the opposite case and is why the list exists: it is not a bake standing in
/// for a UI model widget, it is the screen itself, and its character component is exactly the
/// instance the reference writes (`0x472939 -> 0x710cf0`, the char-select ghost proc). It carries its
/// OWN rig slot, allocated by the booth spawn, so nothing a world unit does can reach it.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct InstanceTintMirrors(
    pub std::collections::HashMap<&'static str, bevy::render::render_resource::Buffer>,
);

/// The live per-slot tint table, indexed by the instance's `MeshTag` rig slot. `Arc`-shared so the
/// render-world extract is a pointer bump, and generation-stamped so a world with nothing tinted
/// uploads nothing at all.
#[derive(Resource, Clone, ExtractResource)]
pub struct InstanceTints {
    slots: Arc<Vec<u32>>,
    generation: u64,
}

impl Default for InstanceTints {
    fn default() -> Self {
        Self {
            slots: Arc::new(vec![IDENTITY; MAX_RIG_SLOTS]),
            generation: 0,
        }
    }
}

impl InstanceTints {
    /// Set slot `slot`'s tint word (already packed — [`pack`], or [`IDENTITY`] to clear). Slot 0 is
    /// the world's no-rig sentinel and is never written: everything unskinned shares it, so a write
    /// there would tint terrain, doodads and WMO surfaces at once.
    pub fn set(&mut self, slot: u16, word: u32) {
        let i = slot as usize;
        if i == 0 || i >= MAX_RIG_SLOTS || self.slots[i] == word {
            return;
        }
        Arc::make_mut(&mut self.slots)[i] = word;
        self.generation += 1;
    }

    /// Clear a slot back to identity — called on the aura's reap edge AND from `RigSkin`'s free
    /// hook, so a slot can never carry a dead unit's tint into the next unit that allocates it.
    pub(crate) fn clear(&mut self, slot: u16) {
        self.set(slot, IDENTITY);
    }

    /// This slot's current word (`IDENTITY` when untinted, or for an out-of-range slot).
    ///
    /// Test support: the render path reads the uploaded region, never this. Not `#[cfg(test)]`
    /// because the aura tests that assert on it live in `benilla-app`, and a dependent crate does
    /// not compile this one's test cfg.
    pub fn get(&self, slot: u16) -> u32 {
        self.slots.get(slot as usize).copied().unwrap_or(IDENTITY)
    }
}

/// Render world (`PrepareResources`): write the whole 8 KB region when anything changed. Gated on
/// the generation, so a world with no tinted unit — the overwhelmingly common case — never touches
/// the queue. Whole-region writes rather than per-slot ranges: the table is smaller than one
/// palette rig's rows, and aura edges are rare (a hundredth the traffic of the pose upload beside
/// it, which is the one that needed range coalescing).
fn upload_instance_tints(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    mirrors: Option<Res<InstanceTintMirrors>>,
    tints: Option<Res<InstanceTints>>,
    mut last: Local<Option<u64>>,
) {
    let Some(tints) = tints else { return };
    // The generation gate covers the mirror list too: a mirror registered after the last write
    // would otherwise never be filled. Mirrors are registered once per booth/scene buffer, so
    // folding their count into the gate costs one comparison and closes that hole.
    let mirrors = mirrors
        .map(|m| m.0.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let gate = tints.generation ^ ((mirrors.len() as u64) << 40);
    if *last == Some(gate) {
        return;
    }
    *last = Some(gate);
    let words = bytemuck::cast_slice(tints.slots.as_slice());
    for buffer in shared.iter().map(|s| &s.0).chain(mirrors.iter()) {
        queue.write_buffer(buffer, region_offset(), words);
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<InstanceTints>()
        .init_resource::<InstanceTintMirrors>()
        .add_plugins(ExtractResourcePlugin::<InstanceTints>::default())
        .add_plugins(ExtractResourcePlugin::<InstanceTintMirrors>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            upload_instance_tints.in_set(RenderSystems::PrepareResources),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's own packing: the full alpha byte rides every authored colour, which is what
    /// keeps `0` free as the identity sentinel — including for the one kit that authors black.
    #[test]
    fn packing_matches_the_reference_and_never_collides_with_identity() {
        // The ghost aura's node value, byte-for-byte (`round(9222653) | 0xff000000`).
        assert_eq!(pack([0x8c, 0xb9, 0xfd]), 0xff8c_b9fd);
        // Spell 27200 Defile authors param0 = 0 — a real black body tint, not an absent one.
        assert_eq!(pack([0, 0, 0]), 0xff00_0000);
        assert_ne!(pack([0, 0, 0]), IDENTITY);
    }

    /// Slot 0 is shared by every unskinned instance in the world (terrain, WMO, doodads, clutter):
    /// writing a tint there would colour the whole scene, so the setter refuses it.
    #[test]
    fn slot_zero_is_never_written() {
        let mut t = InstanceTints::default();
        t.set(0, pack([255, 0, 0]));
        assert_eq!(t.get(0), IDENTITY);
        assert_eq!(t.generation, 0, "and it costs no upload");
    }

    /// A world with nothing tinted never bumps the generation, so it never uploads; a real change
    /// bumps once, and a redundant re-write of the same colour does not.
    #[test]
    fn the_generation_tracks_real_changes_only() {
        let mut t = InstanceTints::default();
        assert_eq!(t.generation, 0);
        t.set(7, pack([0x8c, 0xb9, 0xfd]));
        assert_eq!(t.generation, 1);
        t.set(7, pack([0x8c, 0xb9, 0xfd]));
        assert_eq!(t.generation, 1, "same colour, no upload");
        t.clear(7);
        assert_eq!(t.generation, 2);
        assert_eq!(t.get(7), IDENTITY);
        t.clear(7);
        assert_eq!(t.generation, 2, "already clear");
    }

    /// An out-of-range slot is dropped rather than panicking — the 11-bit tag field cannot express
    /// one, but the setter takes a `u16` and must not trust it.
    #[test]
    fn an_out_of_range_slot_is_dropped() {
        let mut t = InstanceTints::default();
        t.set(u16::MAX, pack([1, 2, 3]));
        assert_eq!(t.generation, 0);
        assert_eq!(t.get(u16::MAX), IDENTITY);
    }
}
