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

/// A charter verb past its binding's checks, which the host applies inside the call
/// ([`super::ScriptCall::Petition`]). Most get no answer of their own (a purchase shows only as
/// the item, an offer answers the target, a refusal comes on the guild error channel), so none
/// updates the snapshot ahead of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionCall {
    /// `BuyGuildCharter(name)` past [`validate_guild_name`]: the purchase `0x4f5050`, which sends
    /// for the registrar NPC latched at `[0xbdceb0]`, not `CGGameUI`'s interaction pair.
    Buy(String),
    /// `BuyGuildCharter(name)`'s name [`validate_guild_name`] refused, with the GlobalStrings key
    /// `0x4f5160` shows (`0x496720`); no packet.
    NameRefused(&'static str),
    /// `TurnInGuildCharter()`: a bag scan on every call, as the reference does (`0x5ef2b0`).
    TurnIn,
    /// `GetTabardInfo()` (`0x4f5300`), not a getter: with a player, `MSG_TABARDVENDOR_ACTIVATE`
    /// for the registrar's guid unless the designer is already open on that guid, or the
    /// `ERR_EMBLEMERROR_NOTABARDGEOSET` refusal while the player's display id is not its native
    /// one (`0x5e00e0`); then the registrar closes (`0x4f5355` → `0x4f5010`).
    TabardInfo,
    /// `SignPetition([n])`: a wire byte defaulting to 1, not 0 (`0x4f46d9`); vmangos skips it.
    Sign(i8),
    /// `OfferPetition()`: the host resolves the current target (`CGGameUI`'s selection pair, not
    /// its interaction pair) and runs the guards.
    Offer,
    /// `RenamePetition(name)`: the name and what [`validate_guild_name`] answered, which the host
    /// shows or sends only with a petition open (`0x4f495e`-`0x4f4970`).
    Rename {
        name: String,
        refusal: Option<&'static str>,
    },
    /// `ClosePetition()`: `0x4f3f60` sends `MSG_PETITION_DECLINE` when a petition was open, no
    /// sign is in flight, a record is cached and we do not own it, then signals `PETITION_CLOSED`.
    Close,
}

impl super::UiScript {
    /// Replace the charter snapshot; the app fires the four events and defers `PETITION_SHOW`.
    pub fn set_petition(&mut self, state: PetitionState) {
        self.model_mut().petition = state;
    }

    /// `0x4f3ef0` inside a close: the signers freed, the record still cached for the
    /// `PETITION_CLOSED` handlers (`0x4f4016`, then `0x4f4078` after the signal).
    pub fn free_petition_signers(&mut self) {
        let mut model = self.model_mut();
        model.petition.signers.clear();
        model.petition.can_sign = true;
    }

    /// The close's last step (`0x4f4078`): the record goes.
    pub fn drop_petition_record(&mut self) {
        self.model_mut().petition.record = None;
    }

    /// Take the `CloseGuildRegistrar()` calls made since the last take: the close core needs the
    /// interaction latch, which a core answers when its own `GUILD_REGISTRAR_CLOSED` raised the
    /// call through `OnHide`.
    pub fn take_registrar_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().registrar_close)
    }

    /// The charter calls a harness VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_petition_calls(&mut self) -> Vec<PetitionCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Petition(call) => Some(call.clone()),
            _ => None,
        })
    }
}

/// Hand `call` to the host, which applies it before the verb returns.
fn submit(lua: &Lua, call: PetitionCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Petition(call))
}

/// The guild-name check `BuyGuildCharter` and `RenamePetition` share (`0x4f5160`): the checker
/// `0x6c9b70` at the client's locale (`[0xc0e080]`, `0x4f5162`), its accept (`0xd`) passing and
/// every other code mapped to a message by the table at `0x4f51ec`; `Err` carries the
/// GlobalStrings key to show, and no packet is built.
pub fn validate_guild_name(
    name: &str,
    lists: &dyn super::NameLists,
    charsets: super::NameCharsets,
) -> Result<(), &'static str> {
    use super::char_name as code;
    Err(match code::check_guild_name(name, lists, charsets) {
        code::SUCCESS => return Ok(()),
        code::NO_NAME => "ERR_GUILD_ENTER_NAME",
        code::TOO_SHORT => "ERR_GUILD_NAME_TOO_SHORT",
        code::MIXED_LANGUAGES => "ERR_GUILD_NAME_MIXED_LANGUAGES",
        code::PROFANE => "ERR_GUILD_NAME_PROFANE",
        code::RESERVED => "ERR_GUILD_NAME_RESERVED",
        code::INVALID_SPACE => "ERR_GUILD_NAME_INVALID_SPACE",
        code::CONSECUTIVE_SPACES => "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES",
        // 2, 3, 7, 8, 9 and 12 (`0x4f51df`).
        _ => "ERR_GUILD_NAME_INVALID",
    })
}

