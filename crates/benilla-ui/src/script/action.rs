//! The action-bar bindings. The app pushes what each of the 120 slots shows and its per-frame
//! state, and drains the queued `UseAction` presses and the slot writes the cursor made
//! (`CMSG_SET_ACTION_BUTTON`); the engine keeps each slot's packed `(kind, action)` so the cursor
//! can move a slot without the app. Actions are keyed by Lua action id, 1..120, one more than the
//! wire's slot index. FrameXML's bonus bar shows actions `(6 + offset - 1) * 12 + i` for the
//! app-pushed `GetBonusBarOffset`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::Model;

/// The kind byte, bits 24-31 of the packed slot word (vmangos `Player.h:133`). Must match
/// `benilla_protocol`'s `ACTION_KIND_*`; this crate has no protocol dependency.
pub(crate) const ACTION_KIND_SPELL: u8 = 0x00;
pub(crate) const ACTION_KIND_MACRO: u8 = 0x40;
pub(crate) const ACTION_KIND_ITEM: u8 = 0x80;

/// What one action slot shows; the app resolves the icon before pushing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionSlot {
    /// The icon texture path (`Interface\Icons\…`); `None` shows the slot's fallback.
    pub texture: Option<String>,
    /// The kind byte, one of the `ACTION_KIND_*` above.
    pub kind: u8,
    /// The spell, macro or item id (bits 0-23 of the packed slot word).
    pub action: u32,
    /// `GetActionCount`'s bag count for an item slot, 0 otherwise.
    pub count: u32,
    /// `IsConsumableAction`: `0x4e5250` reads only the slot's item template, so this rides the
    /// icon's push, not [`ActionState`]'s.
    pub consumable: bool,
}

/// One action's per-frame state, read by `IsUsableAction`, `IsActionInRange`, `GetActionCooldown`
/// and kin; kept apart from [`ActionSlot`] so its churn never fires `ACTIONBAR_SLOT_CHANGED`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionState {
    /// `IsUsableAction`'s first return: castable now.
    pub usable: bool,
    /// `IsUsableAction`'s second return: unusable only for lack of power (FrameXML's blue tint).
    pub not_enough_mana: bool,
    /// `IsActionInRange`: `None` (nil) for a rangeless action or no target, else 1 or 0.
    pub in_range: Option<bool>,
    /// `ActionHasRange`: the action's spell has a range to test.
    pub has_range: bool,
    /// `IsCurrentAction`: the Attack action while auto-attack is on, or the spell being cast.
    pub current: bool,
    /// `IsAutoRepeatAction`: the action's spell is the live autorepeat spell (`0xceac30`).
    pub auto_repeat: bool,
    /// `IsAttackAction`: the action is the melee auto-attack.
    pub is_attack: bool,
    /// `IsEquippedAction`: an item action currently worn.
    pub equipped: bool,
    /// `(start_ms, duration_ms, enabled)`, the start absolute on the `GetTime` clock, so a running
    /// cooldown re-pushes the same triple and a re-arm a new one (`0x6e13e0` returns the start).
    pub cooldown: Option<(i64, u32, bool)>,
}

/// [`ActionState`] with its cooldown in `GetTime` seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StoredActionState {
    pub(crate) state: ActionState,
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

/// One queued `UseAction` press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionUse {
    /// The 1-based Lua action id, as `UseAction` was given it.
    pub action: u32,
    /// `UseAction`'s third argument, which only the self-cast bindings (`SELFACTIONBUTTON1`-`12`)
    /// pass, through `ActionButtonUp(id, 1)`.
    pub on_self: bool,
}

impl super::UiScript {
    /// Push (or clear) one action slot, keyed by Lua action id (1..120).
    pub fn set_action(&mut self, action: u32, slot: Option<ActionSlot>) {
        let mut model = self.model_mut();
        match slot {
            Some(s) => {
                model.actions.insert(action, s);
            }
            None => {
                model.actions.remove(&action);
            }
        }
    }

    /// Push (or clear) one action's state; the cooldown start is already on the `GetTime` clock,
    /// so storing only converts milliseconds to seconds.
    pub fn set_action_state(&mut self, action: u32, state: Option<ActionState>) {
        let mut model = self.model_mut();
        match state {
            Some(s) => {
                let cooldown = s.cooldown.map(|(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                });
                model
                    .action_states
                    .insert(action, StoredActionState { state: s, cooldown });
            }
            None => {
                model.action_states.remove(&action);
            }
        }
    }

