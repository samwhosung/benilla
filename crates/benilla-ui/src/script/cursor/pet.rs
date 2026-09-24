//! The pet bar's drag — **one** Lua verb, `PickupPetAction`, which forks on
//! whether the cursor is already carrying a pet payload. The reference's own bar calls it from all
//! three gestures (`PetActionButton_OnClick`
//! under shift, `OnDragStart`, and `OnReceiveDrag` — `PetActionBarFrame.lua:252-283`), so "pick up"
//! and "drop" are the same binding seen from either side of an empty cursor.
//!
//! **Three things set this surface apart from [`super::bar`]'s**, and each is why this is its own
//! module rather than an arm of that one:
//!
//! - **The payload is a WORD, forwarded verbatim.** `0x4bce00` hands the cursor's dword straight to
//!   the assign core without reading a field of it, and the core's only write is
//!   `bar[target] = *source`. There is no drop-time encoding — see [`CursorPetAction`].
//! - **The bar's contents are the SERVER's.** Nothing but a pet slot can produce this payload and
//!   nothing but a pet slot accepts it, so the drag can only ever *rearrange* ten words the last
//!   `SMSG_PET_SPELLS` delivered. That is the opposite of the action bar, where the 120-slot table
//!   is ours and a drag populates it.
//! - **A displaced occupant is RELOCATED inside the bar before it is handed back.** The action bar
//!   hops the displaced action onto the cursor unconditionally; here the core first tries to keep
//!   it on the bar (a duplicate's slot, or an empty non-token slot), and only a target that was
//!   *not* a token hands its occupant to the cursor.
//!
//! **A drop always empties the cursor first** (`0x495190 ClearCursor(1,1)`, run by all three of
//! `0x4bce00`'s callers *before* the accept decision), and only a write that relocated nothing
//! re-fills it, with the displaced occupant (`0x4bce38 test eax,eax; je`). So every refusal loses
//! the payload — read at the bytes, all six of `0x4bc9a0`'s exits: the no-op on an equal dword
//! (`0x4bc9ed xor eax,eax`), the passive-source reject (`0x4bca28`), the token abort, and both
//! never-blocking front gates all return `EAX = 0`. 1010 shipped the inverse of this as a named
//! divergence — a refused drop kept its payload — and the director retired it (1016): the case it
//! was protecting against is a drop the reference simply does not make, and dropping a pet spell
//! *away* from the bar is how you take it off the bar in the first place.
//!
//! (The **clamp** on slot 11 is still ours, and still deliberate — see [`PET_SLOTS`].)

use mlua::Lua;

use crate::script::Model;

use super::{queue_cursor_update, CursorPayload, CursorPetAction};

/// Pet bar slots — the reference's `NUM_PET_ACTION_SLOTS` and vmangos's
/// `MAX_UNIT_ACTION_BAR_INDEX`, the same 10.
///
/// **The clamp lives here**, and it is a deliberate divergence from the reference's own arithmetic:
/// its callers gate `cmp esi,0xa; jbe`, which admits index 10 (Lua argument 11), and neither
/// `0x4bce00` nor `0x4bc9a0` bounds-checks again — so a real client writes one dword *past* the
/// ten-slot array. Not reproduced.
const PET_SLOTS: u32 = 10;

/// The client's slot type, masked as it masks it (`(packed >> 24) & 0x3F`).
fn kind(packed: u32) -> u8 {
    ((packed >> 24) & 0x3F) as u8
}

/// A **token** slot — a command (7) or a reaction (6). The core treats the two identically
/// everywhere it asks the question: a token occupant is what forces the relocation search, and a
/// token is what disqualifies a slot from being a relocation candidate.
fn is_token(packed: u32) -> bool {
    matches!(kind(packed), 6 | 7)
}

/// Can this word ride the cursor at all? `0x494e20`'s own jump table (`0x494f40`) routes types
/// 1–5 / 6 / 7 and **bails on type 0 or ≥ 8** — so an empty or nonsense word produces no payload,
/// which is what makes "displaced an empty slot" hand back nothing rather than a blank carry.
fn payload_word(packed: u32) -> Option<u32> {
    (1..=7).contains(&kind(packed)).then_some(packed)
}

