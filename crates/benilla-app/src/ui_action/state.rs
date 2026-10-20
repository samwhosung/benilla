//! Each action slot's per-frame state behind `IsUsableAction`, `IsActionInRange`,
//! `IsCurrentAction` and `GetActionCooldown`, diff-pushed into the VM with the reference's events
//! on each change: the cooldown trio (`0x4b31b0`, `0x4f93d0`), the usable pair (`0x4b31c0`, on a
//! cache change only, `0x4e5c00`), combat enter and leave (`0x6256ff`, `0x625778`), and
//! autorepeat start and stop (`0x6e5952`, `0x6ea170`), and between the usable pair each reactive
//! spell's `SPELL_ACTIVE` ([`super::reactive`]). `IsCurrentAction` reads the cast state
//! live, and its pair (`0x4b3250`) is signalled where the cast changes ([`crate::ui_cast`]).
//! Usability is `IsSpellUsableNow 0x6e3d60` ([`crate::spell::usable`]), auto-repeat the `0xceac30`
//! key, and range the squared distance against `GetMinMaxRange 0x6e3480`.

use crate::ui_items::stack_counts;
use std::collections::HashMap;
use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::RangeTargets;
use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::{ActionState, UiScript};

use crate::creature_anim::Engaged;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::spell::Cooldowns;
use crate::target::Selection;

use super::{PlayerActions, Spells};
use crate::query_cache::Ask;
use crate::spell::usable;
use crate::spell::AutoRepeatActive;

/// The feed's memory: what was last pushed, and the edge detectors.
#[derive(Default)]
pub(super) struct StateMemory {
    pushed: HashMap<u32, ActionState>,
    last_generation: Option<u64>,
    engaged: bool,
    auto_repeat: Option<u32>,
    last_cd_trace: Option<Instant>,
}

/// A slot after the macro indirection (`0x4e5a50`), as the usable compute `0x4e5050` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotResolve {
    /// A spell or item slot, or a macro whose bound spell is live (`[rec+0x564] > 0`).
    Action(u8, u32),
    /// A macro that casts nothing (`[rec+0x564] == 0`): usable while it exists (`0x4e5030`),
    /// with no cooldown, range or check (`0x4e50f4`-`0x4e516f`).
    BareMacro,
    /// Grey with nothing to report: a `/cast` that did not resolve (`-1`, refused at
    /// `0x4e518b`), or a macro that no longer exists.
    Dead,
}

/// Resolves a slot through its macro: every `Is*Action`/`GetActionCooldown` binding goes through
/// `0x4e5a50`, so a macro that casts Fireball is the Fireball slot, its own icon aside. Spells
/// only: 1.12 has no `/use`, so no macro names an item.
fn resolve_through_macro(
    kind: u8,
    action: u32,
    bound: &crate::ui_macro::MacroBoundSpells,
) -> SlotResolve {
    use crate::ui_macro::BoundSpell;
    match kind {
        ACTION_KIND_MACRO => match bound.0.get(&action) {
            Some(BoundSpell::Spell(s)) => SlotResolve::Action(ACTION_KIND_SPELL, *s),
            Some(BoundSpell::None) => SlotResolve::BareMacro,
            Some(BoundSpell::Unresolved) | None => SlotResolve::Dead,
        },
        other => SlotResolve::Action(other, action),
    }
}

/// The profession window a profession opener opens (`Effect[0]` 47, `0x4e54fe`), which
/// `IsCurrentAction` holds against the one open: a craft by its `EffectMiscValue[0]` (`0x4e5507`),
/// else a trade skill by its `SkillLineAbility` line (`0x6de040`, `0x4e5522`), as the opener opens
/// it (`0x6e4bd7`). A trade skill whose row is not loaded opens nothing, and checks nothing.
fn profession(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    skill_lines: Option<&crate::ui_spellbook::SkillLines>,
) -> Option<benilla_ui::script::Profession> {
    use benilla_ui::script::Profession;
    if d.effects[0] != benilla_formats::SPELL_EFFECT_TRADE_SKILL {
        return None;
    }
    match u32::try_from(d.effect_misc_value[0]).unwrap_or(0) {
        0 => skill_lines
            .and_then(|s| s.catalog.spell_to_line(spell_id))
            .map(Profession::TradeSkill),
        craft => Some(Profession::Craft(craft)),
    }
}

