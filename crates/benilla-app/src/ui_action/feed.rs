//! The action bar's identity feed: each of the 120 slots' icon and count, pushed to the VM with
//! `ACTIONBAR_SLOT_CHANGED` per changed slot. [`feed_actions`] also drains the UI error queues
//! and feeds the stance page (`GetBonusBarOffset`: the form's `SpellShapeshiftForm.dbc`
//! BonusActionBar column, `0x4e4fc0`).

use crate::ui_items::{count_of, InventoryScope};
use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::{ActionSlot, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};

use super::errors::{first_missing_totem, first_short_reagent, mount_result_key};
use super::weapon_icon::{auto_attack_icon, substitutes_weapon_icon};
use super::{
    cast_fail, show_messages, ui_error_text, CastErrors, MessageSink, MountErrors, PetTameFailures,
    PlayerActions, Shown, Spells, UiError, UiErrorKeys, UiErrorTexts,
};

/// An item action's icon when its template has not answered or its display id has no
/// `ItemDisplayInfo` row: the reference's literal `0x847fe4`, from the resolver's `0x5d8927`.
pub(super) const MISSING_ITEM_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

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

/// The DBC name tables the cast-fail argument arms read, as one parameter to stay within Bevy's
/// parameter limit; each is absent without game data. The shapeshift forms ride [`Spells`].
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct FailNameTables<'w> {
    /// `SpellFocusObject.dbc`, for `0x5e`.
    focus: Option<Res<'w, crate::ui_tradeskill::SpellFocus>>,
    /// `AreaTable.dbc`, for `0x5d`.
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    /// `SpellMechanic.dbc`, for `0x8d`.
    mechanics: Option<Res<'w, super::SpellMechanics>>,
}

impl FailNameTables<'_> {
    fn args<'a>(&'a self, spells: Option<&'a Spells>) -> cast_fail::FailArgs<'a> {
        cast_fail::FailArgs {
            arg: None,
            focus: self.focus.as_deref().map(|f| &f.catalog),
            areas: self.areas.as_deref().map(|a| &a.0),
            mechanics: self.mechanics.as_deref().map(|m| &m.catalog),
            forms: spells.map(|s| &s.forms),
        }
    }
}

