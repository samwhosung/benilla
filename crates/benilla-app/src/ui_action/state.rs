//! Each action slot's per-frame state behind `IsUsableAction`, `IsActionInRange`,
//! `IsCurrentAction` and `GetActionCooldown`, diff-pushed into the VM with the reference's events
//! on each change: the cooldown trio (`0x4b31b0`, `0x4f93d0`), the usable pair (`0x4b31c0`, on a
//! cache change only, `0x4e5c00`), combat enter and leave (`0x6256ff`, `0x625778`), and
//! autorepeat start and stop (`0x6e5952`, `0x6ea170`). `IsCurrentAction` reads the cast state
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
    // control flag and the skill lines.
    cast_state: (
        Res<crate::ui_macro::MacroBoundSpells>,
        Res<crate::spell::SpellModifiers>,
        // The caster's and the target's reach and motion, which the range compare reads.
        crate::spell::RangeUnits,
        // `[0xb4b3e4]`, which the usable walk reads.
        Option<Res<crate::player::Player>>,
        // A profession opener's line (`0x6de040`), which the checked test reads.
        Option<Res<crate::ui_spellbook::SkillLines>>,
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

    let (bound, spell_mods, range_units, player, skill_lines) = &cast_state;
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

    // The events in the reference's flush order, the `ACTIONBAR_*` one first.
    if gen_changed {
        script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
        script.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
        script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
    }
    if usable_changed {
        script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
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
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;

    /// A profession opener (`Effect[0]` 47) names the window `IsCurrentAction` checks it against
    /// (`0x4e54fe`): a nonzero `EffectMiscValue[0]` the craft of that type (Enchanting 3), zero
    /// the trade skill of its `SkillLineAbility` line (Tailoring 3908, line 197); any other spell
    /// none.
    #[test]
    fn a_profession_opener_names_its_window() {
        use benilla_ui::script::Profession;
        let lines = crate::ui_spellbook::SkillLines {
            catalog: benilla_formats::SkillLineCatalog::from_spell_lines([(3908, 197)]),
        };
        let opener = |misc| SpellDisplay {
            effects: [benilla_formats::SPELL_EFFECT_TRADE_SKILL, 0, 0],
            effect_misc_value: [misc, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            profession(7411, &opener(3), Some(&lines)),
            Some(Profession::Craft(3))
        );
        assert_eq!(
            profession(3908, &opener(0), Some(&lines)),
            Some(Profession::TradeSkill(197))
        );
        assert_eq!(profession(3908, &opener(0), None), None, "no rows loaded");
        assert_eq!(
            profession(133, &SpellDisplay::default(), Some(&lines)),
            None
        );
    }

    /// `[rec+0x564]`'s three values (`0x4e5a50`): a live spell, a bare macro, and grey.
    #[test]
    fn a_macro_slot_resolves_through_its_bound_spell() {
        use crate::ui_macro::BoundSpell;
        use benilla_protocol::messages::ACTION_KIND_MACRO;

        let mut bound = crate::ui_macro::MacroBoundSpells::default();
        bound.0.insert(3, BoundSpell::Spell(133)); // macro 3 casts Fireball
        bound.0.insert(4, BoundSpell::None); // macro 4 is `.spawn 16032`
        bound.0.insert(5, BoundSpell::Unresolved); // macro 5 is `/cast Pyroblast`, unknown

        assert_eq!(
            resolve_through_macro(ACTION_KIND_MACRO, 3, &bound),
            SlotResolve::Action(ACTION_KIND_SPELL, 133),
            "from here down the macro IS the Fireball slot"
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_MACRO, 4, &bound),
            SlotResolve::BareMacro,
            "a macro that casts nothing is usable, with no cooldown, no range"
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_MACRO, 5, &bound),
            SlotResolve::Dead,
            "a /cast of an unknown spell is the reference's -1: grey"
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_MACRO, 6, &bound),
            SlotResolve::Dead,
            "a slot whose macro no longer exists: grey"
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_SPELL, 133, &bound),
            SlotResolve::Action(ACTION_KIND_SPELL, 133)
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_ITEM, 117, &bound),
            SlotResolve::Action(ACTION_KIND_ITEM, 117)
        );
    }

    #[test]
    fn the_feed_pushes_a_bare_macro_as_usable() {
        use crate::ui_macro::{BoundSpell, MacroBoundSpells};
        use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        for (slot, index) in [(0u8, 1u32), (1, 2), (2, 3)] {
            actions.buttons.insert(
                slot,
                ActionButton {
                    slot,
                    action: index,
                    kind: ACTION_KIND_MACRO,
                },
            );
        }
        // Macro 1 is `.spawn 16032`, 2 an unknown `/cast Pyroblast`, and 3 does not exist.
        let mut bound = MacroBoundSpells::default();
        bound.0.insert(1, BoundSpell::None);
        bound.0.insert(2, BoundSpell::Unresolved);
        app.insert_resource(actions)
            .insert_resource(bound)
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_action_state);
        app.update();

        let script = app.world().non_send_resource::<UiScript>();
        let usable = |action: u32| {
            script
                .eval::<bool>(&format!(
                    "return (IsUsableAction({action})) and true or false"
                ))
                .unwrap()
        };
        assert!(usable(1), "a macro that casts nothing is a usable button");
        assert!(
            !usable(2),
            "a /cast of an unknown spell is grey (the reference's -1)"
        );
        assert!(!usable(3), "a slot whose macro no longer exists is grey");
        assert!(
            !script
                .eval::<bool>("local _, oom = IsUsableAction(2) return oom and true or false")
                .unwrap(),
            "grey, not the out-of-power blue: notEnoughMana stays 0 on the spell-less leg"
        );
    }

    /// `IsActionInRange` (`0x4e56f0`) hands `GetMinMaxRange` the selected unit, so a ranged row
    /// pads by that unit's reach and the player's alone: a Fireball (0-35) button reads in range
    /// out to 38 yards from a 1.5-reach target, whoever the player swings at, and `ActionHasRange`
    /// (`0x4e5810`, a null target) says the button has a range.
    #[test]
    fn a_ranged_button_pads_its_range_by_the_selected_target_alone() {
        use benilla_protocol::messages::ActionButton;
        use benilla_protocol::ObjectFields;

        const FIREBALL: u32 = 133;
        const SWUNG_AT: u64 = 0xA1;
        const SELECTED: u64 = 0xB2;
        /// `UNIT_FIELD_COMBATREACH`.
        const REACH: u16 = 130;

        let verdict = |distance: f32| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            let mut actions = PlayerActions::default();
            actions.buttons.insert(
                0,
                ActionButton {
                    slot: 0,
                    action: FIREBALL,
                    kind: ACTION_KIND_SPELL,
                },
            );
            let fireball = SpellDisplay {
                range_index: 4,
                ..Default::default()
            };
            let row = benilla_formats::SpellRange {
                min: 0.0,
                max: 35.0,
                flags: 0,
            };
            app.insert_resource(actions)
                .insert_resource(crate::ui_macro::MacroBoundSpells::default())
                .insert_resource(Spells {
                    catalog: benilla_formats::SpellCatalog::from_displays(
                        [(FIREBALL, fireball)].into_iter().collect(),
                    ),
                    forms: Default::default(),
                    ranges: benilla_formats::SpellRangeCatalog::from_rows(
                        [(4, row)].into_iter().collect(),
                    ),
                    cast_times: Default::default(),
                    durations: Default::default(),
                    radii: Default::default(),
                })
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_script::UiClock>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::PendingCast>()
                .init_resource::<crate::spell::QueuedMeleeSpell>()
                .init_resource::<crate::spell::ActiveChannel>()
                .init_resource::<crate::spell::SpellTargeting>()
                .init_resource::<crate::spell::HeldForPick>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::net::Reputations>()
                .init_resource::<Items>()
                .insert_resource(NetCommands(tx));
            // The player swings at a 4.0-reach unit, close by.
            let swung_at = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(2.0, 0.0, 0.0),
                    ObjectStore(ObjectFields::from_pairs(&[
                        (22, 100),
                        (REACH, 4.0f32.to_bits()),
                    ])),
                ))
                .id();
            app.world_mut().spawn((
                SelfPlayer,
                Engaged(SWUNG_AT),
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
            ));
            // The selected unit keeps the descriptor's default 1.5 reach.
            let selected = app
                .world_mut()
                .spawn((
                    Transform::from_xyz(distance, 0.0, 0.0),
                    ObjectStore(ObjectFields::from_pairs(&[(22, 100)])),
                ))
                .id();
            let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
            index.0.insert(SWUNG_AT, swung_at);
            index.0.insert(SELECTED, selected);
            app.insert_resource(Selection {
                target: Some(selected),
                guid: Some(SELECTED),
                last: None,
            });
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.add_systems(Update, feed_action_state);
            app.update();
            let script = app.world().non_send_resource::<UiScript>();
            assert!(script
                .eval::<bool>("return ActionHasRange(1) == 1")
                .unwrap());
            script
                .eval::<i64>("return IsActionInRange(1) or -1")
                .unwrap()
        };

        // 35 + the caster's 1.5 + the selected unit's 1.5 = 38.
        assert_eq!(verdict(37.5), 1, "inside the padded max");
        assert_eq!(
            verdict(38.5),
            0,
            "past it: the swing's 4.0 reach pads nothing"
        );
    }

    /// `IsActionInRange` hands `GetMinMaxRange` the selected unit, so a melee button reads in
    /// range out to 7.667 yards while the player and the selected mob both run, and to the 5.0
    /// floor otherwise (`0x6e3648`-`0x6e36a2`).
    #[test]
    fn a_melee_button_reaches_the_moving_bonus_while_both_units_run() {
        use benilla_protocol::messages::ActionButton;
        use benilla_protocol::ObjectFields;

        const HAMSTRING: u32 = 1715;
        const SELECTED: u64 = 0xB2;

        let verdict = |caster_runs: bool, mob_runs: bool, distance: f32| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            let mut actions = PlayerActions::default();
            actions.buttons.insert(
                0,
                ActionButton {
                    slot: 0,
                    action: HAMSTRING,
                    kind: ACTION_KIND_SPELL,
                },
            );
            let melee = benilla_formats::SpellRange {
                min: 0.0,
                max: 5.0,
                flags: 1,
            };
            app.insert_resource(actions)
                .insert_resource(crate::ui_macro::MacroBoundSpells::default())
                .insert_resource(Spells {
                    catalog: benilla_formats::SpellCatalog::from_displays(
                        [(
                            HAMSTRING,
                            SpellDisplay {
                                range_index: 2,
                                ..Default::default()
                            },
                        )]
                        .into_iter()
                        .collect(),
                    ),
                    forms: Default::default(),
                    ranges: benilla_formats::SpellRangeCatalog::from_rows(
                        [(2, melee)].into_iter().collect(),
                    ),
                    cast_times: Default::default(),
                    durations: Default::default(),
                    radii: Default::default(),
                })
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_script::UiClock>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::PendingCast>()
                .init_resource::<crate::spell::QueuedMeleeSpell>()
                .init_resource::<crate::spell::ActiveChannel>()
                .init_resource::<crate::spell::SpellTargeting>()
                .init_resource::<crate::spell::HeldForPick>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::net::Reputations>()
                .init_resource::<Items>()
                .insert_resource(crate::player::Player::with_move_flags(if caster_runs {
                    crate::creature_anim::move_flags::FORWARD
                } else {
                    0
                }))
                .insert_resource(NetCommands(tx));
            let speeds = crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
                walk: 2.5,
                run: 7.0,
                run_back: 4.5,
                swim: 4.7,
                swim_back: 2.5,
                turn_rate: 3.1,
            });
            app.world_mut().spawn((
                SelfPlayer,
                crate::net::Embodied,
                speeds,
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
            ));
            // A mob at the descriptor's default 1.5 reach; a live spline at 8 yd/s is its run.
            let mob = app
                .world_mut()
                .spawn((
                    speeds,
                    Transform::from_xyz(distance, 0.0, 0.0),
                    ObjectStore(ObjectFields::from_pairs(&[(22, 100)])),
                ))
                .id();
            if mob_runs {
                app.world_mut().entity_mut(mob).insert(crate::net::Spline {
                    points: vec![[distance, 0.0, 0.0], [distance + 16.0, 0.0, 0.0]],
                    start: Instant::now(),
                    duration: std::time::Duration::from_secs(2),
                    id: 1,
                    grounded: true,
                    run_mode: true,
                    deck: None,
                });
            }
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(SELECTED, mob);
            app.insert_resource(Selection {
                target: Some(mob),
                guid: Some(SELECTED),
                last: None,
            });
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.add_systems(Update, feed_action_state);
            app.update();
            let script = app.world().non_send_resource::<UiScript>();
            script
                .eval::<i64>("return IsActionInRange(1) or -1")
                .unwrap()
        };

        // 1.5 + 1.5 + 1.3333 is under the 5.0 floor; the bonus goes on the floor: 7.667.
        assert_eq!(verdict(true, true, 7.5), 1, "both running: inside 7.667");
        assert_eq!(verdict(true, true, 8.0), 0, "past it");
        assert_eq!(verdict(false, true, 7.5), 0, "the player stands");
        assert_eq!(verdict(true, false, 7.5), 0, "the mob stands");
        assert_eq!(verdict(false, false, 4.9), 1, "the floor");
    }

    /// An item slot's on-use spell takes the `0x6e3d60` walk (`0x4e5050`), and every shipped
    /// Food/Drink spell has `Attributes` bit 28, `ATTR_NOT_IN_COMBAT` (the walk's leg 8).
    #[test]
    fn food_on_the_bar_greys_while_the_player_is_in_combat() {
        use benilla_protocol::messages::{ActionButton, ItemUseSpell};
        use benilla_protocol::ObjectFields;

        // One ON_USE block casting 433 "Food", whose shipped `Attributes` are `0x18000100`.
        const FOOD_ITEM: u32 = 1487;
        const FOOD_SPELL: u32 = 433;
        // Raw indices: `ITEM_FIELD_STACK_COUNT`, and `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1.
        const STACK: u16 = 14;
        const PACK_SLOT_1: u16 = 532;

        let lit = |in_combat: bool| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            let mut actions = PlayerActions::default();
            actions.buttons.insert(
                0,
                ActionButton {
                    slot: 0,
                    action: FOOD_ITEM,
                    kind: ACTION_KIND_ITEM,
                },
            );
            let mut items = Items::default();
            items.insert_template(
                FOOD_ITEM,
                Some(benilla_protocol::messages::ItemInfo {
                    use_spell: Some(ItemUseSpell {
                        spell_id: FOOD_SPELL,
                        cooldown_ms: -1,
                        category: 0,
                        category_cooldown_ms: -1,
                    }),
                    ..crate::items::test_template("Conjured Muffin")
                }),
            );
            let food = SpellDisplay {
                attributes: 0x1800_0100,
                ..Default::default()
            };
            app.insert_resource(actions)
                .insert_resource(crate::ui_macro::MacroBoundSpells::default())
                .insert_resource(Spells {
                    catalog: benilla_formats::SpellCatalog::from_displays(
                        [(FOOD_SPELL, food)].into_iter().collect(),
                    ),
                    forms: Default::default(),
                    ranges: Default::default(),
                    cast_times: Default::default(),
                    durations: Default::default(),
                    radii: Default::default(),
                })
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_script::UiClock>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::PendingCast>()
                .init_resource::<crate::spell::QueuedMeleeSpell>()
                .init_resource::<crate::spell::ActiveChannel>()
                .init_resource::<crate::spell::SpellTargeting>()
                .init_resource::<crate::spell::HeldForPick>()
                .init_resource::<Selection>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::net::Reputations>()
                .insert_resource(items)
                .insert_resource(NetCommands(tx));
            crate::items::test_spawn_item(
                app.world_mut(),
                0xF0,
                ObjectFields::from_pairs(&[(3, FOOD_ITEM), (STACK, 10)]),
                false,
            );
            // The player: alive, with the ten muffins in backpack slot 1.
            let flags = (1u32 << 3)
                | if in_combat {
                    crate::player::UNIT_FLAG_IN_COMBAT
                } else {
                    0
                };
            app.world_mut().spawn((
                SelfPlayer,
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),
                    (23, 500),
                    (46, flags),
                    (PACK_SLOT_1, 0xF0),
                ])),
            ));
            // The slot as the identity feed pushed it, with the ten muffins it counted.
            let mut script = UiScript::new().unwrap();
            script.set_action(
                1,
                Some(benilla_ui::script::ActionSlot {
                    kind: ACTION_KIND_ITEM,
                    action: FOOD_ITEM,
                    count: 10,
                    ..Default::default()
                }),
            );
            app.insert_non_send_resource(script);
            app.add_systems(Update, feed_action_state);
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>("return (IsUsableAction(1)) and true or false")
                .unwrap()
        };

        assert!(lit(false), "out of combat the muffins are full-colour");
        assert!(
            !lit(true),
            "in combat the food's on-use spell fails leg 8 and the button greys"
        );
    }

    /// `0x4e5050` reads the slot's cached count (`[0xbc6390]`, the one `GetActionCount` answers),
    /// not the bags: a zero there greys the slot while a copy sits in the pack, and a count there
    /// lights it with nothing in the bags, as for a worn copy the recount stored.
    #[test]
    fn an_item_slot_is_held_by_its_stored_count_not_the_bags() {
        use benilla_protocol::messages::ActionButton;
        use benilla_protocol::ObjectFields;

        const ITEM: u32 = 2512;
        // `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1.
        const PACK_SLOT_1: u16 = 532;

        let lit = |stored: u32, in_pack: bool| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            let mut actions = PlayerActions::default();
            actions.buttons.insert(
                0,
                ActionButton {
                    slot: 0,
                    action: ITEM,
                    kind: ACTION_KIND_ITEM,
                },
            );
            let mut items = Items::default();
            items.insert_template(ITEM, Some(crate::items::test_template("Rough Arrow")));
            app.insert_resource(actions)
                .insert_resource(crate::ui_macro::MacroBoundSpells::default())
                .insert_resource(Spells::empty_for_tests())
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_script::UiClock>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::PendingCast>()
                .init_resource::<crate::spell::QueuedMeleeSpell>()
                .init_resource::<crate::spell::ActiveChannel>()
                .init_resource::<crate::spell::SpellTargeting>()
                .init_resource::<crate::spell::HeldForPick>()
                .init_resource::<Selection>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::net::Reputations>()
                .insert_resource(items)
                .insert_resource(NetCommands(tx));
            crate::items::test_spawn_item(
                app.world_mut(),
                0xF0,
                ObjectFields::from_pairs(&[(3, ITEM), (14, 10)]),
                false,
            );
            let mut store = vec![(22, 100), (23, 500), (46, 1 << 3)];
            if in_pack {
                store.push((PACK_SLOT_1, 0xF0));
            }
            app.world_mut().spawn((
                SelfPlayer,
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&store)),
            ));
            let mut script = UiScript::new().unwrap();
            script.set_action(
                1,
                Some(benilla_ui::script::ActionSlot {
                    kind: ACTION_KIND_ITEM,
                    action: ITEM,
                    count: stored,
                    ..Default::default()
                }),
            );
            app.insert_non_send_resource(script);
            app.add_systems(Update, feed_action_state);
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>("return (IsUsableAction(1)) and true or false")
                .unwrap()
        };

        assert!(lit(3, false), "a stored count lights it, bags or none");
        assert!(
            !lit(0, true),
            "a stored zero greys it with a copy in the pack"
        );
    }

    /// `IsEquippedAction` asks `0x622270` at mask 1 (`0x4e5380`), the walker over the worn slots,
    /// which skips a broken item (`0x6224f4`-`0x62250e`): a worn weapon at durability 0 draws no
    /// border, a whole one does, and a copy in the pack is not worn. The slot's count comes from
    /// the recount (`0x4e6d20`, mask 0), and the usable gate reads that count (`0x4e50ab`): a worn
    /// whole copy lights the slot, a broken one counts 0 and greys it.
    #[test]
    fn a_broken_worn_weapon_draws_no_equipped_border() {
        use benilla_protocol::messages::ActionButton;
        use benilla_protocol::ObjectFields;

        const SWORD: u32 = 2025;
        let equipped = |seat_field: u16, durability: u32| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            let mut actions = PlayerActions::default();
            actions.buttons.insert(
                0,
                ActionButton {
                    slot: 0,
                    action: SWORD,
                    kind: ACTION_KIND_ITEM,
                },
            );
            let mut items = Items::default();
            items.insert_template(SWORD, Some(crate::items::test_template("Sword")));
            app.insert_resource(actions)
                .insert_resource(crate::ui_macro::MacroBoundSpells::default())
                .insert_resource(Spells::empty_for_tests())
                .add_message::<crate::item_watch::ItemNotice>()
                .add_message::<crate::net::WorldEnterCascadeMessage>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_script::UiClock>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<crate::spell::PendingCast>()
                .init_resource::<crate::spell::QueuedMeleeSpell>()
                .init_resource::<crate::spell::ActiveChannel>()
                .init_resource::<crate::spell::SpellTargeting>()
                .init_resource::<crate::spell::HeldForPick>()
                .init_resource::<Selection>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::net::Reputations>()
                .insert_resource(items)
                .insert_resource(NetCommands(tx));
            // The sword: flags 0, durability, maximum 40.
            crate::items::test_spawn_item(
                app.world_mut(),
                0xF0,
                ObjectFields::from_pairs(&[(3, SWORD), (14, 1), (46, durability), (47, 40)]),
                false,
            );
            app.world_mut().spawn((
                SelfPlayer,
                Transform::default(),
                ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),
                    (23, 500),
                    (46, 1 << 3),
                    (seat_field, 0xF0),
                ])),
            ));
            let mut script = UiScript::new().unwrap();
            script.set_action(
                1,
                Some(benilla_ui::script::ActionSlot {
                    kind: ACTION_KIND_ITEM,
                    action: SWORD,
                    count: 1,
                    ..Default::default()
                }),
            );
            app.insert_non_send_resource(script);
            app.add_systems(Update, feed_action_state);
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>("return (IsEquippedAction(1)) and true or false")
                .unwrap()
        };
        // Player slot 15 is the main hand, 23 the first backpack seat.
        assert!(equipped(486 + 2 * 15, 12), "a whole worn sword");
        assert!(!equipped(486 + 2 * 15, 0), "a broken one");
        assert!(
            !equipped(486 + 2 * 23, 12),
            "a copy in the pack is not worn"
        );
    }

    /// `SMSG_CLIENT_CONTROL_UPDATE` clears `[0xb4b3e4]` (`0x4958e0`), and `IsUsableAction` answers
    /// nil for every spell button and spell-less macro but the spell whose immunity covers the fear
    /// (`0x6e3dc3`-`0x6e3e4c`, `0x4e50f4`); control back, they all light.
    #[test]
    fn lost_control_greys_the_bar_but_the_immunity_spell() {
        use benilla_protocol::messages::{ActionButton, ACTION_KIND_MACRO};
        use benilla_protocol::ObjectFields;

        const FIREBALL: u32 = 133;
        const FEAR_AURA: u32 = 5782;
        const WILL: u32 = 7744;
        /// `UNIT_FIELD_FLAGS` bit 23, fleeing.
        const FLEEING: u32 = 0x0080_0000;

        let mut fear = SpellDisplay {
            mechanic: 5,
            ..Default::default()
        };
        fear.effects[0] = 6;
        fear.effect_apply_aura[0] = 7;
        let mut will = SpellDisplay {
            attributes_ex: 0x8000,
            ..Default::default()
        };
        will.effects[0] = 6;
        will.effect_apply_aura[0] = 77;
        will.effect_misc_value[0] = 5;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        for (slot, action, kind) in [
            (0u8, FIREBALL, ACTION_KIND_SPELL),
            (1, WILL, ACTION_KIND_SPELL),
            (2, 1, ACTION_KIND_MACRO),
        ] {
            actions
                .buttons
                .insert(slot, ActionButton { slot, action, kind });
        }
        let mut bound = crate::ui_macro::MacroBoundSpells::default();
        bound.0.insert(1, crate::ui_macro::BoundSpell::None);
        app.insert_resource(actions)
            .insert_resource(bound)
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [
                        (FIREBALL, SpellDisplay::default()),
                        (WILL, will),
                        (FEAR_AURA, fear),
                    ]
                    .into_iter()
                    .collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .init_resource::<crate::player::Player>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[
                (22, 100),
                (23, 500),
                (46, (1 << 3) | FLEEING),
                (47, FEAR_AURA),
            ])),
        ));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                USABLE_EVENTS = 0
                local f = CreateFrame("Frame")
                f:RegisterEvent("ACTIONBAR_UPDATE_USABLE")
                f:SetScript("OnEvent", function() USABLE_EVENTS = USABLE_EVENTS + 1 end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_action_state);
        let events = |app: &mut App| -> i64 {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            s.eval::<i64>("local n = USABLE_EVENTS; USABLE_EVENTS = 0; return n")
                .unwrap()
        };

        let lit = |app: &App| -> Vec<bool> {
            let script = app.world().non_send_resource::<UiScript>();
            (1..=3)
                .map(|n| {
                    script
                        .eval::<bool>(&format!("return (IsUsableAction({n})) and true or false"))
                        .unwrap()
                })
                .collect()
        };

        app.update();
        assert_eq!(
            lit(&app),
            [true, true, true],
            "in control, the whole bar is lit"
        );
        events(&mut app); // the first push's own signal

        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .own_control_lost = true;
        app.update();
        assert_eq!(
            lit(&app),
            [false, true, false],
            "only the fear immunity stays lit"
        );
        assert_eq!(events(&mut app), 1, "the cache change signals the bar once");

        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .own_control_lost = false;
        app.update();
        assert_eq!(lit(&app), [true, true, true], "control back, all lit again");
        assert_eq!(events(&mut app), 1);
    }

    /// A held action (Stealth) reaches the VM as the held marker, and `GetActionCooldown` answers
    /// the poll's own now (`0x6e1517`) with no feed change between polls.
    #[test]
    fn a_held_action_polls_answer_their_own_now() {
        use benilla_protocol::messages::ActionButton;
        use benilla_protocol::ObjectFields;

        const STEALTH: u32 = 1784;
        let stealth = || SpellDisplay {
            attributes: 0x0200_0000,
            recovery_ms: 10_000,
            ..Default::default()
        };
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: STEALTH,
                kind: ACTION_KIND_SPELL,
            },
        );
        app.insert_resource(actions)
            .insert_resource(crate::ui_macro::MacroBoundSpells::default())
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(STEALTH, stealth())].into_iter().collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_script::UiClock>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::ActiveChannel>()
            .init_resource::<crate::spell::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Selection>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(22, 100), (23, 500)])),
        ));
        app.world_mut().resource_mut::<Cooldowns>().start_spell(
            STEALTH,
            &stealth(),
            0,
            Instant::now(),
            None,
        );
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_action_state);

        let read = |app: &App| {
            let script = app.world().non_send_resource::<UiScript>();
            let (start, duration, enable) = script
                .eval::<(f64, f64, i64)>("return GetActionCooldown(1)")
                .unwrap();
            (start, duration, enable, script.now())
        };
        app.update();
        let first = read(&app);
        assert_eq!((first.1, first.2), (0.001, 0), "(now, 0.001, 0)");
        assert_eq!(first.0, first.3);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.02);
        let second = read(&app);
        assert_eq!(second.0, second.3, "the poll answers its own now");
        assert!((second.0 - first.0 - 0.02).abs() < 1e-9, "20 ms apart");
    }
}
