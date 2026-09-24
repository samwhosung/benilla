//! The pet bar's one drag verb, `PickupPetAction`: a pickup with an empty cursor, a drop with a
//! pet payload held (`PetActionBarFrame.lua:252-283`). Only a pet slot makes or takes the
//! payload, a word carried verbatim (`0x4bce00`), so a drag only rearranges the bar the server
//! sent. A drop clears the cursor before deciding (`0x495190`) and only a write that relocated
//! nothing refills it (`0x4bce38`), so a refused drop loses the payload, as in the reference.

use mlua::Lua;

use crate::script::Model;

use super::{queue_cursor_update, CursorPayload, CursorPetAction};

/// Pet bar slots (`NUM_PET_ACTION_SLOTS`, vmangos `MAX_UNIT_ACTION_BAR_INDEX`). Deviation: slot
/// 11 is refused, because the reference's gate (`cmp esi,0xa; jbe`) admits it and neither
/// `0x4bce00` nor `0x4bc9a0` bounds-checks again, so it writes one dword past the array.
const PET_SLOTS: u32 = 10;

/// The slot type as the client masks it.
fn kind(packed: u32) -> u8 {
    ((packed >> 24) & 0x3F) as u8
}

/// A command (7) or reaction (6) word, which the core treats alike.
fn is_token(packed: u32) -> bool {
    matches!(kind(packed), 6 | 7)
}

/// The word as a payload, if it can be one: `0x494e20`'s jump table (`0x494f40`) takes types 1-7
/// only, so displacing an empty slot carries nothing.
fn payload_word(packed: u32) -> Option<u32> {
    (1..=7).contains(&kind(packed)).then_some(packed)
}

/// The duplicate scan's compare (`0x4bca44`, `0x4bca57`): all but the autocast bits, any type.
fn same_action(a: u32, b: u32) -> bool {
    a & 0x3FFF_FFFF == b & 0x3FFF_FFFF
}

/// The word a spell pickup leaves in its slot: type 1, id 0. The duplicate scan skips this
/// source (`0x4bca34`), which would otherwise match every other blanked slot.
fn is_blanked_spell(packed: u32) -> bool {
    kind(packed) == 1 && packed & 0xFFFF == 0
}

/// A slot a token occupant may move to (`0x4bca8b`), such as an empty or blanked-spell slot.
fn is_relocation_candidate(packed: u32) -> bool {
    !is_token(packed) && packed & 0xFFFF == 0
}

/// What one accepted assignment did.
struct Assigned {
    /// The slot the target's previous occupant was moved to, if it was moved.
    reloc: Option<usize>,
    /// The occupant with nowhere to go, for the cursor; `None` if it was relocated or empty.
    displaced: Option<u32>,
}

/// The assign core (`0x4bc9a0`), the whole of a drop; `None` is no write and no send. In order:
/// an identical dword is a no-op (`0x4bc9e2`), a passive spell is refused silently (`0x4bc9f8`),
/// a duplicate's slot takes the occupant, which makes a drag a swap (`0x4bca78`), else a token
/// occupant needs a candidate slot or the drop aborts. The reference's return (`0x4bcb92`), 1
/// only when nothing was relocated, cannot tell an abort from a relocation; this one can.
fn assign(slots: &mut [u32], target: usize, source: u32, passive: bool) -> Option<Assigned> {
    let occupant = *slots.get(target)?;
    if occupant == source {
        return None;
    }
    if kind(source) == 1 && passive {
        return None;
    }

    let mut reloc = None;
    if !is_blanked_spell(source) {
        reloc = (0..slots.len()).find(|&i| i != target && same_action(slots[i], source));
    }
    if reloc.is_none() && is_token(occupant) {
        // No `j != target` guard, as in the reference: the target's token is never a candidate.
        reloc = (0..slots.len()).find(|&j| is_relocation_candidate(slots[j]));
        reloc?; // the abort: a token with nowhere to go writes and sends nothing
    }

    if let Some(j) = reloc {
        slots[j] = occupant;
    }
    slots[target] = source;
    Some(Assigned {
        reloc,
        displaced: reloc.is_none().then(|| payload_word(occupant)).flatten(),
    })
}