pub(super) fn feed_actions(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    mut cast_errors: ResMut<CastErrors>,
    mut mount_errors: ResMut<MountErrors>,
    mut pet_tame_failures: ResMut<PetTameFailures>,
    // The by-key and resolved-text queues as one parameter, at Bevy's 16-parameter limit.
    ui_errors: (ResMut<UiErrorKeys>, ResMut<UiErrorTexts>),
    spells: Option<Res<Spells>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    sub_classes: Option<Res<crate::ui_items::ItemSubClasses>>,
    name_tables: FailNameTables,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<FeedMemory>>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let (mut ui_error_keys, mut ui_error_texts) = ui_errors;
    let memory = memory.get(&script);

    // Cast failures become error lines through [`cast_fail`], all resolved before any event
    // fires. The drain owns the arms that need our bags or the item cache: `0x78` and `0x5c`
    // (`0x6e1e7f`) name the failing slot's item, found as the pre-send check finds it. On a cache
    // miss the entry stays queued, as the reference's callback `0x6e29b0` redisplays on the answer.
    let self_store = self_q.iter().next();
    let mut await_template: Vec<crate::ui_action::CastFail> = Vec::new();
    // The same failures for the combat log: `0x6e1a00` calls both `DisplayError` and the log
    // formatter `0x62c360`.
    let mut fail_lines: Vec<crate::ui_chat::combat::PendingCombat> = Vec::new();
    let fail_args = name_tables.args(spells.as_deref());
    let texts: Vec<cast_fail::CastFailLine> = cast_errors
        .0
        .drain(..)
        .filter_map(|fail| {
            let crate::ui_action::CastFail {
                spell_id,
                reason,
                caster,
                ..
            } = fail;
            let pet = caster == crate::ui_action::Caster::Pet;
            let d = spells.as_ref().and_then(|s| s.catalog.get(spell_id));
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            // The displayed line first and the log line from it, the reference's order.
            let line = (|| -> Option<cast_fail::CastFailLine> {
                // `0x19`-`0x1b` (`0x6e1db7`, through `0x6e2380`): the singular subclass name,
                // "Must have a Wand equipped"; a multi-bit mask names its `ItemSubClassMask.dbc`
                // group, else the first subclass, where the tooltip (`0x52eea7`) uses the plural
                // and joins. The pet's table fills it too (`0x6e904d`).
                if let (0x19..=0x1b, Some(d), Some(subs)) = (reason, d, sub_classes.as_deref()) {
                    if let Some(name) = (d.equipped_item_class >= 0)
                        .then(|| {
                            subs.0.requirement_display_name(
                                d.equipped_item_class as u32,
                                d.equipped_item_subclass_mask,
                            )
                        })
                        .flatten()
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x31` (`0x6e1e54`): the same helper for class 6 (Projectile), mask `1 << arg`,
                // the shift masked to five bits as x86 `shl` masks it (`0x6e1e5c`). The player's
                // arm only: the pet's table (`0x6e93d0`) has no `0x31` entry. vmangos never sends
                // the word (`Spell.cpp:4419-4443`), so against it this declines to the stem.
                if let (false, 0x31, Some(arg), Some(subs)) =
                    (pet, reason, fail.arg, sub_classes.as_deref())
                {
                    const ITEM_CLASS_PROJECTILE: u32 = 6;
                    if let Some(name) = subs
                        .0
                        .requirement_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
                    {
                        let key = cast_fail::CAST_FAIL_KEYS[reason as usize];
                        return get(key)
                            .filter(|s| !s.is_empty())
                            .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                    }
                }
                // `0x78` and `0x5c`, the player's arms only: the pet's table sends both to its
                // generic arm.
                if !pet && (reason == 0x78 || reason == 0x5c) {
                    let d = d?;
                    let failing = if reason == 0x78 {
                        self_store
                            .and_then(|s| first_missing_totem(d, s, &objects))
                            // No self store yet: name the first tool.
                            .or_else(|| d.totems.iter().copied().find(|&t| t != 0))
                    } else {
                        self_store
                            .and_then(|s| first_short_reagent(d, s, &objects))
                            .or_else(|| d.reagents.iter().map(|&(id, _)| id).find(|&id| id != 0))
                    }?;
                    let cached = items
                        .template(failing, 0, &commands)
                        .map(|i| i.name.clone());
                    let name = match cached {
                        Some(name) => name,
                        // Answered unknown: the reference's callback literal (`0x838044`).
                        None if items.template_answered_unknown(failing) => "UNKNOWN".to_string(),
                        None => {
                            // Pending: requeued as a redisplay, which is not logged.
                            await_template.push(fail.requeued());
                            return None;
                        }
                    };
                    let key = if reason == 0x78 {
                        "SPELL_FAILED_TOTEMS"
                    } else {
                        "SPELL_FAILED_REAGENTS"
                    };
                    return get(key)
                        .filter(|s| !s.is_empty())
                        .map(|t| cast_fail::CastFailLine::passthrough(t.replace("%s", &name)));
                }
                cast_fail::cast_fail_text(
                    caster,
                    reason,
                    d,
                    cast_fail::FailArgs {
                        arg: fail.arg,
                        ..fail_args
                    },
                    &get,
                )
            })();
            // The reason, its wire word and both resolved buffers, so a declined arm is readable
            // from a probe run.
            debug!(
                "ui_action: cast fail — {caster:?} spell {spell_id} reason {reason:#04x} \
                 arg {:?} → {:?}",
                fail.arg, line
            );
            // The combat-log line, after the display as in the reference (`DisplayError` at
            // `0x6e21dd`, the log formatter `0x62c360` at `0x6e21fc`), from the argText buffer
            // ([`CastFailLine::logged`]). Nothing is logged for a line that shows nothing, for a
            // redisplay (`0x6e29b0` skips the log) or for the pet (`0x6e8eb0` never calls
            // `0x62c360`). `SMSG_CAST_RESULT` goes to the caster alone, so the `OTHER` keys stay
            // unused.
            if let (Some(shown), Some(display), false, false) =
                (line.as_ref(), d, pet, fail.redisplay)
            {
                // `0x62aff0`: `Attributes` bit 4 marks an ability, which "performs", not "casts".
                const ATTR_IS_ABILITY: u32 = 0x10;
                let family = if display.attributes & ATTR_IS_ABILITY != 0 {
                    crate::ui_chat::combat::SPELLFAILPERFORM
                } else {
                    crate::ui_chat::combat::SPELLFAILCAST
                };
                if !display.name.is_empty() {
                    fail_lines.push(crate::ui_chat::combat::PendingCombat {
                        kind: crate::ui_chat::ChatEventKind::SpellFailedLocalPlayer,
                        family,
                        variant: crate::ui_chat::combat::Variant::SelfOther,
                        subject: 0,
                        object: 0,
                        fills: crate::ui_chat::combat::Fills {
                            spell: display.name.clone(),
                            named: shown.logged().to_string(),
                            ..Default::default()
                        },
                        named: crate::ui_chat::combat::Named::Ready,
                        tries: 0,
                    });
                }
            }
            line
        })
        .collect();
    cast_errors.0.extend(await_template);
    for line in fail_lines {
        sink.chat.push_combat(line);
    }
    show_messages(
        &mut script,
        &mut sink,
        "ui_action",
        texts.into_iter().map(|l| Shown::keyed(l.key, l.text)),
    );

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

    // Already-resolved lines ([`UiErrorTexts`]); the queued kind is the `0x4945b0` flag.
    let resolved: Vec<Shown> = ui_error_texts
        .0
        .drain(..)
        .map(|(text, kind)| Shown::unkeyed(kind, text))
        .collect();
    show_messages(&mut script, &mut sink, "ui_action", resolved);

    // The engine's own by-key refusals (`ERR_PASSIVE_ABILITY`): `benilla_ui` cannot reach
    // [`UiErrorKeys`], so it queues keys that show here, one frame late.
    let engine_keys = script.take_ui_errors();
    let engine_lines: Vec<Shown> = engine_keys
        .into_iter()
        .filter_map(|key| {
            let e = UiError::key(key);
            ui_error_text(&e, &|k| script.lua().globals().get::<String>(k).ok())
                .map(|t| Shown::keyed(e.key, t))
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
            let (texture, count, consumable) = match button.kind {
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
                            &commands,
                        )
                    });
                    (icon, 0, false)
                }
                ACTION_KIND_ITEM => {
                    // Never nil: the reference's resolver `0x5d88b0` returns the question mark,
                    // and the stock `ActionButton.lua:158` hides the icon on a nil texture.
                    let template = items.template(button.action, 0, &commands).cloned();
                    let texture = template
                        .as_ref()
                        .and_then(|t| icons.as_ref()?.catalog.get(t.display_info_id)?.icon.clone())
                        .unwrap_or_else(|| MISSING_ITEM_ICON.to_string());
                    let count = store
                        .map(|s| count_of(&s.0, &objects, button.action, InventoryScope::CARRIED))
                        .unwrap_or(0);
                    // The Count gate (`0x4e5250`, [`ItemInfo::is_consumable`]) reads the icon's
                    // template, so it rides the same push.
                    let consumable = template.as_ref().is_some_and(|t| t.is_consumable());
                    (Some(texture), count, consumable)
                }
                // A macro slot shows the macro's own icon (`0x4e6bf9` calls `0x4f0fd0`), never its
                // bound spell's, though its state follows the bound spell.
                ACTION_KIND_MACRO => (
                    macros
                        .get(button.action as usize)
                        .and_then(|m| m.texture.clone()),
                    0,
                    false,
                ),
                _ => (None, 0, false),
            };
            fresh.insert(
                u32::from(*slot) + 1,
                ActionSlot {
                    texture,
                    kind: button.kind,
                    action: button.action,
                    count,
                    consumable,
                },
            );
        }
        // A macro rename changes the name line (`GetActionText`) but not the slot value, so a
        // macro edit re-fires every macro slot.
        let changed: Vec<u32> = fresh
            .keys()
            .chain(memory.pushed.keys())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|a| {
                fresh.get(a) != memory.pushed.get(a)
                    || (macros_moved && fresh.get(a).is_some_and(|s| s.kind == ACTION_KIND_MACRO))
            })
            .collect();
        for &action in &changed {
            script.set_action(action, fresh.get(&action).cloned());
        }
        memory.pushed = fresh;
        debug!(
            "ui_action: fed {} changed slot(s) ({} occupied)",
            changed.len(),
            memory.pushed.len()
        );
        for action in changed {
            script.fire_event(
                "ACTIONBAR_SLOT_CHANGED",
                vec![ScriptValue::Int(i64::from(action))],
            );
        }
    }

    // An item slot's count and a live icon change without an action-table edit (eating a stack
    // sends no `SMSG_ACTION_BUTTONS`), so the pushed slots refresh every frame.
    if let Some(store) = store {
        for (&action, slot) in memory.pushed.iter_mut() {
            let changed = match slot.kind {
                ACTION_KIND_ITEM => {
                    let fresh = count_of(&store.0, &objects, slot.action, InventoryScope::CARRIED);
                    let changed = fresh != slot.count;
                    if changed {
                        slot.count = fresh;
                    }
                    changed
                }
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
                                &commands,
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
            if changed {
                script.set_action(action, Some(slot.clone()));
                script.fire_event(
                    "ACTIONBAR_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(action))],
                );
            }
        }
    }
}

/// A spell slot's icon, by the reference's `GetActionTexture` resolver `0x4e6a50` in order: the
/// auto-attack faces ([`auto_attack_icon`], `0x4e6870`, `0x4e6990`), then `ActiveIconID` while
/// the spell's own aura is live and cancelable (`0x4e6bbd`, the `0x4e55f0` predicate of
/// [`super::toggle::active_action_toggle`]), then the spell's icon. The spellbook's
/// `GetSpellTexture` (`0x4b3f50`) never serves `ActiveIconID`.
fn spell_action_icon(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    spells: &super::Spells,
    store: Option<&crate::net::ObjectStore>,
    objects: &crate::net::Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    auto_attack_icon(d, store, &spells.forms, objects, items, icons, commands)
        .or_else(|| {
            store
                .filter(|s| super::toggle::active_action_toggle(spell_id, d, s))
                .and_then(|_| d.active_icon.clone())
        })
        .or_else(|| d.icon.clone())
}
