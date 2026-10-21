//! The four duel globals, registered together at `0x849fa8`. What the UI reads arrives as event
//! arguments, so each call only queues a [`DuelRequest`] for the app to send. `AcceptDuel`
//! (`0x4d4ce0`) and `CancelDuel` (`0x4d4cf0`) call `0x4d4830` and `0x4d48b0`; `StartDuel`
//! (`0x4d4c40`) and `StartDuelUnit` (`0x4d4c90`) send no duel opcode but cast the duel spell at
//! the guid they resolve (`0x4d4810`).

use mlua::{Lua, Value};

use super::by_name::Filter;
use super::protected::{self, Tag};
use super::Model;

/// A duel intent queued by the globals, drained by `UiScript::take_duel_requests`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuelRequest {
    /// `AcceptDuel()`, the `DUEL_REQUESTED` popup's Accept: `CMSG_DUEL_ACCEPTED`.
    Accept,
    /// `CancelDuel()`: `CMSG_DUEL_CANCELLED`, which the server reads by duel state as a decline,
    /// a cancel or a forfeit.
    Cancel,
    /// The duel spell at this guid (`0x4d4810`), never zero: `StartDuel(name)` after the by-name
    /// resolver (`0x4d4c6d`, players only, no filter), or `StartDuelUnit(unit)`, the unit popup's
    /// Duel row, after the token resolver (`0x4d4caf`), any unit it names.
    StartByGuid(u64),
}

impl super::UiScript {
    /// Drain the duel intents queued since the last call.
    pub fn take_duel_requests(&mut self) -> Vec<DuelRequest> {
        std::mem::take(&mut self.model_mut().duel_requests)
    }

    /// `StartDuel(name)` for the app's Rust slash parser: the binding's own resolve and queue.
    pub fn start_duel_by_name(&mut self, name: &str) {
        start_duel_by_name(&mut self.model_mut(), name);
    }

    /// Queue an intent from the app's Rust slash parser: `/duel` and `/forfeit`, which the
    /// reference's `SlashCmdList` handlers send through `StartDuel` and `CancelDuel`.
    pub fn queue_duel_request(&mut self, request: DuelRequest) {
        self.model_mut().duel_requests.push(request);
    }
}

/// `StartDuel` (`0x4d4c40`): `0x493aa0` with typemask 0x10 and mode 0 (`0x4d4c6d`), then the duel
/// spell at the guid (`0x4d4810`); a zero guid does nothing (`0x4d4c76`).
fn start_duel_by_name(model: &mut Model, name: &str) {
    if let Some(guid) = model.closest_player(name, Filter::AcceptAll) {
        model.duel_requests.push(DuelRequest::StartByGuid(guid));
    }
}

/// Register the duel globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "AcceptDuel",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::Accept);
            Ok(())
        })?,
    )?;

    // `CancelDuel()`: the popup's Decline, and `/forfeit` (`/concede`, `/yield`) once under way.
    g.set(
        "CancelDuel",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.duel_requests.push(DuelRequest::Cancel);
            Ok(())
        })?,
    )?;

    // StartDuel(name) (`0x4d4c40`): a non-string returns silently (`0x4d4c4f`). The challenge
    // is a plain cast (`CastDuelSpell 0x4d4810` → `0x6e5a90`), behind the gate's tag 2
    // (`0x6e4bc1`), which a name that resolves to nobody never reaches (`0x4d4c76`).
    g.set(
        "StartDuel",
        lua.create_function(|lua, name: Value| {
            let Some(name) = super::binding_abi::optional_string(lua, &name) else {
                return Ok(());
            };
            let guid = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.closest_player(&name, Filter::AcceptAll)
            };
            if let Some(guid) = guid.filter(|_| protected::gate(lua, Tag::CAST)) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.duel_requests.push(DuelRequest::StartByGuid(guid));
            }
            Ok(())
        })?,
    )?;

    // StartDuelUnit(unit) (`0x4d4c90`): a non-string raises its usage (`0x4d4cc5`), and the token
    // goes through `0x515970` (`0x4d4caf`), which raises for an unknown one. Only a zero guid
    // returns (`0x4d4cb8`): any other, ourselves or a unit the object manager does not hold
    // included, goes to the duel cast (`0x4d4cbc`), behind the gate as `StartDuel`'s.
    g.set(
        "StartDuelUnit",
        lua.create_function(|lua, unit: Value| {
            let token =
                super::binding_abi::string_arg(lua, unit, r#"Usage: StartDuelUnit("unit")"#)?;
            let guid = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.unit_guids.guid_of(&token)?.unwrap_or(0)
            };
            if guid != 0 && protected::gate(lua, Tag::CAST) {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.duel_requests.push(DuelRequest::StartByGuid(guid));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod start_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{NameCandidate, PartyMemberInfo, PartyState, UiScript, UnitState};

    fn player_vm(members: &[(&str, u64)]) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                has_object: true,
                ..Default::default()
            }),
        );
        s.set_party(PartyState {
            members: members
                .iter()
                .map(|(name, guid)| PartyMemberInfo {
                    name: (*name).into(),
                    guid: *guid,
                })
                .collect(),
            ..Default::default()
        });
        s
    }

    fn held(s: &mut UiScript, guid: u64, name: &str, dist2: f32, is_player: bool) {
        s.set_name_candidate(
            guid,
            Some(NameCandidate {
                name: name.into(),
                dist2,
                state: UnitState {
                    exists: true,
                    has_object: true,
                    guid,
                    is_player,
                    ..Default::default()
                },
                assistable_alive: false,
            }),
        );
    }

    /// `StartDuel` resolves through `0x493aa0` with typemask 0x10 and mode 0 (`0x4d4c6d`): a prefix
    /// finds a groupmate with no object, a held player outside the group answers when nearer or
    /// longer, accept-all means even a dead or hostile one, a creature is outside the typemask,
    /// and a miss queues nothing.
    #[test]
    fn start_duel_resolves_a_player_by_prefix_in_or_out_of_the_group() {
        let mut s = player_vm(&[("Onerogue", 0x0E)]);
        s.run("StartDuel('One')").unwrap();
        assert_eq!(s.take_duel_requests(), vec![DuelRequest::StartByGuid(0x0E)]);
        // A held stranger with the same prefix is nearer than the roster's placeholder.
        held(&mut s, 0x77, "Onemage", 40.0, true);
        s.run("StartDuel('One')").unwrap();
        assert_eq!(s.take_duel_requests(), vec![DuelRequest::StartByGuid(0x77)]);
        // A creature is no candidate.
        let mut s = player_vm(&[]);
        held(&mut s, 0xF1, "Onewolf", 1.0, false);
        s.run("StartDuel('One')").unwrap();
        assert!(s.take_duel_requests().is_empty());
        // No shared first letter, and no active player object (`0x493ae0`).
        let mut s = player_vm(&[("Onerogue", 0x0E)]);
        s.run("StartDuel('Zed')").unwrap();
        assert!(s.take_duel_requests().is_empty());
        let mut s = UiScript::new().unwrap();
        s.set_party(PartyState {
            members: vec![PartyMemberInfo {
                name: "Onerogue".into(),
                guid: 0x0E,
            }],
            ..Default::default()
        });
        s.run("StartDuel('One')").unwrap();
        assert!(s.take_duel_requests().is_empty());
    }
}
