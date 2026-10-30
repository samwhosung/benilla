//! The inspect bindings: `NotifyInspect` and `ClearInspectPlayer` act inside their calls through
//! the host ([`InspectCall`]), the `GetInventoryItem*` verbs read another player's visible items
//! through the lent host ([`VisibleSlotQuery`]), and `CanInspect` and `CheckInteractDistance` read
//! the reach of the unit the token resolver names through it too ([`UnitReachQuery`]).
//!
//! Worn gear is public descriptor state (`PLAYER_VISIBLE_ITEM_<n>_0`), so the window paints from
//! what the client holds: `SMSG_INSPECT` echoes only the guid (`0x5e7d70`), `InspectFrame_Show`
//! calls `NotifyInspect` and `ShowUIPanel` together (`Blizzard_InspectUI.lua:6-13`), and the paper
//! doll reads all 19 slots on show, inside that call (`InspectPaperDollFrame.lua:57-79`), through
//! `InspectFrame.unit`, so it follows a re-target.

use std::cmp::Ordering;
use std::collections::HashMap;

use mlua::{Lua, Value};

use super::binding_abi;
use super::char_stats::InventorySlots;
use super::Model;

/// `CheckInteractDistance`'s squared thresholds for types 1..=4, `{10², 11.1111², 10², 30²}`
/// (`0x48ba00`, from the `.rdata` at `0x804498`, `0x804490`, `0x80448c`, `0x8044a4`).
pub const INTERACT_DIST_SQ: [f64; 4] = [100.0, 123.45678, 100.0, 900.0];

/// `CanInspect`'s threshold squared (`0xb4d918`, the `.rdata` 10.0 at `0x804498` squared by the
/// initialiser `0x48cc80`); vmangos checks the same 10 yards (`INSPECT_DISTANCE`, `ObjectDefines.h:26`).
pub const CAN_INSPECT_DIST_SQ: f64 = 100.0;

/// One unit this frame, as the two range verbs read it. An entry exists only for a live unit
/// object: the reference maps the token to a GUID (`0x515940`) and answers nil when the object
/// manager has no such unit, as for a party member out of the area, whose GUID comes from the
/// roster (`0x4e81a0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitReach {
    /// Squared distance from the player.
    pub dist_sq: f64,
    /// `0x4944a0`'s legs besides distance: a player unit that is the active player or one it can
    /// cooperate with (`CanCooperate 0x606ba0`, `0x4944e4`). Only `CanInspect` reads it:
    /// `CheckInteractDistance` is a pure distance test, so a creature or an enemy player still
    /// answers it.
    pub inspectable: bool,
}

impl super::UiScript {
    /// Seat the squared distance of the units a token can name, which both range verbs read with
    /// no host to ask ([`UnitReachQuery`]). A verb resolves its token to a guid through
    /// [`super::UnitGuids`], so `"party1target"` and `"PLAYER"` read the unit they name, and a
    /// guid absent from the map answers nil from both, the reference's null-object arms
    /// (`0x48babe` for `CheckInteractDistance`, `0x48a1fa` for `CanInspect`).
    pub fn set_unit_reach(&mut self, reach: HashMap<u64, UnitReach>) {
        self.model_mut().unit_reach = reach;
    }

    /// Seat one unit's reach, which the range verbs read with no host to ask ([`UnitReachQuery`]).
    pub fn set_unit_reach_of(&mut self, guid: u64, reach: UnitReach) {
        self.model_mut().unit_reach.insert(guid, reach);
    }

    /// The unit tokens that name a unit now ([`super::unit::bound_tokens_in`]): the set a field's
    /// change is fanned out to (`0x515e50`). The objects, the target and the interaction globals
    /// and the party and raid rows (`0x491920`, `0x491940`) are read through the lent host as of
    /// the place this runs, the other globals as the VM holds them; with no host to ask, the
    /// snapshot's ([`super::UnitGuids::bound_tokens`]).
    pub fn bound_unit_tokens(&self) -> Vec<String> {
        let model = self.model_ref();
        let guids = &model.unit_guids;
        let globals = super::unit::LiveGlobals::asked(self.lua(), guids);
        super::unit::with_live_objects(self.lua(), |o| super::unit::bound_tokens_in(&globals, o))
            .unwrap_or_else(|| guids.bound_tokens())
    }

