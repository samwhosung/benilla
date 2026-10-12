//! The action-bar cursor verbs. The bar is client-authoritative: placing onto an occupied slot
//! hops the displaced action onto the cursor (`PlaceAction`, `0x4e62e0`). `model.actions` is an
//! optimistic mirror of the app's table; each change queues one `CMSG_SET_ACTION_BUTTON`, so a
//! drag-swap is two sends.

use mlua::Lua;

use crate::script::action::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use crate::script::tick::verb;
use crate::script::{ActionSlot, Model};

use super::{clear_cursor, set_cursor, CursorAction, CursorPayload};

/// The passive-spell refusal's error, errorId `0x9e`: entry `0xb4c0f0` of the table at `0xb4b498`
/// (stride `0x14`) names this key (`0x84133c`).
const PASSIVE_ON_BAR_ERROR: &str = "ERR_PASSIVE_ABILITY";

/// The `CMSG_SET_ACTION_BUTTON` slot word.
fn pack(kind: u8, action: u32) -> u32 {
    (u32::from(kind) << 24) | (action & 0x00FF_FFFF)
}

/// `PickupAction(id)`, the slot's shift-click and drag start (`ActionBarFrame.xml:12-38`). With a
/// payload held it places instead; a pickup also clears the slot on the wire at once. The cursor
/// goes first and the slot after, as `0x4e6130` runs its setter before the slot clear `0x4e5db0`,
/// so the pickup's events still read the action in its slot. The setter is the one for the
/// slot's kind ([`super::put`]): its inner `ClearCursor(1,1)` signals on the empty cursor, then
/// the grid shows. Returns whether to repaint.
pub(super) fn pickup_action(lua: &Lua, id: u32) -> bool {
    let picked = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if model.cursor.is_some() {
            None
        } else {
            // An empty slot returns before any setter (`0x4e61a3`).
            Some(model.actions.get(&id).map(|slot| CursorAction {
                src_slot: id,
                kind: slot.kind,
                action: slot.action,
                texture: slot.texture.clone(),
            }))
        }
    };
    let Some(picked) = picked else {
        return place_action(lua, id);
    };
    let Some(payload) = picked else {
        return false;
    };
    set_cursor(lua, true, |_| Some(CursorPayload::Action(payload)));
    verb(lua, |model| {
        model.actions.remove(&id);
        model.action_sets.push((id, 0));
    });
    true
}

