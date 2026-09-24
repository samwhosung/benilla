//! The action bar (decision 0216 §7, slice 4; byte-verified 0218 §2/§4): `PickupAction`/
//! `PlaceAction` join the ONE payload space as the [`CursorAction`] arm. Two things set this
//! surface apart from bags/doll:
//!
//! - **The bar is client-authoritative and actions HOP** (0218 §4, `PlaceAction 0x4e62e0`) — the
//!   opposite of the post-0218 item swap: placing onto an occupied slot puts the DISPLACED action
//!   on the cursor rather than clearing, because there is no server round-trip to wait on (the
//!   120-slot table is ours; [`place_action`] IS the mutation, not a request for one).
//! - **The engine owns the payload + transition, the app owns the 120-table** (0216 §7's
//!   ownership split): `model.actions` here is the engine's own OPTIMISTIC mirror of the app's
//!   authoritative store, kept only so `HasAction`/`GetActionTexture`/&c. read right the instant a
//!   local pickup/place happens, without waiting a frame for the app to re-feed. Every mutation
//!   also queues `(lua id, packed)` onto [`Model::action_sets`] — the wire intent the app drains
//!   into `CMSG_SET_ACTION_BUTTON`, one send per queued entry (0218 §4: a drag-swap is two sends,
//!   never atomic — this module never coalesces them).

use mlua::Lua;

use crate::script::action::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use crate::script::{ActionSlot, Model};

use super::{queue_cursor_update, CursorAction, CursorPayload};

/// The GlobalStrings key the reference's passive-spell refusal shows — errorId `0x9e`'s entry in
/// the errorId→key table (`0xb4b498 + 0x9e*0x14 = 0xb4c0f0` ← key ptr `0x84133c`), i.e.
/// `ERR_PASSIVE_ABILITY` = "You can't put a passive ability in the action bar.". Queued onto
/// `Model::ui_errors` for the app's action feed to resolve and fire.
const PASSIVE_ON_BAR_ERROR: &str = "ERR_PASSIVE_ABILITY";

/// Pack `(kind, action)` into the wire's `u32` slot word (`kind<<24 | action`, decision 0216 §1).
fn pack(kind: u8, action: u32) -> u32 {
    (u32::from(kind) << 24) | (action & 0x00FF_FFFF)
}

/// `PickupAction(id)` — the action-bar slot's shift-click/drag-start entry point (ref
/// `ActionBarFrame.xml:12-38`'s `IsShiftKeyDown()` fork, `OnDragStart`).
///
/// - **A payload is already held** → falls through to [`place_action`] (the reference's own
///   contract: a shift-click while carrying just places, `ActionButtonTemplate`'s `OnClick` never
///   special-cases it).
/// - **Empty cursor, an occupied slot** → picks it up: payload `Action { src_slot: id, kind,
///   action, texture }`, the slot removed from `model.actions` (optimistic — the app re-pushes an
///   agreeing snapshot once `action_sets` lands), and `action_sets.push((id, 0))` queued — picking
///   an action OFF the bar empties it immediately on the wire too (the classic drag-off).
/// - **Empty cursor, an empty slot** → no-op (nothing to pick up).
///
/// Returns whether the caller should repaint (mirrors the container/doll pickup contract).
pub(super) fn pickup_action(model: &mut Model, id: u32) -> bool {
    if model.cursor.is_some() {
        return place_action(model, id);
    }
    let Some(slot) = model.actions.get(&id) else {
        return false;
    };
    let payload = CursorAction {
        src_slot: id,
        kind: slot.kind,
        action: slot.action,
        texture: slot.texture.clone(),
    };
    model.actions.remove(&id);
    model.cursor = Some(CursorPayload::Action(payload));
    model.action_sets.push((id, 0));
    queue_cursor_update(model);
    true
}