    /// The tokens naming one unit as its event is signalled ([`super::unit::tokens_in`]), the list
    /// `0x515e50` signals a unit event for, read as [`Self::bound_unit_tokens`] reads.
    pub fn unit_tokens_of(&self, guid: u64) -> Vec<String> {
        let model = self.model_ref();
        let guids = &model.unit_guids;
        let globals = super::unit::LiveGlobals::asked(self.lua(), guids);
        super::unit::with_live_objects(self.lua(), |o| super::unit::tokens_in(&globals, o, guid))
            .unwrap_or_else(|| guids.tokens_of(guid))
    }

    /// Whether one unit carries the unit-field watchers now ([`super::unit::field_watchers_in`]),
    /// read as [`Self::bound_unit_tokens`] reads.
    pub fn unit_has_field_watchers(&self, guid: u64) -> bool {
        let model = self.model_ref();
        let guids = &model.unit_guids;
        let globals = super::unit::LiveGlobals::asked(self.lua(), guids);
        super::unit::with_live_objects(self.lua(), |o| {
            super::unit::field_watchers_in(&globals, o, guid)
        })
        .unwrap_or_else(|| guids.has_field_watchers(guid))
    }

    /// Push, or clear with `None`, the visible items of the player `guid` by slot id 1..=19:
    /// what the slot getters read through a token naming another player they may read
    /// (`0x4944a0`) when no host is free to ask ([`VisibleSlotQuery`]). It fires nothing: a change
    /// of them is the visible-item bridge's `UNIT_INVENTORY_CHANGED` (`0x51bd70`).
    pub fn set_visible_equipment(&mut self, guid: u64, slots: Option<InventorySlots>) {
        let mut model = self.model_mut();
        match slots {
            Some(slots) => model.visible_equipment.insert(guid, slots),
            None => model.visible_equipment.remove(&guid),
        };
    }

    /// The tokens `NotifyInspect` named, which a harness's VM kept ([`Self::keep_calls`]).
    pub fn take_inspect_notifies(&mut self) -> Vec<String> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::Inspect(InspectCall::Notify(token)) => Some(token.clone()),
            _ => None,
        })
    }

    /// Whether a harness's VM kept a `ClearInspectPlayer` ([`Self::keep_calls`]).
    pub fn take_inspect_clear(&mut self) -> bool {
        self.take_calls_like(&super::ScriptCall::Inspect(InspectCall::Clear)) > 0
    }
}

/// A slot getter's question of another player's visible items, through the lent host
/// ([`super::live::ask`]): the slot, id 1..=19, as the object's `PLAYER_VISIBLE_ITEM_<slot>` block
/// holds it at the call (`0x4c8328` → `0x5f0d60`). The answer is `None` for an empty slot or a
/// player no longer held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibleSlotQuery {
    pub guid: u64,
    pub slot: u8,
}

impl super::live::LiveQuery for VisibleSlotQuery {
    type Answer = Option<super::InvSlotView>;
}

/// The visible item of the player `guid` at slot id `slot`, read through the lent host as of the
/// call; with no host free to ask, the snapshot ([`super::UiScript::set_visible_equipment`]).
pub(super) fn visible_slot(
    lua: &Lua,
    model: &Model,
    guid: u64,
    slot: usize,
) -> Option<super::InvSlotView> {
    let query = VisibleSlotQuery {
        guid,
        slot: u8::try_from(slot).ok()?,
    };
    match super::live::ask(lua, &query) {
        Some(answer) => answer,
        None => model.visible_equipment.get(&guid)?.get(slot)?.clone(),
    }
}

