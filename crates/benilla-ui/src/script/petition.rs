//! The guild-charter globals of the guild registrar and the petition window. The snapshot keeps
//! the signer list (`[0xbdce20]`) and the cached `CGPetition` record (`[0xbdce28]`) apart, as the
//! reference does, since the bindings read them independently. Predicates are `1` or nil (no
//! `lua_pushboolean`: `0x6f3810`, `0x6f37f0`) and an unresolved name is nil. `PETITION_SHOW` waits
//! for every signer name and the record (`0x4f419b`), a deferral `crate::ui_petition` owns.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi;
use super::Model;

/// `GetPetitionInfo`'s first return when the record's charter bit is set (`0x84cf8c`, selected at
/// `0x4f43fb`): the literal `PetitionFrame.lua:21` compares against.
pub const PETITION_TYPE_CHARTER: &str = "charter";

/// `GetPetitionInfo`'s first return when the charter bit is clear (`0x84b8f8`). 1.12 servers send
/// only charters, but `CanSignPetition`'s guild and full-charter refusals test the same bit.
pub const PETITION_TYPE_PETITION: &str = "petition";

/// The cached `CGPetition` record `[0xbdce28]`, as `GetPetitionInfo` reads it; `None` in
/// [`PetitionState::record`] is the no-record leg.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionRecordView {
    /// [`PETITION_TYPE_CHARTER`] or [`PETITION_TYPE_PETITION`], from the record's `+0x1110` bit 0.
    pub petition_type: String,
    /// The proposed guild's name, the record's `char[0x100]` at `+0x10`.
    pub title: String,
    /// The record's `char[0x1000]` body at `+0x110`; empty on every 1.12 server.
    pub body_text: String,
    /// The record's signature cap (`+0x1118`), which `CanSignPetition` tests (`0x4f4634`), not the
    /// nine name rows; pushed signed (`fild DWORD`).
    pub max_signatures: i32,
    /// The owner's name from the `NameCache`, nil while uncached (`0x4f446d`), never `""`.
    pub originator: Option<String>,
    /// Whether the active player owns the record (`0x4f447a`, `0x4f4481`).
    pub is_originator: bool,
}

/// What the two charter windows read, mirroring the reference module's `.data` state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionState {
    /// `GetGuildCharterCost()`: showlist entry 0's cost (`[0xbdce50]`) in copper, which `0x4f50ed`
    /// compares against `PLAYER_FIELD_COINAGE`. Unsigned, as the binding zero-extends it
    /// (`0x4f5245`), so a negative wire cost reads ~4.29e9; 0 before any showlist.
    pub charter_cost: u32,
    /// The signers in wire order, `None` until the `NameCache` resolves one; filled by the packet,
    /// apart from [`Self::record`]. `GetNumPetitionNames()` counts these, never the owner.
    pub signers: Vec<Option<String>>,
    /// The cached record; `None` is the no-record leg.
    pub record: Option<PetitionRecordView>,
    /// `CanSignPetition()`, computed app-side as three of its four refusals need state the engine
    /// lacks; it is `1` with nothing open, as in the reference.
    pub can_sign: bool,
}

/// A charter intent queued from Lua for the app to send. Most get no answer of their own (a
/// purchase shows only as the item, an offer answers the target, a refusal comes on the guild error
/// channel), so none may update local state optimistically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionRequest {
    /// `BuyGuildCharter(name)`, after [`validate_guild_name`]; the app adds the registrar NPC
    /// latched at `[0xbdceb0]`, not `CGGameUI`'s interaction pair.
    Buy(String),
    /// `TurnInGuildCharter()`: the app scans the bags on every call, as the reference does
    /// (`0x5ef2b0`).
    TurnIn,
    /// `CloseGuildRegistrar()`: sends nothing (`0x4f5010`).
    CloseRegistrar,
    /// `SignPetition([n])`: a wire byte defaulting to 1, not 0 (`0x4f46d9`); vmangos skips it.
    Sign(i8),
    /// `OfferPetition()`: the app resolves the current target (`CGGameUI`'s selection pair, not its
    /// interaction pair) and runs the eight guards.
    Offer,
    /// `RenamePetition(name)`, after [`validate_guild_name`].
    Rename(String),
    /// `ClosePetition()`: `0x4f3f60` sends `MSG_PETITION_DECLINE` when a petition was open, no sign
    /// is in flight, a record is cached and we do not own it; the app decides.
    ClosePetition,
    /// A name [`validate_guild_name`] refused, with the GlobalStrings key to show; no packet.
    NameRefused(&'static str),
}

impl super::UiScript {
    /// Replace the charter snapshot; the app fires the four events and defers `PETITION_SHOW`.
    pub fn set_petition(&mut self, state: PetitionState) {
        self.model_mut().petition = state;
    }

