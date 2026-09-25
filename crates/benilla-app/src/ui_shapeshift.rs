//! The stance bar's feed and drain behind `GetNumShapeshiftForms` (`0x4b4590`),
//! `GetShapeshiftFormInfo` (`0x4b45c0`), `CastShapeshiftForm` (`0x4b4810`) and
//! `GetShapeshiftFormCooldown` (`0x4b49a0`). A known spell joins the bar (`0x4b25b0`) unless
//! `AttributesEx2 & 0x2`, when it has a `SPELL_AURA_MOD_SHAPESHIFT` effect or
//! `AttributesEx2 & 0x10`; the bar sorts by `StanceBarOrder`, negative last, then spell id
//! (`0x4b2bb0`).
//!
//! Only a change of membership or order fires `UPDATE_SHAPESHIFT_FORMS`, the reference's
//! learn/unlearn edge (`0x4b28ff`, `0x4b2e43`), because the stock `ShapeshiftBar_Update` rebuilds
//! the shelf on every fire. A castability change fires `SPELL_UPDATE_USABLE`, as `0x4b31c0` does.
//! Active form, texture and cooldown push silently and repaint on `PLAYER_AURAS_CHANGED` (a form's
//! aura holds a slot, warrior stances included) and `SPELL_UPDATE_COOLDOWN`, so the feed runs
//! before both; a cooldown's expiry fires nothing.

use std::time::Instant;

use bevy::prelude::*;

use benilla_ui::script::{ShapeshiftFormView, UiScript};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, Reputations, SelfPlayer};
use crate::spell::Cooldowns;
use crate::spell::{cast_target, usable, CastCommit, CastLadder};
use crate::target::Selection;
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;

/// Keeps a spell off the stance bar; Ghost Wolf 2645 carries it, so a shaman has no bar.
const ATTR_EX2_STANCE_BAR_EXCLUDE: u32 = 0x2;
/// Admits a spell with no `MOD_SHAPESHIFT` effect; in 1.12 data the live carriers, all with
/// `ActiveIconID` 122, are every rank of the paladin auras (Devotion 465 and the rest) and
/// Ironweave Battlesuit 27733.
const ATTR_EX2_STANCE_BAR_FORCE: u32 = 0x10;

/// `isActive` (`0x4b45c0`), forked on the form id (the first `MOD_SHAPESHIFT` misc value, else 0).
/// A form spell matches the form byte (`4b46a0`). A force-admitted one with `ActiveIconID != 0`
/// (`4b46f2`) needs its own aura live with the cancelable bit (`4b4739`), the action bar toggle's
/// scan (`0x4e55f0`); a paladin aura lights only this way.
fn form_active(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
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
fn form_texture(d: &benilla_formats::SpellDisplay, active: bool) -> Option<String> {
    if active {
        d.active_icon.clone().or_else(|| d.icon.clone())
    } else {
        d.icon.clone()
    }
}

/// What the feed last pushed. The cooldown triple carries the absolute start, so a running
/// cooldown re-derives the same view every frame.
#[derive(Default)]
pub(crate) struct StanceMemory {
    pushed: Option<Vec<ShapeshiftFormView>>,
}

/// The edge one push crossed, as [`push_forms`] announces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormsEdge {
    Unchanged,
    /// Membership or order: `UPDATE_SHAPESHIFT_FORMS`.
    List,
    /// Castability: `SPELL_UPDATE_USABLE`.
    Usable,
    /// Active form, texture or cooldown: pushed, announced by other feeds' events.
    Silent,
}

/// Push the rebuilt list and announce its edge (module doc); callable without a Bevy world.
pub(crate) fn push_forms(
    script: &mut UiScript,
    memory: &mut StanceMemory,
    fresh: Vec<ShapeshiftFormView>,
) -> FormsEdge {
    let old: &[ShapeshiftFormView] = memory.pushed.as_deref().unwrap_or(&[]);
    let edge = if old == fresh.as_slice() {
        FormsEdge::Unchanged
    } else if old.len() != fresh.len()
        || old
            .iter()
            .zip(&fresh)
            .any(|(o, n)| o.spell_id != n.spell_id)
    {
        FormsEdge::List
    } else if old
        .iter()
        .zip(&fresh)
        .any(|(o, n)| o.castable != n.castable)
    {
        FormsEdge::Usable
    } else {
        FormsEdge::Silent
    };
    if edge == FormsEdge::Unchanged {
        return edge;
    }
    memory.pushed = Some(fresh.clone());
    script.set_shapeshift_forms(fresh);
    match edge {
        FormsEdge::List => script.fire_event("UPDATE_SHAPESHIFT_FORMS", vec![]),
        FormsEdge::Usable => script.fire_event("SPELL_UPDATE_USABLE", vec![]),
        FormsEdge::Silent | FormsEdge::Unchanged => {}
    }
    edge
}

