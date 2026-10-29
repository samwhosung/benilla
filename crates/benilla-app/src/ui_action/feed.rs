//! The action bar's feed: the engine's own by-key refusals (the handlers' and the verbs' fire from
//! their lane, [`super::UiErrorKeys`]; a refused cast's line rides the cast's reports,
//! [`crate::ui_cast`]), and a `/reload`'s VM's copy of the bar ([`seed_reloaded_vm`]). The slots'
//! icons, counts and consumable tests are read at the call through the host
//! ([`super::ActionStates`]); the VM's copy is written where the bar is: our player's arrival
//! ([`super::recount::arrival_slot_pass`]), the verbs in their calls, the spell packets' clears
//! and replaces ([`super::spell_slots`]) and the recounts ([`super::recount`]).

use bevy::prelude::*;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::ObjectStore;

use super::weapon_icon::auto_attack_icon;
use super::{show_messages, ui_error_text, MessageSink, Shown, Spells, UiError};

/// An item action's icon when its template has not answered or its display id has no
/// `ItemDisplayInfo` row: the reference's literal `0x847fe4`, from the resolver's `0x5d8927`.
pub(crate) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// `0x4e4fc0`'s offset: the second field of the `SpellShapeshiftForm` row of the player's form
/// byte, 0 with no player or row (`0x4e4fc5`, `0x4e500f`). It is set, and `UPDATE_BONUS_ACTIONBAR`
/// signalled, at each world entry (`0x490979` → `0x4e4f90`) and each form change (`0x5de01b`),
/// nowhere else.
pub(crate) fn bonus_bar_offset(store: Option<&ObjectStore>, spells: Option<&Spells>) -> u8 {
    let form = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    spells
        .and_then(|s| s.forms.get(&u32::from(form)))
        .map(|f| f.bonus_bar)
        .unwrap_or(0) as u8
}

pub(super) fn feed_actions(mut vm: crate::script_calls::VmWith<()>, mut sink: MessageSink) {
    // The engine's refusals, with the host lent: `UI_ERROR_MESSAGE`'s handlers may make calls.
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        // The engine's own by-key refusals (`ERR_PASSIVE_ABILITY`): `benilla_ui` cannot reach
        // [`super::UiErrorKeys`], so it queues keys that show here, one frame late.
        let engine_keys = script.take_ui_errors_with_input();
        let engine_lines: Vec<Shown> = engine_keys
            .into_iter()
            .filter_map(|(key, input)| {
                let e = UiError::key(key);
                ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                    .map(|t| Shown::keyed(e.key, t).under(input))
            })
            .collect();
        show_messages(script, &mut sink, "ui_action", engine_lines);
    });
}

/// A `/reload`'s VM seeded with the bar before its first file runs, its counts and its states,
/// with nothing signalled: the bar and its tables outlive the interface (`0x4e4f50` runs only at
/// the game entry, `0x48f5a4`), so the new interface's `OnLoad`, `ADDON_LOADED` and
/// `VARIABLES_LOADED` handlers read the bar it left ([`super::ActionStates::seed`]).
pub(crate) fn seed_reloaded_vm(world: &mut World, script: &mut benilla_ui::script::UiScript) {
    let seeded = world.run_system_cached_with(
        |bevy::ecs::system::InMut(script): bevy::ecs::system::InMut<
            benilla_ui::script::UiScript,
        >,
         mut states: super::ActionStates| states.seed(script),
        script,
    );
    if let Err(e) = seeded {
        debug!("ui_action: the reload's VM is not seeded with the bar: {e}");
    }
}

/// A spell slot's icon, by the reference's `GetActionTexture` resolver `0x4e6a50` in order: the
/// auto-attack faces ([`auto_attack_icon`], `0x4e6870`, `0x4e6990`), then `ActiveIconID` while
/// the spell's own aura is live and cancelable (`0x4e6bbd`, the `0x4e55f0` predicate of
/// [`super::toggle::active_action_toggle`]), then the spell's icon. The spellbook's
/// `GetSpellTexture` (`0x4b3f50`) never serves `ActiveIconID`.
pub(crate) fn spell_action_icon(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    spells: &super::Spells,
    store: Option<&crate::net::ObjectStore>,
    objects: &crate::net::Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
) -> Option<String> {
    auto_attack_icon(d, store, &spells.forms, objects, items, icons)
        .or_else(|| {
            store
                .filter(|s| super::toggle::active_action_toggle(spell_id, d, s))
                .and_then(|_| d.active_icon.clone())
        })
        .or_else(|| d.icon.clone())
}
