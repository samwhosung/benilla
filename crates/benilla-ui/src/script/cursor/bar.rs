//! The action-bar cursor verbs. The bar is client-authoritative: placing onto an occupied slot
//! hops the displaced action onto the cursor (`PlaceAction`, `0x4e62e0`). `model.actions` is an
//! optimistic mirror of the app's table; each change queues one `CMSG_SET_ACTION_BUTTON`, so a
//! drag-swap is two sends.

use mlua::Lua;

use crate::script::action::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use crate::script::{ActionSlot, Model};

use super::{queue_cursor_update, CursorAction, CursorPayload};

/// The passive-spell refusal's error, errorId `0x9e`: entry `0xb4c0f0` of the table at `0xb4b498`
/// (stride `0x14`) names this key (`0x84133c`).
const PASSIVE_ON_BAR_ERROR: &str = "ERR_PASSIVE_ABILITY";

/// The `CMSG_SET_ACTION_BUTTON` slot word.
fn pack(kind: u8, action: u32) -> u32 {
    (u32::from(kind) << 24) | (action & 0x00FF_FFFF)
}

/// `PickupAction(id)`, the slot's shift-click and drag start (`ActionBarFrame.xml:12-38`). With a
/// payload held it places instead; a pickup also clears the slot on the wire at once. Returns
/// whether to repaint.
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

/// `PlaceAction(id)`, the slot's `OnReceiveDrag` and `UseAction(id, 1)`'s place fork. An occupied
/// slot's action hops onto the cursor with `id` as its source; an item stays in its bag, since a
/// bar item is a reference, not a move. Returns whether to repaint.
pub(crate) fn place_action(model: &mut Model, id: u32) -> bool {
    // The two accept filters (`0x4e62e0`) refuse with a bare return: nothing stored or sent, and
    // the payload stays held. An item needs an on-use spell or an equip slot (`0x4e6571`), so a
    // bag is placeable; a passive spell (`Attributes & 0x40`) also raises errorId `0x9e` through
    // `DisplayError` (`0x4e63ad`, `0x496720`), where the item refusal is mute.
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
        // `PlaceAction` accepts modes 1, 3, 7 and 8 (`0x4e62e0`): a macro, mode 8, packs its id
        // under the macro tag. Pet actions (4), coins (2), stabled pets (10) and vendor rows (5)
        // are refused and stay held.
        CursorPayload::Macro(m) => Some((ACTION_KIND_MACRO, m.index, m.texture.clone())),
        CursorPayload::PetAction(_) => None,
        CursorPayload::StablePet(_) => None,
        CursorPayload::Money(_) => None,
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
            // Count and consumable wait for the app's next-frame re-feed, which knows the item.
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

/// Register `PickupAction` and `PlaceAction` as globals.
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

    #[test]
    fn place_action_onto_occupied_hops_the_displaced_action() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));

        assert!(s.eval::<bool>("return PickupAction(1)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);

        assert!(s.eval::<bool>("return PlaceAction(2)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(2)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
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