    /// Take the charter intents queued since the last drain.
    pub fn take_petition_requests(&mut self) -> Vec<PetitionRequest> {
        std::mem::take(&mut self.model_mut().petition_requests)
    }

    /// Drop the queued close intents, returning how many. Firing `PETITION_CLOSED` runs `OnHide`,
    /// whose `ClosePetition()` queues a close; on a switch to a newly offered charter that close
    /// would shut the new window and decline it on the wire. A user's close survives, as the feed
    /// runs `before(UiInput)` and a click's `OnHide` queues after this.
    pub fn drop_petition_close_intents(&mut self) -> usize {
        let requests = &mut self.model_mut().petition_requests;
        let before = requests.len();
        requests.retain(|r| {
            !matches!(
                r,
                PetitionRequest::CloseRegistrar | PetitionRequest::ClosePetition
            )
        });
        before - requests.len()
    }

    /// Queue one charter intent directly: the test seam.
    #[cfg(test)]
    pub fn queue_petition_request(&mut self, request: PetitionRequest) {
        self.model_mut().petition_requests.push(request);
    }
}

/// The guild-name check `BuyGuildCharter` and `RenamePetition` share (`0x4f5160`), which maps the
/// checker `0x6c9b70`'s code to a message and accepts only `0xd`; `Err` carries the GlobalStrings
/// key to show, and no packet is built. Only codes 0 (empty), 10 (an edge space) and 11
/// (consecutive spaces) are built; the rest pass, as the checker's rules are untraced and the
/// server re-checks every name (`ObjectMgr::IsValidCharterName`).
pub fn validate_guild_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("ERR_GUILD_ENTER_NAME");
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Err("ERR_GUILD_NAME_INVALID_SPACE");
    }
    if name.contains("  ") {
        return Err("ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES");
    }
    Ok(())
}

/// A cached name as a string, an uncached one as nil.
fn name_value(lua: &Lua, name: Option<&String>) -> mlua::Result<Value> {
    match name {
        Some(n) => Ok(Value::String(lua.create_string(n)?)),
        None => Ok(Value::Nil),
    }
}

