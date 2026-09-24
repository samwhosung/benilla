//! The dialog engine's own verbs (decision 1963) — the bindings the stock `StaticPopup.lua`
//! bodies call that no window of ours had ever needed, each built to the reference's own shape
//! (VERIFIED at the bytes unless a line here says INFERRED):
//!
//! | binding | args → returns | here |
//! |---|---|---|
//! | `GetInstanceBootTimeRemaining()` | 0 → 1 number, whole seconds, 0 idle | reads the app's deadline |
//! | `AcceptAreaSpiritHeal()` | 0 → 0 | queues the cached healer for the app to send `0x2E3`; SILENT with none cached |
//! | `CancelAreaSpiritHeal()` | 0 → 0 | the generic cancel-aura of spell 2584 plus `AREA_SPIRIT_HEALER_OUT_OF_RANGE` |
//! | `GetAreaSpiritHealerTime()` | 0 → 1 number, whole seconds | reads the app's deadline |
//! | `AcceptBattlefieldPort(index, accept)` | 2 → 0 | raises on a non-number index, silent off 1..3; the optional-bool answer |
//! | `CancelMeetingStoneRequest()` | 0 → 0 | queued; the app applies the leadership gate |
//! | `IsInMeetingStoneQueue()` | 0 → the number `1` or `nil`, one value always | `[0xb72038] != 0` (1974) |
//! | `GetMeetingStoneStatusText()` | 0 → string or `nil` | the cached text `[0xb7203c]`, `nil` while empty |
//! | `CheckPetUntrainerDist()` | 0 → `1` or `nil` | the app's latch-and-range flag |
//! | `ConfirmPetUnlearn()` | 0 → 0 | counted; the app holds the latch and the money gate |
//!
//! The pet pair is the talent-wipe pair's twin (`talent.rs`), latch for latch. `ForceLogout` is
//! `session.rs`'s. `ReplaceTradeEnchant` is not here: it rides the trade-slot enchant targeting
//! this engine does not build yet, and stays a named gap.

use mlua::{Lua, Value};

use super::binding_abi::{bool_or_default, coerced_number, flag};
use super::Model;

/// The area spirit healer's aura, `0xA18` — the one spell id `CancelAreaSpiritHeal` cancels, and
/// the only cancel-aura argument that also fires `AREA_SPIRIT_HEALER_OUT_OF_RANGE` (`0x6e70b6`).
pub const AREA_SPIRIT_HEALER_SPELL: u32 = 2584;