/// `PickupPetAction(slot)` (`0x4be180`), 1-based. `UNIT_FLAG_POSSESSED` blocks both ends, its
/// gate sitting above the cursor fork (`0x4be20a`). Another payload stays held here, where the
/// reference clears it (`0x4be220`) and picks the slot up. A pickup blanks only a spell slot
/// (`0x4be268`), so a dragged token is copied, not moved.
pub(super) fn pickup_pet_action(model: &mut Model, slot: u32) -> bool {
    if !model.pet_bar.pickup_allowed {
        return false;
    }
    match &model.cursor {
        Some(CursorPayload::PetAction(_)) => return place_pet_action(model, slot),
        Some(_) => return false,
        None => {}
    }
    let Some(index) = slot_index(model, slot) else {
        return false;
    };
    let view = model.pet_bar.slots[index].view.clone();
    let Some(packed) = payload_word(view.packed) else {
        return false;
    };

    model.cursor = Some(CursorPayload::PetAction(CursorPetAction {
        src_slot: slot,
        packed,
        passive: view.passive,
        texture: view.texture.clone(),
    }));
    // A spell slot keeps its type and autocast bits with id 0, and the server is told. `false` is
    // right: the passive gate reads the source's spell record, and id 0 has none.
    if kind(packed) == 1 {
        write_slot(model, index, packed & 0xFFFF_0000, false);
    }
    queue_cursor_update(model);
    true
}

/// The drop: the cursor always empties, and only a homeless occupant refills it (`0x4bce3d`).
/// Returns whether anything was written; the cursor empties on `false` too.
fn place_pet_action(model: &mut Model, slot: u32) -> bool {
    let Some(CursorPayload::PetAction(held)) = model.cursor.clone() else {
        return false;
    };
    // `ClearCursor(1,1)`, unconditionally, before anything is decided.
    model.cursor = None;
    let Some(index) = slot_index(model, slot) else {
        queue_cursor_update(model);
        return false;
    };
    // Read before the write: a displaced word's passive bit and icon are its slot's.
    let occupant_view = model.pet_bar.slots[index].view.clone();
    let Some(assigned) = write_slot(model, index, held.packed, held.passive) else {
        queue_cursor_update(model);
        return false;
    };

    model.cursor = assigned.displaced.map(|packed| {
        CursorPayload::PetAction(CursorPetAction {
            src_slot: slot,
            packed,
            passive: occupant_view.passive,
            texture: occupant_view.texture.clone(),
        })
    });
    queue_cursor_update(model);
    true
}

/// The 0-based index of a 1-based slot on a bar that exists.
fn slot_index(model: &Model, slot: u32) -> Option<usize> {
    if !model.pet_bar.has_bar || slot == 0 || slot > PET_SLOTS {
        return None;
    }
    let index = slot as usize - 1;
    (index < model.pet_bar.slots.len()).then_some(index)
}

/// Run [`assign`] on the engine's optimistic mirror of the ten words and queue the send; the
/// pickup's blank goes through it too, as in the reference (`0x4be268`).
fn write_slot(model: &mut Model, target: usize, source: u32, passive: bool) -> Option<Assigned> {
    let mut words: Vec<u32> = model.pet_bar.slots.iter().map(|s| s.view.packed).collect();
    let assigned = assign(&mut words, target, source, passive)?;

    // One `CMSG_PET_SET_ACTION`: the relocation pair first whenever there was one, changed or
    // not, then the target pair (`0x4bcad4`).
    let mut entries: Vec<(u32, u32)> = Vec::with_capacity(2);
    if let Some(j) = assigned.reloc {
        entries.push((j as u32, words[j]));
    }
    entries.push((target as u32, words[target]));
    model.pet_set_actions.push(entries);

    for (i, word) in words.into_iter().enumerate() {
        model.pet_bar.slots[i].view.packed = word;
    }
    Some(assigned)
}