/// `PlaceAction(id)` — the action-bar slot's click-with-payload/`OnReceiveDrag` entry point (ref
/// `UseAction(id, checkCursor=1)`'s place fork, `ActionBarFrame.xml`'s `OnReceiveDrag`).
///
/// Every arm writes the held action into `model.actions[id]` optimistically and queues
/// `action_sets.push((id, packed))`; what happens to the cursor afterward is the byte-verified
/// divergence from every other surface (0218 §4): an OCCUPIED destination puts the DISPLACED
/// action on the cursor (referencing `id` as its new `src_slot` — the slot it can now be placed
/// FROM), an empty destination just clears.
///
/// - Payload **Action** → `(kind, action)` straight off the held payload.
/// - Payload **Item** → `packed = item_id | ITEM<<24`; the item came from a BAG and STAYS there —
///   a bar item action is a reference, not a move, so no container move is queued here (the app's
///   `drain_action_sets` never touches `container_moves` either).
/// - Payload **Spell** → `packed = spell_id | SPELL<<24` (the producer, `PickupSpell`, lands in
///   slice 5 — this arm already works once something populates the payload).
/// - **Empty cursor** → no-op.
///
/// Returns whether the caller should repaint.
pub(crate) fn place_action(model: &mut Model, id: u32) -> bool {
    // The two accept filters, byte-read (`PlaceAction 0x4e62e0` — decision 0666). Both
    // reject with a **bare return**: no store, no clear, no packet — mechanically identical to
    // clicking with an empty cursor, and the refused payload STAYS on the cursor.
    //
    // - ITEM: placeable iff it has an on-use spell OR is equippable (`4e6571`–`4e6598`). There is
    //   no quality, bind, class/subclass, container or level test anywhere on that path — a BAG
    //   is placeable; a grey trade good with neither is silently refused.
    // - SPELL: a passive (`Attributes & 0x40`) is refused (`4e63ad`), and — unlike the ITEM
    //   refusal, which is mute — it ALSO raises errorId `0x9e` through `CGGameUI::DisplayError`
    //   (`4e63ad: push 0x9e; call 0x496720`). That id resolves through the errorId→key table at
    //   base `0xb4b498`, stride `0x14`: entry `0xb4b498 + 0x9e*0x14 = 0xb4c0f0`, whose static-init
    //   `4861ff: mov [0xb4c0f0], 0x84133c` names [`PASSIVE_ON_BAR_ERROR`] — "You can't put a
    //   passive ability in the action bar." (The same arithmetic reproduces the independently
    //   recorded `ERR_ATTACK_MOUNTED` anchor: `0xa4` → `0xb4c168`.) Closes 0666's named
    //   divergence, which refused silently while the key was unpinned.
    match &model.cursor {
        Some(CursorPayload::Item(i)) if !i.bar_placeable => return false,
        Some(CursorPayload::Spell(s)) if s.passive => {
            model.ui_errors.push(PASSIVE_ON_BAR_ERROR);
            return false;
        }
        _ => {}
    }
    let Some(held) = model.cursor.take() else {
        return false;
    };
    let placeable = match &held {
        CursorPayload::Action(a) => Some((a.kind, a.action, a.texture.clone())),
        CursorPayload::Item(i) => Some((ACTION_KIND_ITEM, i.item_id, i.texture.clone())),
        CursorPayload::Spell(s) => Some((ACTION_KIND_SPELL, s.spell_id, s.texture.clone())),
        // Mode 8 — the one non-item/non-spell payload the reference's `PlaceAction` accepts
        // (`0x4e62e0`'s accept table: pet actions and class abilities are refused,
        // macros are not). It packs the bare macro id under the MACRO tag, exactly as the SPELL
        // and ITEM arms pack theirs.
        CursorPayload::Macro(m) => Some((ACTION_KIND_MACRO, m.index, m.texture.clone())),
        // Mode 4 — the other half of that same table's refusal (decision 1010). A pet action has
        // no `CMSG_SET_ACTION_BUTTON` encoding at all, so there is nothing to pack: the payload
        // goes straight back on the cursor, where the pet bar can still take it.
        CursorPayload::PetAction(_) => None,
        // Mode 10 (decision 1677) — a stabled pet has no `CMSG_SET_ACTION_BUTTON` encoding either,
        // so the action bar refuses it and it goes back on the cursor.
        CursorPayload::StablePet(_) => None,
        // Mode 2 (1962) — `PlaceAction`'s table takes modes 1, 2 and 8 … where its mode 2 is the
        // BAR-slot pickup, not coins; the money payload has no encoding and goes back.
        CursorPayload::Money(_) => None,
        // Mode 5 — a vendor row is not a bar action. `PlaceAction`'s payload table takes modes
        // 1, 2 and 8 only, and the merchant grab is none of them, so it goes back on the cursor
        // where the vendor window can still take it.
        CursorPayload::Merchant(_) => None,
    };
    let Some((kind, action, texture)) = placeable else {
        model.cursor = Some(held);
        return false;
    };
    let displaced = model.actions.insert(
        id,
        ActionSlot {
            texture,
            kind,
            action,
            // The engine holds no item knowledge, so neither half of the Count pair can be
            // answered here: the app's next-frame re-feed resolves the real bag count and the
            // `IsConsumableAction` gate together, off the item template (decision 1301).
            count: 0,
            consumable: false,
        },
    );
    model.action_sets.push((id, pack(kind, action)));
    model.cursor = displaced.map(|d| {
        CursorPayload::Action(CursorAction {
            src_slot: id,
            kind: d.kind,
            action: d.action,
            texture: d.texture,
        })
    });
    queue_cursor_update(model);
    true
}

