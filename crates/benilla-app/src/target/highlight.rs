//! The mouseover / target **model brighten** — the real client's per-model highlight emissive.
//!
//! The reference pushes it on hover/target *change*, never a per-frame compare: the mouseover
//! publisher and the target setter call `SetHighlight 0x614550` / `ClearHighlight 0x6144f0` with a
//! per-object reason bitmask (bit 0 = target, bit 1 = mouseover — hover + target **stack**, and the
//! glow drops only when the last reason clears). `SetHighlight` samples the scene's committed
//! ambient (`[0xce9cd8]`, read at `0x614576`-`0x6145bd`) into the model and holds it until the last
//! reason clears; each batch adds it to its colour before the final clamp (`c29`), lit or unlit,
//! the model's attachments included. `0xff404040` is only the fallback before a map's light loads.
//!
//! benilla carries the flag in **bit 31 of the per-instance `MeshTag`** (the convention home is
//! `benilla_world::mesh_tag`); `wow_model.wgsl` adds the scene ambient when it is set. This
//! system is the bit's only writer: each frame (PostUpdate — after every Update payload writer, so
//! their whole-`u32` overwrites can't strand the bit) it ORs the flag onto every part of the
//! hovered + selected roots and clears it on roots that left the set. The reason bitmask collapses
//! to set membership: an entity is lit while it is hovered *or* targeted — same stacking result.
//! Scope tracks whatever hover/selection can resolve (the reference brightens any hoverable object
//! with a model — units today; GameObjects when they become hoverable).

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_world::mesh_tag::HIGHLIGHT_BIT;

use super::{go_is_nearest, Hovered, HoveredObject, Selection};

/// OR/clear [`HIGHLIGHT_BIT`] on the hovered + targeted roots' part tags. `was_lit` is last frame's
/// root set, so a root that loses both reasons gets its bit cleared exactly once. A hovered
/// GameObject brightens exactly like a unit (the reference's reason bit 1 = mouseover covers any
/// hoverable CGObject — the signpost glow, director-matched 2026-07-13), gated by the same
/// nearer-pick the click router uses so only the object a click would act on lights.
pub(super) fn apply_highlight(
    hovered: Res<Hovered>,
    hovered_go: Res<HoveredObject>,
    selection: Res<Selection>,
    // The hovered corpse's descriptor — the mouseover-eligibility gate below is a field read.
    stores: Query<&crate::net::ObjectStore>,
    children: Query<&Children>,
    mut tags: Query<&mut MeshTag>,
    mut was_lit: Local<Vec<Entity>>,
) {
    // [`Hovered::any`], not `target`: a hovered **corpse** is a pick too, so it
    // holds the brighten off a farther GameObject exactly as a unit does.
    let go = hovered_go
        .target
        .filter(|_| hovered.any().is_none() || go_is_nearest(&hovered, &hovered_go));
    let unit_hover = hovered.target.filter(|_| go.is_none());
    // **And the corpse body brightens too** (1729, byte-verified): the mouseover brighten
    // `0x49295e → 0x4945e0` runs *before* the publisher's type switch and tests nothing but
    // "has a model, has an object" — reason bit 1 is mouseover, and it applies to a corpse
    // exactly as to a unit or a signpost. (Bit 0 is the target brighten, which a corpse can
    // never take: `SetSelection` refuses it.) Gated by the same mouseover eligibility that
    // decides whether it gets a name plate — a bone pile with nothing to take publishes no
    // mouseover, so it neither lights nor labels.
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
    // Re-asserted every frame, not only on change: the Update payload writers (fade/interior/…)
    // overwrite the whole tag without the bit whenever they run.
    for &root in &want {
        set_bit(root, true, &children, &mut tags);
    }
    *was_lit = want;
}

/// Set/clear the highlight bit on `root` and every descendant carrying a `MeshTag` (the model's
/// parts — and its attachments, which the reference brightens with the body). A despawned root
/// simply yields no descendants.
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