/// Computes and diff-pushes every occupied slot's state, and fires the reference's events.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_action_state(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut cooldowns: ResMut<Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    auto_repeat: Res<AutoRepeatActive>,
    // One tuple for Bevy's 16-param ceiling: macro bindings, spell mods, the range units, the
    // control flag, the skill lines and the active player's creation.
    cast_state: (
        Res<crate::ui_macro::MacroBoundSpells>,
        Res<crate::spell::SpellModifiers>,
        // The caster's and the target's reach and motion, which the range compare reads.
        crate::spell::RangeUnits,
        // `[0xb4b3e4]`, which the usable walk reads.
        Option<Res<crate::player::Player>>,
        // A profession opener's line (`0x6de040`), which the checked test reads, and the skill
        // lines the walk's cost finds its level through.
        Option<Res<crate::ui_spellbook::SkillLines>>,
        // The active player's creation, whose arrival (`0x5dea86`) empties the reactive list.
        Query<(), Added<SelfPlayer>>,
    ),
    self_q: Query<(&ObjectStore, &Transform, Has<Engaged>), With<SelfPlayer>>,
    selection: Res<Selection>,
    // One param for the guid index and the item store, for the same ceiling.
    objects: crate::net::Objects,
    units: Query<(&ObjectStore, &Transform), Without<SelfPlayer>>,
    relations: crate::target::RelationWorld,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<StateMemory>>,
    // The reactive list, which lives from one arrival to the next, not for a VM.
    mut reactive: Local<super::reactive::ReactiveList>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    let now = Instant::now();
    // Every cooldown pushes its absolute start on the `GetTime` clock, derived from this one
    // pair, so a running cooldown derives the same start every frame.
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    cooldowns.prune(now);
    let gen_changed = memory.last_generation != Some(cooldowns.generation);
    memory.last_generation = Some(cooldowns.generation);
    // The cooldown-clock trace (`WOW_MOVE_TRACE` sink, tag "cd"): once per second.
    let trace_cd = benilla_assets::trace::enabled()
        && memory
            .last_cd_trace
            .is_none_or(|t| now.duration_since(t).as_secs_f32() >= 1.0);
    if trace_cd {
        memory.last_cd_trace = Some(now);
    }

    let (bound, spell_mods, range_units, player, skill_lines, created) = &cast_state;
    let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
    let me = self_q.iter().next();
    let control_lost = player.as_ref().is_some_and(|p| p.own_control_lost);
    // The bags, walked once per frame for every reagent and totem count below.
    let stacks = me
        .map(|(s, _, _)| stack_counts(&s.0, &objects))
        .unwrap_or_default();
    let engaged = me.is_some_and(|(_, _, e)| e);
    let caster = range_units.caster();
    let self_pos = me.map(|(_, t, _)| t.translation);
    // The target's reach, motion and squared distance (the reference tests dx²+dy²+dz²,
    // `0x6e47b0`).
    let target_entity = selection.guid.and_then(|g| objects.entity(g));
    let target = target_entity.and_then(|e| units.get(e).ok());
    let target_unit = target_entity
        .filter(|_| target.is_some())
        .and_then(|e| range_units.unit(e));
    let dist_sq = match (self_pos, target) {
        (Some(a), Some((_, t))) => Some(a.distance_squared(t.translation)),
        _ => None,
    };

    let mut fresh: HashMap<u32, ActionState> = HashMap::new();
    for (&slot, button) in &actions.buttons {
        let action = u32::from(slot) + 1;
        let mut st = ActionState::default();
        let (kind, id) = match resolve_through_macro(button.kind, button.action, bound) {
            SlotResolve::Action(kind, id) => (kind, id),
            SlotResolve::BareMacro => {
                // `0x4e5050`'s spell-less leg: usable while the player-control flag `[0xb4b3e4]`
                // is set (`0x4e50f4`; `SMSG_CLIENT_CONTROL_UPDATE` clears it, `0x4958e0`).
                st.usable = !control_lost;
                fresh.insert(action, st);
                continue;
            }
            SlotResolve::Dead => {
                fresh.insert(action, st);
                continue;
            }
        };
        let button = &benilla_protocol::messages::ActionButton {
            slot,
            action: id,
            kind,
        };
        match button.kind {
            ACTION_KIND_SPELL => {
                let d = spells.as_ref().and_then(|s| s.catalog.get(button.action));
                let Some(d) = d else {
                    fresh.insert(action, st);
                    continue;
                };
                st.is_attack = d.is_melee_auto_attack();
                // What `IsCurrentAction 0x4e53a0` and `IsAutoRepeatAction 0x4e55b0` hold against
                // the live cast state. The form arm is not the icon's aura scan: a form granted by
                // another spell lights the check but not the icon.
                st.spell = Some(button.action);
                st.form = d.shapeshift_form;
                st.profession = profession(button.action, d, skill_lines.as_deref());
                // The usable walk (`0x6e3d60`); only its power gate sets `notEnoughMana`.
                if let (Some((store, _, _)), Some(sp)) = (me, spells.as_deref()) {
                    let ctx = usable::UsableCtx {
                        store,
                        target: selection.guid.zip(target.map(|(s, _)| s)),
                        rel: relations.get(),
                        cooldowns: &cooldowns,
                        stacks: &stacks,
                        spell_mods,
                        control_lost,
                        skill_lines: skill_catalog,
                    };
                    let (u, oom) =
                        usable::spell_usable(button.action, d, sp, &ctx, &objects, &items);
                    st.usable = u;
                    st.not_enough_mana = oom;
                } else {
                    st.usable = true;
                }
                let row = spells.as_ref().and_then(|s| s.ranges.get(d.range_index));
                // `ActionHasRange 0x4e5810` asks `GetMinMaxRange` with a null target (`0x4e5852`)
                // and tests `|min|` and `|max|` against `FLT_EPSILON`. The auto-attack target the
                // melee arm falls back to cannot move that: its max is at least the 5.0 floor.
                st.has_range = spell_mods
                    .min_max_range(d, row, caster, RangeTargets::default())
                    .is_some_and(|(min, max)| min.abs() > f32::EPSILON || max.abs() > f32::EPSILON);
                // `IsActionInRange 0x4e56f0` runs `CanTargetUnit 0x6e4440`, which binds the selected
                // target, then `IsTargetInRange 0x6e47b0`, which hands `GetMinMaxRange` that target
                // (`0x6e47ca`): its reach pads a ranged row, and its motion with the caster's earns
                // the moving bonus. Nil without a target.
                st.in_range = match dist_sq {
                    Some(d2) if st.has_range => {
                        let targets = RangeTargets {
                            target: target_unit,
                            attack_target: None,
                        };
                        spell_mods
                            .min_max_range(d, row, caster, targets)
                            .map(|(min, max)| d2 >= min * min && d2 <= max * max)
                    }
                    _ => None,
                };
                let info = cooldowns.info(button.action, 0, Some(d), now);
                st.cooldown = info.ui_triple(anchor, ui_now);
                if st.cooldown.is_some() && trace_cd {
                    // The store's clock against the widget's; the sink stamps wall time.
                    benilla_assets::trace::line(
                        "cd",
                        &format!(
                            "tick action={} rem={}ms dur={}ms engine_now={ui_now:.3}",
                            button.action, info.remaining_ms, info.duration_ms,
                        ),
                    );
                }
            }
            ACTION_KIND_ITEM => {
                // Only the `Copy` on-use spell, not a clone of the template per slot per frame.
                // The getter's lookup passes a callback (`0x4e5b3f`) on the bar's events, which no
                // event here marks: an entry is asked when it first shows.
                let use_spell = items
                    .template(button.action, 0, Ask::New(&commands))
                    .and_then(|t| t.use_spell);
                // The count the slot holds (`0xbc6390`), which `GetActionCount` answers too: the
                // gate reads the cache the recount wrote, never the bags (`0x4e50ab`).
                let count = script.action_slot(action).map_or(0, |slot| slot.count);
                // `IsEquippedAction`, the green border: `0x622270` at mask 1 (`0x4e5377`-`0x4e5380`),
                // the walker over the worn slots, so a broken copy is not worn.
                st.equipped = me.is_some_and(|(s, _, _)| {
                    crate::ui_items::find_item(
                        &s.0,
                        &objects,
                        button.action,
                        crate::ui_items::ItemSearch {
                            equipment_only: true,
                            ..Default::default()
                        },
                    )
                    .is_some()
                });
                // `0x4e5050`'s item arm runs the count, `IsItemOnCooldown` and the on-use spell
                // through the same `0x6e3d60` walk. With no player the reference answers (0, 0)
                // (`0x4e5080`), the default state.
                if let Some((store, _, _)) = me {
                    let ctx = usable::UsableCtx {
                        store,
                        target: selection.guid.zip(target.map(|(s, _)| s)),
                        rel: relations.get(),
                        cooldowns: &cooldowns,
                        stacks: &stacks,
                        spell_mods,
                        control_lost,
                        skill_lines: skill_catalog,
                    };
                    let (u, oom) = usable::item_usable(
                        button.action,
                        use_spell.as_ref(),
                        count > 0,
                        &ctx,
                        spells.as_deref(),
                        &objects,
                        &items,
                    );
                    st.usable = u;
                    st.not_enough_mana = oom;
                }
                if let Some(u) = use_spell {
                    let d = spells.as_ref().and_then(|s| s.catalog.get(u.spell_id));
                    let info = cooldowns.info_item(u.spell_id, button.action, u.category, d, now);
                    st.cooldown = info.ui_triple(anchor, ui_now);
                }
            }
            _ => {}
        }
        // The triple holds the absolute start, so a running cooldown diffs equal every frame and
        // a re-arm, even within one frame gap, restarts the sweep.
        fresh.insert(action, st);
    }

    let mut usable_changed = false;
    let keys: Vec<u32> = fresh
        .keys()
        .chain(memory.pushed.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for action in keys {
        let (new, old) = (fresh.get(&action), memory.pushed.get(&action));
        let d = ActionState::default();
        let (n, o) = (new.unwrap_or(&d), old.unwrap_or(&d));
        if new == old {
            continue;
        }
        if (n.usable, n.not_enough_mana) != (o.usable, o.not_enough_mana) {
            usable_changed = true;
        }
        if n.cooldown != o.cooldown && benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "push action={action} cooldown={:?} engine_now={:.3}",
                    n.cooldown,
                    script.now()
                ),
            );
        }
        script.set_action_state(action, new.copied());
    }
    memory.pushed = fresh;

    // The reactive list's re-check (`0x4b31d7`-`0x4b323d`), on the walk the bar takes.
    let list = &mut *reactive;
    let arrived = !created.is_empty();
    if arrived {
        list.arrive();
    }
    if let Some(sp) = spells.as_deref() {
        if arrived || actions.is_changed() || spells.as_ref().is_some_and(|s| s.is_changed()) {
            list.sync(&actions.spells, &sp.catalog);
        }
    }
    let reactive = match (me, spells.as_deref()) {
        (Some((store, _, _)), Some(sp)) => {
            let ctx = usable::UsableCtx {
                store,
                target: selection.guid.zip(target.map(|(s, _)| s)),
                rel: relations.get(),
                cooldowns: &cooldowns,
                stacks: &stacks,
                spell_mods,
                control_lost,
                skill_lines: skill_catalog,
            };
            list.refresh(|id| {
                sp.catalog
                    .get(id)
                    .is_some_and(|d| usable::spell_usable(id, d, sp, &ctx, &objects, &items).0)
            })
        }
        _ => super::reactive::Refresh::default(),
    };

    // The events in the reference's flush order, the `ACTIONBAR_*` one first.
    if gen_changed {
        script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
        script.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
        script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
    }
    // `0x4b31c0`: the bar's event on a slot change (`0x4e5c53`), each reactive spell's word as it
    // turns usable (`0x4b3226`), then the spells' event (`0x4b3246`).
    if usable_changed {
        script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    }
    for name in reactive.activated {
        crate::ui_unit::fire_combat_text(&mut script, "SPELL_ACTIVE", Some(name), None);
    }
    if usable_changed || reactive.changed {
        script.fire_event("SPELL_UPDATE_USABLE", vec![]);
    }
    if engaged != memory.engaged {
        memory.engaged = engaged;
        script.fire_event(
            if engaged {
                "PLAYER_ENTER_COMBAT"
            } else {
                "PLAYER_LEAVE_COMBAT"
            },
            vec![],
        );
    }
    if auto_repeat.0 != memory.auto_repeat {
        memory.auto_repeat = auto_repeat.0;
        script.fire_event(
            if auto_repeat.0.is_some() {
                "START_AUTOREPEAT_SPELL"
            } else {
                "STOP_AUTOREPEAT_SPELL"
            },
            vec![],
        );
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
