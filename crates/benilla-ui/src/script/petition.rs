//! The guild-charter **Era API surface** — the guild registrar and the petition window
//! (**re-derived against the byte law in 1678**).
//!
//! [`super::guild`] is *being* in a guild; this is *founding* one, which 1257 §2 deliberately left
//! out and named as the next slice. Fifteen registered globals across two windows (the three tabard
//! ones are the adjacent family and are not built), and the same shape as every other domain here:
//! the app pushes a [`PetitionState`] snapshot ([`UiScript::set_petition`]) and the getters read
//! that plain data; every verb queues a [`PetitionRequest`] the app drains
//! ([`UiScript::take_petition_requests`]). No ECS or net reach from the engine.
//!
//! Every contract below is byte-verified against the whole `PetitionInfo.cpp` TU (`0x84cfb8`).
//! Where it corrected what this file first shipped, the correction is named on the binding.
//!
//! **The snapshot mirrors the module's own `.data` state, and that is why it looks split.** The
//! real client keeps the signature list (`[0xbdce20]` + the `0x10`-stride signer array) and the
//! cached `CGPetition` record (`[0xbdce28]`) as two independent things, because the bindings read
//! them independently: `GetNumPetitionNames`/`GetPetitionNameInfo` answer off the *packet's* list
//! while `GetPetitionInfo` answers off the *record*, and either can exist without the other. So
//! [`PetitionState`] carries them separately rather than as one "is the window open" struct — a
//! collapsed model cannot express `CanSignPetition`'s no-record leg below, and gets it wrong.
//!
//! **This client has no `lua_pushboolean`.** Every predicate is the number `1` or `nil`
//! (`0x6f3810` / `0x6f37f0`), and a name the `NameCache` has not resolved is **`nil`**, not `""`.
//!
//! **`PETITION_SHOW` is deferred, which is what makes the partial states below unreachable in
//! practice.** The event fires only when no signer name is still resolving *and* the record has
//! arrived (`0x4f419b`-`0x4f41ad`), so the window never paints a blank title or a blank row. The
//! deferral lives app-side in `crate::ui_petition`; the getters here still answer honestly at any
//! moment, because an addon may call them whenever it likes.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// `GetPetitionInfo`'s first return when the record's `"charter"` bit is SET — `0x84cf8c`, selected
/// by `0x4f43fb test BYTE PTR [edi+0x1110],0x1`. The literal the reference compares against
/// (`PetitionFrame.lua:22`).
pub const PETITION_TYPE_CHARTER: &str = "charter";

/// …and when it is clear — `0x84b8f8`. **Reachable**, not dead: the bit is the record's own, so a
/// server that sent a non-charter petition would land here, and `CanSignPetition`'s guild-membership
/// and full-charter refusals are *both* gated on the same bit. 1.12 servers only ever send charters,
/// which is why the reference's `else` arm merely writes the bare word.
pub const PETITION_TYPE_PETITION: &str = "petition";

/// The cached `CGPetition` record, as [`GetPetitionInfo`] reads it — `[0xbdce28]`'s six fields.
///
/// `None` on [`PetitionState::record`] is the no-record leg, which is a real state with its own
/// return tuple; see [`GetPetitionInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionRecordView {
    /// [`PETITION_TYPE_CHARTER`] or [`PETITION_TYPE_PETITION`] — the record's `+0x1110` bit 0.
    pub petition_type: String,
    /// The record's inline `char[0x100]` title (`+0x10`) — the proposed guild's name.
    pub title: String,
    /// The record's inline `char[0x1000]` body text (`+0x110`). Empty on every 1.12 server.
    pub body_text: String,
    /// The record's `+0x1118`, pushed as a **signed** i32 (`fild DWORD`, not the zero-extending
    /// `fild QWORD` the two counters use).
    ///
    /// **That this field is the signature cap is settled inside the binary rather than inferred
    /// from the packet's field order**: `CanSignPetition` refuses at `0x4f4634 cmp ecx,[esi+0x1118]`
    /// against the live signature count. It is *not* the nine name rows — see the module doc of
    /// `crate::ui_petition`.
    pub max_signatures: i32,
    /// The owner's name through the `NameCache` — **`nil` when uncached** (`0x4f446d`), never `""`.
    pub originator: Option<String>,
    /// Whether the active player's guid equals the record's owner (`0x4f447a`/`0x4f4481`).
    pub is_originator: bool,
}

