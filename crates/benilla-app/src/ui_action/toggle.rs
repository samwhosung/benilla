//! Press-again-to-cancel. The reference cancels in its dispatchers, above `TryCast 0x6e4b60`,
//! and the two toggles differ: the active-action one runs in both `UseAction 0x4e5ee0` (cancel
//! `0x4e60c1`) and the `CastSpell` dispatcher `0x4b3300` (cancel `0x4b3466`), the form-match one
//! (`0x4b348b`-`0x4b35e5`, cancel `0x4b35da`) in `CastSpell` only. Both cancel through
//! `CancelAura 0x6e7040`, the buff right-click's `CMSG_CANCEL_AURA`; its own guard
//! (`AttributesEx & 0x2000` set, `& 0x4` clear) never fires for a shipped toggle and is not built.

use benilla_formats::{ShapeshiftForm, SpellDisplay};

use crate::net::ObjectStore;

/// The active-action predicate (`0x4e55f0`, twin `0x4b36f0`): a nonzero raw `ActiveIconID` and
/// the spell's own aura live with the cancelable bit, so the press cancels instead of casting.
/// No form guard: a warrior stance has `ActiveIconID` 0.
pub(crate) fn active_action_toggle(spell_id: u32, d: &SpellDisplay, store: &ObjectStore) -> bool {
    d.active_icon_id != 0
        && store
            .0
            .unit_auras()
            .any(|a| a.spell_id == spell_id && a.is_cancelable())
}

/// The form-match recast: `None` casts (not the active form), `Some(true)` cancels the form,
/// `Some(false)` is a silent no-op when `SpellShapeshiftForm.flags1 & 0x2` is set (`0x4b35cf`,
/// twin of the stance bar's `0x4b4963`).
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

    /// Raw indices: `UNIT_FIELD_AURA` 47, `UNIT_FIELD_AURAFLAGS` 95 (a nibble per slot).
    fn store_with_aura(spell_id: u32, flags_nibble: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (47u16, spell_id),
            (95, flags_nibble),
        ]))
    }

    #[test]
    fn active_action_toggle_needs_the_icon_and_the_live_cancelable_aura() {
        let ghost_wolf = SpellDisplay {
            active_icon_id: 1,
            ..Default::default()
        };
        let plain = SpellDisplay::default();
        // 0xb: occupied (effect-index bits) and cancelable (bit 0).
        let shifted = store_with_aura(2645, 0xb);
        assert!(active_action_toggle(2645, &ghost_wolf, &shifted));
        assert!(!active_action_toggle(2645, &plain, &shifted));
        // 0x2: occupied, not cancelable.
        let uncancelable = store_with_aura(2645, 0x2);
        assert!(!active_action_toggle(2645, &ghost_wolf, &uncancelable));
        assert!(!active_action_toggle(768, &ghost_wolf, &shifted));
        let bare = ObjectStore(ObjectFields::from_pairs(&[]));
        assert!(!active_action_toggle(2645, &ghost_wolf, &bare));
    }

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
