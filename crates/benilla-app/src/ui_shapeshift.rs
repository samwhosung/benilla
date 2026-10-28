//! The stance bar behind `GetNumShapeshiftForms` (`0x4b4590`), `GetShapeshiftFormInfo`
//! (`0x4b45c0`), `CastShapeshiftForm` (`0x4b4810`) and `GetShapeshiftFormCooldown` (`0x4b49a0`),
//! each read at the call through the call host ([`live`]). A known spell joins the bar
//! (`0x4b25b0`) unless `AttributesEx2 & 0x2`, when it has a `SPELL_AURA_MOD_SHAPESHIFT` effect or
//! `AttributesEx2 & 0x10`; the bar sorts by `StanceBarOrder`, negative last, then spell id
//! (`0x4b2bb0`).
//!
//! The list is stored ([`crate::ui_action::PlayerActions::forms`]): the book's reset empties it
//! (`0x4b23a0`, at our player's arrival), and each learn the registrar runs grows it
//! ([`form_learned`]), the arrival's learns one at a time in `SMSG_INITIAL_SPELLS`'s order. Its
//! own learn and unlearn edge, `UPDATE_SHAPESHIFT_FORMS` (`0x4b2904`, `0x4b2e4e`), is raised by
//! the book's handlers ([`crate::ui_spellbook::BookSignals`]) and by the arrival's replay
//! ([`crate::ui_world_enter`]), each listener reading the list as that learn left it.
//! Castability, active form, texture and cooldown change silently: the castable flags refresh
//! inside the usable refresh `0x4b31c0`, whose callers signal `SPELL_UPDATE_USABLE`
//! (`crate::ui_action`), and the rest repaint on `PLAYER_AURAS_CHANGED` (a form's aura holds a
//! slot, warrior stances included) and `SPELL_UPDATE_COOLDOWN`; a cooldown's expiry fires nothing.

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay};

use crate::net::{ClientCommand, ObjectStore};
use crate::spell::CastCommit;

mod live;

pub(crate) use live::StanceLive;

/// Keeps a spell off the stance bar; Ghost Wolf 2645 carries it, so a shaman has no bar.
const ATTR_EX2_STANCE_BAR_EXCLUDE: u32 = 0x2;
/// Admits a spell with no `MOD_SHAPESHIFT` effect; in 1.12 data the live carriers, all with
/// `ActiveIconID` 122, are every rank of the paladin auras (Devotion 465 and the rest) and
/// Ironweave Battlesuit 27733.
const ATTR_EX2_STANCE_BAR_FORCE: u32 = 0x10;

/// Whether the registrar lists `d` on the stance bar (`0x4b27f4`-`0x4b2823`): not
/// `AttributesEx2 & 0x2`, and a `MOD_SHAPESHIFT` effect or `AttributesEx2 & 0x10`.
pub(crate) fn on_stance_bar(d: &SpellDisplay) -> bool {
    d.attributes_ex2 & ATTR_EX2_STANCE_BAR_EXCLUDE == 0
        && (d.shapeshift_form.is_some() || d.attributes_ex2 & ATTR_EX2_STANCE_BAR_FORCE != 0)
}

/// `isActive` (`0x4b45c0`), forked on the form id (the first `MOD_SHAPESHIFT` misc value, else 0).
/// A form spell matches the form byte (`4b46a0`). A force-admitted one with `ActiveIconID != 0`
/// (`4b46f2`) needs its own aura live with the cancelable bit (`4b4739`), the action bar toggle's
/// scan (`0x4e55f0`); a paladin aura lights only this way.
fn form_active(
    spell_id: u32,
    d: &SpellDisplay,
    form_byte: u8,
    store: Option<&ObjectStore>,
) -> bool {
    match d.shapeshift_form.unwrap_or(0) {
        0 => store.is_some_and(|s| crate::ui_action::toggle::active_action_toggle(spell_id, d, s)),
        form => form == u32::from(form_byte),
    }
}

/// `SpellIconID`, or `ActiveIconID` when active and nonzero, under either arm of [`form_active`]:
/// both reach the one block that elects the icon (`4b4754`), so a lit paladin aura wears its
/// `ActiveIconID`. A form spell with the column at 0 is active yet keeps `SpellIconID`
/// (`4b4763 je`).
fn form_texture(d: &SpellDisplay, active: bool) -> Option<String> {
    if active {
        d.active_icon.clone().or_else(|| d.icon.clone())
    } else {
        d.icon.clone()
    }
}

