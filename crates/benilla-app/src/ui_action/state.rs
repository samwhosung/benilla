//! The per-action **dynamic-state feed** (decision 0137 phase 4) — the app-side computation
//! behind the engine's `IsUsableAction`/`IsActionInRange`/`IsCurrentAction`/`GetActionCooldown`
//! family: each occupied action slot's [`ActionState`], recomputed per frame, diff-pushed into
//! the VM, with the reference client's own event edges fired on these transitions:
//!
//! - a cooldown-store change → `ACTIONBAR_UPDATE_COOLDOWN` + `SPELL_UPDATE_COOLDOWN` +
//!   `BAG_UPDATE_COOLDOWN` (the `0x4b31b0`/`0x4f93d0` flush pair the SMSG handlers call);
//! - a usable/oom change on any slot → `ACTIONBAR_UPDATE_USABLE` + `SPELL_UPDATE_USABLE`
//!   (`0x4b31c0`; the client fires only on a cache CHANGE — `0x4e5c00` — hence the diff edge);
//! - a current/auto-repeat change → `ACTIONBAR_UPDATE_STATE` + `CURRENT_SPELL_CAST_CHANGED`
//!   (`0x4b3250`);
//! - our own melee engage/disengage → `PLAYER_ENTER_COMBAT`/`PLAYER_LEAVE_COMBAT`
//!   (`0x6256ff`/`0x625778` — the attack-start/stop handlers);
//! - the live autorepeat key's edges → `START_AUTOREPEAT_SPELL` (`0x6e5952`, at cast-send) /
//!   `STOP_AUTOREPEAT_SPELL` (`0x6ea170`).
//!
//! The per-flag semantics are the reference's: `notEnoughMana` is strictly the power-cost verdict,
//! `IsCurrentAction` (`0x4e53a0`) keys on the engaged attack GUID / the in-flight cast id,
//! `IsAutoRepeatAction` on the `0xceac30` key, and the range test is squared distance against the
//! `GetMinMaxRange 0x6e3480` (its constants transcribed below). The usable pair itself is the full
//! `IsSpellUsableNow 0x6e3d60` gate walk — [`crate::spell::usable`]: reagents, forms, stealth,
//! aura states (the Execute-family target dependence), the works.

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
    /// Last `benilla_assets::trace` "cd tick" stamp — the once-per-second gate (trace runs only).
    last_cd_trace: Option<Instant>,
}

/// What a slot *is* once the MACRO indirection is applied — the reference's slot→spell resolver
/// `0x4e5a50` plus the leg of the usable compute `0x4e5050` that reads its zero (decision 1636).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotResolve {
    /// The slot IS this `(kind, id)` from here down: a SPELL or ITEM slot, or a macro whose
    /// bound spell is live (`[rec+0x564] > 0`).
    Action(u8, u32),
    /// A macro that exists but casts nothing (`[rec+0x564] == 0`): `0x4e5050`'s spell-less leg
    /// (`0x4e50f4`–`0x4e516f`) answers **usable=1** off `0x4e5030` — "the slot's macro id is in
    /// the macro table" — and computes nothing else: no cooldown, no range, no checked ring.
    BareMacro,
    /// Not usable and nothing to report: a `/cast` whose name did not resolve (`-1`, which the
    /// spell path refuses at `0x4e518b: jl`), or a slot whose macro no longer exists (`0x4e5030`
    /// is 0 and `IsActionActive 0x4e55f0` has no spell to find).
    Dead,
}

/// Resolve a slot **through** a macro before any state is computed — the reference's own shape,
/// and the reason a macro button on the bar wears its spell's cooldown swirl, usability tint,
/// range colour and checked ring while showing its own icon.
///
/// Every `Is*Action`/`GetActionCooldown` binding routes through the one slot→spell resolver
/// `0x4e5a50`, whose MACRO arm resolves the macro record and returns `[rec+0x564]` as the slot's
/// spell id. So from here down, a macro that casts Fireball simply *is* the Fireball slot.
/// `GetActionTexture` is the deliberate exception — its macro arm keeps the macro's own icon
/// (`super::feed`).
///
/// The zero is NOT "nothing to report" (0983's reading — B340's grey `.spawn` macro): the field
/// is three-valued and the usable compute reads each value differently, which [`SlotResolve`]
/// carries. Only the SPELL indirection is modelled: 1.12 has no `/use <item>` slash command, so
/// no 1.12 macro body can name an item and the resolver's item leg is unreachable from one.
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