pub(crate) struct UiShapeshiftPlugin;

impl Plugin for UiShapeshiftPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // The feed precedes the two event sets whose handlers re-read this list; the
                // drain follows the input pass, so a click goes out the same frame.
                feed_shapeshift_bar
                    .in_set(UnitFeed)
                    .before(crate::ui_action::CooldownEvents)
                    .before(crate::ui_aura::AuraEvents),
                drain_shapeshift_casts.after(UiInput),
            ),
        );
    }
}

/// Build the bar from the known spells and the catalog (module doc), and diff-push it.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
fn feed_shapeshift_bar(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    cooldowns: Res<Cooldowns>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    selection: Res<Selection>,
    // The selection's guid index, and the bag walk behind each form's reagent leg.
    objects: Objects,
    units: Query<&ObjectStore, Without<SelfPlayer>>,
    factions: Option<Res<crate::target::Factions>>,
    reputations: Res<Reputations>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    clock: Res<crate::ui_script::UiClock>,
    spell_mods: Res<crate::spell::SpellModifiers>,
    mut memory: Local<crate::ui_script::VmMemo<StanceMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    let Some(spells) = spells else {
        return;
    };
    let store = self_q.iter().next();
    let form_byte = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let now = Instant::now();
    // The frame's clock pair: a local `Instant::now()` would wobble the derived start.
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    // The usable walk's target leg (Execute) reads the current target.
    let target_store = selection
        .guid
        .and_then(|g| objects.entity(g))
        .and_then(|e| units.get(e).ok());

    // Admission and order (module doc).
    let mut rows: Vec<(u32, &benilla_formats::SpellDisplay)> = actions
        .spells
        .iter()
        .filter_map(|&id| {
            let d = spells.catalog.get(id)?;
            let admitted = d.attributes_ex2 & ATTR_EX2_STANCE_BAR_EXCLUDE == 0
                && (d.shapeshift_form.is_some()
                    || d.attributes_ex2 & ATTR_EX2_STANCE_BAR_FORCE != 0);
            admitted.then_some((id, d))
        })
        .collect();
    rows.sort_by_key(|&(id, d)| {
        let order = if d.stance_bar_order < 0 {
            i64::MAX
        } else {
            i64::from(d.stance_bar_order)
        };
        (order, id)
    });

    // The bags, walked once for every form's reagent leg.
    let carried = store
        .map(|s| crate::ui_items::carried_counts(&s.0, &objects))
        .unwrap_or_default();
    let fresh: Vec<ShapeshiftFormView> = rows
        .into_iter()
        .map(|(id, d)| {
            let active = form_active(id, d, form_byte, store);
            // `isCastable`: true for the active form, else the `0x6e3d60` walk the action bar's
            // `IsUsableAction` runs.
            let castable = active
                || store.is_none_or(|s| {
                    let ctx = usable::UsableCtx {
                        store: s,
                        target_store,
                        factions: factions.as_deref(),
                        reputations: &reputations,
                        cooldowns: &cooldowns,
                        carried: &carried,
                        spell_mods: &spell_mods,
                    };
                    usable::spell_usable(id, d, &spells, &ctx, &objects, &items, &commands).0
                });
            let texture = form_texture(d, active);
            let cooldown = cooldowns
                .info(id, 0, Some(d), now)
                .ui_triple(anchor, ui_now);
            ShapeshiftFormView {
                spell_id: id,
                texture,
                name: d.name.clone(),
                active,
                castable,
                cooldown,
            }
        })
        .collect();

    let count = fresh.len();
    let edge = push_forms(&mut script, memory, fresh);
    if edge != FormsEdge::Unchanged {
        debug!("ui_shapeshift: {count} form(s), active form byte {form_byte}, {edge:?} edge");
    }
}

/// Drain `CastShapeshiftForm` (`0x4b4810`), forked on the form id like the info call: the active
/// form cancels unless `SpellShapeshiftForm.dbc` `flags1 & 0x2` makes it a silent no-op
/// (`0x4b4963`, warrior stances); a force-admitted spell whose aura is up cancels, with no DBC
/// guard since it has no form id. Anything else casts.
fn drain_shapeshift_casts(
    script: Option<NonSendMut<UiScript>>,
    targeting: cast_target::CastTargeting,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    mut ladder: CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_shapeshift_casts() {
        let store = self_store.iter().next();
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
                continue;
            }
            Some(false) => continue,
            None => {}
        }
        debug!("ui_shapeshift: cast form {spell_id}");
        ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
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