/// A token's texture is a global's name (`GetPetActionInfo`, `0x4bdc50`), which the bar resolves
/// with `getglobal` (`PetActionBarFrame.lua:102`); the cursor needs the path, resolved here.
fn resolve_token_icon(lua: &Lua) {
    let name = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match &model.cursor {
            Some(CursorPayload::PetAction(p)) if is_token(p.packed) => p.texture.clone(),
            _ => None,
        }
    };
    let Some(name) = name else {
        return;
    };
    let path: Option<String> = lua.globals().get(name.as_str()).unwrap_or_default();
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(CursorPayload::PetAction(p)) = &mut model.cursor {
        p.texture = path;
    }
}

/// Register the pet bar's one cursor global.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "PickupPetAction",
        lua.create_function(|lua, slot: u32| {
            let took = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                pickup_pet_action(&mut model, slot)
            };
            // Both the pickup and a drop's displaced occupant can put a token on the cursor.
            resolve_token_icon(lua);
            Ok(took)
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{PetActionView, UiScript};

    /// A hunter's bar words. `FILLER` is vmangos's unused slot, `ACT_DISABLED` with spell 0: type
    /// 1, not a zero word, so it must stay as sent.
    const ATTACK: u32 = 0x0700_0002;
    const FOLLOW: u32 = 0x0700_0001;
    const DEFENSIVE: u32 = 0x0600_0001;
    const CLAW: u32 = 0xC100_0BC2;
    const GROWL: u32 = 0x8100_0EC0;
    const BITE: u32 = 0x8100_0EC1;
    const FILLER: u32 = 0x8100_0000;

    /// A token drag always takes this path: a token pickup leaves its slot as it was.
    #[test]
    fn a_source_still_on_the_bar_swaps_with_its_destination() {
        let mut bar = [CLAW, GROWL, FILLER, ATTACK];
        let a = assign(&mut bar, 1, CLAW, false).unwrap();
        assert_eq!(bar[1], CLAW, "the source word landed");
        assert_eq!(a.reloc, Some(0), "and its own old slot took the occupant");
        assert_eq!(bar[0], GROWL);
        assert_eq!(a.displaced, None, "nothing left over to carry");
    }

    /// The pickup's blank (the duplicate scan skips it, `0x4bca34`) leaves no copy to swap with.
    #[test]
    fn a_blanked_source_displaces_the_occupant_instead_of_swapping() {
        let mut bar = [CLAW, GROWL, FILLER, ATTACK];
        assign(&mut bar, 0, CLAW & 0xFFFF_0000, false).unwrap();
        assert_eq!(bar[0], CLAW & 0xFFFF_0000, "the pickup blanked slot 0");
        let a = assign(&mut bar, 1, CLAW, false).unwrap();
        assert_eq!(bar[1], CLAW);
        assert_eq!(a.reloc, None);
        assert_eq!(a.displaced, Some(GROWL));
    }

    #[test]
    fn a_token_occupant_is_relocated_not_displaced() {
        let mut bar = [CLAW, ATTACK, FILLER, DEFENSIVE];
        let a = assign(&mut bar, 1, BITE, false).unwrap();
        assert_eq!(bar[1], BITE);
        assert_eq!(a.reloc, Some(2), "ATTACK moved to the filler slot");
        assert_eq!(bar[2], ATTACK);
        assert_eq!(a.displaced, None, "nothing is left over to carry");
    }

    #[test]
    fn a_token_occupant_with_no_candidate_aborts_entirely() {
        let mut bar = [CLAW, ATTACK, GROWL, DEFENSIVE];
        let before = bar;
        assert!(assign(&mut bar, 1, BITE, false).is_none());
        assert_eq!(bar, before, "nothing written");
    }

    #[test]
    fn the_self_drop_and_the_passive_source_are_no_ops() {
        let mut bar = [CLAW, GROWL, FILLER, ATTACK];
        assert!(assign(&mut bar, 0, CLAW, false).is_none());
        assert!(assign(&mut bar, 1, CLAW, true).is_none(), "passive source");
        // The autocast bit is part of the compare: flipped, the same spell writes.
        assert!(assign(&mut bar, 0, CLAW & !0x4000_0000, false).is_some());
    }

    fn view(packed: u32, name: &str) -> PetActionView {
        PetActionView {
            name: (packed != 0).then(|| name.to_string()),
            packed,
            ..Default::default()
        }
    }

    /// A token slot as fed: `name` and `texture` are global names, as from `GetPetActionInfo`.
    fn token_view(packed: u32, name: &str, texture: &str) -> PetActionView {
        PetActionView {
            is_token: true,
            texture: Some(texture.to_string()),
            ..view(packed, name)
        }
    }

    /// A hunter's default bar, as vmangos `CharmInfo::InitPetActionBar` lays it out.
    fn hunter_bar() -> Vec<PetActionView> {
        vec![
            token_view(ATTACK, "PET_ACTION_ATTACK", "PET_ATTACK_TEXTURE"),
            token_view(FOLLOW, "PET_ACTION_FOLLOW", "PET_FOLLOW_TEXTURE"),
            token_view(0x0700_0003, "PET_ACTION_WAIT", "PET_WAIT_TEXTURE"),
            view(CLAW, "Claw"),
            view(GROWL, "Growl"),
            view(FILLER, ""),
            view(FILLER, ""),
            token_view(0x0600_0002, "PET_MODE_AGGRESSIVE", "PET_AGGRESSIVE_TEXTURE"),
            token_view(DEFENSIVE, "PET_MODE_DEFENSIVE", "PET_DEFENSIVE_TEXTURE"),
            token_view(0x0600_0000, "PET_MODE_PASSIVE", "PET_PASSIVE_TEXTURE"),
        ]
    }

    fn bar_script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, true, hunter_bar());
        s
    }

    #[test]
    fn picking_up_a_spell_blanks_its_slot_and_sends() {
        let mut s = bar_script();
        assert!(s.eval::<bool>("return PickupPetAction(4)").unwrap());
        assert_eq!(
            s.eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
                .unwrap(),
            ("petaction".to_string(), 4)
        );
        assert_eq!(
            s.take_pet_set_actions(),
            vec![vec![(3, CLAW & 0xFFFF_0000)]],
            "one pair, 0-based, the id zeroed and everything else kept"
        );
    }

    #[test]
    fn a_dragged_token_carries_a_resolved_icon_path() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, true, hunter_bar());
        s.run(r#"PET_ATTACK_TEXTURE = "Interface\\Icons\\Ability_GhoulFrenzy""#)
            .unwrap();

        s.run("PickupPetAction(1)").unwrap();
        let Some(CursorPayload::PetAction(p)) = s.cursor_payload() else {
            panic!("the command word is on the cursor")
        };
        assert_eq!(
            p.texture.as_deref(),
            Some("Interface\\Icons\\Ability_GhoulFrenzy"),
            "the global was resolved, not carried as a name"
        );

        // An unset global resolves to no icon, not to its own name.
        s.run("ClearCursor() PET_ATTACK_TEXTURE = nil PickupPetAction(1)")
            .unwrap();
        let Some(CursorPayload::PetAction(p)) = s.cursor_payload() else {
            panic!("still picked up")
        };
        assert_eq!(p.texture, None);
    }

    #[test]
    fn picking_up_a_token_leaves_the_slot_alone() {
        let mut s = bar_script();
        assert!(s.eval::<bool>("return PickupPetAction(1)").unwrap());
        assert!(
            s.take_pet_set_actions().is_empty(),
            "nothing was written, so nothing is sent"
        );
    }

    #[test]
    fn a_drag_between_two_spell_slots_sends_twice_and_carries_the_occupant() {
        let mut s = bar_script();
        s.run("PickupPetAction(4) PickupPetAction(5)").unwrap();
        assert_eq!(
            s.take_pet_set_actions(),
            vec![vec![(3, CLAW & 0xFFFF_0000)], vec![(4, CLAW)]]
        );
        assert_eq!(
            s.eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
                .unwrap(),
            ("petaction".to_string(), 5),
            "Growl was displaced and is now held, addressed as the slot it came from"
        );
    }

    /// The relocation and the write share one send: the server counts the pairs by body size
    /// (vmangos `Server/Packets/Pet.cpp:56`).
    #[test]
    fn dropping_onto_a_token_relocates_it_in_one_send() {
        let mut s = bar_script();
        s.run("PickupPetAction(4) PickupPetAction(1)").unwrap();
        let sends = s.take_pet_set_actions();
        assert_eq!(sends.len(), 2, "the pickup's blank, then the drop");
        assert_eq!(
            sends[1],
            vec![(3, ATTACK), (0, CLAW)],
            "the relocation pair FIRST, then the write — the binary's own order. And the slot the \
             spell was just picked OUT of is the first candidate, so Attack lands there."
        );
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    /// The reachable refusal is a passive spell, which the pickup does not gate; its blank
    /// already went to the server, which is how a pet spell is taken off the bar.
    #[test]
    fn a_refused_drop_eats_the_payload() {
        let mut s = UiScript::new().unwrap();
        let mut bar = hunter_bar();
        bar[3] = PetActionView {
            passive: true,
            ..view(CLAW, "Claw")
        };
        s.set_pet_actions(true, true, true, bar);

        s.run("PickupPetAction(4) PickupPetAction(5)").unwrap();
        assert_eq!(
            s.take_pet_set_actions(),
            vec![vec![(3, CLAW & 0xFFFF_0000)]],
            "the pickup's own blank went through; the drop wrote and sent nothing"
        );
        assert!(
            s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
            "and the cursor is empty — the reference cleared it before it ever decided"
        );
    }

    #[test]
    fn slot_eleven_is_refused_rather_than_written_past_the_array() {
        let mut s = bar_script();
        assert!(!s.eval::<bool>("return PickupPetAction(11)").unwrap());
        assert!(!s.eval::<bool>("return PickupPetAction(0)").unwrap());
        assert!(s.take_pet_set_actions().is_empty());
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    /// The third flag, `pickup_allowed`, is false under `UNIT_FLAG_POSSESSED`.
    #[test]
    fn a_possessed_bar_takes_no_drag_at_all() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, false, hunter_bar());
        assert!(!s.eval::<bool>("return PickupPetAction(4)").unwrap());
        assert!(s.take_pet_set_actions().is_empty());
    }

    #[test]
    fn the_pet_payload_and_the_action_bar_refuse_each_other() {
        let mut s = bar_script();
        s.set_action(
            1,
            Some(crate::script::ActionSlot {
                texture: Some("Interface\\Icons\\Spell_A".into()),
                kind: 0,
                action: 133,
                count: 0,
                consumable: false,
            }),
        );

        s.run(
            r#"
            petshows, barshows = 0, 0
            local f = CreateFrame("Frame", "PetGridListener")
            f:RegisterEvent("PET_BAR_SHOWGRID")
            f:RegisterEvent("ACTIONBAR_SHOWGRID")
            f:SetScript("OnEvent", function()
                if event == "PET_BAR_SHOWGRID" then petshows = petshows + 1 end
                if event == "ACTIONBAR_SHOWGRID" then barshows = barshows + 1 end
            end)
            "#,
        )
        .unwrap();

        s.run("PickupPetAction(4)").unwrap();
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return petshows").unwrap(), 1);
        assert_eq!(
            s.eval::<i64>("return barshows").unwrap(),
            0,
            "the action bar's grid stays down — nothing can land there"
        );
        // PlaceAction refuses it outright and leaves it held.
        assert!(!s.eval::<bool>("return PlaceAction(1)").unwrap());
        assert!(s
            .eval::<bool>("local k = GetCursorInfo() return k == 'petaction'")
            .unwrap());
        assert!(
            s.take_action_sets().is_empty(),
            "and writes nothing to the action bar"
        );
    }
}