/// Compute + diff-push every occupied slot's dynamic state, and fire the reference event edges.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_action_state(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut cooldowns: ResMut<Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    auto_repeat: Res<AutoRepeatActive>,
    // One tuple param (Bevy's 16-SystemParam ceiling): our own cast tracking — the in-flight
    // guard, the queued on-next-swing strike, the running channel, and the awaiting-click
    // ground targeting — plus the macro→spell binding the MACRO arm resolves through
    // (decision 0983) and the talent spell-modifier tables that leg 12's cost reads through,
    // both of which ride here for the same ceiling reason.
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
    // The object lookup (2334) — the guid index this system already resolved its selection
    // through, plus the item-store read the ITEM arms need. One param, not two: this signature
    // sits at Bevy's 16-SystemParam ceiling.
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
    // The frame's atomic clock pair — `ui_triple`'s conversion base: every cooldown is pushed as
    // its absolute start on the GetTime clock, derived through the ONE lawful pair
    // ([`crate::ui_script::UiClock`]) so a running cooldown re-derives the same start every frame.
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
    // The bags, walked ONCE for the frame: every reagent, totem and item-count question below
    // reads this table. It used to be one whole walk per question — per reagent per spell slot,
    // per item slot — for the same bags each time (1697 item 13).
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
    // The current target's reach + squared distance (the client tests dx²+dy²+dz² — 0x6e47b0).
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
                // The spell-less leg of `0x4e5050`: the macro exists, so the slot is usable —
                // full colour on the bar — and there is no other state to compute (1636). The
                // leg's one gate benilla does not model is `[0xb4b3e4]`, the player-control
                // flag (1 from boot, `0x48f626`; 0 only across a control loss — taxi/fear/charm —
                // through `0x4958e0`); for that span the reference greys every spell-less
                // macro and item.
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
                // The Attack action is "current" while auto-attack is engaged; a castable
                // spell while it is our in-flight cast OR our queued on-next-swing strike OR our
                // running channel (the ref reads one inflight id `0xceca88` — which a queued
                // Heroic Strike *occupies* until the swing fires it — plus the channel id
                // `0xceac58`; our model splits the queue into its own slot, same observable) —
                // OR the shapeshift arm (`IsCurrentAction`'s predicate `0x4e53a0` @ `0x4e5556`):
                // a MOD_SHAPESHIFT spell whose form == the player's form byte reads checked.
                // Deliberately NOT the icon's aura-scan predicate — the two are different
                // functions in the binary and the asymmetry is load-bearing (a form granted by a
                // different spell lights the check without swapping the icon).
                st.current = if st.is_attack {
                    engaged
                } else {
                    current_cast == Some(button.action)
                        || queued_melee.current() == Some(button.action)
                        || channel.current(now) == Some(button.action)
                        // The awaiting-target arm (`0x4e53a0` @ `0x4e54d0`: the `0x6e48e0`
                        // targeting-spell read) — checked while the ground click is pending.
                        || targeting.spell() == Some(button.action)
                        || (form_byte != 0 && d.shapeshift_form == Some(u32::from(form_byte)))
                };
                st.auto_repeat = auto_repeat.0 == Some(button.action);
                // The full usable walk (`0x6e3d60` — [`super::usable`]): reagents, combo
                // points, forms, stealth, aura states, the bit-25 cooldown fold, and the power
                // gate (the sole notEnoughMana writer). Target-dependent for the Execute family
                // only. `spells` is necessarily Some here — `d` came out of it.
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
                // The range verdict vs the current target (`0x4e56f0`); nil without one.
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
                    // The store (Instant clock) vs the widget (GetTime clock) — the sink
                    // stamps the wall time, so drift between the two clocks reads directly.
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
                // Only the on-use spell leaves the template (`ItemUseSpell` is `Copy`) — not a
                // clone of the whole `ItemInfo` (its Strings and Vecs) per item slot per frame.
                let use_spell = items
                    .template(button.action, 0, &commands)
                    .and_then(|t| t.use_spell);
                // `IsConsumableAction` is NOT fed from here. It reads nothing but this template
                // (`0x4e5250`), so it is slot IDENTITY, and it rides the identity feed's push
                // beside the count it gates — `super::feed`'s ITEM arm, decision 1301.
                let count = carried.get(&button.action).copied().unwrap_or(0);
                // Worn on any equipment slot (0..18) — the green border's IsEquippedAction.
                st.equipped = me.is_some_and(|(s, _, _, _)| {
                    (0..19).any(|i| {
                        s.0.player_inv_slot(i)
                            .and_then(|g| objects.object(g))
                            .and_then(|o| o.object_entry())
                            == Some(button.action)
                    })
                });
                // The rest of `0x4e5050`'s ITEM arm — the count gate, `IsItemOnCooldown`, and
                // the item's on-use spell run through the SAME `0x6e3d60` walk a spell slot
                // takes ([`super::usable::item_usable`]). Food greys in combat from leg 8 there.
                // No active player and the reference answers (0,0) before resolving anything
                // (`0x4e5080`) — which is `ActionState::default()`'s `usable`.
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
        // No between-generation carry: the triple holds the ABSOLUTE start, so one running
        // cooldown re-derives the same value every frame (no diff churn) and a re-arm derives a
        // new one (the sweep restarts). The old `(remaining, duration)` carry-the-stale-triple
        // scheme aliased a fail-clear+re-arm inside one inter-feed gap into "unchanged" — the
        // vanished-GCD-pie-on-spam bug.
        fresh.insert(action, st);
    }

    // Diff-push + collect which event families changed.
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

    // The event edges, in the client's own flush order (the ACTIONBAR_* sibling first).
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

    /// A MACRO slot resolves through its bound spell for EVERY dynamic read (decision 0983) —
    /// the `0x4e5a50` law — and the three values of `[rec+0x564]` split three ways at the usable
    /// compute (decision 1636): a live spell IS that spell; a macro that casts nothing is a bare,
    /// usable button (B340's `.spawn` macro); an unresolved `/cast` — or a slot whose macro is
    /// gone — is grey.
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
        // Spell and item slots pass through untouched.
        assert_eq!(
            resolve_through_macro(ACTION_KIND_SPELL, 133, &bound),
            SlotResolve::Action(ACTION_KIND_SPELL, 133)
        );
        assert_eq!(
            resolve_through_macro(ACTION_KIND_ITEM, 117, &bound),
            SlotResolve::Action(ACTION_KIND_ITEM, 117)
        );
    }

    /// The feed end to end, at the symptom (B340): a MACRO slot whose macro casts nothing is
    /// pushed **usable** — `IsUsableAction` answers true in the VM, the full-colour icon — while
    /// a `/cast` of an unknown spell, and a slot whose macro is gone, are pushed grey. The
    /// pre-1636 feed pushed `ActionState::default()` for all three, whose `usable` is false: every
    /// GM `.spawn` macro on the bar was grey.
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
        // Macro 1 is `.spawn 16032`, macro 2 is `/cast Pyroblast` with Pyroblast unknown, and
        // macro 3 does not exist.
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

    /// **Food on the bar greys in combat** — the feed end to end, at the symptom. An ITEM slot's
    /// usable verdict is the reference's `0x4e5050` ITEM arm, which resolves the item's on-use
    /// spell (`0x4e5a50`) and walks it through `Spell_C::IsSpellUsableNow 0x6e3d60`; every
    /// Food/Drink spell in the shipped `Spell.dbc` carries `Attributes` bit 28
    /// (`ATTR_NOT_IN_COMBAT`, the walk's leg 8), so a stack of food is grey while
    /// `UNIT_FLAG_IN_COMBAT` is up and full-colour the moment it drops. Before this test the
    /// ITEM arm answered `count > 0 || equipped` and nothing else, and food stayed lit.
    #[test]
    fn food_on_the_bar_greys_while_the_player_is_in_combat() {
        use benilla_protocol::messages::{ActionButton, ItemUseSpell};
        use benilla_protocol::ObjectFields;

        // `Conjured Muffin`-shaped: one ON_USE block casting spell 433 "Food", which the shipped
        // DBC gives `Attributes = 0x18000100` — bit 28 among them.
        const FOOD_ITEM: u32 = 1487;
        const FOOD_SPELL: u32 = 433;
        // Descriptor indices, raw (the codebase's test idiom): `ITEM_FIELD_STACK_COUNT` and
        // `PLAYER_FIELD_PACK_SLOT_1` — the backpack's first slot, the walker's CARRIED section.
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
            // The ten muffins, as the one index's own item entity (2334).
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