/// An inspect verb the host applies inside its call ([`super::ScriptCall::Inspect`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectCall {
    /// `NotifyInspect(unit)` (`0x48a310` → `0x5decc0`): `CMSG_INSPECT` for the unit's guid, which
    /// also sets the server-side selection (`MiscHandler.cpp:945`), then the inspect target set to
    /// it (`0x5ded10`, `0x5ded1c`).
    Notify(String),
    /// `ClearInspectPlayer()` (`0x4c9780` → `0x4c6f70(0)`): the inspect target dropped.
    Clear,
    /// `RequestInspectHonorData()` past its two flags (`0x4c80a6`, `0x4c80b6`): with a target,
    /// `MSG_INSPECT_HONOR_STATS` for it (`0x4c8112`).
    Honor,
}

/// The range verbs' question of the lent host ([`super::live::ask`]): the unit `guid` as the
/// object manager holds it at the call (`0x468460`, typemask 8), its squared distance from the
/// active player and `0x4944a0`'s other legs, read there (`0x48a26f`, `0x48ba00`); `None` for no
/// unit held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitReachQuery(pub u64);

impl super::live::LiveQuery for UnitReachQuery {
    type Answer = Option<UnitReach>;
}

/// `guid`'s reach at the call: the lent host's, else the seat ([`super::UiScript::set_unit_reach`]).
pub(super) fn reach_of(lua: &Lua, model: &Model, guid: u64) -> Option<UnitReach> {
    super::live::ask(lua, &UnitReachQuery(guid))
        .unwrap_or_else(|| model.unit_reach.get(&guid).copied())
}

/// The reach of the unit `token` names, through the resolver (`0x515970`, which both verbs call via
/// `0x515940`): the guid it answers, then that guid's reach. A token the resolver does not
/// recognise raises `Unknown unit name` (`0x515c14`).
fn reach(lua: &Lua, token: &str) -> mlua::Result<Option<UnitReach>> {
    super::unit::check_unit_token(&Some(token.to_string()))?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let guid = super::unit::guid_of(lua, &model, token)?;
    Ok(guid.and_then(|guid| reach_of(lua, &model, guid)))
}