    /// Push the current bonus-bar page offset (0 = the plain main bar; warrior stances 1..3).
    pub fn set_bonus_bar_offset(&mut self, offset: u8) {
        self.model_mut().bonus_bar_offset = offset;
    }

    /// Drain the presses queued by `UseAction` since the last call.
    pub fn take_action_uses(&mut self) -> Vec<ActionUse> {
        std::mem::take(&mut self.model_mut().action_uses)
    }

    /// Drain the `(lua action id, packed)` pairs `PickupAction`/`PlaceAction` queued; the app
    /// sends each as `CMSG_SET_ACTION_BUTTON` (`packed == 0` clears) and updates its own store.
    pub fn take_action_sets(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.model_mut().action_sets)
    }

    /// Drain the GlobalStrings keys of engine-side refusals; the app fires `UI_ERROR_MESSAGE` for
    /// each, where the reference calls `CGGameUI::DisplayError` inline.
    pub fn take_ui_errors(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.model_mut().ui_errors)
    }
}

/// `UseAction`'s flag test: `nil`, `false` and `0` are false, unlike Lua, where `0` is true.
pub(super) fn truthy_nonzero(v: &Value) -> bool {
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Number(n) => *n != 0.0,
        _ => true,
    }
}

/// Register the action globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "HasAction",
        lua.create_function(|lua, action: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.actions.contains_key(&action)))
        })?,
    )?;

    g.set(
        "GetActionTexture",
        lua.create_function(|lua, action: u32| {
            let tex = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.actions.get(&action).and_then(|s| s.texture.clone())
            };
            match tex {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetActionText(slot): the macro's name, else nil. `0x4e7050` tests only
    // `raw & 0xf0000000 == 0x40000000`, so spell, item and empty slots all answer nil; a missing or
    // non-number slot raises `Usage: GetActionText(slot)` (`0x84bff4`); the slot is 1-based
    // (`0x4e7072` `dec eax`). An empty name answers "": it is an inline buffer at `+0x24`, so
    // `0x6f3890`'s null guard never fires.
    // The reference's macro payload (`raw & 0x3fffffff`) is a hash key into `[0xbdcc54]`; ours is
    // the 1..36 index, the lookup the app's macro icon makes too, so text and icon agree.
    // Deviation: an out-of-range slot answers nil, because the reference indexes `0xbc6980` with
    // no bounds check and reads the memory beside its 120 slots.
    g.set(
        "GetActionText",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, "Usage: GetActionText(slot)")?;
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                u32::try_from(slot)
                    .ok()
                    .and_then(|s| model.actions.get(&s))
                    .filter(|s| s.kind & 0xf0 == ACTION_KIND_MACRO)
                    .and_then(|s| model.macros.get(s.action as usize))
                    .map(|m| m.name.clone())
            };
            match name {
                Some(n) => Ok(Value::String(lua.create_string(&n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // UseAction(action [, checkCursor [, onSelf]]): a nonzero `checkCursor` while the cursor holds
    // a payload places it, as `PlaceAction` does; otherwise the press is queued with `onSelf`, the
    // self-cast flag. FrameXML passes 1 from a click and 0 from a keybind; the binding's own test
    // of it is untraced, and place-on-click is the reading that split implies.
    g.set(
        "UseAction",
        lua.create_function(|lua, (action, rest): (u32, MultiValue)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mut rest = rest.iter();
            let check_cursor = rest.next().is_some_and(truthy_nonzero);
            let on_self = rest.next().is_some_and(truthy_nonzero);
            if check_cursor && model.cursor.is_some() {
                super::cursor::place_action(&mut model, action);
            } else {
                model.action_uses.push(ActionUse { action, on_self });
            }
            Ok(())
        })?,
    )?;

    g.set(
        "GetActionCount",
        lua.create_function(|lua, action: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model
                .actions
                .get(&action)
                .filter(|s| s.kind == ACTION_KIND_ITEM)
                .map_or(0, |s| s.count))
        })?,
    )?;

    g.set(
        "GetBonusBarOffset",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.bonus_bar_offset))
        })?,
    )?;

    // ChangeActionBarPage(): fires `ACTIONBAR_PAGE_CHANGED` synchronously and returns nothing
    // (`0x4e7650`); the page itself is FrameXML's `CURRENT_ACTIONBAR_PAGE`.
    g.set(
        "ChangeActionBarPage",
        lua.create_function(|lua, ()| {
            super::tick::fire_event_into(lua, "ACTIONBAR_PAGE_CHANGED", Vec::new());
            Ok(())
        })?,
    )?;

    // ── Per-action state ──
    // 1/nil booleans and `IsActionInRange`'s nil/0/1, which FrameXML tests with a plain `if`.

    fn state_flag(lua: &Lua, action: u32, pick: impl Fn(&ActionState) -> bool) -> Value {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match model.action_states.get(&action) {
            Some(s) if pick(&s.state) => Value::Integer(1),
            _ => Value::Nil,
        }
    }

    g.set(
        "IsUsableAction",
        lua.create_function(|lua, action: u32| {
            Ok((
                state_flag(lua, action, |s| s.usable),
                state_flag(lua, action, |s| s.not_enough_mana),
            ))
        })?,
    )?;

    g.set(
        "IsActionInRange",
        lua.create_function(|lua, action: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(
                match model
                    .action_states
                    .get(&action)
                    .and_then(|s| s.state.in_range)
                {
                    Some(true) => Value::Integer(1),
                    Some(false) => Value::Integer(0),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    g.set(
        "ActionHasRange",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.has_range)))?,
    )?;
    g.set(
        "IsCurrentAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.current)))?,
    )?;
    g.set(
        "IsAutoRepeatAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.auto_repeat)))?,
    )?;
    g.set(
        "IsAttackAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.is_attack)))?,
    )?;
    // From the slot, not the state map: `0x4e5250` depends only on the item template.
    g.set(
        "IsConsumableAction",
        lua.create_function(|lua, action: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            match model.actions.get(&action) {
                Some(s) if s.consumable => Ok(Value::Integer(1)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    g.set(
        "IsEquippedAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.equipped)))?,
    )?;

    // GetActionCooldown(action) → start, duration, enable, in `GetTime` seconds. An absent or
    // elapsed enabled cooldown answers `(0, 0, 1)`, or a later `CooldownFrame_SetTimer` would
    // replay the sweep and its finish flash.
    g.set(
        "GetActionCooldown",
        lua.create_function(|lua, action: u32| {
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(
                match model.action_states.get(&action).and_then(|s| s.cooldown) {
                    Some((start, duration, enabled)) if start + duration > now || !enabled => {
                        (start, duration, i32::from(enabled))
                    }
                    _ => (0.0, 0.0, 1),
                },
            )
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ActionSlot;
    use crate::script::UiScript;

    fn uses(s: &mut UiScript) -> Vec<(u32, bool)> {
        s.take_action_uses()
            .into_iter()
            .map(|u| (u.action, u.on_self))
            .collect()
    }

    /// The numeric test: `0` is not the modifier, though Lua would call it true.
    #[test]
    fn use_actions_third_argument_is_the_self_cast_modifier() {
        let mut s = UiScript::new().unwrap();
        s.run("UseAction(1)").unwrap();
        s.run("UseAction(2, 0)").unwrap();
        s.run("UseAction(3, 0, 0)").unwrap();
        s.run("UseAction(4, 0, 1)").unwrap();
        s.run("UseAction(5, nil, 1)").unwrap();
        assert_eq!(
            uses(&mut s),
            vec![(1, false), (2, false), (3, false), (4, true), (5, true)],
            "only a numeric-nonzero third argument is the modifier"
        );
    }

    #[test]
    fn change_action_bar_page_fires_the_event_and_nothing_else() {
        let s = UiScript::new().unwrap();
        s.run(
            "CURRENT_ACTIONBAR_PAGE = 3 \
             local f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_PAGE_CHANGED') \
             f:SetScript('OnEvent', function() SEEN = event .. ':' .. CURRENT_ACTIONBAR_PAGE end) \
             N = table.getn({ChangeActionBarPage()})",
        )
        .unwrap();
        assert_eq!(
            s.eval::<String>("return SEEN").unwrap(),
            "ACTIONBAR_PAGE_CHANGED:3",
            "the event fires synchronously, and the page is whatever FrameXML wrote"
        );
        assert_eq!(s.eval::<i64>("return N").unwrap(), 0, "zero return values");
    }

    #[test]
    fn action_snapshot_reads_and_use_queues() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return HasAction(73)").unwrap());
        assert!(s
            .eval::<bool>("return GetActionTexture(73) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetBonusBarOffset()").unwrap(), 0);

        s.set_action(
            73,
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
                kind: 0x00,
                action: 133,
                count: 0,
                consumable: false,
            }),
        );
        s.set_bonus_bar_offset(1);
        assert!(s.eval::<bool>("return HasAction(73)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(73)").unwrap(),
            "Interface\\Icons\\Ability_Rogue_Ambush"
        );
        assert_eq!(s.eval::<i64>("return GetBonusBarOffset()").unwrap(), 1);

        s.run("UseAction(73)").unwrap();
        s.run("UseAction(74, 0, 1)").unwrap();
        assert_eq!(uses(&mut s), vec![(73, false), (74, true)]);
        assert!(s.take_action_uses().is_empty());

        s.set_action(73, None);
        assert!(!s.eval::<bool>("return HasAction(73)").unwrap());
    }

    #[test]
    fn get_action_count_reads_the_pushed_count() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetActionCount(5)").unwrap(), 0);
        s.set_action(
            5,
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                kind: 0x80,
                action: 117,
                count: 4,
                consumable: false,
            }),
        );
        assert_eq!(s.eval::<i64>("return GetActionCount(5)").unwrap(), 4);
    }

    /// A place needs a nonzero `checkCursor` and a held payload; a keybind's `0` queues the use.
    #[test]
    fn use_action_check_cursor_routes_to_place_or_use() {
        use crate::script::cursor::{CursorAction, CursorPayload};

        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x00,
            action: 111,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));

        s.run("UseAction(9, 0)").unwrap();
        assert_eq!(uses(&mut s), vec![(9, false)]);
        assert!(s.cursor_payload().is_some(), "checkCursor 0 never places");

        s.run("UseAction(9, 1)").unwrap();
        assert!(s.take_action_uses().is_empty(), "routed to place, not use");
        assert!(s.cursor_payload().is_none(), "empty destination clears");
        assert_eq!(s.take_action_sets(), vec![(9, 111)]);

        s.run("UseAction(10, 1)").unwrap();
        assert_eq!(uses(&mut s), vec![(10, false)]);
    }

    // ── `GetActionText` (`0x4e7050`) ──

    #[test]
    fn get_action_text_is_the_macro_name_and_nil_for_everything_else() {
        use crate::script::{MacroState, MacroView};

        let mut s = UiScript::new().unwrap();
        s.set_macros(MacroState {
            account: vec![
                MacroView {
                    name: "Pull".into(),
                    ..Default::default()
                },
                MacroView {
                    name: String::new(),
                    ..Default::default()
                },
            ],
            character: Vec::new(),
        });
        let slot = |kind: u8, action: u32| {
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\Spell_A".into()),
                kind,
                action,
                count: 0,
                consumable: false,
            })
        };
        s.set_action(1, slot(0x40, 1)); // macro "Pull"
        s.set_action(2, slot(0x40, 2)); // a macro with an empty name
        s.set_action(3, slot(0x40, 30)); // no such macro
        s.set_action(4, slot(0x00, 133)); // a spell
        s.set_action(5, slot(0x80, 117)); // an item

        assert_eq!(s.eval::<String>("return GetActionText(1)").unwrap(), "Pull");
        assert_eq!(
            s.eval::<String>("return GetActionText(2)").unwrap(),
            "",
            "an empty macro NAME is the empty string, not nil — `+0x24` is an inline buffer"
        );
        for (slot, why) in [
            (3, "a macro id that resolves to nothing"),
            (4, "a SPELL — the same nil arm"),
            (5, "an ITEM — the same nil arm"),
            (6, "an empty slot"),
            (121, "past the 120-slot array"),
            (0, "slot 0 (1-based)"),
        ] {
            assert!(
                s.eval::<bool>(&format!("return GetActionText({slot}) == nil"))
                    .unwrap(),
                "{why} must be nil"
            );
        }
        // One value on every non-raising path.
        assert_eq!(s.arity("GetActionText(4)").unwrap(), 1);
    }

    /// The raise is `0x4e70be` into `0x6f4940`, which never returns.
    #[test]
    fn get_action_text_raises_on_a_bad_slot() {
        let s = UiScript::new().unwrap();
        for call in ["GetActionText()", "GetActionText({})"] {
            let err = s
                .eval::<mlua::Value>(&format!("return {call}"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: GetActionText(slot)"),
                "{call} must raise, got {err}"
            );
        }
    }
}