/// The bar's order (`0x4b2bb0`): by `StanceBarOrder`, negative last, then spell id; a spell with
/// no record last of all.
fn bar_key(id: u32, catalog: &SpellCatalog) -> (i64, u32) {
    let order = match catalog.get(id) {
        Some(d) if d.stance_bar_order >= 0 => i64::from(d.stance_bar_order),
        _ => i64::MAX,
    };
    (order, id)
}

/// The registrar's form leg (`0x4b27f4`-`0x4b2904`) for `spell_id`, on the stored list `forms`
/// ([`crate::ui_action::PlayerActions::forms`]): a stance-bar spell (module doc) replacing
/// `replaces` takes its place where the list holds it, unsorted, and adds nothing where it does not
/// (`0x4b2849`-`0x4b2878`); any other is appended unless listed (`0x4b2893`) and the list sorted
/// (`0x4b28f7`). Whether the list moved, which signals `UPDATE_SHAPESHIFT_FORMS` (`0x4b2904`).
pub(crate) fn form_learned(
    forms: &mut Vec<u32>,
    spell_id: u32,
    replaces: Option<u32>,
    catalog: &SpellCatalog,
) -> bool {
    if !catalog.get(spell_id).is_some_and(on_stance_bar) {
        return false;
    }
    if let Some(old) = replaces {
        return match forms.iter().position(|&f| f == old) {
            Some(at) => {
                forms[at] = spell_id;
                true
            }
            None => false,
        };
    }
    if forms.contains(&spell_id) {
        return false;
    }
    forms.push(spell_id);
    forms.sort_by_key(|&id| bar_key(id, catalog));
    true
}

/// A learn's or an unlearn's form leg, kept to apply again ([`crate::ui_action::PlayerActions::form_edits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormEdit {
    Learned {
        spell_id: u32,
        replaces: Option<u32>,
    },
    Unlearned {
        spell_id: u32,
    },
}

impl FormEdit {
    /// The edit's leg on `forms`, signalling nothing.
    pub(crate) fn apply(self, forms: &mut Vec<u32>, catalog: &SpellCatalog) {
        match self {
            FormEdit::Learned { spell_id, replaces } => {
                form_learned(forms, spell_id, replaces, catalog);
            }
            FormEdit::Unlearned { spell_id } => {
                form_unlearned(forms, spell_id);
            }
        }
    }
}

/// The unlearn's form leg (`0x4b2dc2`-`0x4b2e4e`): the last copy of `spell_id` the list holds
/// leaves it, the rest closing up; whether it held one, which signals `UPDATE_SHAPESHIFT_FORMS`.
pub(crate) fn form_unlearned(forms: &mut Vec<u32>, spell_id: u32) -> bool {
    match forms.iter().rposition(|&f| f == spell_id) {
        Some(at) => {
            forms.remove(at);
            true
        }
        None => false,
    }
}

