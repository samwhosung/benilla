//! The action-bar cursor verbs. The bar is client-authoritative: placing onto an occupied slot
//! hops the displaced action onto the cursor (`PlaceAction`, `0x4e62e0`). `model.actions` is an
//! optimistic mirror of the app's table; each change queues one `CMSG_SET_ACTION_BUTTON`, so a
//! drag-swap is two sends.

use mlua::Lua;

use crate::script::action::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use crate::script::tick::verb;
use crate::script::{ActionSlot, Model};

use super::{clear_cursor, set_cursor, CursorAction, CursorMacro, CursorPayload, CursorSpell};

/// The passive-spell refusal's error, errorId `0x9e`: entry `0xb4c0f0` of the table at `0xb4b498`
/// (stride `0x14`) names this key (`0x84133c`).
const PASSIVE_ON_BAR_ERROR: &str = "ERR_PASSIVE_ABILITY";

/// The `CMSG_SET_ACTION_BUTTON` slot word.
fn pack(kind: u8, action: u32) -> u32 {
    (u32::from(kind) << 24) | (action & 0x00FF_FFFF)
}

/// The payload slot `id`'s setter puts on the cursor, the setter of the action's kind
/// (`0x4e6130`, and `0x4e62e0`'s hop): an item action the row grab's mode 7 (`0x4e6215`,
/// `0x4e6450`), a spell the spell setter's mode 3 (`0x4e6254`, `0x4e6494`), a macro the macro
/// setter's mode 8 (`0x4e629d`, `0x4e64e2`). `None` is an empty slot; `Some(None)` an action no
/// setter takes, a macro gone from the list (`0x4e6279`) or an unknown kind (`0x4e626e`): a
/// pickup then sets nothing, and a place clears the cursor as for an empty slot.
fn lift(model: &Model, id: u32) -> Option<Option<CursorPayload>> {
    let slot = model.actions.get(&id)?;
    Some(match slot.kind {
        ACTION_KIND_ITEM => Some(CursorPayload::Action(CursorAction {
            src_slot: id,
            kind: slot.kind,
            action: slot.action,
            texture: slot.texture.clone(),
        })),
        ACTION_KIND_SPELL => Some(CursorPayload::Spell(CursorSpell {
            spell_id: slot.action,
            texture: slot.texture.clone(),
            // The setter reads the spell's record; the book is where the engine has it.
            passive: model
                .spellbook
                .slots
                .iter()
                .any(|s| s.spell_id == slot.action && s.passive),
        })),
        ACTION_KIND_MACRO => model.macros.get(slot.action as usize).map(|m| {
            CursorPayload::Macro(CursorMacro {
                index: slot.action,
                texture: m.texture.clone(),
            })
        }),
        _ => None,
    })
}

/// `PickupAction(id)`, the slot's shift-click and drag start (`ActionBarFrame.xml:12-38`). With a
/// spell, item, macro or bar item held it places instead (`0x4e615d`-`0x4e6194`); with nothing
/// or anything else held it picks up, the setter's clear dropping coins, a pet action, a stabled
/// pet or a vendor row. A pickup also clears the slot on the wire at once. The cursor
/// goes first and the slot after, as `0x4e6130` runs its setter ([`lift`]) before the slot clear
/// `0x4e5db0`, so the pickup's events still read the action in its slot: the setter's inner
/// `ClearCursor(1,1)` signals on the empty cursor, then its tail. The slot clears even when no
/// setter takes the action (`0x4e62ba`). Returns whether to repaint.
pub(super) fn pickup_action(lua: &Lua, id: u32) -> bool {
    let picked = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if matches!(
            model.cursor,
            Some(
                CursorPayload::Spell(_)
                    | CursorPayload::Item(_)
                    | CursorPayload::Macro(_)
                    | CursorPayload::Action(_)
            )
        ) {
            None
        } else {
            // An empty slot returns before any setter (`0x4e61a3`).
            Some(lift(&model, id))
        }
    };
    let Some(picked) = picked else {
        return place_action(lua, id);
    };
    let Some(payload) = picked else {
        return false;
    };
    if let Some(payload) = payload {
        set_cursor(lua, true, |_| Some(payload));
    }
    verb(lua, |model| {
        model.actions.remove(&id);
        model.action_sets.push((id, 0));
    });
    true
}