/// What the two charter windows read — the module's `.data` state, in the same three pieces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionState {
    /// `GetGuildCharterCost()` — showlist **entry\[0\]**'s fourth dword (`[0xbdce50]`), in copper.
    ///
    /// **Unsigned**, because the binding pushes it through the `fild QWORD` idiom with the high
    /// dword forced to zero (`0x4f5245`): a negative `charterCost` on the wire surfaces in Lua as
    /// ~4.29e9, not as a negative number. `0` before any showlist has arrived, and after the
    /// world-enter clear.
    ///
    /// That it is **copper** is settled without going through the UI: `0x4f50ed` compares this cell
    /// directly against `PLAYER_FIELD_COINAGE` and refuses with `ERR_NOT_ENOUGH_MONEY`.
    pub charter_cost: u32,
    /// The open petition's signers in wire order, each resolved through the `NameCache` — `None`
    /// where the name has not landed. Its length is `GetNumPetitionNames()`, which counts
    /// **signatures only**: the petition's owner is not among them.
    ///
    /// Independent of [`Self::record`] on purpose (module doc): the packet fills this, the cache
    /// fills that, and the bindings read one each.
    pub signers: Vec<Option<String>>,
    /// The cached record, or `None` — the no-record leg.
    pub record: Option<PetitionRecordView>,
    /// `CanSignPetition()`, computed app-side because three of its four refusals need state the
    /// engine does not hold. See [`PetitionState::can_sign`]'s own note in `crate::ui_petition`;
    /// the shape to remember here is that **it is `1` when nothing is open at all**.
    pub can_sign: bool,
}

/// A charter intent queued from Lua, drained by the app into its send.
///
/// **Almost every one is fire-and-forget, and that is the wire's shape**: buying is answered only by
/// the item appearing, offering is answered to the *target*, and a refusal comes back on the guild
/// family's own error channel. Nothing here may update local state optimistically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetitionRequest {
    /// `BuyGuildCharter(name)` — the registrar's Purchase button, **after** the name passed
    /// [`validate_guild_name`]. The app supplies the NPC guid from the latched registrar, which is
    /// the petition module's own `[0xbdceb0]` and *not* `CGGameUI`'s interaction pair.
    Buy(String),
    /// `TurnInGuildCharter()` — no argument. The app scans the bags, because the real client does
    /// (`0x5ef2b0`, and it latches nothing: it re-scans on every call).
    TurnIn,
    /// `CloseGuildRegistrar()` — **sends nothing of its own**, VERIFIED by a closure walk over
    /// `0x4f5010` that finds no `CDataStore` build and no send, with this TU's own four sends as
    /// the positive control.
    CloseRegistrar,
    /// `SignPetition([n])` — the optional argument is a **byte on the wire** and defaults to `1`
    /// (`0x4f46d9`), not to `0`. vmangos skips it, so only a golden can tell the difference.
    Sign(i8),
    /// `OfferPetition()` — no argument; the app resolves the **current target** (`CGGameUI`'s
    /// *selection* pair, not its interaction pair) and runs the eight guards.
    Offer,
    /// `RenamePetition(name)` — after [`validate_guild_name`].
    Rename(String),
    /// `ClosePetition()` — **and this one can put bytes on the wire.** `0x4f3f60`'s decline leg
    /// sends `MSG_PETITION_DECLINE` whenever a petition was open, no sign is in flight, a record is
    /// cached, and we are **not** its owner. The app decides that; the verb just says "close".
    ClosePetition,
    /// A name the client itself refused ([`validate_guild_name`]) — carries the GlobalStrings key
    /// of the message to show. **No packet is built**: the real client's validator runs before the
    /// send and emits through the message catalog on its own.
    NameRefused(&'static str),
}

impl super::UiScript {
    /// Push the charter snapshot, replacing whatever was there. A bare setter — firing the four
    /// events on their edges (and *deferring* `PETITION_SHOW`) is the app's job.
    pub fn set_petition(&mut self, state: PetitionState) {
        self.model_mut().petition = state;
    }

    /// Take the charter intents queued since the last drain.
    pub fn take_petition_requests(&mut self) -> Vec<PetitionRequest> {
        std::mem::take(&mut self.model_mut().petition_requests)
    }

    /// Drop any queued close intent, reporting how many went — **the close-intent consumption
    /// decision 0096 named**, and the one thing a window switch cannot work without.
    ///
    /// Firing `PETITION_CLOSED` runs the frame's `OnHide` synchronously, and that handler calls
    /// `ClosePetition()`, which queues a close. On a *switch* — a charter offered to us while ours
    /// is open — the feed fires `PETITION_CLOSED` then `PETITION_SHOW` for the new charter, and the
    /// close queued by the first would be drained onto the session the second just opened: the new
    /// charter's window flashes and shuts. **And here it would also put a `MSG_PETITION_DECLINE` on
    /// the wire for a charter we did not decline**, which is the sharper reason it must go.
    ///
    /// It cannot eat a *user's* close by mistake, and the ordering is what guarantees that rather
    /// than a flag: the feed runs `before(UiInput)`, so a click's own `OnHide` queues its close
    /// after this has already run.
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

