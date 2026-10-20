//! The pet bar's one drag verb, `PickupPetAction`: a drop with a pet payload held, a pickup
//! otherwise (`PetActionBarFrame.lua:252-283`). Only a pet slot makes or takes the
//! payload, a word carried verbatim (`0x4bce00`), so a drag only rearranges the bar the server
//! sent. A drop clears the cursor before deciding (`0x495190`) and only a write that relocated
//! nothing refills it (`0x4bce38`), so a refused drop loses the payload, as in the reference.
//! Both bar writes, the drop's (`0x4bce33`) and the pickup's blank (`0x4be27f`), ask the pet's
//! usability predicate first (`0x4bc9d0`): on an unusable pet the drop writes nothing and refills
//! nothing, and the pickup holds its action with the slot left as it was.

use mlua::{Lua, Value};

use crate::script::binding_abi;
use crate::script::pet::StoredPetAction;
use crate::script::tick::verb;
use crate::script::{Model, PetActionView};

use super::{clear_cursor, set_cursor, CursorPayload, CursorPetAction};

/// Pet bar slots (`NUM_PET_ACTION_SLOTS`, vmangos `MAX_UNIT_ACTION_BAR_INDEX`). Deviation: slot
/// 11 is refused, because the reference's gate (`cmp esi,0xa; jbe`) admits it and neither
/// `0x4bce00` nor `0x4bc9a0` bounds-checks again, so it writes one dword past the array, and the
/// autocast toggle (`0x4bcbb0`) reads that word (`[0xb71438+0x28]`) with no bounds check.
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

/// `PickupPetAction(slot)` (`0x4be180`), 1-based. A pet whose object does not resolve
/// (`0x4be1f7`) or is `UNIT_FLAG_POSSESSED` (`0x4be20a`) blocks both ends, the gate sitting above
/// the cursor fork, so the cursor is left as it was. Past it the binding clears with
/// `ClearCursor(1,1)` (`0x4be220`), whatever is held, before anything is decided: a held pet
/// action drops, anything else is dropped and the slot picked up. The pickup is the pet-action
/// setter (`0x494e20`), whose own inner clear signals again on the empty cursor before the set
/// and its `PET_BAR_SHOWGRID`. A pickup blanks only a spell slot (`0x4be268`), so a dragged token
/// is copied, not moved; the blank comes after the setter's events, so they read the slot as it
/// was. Returns whether to repaint.
pub(super) fn pickup_pet_action(lua: &Lua, slot: u32) -> bool {
    {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if !model.pet_bar.pickup_allowed {
            return false;
        }
    }
    match clear_and_drop(lua, slot) {
        Some(wrote) => wrote,
        None => {
            let picked = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                pick_pet_action(&model, slot)
            };
            let Some((payload, blank)) = picked else {
                return false;
            };
            set_pet_action(lua, payload);
            if let Some((index, word)) = blank {
                verb(lua, |model| {
                    // A spell word with id 0 paints nothing (`0x4bdc50`'s spell arm finds no
                    // record).
                    let blank = PetActionView {
                        packed: word,
                        ..Default::default()
                    };
                    write_slot(model, index, word, false, blank);
                });
            }
            true
        }
    }
}

/// The arm `PickupPetAction` (`0x4be220`-`0x4be231`) and `TogglePetAutocast` (`0x4be2f8`-
/// `0x4be309`) share: read the pet-action payload (`0xb4e2f8`), clear the cursor with
/// `ClearCursor(1,1)` whatever is held, then drop a held payload on `slot` through the assign
/// core. `Some(wrote)` is a payload dropped, `None` is none held, the cursor cleared all the same.
pub(in crate::script) fn clear_and_drop(lua: &Lua, slot: u32) -> Option<bool> {
    let held = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match &model.cursor {
            Some(CursorPayload::PetAction(held)) => Some(held.clone()),
            _ => None,
        }
    };
    verb(lua, clear_cursor);
    let held = held?;
    let (wrote, displaced) = verb(lua, |model| place_pet_action(model, slot, &held));
    if let Some(displaced) = displaced {
        set_pet_action(lua, displaced);
    }
    Some(wrote)
}

/// The pet-action setter (`0x494e20`): a token's icon is resolved before its inner clear, then
/// the clear and the set run as their own segments ([`set_cursor`]).
fn set_pet_action(lua: &Lua, mut payload: CursorPetAction) {
    resolve_token_icon(lua, &mut payload);
    set_cursor(lua, true, |_| Some(CursorPayload::PetAction(payload)));
}