impl super::UiScript {
    /// `ConfirmPetUnlearn()` calls since the last drain.
    pub fn take_pet_unlearn_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().pet_unlearn_confirms)
    }

    /// What `CheckPetUntrainerDist()` answers: a pending question whose trainer is in reach.
    pub fn set_pet_untrainer_pending(&mut self, pending: bool) {
        self.model_mut().pet_untrainer_pending = pending;
    }

    /// `GetInstanceBootTimeRemaining()`'s answer, whole seconds (0 idle).
    pub fn set_instance_boot_secs(&mut self, secs: u32) {
        self.model_mut().instance_boot_secs = secs;
    }

    /// The area spirit healer: whether one is cached, and the seconds to its next wave.
    pub fn set_area_spirit_healer(&mut self, cached: bool, secs: u32) {
        let mut model = self.model_mut();
        model.area_spirit_healer_cached = cached;
        model.area_spirit_secs = secs;
    }

    /// `AcceptAreaSpiritHeal()` calls since the last drain (each one a `0x2E3` to send).
    pub fn take_area_spirit_accepts(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().area_spirit_accepts)
    }

    /// `AcceptBattlefieldPort` calls since the last drain: `(slot 1..=3, accept)`.
    pub fn take_battlefield_port_requests(&mut self) -> Vec<(u8, bool)> {
        std::mem::take(&mut self.model_mut().battlefield_port_requests)
    }

    /// `CancelMeetingStoneRequest()` calls since the last drain.
    pub fn take_meeting_stone_cancels(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().meeting_stone_cancels)
    }

    /// The meeting stone's two globals (1974): the queued area id (`[0xb72038]`, `0` = none) and
    /// the cached status text (`[0xb7203c]`, `None` from process start to world enter and after
    /// world leave). The app rebuilds the text; the VM only hands it back.
    pub fn set_meeting_stone(&mut self, area: u32, status_text: Option<String>) {
        let mut model = self.model_mut();
        model.meeting_stone_area = area;
        model.meeting_stone_status_text = status_text;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // 0 args → 1 number: `max(deadline − now, 0) / 1000`, truncated (`GetInstanceBootTimeRemaining
    // 0x48b620`).
    g.set(
        "GetInstanceBootTimeRemaining",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.instance_boot_secs))
        })?,
    )?;

    // `AcceptAreaSpiritHeal 0x48df20`: `if (cached healer == 0:0) return;` — SILENT — else `0x2E3`
    // + the cached guid.
    g.set(
        "AcceptAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.area_spirit_healer_cached {
                model.area_spirit_accepts += 1;
            }
            Ok(())
        })?,
    )?;
    // `CancelAreaSpiritHeal 0x48df30`: `0x6e7040(0xA18)` — the generic cancel-aura routine: the
    // event fires (only for this spell), then `0x136` + `u32 2584`, no guid. The routine's refusal
    // — AttributesEx bit 13 set, bit 2 clear, and `0x5ee290(player)` — never reaches its third leg
    // for spell 2584: the shipped Spell.dbc fails the first two, checked against the data by the
    // app's `spell_2584_never_trips_the_cancel_gate`. So the send is unconditional here.
    g.set(
        "CancelAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model
                .pending_events
                .push(("AREA_SPIRIT_HEALER_OUT_OF_RANGE".to_string(), Vec::new()));
            model.cancel_aura_requests.push(AREA_SPIRIT_HEALER_SPELL);
            Ok(())
        })?,
    )?;
    // 0 → 1 number, whole seconds: `max(0, [0xb4e338] − now) / 1000`.
    g.set(
        "GetAreaSpiritHealerTime",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.area_spirit_secs))
        })?,
    )?;

    // `AcceptBattlefieldPort 0x4ab3b0`: arg 1 must satisfy `lua_isnumber` (a number or a numeric
    // string) or the binding RAISES `Usage:`; truncated, 1-based, off 1..=3 SILENT. Arg 2 is the
    // reference's optional-boolean coercion (`0x6f1c10`, default 0), normalised to one byte.
    g.set(
        "AcceptBattlefieldPort",
        lua.create_function(|lua, (index, accept): (Value, Value)| {
            let is_number = match &index {
                Value::Integer(_) | Value::Number(_) => true,
                Value::String(s) => s
                    .to_str()
                    .ok()
                    .is_some_and(|t| t.trim().parse::<f64>().is_ok()),
                _ => false,
            };
            if !is_number {
                return Err(mlua::Error::runtime(
                    "Usage: AcceptBattlefieldPort(index, accept)",
                ));
            }
            let slot = coerced_number(lua, Some(index)).trunc();
            if !(1.0..=3.0).contains(&slot) {
                return Ok(());
            }
            let accept = bool_or_default(Some(&accept), false);
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_port_requests.push((slot as u8, accept));
            Ok(())
        })?,
    )?;

    // `CancelMeetingStoneRequest 0x4ca5a0`: `0x293`, empty; gated only on party leadership (the
    // app's, which holds the party) — it clears nothing, the server's reply does.
    g.set(
        "CancelMeetingStoneRequest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.meeting_stone_cancels += 1;
            Ok(())
        })?,
    )?;

    // `IsInMeetingStoneQueue()` (`0x4ca570`): `mov eax,1` on BOTH legs — the number `1` when the
    // queued area is non-zero, else nil; never `0` (truthy in Lua — it would pin the icon shown),
    // never a boolean.
    g.set(
        "IsInMeetingStoneQueue",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.meeting_stone_area != 0))
        })?,
    )?;

    // `GetMeetingStoneStatusText()` (`0x4ca5b0`): `lua_pushstring([0xb7203c])`, whose NULL leg
    // tail-jumps to `lua_pushnil` — a string or nil, one value, never `""` for "nothing".
    g.set(
        "GetMeetingStoneStatusText",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match &model.meeting_stone_status_text {
                Some(text) => Value::String(lua.create_string(text)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // `CheckPetUntrainerDist 0x48d1c0`: `1` or `nil` — never `0`. The range test
    // (`d² <= INTERACT_DISTANCE²`, a NaN OUT) is the app's, on the latched trainer.
    g.set(
        "CheckPetUntrainerDist",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_untrainer_pending))
        })?,
    )?;
    // `ConfirmPetUnlearn 0x48dca0`: the confirm arm — the latch's guid, the money gate
    // (`ERR_NOT_ENOUGH_MONEY`, no packet) and `0x2F0`, all the app's; here the call is counted.
    g.set(
        "ConfirmPetUnlearn",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_unlearn_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_clocks_read_what_the_app_feeds() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetInstanceBootTimeRemaining()")
                .unwrap(),
            0
        );
        assert_eq!(
            s.eval::<i64>("return GetAreaSpiritHealerTime()").unwrap(),
            0
        );
        s.set_instance_boot_secs(42);
        s.set_area_spirit_healer(true, 17);
        assert_eq!(
            s.eval::<i64>("return GetInstanceBootTimeRemaining()")
                .unwrap(),
            42
        );
        assert_eq!(
            s.eval::<i64>("return GetAreaSpiritHealerTime()").unwrap(),
            17
        );
    }

    #[test]
    fn accept_area_spirit_heal_is_silent_without_a_cached_healer() {
        let mut s = UiScript::new().unwrap();
        s.run("AcceptAreaSpiritHeal()").unwrap();
        assert_eq!(
            s.take_area_spirit_accepts(),
            0,
            "no healer cached: nothing sent"
        );
        s.set_area_spirit_healer(true, 5);
        s.run("AcceptAreaSpiritHeal() AcceptAreaSpiritHeal()")
            .unwrap();
        assert_eq!(s.take_area_spirit_accepts(), 2);
    }

    #[test]
    fn cancel_area_spirit_heal_cancels_the_aura_and_fires_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"F = CreateFrame("Frame") F:RegisterEvent("AREA_SPIRIT_HEALER_OUT_OF_RANGE")
               F:SetScript("OnEvent", function() GOT = event end) CancelAreaSpiritHeal()"#,
        )
        .unwrap();
        s.tick(0.0);
        assert_eq!(
            s.eval::<String>("return GOT").unwrap(),
            "AREA_SPIRIT_HEALER_OUT_OF_RANGE"
        );
        assert_eq!(
            s.take_cancel_aura_requests(),
            vec![super::AREA_SPIRIT_HEALER_SPELL]
        );
    }

    #[test]
    fn accept_battlefield_port_raises_on_a_bad_index_and_coerces_the_answer() {
        let mut s = UiScript::new().unwrap();
        assert!(
            s.run("AcceptBattlefieldPort(nil, 1)").is_err(),
            "a non-number index raises"
        );
        assert!(s.run(r#"AcceptBattlefieldPort("x", 1)"#).is_err());
        s.run("AcceptBattlefieldPort(4, 1) AcceptBattlefieldPort(0, 1)")
            .unwrap();
        assert!(
            s.take_battlefield_port_requests().is_empty(),
            "off 1..3 is silent"
        );
        s.run(r#"AcceptBattlefieldPort(1, 1) AcceptBattlefieldPort("2", "off") AcceptBattlefieldPort(3.9) AcceptBattlefieldPort(2, true)"#)
            .unwrap();
        assert_eq!(
            s.take_battlefield_port_requests(),
            vec![(1, true), (2, false), (3, false), (2, true)],
            "truncated index, the optional-bool table, nil defaulting to no"
        );
    }

    /// The meeting stone pair: `1`/nil on the area, string/nil on the text — one value each.
    #[test]
    fn the_meeting_stone_pair_answers_one_or_nil_and_string_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("IsInMeetingStoneQueue()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return IsInMeetingStoneQueue() == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetMeetingStoneStatusText() == nil")
            .unwrap());
        s.set_meeting_stone(1519, Some("Looking for more for Stormwind City".into()));
        assert_eq!(s.eval::<i64>("return IsInMeetingStoneQueue()").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return GetMeetingStoneStatusText()")
                .unwrap(),
            "Looking for more for Stormwind City"
        );
        s.set_meeting_stone(0, Some("Unknown".into()));
        assert!(
            s.eval::<bool>("return IsInMeetingStoneQueue() == nil")
                .unwrap(),
            "area 0: not queued, whatever the text says"
        );
        assert_eq!(
            s.eval::<String>("return GetMeetingStoneStatusText()")
                .unwrap(),
            "Unknown"
        );
    }

    #[test]
    fn the_pet_pair_counts_and_answers_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return CheckPetUntrainerDist() == nil")
            .unwrap());
        s.set_pet_untrainer_pending(true);
        assert_eq!(s.eval::<i64>("return CheckPetUntrainerDist()").unwrap(), 1);
        s.run("ConfirmPetUnlearn() ConfirmPetUnlearn() CancelMeetingStoneRequest()")
            .unwrap();
        assert_eq!(s.take_pet_unlearn_confirms(), 2);
        assert_eq!(s.take_meeting_stone_cancels(), 1);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