    /// Queue one charter intent directly — the test seam.
    #[cfg(test)]
    pub fn queue_petition_request(&mut self, request: PetitionRequest) {
        self.model_mut().petition_requests.push(request);
    }
}

/// The client-side guild-name check `BuyGuildCharter` and `RenamePetition` **share** — `0x4f5160`,
/// which runs the name through the locale-aware string checker `0x6c9b70` and maps its code to one
/// of seven messages, accepting **only** code `0xd`. `Ok(())` is that acceptance; `Err(key)` is the
/// GlobalStrings key of the line to show, and **no packet is built**.
///
/// **Partial, deliberately, and here is exactly how far it goes.** The code space belongs to
/// `0x6c9b70`, which is not read in full, so only the checks whose meaning is unambiguous from
/// the message keys are implemented — empty, a leading or trailing space, and consecutive spaces.
/// The three that need data we do not have (`ERR_GUILD_NAME_TOO_SHORT`'s minimum,
/// `ERR_GUILD_NAME_PROFANE`'s word list, `ERR_GUILD_NAME_MIXED_LANGUAGES`' script rules) **pass**
/// rather than guess: refusing a name the server would have accepted is the worse failure, and the
/// server re-checks every one of them anyway (`ObjectMgr::IsValidCharterName`). What this buys over
/// sending blindly is the case the reference makes loudest and the server answers with silence —
/// clicking Purchase with an empty box.
///
/// The reference's full table of codes, with what each maps to:
///
/// | code | key | implemented |
/// |---|---|---|
/// | 0 | `ERR_GUILD_ENTER_NAME` | **yes** — the empty string |
/// | 1 | `ERR_GUILD_NAME_TOO_SHORT` | no — the minimum is inside `0x6c9b70` |
/// | 4 | `ERR_GUILD_NAME_MIXED_LANGUAGES` | no |
/// | 5 | `ERR_GUILD_NAME_PROFANE` | no |
/// | 6 | `ERR_GUILD_NAME_RESERVED` | no |
/// | 10 | `ERR_GUILD_NAME_INVALID_SPACE` | **yes** — a leading or trailing space |
/// | 11 | `ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES` | **yes** |
/// | 2,3,7,8,9,>0xb | `ERR_GUILD_NAME_INVALID` | no |
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

/// 1.12's `1`/`nil`, never `true`/`false` — this client has no `lua_pushboolean`.
fn era_bool(on: bool) -> Value {
    if on {
        Value::Integer(1)
    } else {
        Value::Nil
    }
}