/// `CastShapeshiftForm` (`0x4b4810`), forked on the form id like the info call: the active form
/// cancels unless `SpellShapeshiftForm.dbc` `flags1 & 0x2` makes it a silent no-op (`0x4b4963`,
/// warrior stances); a force-admitted spell whose aura is up cancels, with no DBC guard since it
/// has no form id. Anything else casts.
pub(crate) fn cast_form(cast: &mut crate::spell::ScriptCast, spell_id: u32) {
    let crate::spell::ScriptCast { targeting, ladder } = cast;
    let store = targeting.self_store.iter().next();
    let form_byte = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let d = ladder.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
    // The active form's row, for the `0x4b4963` guard (shared with `crate::ui_action::toggle`).
    let row = ladder
        .spells
        .as_ref()
        .and_then(|s| s.forms.get(&u32::from(form_byte)));
    // The force-admit arm uses `form_active`'s predicate, so a lit button always cancels.
    let disposition = d.and_then(|d| {
        if d.shapeshift_form.unwrap_or(0) == 0 {
            store
                .filter(|s| crate::ui_action::toggle::active_action_toggle(spell_id, d, s))
                .map(|_| true)
        } else {
            crate::ui_action::toggle::form_recast_disposition(d, form_byte, row)
        }
    });
    match disposition {
        Some(true) => {
            debug!("ui_shapeshift: cancel form aura {spell_id}");
            let _ = ladder
                .commands
                .0
                .send(ClientCommand::CancelAura { spell_id });
        }
        Some(false) => {}
        None => {
            debug!("ui_shapeshift: cast form {spell_id}");
            ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_AURA` 47 and `UNIT_FIELD_AURAFLAGS` 95 (nibble-packed); `0xb` is occupied plus
    /// cancelable (bit 0).
    fn player_with_aura(spell_id: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(47u16, spell_id), (95, 0xb)]))
    }

    /// Devotion Aura as the shipped `Spell.dbc` has it: `AttributesEx2 0x10`, no `MOD_SHAPESHIFT`
    /// effect, `ActiveIconID` 122, `StanceBarOrder` 0.
    fn devotion_aura() -> SpellDisplay {
        SpellDisplay {
            attributes_ex2: ATTR_EX2_STANCE_BAR_FORCE,
            shapeshift_form: None,
            active_icon_id: 122,
            stance_bar_order: 0,
            ..Default::default()
        }
    }

    /// A paladin aura has no `MOD_SHAPESHIFT` effect, so only the aura scan can light it.
    #[test]
    fn a_force_admitted_aura_latches_on_its_own_live_aura_not_the_form_byte() {
        let devotion = devotion_aura();
        let up = player_with_aura(465);

        assert!(
            form_active(465, &devotion, 0, Some(&up)),
            "Devotion Aura is up: the button must read active, with the form byte at 0"
        );
        assert!(
            !form_active(7294, &devotion_aura(), 0, Some(&up)),
            "a DIFFERENT aura's button stays dark while Devotion is the one that is up"
        );
        assert!(
            !form_active(
                465,
                &devotion,
                0,
                Some(&ObjectStore(ObjectFields::from_pairs(&[])))
            ),
            "no aura, no latch"
        );
        assert!(
            !form_active(465, &devotion, 0, None),
            "no player object, no latch"
        );

        // `ActiveIconID == 0` closes the arm (`4b46f2`).
        let iconless = SpellDisplay {
            active_icon_id: 0,
            ..devotion_aura()
        };
        assert!(!form_active(465, &iconless, 0, Some(&up)));
    }

    /// Both arms reach the block that elects `ActiveIconID` (`0x4b45c0`); the zero fallback
    /// (`4b4763 je`) is reachable only on the form arm.
    #[test]
    fn the_active_icon_swap_reaches_both_arms() {
        let shield = Some("Interface\\Icons\\Spell_Holy_DevotionAura".to_string());
        let swirl = Some("Interface\\Icons\\Spell_Nature_WispSplode".to_string());

        // Cat Form, the form arm.
        let cat = SpellDisplay {
            shapeshift_form: Some(1),
            active_icon_id: 122,
            active_icon: swirl.clone(),
            icon: shield.clone(),
            ..Default::default()
        };
        assert_eq!(form_texture(&cat, true), swirl);
        assert_eq!(form_texture(&cat, false), shield);

        // A paladin aura, the force-admit arm, elects the same column (`4b4754`).
        let devotion = SpellDisplay {
            active_icon: swirl.clone(),
            icon: shield.clone(),
            ..devotion_aura()
        };
        assert_eq!(
            form_texture(&devotion, true),
            swirl,
            "the aura-scan hit falls through into the block that elects ActiveIconID"
        );
        assert_eq!(form_texture(&devotion, false), shield);

        // `4b475c` sets `isActive` before `4b4763` tests the column: a warrior stance is active
        // yet keeps `SpellIconID`.
        let stance = SpellDisplay {
            shapeshift_form: Some(17),
            active_icon_id: 0,
            active_icon: None,
            icon: shield.clone(),
            ..Default::default()
        };
        assert_eq!(
            form_texture(&stance, true),
            shield,
            "active and unswapped are decided by different bytes and do not imply each other"
        );
    }

    /// A form spell reads the form byte, never its aura; this pins the fork, not the data.
    #[test]
    fn a_shapeshift_form_still_latches_on_the_form_byte() {
        let battle_stance = SpellDisplay {
            shapeshift_form: Some(17),
            active_icon_id: 0,
            ..Default::default()
        };
        let no_auras = ObjectStore(ObjectFields::from_pairs(&[]));
        assert!(form_active(2457, &battle_stance, 17, Some(&no_auras)));
        assert!(!form_active(2457, &battle_stance, 1, Some(&no_auras)));
        assert!(
            !form_active(2457, &battle_stance, 0, Some(&player_with_aura(2457))),
            "out of the form: a live aura row is not what the MOD_SHAPESHIFT arm reads"
        );
    }
}
