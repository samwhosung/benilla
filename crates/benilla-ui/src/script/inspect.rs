//! The inspect bindings: `NotifyInspect` and `ClearInspectPlayer` queue intents, the app pushes
//! the inspected unit's equipment ([`UiScript::set_inspect`]) for the unit-keyed
//! `GetInventoryItem*` verbs, and `CanInspect` and `CheckInteractDistance` read a per-guid
//! distance map ([`UiScript::set_unit_reach`]) through the token resolver ([`super::UnitGuids`]).
//!
//! Worn gear is public descriptor state (`PLAYER_VISIBLE_ITEM_<n>_0`), so the window paints from
//! what the client holds: `SMSG_INSPECT` echoes only the guid, `InspectFrame_Show` calls
//! `NotifyInspect` and `ShowUIPanel` together (`Blizzard_InspectUI.lua:6-13`), and the paper doll
//! reads all 19 slots on show (`InspectPaperDollFrame.lua:57-79`). The view is keyed by unit token,
//! as the reference keeps `InspectFrame.unit`, so it follows a re-target.

use std::cmp::Ordering;
use std::collections::HashMap;

use mlua::{Lua, Value};

use super::char_stats::InventorySlots;
use super::Model;

/// `CheckInteractDistance`'s squared thresholds for types 1..=4, `{10², 11.1111², 10², 30²}`
/// (`0x48ba00`, from the `.rdata` at `0x804498`, `0x804490`, `0x80448c`, `0x8044a4`).
pub const INTERACT_DIST_SQ: [f64; 4] = [100.0, 123.45678, 100.0, 900.0];

/// `CanInspect`'s threshold squared (`0xb4d918`, the `.rdata` 10.0 at `0x804498` squared by
/// `0x48a1b0`); vmangos checks the same 10 yards (`INSPECT_DISTANCE`, `ObjectDefines.h:26`).
pub const CAN_INSPECT_DIST_SQ: f64 = 100.0;

/// One unit this frame, as the two range verbs read it. An entry exists only for a live unit
/// object: the reference maps the token to a GUID (`0x515940`) and answers nil when the object
/// manager has no such unit, as for a party member out of the area, whose GUID comes from the
/// roster (`0x4e81a0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitReach {
    /// Squared distance from the player.
    pub dist_sq: f64,
    /// A player, and not a valid attack target: vmangos's inspect refusals besides distance
    /// (`MiscHandler.cpp:945-956`). Only `CanInspect` reads it: `CheckInteractDistance` is a pure
    /// distance test, so a creature or an enemy player still answers it.
    pub inspectable: bool,
}

/// What the app has resolved for the unit currently being inspected.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectView {
    /// The inspected unit token, the reference's `InspectFrame.unit`; the inventory verbs match
    /// their `unit` argument against it.
    pub unit: String,
    /// The player the token resolved to, so the app can tell a new player behind the same token.
    pub guid: u64,
    /// Equipment by inventory slot id 1..=19, like the self feed; ammo (0) and bags (20..=23) stay
    /// `None`, since another player's are not visible.
    pub slots: InventorySlots,
}

impl super::UiScript {
    /// Push the squared distance of every live unit a token can name this frame, creature
    /// included; both range verbs read the same d². A verb resolves its token to a guid through
    /// [`super::UnitGuids`], so `"party1target"` and `"PLAYER"` read the unit they name, and a
    /// guid absent from the map answers nil from both, the reference's null-object arms
    /// (`0x48babe` for `CheckInteractDistance`, `0x48a1fa` for `CanInspect`). It changes every frame, so it stays off [`super::UnitState`], whose diffs
    /// fire `UNIT_*` events.
    pub fn set_unit_reach(&mut self, reach: HashMap<u64, UnitReach>) {
        self.model_mut().unit_reach = reach;
    }

    /// The guids of the held units the resolver's inputs reach ([`Self::set_unit_guids`]): every
    /// unit a token can name, the `target` chains included, that the object manager holds. These
    /// are the guids [`Self::set_unit_reach`] is measured for.
    pub fn held_unit_guids(&self) -> Vec<u64> {
        self.model_ref().unit_guids.held.keys().copied().collect()
    }