/// `PlaceAction(id)`, the slot's `OnReceiveDrag` and `UseAction(id, 1)`'s place fork. An occupied
/// slot's action hops onto the cursor with `id` as its source, through its setter, whose inner
/// `ClearCursor(1,1)` drops the held payload, hiding the grid, before the setter shows it again
/// (`0x4e6450`, `0x4e6494`, `0x4e64e2`); an empty slot's place is that clear alone (`0x4e650f`).
/// An item stays in its bag, since a bar item is a reference, not a move. The cursor goes first
/// and the slot after, as `0x4e62e0` writes the slot past the setter, so the cursor's events read
/// the slot as it was. Returns whether to repaint.
pub(crate) fn place_action(lua: &Lua, id: u32) -> bool {
    let Some((placed, displaced)) = verb(lua, |model| plan_place(model, id)) else {
        return false;
    };
    let (kind, action, texture) = placed;
    match displaced {
        Some(displaced) => {
            set_cursor(lua, true, |_| Some(CursorPayload::Action(displaced)));
        }
        None => verb(lua, clear_cursor),
    }
    verb(lua, |model| {
        model.actions.insert(
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
    });
    true
}

/// What the slot takes and the action it displaces.
type PlacePlan = ((u8, u32, Option<String>), Option<CursorAction>);

/// `PlaceAction`'s decision: the accept filters, then what the slot takes from the held payload
/// and the occupant that hops. `None` is a refusal that leaves the payload held.
fn plan_place(model: &mut Model, id: u32) -> Option<PlacePlan> {
    // The two accept filters (`0x4e62e0`) refuse with a bare return: nothing stored or sent, and
    // the payload stays held. An item needs an on-use spell or an equip slot (`0x4e6571`), so a
    // bag is placeable; a passive spell (`Attributes & 0x40`) also raises errorId `0x9e` through
    // `DisplayError` (`0x4e63ad`, `0x496720`), where the item refusal is mute.
    match &model.cursor {
        Some(CursorPayload::Item(i)) if !i.bar_placeable => return None,
        Some(CursorPayload::Spell(s)) if s.passive => {
            model.ui_errors.push(PASSIVE_ON_BAR_ERROR);
            return None;
        }
        _ => {}
    }
    let placed = match model.cursor.as_ref()? {
        CursorPayload::Action(a) => (a.kind, a.action, a.texture.clone()),
        CursorPayload::Item(i) => (ACTION_KIND_ITEM, i.item_id, i.texture.clone()),
        CursorPayload::Spell(s) => (ACTION_KIND_SPELL, s.spell_id, s.texture.clone()),
        // `PlaceAction` accepts modes 1, 3, 7 and 8 (`0x4e62e0`): a macro, mode 8, packs its id
        // under the macro tag. Pet actions (4), coins (2), stabled pets (10) and vendor rows (5)
        // are refused and stay held.
        CursorPayload::Macro(m) => (ACTION_KIND_MACRO, m.index, m.texture.clone()),
        CursorPayload::PetAction(_)
        | CursorPayload::StablePet(_)
        | CursorPayload::Money(_)
        | CursorPayload::Merchant(_) => return None,
    };
    let displaced = model.actions.get(&id).map(|d| CursorAction {
        src_slot: id,
        kind: d.kind,
        action: d.action,
        texture: d.texture.clone(),
    });
    Some((placed, displaced))
}

/// Register `PickupAction` and `PlaceAction` as globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "PickupAction",
        lua.create_function(|lua, id: u32| {
            pickup_action(lua, id);
            Ok(())
        })?,
    )?;
    g.set(
        "PlaceAction",
        lua.create_function(|lua, id: u32| {
            place_action(lua, id);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{pickup_action, place_action};
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

        assert!(pickup_action(&s.lua, 1));
        assert!(
            !s.eval::<bool>("return HasAction(1)").unwrap(),
            "removed from the engine's optimistic mirror"
        );
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Action(c)) if c.src_slot == 1),
            "{:?}",
            s.cursor_payload()
        );
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);
    }

    /// The cursor's events run before the slot write, as `0x4e6130` runs its setter before the
    /// slot clear `0x4e5db0` and `0x4e62e0` hops the displaced action before it writes: a handler
    /// reads each slot as it was.
    #[test]
    fn the_cursor_events_fire_before_the_slot_write() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\Spell_B", 0x00, 222)));
        crate::script::tick::heard::listen(
            &s,
            &["CURSOR_UPDATE"],
            Some("tostring(GetActionTexture(1)) .. ' ' .. tostring(GetActionTexture(2))"),
        );
        s.run("PickupAction(1)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["CURSOR_UPDATE Interface\\Icons\\Spell_A Interface\\Icons\\Spell_B"]
        );
        assert!(!s.eval::<bool>("return HasAction(1)").unwrap());
        s.run("PlaceAction(2)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["CURSOR_UPDATE nil Interface\\Icons\\Spell_B"]
        );
        assert_eq!(
            s.eval::<String>("return GetActionTexture(2)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
    }

    #[test]
    fn pickup_action_on_an_empty_slot_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        assert!(!pickup_action(&s.lua, 5));
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

        assert!(place_action(&s.lua, 7));
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

        assert!(pickup_action(&s.lua, 1));
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);

        assert!(place_action(&s.lua, 2));
        assert_eq!(
            s.eval::<String>("return GetActionTexture(2)").unwrap(),
            "Interface\\Icons\\Spell_A"
        );
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

        assert!(place_action(&s.lua, 4));
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

        assert!(place_action(&s.lua, 9));
        assert_eq!(s.take_action_sets(), vec![(9, 133)]); // 0x00<<24 | 133
    }

    #[test]
    fn place_action_with_an_empty_cursor_is_a_no_op() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
        assert!(!place_action(&s.lua, 1));
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
        assert!(pickup_action(&s.lua, 5));
        assert!(s.cursor_payload().is_none(), "placed onto the empty slot 5");
        assert!(s.eval::<bool>("return HasAction(5)").unwrap());
        assert_eq!(s.take_action_sets(), vec![(5, 111)]);
    }

    #[test]
    /// The setters show the grid (`0x494dd1`) and every clear's tail hides it (`0x4952b3`), so a
    /// hop's inner clear hides it before the displaced action's setter shows it again.
    fn showgrid_hidegrid_fire_on_gain_and_loss_and_twice_on_a_hop() {
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

        s.run("PlaceAction(2)").unwrap(); // Some -> Some (hop): HIDE, then SHOW
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 1);

        s.run("ClearCursor()").unwrap(); // Some -> None: HIDE
        s.tick(0.01);
        assert_eq!(s.eval::<i64>("return shows").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return hides").unwrap(), 2);
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

        assert!(!place_action(&s.lua, 4));
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

        assert!(!place_action(&s.lua, 4));
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

        assert!(place_action(&s.lua, 4));
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

        assert!(place_action(&s.lua, 4));
        assert_eq!(s.take_action_sets(), vec![(4, 0x8000_0000u32 | 4496)]);
    }

    /// A bar pickup runs the setter of its slot's kind (`0x4e6130`): a spell's shows the grid and
    /// signals nothing after (`0x494dd1`); an item's row grab shows it and signals after its set
    /// (`0x49513a`, `0x49515e`). Each inner clear signals first, so a hop hides the grid and
    /// `CURSOR_UPDATE` reads the emptied cursor before the displaced action goes on.
    #[test]
    fn a_bar_pickup_and_a_hop_signal_as_their_setters() {
        let s = {
            let mut s = UiScript::new().unwrap();
            s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 111)));
            s.set_action(2, Some(action_slot("Interface\\Icons\\INV_A", 0x80, 4540)));
            s
        };
        crate::script::cursor::install_kind_probe(&s);
        crate::script::tick::heard::listen(
            &s,
            &["CURSOR_UPDATE", "ACTIONBAR_SHOWGRID", "ACTIONBAR_HIDEGRID"],
            Some("CursorKind()"),
        );
        s.run("PickupAction(1)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["CURSOR_UPDATE none", "ACTIONBAR_SHOWGRID action"]
        );
        s.run("PlaceAction(2)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "ACTIONBAR_HIDEGRID none",
                "CURSOR_UPDATE none",
                "ACTIONBAR_SHOWGRID action",
                "CURSOR_UPDATE action"
            ]
        );
        // Onto an empty slot the place is the clear alone (`0x4e650f`).
        s.run("PlaceAction(3)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["ACTIONBAR_HIDEGRID none", "CURSOR_UPDATE none"]
        );
    }
}