/// The slot's word as a payload past the binding's clear (`0x4be23e`): the bar must exist and the
/// word be one the setter takes. A spell slot also owes its blank, `(index, word)`: it keeps its
/// type and autocast bits with id 0, and the server is told. `false` is right for that write: the
/// passive gate reads the source's spell record, and id 0 has none.
fn pick_pet_action(model: &Model, slot: u32) -> Option<(CursorPetAction, Option<(usize, u32)>)> {
    let index = slot_index(model, slot)?;
    let view = &model.pet_bar.slots[index].view;
    let packed = payload_word(view.packed)?;
    let payload = CursorPetAction {
        src_slot: slot,
        packed,
        passive: view.passive,
        texture: view.texture.clone(),
        view: view.clone(),
    };
    Some((
        payload,
        (kind(packed) == 1).then_some((index, packed & 0xFFFF_0000)),
    ))
}

/// The drop past its clear: the bar write (`0x4bce00`). Only a homeless occupant goes back on the
/// cursor (`0x4bce3d`), through the setter as a pickup does. Returns whether anything was
/// written, and that occupant.
fn place_pet_action(
    model: &mut Model,
    slot: u32,
    held: &CursorPetAction,
) -> (bool, Option<CursorPetAction>) {
    let Some(index) = slot_index(model, slot) else {
        return (false, None);
    };
    // Read before the write: a displaced word's passive bit and icon are its slot's.
    let occupant_view = model.pet_bar.slots[index].view.clone();
    let Some(assigned) = write_slot(model, index, held.packed, held.passive, held.view.clone())
    else {
        return (false, None);
    };
    let displaced = assigned.displaced.map(|packed| CursorPetAction {
        src_slot: slot,
        packed,
        passive: occupant_view.passive,
        texture: occupant_view.texture.clone(),
        view: occupant_view.clone(),
    });
    (true, displaced)
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
/// pickup's blank goes through it too, as in the reference (`0x4be268`). `0x4bc9a0` takes its
/// send flag from both callers as 1, which makes it ask `0x4bcf70` before anything else
/// (`0x4bc9c7`-`0x4bc9d7`); the app's answer, the one the bar's `GetPetActionsUsable` reads, is
/// `PetBarState::actions_usable`. Each word carries its paint: a relocated occupant its own, the
/// target `source_view`. The send flag also signals `PET_BAR_UPDATE` after the send
/// (`0x4bcb6d`), in place.
fn write_slot(
    model: &mut Model,
    target: usize,
    source: u32,
    passive: bool,
    source_view: PetActionView,
) -> Option<Assigned> {
    if !model.pet_bar.actions_usable {
        return None;
    }
    let mut words: Vec<u32> = model.pet_bar.slots.iter().map(|s| s.view.packed).collect();
    let assigned = assign(&mut words, target, source, passive)?;
    if let Some(j) = assigned.reloc {
        let occupant = std::mem::take(&mut model.pet_bar.slots[target]);
        model.pet_bar.slots[j] = occupant;
    }
    model.pet_bar.slots[target] = StoredPetAction {
        view: source_view,
        cooldown: None,
    };

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
    model.raise("PET_BAR_UPDATE", Vec::new());
    Some(assigned)
}

/// A token's texture is a global's name (`GetPetActionInfo`, `0x4bdc50`), which the bar resolves
/// with `getglobal` (`PetActionBarFrame.lua:102`); the cursor needs the path, resolved here, as
/// the setter resolves the token's string before its clear (`0x494ede`).
fn resolve_token_icon(lua: &Lua, payload: &mut CursorPetAction) {
    if !is_token(payload.packed) {
        return;
    }
    let Some(name) = payload.texture.clone() else {
        return;
    };
    payload.texture = lua.globals().raw_get(name.as_str()).unwrap_or_default();
}

/// Register the pet bar's one cursor global.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "PickupPetAction",
        lua.create_function(|lua, slot: Value| {
            // `is-number` else `Usage: PickupPetAction(index)` (`0x84793c`, `0x4be196`).
            let slot = binding_abi::index_arg32(lua, slot, "Usage: PickupPetAction(index)")?;
            pickup_pet_action(lua, slot);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::cursor::bar::place_action;
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

    /// A drop clears first (`0x4be220`): its grid hide and `CURSOR_UPDATE` read the bar before the
    /// write (`0x4bce00`). The displaced occupant goes through the setter after it (`0x4bce40`):
    /// its inner clear's `CURSOR_UPDATE`, then the set's `PET_BAR_SHOWGRID` (`0x494f28`), and no
    /// `CURSOR_UPDATE` after. The written word is engine state, read here through a test global.
    #[test]
    fn a_drop_signals_its_clear_before_the_bar_write() {
        let s = bar_script();
        let word = s
            .lua
            .create_function(|lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(model.pet_bar.slots[4].view.packed == CLAW)
            })
            .unwrap();
        s.lua.globals().set("SlotFiveIsClaw", word).unwrap();
        s.run("PickupPetAction(4)").unwrap();
        crate::script::tick::heard::listen(
            &s,
            &[
                "PET_BAR_HIDEGRID",
                "CURSOR_UPDATE",
                "PET_BAR_SHOWGRID",
                "PET_BAR_UPDATE",
            ],
            Some("tostring(SlotFiveIsClaw()) .. ' ' .. tostring((GetPetActionInfo(5)))"),
        );
        // Growl is displaced and rides the cursor out, signalling after the write, whose
        // `PET_BAR_UPDATE` (`0x4bcb6d`) reads the slot already painted as Claw.
        s.run("PickupPetAction(5)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "PET_BAR_HIDEGRID false Growl",
                "CURSOR_UPDATE false Growl",
                "PET_BAR_UPDATE true Claw",
                "CURSOR_UPDATE true Claw",
                "PET_BAR_SHOWGRID true Claw"
            ]
        );
        assert!(s.eval::<bool>("return SlotFiveIsClaw()").unwrap());
    }

    #[test]
    fn picking_up_a_spell_blanks_its_slot_and_sends() {
        let mut s = bar_script();
        assert!(pickup_pet_action(&s.lua, 4));
        assert!(
            matches!(s.cursor_payload(), Some(crate::script::CursorPayload::PetAction(c)) if c.src_slot == 4),
            "{:?}",
            s.cursor_payload()
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

    /// `0x4bc9d0`: the write asks the pet's usability predicate first, and the app's answer is the
    /// one the bar last pushed. The pickup still lifts its action (`0x4be25d`), but its blank
    /// (`0x4be27f`) is refused, so the slot keeps the word and nothing is sent; the drop clears the
    /// cursor before it decides (`0x4be220`), and its refused write refills nothing (`0x4bce38`).
    #[test]
    fn an_unusable_bar_writes_nothing_at_either_end() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, false, true, hunter_bar());

        assert!(pickup_pet_action(&s.lua, 4));
        assert!(
            matches!(s.cursor_payload(), Some(crate::script::CursorPayload::PetAction(c))
                if c.src_slot == 4 && c.packed == CLAW),
            "the pickup lifted Claw: {:?}",
            s.cursor_payload()
        );
        assert!(
            s.take_pet_set_actions().is_empty(),
            "but its blank was refused, so nothing is sent"
        );

        s.run("PickupPetAction(5)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "the drop cleared the cursor and nothing refilled it"
        );
        assert!(s.take_pet_set_actions().is_empty(), "and wrote nothing");

        // Slot 4 was never blanked: a second pickup lifts the whole word, autocast bits and all,
        // where a blanked slot would give the type with id 0.
        s.run("PickupPetAction(4)").unwrap();
        assert!(
            matches!(s.cursor_payload(), Some(crate::script::CursorPayload::PetAction(c))
                if c.packed == CLAW),
            "{:?}",
            s.cursor_payload()
        );
        // Growl kept its slot too: the refused drop never displaced it.
        s.run("ClearCursor() PickupPetAction(5)").unwrap();
        assert!(matches!(
            s.cursor_payload(),
            Some(crate::script::CursorPayload::PetAction(c)) if c.packed == GROWL
        ));
        assert!(s.take_pet_set_actions().is_empty());
    }

    /// The same bar made usable takes the same two writes, which is what the test above stops.
    #[test]
    fn a_usable_bar_takes_both_writes() {
        let mut s = bar_script();
        s.run("PickupPetAction(4) PickupPetAction(5)").unwrap();
        assert_eq!(
            s.take_pet_set_actions().len(),
            2,
            "the pickup's blank and the drop"
        );
    }

    #[test]
    fn picking_up_a_token_leaves_the_slot_alone() {
        let mut s = bar_script();
        assert!(pickup_pet_action(&s.lua, 1));
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
        assert!(
            matches!(s.cursor_payload(), Some(crate::script::CursorPayload::PetAction(c)) if c.src_slot == 5),
            "Growl was displaced and is now held, addressed as the slot it came from: {:?}",
            s.cursor_payload()
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
        assert!(s.cursor_payload().is_none());
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
            s.cursor_payload().is_none(),
            "and the cursor is empty — the reference cleared it before it ever decided"
        );
    }

    #[test]
    fn slot_eleven_is_refused_rather_than_written_past_the_array() {
        let mut s = bar_script();
        assert!(!pickup_pet_action(&s.lua, 11));
        assert!(!pickup_pet_action(&s.lua, 0));
        assert!(s.take_pet_set_actions().is_empty());
        assert!(s.cursor_payload().is_none());
    }

    /// The third flag, `pickup_allowed`, is false for a pet that is out of view or possessed.
    #[test]
    fn a_possessed_bar_takes_no_drag_at_all() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, false, hunter_bar());
        assert!(!pickup_pet_action(&s.lua, 4));
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
                use_spell: None,
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
        assert!(!place_action(&s.lua, 1));
        assert!(matches!(
            s.cursor_payload(),
            Some(crate::script::CursorPayload::PetAction(_))
        ));
        assert!(
            s.take_action_sets().is_empty(),
            "and writes nothing to the action bar"
        );
    }

    /// The binding's `ClearCursor(1,1)` (`0x4be220`) drops whatever is held, then the pet-action
    /// setter clears again (`0x494f00`) before the set and its `PET_BAR_SHOWGRID` (`0x494f28`):
    /// two `CURSOR_UPDATE`s on the emptied cursor and none after.
    #[test]
    fn a_pickup_clears_twice_then_shows_the_pet_grid() {
        let mut s = bar_script();
        s.set_cursor_for_test(CursorPayload::Macro(crate::script::cursor::CursorMacro {
            id: 1,
            texture: None,
        }));
        crate::script::cursor::install_kind_probe(&s);
        crate::script::tick::heard::listen(
            &s,
            &[
                "CURSOR_UPDATE",
                "PET_BAR_SHOWGRID",
                "PET_BAR_HIDEGRID",
                "PET_BAR_UPDATE",
            ],
            Some("CursorKind() .. ' ' .. tostring((GetPetActionInfo(4)))"),
        );
        assert!(pickup_pet_action(&s.lua, 4));
        // The spell slot's blank writes through `0x4bc9a0` after the setter (`0x4be27f`).
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "CURSOR_UPDATE none Claw",
                "CURSOR_UPDATE none Claw",
                "PET_BAR_SHOWGRID pet Claw",
                "PET_BAR_UPDATE pet nil"
            ]
        );
    }

    /// A bar whose slot 5 (Growl) allows autocast, for `TogglePetAutocast`.
    fn autocast_bar() -> UiScript {
        let mut bar = hunter_bar();
        bar[4].autocast_allowed = true;
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, true, bar);
        s
    }

    /// `TogglePetAutocast` (`0x4be290`) clears the cursor with `ClearCursor(1,1)` (`0x4be2f8`)
    /// whatever is held; a held pet action is then dropped on the slot (`0x4be309`) and nothing
    /// toggles, where an empty or other payload falls through to the toggle (`0x4bcbb0`).
    #[test]
    fn toggle_autocast_with_a_pet_action_held_drops_it_on_the_slot() {
        let mut s = autocast_bar();
        s.run("PickupPetAction(4)").unwrap();
        let _ = s.take_pet_set_actions();
        s.run("TogglePetAutocast(5)").unwrap();
        assert!(
            s.take_pet_autocast_toggles().is_empty(),
            "the slot was dropped on, not toggled"
        );
        assert_eq!(
            s.eval::<String>("return (GetPetActionInfo(5))").unwrap(),
            "Claw"
        );
        assert_eq!(
            s.take_pet_set_actions(),
            vec![vec![(4, CLAW)]],
            "the drop's bar write was sent"
        );
        // Growl was displaced with nowhere to go and rides the cursor out.
        assert!(matches!(
            s.cursor_payload(),
            Some(CursorPayload::PetAction(c)) if c.packed == GROWL
        ));
    }

    #[test]
    fn toggle_autocast_clears_another_payload_and_still_toggles() {
        let mut s = autocast_bar();
        s.set_cursor_for_test(CursorPayload::Money(crate::script::cursor::CursorMoney {
            copper: 5,
        }));
        s.run("TogglePetAutocast(5)").unwrap();
        assert!(s.cursor_payload().is_none(), "ClearCursor(1,1) ran first");
        assert_eq!(s.take_pet_autocast_toggles(), vec![5]);
        // An empty cursor toggles as before.
        s.run("TogglePetAutocast(5)").unwrap();
        assert_eq!(s.take_pet_autocast_toggles(), vec![5]);
    }

    /// The reference raises on a non-number and past the eleventh slot (`0x4be2a6`, `0x4be2d5`),
    /// before it touches the cursor.
    #[test]
    fn toggle_autocast_raises_before_it_clears() {
        let s = autocast_bar();
        s.run("PickupPetAction(4)").unwrap();
        for (call, text) in [
            ("TogglePetAutocast()", "Usage: TogglePetAutocast(index)"),
            ("TogglePetAutocast(0)", "Invalid slot in TogglePetAutocast"),
            ("TogglePetAutocast(12)", "Invalid slot in TogglePetAutocast"),
        ] {
            let e = s
                .eval::<String>(&format!(
                    "local ok, e = pcall(function() {call} end) return tostring(e)"
                ))
                .unwrap();
            assert!(e.contains(text), "{call}: {e}");
            assert!(
                matches!(s.cursor_payload(), Some(CursorPayload::PetAction(_))),
                "{call} left the cursor alone"
            );
        }
    }
}
