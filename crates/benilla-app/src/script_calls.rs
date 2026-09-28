//! The one point in the frame where the script calls that touch the selection, the player's cast
//! or the targeting cursor take effect, in the order the script made them. In the reference each
//! such call is done when it returns (the macro runner `0x4f14e0` fires every line in one pass;
//! `TargetByName` `0x489d60` and `TargetUnit` `0x4899d0` commit through `SetSelection 0x493540`;
//! `CastSpellByName` `0x4b4ab0` casts at the selection as it stands then), so `/target Bob` then
//! `/cast Flash Heal` heals Bob, and `/cast` then `/target` casts at the old target. A fixed order
//! between per-kind drains cannot give both, so the VM keeps one queue ([`ScriptCall`]) and this
//! system applies it front to back, each call through its owner's applier.

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::{ScriptCall, UiScript};

/// How deep calls made while applying a call may nest in one frame: a macro whose line presses a
/// macro, and so on. Past it (a macro that presses itself), what is left waits for the next frame,
/// so a frame always ends.
const MAX_NESTING: u8 = 8;

/// Apply this frame's script calls in call order, in the target chain after the world clicks.
#[allow(clippy::type_complexity)] // the appliers, one per owner of the state a call touches
pub(crate) fn apply_script_calls(
    script: Option<NonSendMut<UiScript>>,
    mut appliers: ParamSet<(
        crate::target::ScriptSelect,
        crate::spell::ScriptCursor,
        crate::spell::SelfCancel,
        crate::spell::ScriptCast,
        crate::ui_action::ActionPress,
        crate::ui_items::ScriptItemUse,
        crate::ui_pet::PetPress,
    )>,
) {
    let Some(mut script) = script else {
        return;
    };
    in_call_order(&mut script, |script, call| match call {
        ScriptCall::Select(request) => appliers.p0().select(request),
        ScriptCall::TargetByName { name, exact } => appliers.p0().target_by_name(&name, exact),
        ScriptCall::TargetNearest { mode, reverse } => {
            appliers.p0().target_nearest(mode, reverse);
        }
        ScriptCall::TargetLastTarget => appliers.p0().target_last_target(),
        ScriptCall::ClearTarget => appliers.p0().clear_target(),
        ScriptCall::AttackTarget => appliers.p4().attack_target(),
        ScriptCall::SpellTargetUnit(token) => appliers.p1().spell_target_unit(&token),
        ScriptCall::SpellStopTargeting => appliers.p1().stop_targeting(),
        ScriptCall::SpellStopCasting => appliers.p2().stop_casting(),
        ScriptCall::CastSpell(spell_id) => {
            crate::ui_spellbook::cast_spell(&mut appliers.p3(), spell_id);
        }
        ScriptCall::CastShapeshiftForm(spell_id) => {
            crate::ui_shapeshift::cast_form(&mut appliers.p3(), spell_id);
        }
        ScriptCall::UseAction(press) => {
            crate::ui_action::use_action(&mut appliers.p4(), script, press);
        }
        ScriptCall::UseContainerItem { bag, slot } => {
            appliers.p5().use_container_item(script, bag, slot);
        }
        ScriptCall::UseInventoryItem(id) => appliers.p5().use_inventory_item(script, id),
        ScriptCall::CastPetSpell(spell_id) => {
            crate::ui_pet_book::cast_pet_spell(&mut appliers.p6(), spell_id);
        }
        ScriptCall::PetAction(slot) => appliers.p6().press_slot(slot),
        ScriptCall::PetOrder(packed) => appliers.p6().order(packed),
    });
}

