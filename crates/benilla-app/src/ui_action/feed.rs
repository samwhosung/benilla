//! The action bar's identity feed: each of the 120 slots' icon and count, pushed to the VM with
//! `ACTIONBAR_SLOT_CHANGED` per changed slot; between resolves an item slot's count moves by the
//! item watchers' recount ([`super::recount`]). [`feed_actions`] also drains the UI error queues
//! (a refused cast's line rides the cast's reports, [`crate::ui_cast`]) and feeds the stance page
//! (`GetBonusBarOffset`: the form's `SpellShapeshiftForm.dbc` BonusActionBar column, `0x4e4fc0`).

use crate::query_cache::Ask;
use crate::ui_items::action_count;
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::{ActionSlot, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::errors::mount_result_key;
use super::weapon_icon::{auto_attack_icon, substitutes_weapon_icon};
use super::{
    show_messages, ui_error_text, MessageSink, MountErrors, PetTameFailures, PlayerActions, Shown,
    Spells, UiError, UiErrorKeys, UiErrorTexts,
};

/// An item action's icon when its template has not answered or its display id has no
/// `ItemDisplayInfo` row: the reference's literal `0x847fe4`, from the resolver's `0x5d8927`.
pub(crate) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The feed's memory of what it last pushed, for per-slot change events.
#[derive(Default)]
pub(super) struct FeedMemory {
    pushed: HashMap<u32, ActionSlot>,
    bonus_offset: u8,
    /// Whether the identity resolve has run against this VM: a new VM resets this memory, while
    /// every other input to the gate is host-side and may still match.
    resolved: bool,
    /// The [`Items::template_epoch`] of the last resolve; an advance re-resolves.
    template_epoch: u64,
    /// The macro-table generation of the last resolve: a macro edit changes a bar icon while
    /// touching neither the action table nor any item template.
    macro_generation: u64,
}

pub(super) fn feed_actions(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    mut mount_errors: ResMut<MountErrors>,
    mut pet_tame_failures: ResMut<PetTameFailures>,
    // The by-key and resolved-text queues and the cover that holds the latter, as one parameter,
    // at Bevy's 16-parameter limit.
    ui_errors: (
        ResMut<UiErrorKeys>,
        ResMut<UiErrorTexts>,
        Option<Res<crate::loading_screen::LoadingScreen>>,
    ),
    spells: Option<Res<Spells>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let (mut ui_error_keys, mut ui_error_texts, cover) = ui_errors;
    let memory = memory.get(&script);

    // (Dis)mount refusals by key ([`mount_result_key`]); none of these strings takes arguments.
    let mount_texts: Vec<(&'static str, String)> = mount_errors
        .0
        .drain(..)
        .filter_map(|(mount, code)| {
            let key = mount_result_key(mount, code)?;
            Some((key, script.lua().globals().get::<String>(key).ok()?))
        })
        .collect();
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        mount_texts
            .into_iter()
            .map(|(key, text)| Shown::keyed(key, text)),
    );

    // Taming refusals ([`PetTameFailures`]): the reason's `PETTAME_*` string fills
    // `ERR_TAME_FAILED` ("%s."), two lookups in that order.
    let tame_texts: Vec<Shown> = pet_tame_failures
        .0
        .drain(..)
        .filter_map(|reason| {
            let reason_key = benilla_protocol::messages::pet_tame_failure_key(reason);
            let reason_text = script.lua().globals().get::<String>(reason_key).ok()?;
            super::keyed_line_s(&script, "ERR_TAME_FAILED", &[&reason_text])
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", tame_texts);

    // Client-local refusals by key ([`UiErrorKeys`]); the key also names the catalog record.
    let key_lines: Vec<Shown> = ui_error_keys
        .0
        .drain(..)
        .filter_map(|e| {
            ui_error_text(&e, &|key| script.lua().globals().get::<String>(key).ok())
                .map(|t| Shown::keyed(e.key, t))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", key_lines);

    // Already-resolved lines ([`UiErrorTexts`]); the queued kind is the `0x4945b0` flag. Held
    // while the loading cover is up, so a login notice's hold starts at the reveal: the reference
    // sends `CMSG_PLAYER_LOGIN` only after its world load (`0x46c272`), and its cover drops at the
    // player's create (`0x5debbd`), which vmangos sends ahead of the notices (`Maps/Map.cpp:432`,
    // `Handlers/CharacterHandler.cpp:675`).
    if !cover.is_some_and(|c| c.covering()) {
        let resolved: Vec<Shown> = ui_error_texts
            .0
            .drain(..)
            .map(|(text, kind)| Shown::unkeyed(kind, text))
            .collect();
        show_messages(&mut script, &mut sink, "ui_action", resolved);
    }

    // The engine's own by-key refusals (`ERR_PASSIVE_ABILITY`): `benilla_ui` cannot reach
    // [`UiErrorKeys`], so it queues keys that show here, one frame late.
    let engine_keys = script.take_ui_errors_with_input();
    let engine_lines: Vec<Shown> = engine_keys
        .into_iter()
        .filter_map(|(key, input)| {
            let e = UiError::key(key);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t).under(input))
        })
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", engine_lines);

    let store = self_q.iter().next();

    // Stance page: our form's bonus bar offset, with `UPDATE_BONUS_ACTIONBAR` on change.
    let form = store.map(|s| s.0.unit_shapeshift_form()).unwrap_or(0);
    let offset = spells
        .as_ref()
        .and_then(|s| s.forms.get(&u32::from(form)))
        .map(|f| f.bonus_bar)
        .unwrap_or(0) as u8;
    if offset != memory.bonus_offset {
        debug!("ui_action: bonus bar offset {} (form {form})", offset);
        memory.bonus_offset = offset;
        script.set_bonus_bar_offset(offset);
        script.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    }

    // The identity resolve reruns on a new VM, a bar edit, a macro edit or a landed item
    // template: a template is fetched ask-once, so a cold slot's first resolve only issues the
    // query, and the epoch advance redisplays it, like the reference's cache callback.
    let template_epoch = items.template_epoch();
    let macro_generation = script.macros_generation();
    let macros_moved = macro_generation != memory.macro_generation;
    if !memory.resolved || actions.dirty || template_epoch != memory.template_epoch || macros_moved
    {
        actions.dirty = false;
        memory.resolved = true;
        memory.template_epoch = template_epoch;
        memory.macro_generation = macro_generation;
        let macros = script.macros();

        // Each occupied slot's display, diffed against what the VM holds; a changed slot is
        // pushed and fires `ACTIONBAR_SLOT_CHANGED` with its Lua action id.
        let mut fresh: HashMap<u32, ActionSlot> = HashMap::new();
        for (slot, button) in &actions.buttons {
            let (texture, count, consumable, use_spell) = match button.kind {
                ACTION_KIND_SPELL => {
                    let icon = spells.as_ref().and_then(|sp| {
                        let d = sp.catalog.get(button.action)?;
                        spell_action_icon(
                            button.action,
                            d,
                            sp,
                            store,
                            &objects,
                            &items,
                            icons.as_deref(),
                        )
                    });
                    (icon, 0, false, None)
                }
                ACTION_KIND_ITEM => {
                    // Never nil: the reference's resolver `0x5d88b0` returns the question mark,
                    // and the stock `ActionButton.lua:158` hides the icon on a nil texture. The
                    // texture getter looks it up with a callback (`0x4e6b24`) on the bar's events,
                    // which no event here marks: an entry is asked when it first shows.
                    let template = items
                        .template(button.action, 0, Ask::New(&commands))
                        .cloned();
                    let texture = template
                        .as_ref()
                        .and_then(|t| icons.as_ref()?.catalog.get(t.display_info_id)?.icon.clone())
                        .unwrap_or_else(|| MISSING_ITEM_ICON.to_string());
                    let count = store
                        .map(|s| action_count(&s.0, &objects, button.action, template.as_ref()))
                        .unwrap_or(0);
                    // The Count gate (`0x4e5250`, [`ItemInfo::is_consumable`]) reads the icon's
                    // template, so it rides the same push.
                    let consumable = template.as_ref().is_some_and(|t| t.is_consumable());
                    // The spell `0x4e5a50` resolves the slot to (`0x4e5ae0`): the first block with
                    // a spell and trigger 0.
                    let use_spell = template.as_ref().and_then(|t| {
                        t.spells
                            .iter()
                            .find(|sp| sp.spell_id != 0 && sp.trigger == 0)
                            .map(|sp| sp.spell_id)
                    });
                    (Some(texture), count, consumable, use_spell)
                }
                // A macro slot shows the macro's own icon (`0x4e6bf9` calls `0x4f0fd0`), never its
                // bound spell's, though its state follows the bound spell.
                ACTION_KIND_MACRO => (
                    macros
                        .get(button.action as usize)
                        .and_then(|m| m.texture.clone()),
                    0,
                    false,
                    None,
                ),
                _ => (None, 0, false, None),
            };
            fresh.insert(
                u32::from(*slot) + 1,
                ActionSlot {
                    texture,
                    kind: button.kind,
                    action: button.action,
                    count,
                    consumable,
                    use_spell,
                },
            );
        }
        // A slot is pushed only when this feed's own source moved since its last push, so a
        // verb's write (`PickupAction`, `PlaceAction`, an addon's from a handler this frame) is
        // never rolled back by a store that has not caught up; it fires `ACTIONBAR_SLOT_CHANGED`
        // only when what it pushes differs visibly from what the VM shows, so the store catching
        // up with a verb is no change. The item's ON_USE spell is pushed but never signalled: the
        // place leaves it to this re-feed, and no handler paints it. A macro rename changes the
        // name line (`GetActionText`) but not the slot value, so a macro edit re-fires every
        // macro slot.
        let held = script.action_slots();
        let shown = |s: Option<&ActionSlot>| {
            s.map(|s| ActionSlot {
                use_spell: None,
                ..s.clone()
            })
        };
        let changed: Vec<(u32, bool)> = fresh
            .keys()
            .chain(memory.pushed.keys())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(|a| {
                let macro_moved =
                    macros_moved && fresh.get(&a).is_some_and(|s| s.kind == ACTION_KIND_MACRO);
                let moved = fresh.get(&a) != memory.pushed.get(&a);
                let signal = macro_moved || (moved && shown(fresh.get(&a)) != shown(held.get(&a)));
                (moved || macro_moved).then_some((a, signal))
            })
            .collect();
        for &(action, _) in &changed {
            script.set_action(action, fresh.get(&action).cloned());
        }
        memory.pushed = fresh;
        debug!(
            "ui_action: fed {} changed slot(s) ({} occupied)",
            changed.len(),
            memory.pushed.len()
        );
        for (action, _) in changed.into_iter().filter(|&(_, signal)| signal) {
            script.fire_event(
                "ACTIONBAR_SLOT_CHANGED",
                vec![ScriptValue::Int(i64::from(action))],
            );
        }
    }

    // A live icon changes without an action-table edit, so the pushed spell slots refresh every
    // frame. An item slot's count is the item watchers' (`super::recount`), never polled.
    if let Some(store) = store {
        for (&action, slot) in memory.pushed.iter_mut() {
            let changed = match slot.kind {
                // Live icons: a weapon face follows the equipped weapon and form (Attack's
                // `0x4e6870`), and a toggle's `ActiveIconID` follows its own aura (`0x4e6bbd`).
                ACTION_KIND_SPELL => {
                    let d = spells
                        .as_ref()
                        .and_then(|s| s.catalog.get(slot.action))
                        .filter(|d| substitutes_weapon_icon(d) || d.active_icon_id != 0);
                    match (d, spells.as_ref()) {
                        (Some(d), Some(sp)) => {
                            let fresh = spell_action_icon(
                                slot.action,
                                d,
                                sp,
                                Some(store),
                                &objects,
                                &items,
                                icons.as_deref(),
                            );
                            let changed = fresh != slot.texture;
                            if changed {
                                debug!(
                                    "ui_action: live icon swap slot {action} ({}) -> {fresh:?}",
                                    slot.action
                                );
                                slot.texture = fresh;
                            }
                            changed
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            // Only into the VM's slot when it still holds this action: a verb that moved another
            // there since keeps its write until the identity resolve catches up.
            if changed {
                let held = script.action_slot(action);
                if held
                    .as_ref()
                    .is_some_and(|h| (h.kind, h.action) == (slot.kind, slot.action))
                {
                    let mut next = held.unwrap_or_default();
                    next.texture = slot.texture.clone();
                    script.set_action(action, Some(next));
                    script.fire_event(
                        "ACTIONBAR_SLOT_CHANGED",
                        vec![ScriptValue::Int(i64::from(action))],
                    );
                }
            }
        }
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