/// Whether `0x4944a0`'s distance leg passes: in range unless `100.0 < d²`, since `test ah,0x41`
/// after the compare counts an unordered one as in range, so a NaN distance passes (`0x494531`),
/// as `CanInspect`'s own test before it does (`0x48a28b`).
pub(super) fn in_inspect_range(r: &UnitReach) -> bool {
    matches!(
        r.dist_sq.partial_cmp(&CAN_INSPECT_DIST_SQ),
        Some(Ordering::Less | Ordering::Equal) | None
    )
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CanInspect(unit) → 1/nil (`0x48a1b0`), the gate of `InspectFrame_Show`
    // (`Blizzard_InspectUI.lua:8`). `Usage:` unless the argument is a string or a number
    // (`0x48a1ca`); the resolver raises for an unknown token. Each refusal shows its error through
    // `0x496720` before the answer, which is `0x4944a0`'s (`0x48a2a1`): no object
    // `ERR_UNIT_NOT_FOUND`, or `ERR_GENERIC_NO_TARGET` for an empty token (`0x48a2d6`-`0x48a2f3`);
    // no active player, a unit that is no player, or another one the player cannot cooperate with
    // `ERR_INVALID_INSPECT_TARGET` (`0x48a21f`, `0x48a231`, `0x48a245` → `0x48a2c7`); beyond 10
    // yards `ERR_OUT_OF_RANGE` (`0x48a292`). Those legs ride in [`UnitReach::inspectable`].
    g.set(
        "CanInspect",
        lua.create_function(|lua, unit: Value| {
            let unit = binding_abi::string_arg(lua, unit, "Usage: CanInspect(unit)")?;
            let reach = reach(lua, &unit)?;
            let refusal = match &reach {
                None if unit.starts_with('\0') || unit.is_empty() => Some("ERR_GENERIC_NO_TARGET"),
                None => Some("ERR_UNIT_NOT_FOUND"),
                Some(r) if !r.inspectable => Some("ERR_INVALID_INSPECT_TARGET"),
                Some(r) if !in_inspect_range(r) => Some("ERR_OUT_OF_RANGE"),
                Some(_) => None,
            };
            if let Some(key) = refusal {
                super::calls::display_error(lua, key)?;
            }
            let ok = reach.is_some_and(|r| r.inspectable && in_inspect_range(&r));
            Ok(if ok { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // CheckInteractDistance(unit, type) → 1/nil (`0x48ba00`): in range only when
    // `d² < table[type-1]`, strictly, unlike `CanInspect` (`test ah,0x5; jp`). `is-number-or-string`
    // on the unit and `is-number` on `type` gate both, else
    // `Usage: CheckInteractDistance("unit", distIndex)` (`0x842900`, `0x48bb42`); then the resolver
    // raises for an unknown token (`0x48ba7e`). A token with no unit answers nil (`0x48babe`), as
    // does a `type` outside 1..=4 once truncated toward zero, the compare being unsigned
    // (`0x48bac5`); no arm shows an error.
    g.set(
        "CheckInteractDistance",
        lua.create_function(|lua, (unit, kind): (Value, Value)| {
            const USAGE: &str = r#"Usage: CheckInteractDistance("unit", distIndex)"#;
            let unit = binding_abi::string_arg(lua, unit, USAGE)?;
            let kind = binding_abi::index_arg(lua, kind, USAGE)?;
            let thr = kind
                .checked_sub(1)
                .and_then(|i| INTERACT_DIST_SQ.get(i).copied());
            let ok = match (reach(lua, &unit)?, thr) {
                (Some(r), Some(thr)) => r.dist_sq < thr,
                _ => false,
            };
            Ok(if ok { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // NotifyInspect(unit) (`Blizzard_InspectUI.lua:9`): fire-and-forget, as nothing waits on the
    // reply. `is-number-or-string` else `Usage: NotifyInspect(unit)` (`0x842724`, `0x48a38b`);
    // with no player object it returns (`0x48a35a`), else the token goes through the resolver
    // (`0x48a36a` → `0x515970`), which raises `Unknown unit name` for one it does not know.
    g.set(
        "NotifyInspect",
        lua.create_function(|lua, unit: Value| {
            let unit = binding_abi::string_arg(lua, unit, "Usage: NotifyInspect(unit)")?;
            if !super::battlefield_queue::player_exists(
                lua,
                &lua.app_data_ref::<Model>().expect("model app_data"),
            ) {
                return Ok(());
            }
            super::unit::check_unit_token(&Some(unit.clone()))?;
            super::calls::submit(lua, super::ScriptCall::Inspect(InspectCall::Notify(unit)))
        })?,
    )?;

    // ClearInspectPlayer(): called from `InspectFrame_OnHide` (`Blizzard_InspectUI.lua:58`).
    g.set(
        "ClearInspectPlayer",
        lua.create_function(|lua, ()| {
            super::calls::submit(lua, super::ScriptCall::Inspect(InspectCall::Clear))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::script::{UiScript, UnitGuids};

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0001;
    const P1: u64 = 0x21;
    const P1_PET: u64 = 0xF140_0000_0000_0021;
    const OUT: u64 = 0x44;

    fn reach(dist_sq: f64, inspectable: bool) -> UnitReach {
        UnitReach {
            dist_sq,
            inspectable,
        }
    }

    /// Us at the origin targeting a mob 20 yards off, which targets party1 5 yards off, who
    /// targets us; party1's pet targets nobody. `OUT` is a roster member with no object.
    fn seeded() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            pet: P1_PET,
            target: MOB,
            mouseover: MOB,
            party: [P1, OUT, 0, 0],
            party_pets: [P1_PET, 0, 0, 0],
            raid: vec![ME, P1],
            held: HashMap::from([(ME, MOB), (MOB, P1), (P1, ME), (P1_PET, 0)]),
            ..Default::default()
        });
        s.set_unit_reach(HashMap::from([
            (ME, reach(0.0, true)),
            (MOB, reach(400.0, false)),
            (P1, reach(25.0, true)),
            (P1_PET, reach(2500.0, false)),
        ]));
        s
    }

    fn answers(s: &UiScript, expr: &str) -> bool {
        s.eval::<bool>(&format!("return {expr} ~= nil")).unwrap()
    }

    /// A token reads the reach of the unit it names, the `target` chain included: the verbs
    /// resolve through [`UnitGuids`], so a chain is asked for no entry of its own.
    #[test]
    fn a_target_chain_reads_the_reach_of_the_unit_it_names() {
        let s = seeded();
        // The base, then each hop: MOB is 20 yards off, outside the 10-yard row, inside the 30.
        assert!(!answers(&s, r#"CheckInteractDistance("target", 1)"#));
        assert!(answers(&s, r#"CheckInteractDistance("target", 4)"#));
        assert!(!answers(&s, r#"CheckInteractDistance("playertarget", 1)"#));
        assert!(answers(&s, r#"CheckInteractDistance("playertarget", 4)"#));
        // party1 is 5 yards off, and so is whom the target targets.
        assert!(answers(&s, r#"CheckInteractDistance("targettarget", 1)"#));
        assert!(answers(&s, r#"CheckInteractDistance("party1", 1)"#));
        // party1 targets us: distance 0, where `party1` itself is 5 yards.
        assert!(answers(&s, r#"CheckInteractDistance("party1target", 1)"#));
        assert!(answers(&s, r#"CheckInteractDistance("raid2target", 1)"#));
        assert!(answers(
            &s,
            r#"CheckInteractDistance("targettargettarget", 1)"#
        ));
        // The mouseover is the mob, so its target is party1 again.
        assert!(answers(
            &s,
            r#"CheckInteractDistance("mouseovertarget", 1)"#
        ));
        // Four hops come back round to the mob.
        assert!(!answers(
            &s,
            r#"CheckInteractDistance("targettargettargettarget", 1)"#
        ));
        // The pet is 50 yards off, outside every row.
        assert!(!answers(&s, r#"CheckInteractDistance("partypet1", 4)"#));
    }

    /// The other range verb reads the same entry, and its player-only refusal rides in it.
    #[test]
    fn can_inspect_reads_the_chain_too() {
        let s = seeded();
        // party1 is a player in range, and the mob is a creature, which inspect refuses.
        assert!(answers(&s, r#"CanInspect("party1")"#));
        assert!(answers(&s, r#"CanInspect("targettarget")"#));
        assert!(answers(&s, r#"CanInspect("party1target")"#));
        assert!(!answers(&s, r#"CanInspect("target")"#));
        assert!(!answers(&s, r#"CanInspect("party1targettarget")"#));
    }

    /// A chain that ends at nobody, a unit with no object and text after the chain each answer
    /// nil, a token the resolver does not know raises, and the compares fold case.
    #[test]
    fn a_chain_that_names_nobody_answers_nil_and_case_folds() {
        let s = seeded();
        for token in [
            // The pet targets nobody, so a hop off it names nobody.
            "partypet1target",
            "pettarget",
            // A roster member with no object, and a hop off it.
            "party2",
            "party2target",
            "party3target",
            "party1foo",
            "party1targetfoo",
            "",
        ] {
            let call = format!(r#"CheckInteractDistance("{token}", 4)"#);
            assert!(!answers(&s, &call), "{call}");
            let call = format!(r#"CanInspect("{token}")"#);
            assert!(!answers(&s, &call), "{call}");
        }
        // `npc` is one exact compare with no chain, so the resolver knows no `npctarget`.
        for call in [
            r#"CheckInteractDistance("npctarget", 4)"#,
            r#"CanInspect("npctarget")"#,
        ] {
            assert!(s.run(call).is_err(), "{call}");
        }
        assert!(answers(&s, r#"CheckInteractDistance("PARTY1TARGET", 1)"#));
        assert!(answers(&s, r#"CanInspect("Party1Target")"#));
    }

    /// `CanInspect` (`0x48a1b0`) raises `Usage: CanInspect(unit)` unless its argument is a string
    /// or a number (`0x48a1ca`), and the resolver raises for a token it does not know (`0x515940`
    /// through `0x515970`). Before it answers by `0x4944a0` (`0x48a2a1`), each refusal shows its
    /// error through `0x496720`: no object `ERR_UNIT_NOT_FOUND` (`0x48a2df`), or
    /// `ERR_GENERIC_NO_TARGET` for an empty token (`0x48a2ee`); a unit that is no player, or
    /// another one the player cannot cooperate with, `ERR_INVALID_INSPECT_TARGET` (`0x48a2c7`); one
    /// beyond 10 yards `ERR_OUT_OF_RANGE` (`0x48a292`). An inspectable unit in range shows none.
    #[test]
    fn can_inspect_raises_and_shows_each_refusal_before_it_answers() {
        let mut s = seeded();
        for chunk in ["CanInspect()", "CanInspect({})", "CanInspect(nil)"] {
            let err = s.run(chunk).expect_err(chunk).to_string();
            assert!(err.contains("Usage: CanInspect(unit)"), "{chunk}: {err}");
        }
        for chunk in ["CanInspect(\"bogus\")", "CanInspect(5)"] {
            let err = s.run(chunk).expect_err(chunk).to_string();
            assert!(err.contains("Unknown unit name"), "{chunk}: {err}");
        }
        assert!(s.take_ui_errors().is_empty(), "a raise shows nothing");
        for (token, shown) in [
            ("party1", None),
            ("party2", Some("ERR_UNIT_NOT_FOUND")),
            ("party3", Some("ERR_UNIT_NOT_FOUND")),
            ("", Some("ERR_GENERIC_NO_TARGET")),
            ("target", Some("ERR_INVALID_INSPECT_TARGET")),
            ("partypet1", Some("ERR_INVALID_INSPECT_TARGET")),
        ] {
            let call = format!(r#"CanInspect("{token}")"#);
            assert_eq!(answers(&s, &call), shown.is_none(), "{call}");
            assert_eq!(s.take_ui_errors(), Vec::from_iter(shown), "{call}");
        }
        // party1 20 yards off: inspectable, out of range.
        s.set_unit_reach(HashMap::from([(P1, reach(400.0, true))]));
        assert!(!answers(&s, r#"CanInspect("party1")"#));
        assert_eq!(s.take_ui_errors(), vec!["ERR_OUT_OF_RANGE"]);
    }

    /// `CheckInteractDistance` (`0x48ba00`) checks both arguments first (`0x48ba11`-`0x48ba2c`),
    /// then the resolver raises for a token it does not know (`0x48ba7e` → `0x515940`); it shows
    /// no error on any arm.
    #[test]
    fn check_interact_distance_raises_for_an_unknown_token_and_shows_nothing() {
        let mut s = seeded();
        let err = s
            .run(r#"CheckInteractDistance("bogus", 1)"#)
            .expect_err("an unknown token")
            .to_string();
        assert!(err.contains("Unknown unit name"), "{err}");
        let err = s
            .run(r#"CheckInteractDistance("bogus", {})"#)
            .expect_err("a bad index")
            .to_string();
        assert!(err.contains("Usage: CheckInteractDistance"), "{err}");
        assert!(!answers(&s, r#"CheckInteractDistance("party2", 4)"#));
        assert!(!answers(&s, r#"CheckInteractDistance("", 4)"#));
        assert!(s.take_ui_errors().is_empty());
    }

    /// `CanInspect`'s refusal shows through `0x496720` before the answer: the chunk's next line
    /// reads it.
    #[test]
    fn can_inspects_refusal_shows_inside_its_call() {
        use crate::script::ui_errors::shown::seen_in_call;
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        let (seen, _) = seen_in_call(&mut s, r#"CanInspect("target")"#);
        assert_eq!(seen, "ERR_UNIT_NOT_FOUND");
        assert!(s.take_ui_errors().is_empty(), "nothing waits for the feed");
    }
}
