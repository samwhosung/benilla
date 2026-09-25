//! **`DropItemOnUnit 0x48d960`** — the cursor's held item dropped onto a unit, which in 1.12 is
//! how you feed your pet.
//!
//! The reference's binding forks on which unit it was given:
//!
//! - **PET** — the unit guid equals `[0xb714a0]/[0xb714a4]`, the charm-else-summon pet global →
//!   `0x6ea1e0`, the validator whose three gates are transcribed in [`feedable_pet`] → `CastSpell`
//!   with the learned Feed Pet spell and **`castItem = NULL`**, then `BindTarget(the food)`.
//! - **PLAYER** — a trade (`CMSG_INITIATE_TRADE 0x116` via `0x5d3fb0`, else a trade-slot
//!   placement). Not modelled here: the trade window owns it, and no node does yet. A drop on a
//!   player is silently refused, which is the same shape every other refusal on this path takes.
//!
//! **The wire is `CMSG_CAST_SPELL (0x12E)`, not `CMSG_USE_ITEM`,** and that falls out of the null
//! cast item: the sender's discriminator (`0x6e5513..0x6e55a7`) takes its no-item arm
//! (`je 0x6e5808`), skipping the `push 0xab` block outright. The block that goes out is
//! `{ spellId, flags = TARGET_FLAG_ITEM (0x0010), the FOOD's guid }` — **the pet's guid never
//! reaches the wire at all**, which is why this is [`TargetedBind::Item`] and not a unit bind.
//!
//! Two behaviours worth stating because they are easy to get backwards:
//!
//! - **`ClearCursor 0x495190(1,1)` runs on SUCCESS only.** Every refusal is silent and leaves the
//!   payload on the cursor — no error line, no drop. So a mis-drop costs nothing and you are still
//!   holding the food.
//! - **There is no range test on this path.** Neither function contains a floating-point
//!   instruction, and the one reachable range helper sits behind a `test [0xceac5c],0x8202` mask
//!   disjoint from `0x0010`. The server judges distance.

use bevy::prelude::*;

use benilla_ui::script::{CursorPayload, UiScript};

use crate::spell::{CastCommit, TargetedBind};

/// `0x6ea1e0` — is this unit a pet *I* can feed? All three gates, in the reference's order. Any
/// miss is a silent refusal (see the module docs), so this returns a plain bool rather than a
/// reason.
///
/// The gates are about **ownership and provenance**, not about the food: what may be fed to a pet
/// is the server's call (vmangos checks the item's food type against the pet's diet and answers
/// `SPELL_FAILED_WRONG_PET_FOOD`), and the client never looks at the item here at all.
fn feedable_pet(
    pet: &benilla_protocol::ObjectFields,
    self_guid: u64,
    feed_pet_known: bool,
) -> bool {
    // 1 · `UNIT_CREATED_BY_SPELL` non-zero — something summoned it.
    pet.unit_created_by_spell().is_some()
        // 2 · `UNIT_FIELD_CREATEDBY == my guid` — and that something was me.
        && pet.unit_created_by() == Some(self_guid)
        // 3 · the learn-time latch `[0xcecad8]` — I actually know Feed Pet. A warlock's imp is a
        //     summon owned by its master and passes both field gates; this is the leg that stops
        //     it, because only a hunter ever learns a `SPELL_EFFECT_FEED_PET` spell.
        && feed_pet_known
}