/// [`validate_guild_name`] against the lists and the charsets this VM's client holds.
fn check(model: &Model, name: &str) -> Result<(), &'static str> {
    validate_guild_name(name, &*model.glue.name_lists(), model.name_charsets())
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetPetitionNameInfo(index)` (`0x84cf94`, `0x4f453a`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: GetPetitionNameInfo(index)",
            )?);
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
    // The five that carry nothing, registered in the reference's table order; none touches the
    // snapshot, which the server owns. `GetTabardInfo`'s stock caller is commented out
    // (`GuildRegistrarFrame.xml:190`).
    let call = |call: PetitionCall| lua.create_function(move |lua, ()| submit(lua, call.clone()));
    g.set("TurnInGuildCharter", call(PetitionCall::TurnIn)?)?;
    // CloseGuildRegistrar() (`0x4f5010`) is the close core, which needs the interaction latch, so
    // it stays latch-driven ([`UiScript::take_registrar_close`]) and its `GUILD_REGISTRAR_CLOSED`
    // signals after the verb, where the reference's signals inside it (`0x4f503a`).
    g.set(
        "CloseGuildRegistrar",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .registrar_close = true;
            Ok(())
        })?,
    )?;
    // GetTabardInfo() (`0x4f5300`): the activate inside the call, then the registrar's close core
    // (`0x4f5355` → `0x4f5010`), latch-driven as `CloseGuildRegistrar` is, so its signal comes
    // after the verb, not inside it (`0x4f503a`).
    g.set(
        "GetTabardInfo",
        lua.create_function(|lua, ()| {
            submit(lua, PetitionCall::TabardInfo)?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .registrar_close = true;
            Ok(())
        })?,
    )?;
    g.set("OfferPetition", call(PetitionCall::Offer)?)?;
    g.set("ClosePetition", call(PetitionCall::Close)?)?;

    // SignPetition([n]) (`0x4f46d0`): the optional argument rides the wire as a byte defaulting to
    // 1, not 0 (`0x4f46d9`, `0x4f4749 Put8`); the server skips it.
    g.set(
        "SignPetition",
        lua.create_function(|lua, n: Value| {
            // `lua_isnumber` (`0x4f46e2`) gates the read, a miss keeping the 1 (`0x4f46d9`); `_ftol`
            // truncates toward zero, then the `Put8` keeps the low byte.
            let byte = crate::script::binding_abi::is_number(lua, n)
                .map_or(1i8, |v| crate::script::binding_abi::ftol(v) as i8);
            submit(lua, PetitionCall::Sign(byte))
        })?,
    )?;

    // BuyGuildCharter(guildName) (`0x4f5260`) returns 1 or nil for name validity, not for a send:
    // `lua_isstring` (`0x4f5268`) else its usage line (`0x84d090`), then the shared validator
    // (`0x4f5294`), whose refusal shows inside the call, and the purchase behind it.
    g.set(
        "BuyGuildCharter",
        lua.create_function(|lua, name: Value| {
            let name = binding_abi::string_arg(lua, name, "Usage: BuyGuildCharter(guildName)")?;
            let checked = check(&lua.app_data_ref::<Model>().expect("model app_data"), &name);
            match checked {
                Ok(()) => {
                    submit(lua, PetitionCall::Buy(name))?;
                    Ok(Value::Integer(1))
                }
                Err(key) => {
                    submit(lua, PetitionCall::NameRefused(key))?;
                    Ok(Value::Nil)
                }
            }
        })?,
    )?;

    // RenamePetition(name) (`0x4f4930`): zero values; `lua_isstring` (`0x4f493f`) else the usage
    // line as shipped, unbalanced (`0x84cfec`); then the host tests the open item before the
    // name's answer counts (`0x4f495e`-`0x4f4970`).
    g.set(
        "RenamePetition",
        lua.create_function(|lua, name: Value| {
            let name = binding_abi::string_arg(lua, name, "Usage(RenamePetition(\"name\")")?;
            let refusal = check(&lua.app_data_ref::<Model>().expect("model app_data"), &name).err();
            submit(lua, PetitionCall::Rename { name, refusal })
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
        s.keep_calls();
        s.run("SignPetition(); SignPetition(7); SignPetition(3.9)")
            .unwrap();
        assert_eq!(
            s.take_petition_calls(),
            vec![
                PetitionCall::Sign(1),
                PetitionCall::Sign(7),
                PetitionCall::Sign(3),
            ],
            "absent = 1, present = truncated toward zero"
        );
    }

    /// A refused name hands its message to the host instead of a packet; the rename hands both,
    /// for the host to weigh against the open petition (`0x4f4970`).
    #[test]
    fn the_name_validator_gates_both_verbs() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
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
            s.take_petition_calls(),
            vec![
                PetitionCall::Buy("Legacy".into()),
                PetitionCall::NameRefused("ERR_GUILD_ENTER_NAME"),
                PetitionCall::Rename {
                    name: "Legacy".into(),
                    refusal: None
                },
                PetitionCall::Rename {
                    name: "  spaced  ".into(),
                    refusal: Some("ERR_GUILD_NAME_INVALID_SPACE")
                },
            ]
        );
    }

    /// Both verbs test argument 1 with `lua_isstring` (`0x6f3510`, `0x4f5268`, `0x4f493f`), which a
    /// number passes; nil, a boolean or a table raises the binding's own line (`0x84d090` at
    /// `0x4f5271`, `0x84cfec` at `0x4f4948`), the rename's unbalanced as shipped.
    #[test]
    fn a_non_string_name_raises_each_verbs_usage_line() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let raised = |s: &UiScript, call: &str| -> String {
            s.eval::<String>(&format!(
                "local ok, e = pcall(function() {call} end) \
                 if ok then return \"<no raise>\" end return tostring(e)"
            ))
            .unwrap()
        };
        for arg in ["", "nil", "true", "{}"] {
            let got = raised(&s, &format!("BuyGuildCharter({arg})"));
            assert!(
                got.contains("Usage: BuyGuildCharter(guildName)"),
                "BuyGuildCharter({arg}): {got}"
            );
            let got = raised(&s, &format!("RenamePetition({arg})"));
            assert!(
                got.contains("Usage(RenamePetition(\"name\")"),
                "RenamePetition({arg}): {got}"
            );
        }
        assert_eq!(raised(&s, "BuyGuildCharter(5)"), "<no raise>");
        assert_eq!(raised(&s, "RenamePetition(5)"), "<no raise>");
    }

    /// One word per list, so a test can tell which lists a check searches.
    struct ThreeWords;

    impl crate::script::NameLists for ThreeWords {
        fn names_profanity(&self, name: &str) -> bool {
            name.to_ascii_lowercase().contains("darn")
        }

        fn chat_profanity(&self, name: &str) -> bool {
            name.to_ascii_lowercase().contains("heck")
        }

        fn names_reserved(&self, name: &str) -> bool {
            name.to_ascii_lowercase().contains("thrall")
        }
    }

    /// What `BuyGuildCharter(name)` queues: the buy, or the refusal's key.
    fn bought(s: &mut UiScript, name: &str) -> PetitionCall {
        s.run(&format!("BuyGuildCharter({name:?})")).unwrap();
        let mut requests = s.take_petition_calls();
        assert_eq!(requests.len(), 1, "{name:?}: {requests:?}");
        requests.remove(0)
    }

    /// The guild name check `0x6c9b70`: the validator `0x6c96c0` at the client's locale
    /// (`[0xc0e080]`, `0x4f5162`) with a space as its one extra character (`0x6c9b88`), the
    /// NamesProfanity and ChatProfanity lists on and NamesReserved off (`0x6c9b76`-`0x6c9b7a`), and
    /// a cap of 24 units (`0x6c9b9e`, code 2); `0x4f51ec` maps each code to its message.
    #[test]
    fn the_guild_name_check_is_the_clients() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_glue_name_lists(std::rc::Rc::new(ThreeWords));
        let refused = |key: &'static str| PetitionCall::NameRefused(key);
        for (name, want) in [
            ("", refused("ERR_GUILD_ENTER_NAME")),
            ("A", refused("ERR_GUILD_NAME_TOO_SHORT")),
            // A space is the one extra (`0x6c9851`): never first, never last, never doubled.
            (" ", refused("ERR_GUILD_NAME_INVALID_SPACE")),
            (" Legacy", refused("ERR_GUILD_NAME_INVALID_SPACE")),
            ("Legacy ", refused("ERR_GUILD_NAME_INVALID_SPACE")),
            ("A ", refused("ERR_GUILD_NAME_INVALID_SPACE")),
            (
                "Legacy  of",
                refused("ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES"),
            ),
            ("Ab  ", refused("ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES")),
            // Outside the charset (3), the apostrophe included, and three alike (9) are the
            // generic message (`0x4f51df`).
            ("Legacy1", refused("ERR_GUILD_NAME_INVALID")),
            ("Leg'acy", refused("ERR_GUILD_NAME_INVALID")),
            ("Legacy_", refused("ERR_GUILD_NAME_INVALID")),
            ("Leeegacy", refused("ERR_GUILD_NAME_INVALID")),
            ("BaAa", refused("ERR_GUILD_NAME_INVALID")),
            ("李小龙", refused("ERR_GUILD_NAME_INVALID")),
            // 25 units is over the cap (2); 24 is not.
            (
                "Abcdefghij Abcdefghij Abc",
                refused("ERR_GUILD_NAME_INVALID"),
            ),
            (
                "Abcdefghij Abcdefghij Ab",
                PetitionCall::Buy("Abcdefghij Abcdefghij Ab".into()),
            ),
            // A match in either profanity list is RESERVED (`0x6c9a51`, then `0x4f51b8`); the
            // reserved list is not searched.
            ("Darnwood", refused("ERR_GUILD_NAME_RESERVED")),
            ("Heckwood", refused("ERR_GUILD_NAME_RESERVED")),
            ("Thrallwood", PetitionCall::Buy("Thrallwood".into())),
            (
                "Legacy of Steel",
                PetitionCall::Buy("Legacy of Steel".into()),
            ),
            ("Éclair", PetitionCall::Buy("Éclair".into())),
            ("Ab", PetitionCall::Buy("Ab".into())),
        ] {
            assert_eq!(bought(&mut s, name), want, "{name:?}");
        }
        // The walk decides by position: a digit before a doubled space is INVALID.
        assert_eq!(bought(&mut s, "A1  b"), refused("ERR_GUILD_NAME_INVALID"));
        // RenamePetition runs the same check (`0x4f4987`).
        s.run("RenamePetition('A')").unwrap();
        assert_eq!(
            s.take_petition_calls(),
            vec![PetitionCall::Rename {
                name: "A".into(),
                refusal: Some("ERR_GUILD_NAME_TOO_SHORT")
            }]
        );
    }

    /// The charsets are the client's locale's (`0x6c9a70`): a zhCN client takes ideographs, and
    /// the cap stays 24 whatever the charset.
    #[test]
    fn the_guild_name_check_reads_the_locales_charsets() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_locale(4);
        assert_eq!(bought(&mut s, "李小龙"), PetitionCall::Buy("李小龙".into()));
        assert_eq!(
            bought(&mut s, "李a"),
            PetitionCall::NameRefused("ERR_GUILD_NAME_INVALID")
        );
        let long: String = ["一", "二", "三", "四", "五", "六", "七", "八"].concat();
        assert_eq!(bought(&mut s, &long), PetitionCall::Buy(long.clone()));
    }

    /// `GetTabardInfo()` pushes nothing (`0x4f535f`), hands the host the designer's opener and
    /// leaves the registrar's close to the latch (`0x4f5355`).
    #[test]
    fn get_tabard_info_is_a_call_with_no_returns() {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        assert_eq!(
            s.eval::<i64>("local n = function(...) return arg.n end return n(GetTabardInfo())")
                .unwrap(),
            0
        );
        assert_eq!(s.take_petition_calls(), vec![PetitionCall::TabardInfo]);
        assert!(s.take_registrar_close());
        assert!(!s.take_registrar_close(), "taken");
    }
}