/// Register the charter globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The petition window ──────────────────────────────────────────────────────────────────
    // GetPetitionInfo() (`0x4f43d0`): six values on both legs (`0x4f4493`, `0x4f44d4`), in the
    // order `PetitionFrame.lua:9` destructures them; with no record, `nil, nil, nil, 0, nil, nil`,
    // the fourth the number 0 (`0x4f44b4`).
    g.set(
        "GetPetitionInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = model.petition.record.as_ref() else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.petition_type)?),
                Value::String(lua.create_string(&r.title)?),
                Value::String(lua.create_string(&r.body_text)?),
                Value::Integer(i64::from(r.max_signatures)),
                name_value(lua, r.originator.as_ref())?,
                binding_abi::flag(r.is_originator),
            ]))
        })?,
    )?;

    // GetNumPetitionNames() (`0x4f44e0`): `[0xbdce20]`, unsigned; signatures only, never the owner.
    g.set(
        "GetNumPetitionNames",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.petition.signers.len() as i64)
        })?,
    )?;

    // GetPetitionNameInfo(index) (`0x4f4510`): 1-based, one value on every leg. The bound test is
    // unsigned (`0x4f456c`), so 0 or a negative index wraps and answers nil. A non-numeric
    // argument raises, through mlua's coercion as through the reference's `Usage:`.
    g.set(
        "GetPetitionNameInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(name) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.petition.signers.get(i))
            else {
                return Ok(Value::Nil);
            };
            name_value(lua, name.as_ref())
        })?,
    )?;

    // CanSignPetition() (`0x4f45e0`), the Sign button's only gate, is `1` with no petition open:
    // `0x4f45f7` jumps past the three record refusals into a scan of the zeroed signer array. The
    // window's `isOriginator` branch is what hides the button.
    g.set(
        "CanSignPetition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(binding_abi::flag(model.petition.can_sign))
        })?,
    )?;

    // ── The registrar window ─────────────────────────────────────────────────────────────────
    // GetGuildCharterCost() (`0x4f5230`): copper, unsigned, 0 before any showlist.
    g.set(
        "GetGuildCharterCost",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.petition.charter_cost))
        })?,
    )?;

    // ── The verbs ────────────────────────────────────────────────────────────────────────────
    // The four that carry and push nothing; none may touch the snapshot, which the server owns.
    for (global, request) in [
        ("TurnInGuildCharter", PetitionRequest::TurnIn),
        ("CloseGuildRegistrar", PetitionRequest::CloseRegistrar),
        ("OfferPetition", PetitionRequest::Offer),
        ("ClosePetition", PetitionRequest::ClosePetition),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.petition_requests.push(request.clone());
                Ok(())
            })?,
        )?;
    }

    // SignPetition([n]) (`0x4f46d0`): the optional argument rides the wire as a byte defaulting to
    // 1, not 0 (`0x4f46d9`, `0x4f4749 Put8`); the server skips it.
    g.set(
        "SignPetition",
        lua.create_function(|lua, n: Option<f64>| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `trunc` toward zero, then narrowed the way the `Put8` does.
            let byte = n.map_or(1i8, |v| v.trunc() as i64 as i8);
            model.petition_requests.push(PetitionRequest::Sign(byte));
            Ok(())
        })?,
    )?;

    // BuyGuildCharter(guildName) (`0x4f5260`) returns 1 or nil for name validity, not for a send:
    // `0x4f5294` runs the shared validator, and the action behind it has five silent refusals.
    g.set(
        "BuyGuildCharter",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            match validate_guild_name(&name) {
                Ok(()) => {
                    model.petition_requests.push(PetitionRequest::Buy(name));
                    Ok(Value::Integer(1))
                }
                Err(key) => {
                    model
                        .petition_requests
                        .push(PetitionRequest::NameRefused(key));
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;

    // RenamePetition(name) (`0x4f4930`): zero values, the same validator, silent beyond its
    // message. A bad argument raises mlua's coercion error; the reference's usage string is
    // `Usage(RenamePetition("name")`, unbalanced.
    g.set(
        "RenamePetition",
        lua.create_function(|lua, name: String| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            match validate_guild_name(&name) {
                Ok(()) => model.petition_requests.push(PetitionRequest::Rename(name)),
                Err(key) => model
                    .petition_requests
                    .push(PetitionRequest::NameRefused(key)),
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn record() -> PetitionRecordView {
        PetitionRecordView {
            petition_type: PETITION_TYPE_CHARTER.into(),
            title: "Legacy".into(),
            body_text: String::new(),
            max_signatures: 9,
            originator: Some("Founder".into()),
            is_originator: false,
        }
    }

    fn open(script: &mut UiScript, r: PetitionRecordView, signers: Vec<Option<String>>) {
        script.set_petition(PetitionState {
            charter_cost: 1000,
            signers,
            record: Some(r),
            can_sign: true,
        });
    }

    /// The six returns, in the reference's own destructuring order.
    #[test]
    fn get_petition_info_returns_the_reference_six_in_order() {
        let mut s = UiScript::new().unwrap();
        open(&mut s, record(), vec![]);
        let (kind, title, body, max, originator, is_originator) = s
            .eval::<(String, String, String, i32, String, Option<u32>)>("return GetPetitionInfo()")
            .unwrap();
        assert_eq!(kind, "charter");
        assert_eq!(title, "Legacy");
        assert_eq!(body, "");
        assert_eq!(max, 9);
        assert_eq!(originator, "Founder");
        assert_eq!(is_originator, None, "era nil, not false");

        open(
            &mut s,
            PetitionRecordView {
                is_originator: true,
                ..record()
            },
            vec![],
        );
        assert_eq!(
            s.eval::<Option<u32>>("local _,_,_,_,_,o = GetPetitionInfo(); return o")
                .unwrap(),
            Some(1),
            "era 1, not true"
        );
    }

    #[test]
    fn the_no_record_leg_is_six_values_with_a_numeric_zero() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("GetPetitionInfo()").unwrap(),
            6,
            "six values even with nothing open"
        );
        assert_eq!(
            s.eval::<i64>("local _,_,_,m = GetPetitionInfo(); return m")
                .unwrap(),
            0,
            "the fourth is the number 0, not nil"
        );
        assert_eq!(
            s.eval::<Option<String>>("return (GetPetitionInfo())")
                .unwrap(),
            None,
            "…and the first really is nil"
        );
    }

    /// The zero and negative indices matter because the reference's bound test is unsigned.
    #[test]
    fn petition_names_are_one_based_and_uncached_reads_nil() {
        let mut s = UiScript::new().unwrap();
        open(
            &mut s,
            record(),
            vec![Some("Aaa".into()), None, Some("Ccc".into())],
        );
        assert_eq!(
            s.eval::<i64>("return GetNumPetitionNames()").unwrap(),
            3,
            "three signers — the owner is not one of them"
        );
        assert_eq!(
            s.eval::<String>("return GetPetitionNameInfo(1)").unwrap(),
            "Aaa"
        );
        assert_eq!(
            s.eval::<Option<String>>("return GetPetitionNameInfo(2)")
                .unwrap(),
            None,
            "an unresolved name is nil, never an empty string"
        );
        assert_eq!(
            s.eval::<String>("return GetPetitionNameInfo(3)").unwrap(),
            "Ccc",
            "and it keeps its ROW — the list does not close up around it"
        );
        for out_of_range in ["0", "4", "-1"] {
            assert_eq!(
                s.eval::<Option<String>>(&format!("return GetPetitionNameInfo({out_of_range})"))
                    .unwrap(),
                None,
                "index {out_of_range} answers nil"
            );
        }
    }

    /// The `1` with nothing open is the reference's own asymmetry, not a bug to fix.
    #[test]
    fn can_sign_petition_answers_one_with_nothing_open() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<Option<u32>>("return CanSignPetition()").unwrap(),
            None,
            "the app's own default is a refusal…"
        );

        let mut s = UiScript::new().unwrap();
        s.set_petition(PetitionState {
            can_sign: true,
            ..PetitionState::default()
        });
        assert_eq!(
            s.eval::<Option<u32>>("return CanSignPetition()").unwrap(),
            Some(1),
            "…but the binding reports whatever the app computed, including the no-record 1"
        );
    }

    #[test]
    fn charter_cost_is_unsigned_copper_and_the_registrars() {
        let mut s = UiScript::new().unwrap();
        s.set_petition(PetitionState {
            charter_cost: 1000,
            ..PetitionState::default()
        });
        assert_eq!(s.eval::<i64>("return GetGuildCharterCost()").unwrap(), 1000);

        // A negative wire cost reads ~4.29e9: the binding zero-extends it (`fild QWORD`).
        s.set_petition(PetitionState {
            charter_cost: (-1i32) as u32,
            ..PetitionState::default()
        });
        assert_eq!(
            s.eval::<i64>("return GetGuildCharterCost()").unwrap(),
            4_294_967_295
        );

        s.set_petition(PetitionState::default());
        assert_eq!(s.eval::<i64>("return GetGuildCharterCost()").unwrap(), 0);
    }

    #[test]
    fn sign_petition_defaults_its_wire_byte_to_one() {
        let mut s = UiScript::new().unwrap();
        s.run("SignPetition(); SignPetition(7); SignPetition(3.9)")
            .unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Sign(1),
                PetitionRequest::Sign(7),
                PetitionRequest::Sign(3),
            ],
            "absent = 1, present = truncated toward zero"
        );
    }

    /// A refused name queues its message instead of a packet.
    #[test]
    fn the_name_validator_gates_both_verbs_and_only_buy_reports_it() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<Option<u32>>("return BuyGuildCharter(\"Legacy\")")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.eval::<Option<u32>>("return BuyGuildCharter(\"\")")
                .unwrap(),
            None,
            "an empty name is refused locally — the server answers it with silence"
        );
        assert_eq!(
            s.arity("RenamePetition(\"Legacy\")").unwrap(),
            0,
            "rename pushes nothing at all"
        );
        s.run("RenamePetition(\"  spaced  \")").unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Buy("Legacy".into()),
                PetitionRequest::NameRefused("ERR_GUILD_ENTER_NAME"),
                PetitionRequest::Rename("Legacy".into()),
                PetitionRequest::NameRefused("ERR_GUILD_NAME_INVALID_SPACE"),
            ]
        );
    }

    #[test]
    fn the_validator_refuses_only_what_it_can_prove() {
        assert_eq!(validate_guild_name(""), Err("ERR_GUILD_ENTER_NAME"));
        assert_eq!(
            validate_guild_name(" Legacy"),
            Err("ERR_GUILD_NAME_INVALID_SPACE")
        );
        assert_eq!(
            validate_guild_name("Legacy "),
            Err("ERR_GUILD_NAME_INVALID_SPACE")
        );
        assert_eq!(
            validate_guild_name("Legacy  of Steel"),
            Err("ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES")
        );
        // Passed through: the length, profanity and script rules live in the untraced `0x6c9b70`,
        // and the server re-checks them.
        for ok in ["Legacy of Steel", "A", "Ab", "Éclair", "x y z"] {
            assert_eq!(
                validate_guild_name(ok),
                Ok(()),
                "{ok:?} passes to the server"
            );
        }
    }

    #[test]
    fn dropping_close_intents_leaves_the_other_verbs_untouched() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            SignPetition()
            ClosePetition()
            BuyGuildCharter("Legacy")
            CloseGuildRegistrar()
            OfferPetition()
        "#,
        )
        .unwrap();
        assert_eq!(s.drop_petition_close_intents(), 2);
        assert_eq!(
            s.take_petition_requests(),
            vec![
                PetitionRequest::Sign(1),
                PetitionRequest::Buy("Legacy".into()),
                PetitionRequest::Offer,
            ],
        );
        assert_eq!(s.drop_petition_close_intents(), 0);
    }
}