/// Register the action-bar's cursor globals — top-level, matching the reference
/// (`PickupAction`/`PlaceAction` are not namespaced any more than `PickupContainerItem`'s cursor
/// siblings are).
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupAction",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(pickup_action(&mut model, id))
        })?,
    )?;
    g.set(
        "PlaceAction",
        lua.create_function(|lua, id: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(place_action(&mut model, id))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::cursor::{CursorAction, CursorItem, CursorPayload, CursorSpell};
    use crate::script::{ActionSlot, UiScript};

    fn action_slot(texture: &str, kind: u8, action: u32) -> ActionSlot {
        ActionSlot {
            texture: Some(texture.into()),
            kind,
            action,
            count: 0,
            consumable: false,
        }
    }

    #[test]
    fn pickup_action_empties_the_slot_and_queues_a_clear() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 133)));

        assert!(s.eval::<bool>("return PickupAction(1)").unwrap());
        assert!(
            !s.eval::<bool>("return HasAction(1)").unwrap(),
            "removed from the engine's optimistic mirror"
        );
        let (kind, id) = s
            .eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
            .unwrap();
        assert_eq!((kind.as_str(), id), ("action", 1));
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);
    }

    #[test]
    fn pickup_action_on_an_empty_slot_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return PickupAction(5)").unwrap());
        assert!(s.cursor_payload().is_none());
        assert!(s.take_action_sets().is_empty());
    }

    #[test]
    fn place_action_onto_empty_writes_the_slot_and_clears_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 3,
            kind: 0x00,
            action: 133,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));

        assert!(s.eval::<bool>("return PlaceAction(7)").unwrap());
        assert!(s.cursor_payload().is_none(), "empty destination clears");
        assert!(s.eval::<bool>("return HasAction(7)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(7)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
        // 0x00<<24 | 133 == 133.
        assert_eq!(s.take_action_sets(), vec![(7, 133)]);
    }

    /// The byte-verified divergence from bags/doll (0218 §4): placing onto an OCCUPIED action
    /// slot HOPS the displaced action onto the cursor — two `action_sets` entries across the
    /// gesture (the pickup's clear, then the place's write), never a container move.
    #[test]
    fn place_action_onto_occupied_hops_the_displaced_action() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));

        assert!(s.eval::<bool>("return PickupAction(1)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);

        assert!(s.eval::<bool>("return PlaceAction(2)").unwrap());
        // Slot 2 now shows the placed action (111).
        assert_eq!(
            s.eval::<String>("return GetActionTexture(2)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
        // The displaced action (222) is now the held payload, sourced from slot 2.
        let (kind, src) = s
            .eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
            .unwrap();
        assert_eq!((kind.as_str(), src), ("action", 2));
        assert_eq!(
            s.cursor_payload(),
            Some(CursorPayload::Action(CursorAction {
                src_slot: 2,
                kind: 0x00,
                action: 222,
                texture: Some("Interface\\Icons\\Spell_B".into()),
            }))
        );
        assert_eq!(s.take_action_sets(), vec![(2, 111)]);

        // A hop is Some→Some: no HIDE+SHOW churn out of one gesture.
        // (Exercised directly by the SHOWGRID/HIDEGRID test below.)
    }

    #[test]
    fn place_action_item_payload_packs_the_item_kind_and_leaves_the_bag_untouched() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(s.cursor_payload().is_none());
        // 0x80<<24 | 117.
        assert_eq!(s.take_action_sets(), vec![(4, 0x8000_0000 | 117)]);
        assert!(
            s.take_container_moves().is_empty(),
            "a bar item action is a reference, not a move"
        );
    }

    #[test]
    fn place_action_spell_payload_packs_the_spell_kind() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));

        assert!(s.eval::<bool>("return PlaceAction(9)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(9, 133)]); // 0x00<<24 | 133
    }

    #[test]
    fn place_action_with_an_empty_cursor_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        assert!(!s.eval::<bool>("return PlaceAction(1)").unwrap());
        assert!(s.eval::<bool>("return HasAction(1)").unwrap(), "untouched");
        assert!(s.take_action_sets().is_empty());
    }

    /// Shift-click while ALREADY holding just places (the reference's own `OnClick` never
    /// special-cases it): `PickupAction` on a held cursor routes straight to [`super::place_action`].
    #[test]
    fn pickup_action_with_a_payload_held_falls_through_to_place() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x00,
            action: 111,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));
        assert!(s.eval::<bool>("return PickupAction(5)").unwrap());
        assert!(s.cursor_payload().is_none(), "placed onto the empty slot 5");
        assert!(s.eval::<bool>("return HasAction(5)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(5, 111)]);
    }

    /// `ACTIONBAR_SHOWGRID`/`ACTIONBAR_HIDEGRID` fire on the cursor's None↔Some edges (decision
    /// 0216 §7) — a pickup shows the grid, a place-onto-empty hides it, and a HOP (Some→Some)
    /// fires neither (no HIDE+SHOW churn out of one gesture).
    #[test]
    fn showgrid_hidegrid_fire_on_gain_and_loss_not_on_a_hop() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));
        s.run(
            r#"
            shows, hides = 0, 0
            local f = CreateFrame("Frame", "GridListener")
            f:RegisterEvent("ACTIONBAR_SHOWGRID")
            f:RegisterEvent("ACTIONBAR_HIDEGRID")
            f:SetScript("OnEvent", function()
                if event == "ACTIONBAR_SHOWGRID" then shows = shows + 1 end
                if event == "ACTIONBAR_HIDEGRID" then hides = hides + 1 end
            end)
            "#,
        )
        .unwrap();

        s.run("PickupAction(1)").unwrap(); // None -> Some: SHOW
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 0);

        s.run("PlaceAction(2)").unwrap(); // Some -> Some (hop): neither
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 0);

        s.run("ClearCursor()").unwrap(); // Some -> None: HIDE
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 1);
    }

    /// `PlaceAction`'s ITEM filter (decision 0666): an item with neither an on-use spell nor an
    /// equip slot — a grey trade good — is refused, and the refusal is a **bare return**: nothing
    /// stored, nothing sent, and the payload is still on the cursor afterwards.
    #[test]
    fn a_non_usable_non_equippable_item_is_refused_and_stays_held() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: false, // no on-use spell, InventoryType 0
            bag: 0,
            slot: 1,
            item_id: 2589, // Linen Cloth
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: Vec::new(),
        }));

        assert!(!s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(!s.eval::<bool>("return HasAction(4)").unwrap(), "no store");
        assert!(s.take_action_sets().is_empty(), "no packet");
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Item(_))),
            "a refused payload stays on the cursor — the reference never clears it"
        );
        assert!(
            s.take_ui_errors().is_empty(),
            "the ITEM refusal is MUTE — only the SPELL arm carries a DisplayError (`4e63ad`); a \
             toast here would be ours, not the reference's"
        );
    }

    /// The SPELL twin: a passive cannot go on the bar (`Attributes & 0x40`) — and unlike the item
    /// arm this one SPEAKS, raising errorId `0x9e` = `ERR_PASSIVE_ABILITY` (see
    /// [`super::place_action`]). The refusal and the toast are separate halves: the refusal is
    /// still a bare return, so nothing is stored, nothing is sent, and the spell stays held.
    #[test]
    fn a_passive_spell_is_refused_with_the_refs_error_and_stays_held() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 674, // Dual Wield, a passive
            texture: Some("Interface\\Icons\\Ability_DualWield".into()),
            passive: true,
        }));

        assert!(!s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(!s.eval::<bool>("return HasAction(4)").unwrap());
        assert!(s.take_action_sets().is_empty());
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))));
        assert_eq!(
            s.take_ui_errors(),
            vec![super::PASSIVE_ON_BAR_ERROR],
            "the ref's errorId 0x9e toast — 'You can't put a passive ability in the action bar.'"
        );
        assert!(
            s.take_ui_errors().is_empty(),
            "…and the queue drains, so the toast fires once per refusal"
        );
    }

    /// The other half of the same law: an ACTIVE spell places normally and raises nothing. Guards
    /// the obvious over-correction — a toast on every spell drop.
    #[test]
    fn an_active_spell_places_without_an_error() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133, // Fireball
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            passive: false,
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert!(s.eval::<bool>("return HasAction(4)").unwrap());
        assert!(s.take_ui_errors().is_empty());
    }

    /// The positive control on the other side of both filters — a BAG (`InventoryType` 18, no
    /// on-use spell) IS placeable, which is the one answer people find surprising.
    #[test]
    fn a_bag_is_placeable() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true, // equippable (BAG), even with no on-use spell
            bag: 0,
            slot: 1,
            item_id: 4496, // Small Brown Pouch
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            link: None,
            count: None,
            quality: Some(1),
            equip_slots: vec![20, 21, 22, 23],
        }));

        assert!(s.eval::<bool>("return PlaceAction(4)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(4, 0x8000_0000u32 | 4496)]);
    }
}
