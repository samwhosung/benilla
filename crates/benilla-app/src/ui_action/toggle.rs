//! The plain-path **press-again-to-cancel** toggles. The reference client cancels an active
//! toggle spell in its DISPATCHERS, above `TryCast 0x6e4b60` (which itself has no cancel branch),
//! and the two plain paths are deliberately asymmetric:
//!
//! - **Active-action toggle** ([`active_action_toggle`], predicate `0x4e55f0` and its byte-twin
//!   `0x4b36f0`) — on BOTH `UseAction 0x4e5ee0` (cancel `0x4e60c1`) and the `CastSpell`/
//!   `CastSpellByName` dispatcher `0x4b3300` (cancel `0x4b3466`): the spell's raw
//!   **ActiveIconID ≠ 0** and its own id sits in a live `UNIT_FIELD_AURA` slot with the
//!   AURAFLAGS **cancelable bit** (nibble bit 0). No `SpellShapeshiftForm.dbc` guard — the
//!   data is the guard (a warrior stance has ActiveIconID 0, so the predicate never fires).
//!   Ghost Wolf, the druid forms, Stealth and Shadowform all carry a nonzero column.
//! - **Form-match toggle** ([`form_recast_disposition`], `0x4b348b`–`0x4b35e5`) — ONLY inside
//!   `0x4b3300`, never `UseAction`: the spell's MOD_SHAPESHIFT form equals the caster's form
//!   byte → cancel (`0x4b35da`), guarded by `SpellShapeshiftForm.flags1 & 0x2` (`0x4b35cf`, the
//!   separately-compiled twin of the stance bar's `0x4b4963`): guard set = **silent no-op**
//!   (bare epilogue, not a fall-through to cast).
//!
//! Both cancels are `Spell_C::CancelAura 0x6e7040` → `CMSG_CANCEL_AURA` — wire-identical to a
//! buff right-click. Its own internal guard (`AttributesEx & 0x2000` set and `& 0x4` clear)
//! never fires for the shipped toggle spells, and stays unmodeled.

use benilla_formats::{ShapeshiftForm, SpellDisplay};

use crate::net::ObjectStore;

/// The generic active-action toggle predicate (`0x4e55f0`/`0x4b36f0`): a nonzero raw
/// ActiveIconID, and the spell's OWN id live in the player's aura slots with the cancelable
/// flag. True = the press cancels the aura instead of casting.
pub(crate) fn active_action_toggle(spell_id: u32, d: &SpellDisplay, store: &ObjectStore) -> bool {
    d.active_icon_id != 0
        && store
            .0
            .unit_auras()
            .any(|a| a.spell_id == spell_id && a.is_cancelable())
}

/// The shapeshift-specific recast fork (`0x4b348b`–`0x4b35e5`, and the stance bar's own
/// MOD_SHAPESHIFT case — the same twin guard): `None` = not the active form, fall through to
/// the cast; `Some(true)` = cancel the form aura; `Some(false)` = the non-cancelable guard
/// (`SpellShapeshiftForm.flags1 & 0x2`) — a silent no-op.
pub(crate) fn form_recast_disposition(
    d: &SpellDisplay,
    form_byte: u8,
    form_row: Option<&ShapeshiftForm>,
) -> Option<bool> {
    if form_byte == 0 || d.shapeshift_form != Some(u32::from(form_byte)) {
        return None;
    }
    Some(form_row.is_some_and(|r| r.cancelable()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    /// Aura field indices mirrored from the protocol crate's own consts (private there):
    /// UNIT_FIELD_AURA 47, UNIT_FIELD_AURAFLAGS 95 (nibble-packed, 8 slots per u32).
    fn store_with_aura(spell_id: u32, flags_nibble: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (47u16, spell_id),
            (95, flags_nibble),
        ]))
    }

    /// Mechanism 1: fires only on ActiveIconID ≠ 0 AND own live cancelable aura.
    #[test]
    fn active_action_toggle_needs_the_icon_and_the_live_cancelable_aura() {
        let ghost_wolf = SpellDisplay {
            active_icon_id: 1,
            ..Default::default()
        };
        let plain = SpellDisplay::default();
        // Occupied (eff-index bit) + cancelable bit 0 → 0xb covers both.
        let shifted = store_with_aura(2645, 0xb);
        assert!(active_action_toggle(2645, &ghost_wolf, &shifted));
        // No active icon: never a toggle, whatever the aura says.
        assert!(!active_action_toggle(2645, &plain, &shifted));
        // Aura present but NOT cancelable (eff-index bits only): no toggle.
        let uncancelable = store_with_aura(2645, 0x2);
        assert!(!active_action_toggle(2645, &ghost_wolf, &uncancelable));
        // A different spell's aura: no toggle.
        assert!(!active_action_toggle(768, &ghost_wolf, &shifted));
        // No aura at all: no toggle.
        let bare = ObjectStore(ObjectFields::from_pairs(&[]));
        assert!(!active_action_toggle(2645, &ghost_wolf, &bare));
    }

    /// Mechanism 2: active-form match forks cancel / silent no-op on the flags1 & 0x2 guard;
    /// any other form falls through.
    #[test]
    fn form_recast_forks_on_the_active_form_and_the_cancelable_guard() {
        let ghost_wolf = SpellDisplay {
            shapeshift_form: Some(16),
            ..Default::default()
        };
        let wolf_row = ShapeshiftForm {
            flags: 0x40,
            ..Default::default()
        };
        let stance_row = ShapeshiftForm {
            flags: 0x7,
            ..Default::default()
        };
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 16, Some(&wolf_row)),
            Some(true)
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 16, Some(&stance_row)),
            Some(false),
            "the non-cancelable guard is a silent no-op, never a cast"
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 0, Some(&wolf_row)),
            None
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 1, Some(&wolf_row)),
            None
        );
        let plain = SpellDisplay::default();
        assert_eq!(form_recast_disposition(&plain, 16, Some(&wolf_row)), None);
    }
}
