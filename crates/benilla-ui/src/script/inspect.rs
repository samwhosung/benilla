//! The inspect bindings: `NotifyInspect` and `ClearInspectPlayer` queue intents, the app pushes
//! the inspected unit's equipment ([`UiScript::set_inspect`]) for the unit-keyed
//! `GetInventoryItem*` verbs, and `CanInspect` and `CheckInteractDistance` read a per-token
//! distance map ([`UiScript::set_unit_reach`]).
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

/// One token's unit this frame, as the two range verbs read it. An entry exists only when the
/// token resolved to a live unit object: the reference maps the token to a GUID (`0x515940`) and
/// answers nil when the object manager has no such unit, as for a party member out of the area,
/// whose GUID comes from the roster (`0x4e81a0`).
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
    /// Push the squared distance of every token that resolved to a live unit this frame, creature
    /// included; both range verbs read the same d². A token absent from the map answers nil from
    /// both, the reference's null-object arm (`0x48babe`). It changes every frame, so it stays off
    /// [`super::UnitState`], whose diffs fire `UNIT_*` events. Keys are lowercase: the token
    /// resolver folds case (`_strnicmp`).
    pub fn set_unit_reach(&mut self, reach: HashMap<String, UnitReach>) {
        self.model_mut().unit_reach = reach;
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

/// The reach pushed for `token`, case-folded: both verbs resolve the token through `0x515970`,
/// whose compares are `_strnicmp`.
fn reach(lua: &Lua, token: &Option<String>) -> Option<UnitReach> {
    let token = token.as_deref()?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    if token.bytes().any(|b| b.is_ascii_uppercase()) {
        model.unit_reach.get(&token.to_ascii_lowercase()).copied()
    } else {
        model.unit_reach.get(token).copied()
    }
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
