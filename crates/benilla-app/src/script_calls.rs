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
        crate::ui_action::AttackPress,
    )>,
) {
    let Some(mut script) = script else {
        return;
    };
    in_call_order(&mut script, |script, call| {
        match call {
            ScriptCall::Select(request) => appliers.p0().select(request),
            ScriptCall::TargetByName { name, exact } => appliers.p0().target_by_name(&name, exact),
            ScriptCall::TargetNearest { mode, reverse } => {
                appliers.p0().target_nearest(mode, reverse);
            }
            ScriptCall::TargetLastTarget => appliers.p0().target_last_target(),
            ScriptCall::ClearTarget => appliers.p0().clear_target(),
            ScriptCall::AttackTarget => appliers.p7().attack_target(),
            ScriptCall::SpellTargetUnit(token) => appliers.p1().spell_target_unit(&token),
            ScriptCall::SpellStopTargeting => appliers.p1().stop_targeting(),
            ScriptCall::SpellStopCasting => appliers.p2().stop_casting(),
            ScriptCall::CastSpell { spell_id, on_self } => {
                crate::ui_spellbook::cast_spell(&mut appliers.p3(), spell_id, on_self);
            }
            ScriptCall::CastShapeshiftForm(spell_id) => {
                crate::ui_shapeshift::cast_form(&mut appliers.p3(), spell_id);
            }
            ScriptCall::UseAction(press) => {
                let used = crate::ui_action::use_action(&mut appliers.p4(), script, press);
                if used == crate::ui_action::UseOutcome::Attack {
                    appliers.p7().attack_target();
                }
            }
            ScriptCall::UseContainerItem(call) => {
                appliers.p5().use_container_item(script, call);
            }
            ScriptCall::UseInventoryItem { slot, on_self } => {
                appliers.p5().use_inventory_item(script, slot, on_self)
            }
            ScriptCall::CastPetSpell { spell_id, on_self } => {
                crate::ui_pet_book::cast_pet_spell(&mut appliers.p6(), spell_id, on_self);
            }
            ScriptCall::PetAction(slot) => appliers.p6().press_slot(slot),
            ScriptCall::PetOrder(packed) => appliers.p6().order(packed),
        }
        // TryCast's attack pick can move the selection mid-cast (`0x6e4efb`), so a press held
        // there is picked and resumed before the next call reads the selection.
        let Some(held) = appliers.p3().ladder.take_held() else {
            return;
        };
        if appliers.p7().pick_for_cast(held) {
            let mut cast = appliers.p3();
            let ctx = cast.targeting.context();
            cast.ladder.resume_after_pick(held, &ctx);
        }
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
    use benilla_protocol::messages::{
        ActionButton, GroupMemberEntry, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
    };
    use benilla_ui::script::{PetBookState, SpellBookState, SpellSlotView, SpellTabView};
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    const ME: u64 = 0x10;
    /// The unit selected before the script runs.
    const OLD: u64 = 0x21;
    /// `party1`, the unit the script selects.
    const NEW: u64 = 0x22;
    /// Not in the (absent) spell catalog, so the cast's wire target is the selection as it stands.
    const HEAL: u32 = 2050;
    /// A mob the TAB scan can pick: it alone carries a `NetEntity`.
    const MOB: u64 = 0x31;
    /// `UNIT_FIELD_FACTIONTEMPLATE`, `UNIT_FIELD_HEALTH`, `UNIT_FIELD_FLAGS`.
    const TEMPLATE: u16 = 35;
    const HEALTH: u16 = 22;
    const FLAGS: u16 = 46;
    /// The fixture's templates: ours (group 1, friendly toward group 2, enemy of group 4), a
    /// friendly NPC's (group 2) and a hostile mob's (group 4).
    const OURS: u32 = 1;
    const FRIEND: u32 = 2;
    const FOE: u32 = 3;

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
        // We are a player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PVP_ATTACKABLE`), which selects
        // `CanAttack`'s player arm: with no `FactionTemplate.dbc` a plain unit is attackable.
        let me = world.spawn((SelfPlayer, unit(ME, 0.0))).id();
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
            ])));
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

    /// `MOB` at 3 yd, alive, with `fields` over its health.
    fn spawn_mob(f: &mut Frame, fields: &[(u16, u32)]) {
        let world = f.app.world_mut();
        let mut pairs = vec![(HEALTH, 100), (28, 100)];
        pairs.extend_from_slice(fields);
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
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(MOB, mob);
    }

    /// Give the world a faction table, us [`OURS`] and `OLD` the `old` template with `fields`
    /// over it.
    fn with_factions(f: &mut Frame, old: u32, fields: &[(u16, u32)]) {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;
        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        let world = f.app.world_mut();
        world.insert_resource(crate::target::Factions::from_catalog(
            FactionCatalog::from_rows(
                HashMap::from([
                    (OURS, tpl(OURS, 1, 2, 4)),
                    (FRIEND, tpl(FRIEND, 2, 0, 0)),
                    (FOE, tpl(FOE, 4, 0, 0)),
                ]),
                HashMap::new(),
            ),
        ));
        let index = &world.resource::<GuidIndex>().0;
        let (me, old_unit) = (index[&ME], index[&OLD]);
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, 100),
                (28, 100),
                (FLAGS, 0x8),
                (TEMPLATE, OURS),
            ])));
        let mut pairs = vec![(HEALTH, 100), (28, 100), (TEMPLATE, old)];
        pairs.extend_from_slice(fields);
        world
            .entity_mut(old_unit)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
                &pairs,
            )));
    }

    /// The `CMSG_ATTACKSWING`s sent, and whether a `CMSG_ATTACKSTOP` went out.
    fn swings(f: &Frame) -> (Vec<u64>, bool) {
        let mut stopped = false;
        let swings =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(guid),
                    ClientCommand::AttackStop => {
                        stopped = true;
                        None
                    }
                    _ => None,
                })
                .collect();
        (swings, stopped)
    }

    /// The keys of the error lines raised.
    fn errors(f: &Frame) -> Vec<&'static str> {
        f.app
            .world()
            .resource::<crate::ui_action::UiErrorKeys>()
            .0
            .iter()
            .map(|e| e.key)
            .collect()
    }

    /// `CastSpellByName(name, onSelf)` (`0x4b4ab0`) and `CastSpell(slot, book, onSelf)` (`0x4b42f0`)
    /// swap the target guid for the active player's (`0x4b4afa`, `0x4b4345`) when
    /// `GetBoolOrDefault` reads the flag true, so the cast goes to us whoever is selected, and the
    /// selection stays where it was. Without the flag it goes to the selection.
    #[test]
    fn on_self_casts_at_the_player_and_leaves_the_selection() {
        for (lua, target, selection) in [
            (r#"CastSpellByName("Heal", 1)"#, ME, OLD),
            (r#"CastSpellByName("Heal", "yes")"#, ME, OLD),
            (r#"CastSpell(1, "spell", 1)"#, ME, OLD),
            (
                r#"TargetUnit("party1") CastSpellByName("Heal", 1)"#,
                ME,
                NEW,
            ),
            (r#"CastSpellByName("Heal")"#, OLD, OLD),
            (r#"CastSpellByName("Heal", 0.5)"#, OLD, OLD),
            (r#"CastSpellByName("Heal", "false")"#, OLD, OLD),
            (r#"CastSpell(1, "spell")"#, OLD, OLD),
            (r#"CastSpell(1, "spell", "0")"#, OLD, OLD),
        ] {
            let mut f = frame(true);
            run(&mut f, lua);
            assert_eq!(casts(&f), vec![(HEAL, Some(target))], "{lua}");
            assert_eq!(selected(&f), Some(selection), "{lua}");
        }
    }

    /// Power Word: Fortitude's word, an assist unit (implicit target 21), in the catalog and
    /// the book.
    const FORTITUDE: u32 = 1243;

    fn with_fortitude(f: &mut Frame) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(FORTITUDE, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world
            .non_send_resource_mut::<UiScript>()
            .set_spellbook(SpellBookState {
                tabs: vec![SpellTabView {
                    name: "Priest".into(),
                    texture: None,
                    offset: 0,
                    num_spells: 1,
                }],
                slots: vec![SpellSlotView {
                    spell_id: FORTITUDE,
                    name: "Fortitude".into(),
                    ..Default::default()
                }],
            });
    }

    /// The bind runs the ordinary relation chain over the player: with a unit we cannot assist
    /// selected and `autoSelfCast` off, the plain call binds nothing (the cursor comes up), while
    /// `onSelf` binds us; with a friendly player selected the plain call binds that player.
    #[test]
    fn on_self_binds_the_player_where_the_selection_is_not_assistable() {
        for (selected_template, lua, target) in [
            (FOE, r#"CastSpellByName("Fortitude")"#, None),
            (FOE, r#"CastSpellByName("Fortitude", 1)"#, Some(ME)),
            (FOE, r#"CastSpell(1, "spell", 1)"#, Some(ME)),
            (FRIEND, r#"CastSpellByName("Fortitude")"#, Some(OLD)),
            (FRIEND, r#"CastSpellByName("Fortitude", 1)"#, Some(ME)),
        ] {
            let mut f = frame(true);
            // `CanAssist` passes a friendly player-controlled unit (`UNIT_FLAG_PLAYER_CONTROLLED`).
            with_factions(&mut f, selected_template, &[(FLAGS, 0x8)]);
            with_fortitude(&mut f);
            run(&mut f, lua);
            let sent: Vec<_> = target.into_iter().map(|t| (FORTITUDE, Some(t))).collect();
            assert_eq!(casts(&f), sent, "{lua} at template {selected_template}");
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
    }

    /// The pet's guid, and the pet book's Growl (`CMSG_PET_ACTION`'s type-1 word, `0x4b34ce`).
    const PET: u64 = 0x50;
    const GROWL: u32 = 2649;

    /// The `CMSG_PET_ACTION`s sent, as `(pet, word, target)`.
    fn pet_actions(f: &Frame) -> Vec<(u64, u32, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::PetAction {
                    pet_guid,
                    packed,
                    target_guid,
                } => Some((pet_guid, packed, target_guid)),
                _ => None,
            })
            .collect()
    }

    /// `0x4b3300`'s pet fork sends the guid it was handed as the order's target (`0x4b34ce`), so
    /// `CastSpell(slot, "pet", 1)` aims the pet's spell at the player, not the selection.
    #[test]
    fn a_pet_book_cast_on_self_aims_at_the_player() {
        for (lua, target) in [
            (r#"CastSpell(1, "pet", 1)"#, ME),
            (r#"CastSpell(1, "pet", "on")"#, ME),
            (r#"CastSpell(1, "pet")"#, OLD),
            (r#"CastSpell(1, "pet", "0")"#, OLD),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_pet::PetBar>()
                .spells
                .pet_guid = PET;
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_pet_book(PetBookState {
                    token: Some("PET".into()),
                    slots: vec![SpellSlotView {
                        spell_id: GROWL,
                        name: "Growl".into(),
                        ..Default::default()
                    }],
                });
            run(&mut f, lua);
            assert_eq!(
                pet_actions(&f),
                vec![(PET, 0x0100_0000 | GROWL, target)],
                "{lua}"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
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
        let mut f = frame(true);
        // OLD and NEW carry no `NetEntity`, so the scan's one candidate is the mob.
        spawn_mob(&mut f, &[]);
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
        let mut f = frame(true);
        run(&mut f, r#"AttackTarget() TargetUnit("party1")"#);
        assert_eq!(swings(&f), (vec![OLD], false));
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") AttackTarget()"#);
        assert_eq!(swings(&f), (vec![NEW], false));
    }

    /// `TargetByName` (`0x489d60`) commits through `SetSelection 0x493540` before it returns, so
    /// `UnitName("target")` on the next line of the same chunk names the new unit, and the frame's
    /// one application commits the same pick.
    #[test]
    fn target_by_name_names_the_new_target_in_the_same_chunk() {
        let mut f = frame(true);
        spawn_mob(&mut f, &[]);
        let world = f.app.world_mut();
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(OLD, "Old Mob".into(), None);
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(NEW, "Red Whelp".into(), None);
        world
            .run_system_once(crate::ui_unit::feed_name_candidates)
            .expect("the candidate feed runs as a one-shot system");
        let named = f
            .app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<String>("TargetByName('Red Affinity', true) return UnitName('target')")
            .expect("the chunk runs");
        assert_eq!(named, "Red Affinity");
        f.app
            .world_mut()
            .run_system_once(apply_script_calls)
            .expect("the calls apply as a one-shot system");
        assert_eq!(
            selected(&f),
            Some(MOB),
            "the commit picks what the chunk read"
        );
    }

    /// Two namesakes: the ranking decides, and the chunk shows the one the commit then selects --
    /// the nearer, and of two at one distance the lower guid, whatever order either side walks.
    #[test]
    fn target_by_name_shows_the_namesake_the_commit_picks() {
        /// A namesake of `MOB` on our other side, told apart by its health.
        const TWIN: u64 = 0x30;
        for (twin_x, picked, health) in [(-4.0, MOB, 100), (-3.0, TWIN, 40)] {
            let mut f = frame(true);
            spawn_mob(&mut f, &[]);
            let world = f.app.world_mut();
            let twin = world
                .spawn((
                    Guid(TWIN),
                    Transform::from_xyz(twin_x, 0.0, 0.0),
                    GlobalTransform::from_translation(Vec3::new(twin_x, 0.0, 0.0)),
                    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                        (HEALTH, 40),
                        (28, 100),
                    ])),
                ))
                .id();
            world.resource_mut::<GuidIndex>().0.insert(TWIN, twin);
            let mut names = world.resource_mut::<crate::names::NameCache>();
            names.insert_player(MOB, "Red Affinity".into(), None);
            names.insert_player(TWIN, "Red Affinity".into(), None);
            world
                .run_system_once(crate::ui_unit::feed_name_candidates)
                .expect("the candidate feed runs as a one-shot system");
            let shown = f
                .app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>("TargetByName('Red Affinity') return UnitHealth('target')")
                .expect("the chunk runs");
            assert_eq!(shown, health, "the twin {twin_x} yd off");
            f.app
                .world_mut()
                .run_system_once(apply_script_calls)
                .expect("the calls apply as a one-shot system");
            assert_eq!(selected(&f), Some(picked), "the twin {twin_x} yd off");
        }
    }

    /// The candidate feed marks a `UNIT_FLAG_NOT_SELECTABLE` unit as the commit's `IsSelectable`
    /// does (`0x4935ee`): the nearer namesake is the ranked pick and it is refused, so the chunk
    /// keeps the old target's view and the commit keeps its selection, not the farther namesake.
    #[test]
    fn target_by_name_shows_nothing_when_the_commit_refuses_its_pick() {
        /// `UNIT_FLAG_NOT_SELECTABLE`, `UNIT_FIELD_FLAGS` bit 25.
        const NOT_SELECTABLE: u32 = 1 << 25;
        /// A selectable namesake of `MOB` farther off, told apart by its health.
        const TWIN: u64 = 0x30;
        let mut f = frame(true);
        spawn_mob(&mut f, &[(FLAGS, NOT_SELECTABLE)]);
        let world = f.app.world_mut();
        let twin = world
            .spawn((
                Guid(TWIN),
                Transform::from_xyz(-4.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(-4.0, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (HEALTH, 40),
                    (28, 100),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(TWIN, twin);
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(TWIN, "Red Affinity".into(), None);
        world
            .run_system_once(crate::ui_unit::feed_name_candidates)
            .expect("the candidate feed runs as a one-shot system");
        let shown = f
            .app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<i64>("TargetByName('Red Affinity') return UnitHealth('target')")
            .expect("the chunk runs");
        assert_eq!(
            shown, 0,
            "the old target's view, not the twin's 40 or the mob's 100"
        );
        f.app
            .world_mut()
            .run_system_once(apply_script_calls)
            .expect("the calls apply as a one-shot system");
        assert_eq!(selected(&f), Some(OLD), "the commit refuses the same pick");
    }

    /// `0x6130a3`: a unit we are friendly toward is dropped (`0x6130a8`) and `TargetNearestEnemy`
    /// runs (`0x6130b5`), moving the selection; the swing goes at what it picked (`0x6130c1`).
    /// The Attack button reaches the same `0x6131a0` through `TryCast` (`0x6e4c90`).
    #[test]
    fn attack_with_a_friend_selected_swings_at_the_nearest_enemy_instead() {
        for lua in ["AttackTarget()", "UseAction(1)"] {
            let mut f = frame(true);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: crate::ui_action::SPELL_ATTACK,
                        kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(selected(&f), Some(MOB), "{lua}");
            assert_eq!(swings(&f), (vec![MOB], false), "{lua}");
            assert_eq!(errors(&f), Vec::<&str>::new(), "{lua}");
        }
    }

    /// The retarget commits before `AttackTarget` returns, so a cast after it in the same script
    /// goes to the enemy it picked, from a friend or from nothing selected.
    #[test]
    fn a_cast_after_attack_target_goes_to_the_enemy_it_picked() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            run(&mut f, r#"AttackTarget() CastSpellByName("Heal")"#);
            assert_eq!(
                casts(&f),
                vec![(HEAL, Some(MOB))],
                "selected before: {selected_before}"
            );
        }
    }

    /// With no enemy in reach the scan moves nothing (`0x493f60`), so the selection re-read at
    /// `0x6130c1` is the friend, which the final gate's `CanAttack` refuses (`0x613171`); only an
    /// empty selection is "There is nothing to attack." (`0x6130d9`). Neither swings.
    #[test]
    fn attack_with_no_enemy_near_refuses_by_what_is_selected() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));

        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), None);
        assert_eq!(errors(&f), vec!["ERR_NO_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// A hostile corpse is kept (`0x6130a3` reads the reaction alone), so no scan runs past it to
    /// the live mob, and the final gate refuses a unit neither alive nor feigning (`0x613152`–
    /// `0x613165`): "You cannot attack that target.", and no swing, which vmangos would stop
    /// (`CombatHandler.cpp:53-58`).
    #[test]
    fn attack_at_a_corpse_cannot_attack_it() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(HEALTH, 0)]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// `0x6131a0` toggles only past the validator: attacking a live foe stops, and a press the
    /// validator refuses neither stops nor swings.
    #[test]
    fn attack_toggles_off_only_past_the_validator() {
        for (health, stopped, refused) in [
            (100, true, None),
            (0, false, Some("ERR_INVALID_ATTACK_TARGET")),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, health)]);
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(crate::creature_anim::Engaged(OLD));
            run(&mut f, "AttackTarget()");
            assert_eq!(swings(&f), (vec![], stopped), "health {health}");
            assert_eq!(
                errors(&f),
                refused.into_iter().collect::<Vec<_>>(),
                "health {health}"
            );
        }
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

    /// Sinister Strike: `AttributesEx & 0x200`, one of predicate `0x6e5200`'s bits, and the enemy
    /// word (implicit target 6).
    const SINISTER_STRIKE: u32 = 1752;

    /// Sinister Strike in the catalog and on action slot 1.
    fn with_strike(f: &mut Frame) {
        let world = f.app.world_mut();
        let strike = benilla_formats::SpellDisplay {
            attributes_ex: 0x200,
            implicit_target_a1: 6,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SINISTER_STRIKE, strike)].into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: SINISTER_STRIKE,
                    kind: ACTION_KIND_SPELL,
                },
            );
    }

    /// The selections, strikes and swings sent, in wire order.
    fn strike_wire(f: &Frame) -> Vec<(&'static str, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(("select", guid)),
                ClientCommand::CastSpell {
                    spell_id: SINISTER_STRIKE,
                    target: Some(guid),
                } => Some(("cast", guid)),
                ClientCommand::AttackSwing { guid } => Some(("swing", guid)),
                _ => None,
            })
            .collect()
    }

    /// TryCast's attack pick (`0x6e4efb` → `0x612df0`): a strike pressed with no target selects the
    /// nearest enemy before its bind, so the selection goes out ahead of the cast and its swing,
    /// and the call after it reads the selection only once the cast is done.
    #[test]
    fn a_strike_with_no_target_selects_the_nearest_enemy_and_casts_at_it() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(
            strike_wire(&f),
            vec![
                ("select", MOB),
                ("cast", MOB),
                ("swing", MOB),
                ("select", NEW)
            ]
        );
    }

    /// A self-cast press passes the caster to the pick (`0x4e610e`), and the reaction to oneself is
    /// 4 (`0x606200`), so the pick Tabs to the enemy. With no faction table the caster would read
    /// neutral and be kept.
    #[test]
    fn a_self_cast_strike_selects_the_nearest_enemy() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, "UseAction(1, 0, 1)");
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// The pick's keep test (`0x6130a3`) drops a friend, so a strike at one retargets the nearest
    /// enemy, where the bind alone would refuse it as an invalid target.
    #[test]
    fn a_strike_at_a_friend_retargets_the_nearest_enemy() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// `0x6130d9`: with no enemy to acquire, the strike stops at the pick with "There is nothing to
    /// attack.", not the bind's "You have no target.".
    #[test]
    fn a_strike_with_nothing_to_attack_says_so_and_sends_nothing() {
        let mut f = frame(false);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(casts(&f), vec![]);
        assert_eq!(selected(&f), None);
        let world = f.app.world();
        assert_eq!(
            world.resource::<crate::ui_action::UiErrorKeys>().0,
            vec![crate::ui_action::UiError::key("ERR_NO_ATTACK_TARGET")]
        );
        assert!(world
            .resource::<crate::ui_action::CastErrors>()
            .0
            .is_empty());
    }

    const POTION: u64 = 0x4000_0000_0000_0f07;
    const POTION_ENTRY: u32 = 929;

    /// Backpack slot 1 holds `POTION`, of a template `t` makes, and bar slot 1 names its entry;
    /// the VM's backpack shows the slot as the container feed pushes it.
    fn with_potion(f: &mut Frame, t: benilla_protocol::messages::ItemInfo) {
        let world = f.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
                (532, POTION as u32),
                (533, (POTION >> 32) as u32),
            ])));
        crate::items::test_spawn_item(
            world,
            POTION,
            benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
            false,
        );
        let item_class = Some(t.class);
        world
            .resource_mut::<crate::items::Items>()
            .insert_template(POTION_ENTRY, Some(t));
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: POTION_ENTRY,
                    kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                },
            );
        world.non_send_resource_mut::<UiScript>().set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                num_slots: 16,
                slots: std::collections::HashMap::from([(
                    1,
                    benilla_ui::script::ContainerSlot {
                        item_id: POTION_ENTRY,
                        item_class,
                        count: 1,
                        guid: POTION,
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );
        let _ = f.rx.try_iter().count();
    }

    /// The bar's item use tests its copy's lock inside the call (`0x4e607b` → `0x5d8d32`): a
    /// pickup after it in the frame does not stop it, and one before it does, though the cursor
    /// let the item go again before the frame applied the press.
    #[test]
    fn a_bar_item_use_reads_the_lock_as_its_call_found_it() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Potion")
            },
        );
        run(&mut f, "UseAction(1) PickupContainerItem(0, 1)");
        assert!(
            f.rx.try_iter().count() > 0,
            "the use goes out; the pickup came after its call"
        );
        run(
            &mut f,
            "ClearCursor() PickupContainerItem(0, 1) UseAction(1) ClearCursor()",
        );
        assert_eq!(
            f.rx.try_iter().count(),
            0,
            "the copy was locked when the use was called"
        );
    }

    /// The bar's equip arm (`0x4e602a` → `0x5e1840` → `0x5e1480`) loads a projectile with
    /// `CMSG_SET_AMMO`, by its class (`0x5e16ff`, `0x5e1704`), and the arm's `ClearCursor(1,1)`
    /// unlocks it, signalling `ITEM_LOCK_CHANGED` though nothing held it (`0x5e1763`).
    #[test]
    fn a_bar_projectile_loads_by_class_and_signals_its_unlock() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                class: 6,
                inventory_type: 24,
                ..crate::items::test_template("Rough Arrow")
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "SIGNALS = 0 local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() SIGNALS = SIGNALS + 1 end)",
            )
            .unwrap();
        run(&mut f, "UseAction(1)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::SetAmmo {
                    entry: POTION_ENTRY
                }
            )),
            "{sent:?}"
        );
        let script = f.app.world().non_send_resource::<UiScript>();
        assert_eq!(script.eval::<i64>("return SIGNALS").unwrap(), 1);
        assert!(!script.item_locked(POTION));
    }

    /// The bar's item use reaches `CGItem::Use` (`0x4e607b`), which refuses a locked copy first
    /// (`0x5d8d32`): nothing goes out until the lock lifts.
    #[test]
    fn a_bar_item_use_skips_a_locked_copy() {
        let mut f = frame(false);
        {
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (22, 100),
                    (28, 100),
                    (46, 0x8),
                    (532, POTION as u32),
                    (533, (POTION >> 32) as u32),
                ])));
            crate::items::test_spawn_item(
                world,
                POTION,
                benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
                false,
            );
            world.resource_mut::<crate::items::Items>().insert_template(
                POTION_ENTRY,
                Some(benilla_protocol::messages::ItemInfo {
                    start_quest: 42,
                    ..crate::items::test_template("Questing Potion")
                }),
            );
            world
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: POTION_ENTRY,
                        kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                    },
                );
            world.non_send_resource_mut::<UiScript>().lock_item(POTION);
        }
        let _ = f.rx.try_iter().count();
        run(&mut f, "UseAction(1)");
        assert_eq!(f.rx.try_iter().count(), 0, "a locked copy is not used");
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(POTION);
        run(&mut f, "UseAction(1)");
        assert!(f.rx.try_iter().count() > 0, "unlocked, the use goes out");
    }
    const HELD: u64 = 0x4000_0000_0000_0001;
    const HELD_ENTRY: u32 = 7001;
    const HELM: u64 = 0x4000_0000_0000_0002;
    const HELM_ENTRY: u32 = 7002;

    /// Backpack slot 1 holds an ordinary item, slot 2 a helm of `bonding`, and bar slot 1 names
    /// the helm's entry; the VM's backpack shows both as the container feed pushes them.
    fn with_bar_helm(f: &mut Frame, bonding: u32) {
        let world = f.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        // Backpack slots 1 and 2 are the fields at 532 and 534.
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
                (532, HELD as u32),
                (533, (HELD >> 32) as u32),
                (534, HELM as u32),
                (535, (HELM >> 32) as u32),
            ])));
        for (guid, entry) in [(HELD, HELD_ENTRY), (HELM, HELM_ENTRY)] {
            crate::items::test_spawn_item(
                world,
                guid,
                benilla_protocol::ObjectFields::from_pairs(&[(3, entry)]),
                false,
            );
        }
        let mut items = world.resource_mut::<crate::items::Items>();
        items.insert_template(HELD_ENTRY, Some(crate::items::test_template("Held Thing")));
        items.insert_template(
            HELM_ENTRY,
            Some(benilla_protocol::messages::ItemInfo {
                inventory_type: 1,
                bonding,
                ..crate::items::test_template("A Helm")
            }),
        );
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: HELM_ENTRY,
                    kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                },
            );
        let slot = |guid, entry, equip: Vec<u8>, bonding| benilla_ui::script::ContainerSlot {
            guid,
            item_id: entry,
            count: 1,
            bonding,
            equip_slots: equip,
            ..Default::default()
        };
        let mut slots = std::collections::HashMap::new();
        slots.insert(1, slot(HELD, HELD_ENTRY, vec![], 0));
        slots.insert(2, slot(HELM, HELM_ENTRY, vec![1], bonding));
        world.non_send_resource_mut::<UiScript>().set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: Some("Backpack".into()),
                num_slots: 16,
                slots,
            }),
        );
    }

    /// The bar's equip runs `0x5e1840` (`0x4e602a`): `SetCursorItem`'s inner clear drops and
    /// unlocks a held item (`0x494bb6`), the bar's item takes the cursor and `AutoEquipCursorItem`
    /// sends it, so a key press leaves the cursor empty and the held item free.
    #[test]
    fn a_bar_equip_drops_the_held_item_and_unlocks_it() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        // Hold the first item, then press the bar key (`checkCursor` 0).
        run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");

        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_none(),
            "the held item was dropped and the bar's item sent off the cursor"
        );
        assert!(!script.item_locked(HELD), "the drop unlocked the held item");
        assert!(!script.item_locked(HELM), "the bar's pick takes no lock");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 24
                }
            )),
            "and the helm is equipped from backpack slot 2: {sent:?}"
        );
        // The set's pickup and the send's put-down both landed inside the call, so the pair is
        // played here, the held item's own put-down being the cursor diff's.
        let sounds: Vec<_> = f
            .app
            .world_mut()
            .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
            .drain()
            .collect();
        assert_eq!(sounds.len(), 1, "one pair, for the helm");
    }

    /// A bind-on-equip helm from the bar asks first (`0x5e16c6`) and stays on the cursor
    /// (`0x5e16ce`), not locked, since `SetCursorItem` takes no lock; the held item went back.
    #[test]
    fn a_bar_equip_of_a_bind_on_equip_item_holds_it_under_the_question() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 2);
        run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");

        let script = f.app.world().non_send_resource::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(HELM));
        assert!(!script.item_locked(HELM));
        assert!(!script.item_locked(HELD), "the held item went back");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(sent.is_empty(), "the question sends nothing: {sent:?}");
        assert_eq!(
            f.app
                .world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "and files its record"
        );
        assert!(
            f.app
                .world_mut()
                .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
                .drain()
                .next()
                .is_none(),
            "a held item plays no pair; the cursor diff plays its pickup"
        );
    }
    /// `SetCursorItem` refuses without control of the character (`0x494b8d`, `[0xb4b3e4]` clear
    /// and no `UNIT_FLAG_TAXI_FLIGHT`) before its clear and its set: the bar's equip puts nothing
    /// on the cursor and, with it empty, `AutoEquipCursorItem` has nothing to send.
    #[test]
    fn a_bar_equip_without_control_does_nothing() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        run(&mut f, "UseAction(1, 0)");
        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(script.cursor_item().is_none());
    }

    /// The refused set does not stop `0x5e1840`: `AutoEquipCursorItem` runs on the cursor as it
    /// stood (`0x5e18c9` falls through to `0x5e18d5`), so a held item is the one sent, the bar's
    /// never touched.
    #[test]
    fn a_refused_bar_set_still_auto_equips_what_is_held() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 23
                }]
            ),
            "the held item from backpack slot 1 goes, not the helm: {sent:?}"
        );
        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(script.cursor_item().is_none());
    }

    /// A taxi passenger has no control but passes the gate (`0x494ba6`-`0x494ba9`).
    #[test]
    fn a_bar_equip_on_a_taxi_passes_the_control_gate() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        {
            let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_player_control(false);
            script.set_on_taxi(true);
        }
        run(&mut f, "UseAction(1, 0)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 24
                }]
            ),
            "{sent:?}"
        );
    }

    /// `0x5e1840` refuses an item in the trade window with its line (`0x4bfd20`, `0x5e18a5`) and
    /// ends before the set: the bar sends nothing and the held item stays.
    #[test]
    fn a_bar_equip_of_an_item_in_the_trade_window_is_refused() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        run(
            &mut f,
            "PickupContainerItem(0, 2) ClickTradeButton(1) ClearCursor() PickupContainerItem(0, 1)",
        );
        let _ = f.rx.try_iter().count();
        run(&mut f, "UseAction(1, 0)");

        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(HELD));
        assert_eq!(script.take_ui_errors(), vec!["ERR_EQUIP_TRADE_ITEM"]);
    }

    /// The bar's item use reaches `CGItem::Use` (`0x4e607b`), whose control gate (`0x5d8eaa`) ends
    /// it with `ERR_CLIENT_LOCKED_OUT` unless the item's on-use spell is one that passes.
    #[test]
    fn a_bar_item_use_meets_the_control_gate() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                use_spell: Some(benilla_protocol::messages::ItemUseSpell {
                    spell_id: 4321,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }),
                ..crate::items::test_template("A Trinket")
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        let _ = f.rx.try_iter().count();
        run(&mut f, "UseAction(1)");
        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        let keys = |f: &Frame| -> Vec<&'static str> {
            f.app
                .world()
                .resource::<crate::ui_action::UiErrorKeys>()
                .0
                .iter()
                .map(|e| e.key)
                .collect()
        };
        assert_eq!(keys(&f), vec!["ERR_CLIENT_LOCKED_OUT"]);

        // The spell is one the app fed as passing: the gate lets the use on to the cast.
        f.app
            .world_mut()
            .resource_mut::<crate::ui_action::UiErrorKeys>()
            .0
            .clear();
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_control_bypass([4321].into());
        run(&mut f, "UseAction(1)");
        assert!(keys(&f).is_empty(), "{:?}", keys(&f));
    }
}
