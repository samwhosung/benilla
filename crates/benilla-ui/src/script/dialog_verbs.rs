//! The verbs the stock `StaticPopup.lua` dialogs call: the instance-boot and area spirit healer
//! clocks, the battlefield port answer, the meeting stone pair and the pet untrain pair.

use mlua::{Lua, Value};

use super::binding_abi::{bool_or_default, flag, number_arg};
use super::live::ask;
use super::{
    meeting_stone_text, AreaSpiritHealerSecsQuery, InstanceBootSecsQuery, MeetingStoneQuery, Model,
    PetUntrainerPendingQuery,
};

/// The meeting stone's verb the host applies inside its call ([`super::ScriptCall::MeetingStone`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingStoneCall {
    /// `CancelMeetingStoneRequest()` (`0x4ca5a0` → `0x4ca120`): in a party whose leader is not the
    /// player `ERR_MEETING_STONE_NOT_LEADER` (`0x4ca14e`), else `CMSG_MEETINGSTONE_LEAVE`.
    Cancel,
}

impl super::UiScript {
    /// The `ConfirmPetUnlearn()` calls a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_pet_unlearn_confirms(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::ConfirmPetUnlearn) as u32
    }

    /// Set what `CheckPetUntrainerDist()` answers with no host lent.
    pub fn set_pet_untrainer_pending(&mut self, pending: bool) {
        self.model_mut().pet_untrainer_pending = pending;
    }

    /// Set `GetInstanceBootTimeRemaining()`'s answer with no host lent, whole seconds (0 idle).
    pub fn set_instance_boot_secs(&mut self, secs: u32) {
        self.model_mut().instance_boot_secs = secs;
    }

    /// Set the seconds to the area spirit healer's next wave (`[0xb4e338]`) read with no host lent.
    pub fn set_area_spirit_healer_secs(&mut self, secs: u32) {
        self.model_mut().area_spirit_secs = secs;
    }

    /// The `AcceptBattlefieldPort` calls a harness's VM kept: `(slot 1..=3, accept)`.
    pub fn take_battlefield_port_requests(&mut self) -> Vec<(u8, bool)> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Battlefield(super::BattlefieldCall::Port { index, accept }) => {
                Some((*index, *accept))
            }
            _ => None,
        })
    }

    /// The `CancelMeetingStoneRequest()` calls a harness's VM kept.
    pub fn take_meeting_stone_cancels(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::MeetingStone(MeetingStoneCall::Cancel)) as u32
    }

    /// Set the meeting stone's two globals read with no host lent: the queued area id
    /// (`0xb72038`, 0 for none) and the status text (`0xb7203c`, `None` outside the world).
    pub fn set_meeting_stone(&mut self, area: u32, status_text: Option<String>) {
        let mut model = self.model_mut();
        model.meeting_stone_area = area;
        model.meeting_stone_status_text = status_text;
    }
}

