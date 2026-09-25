//! Each action slot's per-frame state behind `IsUsableAction`, `IsActionInRange`,
//! `IsCurrentAction` and `GetActionCooldown`, diff-pushed into the VM with the reference's events
//! on each change: the cooldown trio (`0x4b31b0`, `0x4f93d0`), the usable pair (`0x4b31c0`, on a
//! cache change only, `0x4e5c00`), the state pair (`0x4b3250`), combat enter and leave
//! (`0x6256ff`, `0x625778`), and autorepeat start and stop (`0x6e5952`, `0x6ea170`). Usability is
//! `IsSpellUsableNow 0x6e3d60` ([`crate::spell::usable`]), auto-repeat the `0xceac30` key, and
//! range the squared distance against `GetMinMaxRange 0x6e3480`.

use crate::ui_items::carried_counts;
use std::collections::HashMap;
use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::{ActionState, UiScript};

use crate::creature_anim::{Casting, Engaged};
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::spell::Cooldowns;
use crate::target::Selection;

use super::{PlayerActions, Spells};
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

/// Computes and diff-pushes every occupied slot's state, and fires the reference's events.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_action_state(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut cooldowns: ResMut<Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    auto_repeat: Res<AutoRepeatActive>,
    // One tuple for Bevy's 16-param ceiling: cast tracking, macro bindings and spell mods.
    cast_state: (
        Res<crate::spell::PendingCast>,
        Res<crate::spell::QueuedMeleeSpell>,
        Res<crate::spell::ActiveChannel>,
        Res<crate::spell::SpellTargeting>,
        Res<crate::ui_macro::MacroBoundSpells>,
        Res<crate::spell::SpellModifiers>,
    ),
    self_q: Query<(&ObjectStore, &Transform, Has<Engaged>, Option<&Casting>), With<SelfPlayer>>,
    selection: Res<Selection>,
    // One param for the guid index and the item store, for the same ceiling.
    objects: crate::net::Objects,
    units: Query<(&ObjectStore, &Transform), Without<SelfPlayer>>,
    factions: Option<Res<crate::target::Factions>>,
    reputations: Res<crate::net::Reputations>,
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

    let (pending, queued_melee, channel, targeting, bound, spell_mods) = &cast_state;
    let me = self_q.iter().next();
    // The bags, walked once per frame for every reagent, totem and item count below.
    let carried = me
        .map(|(s, _, _, _)| carried_counts(&s.0, &objects))
        .unwrap_or_default();
    let engaged = me.is_some_and(|(_, _, e, _)| e);
    let form_byte = me
        .map(|(s, _, _, _)| s.0.unit_shapeshift_form())
        .unwrap_or(0);
    let casting_spell = me.and_then(|(_, _, _, c)| c.map(|c| c.spell_id));
    let current_cast = pending.current(now).or(casting_spell);
    let self_reach = me.map_or(1.5, |(s, _, _, _)| s.0.unit_combat_reach());
    let self_pos = me.map(|(_, t, _, _)| t.translation);
    // The target's reach and squared distance (the reference tests dx²+dy²+dz², `0x6e47b0`).
    let target = selection
        .guid
        .and_then(|g| objects.entity(g))
        .and_then(|e| units.get(e).ok());
    let target_reach = target.map(|(s, _)| s.0.unit_combat_reach());
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
                // `0x4e5050`'s spell-less leg: usable. It also greys every spell-less macro and
                // item while the player-control flag `[0xb4b3e4]` is clear (taxi, fear, charm,
                // `0x4958e0`; set at boot, `0x48f626`); that gate is not built.
                st.usable = true;
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
                // `IsCurrentAction 0x4e53a0`: Attack while engaged; a spell while in flight
                // (`0xceca88`, which a queued on-swing strike holds), channelled (`0xceac58`), or
                // when its form is ours (`0x4e5556`). The form arm is not the icon's aura scan:
                // a form granted by another spell lights the check but not the icon.
                st.current = if st.is_attack {
                    engaged
                } else {
                    current_cast == Some(button.action)
                        || queued_melee.current() == Some(button.action)
                        || channel.current(now) == Some(button.action)
                        // Checked while its ground click is pending (`0x4e54d0`, `0x6e48e0`).
                        || targeting.spell() == Some(button.action)
                        || (form_byte != 0 && d.shapeshift_form == Some(u32::from(form_byte)))
                };
                st.auto_repeat = auto_repeat.0 == Some(button.action);
                // The usable walk (`0x6e3d60`); only its power gate sets `notEnoughMana`.
                if let (Some((store, _, _, _)), Some(sp)) = (me, spells.as_deref()) {
                    let ctx = usable::UsableCtx {
                        store,
                        target_store: target.map(|(s, _)| s),
                        factions: factions.as_deref(),
                        reputations: &reputations,
                        cooldowns: &cooldowns,
                        carried: &carried,
                        spell_mods,
                    };
                    let (u, oom) = usable::spell_usable(
                        button.action,
                        d,
                        sp,
                        &ctx,
                        &objects,
                        &items,
                        &commands,
                    );
                    st.usable = u;
                    st.not_enough_mana = oom;
                } else {
                    st.usable = true;
                }
                // The range verdict against the target (`0x4e56f0`); nil without one.
                let row = spells.as_ref().and_then(|s| s.ranges.get(d.range_index));
                let resolved = benilla_formats::min_max_range(d, row, self_reach, target_reach);
                st.has_range = resolved
                    .is_some_and(|(min, max)| min.abs() > f32::EPSILON || max.abs() > f32::EPSILON);
                st.in_range = match (resolved, dist_sq) {
                    (Some((min, max)), Some(d2)) if st.has_range => {
                        Some(d2 >= min * min && d2 <= max * max)
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
                let use_spell = items
                    .template(button.action, 0, &commands)
                    .and_then(|t| t.use_spell);
                // `IsConsumableAction` (`0x4e5250`) reads only the template, so the identity
                // feed pushes it with the count it gates, not this one.
                let count = carried.get(&button.action).copied().unwrap_or(0);
                // `IsEquippedAction`, the green border: worn in any equipment slot (0..18).
                st.equipped = me.is_some_and(|(s, _, _, _)| {
                    (0..19).any(|i| {
                        s.0.player_inv_slot(i)
                            .and_then(|g| objects.object(g))
                            .and_then(|o| o.object_entry())
                            == Some(button.action)
                    })
                });
                // `0x4e5050`'s item arm runs the count, `IsItemOnCooldown` and the on-use spell
                // through the same `0x6e3d60` walk. With no player the reference answers (0, 0)
                // (`0x4e5080`), the default state.
                if let Some((store, _, _, _)) = me {
                    let ctx = usable::UsableCtx {
                        store,
                        target_store: target.map(|(s, _)| s),
                        factions: factions.as_deref(),
                        reputations: &reputations,
                        cooldowns: &cooldowns,
                        carried: &carried,
                        spell_mods,
                    };
                    let (u, oom) = usable::item_usable(
                        button.action,
                        use_spell.as_ref(),
                        count > 0 || st.equipped,
                        &ctx,
                        spells.as_deref(),
                        &objects,
                        &items,
                        &commands,
                    );
                    st.usable = u;
                    st.not_enough_mana = oom;
                }
                if let Some(u) = use_spell {
                    let d = spells.as_ref().and_then(|s| s.catalog.get(u.spell_id));
                    let info = cooldowns.info(u.spell_id, button.action, d, now);
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
    let mut state_changed = false;
    let keys: Vec<u32> = fresh
        .keys()
        .chain(memory.pushed.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for action in keys {
        let (new, old) = (fresh.get(&action), memory.pushed.get(&action));
        if new == old {
            continue;
        }
        let d = ActionState::default();
        let (n, o) = (new.unwrap_or(&d), old.unwrap_or(&d));
        if (n.usable, n.not_enough_mana) != (o.usable, o.not_enough_mana) {
            usable_changed = true;
        }
        if (n.current, n.auto_repeat) != (o.current, o.auto_repeat) {
            state_changed = true;
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
    if state_changed {
        script.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
        script.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
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
            app.insert_non_send_resource(UiScript::new().unwrap());
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
}