/// A cached name pushes as a string; an uncached one pushes **nil**.
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
    // GetPetitionInfo() (`0x4f43d0`) — **exactly six values on BOTH legs** (`mov eax,0x6` at
    // `0x4f4493` and `0x4f44d4`), in the order the reference destructures them
    // (`PetitionFrame.lua:10`).
    //
    // The no-record leg is NOT "return nothing": it is `nil, nil, nil, 0, nil, nil`, with the
    // fourth pushed as the *number* zero (`0x4f44b4 push 0; push 0; call 0x6f3810`). This file
    // first shipped an empty return, which reads identically through the reference's own
    // destructure and differently to anything that counts its arguments.
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
                era_bool(r.is_originator),
            ]))
        })?,
    )?;

    // GetNumPetitionNames() (`0x4f44e0`, 44 bytes — one field read) — `[0xbdce20]`, pushed
    // UNSIGNED. It counts **signatures only**; the owner is painted separately by the window and is
    // never in this list.
    g.set(
        "GetNumPetitionNames",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.petition.signers.len() as i64)
        })?,
    )?;

    // GetPetitionNameInfo(index) (`0x4f4510`) — **1-based** (`0x4f4569 dec eax`), one value on
    // every leg. The bound test `0x4f456c jae` is UNSIGNED, so index `< 1` wraps to `0xffffffff`
    // and fails the same comparison — which is why a zero or negative index answers nil rather
    // than the last row. An uncached name is **nil**, not `""`.
    //
    // A non-numeric argument is `luaL_error("Usage: GetPetitionNameInfo(index)")`, which longjmps;
    // mlua's coercion raises for us on the same input, so the shape is preserved.
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

    // CanSignPetition() (`0x4f45e0`) — the Sign button's only gate.
    //
    // **It returns `1` with NO petition open**, and that is the reference's own asymmetry rather
    // than a reading error: `0x4f45f7 je 0x4f4655` jumps past the three record-dependent refusals
    // straight into the signer scan, over an array the close path has already zeroed. A client that
    // treats this as a sufficient precondition lets the user click Sign with nothing to sign — so
    // the window's own `isOriginator` branch is what actually keeps the button off screen.
    g.set(
        "CanSignPetition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(era_bool(model.petition.can_sign))
        })?,
    )?;

    // ── The registrar window ─────────────────────────────────────────────────────────────────
    // GetGuildCharterCost() (`0x4f5230`) — copper, unsigned, `0` before any showlist.
    g.set(
        "GetGuildCharterCost",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.petition.charter_cost))
        })?,
    )?;

    // ── The verbs ────────────────────────────────────────────────────────────────────────────
    // The four that carry nothing and push nothing. None may touch the snapshot: none of them is
    // acknowledged, so a local update would show a charter as signed that the server refused.
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

    // SignPetition([n]) (`0x4f46d0`) — the argument is OPTIONAL and rides the wire as a byte,
    // defaulting to **1**, not 0 (`0x4f46d9 edi = 1`, `0x4f4749 Put8`). The server skips the byte,
    // so nothing observable depends on it — which is exactly why it is easy to ship as 0 and never
    // find out.
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

    // BuyGuildCharter(guildName) (`0x4f5260`) — **returns the number `1` or `nil`**, and what it
    // reports is NAME VALIDITY, not that a packet was sent: `0x4f5294` runs the shared validator and
    // only its non-zero return reaches the action, which then has five silent refusals of its own.
    // Nothing in the shipped FrameXML reads the return; an addon can.
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

    // RenamePetition(name) (`0x4f4930`) — **0 values**, the SAME validator, and silent on refusal
    // beyond the validator's own message. (Its usage string is literally
    // `Usage(RenamePetition("name")` in the binary — Blizzard's own unbalanced parenthesis. It is
    // unreachable from our binding, which raises through mlua's coercion instead, and is recorded
    // here rather than reproduced.)
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

    /// **The no-record leg is six values, not none** — `nil, nil, nil, 0, nil, nil`, with the
    /// fourth the *number* zero.
    ///
    /// This file first shipped an empty return, which the reference's own six-way destructure
    /// cannot tell apart (both give six nils… except the fourth, which is `0` here and `nil`
    /// there). The COUNT is what separates them — [`UiScript::arity`] — and it is the whole point
    /// of the test.
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

    /// The name list is 1-based and bounded, an uncached name is **nil**, and the count excludes
    /// the owner. The zero/negative cases matter because the binary's bound test is *unsigned*.
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

    /// `CanSignPetition()` is era-boolean **and answers `1` with nothing open** — the reference's
    /// own asymmetry, pinned so nobody "fixes" it into a nil.
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

    /// The cost is copper, unsigned, and belongs to the registrar rather than to any petition.
    #[test]
    fn charter_cost_is_unsigned_copper_and_the_registrars() {
        let mut s = UiScript::new().unwrap();
        s.set_petition(PetitionState {
            charter_cost: 1000,
            ..PetitionState::default()
        });
        assert_eq!(s.eval::<i64>("return GetGuildCharterCost()").unwrap(), 1000);

        // A negative wire cost surfaces as ~4.29e9, not as a negative — the `fild QWORD` idiom.
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

    /// `SignPetition`'s optional argument rides the wire as a byte and **defaults to 1**.
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

    /// `BuyGuildCharter` reports **name validity** as `1`/`nil`, and a refused name queues its
    /// message instead of a packet. `RenamePetition` shares the validator and returns nothing.
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

    /// The validator's three implemented codes, and — just as load-bearing — the names it lets
    /// through rather than guessing at.
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
        // Passed through on purpose: the minimum length, the profanity list and the script rules
        // all live inside `0x6c9b70`, which is not read in full, and refusing a name the server
        // would accept is the worse failure. The server re-checks every one of them.
        for ok in ["Legacy of Steel", "A", "Ab", "Éclair", "x y z"] {
            assert_eq!(
                validate_guild_name(ok),
                Ok(()),
                "{ok:?} passes to the server"
            );
        }
    }

    /// The close-intent consumption removes exactly the two close verbs and preserves the order of
    /// everything else.
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