/// `PlaceAction(id)`, the slot's `OnReceiveDrag` and `UseAction(id, 1)`'s place fork. An occupied
/// slot's action hops onto the cursor with `id` as its source, through its setter ([`lift`]),
/// whose inner `ClearCursor(1,1)` drops the held payload, hiding the grid, before the setter shows
/// it again (`0x4e6450`, `0x4e6494`, `0x4e64e2`); an empty slot's place is that clear alone
/// (`0x4e650f`), as is the place over an occupant no setter takes, an unknown kind or a macro
/// gone from the list (`0x4e64b3`, `0x4e64be`; `0x4e6694`, `0x4e66a0`; `0x4e67f3`, `0x4e67fe`).
/// An item stays in its bag, since a bar item is a reference, not a move. The cursor goes first
/// and the slot after, as `0x4e62e0` writes the slot past the setter, so the cursor's events read
/// the slot as it was. Returns whether to repaint.
pub(crate) fn place_action(lua: &Lua, id: u32) -> bool {
    let (placed, displaced) = match verb(lua, |model| plan_place(model, id)) {
        Place::Refused => return false,
        // The held action is already the slot's: `ClearCursor(1,1)` and no write (`0x4e63d4`,
        // `0x4e65db`, `0x4e673a`).
        Place::Same => {
            verb(lua, clear_cursor);
            return false;
        }
        Place::Write(placed, displaced) => (placed, displaced),
    };
    let (kind, action, texture) = placed;
    match displaced {
        Some(Some(displaced)) => {
            set_cursor(lua, true, |_| Some(displaced));
        }
        Some(None) | None => verb(lua, clear_cursor),
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
                use_spell: None,
            },
        );
        model.action_sets.push((id, pack(kind, action)));
    });
    true
}

/// `PlaceAction`'s decision.
enum Place {
    /// An accept filter refused: the payload stays held.
    Refused,
    /// The held action is already in the slot.
    Same,
    /// What the slot takes, and the occupant that hops ([`lift`]).
    Write((u8, u32, Option<String>), Option<Option<CursorPayload>>),
}

/// `PlaceAction`'s decision: the accept filters, then whether the held action is the slot's own,
/// then what the slot takes from the held payload and the occupant that hops.
fn plan_place(model: &mut Model, id: u32) -> Place {
    // The two accept filters (`0x4e62e0`) refuse with a bare return: nothing stored or sent, and
    // the payload stays held. An item needs an on-use spell or an equip slot (`0x4e6571`), so a
    // bag is placeable; a passive spell (`Attributes & 0x40`) also raises errorId `0x9e` through
    // `DisplayError` (`0x4e63ad`, `0x496720`), where the item refusal is mute.
    match &model.cursor {
        Some(CursorPayload::Item(i)) if !i.bar_placeable => return Place::Refused,
        Some(CursorPayload::Spell(s)) if s.passive => {
            model.ui_errors.push(PASSIVE_ON_BAR_ERROR);
            return Place::Refused;
        }
        _ => {}
    }
    let Some(held) = model.cursor.as_ref() else {
        return Place::Refused;
    };
    let placed = match held {
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
        | CursorPayload::Merchant(_) => return Place::Refused,
    };
    // Each arm compares the held action with the slot's: the spell arm a spell id with the spell
    // the slot resolves to (`0x4e63d0`, [`resolved_spell`]), so a macro bound to it or an item
    // casting it matches too; the item arm an item entry, a bag item's or a bar item's alike
    // (`0x4e65cf`); the macro arm a macro id (`0x4e672f`).
    let (kind, action, _) = &placed;
    let same = match model.actions.get(&id) {
        Some(_) if *kind == ACTION_KIND_SPELL => resolved_spell(model, id) == Some(*action),
        Some(slot) => slot.kind == *kind && slot.action == *action,
        None => false,
    };
    if same {
        return Place::Same;
    }
    Place::Write(placed, lift(model, id))
}