/// Drain the feed asks and run the pet leg — **both** of the reference's entries into `0x6ea1e0`:
///
/// - the **frame**: `PetFrame_OnClick`'s `DropItemOnUnit("pet")`, queued by the VM;
/// - the **world**: a left click on the pet's own model with an item held. `0x6ea1e0`'s second
///   caller is `0x4927e8`, inside the world left-click worker `0x4925d0`'s object leg, so dropping
///   food on the body feeds it identically. The generic object-leg rule still stands around it
///   (`cursor::world_drop_click`: an `Object` pick drops nothing and keeps the payload, and the
///   click still selects) — the feed is an extra arm inside that leg, not a replacement for it.
///
/// One law, one place: whichever entry it came from, the ask runs the same three gates and the same
/// commit.
pub(crate) fn drop_item_on_unit(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&crate::net::Guid, &crate::net::ObjectStore), With<crate::net::SelfPlayer>>,
    stores: Query<&crate::net::ObjectStore>,
    index: Res<crate::net::GuidIndex>,
    pet_bar: Res<crate::ui_pet::PetBar>,
    learned: Res<super::LearnedAbilities>,
    press: Res<crate::target::PressPick>,
    mut clicks: MessageReader<benilla_world::interact::WorldClick>,
    mut ladder: crate::spell::CastLadder,
) {
    let Some(mut script) = script else {
        clicks.clear();
        return;
    };
    let hovered = press.hovered;
    let mut tokens = script.take_drop_item_on_unit();
    // The world entry: a left click whose pick IS the pet. The click's own press latch is the same
    // nearest-object pick it selects from, so cursor and click agree by construction (1122: the
    // *live* hover no longer does, since a click can end a gesture that orbited).
    let clicked_pet = clicks.read().count() > 0
        && pet_bar.spells.pet_guid != 0
        && hovered.guid == Some(pet_bar.spells.pet_guid);
    if clicked_pet {
        tokens.push("pet".to_string());
    }
    if tokens.is_empty() {
        return;
    }
    let Some((self_guid, self_store)) = self_q.iter().next() else {
        return;
    };
    for token in tokens {
        // The reference compares the resolved unit's guid against the pet global; our `"pet"` token
        // resolves off the same cached guid the pet frame and `UNIT_PET` read, so
        // any other token simply is not the pet and takes the unmodelled trade leg.
        if token != "pet" {
            debug!("DropItemOnUnit({token}) — only the pet leg is modelled; refused, payload kept");
            continue;
        }
        let Some(pet_fields) = (pet_bar.spells.pet_guid != 0)
            .then(|| index.0.get(&pet_bar.spells.pet_guid))
            .flatten()
            .and_then(|&e| stores.get(e).ok())
        else {
            continue; // no pet, or it hasn't streamed in — silent, payload kept
        };
        let Some(spell_id) = learned.feed_pet else {
            continue; // gate 3 fails outright: we never learned Feed Pet
        };
        if !feedable_pet(&pet_fields.0, self_guid.0, true) {
            debug!("DropItemOnUnit(\"pet\") — not a pet I summoned; refused, payload kept");
            continue;
        }
        // The food itself: the cursor's held item, resolved to the live guid the same way the bag
        // click seam resolves a clicked slot. A Spell/Action/Macro payload is not an item and
        // cannot be dropped on a unit.
        let Some(CursorPayload::Item(held)) = script.cursor_payload() else {
            continue;
        };
        let slot0 = u8::try_from(held.slot.saturating_sub(1)).unwrap_or(0);
        let Some(item_guid) =
            crate::ui_items::slot_guid(&self_store.0, held.bag, slot0, &ladder.objects)
        else {
            continue; // the slot emptied under us — silent, as every refusal here is
        };
        debug!("DropItemOnUnit(\"pet\") — Feed Pet {spell_id} at item {item_guid:#x}");
        ladder.commit_targeted(spell_id, CastCommit::Spell, TargetedBind::Item(item_guid));
        // `ClearCursor(1,1)` — success only.
        script.clear_cursor_payload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x6ea1e0`'s three gates, each failed alone. The warlock's-imp row is the
    /// one that earns gate 3: it passes both field gates and must still be refused.
    #[test]
    fn the_feed_gates_are_ownership_and_provenance() {
        use benilla_protocol::messages::ObjectFields;
        const ME: u64 = 0xDEAD_BEEF;
        // Field 146 is `UNIT_CREATED_BY_SPELL`; 14/15 are `UNIT_FIELD_CREATEDBY`'s guid pair.
        let pet = |created_by_spell: Option<u32>, created_by: Option<u64>| {
            let mut pairs: Vec<(u16, u32)> = Vec::new();
            if let Some(s) = created_by_spell {
                pairs.push((146, s));
            }
            if let Some(g) = created_by {
                pairs.push((14, g as u32));
                pairs.push((15, (g >> 32) as u32));
            }
            ObjectFields::from_pairs(&pairs)
        };
        let mine = pet(Some(883), Some(ME));
        assert!(feedable_pet(&mine, ME, true), "my own summoned pet feeds");

        // 1 · nothing summoned it — a plain creature that happens to name me.
        assert!(!feedable_pet(&pet(None, Some(ME)), ME, true));
        // 2 · summoned, but by somebody else.
        assert!(!feedable_pet(&pet(Some(883), Some(1)), ME, true));
        // 3 · a warlock's imp: BOTH field gates pass, and it is still not feedable, because only a
        //     hunter ever learns a SPELL_EFFECT_FEED_PET spell.
        assert!(!feedable_pet(&mine, ME, false));
    }
}