/// The duplicate scan's comparison key (`0x4bca44`/`0x4bca57`, mask `0x3FFFFFFF`) — everything but
/// the two autocast bits. **Not** type-1-only: the mask covers the whole type+action field for
/// any source type.
fn same_action(a: u32, b: u32) -> bool {
    a & 0x3FFF_FFFF == b & 0x3FFF_FFFF
}

/// The blanked-spell shape `PickupPetAction` writes back into a picked-from slot: type 1 with a
/// zero id. The duplicate scan is skipped for exactly this source (`0x4bca34`–`0x4bca3e`) — it
/// would otherwise match every other blanked slot on the bar.
fn is_blanked_spell(packed: u32) -> bool {
    kind(packed) == 1 && packed & 0xFFFF == 0
}

/// A relocation **candidate** (`0x4bca8b`–`0x4bcab0`): not a token, and its low 16 bits zero. So an
/// all-zero slot qualifies and so does a blanked type-1 slot — not simply "token-or-empty" in
/// either direction.
fn is_relocation_candidate(packed: u32) -> bool {
    !is_token(packed) && packed & 0xFFFF == 0
}

/// What one accepted assignment did.
struct Assigned {
    /// The slot the target's previous occupant was moved to, if it was moved.
    reloc: Option<usize>,
    /// The occupant left with nowhere to go — the cursor's new load. `None` when it was relocated,
    /// and `None` when the target was empty (nothing to carry).
    displaced: Option<u32>,
}

/// The assign core `0x4bc9a0(target, source, sendFlag = 1)` — "set a pet action", and the whole of
/// what a drop does. `None` = nothing was written and nothing should be sent.
///
/// The gates, in the binary's order:
///
/// 1. **no-op** when the source equals the target's current word, compared as the **full unmasked
///    dword** (`0x4bc9e2`) — autocast bits included, so re-dropping a slot on itself is inert.
/// 2. **silent reject** of a passive spell source (`0x4bc9f8`–`0x4bca2e`). No error id, unlike the
///    player bar's `PlaceAction`, which raises `0x9e` for the same test.
/// 3. **duplicate scan**, slots ascending: the first slot that carries the same action (and is not
///    the target) becomes the relocation slot, and the token search is **short-circuited entirely**
///    (`0x4bca78`). This is what makes the ordinary drag a swap.
/// 4. otherwise, if the target's occupant **is a token**, search for a candidate slot to move it
///    to; **no candidate ⇒ abort**, nothing written and nothing sent.
/// 5. write `bar[reloc] = old occupant` (when there is one) and `bar[target] = source`.
///
/// The reference's return value is `EAX = 1` iff **no** relocation happened (`0x4bcb92 setl`) —
/// so an abort and a successful relocation are indistinguishable at the call. That is the one thing
/// a re-implementation must not read as "did the write happen", which is why this returns
/// [`Assigned`] and never a bool.
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
        // No `j != target` guard, and none is needed: this branch only runs when the target's own
        // occupant IS a token, which disqualifies it as a candidate. The bytes have no guard here
        // either.
        reloc = (0..slots.len()).find(|&j| is_relocation_candidate(slots[j]));
        reloc?; // the abort — a token occupant with nowhere to go writes and sends nothing
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

/// `PickupPetAction(slot)` — the pet bar's one drag verb, 1-based (ref
/// `PetActionBarFrame.lua:252-283`; the binding `0x4be180`).
///
/// - **`UNIT_FLAG_POSSESSED` set** ⇒ nothing at all. The gate sits above the cursor fork
///   (`0x4be1c1`), so it blocks the drop as well as the pick-up.
/// - **carrying a pet payload** ⇒ [`place_pet_action`].
/// - **carrying anything else** ⇒ nothing. The reference only enters its drop trampoline when the
///   *pet* payload global is non-empty; a held spell or item is not that global and simply stays
///   held. (Which is the same fact as "a pet bar cannot be populated, only rearranged".)
/// - **empty cursor** ⇒ pick the slot up: the payload is its word verbatim, and — **for a spell
///   slot only** (`0x4be268 cmp cl,1`) — the slot is written back with its id zeroed, through the
///   same assign core and with the same send. A token slot keeps its contents while its word rides
///   the cursor, which is how a command ends up *copied* onto another slot rather than moved.
///
/// Returns whether the caller should repaint.
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
    // A SPELL slot empties as it is picked up — id zeroed, type and autocast bits kept — and the
    // server is told. Everything else keeps its slot.
    //
    // `passive: false` is not a shortcut: the assign core's passive gate reads the record of the
    // **source word's own spell id**, and the blanked word's id is 0, which resolves to no record
    // at all. So a passive spell still empties its slot when picked up — the refusal comes later,
    // when the payload is dropped somewhere.
    if kind(packed) == 1 {
        write_slot(model, index, packed & 0xFFFF_0000, false);
    }
    queue_cursor_update(model);
    true
}