/// The area spirit healer's aura, `0x4923a0`'s spell (`0xa18`).
const AREA_SPIRIT_HEALER_AURA: u32 = 2584;

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x48b620`: `max(deadline - now, 0) / 1000`, truncated.
    g.set(
        "GetInstanceBootTimeRemaining",
        lua.create_function(|lua, ()| {
            let secs = ask(lua, &InstanceBootSecsQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.instance_boot_secs
            });
            Ok(i64::from(secs))
        })?,
    )?;

    // `0x48df20` → `0x492310`: the host sends `0x2E3` with the engine's cached guid, silent with
    // none.
    g.set(
        "AcceptAreaSpiritHeal",
        lua.create_function(|lua, ()| {
            super::calls::submit(
                lua,
                super::ScriptCall::Death(super::DeathAction::AcceptAreaSpiritHeal),
            )
        })?,
    )?;
    // `0x48df30` runs the generic cancel-aura (`0x4923a0` → `0x6e7040`) on spell 2584, which the
    // host runs whole inside the call: its event in place (`0x6e70c3`), then `0x136` with
    // `u32 2584`, no guid. Its refusal gate (`AttributesEx` bit 13 set, bit 2 clear, then
    // `0x5ee290`) never trips for 2584 in the shipped `Spell.dbc` (the app's
    // `spell_2584_never_trips_the_cancel_gate`).
    g.set(
        "CancelAreaSpiritHeal",
        lua.create_function(|lua, ()| super::aura::cancel_aura(lua, AREA_SPIRIT_HEALER_AURA))?,
    )?;
    // Whole seconds: `max(0, [0xb4e338] - now) / 1000`.
    g.set(
        "GetAreaSpiritHealerTime",
        lua.create_function(|lua, ()| {
            let secs = ask(lua, &AreaSpiritHealerSecsQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.area_spirit_secs
            });
            Ok(i64::from(secs))
        })?,
    )?;

    // `0x4ab3b0`: an index failing `lua_isnumber` raises, one off 1..=3 is silent; the answer takes
    // the reference's optional-boolean coercion (`0x6f1c10`, default no).
    g.set(
        "AcceptBattlefieldPort",
        lua.create_function(|lua, (index, accept): (Value, Value)| {
            // `0x6f34d0`, then `_ftol` (`0x4ab3ea`) and `dec` against 3.
            let slot = number_arg(lua, index, "Usage: AcceptBattlefieldPort(index, accept)")?;
            if !(1..=3).contains(&slot) {
                return Ok(());
            }
            let accept = bool_or_default(Some(&accept), false);
            super::battlefield_queue::submit(
                lua,
                super::BattlefieldCall::Port {
                    index: slot as u8,
                    accept,
                },
            )
        })?,
    )?;

    // `0x4ca5a0` → `0x4ca120`: the host's leadership refusal or empty `0x293`; only the reply
    // clears.
    g.set(
        "CancelMeetingStoneRequest",
        lua.create_function(|lua, ()| {
            super::calls::submit(
                lua,
                super::ScriptCall::MeetingStone(MeetingStoneCall::Cancel),
            )
        })?,
    )?;

    // `0x4ca570`: the number `1` or nil, never `0` (truthy, it would pin the icon) or a boolean.
    g.set(
        "IsInMeetingStoneQueue",
        lua.create_function(|lua, ()| {
            let area = match ask(lua, &MeetingStoneQuery) {
                Some(stone) => stone.area,
                None => {
                    lua.app_data_ref::<Model>()
                        .expect("model app_data")
                        .meeting_stone_area
                }
            };
            Ok(flag(area != 0))
        })?,
    )?;

    // `0x4ca5b0`: a string or nil, never `""` for nothing; the text the last rebuild left
    // (`0x4ca070`).
    g.set(
        "GetMeetingStoneStatusText",
        lua.create_function(|lua, ()| {
            let text = match ask(lua, &MeetingStoneQuery) {
                Some(stone) => meeting_stone_text(lua, &stone.text),
                None => lua
                    .app_data_ref::<Model>()
                    .expect("model app_data")
                    .meeting_stone_status_text
                    .clone(),
            };
            Ok(match text {
                Some(text) => Value::String(lua.create_string(&text)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // `0x48d1c0`: `1` or nil, never `0`; the latch read at the call, whose range the app tests
    // (`0x5dfd30`).
    g.set(
        "CheckPetUntrainerDist",
        lua.create_function(|lua, ()| {
            let pending = ask(lua, &PetUntrainerPendingQuery).unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.pet_untrainer_pending
            });
            Ok(flag(pending))
        })?,
    )?;
    // `0x48dca0` → `PetUnlearnDialog(0, 0)` (`0x5dfba0`): the host holds the latch and the money
    // gate, and sends `0x2F0` inside the call.
    g.set(
        "ConfirmPetUnlearn",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::ConfirmPetUnlearn)
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
        s.keep_calls();
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
        s.set_area_spirit_healer_secs(17);
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

    /// Each `AcceptAreaSpiritHeal()` is the host's: the engine's cache decides the send
    /// (`0x49231b`).
    #[test]
    fn accept_area_spirit_heal_hands_each_call_to_the_host() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("AcceptAreaSpiritHeal() AcceptAreaSpiritHeal()")
            .unwrap();
        assert_eq!(
            s.take_calls_like(&crate::script::ScriptCall::Death(
                crate::script::DeathAction::AcceptAreaSpiritHeal
            )),
            2
        );
    }

    #[test]
    fn cancel_area_spirit_heal_cancels_the_areas_aura() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.run("CancelAreaSpiritHeal()").unwrap();
        assert_eq!(s.take_cancel_aura_requests(), [2584]);
    }

    #[test]
    fn accept_battlefield_port_raises_on_a_bad_index_and_coerces_the_answer() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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

    #[test]
    fn the_meeting_stone_pair_answers_one_or_nil_and_string_or_nil() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
        s.keep_calls();
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