/// The spell slot `id` resolves to (`0x4e5a50`): a spell slot's own; a macro slot's bound spell
/// (`0x4e5ba0`, the record's `+0x564`), when the macro exists (`0x4e5030`); an item slot's ON_USE
/// spell (`0x4e5ae0`). `None` for none, an empty slot or an unbound macro.
fn resolved_spell(model: &Model, id: u32) -> Option<u32> {
    let slot = model.actions.get(&id)?;
    let spell = match slot.kind {
        ACTION_KIND_SPELL => slot.action,
        ACTION_KIND_MACRO => {
            model.macros.get(slot.action as usize)?;
            let bound = model.macro_bindings.get(&slot.action)?.spell;
            u32::try_from(bound).ok()?
        }
        ACTION_KIND_ITEM => slot.use_spell?,
        _ => return None,
    };
    (spell != 0).then_some(spell)
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
            use_spell: None,
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
            matches!(s.cursor_payload(), Some(CursorPayload::Spell(c)) if c.spell_id == 133),
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
            Some(CursorPayload::Spell(CursorSpell {
                spell_id: 222,
                texture: Some("Interface\\Icons\\Spell_B".into()),
                passive: false,
            }))
        );
        assert_eq!(s.take_action_sets(), vec![(2, 111)]);
    }

    #[test]
    fn place_action_item_payload_packs_the_item_kind_and_leaves_the_bag_untouched() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            item_class: None,
            guid: 0x114,
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
            item_class: None,
            guid: 0x115,
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
            item_class: None,
            guid: 0x116,
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
    /// (`0x49513a`, `0x49515e`). Each inner clear signals first, so a hop hides the grid, still
    /// reading the dropped action (`0x4952b3` comes before `0x4952bf`), and `CURSOR_UPDATE` reads
    /// the emptied cursor before the displaced action goes on.
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
            ["CURSOR_UPDATE none", "ACTIONBAR_SHOWGRID spell"]
        );
        s.run("PlaceAction(2)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            [
                "ACTIONBAR_HIDEGRID spell",
                "CURSOR_UPDATE none",
                "ACTIONBAR_SHOWGRID action",
                "CURSOR_UPDATE action"
            ]
        );
        // Onto an empty slot the place is the clear alone (`0x4e650f`).
        s.run("PlaceAction(3)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["ACTIONBAR_HIDEGRID action", "CURSOR_UPDATE none"]
        );
    }

    /// A bar spell lifts as the spell setter's mode 3 (`0x4e6254`), so `CursorHasSpell`
    /// (`0x489600`) answers 1 and `PickupSpell`'s held check (`0x4b3285`) sees it; a bar macro
    /// lifts as the macro setter's mode 8 (`0x4e629d`), and one gone from the list lifts nothing
    /// while its slot still clears (`0x4e6279`, `0x4e62ba`).
    #[test]
    fn bar_spells_and_macros_lift_with_their_setters() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 133)));
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();
        s.set_action(
            2,
            Some(action_slot("Interface\\Icons\\Ability_Ambush", 0x40, 1)),
        );
        s.set_action(
            3,
            Some(action_slot("Interface\\Icons\\Ability_Ambush", 0x40, 9)),
        );

        s.run("PickupAction(1)").unwrap();
        assert!(s.eval::<bool>("return CursorHasSpell() == 1").unwrap());
        s.run("ClearCursor() PickupAction(2)").unwrap();
        assert!(matches!(
            s.cursor_payload(),
            Some(CursorPayload::Macro(m)) if m.index == 1
        ));
        s.run("ClearCursor()").unwrap();
        crate::script::tick::heard::listen(&s, &["CURSOR_UPDATE"], None);
        assert!(pickup_action(&s.lua, 3));
        assert!(s.cursor_payload().is_none());
        assert!(
            crate::script::tick::heard::take(&s).is_empty(),
            "no setter, no clear"
        );
        assert_eq!(s.take_action_sets(), vec![(1, 0), (2, 0), (3, 0)]);
    }

    /// `PickupAction` places only a held spell, item, macro or bar item (`0x4e615d`-`0x4e6194`);
    /// with coins held it picks the slot up, the setter's `ClearCursor(1,1)` dropping the coins
    /// with their `PLAYER_MONEY`. An empty slot does nothing (`0x4e61a3`).
    #[test]
    fn pickup_action_lifts_over_a_payload_it_cannot_place() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 133)));
        s.run("PickupPlayerMoney(10)").unwrap();
        crate::script::tick::heard::listen(&s, &["PLAYER_MONEY"], None);
        s.run("PickupAction(2)").unwrap();
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Money(_))));
        s.run("PickupAction(1)").unwrap();
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))));
        assert_eq!(crate::script::tick::heard::take(&s), ["PLAYER_MONEY"]);
        assert_eq!(s.take_action_sets(), vec![(1, 0)]);
    }

    /// The held action already in the slot: `ClearCursor(1,1)` and return, no write and no send,
    /// for a spell (`0x4e63c8`-`0x4e63d4`), an item (`0x4e65cf`-`0x4e65db`) and a macro
    /// (`0x4e672f`-`0x4e673a`).
    #[test]
    fn placing_the_slots_own_action_clears_and_writes_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_action(1, Some(action_slot("Interface\\Icons\\Spell_A", 0x00, 133)));
        s.set_action(2, Some(action_slot("Interface\\Icons\\INV_A", 0x80, 4540)));
        s.run(r#"CreateMacro("Ambush", 1, "/cast Ambush")"#)
            .unwrap();
        s.set_action(
            3,
            Some(action_slot("Interface\\Icons\\Ability_Ambush", 0x40, 1)),
        );
        let held = [
            CursorPayload::Spell(CursorSpell {
                spell_id: 133,
                texture: None,
                passive: false,
            }),
            CursorPayload::Item(CursorItem {
                item_class: None,
                guid: 0x117,
                bar_placeable: true,
                bag: 0,
                slot: 1,
                item_id: 4540,
                texture: None,
                link: None,
                count: None,
                quality: None,
                equip_slots: Vec::new(),
            }),
            CursorPayload::Macro(crate::script::cursor::CursorMacro {
                index: 1,
                texture: None,
            }),
        ];
        for (slot, payload) in [1, 2, 3].into_iter().zip(held) {
            s.set_cursor_for_test(payload);
            assert!(!place_action(&s.lua, slot));
            assert!(
                s.cursor_payload().is_none(),
                "slot {slot}: the cursor clears"
            );
            assert!(
                s.take_action_sets().is_empty(),
                "slot {slot}: nothing written"
            );
        }
    }

    /// A place over an occupant no setter takes, here a macro gone from the list, clears the
    /// cursor before the write, as over an empty slot (`0x4e64be` to `0x4e6508`).
    #[test]
    fn a_place_over_a_dangling_macro_clears_the_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_action(
            3,
            Some(action_slot("Interface\\Icons\\Ability_Ambush", 0x40, 9)),
        );
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        crate::script::tick::heard::listen(&s, &["CURSOR_UPDATE"], None);
        assert!(place_action(&s.lua, 3));
        assert!(s.cursor_payload().is_none(), "the spell is dropped");
        assert_eq!(crate::script::tick::heard::take(&s), ["CURSOR_UPDATE"]);
        assert_eq!(s.take_action_sets(), vec![(3, 133)]);
    }

    /// The spell arm compares the held spell with the spell the slot resolves to (`0x4e5a50`): a
    /// macro bound to it (`0x4e5ba0`) or an item casting it (`0x4e5ae0`) is the same action, and
    /// the place only clears the cursor.
    #[test]
    fn a_spell_dropped_on_a_macro_or_item_casting_it_clears_the_cursor() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"CreateMacro("Fire", 1, "/cast Fireball")"#)
            .unwrap();
        s.set_macro_bindings(std::collections::HashMap::from([(
            1,
            crate::script::MacroBinding {
                spell: 133,
                pet_book: false,
            },
        )]));
        s.set_action(1, Some(action_slot("Interface\\Icons\\Fire", 0x40, 1)));
        let mut item = action_slot("Interface\\Icons\\INV_A", 0x80, 4540);
        item.use_spell = Some(8690);
        s.set_action(2, Some(item));
        for (slot, spell_id) in [(1, 133), (2, 8690)] {
            s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
                spell_id,
                texture: None,
                passive: false,
            }));
            assert!(!place_action(&s.lua, slot));
            assert!(s.cursor_payload().is_none(), "slot {slot}: cleared");
            assert!(
                s.take_action_sets().is_empty(),
                "slot {slot}: nothing written"
            );
        }
        // Another spell on the macro slot places, the macro hopping onto the cursor.
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            spell_id: 116,
            texture: None,
            passive: false,
        }));
        assert!(place_action(&s.lua, 1));
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Macro(_))));
    }
}