/// Apply every queued call front to back. Applying one may queue more: a macro's lines run
/// inside `UseAction` (`0x4e6098 call 0x4f1460`), so the calls they make go before the next one
/// queued.
pub(crate) fn in_call_order(
    script: &mut UiScript,
    mut apply: impl FnMut(&mut UiScript, ScriptCall),
) {
    let mut queue: VecDeque<(ScriptCall, u8)> = script
        .take_script_calls()
        .into_iter()
        .map(|call| (call, 0))
        .collect();
    let mut next_frame = Vec::new();
    while let Some((call, depth)) = queue.pop_front() {
        apply(script, call);
        let nested = script.take_script_calls();
        if depth >= MAX_NESTING {
            next_frame.extend(nested);
            continue;
        }
        for call in nested.into_iter().rev() {
            queue.push_front((call, depth + 1));
        }
    }
    if !next_frame.is_empty() {
        script.requeue_script_calls(next_frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{
        ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer,
    };
    use crate::target::Selection;
    use benilla_protocol::messages::{ActionButton, GroupMemberEntry, ACTION_KIND_MACRO};
    use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView};
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    const ME: u64 = 0x10;
    /// The unit selected before the script runs.
    const OLD: u64 = 0x21;
    /// `party1`, the unit the script selects.
    const NEW: u64 = 0x22;
    /// Not in the (absent) spell catalog, so the cast's wire target is the selection as it stands.
    const HEAL: u32 = 2050;

    struct Frame {
        app: App,
        rx: Receiver<ClientCommand>,
    }

    /// The whole client, headless, with us, `OLD` (selected when `selected`) and `NEW` as
    /// `party1`, and a VM whose book holds Heal.
    fn frame(selected: bool) -> Frame {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        // Fields 22 health and 28 max health: every unit is alive.
        let unit = |guid: u64, x: f32| {
            (
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (22, 100),
                    (28, 100),
                ])),
            )
        };
        let me = world.spawn((SelfPlayer, unit(ME, 0.0))).id();
        let old = world.spawn(unit(OLD, 5.0)).id();
        let new = world.spawn(unit(NEW, 6.0)).id();
        let index = &mut world.resource_mut::<GuidIndex>().0;
        index.insert(ME, me);
        index.insert(OLD, old);
        index.insert(NEW, new);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: "Bob".into(),
                guid: NEW,
                status: 1,
                flags: 0,
            });
        if selected {
            *world.resource_mut::<Selection>() = Selection {
                target: Some(old),
                guid: Some(OLD),
                ..Default::default()
            };
        }
        let mut script = UiScript::new().expect("a VM");
        script.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Holy".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: HEAL,
                name: "Heal".into(),
                ..Default::default()
            }],
        });
        if selected {
            // The frame's unit snapshot, as the feed pushed it before the input pass.
            script.set_unit(
                "target",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    ..Default::default()
                }),
            );
        }
        world.insert_non_send_resource(script);
        Frame { app, rx }
    }

    /// Run `lua` in the VM, then this frame's one application of what it queued.
    fn run(f: &mut Frame, lua: &str) {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(lua)
            .expect("the script runs");
        f.app
            .world_mut()
            .run_system_once(apply_script_calls)
            .expect("the calls apply as a one-shot system");
    }

    /// The `CMSG_CAST_SPELL`s sent, as `(spell, target)`.
    fn casts(f: &Frame) -> Vec<(u32, Option<u64>)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => Some((spell_id, target)),
                _ => None,
            })
            .collect()
    }

    fn selected(f: &Frame) -> Option<u64> {
        f.app.world().resource::<Selection>().guid
    }

    /// `/target` then `/cast`: `TargetUnit` commits through `SetSelection 0x493540` before it
    /// returns, and `CastSpellByName` (`0x4b4ab0`) casts at the selection as it stands then.
    #[test]
    fn a_cast_after_a_target_goes_to_the_new_target() {
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `/cast` then `/target`: the cast is done before the selection moves.
    #[test]
    fn a_cast_before_a_target_goes_to_the_old_target() {
        let mut f = frame(true);
        run(&mut f, r#"CastSpellByName("Heal") TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `ClearTarget` (`0x489ff0`) reads the selection at call time and deselects through
    /// `0x493540(0,0)`: after `TargetUnit("player")` there is one to clear, with or without a
    /// target before the script ran.
    #[test]
    fn a_clear_after_a_target_ends_with_nothing_selected() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            run(&mut f, r#"TargetUnit("player") ClearTarget()"#);
            assert_eq!(selected(&f), None, "selected before: {selected_before}");
            let world = f.app.world();
            assert_eq!(world.resource::<Selection>().target, None);
            let sends: Vec<u64> =
                f.rx.try_iter()
                    .filter_map(|c| match c {
                        ClientCommand::SetSelection { guid } => Some(guid),
                        _ => None,
                    })
                    .collect();
            assert_eq!(sends, vec![ME, 0], "selected before: {selected_before}");
        }
    }

    /// `TargetNearestEnemy` (`0x489a80` → `0x493f60(0, 1)`) commits through `SetSelection` before
    /// it returns, so the cast after it in the same script goes to the unit the TAB cycle picked.
    #[test]
    fn a_cast_after_target_nearest_enemy_goes_to_the_picked_unit() {
        const MOB: u64 = 0x31;
        let mut f = frame(true);
        let world = f.app.world_mut();
        // With no `FactionTemplate.dbc` a plain unit is attackable; OLD and NEW carry no
        // `NetEntity`, so the scan's one candidate is the mob.
        let mob = world
            .spawn((
                Guid(MOB),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(3.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(3.0, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (22, 100),
                    (28, 100),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(MOB, mob);
        // We are a player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PVP_ATTACKABLE`), which selects
        // `CanAttack`'s player arm.
        let me = world.resource::<GuidIndex>().0[&ME];
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
            ])));
        run(&mut f, r#"TargetNearestEnemy() CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(MOB))]);
        assert_eq!(selected(&f), Some(MOB));
    }

    /// `TargetLastTarget` (`0x489b00`) reads the pair `SetSelection` stamps with the outgoing
    /// selection (`0x49361d`): a held pair re-selects, which swaps the two; an empty one deselects
    /// through `0x493540(0,0)` (`0x489b2d`), which stamps what it drops.
    #[test]
    fn target_last_target_swaps_and_deselects_on_an_empty_pair() {
        let mut f = frame(false);
        // Selecting from nothing stamps nothing: the pair is empty, so the call deselects.
        run(&mut f, r#"TargetUnit("party1") TargetLastTarget()"#);
        assert_eq!(selected(&f), None);
        // The deselect stamped NEW, so the next call brings it back.
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(NEW));
        // A switch stamps the outgoing unit; the call swaps back, and the cast after it in the
        // same script goes to the unit it re-selected.
        run(
            &mut f,
            r#"TargetUnit("player") TargetLastTarget() CastSpellByName("Heal")"#,
        );
        assert_eq!(selected(&f), Some(NEW));
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(ME), "and back again");
    }

    /// `AttackTarget` (`0x489b50` → `0x6131a0`) swings at the selection as the calls before it
    /// left it.
    #[test]
    fn attack_target_swings_at_the_selection_in_call_order() {
        let swings = |f: &Frame| -> Vec<u64> {
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        };
        let mut f = frame(true);
        run(&mut f, r#"AttackTarget() TargetUnit("party1")"#);
        assert_eq!(swings(&f), vec![OLD]);
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") AttackTarget()"#);
        assert_eq!(swings(&f), vec![NEW]);
    }

    /// A macro's lines run inside `UseAction` (`0x4e6098 call 0x4f1460`), so what they call lands
    /// before the calls the pressing script makes after it: the macro's cast goes to the old
    /// target, then the script's `TargetUnit` moves the selection.
    #[test]
    fn a_macros_calls_land_inside_the_use_action_that_ran_it() {
        let mut f = frame(true);
        {
            let world = f.app.world_mut();
            let script = world.non_send_resource::<UiScript>();
            // `ChatFrame1`'s `EXECUTE_CHAT_LINE` handler (`ChatFrame.lua:1343`) sends the line
            // through the edit box; this one runs it as Lua, which is all the order needs.
            script
                .run(
                    r#"local f = CreateFrame("Frame")
                    f:RegisterEvent("EXECUTE_CHAT_LINE")
                    f:SetScript("OnEvent", function() RunScript(arg1) end)
                    MACRO_INDEX = CreateMacro("heal", 1, 'CastSpellByName("Heal")', 1, nil)"#,
                )
                .expect("the macro is made");
            let index: u32 = script.eval("return MACRO_INDEX").expect("an index");
            world
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: index,
                        kind: ACTION_KIND_MACRO,
                    },
                );
        }
        run(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }
}