/// The drop half. Runs the assign core against the engine's optimistic mirror of the ten words,
/// queues the resulting `(0-based position, word)` pairs for the app to send as one
/// `CMSG_PET_SET_ACTION`, and disposes of the cursor:
///
/// - **write, occupant homeless** ⇒ the occupant goes onto the cursor (the trampoline's
///   `0x4bce3d`, reached only on the core's `EAX = 1`) — the familiar swap-in-hand.
/// - **write, occupant relocated** ⇒ the cursor empties; the displaced word is still on the bar.
/// - **no write** (self-drop, passive source, or the token abort) ⇒ the cursor empties too, and
///   the payload is gone. `ClearCursor` ran before the decision and `EAX = 0` never re-fills.
///
/// The cursor is therefore emptied on **every** path out of here, including the ones that return
/// `false` — `false` is "nothing was written", not "nothing happened".
fn place_pet_action(model: &mut Model, slot: u32) -> bool {
    let Some(CursorPayload::PetAction(held)) = model.cursor.clone() else {
        return false;
    };
    // `ClearCursor(1,1)` — unconditional, and *before* anything is decided. Past the possessed
    // gate in `PickupPetAction` there is no way back out of here still holding the payload.
    model.cursor = None;
    let Some(index) = slot_index(model, slot) else {
        queue_cursor_update(model);
        return false;
    };
    // The target's own view, read BEFORE the write — a displaced word's passive bit and icon are
    // its slot's, not the payload's we are about to consume.
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

/// The 1-based button index → an in-range slot of a bar that exists.
fn slot_index(model: &Model, slot: u32) -> Option<usize> {
    if !model.pet_bar.has_bar || slot == 0 || slot > PET_SLOTS {
        return None;
    }
    let index = slot as usize - 1;
    (index < model.pet_bar.slots.len()).then_some(index)
}

/// Run [`assign`] against the engine's mirror and queue what changed. Split out because the pickup
/// path needs it too — the reference routes its own slot-blanking through the same core, with the
/// same send (`0x4be268`+).
///
/// The mirror is optimistic in exactly [`super::bar`]'s sense: the app owns the authoritative ten
/// words and re-pushes them, but a drag must read right in the same frame it happens.
fn write_slot(model: &mut Model, target: usize, source: u32, passive: bool) -> Option<Assigned> {
    let mut words: Vec<u32> = model.pet_bar.slots.iter().map(|s| s.view.packed).collect();
    let assigned = assign(&mut words, target, source, passive)?;

    // One send carrying one or two `(position, data)` pairs — the optional relocation pair FIRST,
    // then the mandatory target pair, exactly the order `0x4bcad4`+ builds them and exactly what
    // `CMSG_PET_SET_ACTION` accepts. The relocation pair goes out whenever there was a relocation,
    // without re-testing whether the word changed: that is the binary's own `if (relocSlot >= 0)`.
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

/// A **token** word's `texture` is the NAME of a global, not a path — `GetPetActionInfo 0x4bdc50`'s
/// own convention: the two `char[32]` tables hand back `PET_ATTACK_TEXTURE`, and the bar paints
/// with `getglobal(texture)`. The cursor takes a PATH, so a command or reaction word that reaches
/// the cursor has to carry one, or the drag is invisible — you pick Attack up and nothing sticks
/// to the pointer. Resolved here, the one place a slot's view becomes a cursor payload, exactly as
/// `PetActionBar.xml` resolves it in the one place a slot's view becomes a button.
///
/// A spell word already carries a real `Interface\Icons\…` path and is left alone.
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
            // Both halves can leave a TOKEN on the cursor — the pick-up directly, the drop through
            // its displaced occupant — so the resolution hangs off the binding, not off either arm.
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

    /// The words a hunter's bar actually carries. `FILLER` is vmangos's own unused-slot shape
    /// (`ACT_DISABLED` + spell id 0) — masked type 1 with a zero low word, which is exactly the
    /// relocation candidate the core hunts for, and the reason it must not be flattened to 0.
    const ATTACK: u32 = 0x0700_0002;
    const FOLLOW: u32 = 0x0700_0001;
    const DEFENSIVE: u32 = 0x0600_0001;
    const CLAW: u32 = 0xC100_0BC2;
    const GROWL: u32 = 0x8100_0EC0;
    const BITE: u32 = 0x8100_0EC1;
    const FILLER: u32 = 0x8100_0000;

    /// A source that is **still on the bar** swaps with its destination: the duplicate scan finds
    /// the copy, that slot takes the occupant, and the cursor comes away empty. This is the path a
    /// TOKEN drag always takes, because a token pickup does not blank its source slot.
    #[test]
    fn a_source_still_on_the_bar_swaps_with_its_destination() {
        let mut bar = [CLAW, GROWL, FILLER, ATTACK];
        let a = assign(&mut bar, 1, CLAW, false).unwrap();
        assert_eq!(bar[1], CLAW, "the source word landed");
        assert_eq!(a.reloc, Some(0), "and its own old slot took the occupant");
        assert_eq!(bar[0], GROWL);
        assert_eq!(a.displaced, None, "nothing left over to carry");
    }

    /// A SPELL drag is the other shape, and the pickup's blank is what makes it so: a blanked
    /// source is the one thing the duplicate scan skips (`0x4bca34`), so nothing matches and the
    /// occupant is displaced onto the cursor instead.
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

    /// A TOKEN occupant is never displaced onto the cursor: the core relocates it to the first
    /// candidate (not a token, low 16 zero) and the cursor comes away empty.
    #[test]
    fn a_token_occupant_is_relocated_not_displaced() {
        let mut bar = [CLAW, ATTACK, FILLER, DEFENSIVE];
        let a = assign(&mut bar, 1, BITE, false).unwrap();
        assert_eq!(bar[1], BITE);
        assert_eq!(a.reloc, Some(2), "ATTACK moved to the filler slot");
        assert_eq!(bar[2], ATTACK);
        assert_eq!(a.displaced, None, "nothing is left over to carry");
    }

    /// …and with nowhere to put it, the whole assignment ABORTS: no write, no send. The reference
    /// reports this identically to a successful relocation (`EAX = 0`), which is why the outcome
    /// here is an `Option` and never a bool.
    #[test]
    fn a_token_occupant_with_no_candidate_aborts_entirely() {
        let mut bar = [CLAW, ATTACK, GROWL, DEFENSIVE];
        let before = bar;
        assert!(assign(&mut bar, 1, BITE, false).is_none());
        assert_eq!(bar, before, "nothing written");
    }

    /// The two silent refusals: dropping a word onto the slot it already occupies (compared as the
    /// FULL dword, autocast bits and all), and a passive spell source.
    #[test]
    fn the_self_drop_and_the_passive_source_are_no_ops() {
        let mut bar = [CLAW, GROWL, FILLER, ATTACK];
        assert!(assign(&mut bar, 0, CLAW, false).is_none());
        assert!(assign(&mut bar, 1, CLAW, true).is_none(), "passive source");
        // The autocast bit is part of the compare — the same spell with its ring flipped is a
        // different word and DOES write.
        assert!(assign(&mut bar, 0, CLAW & !0x4000_0000, false).is_some());
    }

    fn view(packed: u32, name: &str) -> PetActionView {
        PetActionView {
            name: (packed != 0).then(|| name.to_string()),
            packed,
            ..Default::default()
        }
    }

    /// A token slot as the feed really pushes it: `name` and `texture` are both the NAMES OF
    /// GLOBALS, which is `GetPetActionInfo`'s own convention for types 6 and 7.
    fn token_view(packed: u32, name: &str, texture: &str) -> PetActionView {
        PetActionView {
            is_token: true,
            texture: Some(texture.to_string()),
            ..view(packed, name)
        }
    }

    /// A hunter's default bar: three commands, four spell slots (two known, two filler), three
    /// reactions — the arrangement `CharmInfo::InitPetActionBar` writes.
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

    /// Picking a SPELL slot up empties it — id zeroed, type and autocast bits kept — and tells the
    /// server in the same breath. The payload is the word verbatim.
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

    /// A dragged TOKEN carries a real icon PATH, not the name of the global that holds one.
    ///
    /// `GetPetActionInfo` hands a token slot `("PET_ACTION_ATTACK", …, "PET_ATTACK_TEXTURE")` —
    /// two global names — and the bar paints with `getglobal`. The cursor cannot: it needs a path
    /// to decode, so an unresolved name means dragging Attack, Follow or Stay sticks *nothing* to
    /// the pointer, which is what the director saw. A spell slot's path is left exactly as it is.
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

        // An unset global resolves to nothing rather than to its own name — better a missing icon
        // than a path that cannot exist.
        s.run("ClearCursor() PET_ATTACK_TEXTURE = nil PickupPetAction(1)")
            .unwrap();
        let Some(CursorPayload::PetAction(p)) = s.cursor_payload() else {
            panic!("still picked up")
        };
        assert_eq!(p.texture, None);
    }

    /// A TOKEN slot keeps its contents while its word rides the cursor (`0x4be268 cmp cl,1` — the
    /// blank is type-1-only), so a command ends up COPIED onto its destination rather than moved.
    #[test]
    fn picking_up_a_token_leaves_the_slot_alone() {
        let mut s = bar_script();
        assert!(s.eval::<bool>("return PickupPetAction(1)").unwrap());
        assert!(
            s.take_pet_set_actions().is_empty(),
            "nothing was written, so nothing is sent"
        );
    }

    /// The whole gesture, end to end: pick a spell up and drop it two slots along. The pickup's
    /// blank and the drop's write are two separate sends, and the displaced spell comes back on
    /// the cursor.
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

    /// Dropping onto a COMMAND relocates that command to a filler slot instead of handing it back
    /// — the cursor comes away empty, and ONE send carries both pairs (the server tells the two
    /// forms apart by body size, so they must not be split).
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

    /// A refused drop **eats** the payload — `ClearCursor` ran before the decision and `EAX = 0`
    /// never re-fills. 1010 shipped the inverse as a named divergence; 1016 retired it.
    ///
    /// The reachable refusal is the passive-spell source: the pickup has no passive gate, so a
    /// passive slot picks up fine and is then refused everywhere it is dropped. Losing it is not
    /// the data loss it looks like — the pickup's blank already went to the server, which is
    /// exactly how a pet spell is taken off the bar.
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

    /// The clamp. The reference's callers admit Lua argument 11 and its assign core never bounds
    /// again, so a real client writes one dword past the array; we refuse instead.
    #[test]
    fn slot_eleven_is_refused_rather_than_written_past_the_array() {
        let mut s = bar_script();
        assert!(!s.eval::<bool>("return PickupPetAction(11)").unwrap());
        assert!(!s.eval::<bool>("return PickupPetAction(0)").unwrap());
        assert!(s.take_pet_set_actions().is_empty());
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    /// `UNIT_FLAG_POSSESSED` blocks BOTH ends, because the reference's gate sits above the cursor
    /// fork: you cannot pick a possessed unit's slot up, and you cannot drop onto its bar either.
    #[test]
    fn a_possessed_bar_takes_no_drag_at_all() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, false, hunter_bar());
        assert!(!s.eval::<bool>("return PickupPetAction(4)").unwrap());
        assert!(s.take_pet_set_actions().is_empty());
    }

    /// The two payload spaces do not overlap in either direction, and the two grids follow them:
    /// a held spell lights the ACTION bar's grid and cannot land on a pet slot; a held pet action
    /// lights the PET bar's and cannot land on an action slot.
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
