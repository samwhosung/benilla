//! `DropItemOnUnit 0x48d960`: a held item dropped on a unit, which in 1.12 feeds the pet. On the
//! pet (the guid global `[0xb714a0]`) the validator `0x6ea1e0` casts the learned Feed Pet with no
//! cast item, so the wire is `CMSG_CAST_SPELL` with `TARGET_FLAG_ITEM` and the food's guid, never
//! the pet's (`0x6e5513`-`0x6e55a7`). On a player the reference starts a trade (`0x5d3fb0`) or
//! fills a trade slot; that leg is not built and refuses. Every refusal is silent and keeps the
//! payload (`ClearCursor 0x495190` runs on success only), and the server alone judges range.

use bevy::prelude::*;

use benilla_ui::script::{CursorPayload, UiScript};

use crate::spell::{CastCommit, TargetedBind};

/// `0x6ea1e0`'s three gates, in order: is this a pet I can feed? The food is not checked here;
/// the server judges the diet (`Spell.cpp:5912`, `SPELL_FAILED_WRONG_PET_FOOD`).
fn feedable_pet(
    pet: &benilla_protocol::ObjectFields,
    self_guid: u64,
    feed_pet_known: bool,
) -> bool {
    // 1: `UNIT_CREATED_BY_SPELL` is set, so something summoned it.
    pet.unit_created_by_spell().is_some()
        // 2: `UNIT_FIELD_CREATEDBY` is my guid.
        && pet.unit_created_by() == Some(self_guid)
        // 3: the Feed Pet latch `[0xcecad8]`: an imp passes 1 and 2, but only hunters learn it.
        && feed_pet_known
}

/// Runs the pet leg for both of the reference's entries into `0x6ea1e0`: `PetFrame_OnClick`'s
/// `DropItemOnUnit("pet")`, and a left click on the pet's model with an item held (`0x4927e8`,
/// in the world click's object leg `0x4925d0`, where the click still selects).
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
    // The world entry: a click whose press-time pick is the pet. That is the pick the click
    // selects from; the live hover can differ after a drag.
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
        // Only the "pet" token is the pet; any other unit takes the unbuilt trade leg.
        if token != "pet" {
            debug!("DropItemOnUnit({token}) — only the pet leg is modelled; refused, payload kept");
            continue;
        }
        let Some(pet_fields) = (pet_bar.spells.pet_guid != 0)
            .then(|| index.0.get(&pet_bar.spells.pet_guid))
            .flatten()
            .and_then(|&e| stores.get(e).ok())
        else {
            continue; // no pet, or not streamed in: silent, payload kept
        };
        let Some(spell_id) = learned.feed_pet else {
            continue; // gate 3 fails outright: we never learned Feed Pet
        };
        if !feedable_pet(&pet_fields.0, self_guid.0, true) {
            debug!("DropItemOnUnit(\"pet\") — not a pet I summoned; refused, payload kept");
            continue;
        }
        // The food: the held item's live guid. Only an item payload can be dropped on a unit.
        let Some(CursorPayload::Item(held)) = script.cursor_payload() else {
            continue;
        };
        let slot0 = u8::try_from(held.slot.saturating_sub(1)).unwrap_or(0);
        let Some(item_guid) =
            crate::ui_items::slot_guid(&self_store.0, held.bag, slot0, &ladder.objects)
        else {
            continue; // the slot emptied: silent, as every refusal here is
        };
        debug!("DropItemOnUnit(\"pet\") — Feed Pet {spell_id} at item {item_guid:#x}");
        ladder.commit_targeted(spell_id, CastCommit::Spell, TargetedBind::Item(item_guid));
        // `ClearCursor(1,1)`, on success only.
        script.clear_cursor_payload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x6ea1e0`'s three gates, each failed alone.
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

        // 1: not summoned, though it names me.
        assert!(!feedable_pet(&pet(None, Some(ME)), ME, true));
        // 2: summoned by someone else.
        assert!(!feedable_pet(&pet(Some(883), Some(1)), ME, true));
        // 3: a warlock's imp passes both field gates, but its owner never learns Feed Pet.
        assert!(!feedable_pet(&mine, ME, false));
    }
}
