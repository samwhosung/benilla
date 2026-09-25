//! The mouseover and target model brighten, the reference's per-model highlight emissive.
//!
//! The reference sets it on a change, not per frame: `SetHighlight 0x614550` and `ClearHighlight
//! 0x6144f0` keep a per-object reason mask (bit 0 target, bit 1 mouseover), and the glow drops when
//! the last reason clears. `SetHighlight` samples the scene's committed ambient (`[0xce9cd8]`,
//! `0x614576`..`0x6145bd`) into the model, and every batch adds it before the final clamp (`c29`),
//! lit or unlit, attachments included; `0xff404040` is only the fallback before a map's light
//! loads.
//!
//! benilla carries the flag in bit 31 of the per-instance `MeshTag` (`benilla_world::mesh_tag`),
//! and `wow_model.wgsl` adds the scene ambient when it is set.

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_world::mesh_tag::HIGHLIGHT_BIT;

use super::{go_is_nearest, Hovered, HoveredObject, Selection};

/// The bit's only writer: ORs [`HIGHLIGHT_BIT`] onto every part of the hovered and targeted roots
/// and clears it on roots that left the set, the reason mask collapsed to set membership. Runs in
/// PostUpdate, after the Update writers that overwrite the whole tag. The mouseover reason covers
/// any hoverable object, so a GameObject lights like a unit, through the click's nearer-pick.
pub(super) fn apply_highlight(
    hovered: Res<Hovered>,
    hovered_go: Res<HoveredObject>,
    selection: Res<Selection>,
    stores: Query<&crate::net::ObjectStore>,
    children: Query<&Children>,
    mut tags: Query<&mut MeshTag>,
    mut was_lit: Local<Vec<Entity>>,
) {
    // `any`, not `target`: a hovered corpse also holds the brighten off a farther GameObject.
    let go = hovered_go
        .target
        .filter(|_| hovered.any().is_none() || go_is_nearest(&hovered, &hovered_go));
    let unit_hover = hovered.target.filter(|_| go.is_none());
    // A hovered corpse brightens too: the mouseover brighten (`0x49295e → 0x4945e0`) runs before
    // the publisher's type switch. Gated like its name plate: a bone pile with nothing to take
    // publishes no mouseover.
    let corpse_hover = hovered
        .corpse
        .filter(|_| go.is_none())
        .filter(|e| stores.get(*e).is_ok_and(super::corpse_mouseover_eligible));
    let mut want: Vec<Entity> = Vec::new();
    for root in [unit_hover, corpse_hover, go, selection.target]
        .into_iter()
        .flatten()
    {
        if !want.contains(&root) {
            want.push(root);
        }
    }
    for &root in was_lit.iter() {
        if !want.contains(&root) {
            set_bit(root, false, &children, &mut tags);
        }
    }
    // Every frame, not on change: the Update writers overwrite the whole tag without the bit.
    for &root in &want {
        set_bit(root, true, &children, &mut tags);
    }
    *was_lit = want;
}

/// Sets or clears the bit on `root` and every descendant `MeshTag`, attachments included.
fn set_bit(root: Entity, on: bool, children: &Query<&Children>, tags: &mut Query<&mut MeshTag>) {
    for e in std::iter::once(root).chain(children.iter_descendants(root)) {
        if let Ok(mut tag) = tags.get_mut(e) {
            let bits = if on {
                tag.0 | HIGHLIGHT_BIT
            } else {
                tag.0 & !HIGHLIGHT_BIT
            };
            if tag.0 != bits {
                tag.0 = bits;
            }
        }
    }
}