    /// Push (or clear, with `None`) the inspected unit's equipment. It fires nothing: the app fires
    /// `UNIT_INVENTORY_CHANGED` for the token, which the stock slot buttons listen for.
    pub fn set_inspect(&mut self, view: Option<InspectView>) {
        self.model_mut().inspect = view;
    }

    /// Drain the tokens `NotifyInspect` queued; the app sends `CMSG_INSPECT` for each, which also
    /// sets the server-side selection (`MiscHandler.cpp:945`).
    pub fn take_inspect_notifies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().inspect_notifies)
    }

    /// Whether `ClearInspectPlayer` was called since the last drain; the app drops its target.
    pub fn take_inspect_clear(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().inspect_clear)
    }
}

/// The reach of the unit `token` names, through the resolver (`0x515970`, which both verbs call via
/// `0x515940`): the guid it answers, then that guid's entry. A token the resolver does not
/// recognise answers nobody here.
fn reach(lua: &Lua, token: &Option<String>) -> Option<UnitReach> {
    let token = token.as_deref()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let guid = model.unit_guids.resolve(token).ok().flatten()?;
    model.unit_reach.get(&guid).copied()
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CanInspect(unit) → 1/nil (`0x48a1b0`), the gate of `InspectFrame_Show`
    // (`Blizzard_InspectUI.lua:8`): in range unless `100.0 < d²`, and `test ah,0x41; jne` counts an
    // unordered compare as in range, so a NaN distance passes. The player and attackability legs
    // ride in [`UnitReach::inspectable`].
    g.set(
        "CanInspect",
        lua.create_function(|lua, unit: Option<String>| {
            let ok = match reach(lua, &unit) {
                Some(r) => {
                    r.inspectable
                        && matches!(
                            r.dist_sq.partial_cmp(&CAN_INSPECT_DIST_SQ),
                            Some(Ordering::Less | Ordering::Equal) | None
                        )
                }
                // No live unit: nil, through `0x4944a0`'s null-`this` guard.
                None => false,
            };
            Ok(if ok { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // CheckInteractDistance(unit, type) → 1/nil (`0x48ba00`): in range only when
    // `d² < table[type-1]`, strictly, unlike `CanInspect` (`test ah,0x5; jp`). A token with no unit
    // answers nil (`0x48babe`), as does a `type` outside 1..=4 once truncated toward zero, the
    // compare being unsigned (`0x48bac5`); a missing or non-number `type` raises (`0x48bb48`).
    // The reference also raises for a missing unit (`0x48ba18`) or an unknown token (`0x515c14`),
    // where this answers nil.
    g.set(
        "CheckInteractDistance",
        lua.create_function(|lua, (unit, kind): (Option<String>, Option<f64>)| {
            let Some(kind) = kind else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: CheckInteractDistance(\"unit\", distIndex)".into(),
                ));
            };
            let thr = usize::try_from(kind.trunc() as i64)
                .ok()
                .and_then(|k| k.checked_sub(1))
                .and_then(|i| INTERACT_DIST_SQ.get(i).copied());
            let ok = match (reach(lua, &unit), thr) {
                (Some(r), Some(thr)) => r.dist_sq < thr,
                _ => false,
            };
            Ok(if ok { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // NotifyInspect(unit) (`Blizzard_InspectUI.lua:9`): fire-and-forget, as nothing waits on the
    // reply.
    g.set(
        "NotifyInspect",
        lua.create_function(|lua, unit: String| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .inspect_notifies
                .push(unit);
            Ok(())
        })?,
    )?;

    // ClearInspectPlayer(): called from `InspectFrame_OnHide` (`Blizzard_InspectUI.lua:58`).
    g.set(
        "ClearInspectPlayer",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .inspect_clear = true;
            Ok(())
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

    /// A chain that ends at nobody, a unit with no object, text after the chain and a token the
    /// resolver does not read as a chain each answer nil, and the compares fold case.
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
            // `npc` is an exact compare with no chain.
            "npctarget",
            "bogus",
            "",
        ] {
            let call = format!(r#"CheckInteractDistance("{token}", 4)"#);
            assert!(!answers(&s, &call), "{call}");
            let call = format!(r#"CanInspect("{token}")"#);
            assert!(!answers(&s, &call), "{call}");
        }
        assert!(answers(&s, r#"CheckInteractDistance("PARTY1TARGET", 1)"#));
        assert!(answers(&s, r#"CanInspect("Party1Target")"#));
    }
}
