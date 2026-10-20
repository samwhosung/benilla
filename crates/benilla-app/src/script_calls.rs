//! The script calls that touch the selection, the player's cast or the targeting cursor, applied
//! in the order the script made them, through one host ([`ScriptCallHost`]). In the reference each
//! such call is done when it returns (the macro runner `0x4f14e0` fires every line in one pass;
//! `TargetByName` `0x489d60` and `TargetUnit` `0x4899d0` commit through `SetSelection 0x493540`;
//! `CastSpellByName` `0x4b4ab0` reaches `ArmCastTargets 0x6e5250` through direct calls), so
//! `/target Bob` then `/cast Flash Heal` heals Bob, `/cast` then `/target` casts at the old target,
//! and `SpellIsTargeting()` on the line after a cast reads the word it armed.
//!
//! The entries that run a script with an input event up lend the host to the VM
//! ([`benilla_ui::script::UiScript::hosted`]): the mouse and key feed and the bindings pass, so
//! each call applies inside its verb. There an addon's or a macro's cast passes the gate's tag 2
//! (`0x494a92`); the client's own code passes it outside any input event (`0x494a5a` →
//! `0x494b11`). A call made elsewhere (an `OnUpdate`, a handler a feed's event runs) waits in the
//! VM's queue for the next lend, which applies it before its own script runs and under the events
//! it was made under; [`apply_script_calls`] lends the host once a frame after the world clicks.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{CallHost, CallTail, ScriptCall, UiScript};

/// The player's Attack (`0x6131a0`) in the reference's order: the validator's `SelectTarget`s bind a
/// unit to a standing targeting cursor's cast inside the validator, so that cast is sent before the
/// swing the validated target gets.
macro_rules! attack_in_order {
    ($appliers:expr, $may_pick:expr) => {{
        let target = $appliers.p7().validate_attack($may_pick);
        $appliers.p1().bind_parked();
        if let Some(guid) = target {
            $appliers.p7().start_attack(guid);
        }
    }};
}

/// The pet bar's Attack (`0x4bd40d`) runs the same validator, so a cast it binds to a standing
/// targeting cursor goes out ahead of the `CMSG_PET_ACTION` it validated.
macro_rules! pet_in_order {
    ($appliers:expr, $begin:ident($arg:expr)) => {{
        let send = $appliers.p6().$begin($arg);
        $appliers.p1().bind_parked();
        if let Some(send) = send {
            $appliers.p6().finish_press(send);
        }
    }};
}

/// The systems that lend [`ScriptCallHost`] to the VM: after what moves the world ahead of the
/// input pass, so the calls read it as [`apply_script_calls`] does. benilla runs the frame's UI
/// records, then its bindings, then its world clicks, where the reference dispatches each input
/// record whole in the order it arrived (`0x423920` → `0x4239a0`, `0x4245b0`) (#1988).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CallHosts;

/// The engine half of every script call: one applier per owner of the state a call touches.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // the appliers, one per owner of the state a call touches
pub(crate) struct ScriptCallHost<'w, 's> {
    appliers: ParamSet<
        'w,
        's,
        (
            crate::target::ScriptSelect<'w, 's>,
            crate::spell::ScriptCursor<'w, 's>,
            // The cast's own state: its cancel, the view the VM reads after each call, and the
            // reports the call made.
            ParamSet<
                'w,
                's,
                (
                    crate::spell::SelfCancel<'w, 's>,
                    crate::spell::CastState<'w, 's>,
                    crate::ui_cast::CallReports<'w, 's>,
                ),
            >,
            crate::spell::ScriptCast<'w, 's>,
            crate::ui_action::ActionPress<'w, 's>,
            crate::ui_items::ScriptItemUse<'w, 's>,
            crate::ui_pet::PetPress<'w, 's>,
            crate::ui_action::AttackPress<'w, 's>,
        ),
    >,
}

impl CallHost for ScriptCallHost<'_, '_> {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        let appliers = &mut self.appliers;
        // The gate's answer for this call, made where the Lua verb ran: its casts start only if
        // tag 2 passed there. The ladder reads it while this call applies.
        appliers.p3().ladder.permit(call.may_cast());
        let selected_before = appliers.p0().selected();
        let report_mark = appliers.p2().p2().mark();
        let mut tail: Option<CallTail> = None;
        match call {
            ScriptCall::Select { request, .. } => appliers.p0().select(request),
            ScriptCall::TargetByName { name, exact, .. } => {
                appliers.p0().target_by_name(&name, exact)
            }
            ScriptCall::TargetNearest { mode, reverse, .. } => {
                appliers.p0().target_nearest(mode, reverse);
            }
            ScriptCall::TargetLastTarget { .. } => appliers.p0().target_last_target(),
            ScriptCall::ClearTarget => appliers.p0().clear_target(),
            ScriptCall::AttackTarget { may_pick, .. } => attack_in_order!(appliers, may_pick),
            ScriptCall::SpellTargetUnit(token) => appliers.p1().spell_target_unit(&token),
            ScriptCall::SpellStopTargeting => appliers.p1().stop_targeting(),
            ScriptCall::SpellStopCasting => appliers.p2().p0().stop_casting(),
            ScriptCall::CastSpell {
                spell_id, on_self, ..
            } => {
                crate::ui_spellbook::cast_spell(&mut appliers.p3(), spell_id, on_self);
            }
            ScriptCall::CastShapeshiftForm { spell_id, .. } => {
                crate::ui_shapeshift::cast_form(&mut appliers.p3(), spell_id);
            }
            ScriptCall::UseAction(press) => {
                match crate::ui_action::use_action(&mut appliers.p4(), script, press) {
                    // The Attack slot is a cast, so the gate's tag 4 (the pick) is asked only
                    // after tag 2 passed, and so passes as well.
                    crate::ui_action::UseOutcome::Attack => attack_in_order!(appliers, true),
                    crate::ui_action::UseOutcome::Macro(id) => {
                        tail = Some(Box::new(move |script: &mut UiScript| {
                            if !crate::ui_macro::run_macro(script, id) {
                                debug!("script_calls: macro {id} is empty");
                            }
                        }));
                    }
                    crate::ui_action::UseOutcome::Done => {}
                }
            }
            ScriptCall::UseContainerItem(call) => {
                appliers.p5().use_container_item(script, call);
            }
            ScriptCall::UseInventoryItem { slot, on_self, .. } => {
                appliers.p5().use_inventory_item(script, slot, on_self)
            }
            ScriptCall::CastPetSpell { spell_id, on_self } => {
                crate::ui_pet_book::cast_pet_spell(&mut appliers.p6(), spell_id, on_self);
            }
            ScriptCall::PetAction(slot) => pet_in_order!(appliers, begin_slot(slot)),
            ScriptCall::PetOrder(packed) => pet_in_order!(appliers, begin_order(packed)),
        }
        // A selection this call made with the targeting cursor up was handed to the cast
        // (`SelectTarget`, `0x4935d5`) and is bound now, before the next call reads either.
        appliers.p1().bind_parked();
        // TryCast's attack pick can move the selection mid-cast (`0x6e4efb`), so a press held
        // there is picked and resumed before the next call reads the selection.
        if let Some(held) = appliers.p3().ladder.take_held() {
            // A held press passed tag 2 to be held, so the pick's tag 4 passes as well.
            if appliers.p7().pick_for_cast(held) {
                let mut cast = appliers.p3();
                let ctx = cast.targeting.context();
                cast.ladder.resume_after_pick(held, &ctx);
            }
        }
        // Whatever casts next is the engine's own, which asks no gate.
        appliers.p3().ladder.permit(true);
        // The reference signals `PLAYER_TARGET_CHANGED` inside `SetSelection`, so inside this
        // call's input events; the unit feed signals it a step later, and asks for them.
        let selected_after = appliers.p0().selected();
        let input = script.input_events();
        if selected_after != selected_before {
            script.note_selection_cause(selected_after, input);
        }
        // The reports the call made, in its order: a refusal's `UI_ERROR_MESSAGE` and its
        // `SPELLCAST_FAILED` (`ReportCastFailure 0x6e1a00` inside `TryCast 0x6e4b60`), a cancel's
        // event, `CURRENT_SPELL_CAST_CHANGED` at each of the attempt's signals, all signalled
        // below as the call returns, under its input events.
        let ready = appliers.p2().p2().ready_since(script, report_mark);
        // The cast state the call left, for the script's next line: `SpellStopCasting`
        // (`0x6e6e80`) finds the cast it sent in flight (`0x6e5026`), and `IsCurrentAction`
        // (`0x4e53a0`) checks its slot, as stock `ActionButtonUp` reads straight after `UseAction`
        // (`ActionButton.lua:54-55`).
        let view = appliers.p2().p1().view();
        script.set_cast_view(view);
        // The word the call left: `SpellIsTargeting` (`0x6e6cd0`) reads it, and `SpellTargetUnit`
        // binds to it.
        appliers.p1().push_view(script);
        if ready.is_empty() {
            return tail;
        }
        let macro_lines = tail.take();
        Some(Box::new(move |script: &mut UiScript| {
            crate::ui_cast::fire(script, ready);
            if let Some(lines) = macro_lines {
                lines(script);
            }
        }))
    }
}

impl ScriptCallHost<'_, '_> {
    /// The units a selection writer outside the calls (a loot window's select, the chat layer's
    /// `/target`) found the targeting cursor up for (`SelectTarget`, `0x4935d5`), bound to the cast
    /// in their place.
    fn bind_parked(&mut self) {
        self.appliers.p1().bind_parked();
    }
}

/// Run `f` on `script` with `host` lent, when the entry has one: a harness without the appliers'
/// state runs it unhosted, and its calls wait in the queue.
pub(crate) fn lent<R>(
    script: &mut UiScript,
    host: Option<&mut ScriptCallHost>,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    match host {
        Some(host) => script.hosted(host, f),
        None => f(script),
    }
}

/// Apply the calls queued where no host was lent, in call order, in the target chain after the
/// world clicks: once a frame, through the host the input pass lends.
pub(crate) fn apply_script_calls(script: Option<NonSendMut<UiScript>>, mut host: ScriptCallHost) {
    host.bind_parked();
    let Some(mut script) = script else {
        return;
    };
    script.hosted(&mut host, UiScript::drain_calls);
}

/// A host that applies each call with `apply`, for a harness that drains a VM's queue without the
/// app's appliers.
#[cfg(test)]
pub(crate) struct FnHost<F>(pub(crate) F);

#[cfg(test)]
impl<F: FnMut(&mut UiScript, ScriptCall)> CallHost for FnHost<F> {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        (self.0)(script, call);
        None
    }
}

/// Drain `script`'s queue through `apply`, front to back, each call with the input events its
/// verb ran under raised again, and the calls applying one makes before the next queued.
#[cfg(test)]
pub(crate) fn in_call_order(script: &mut UiScript, apply: impl FnMut(&mut UiScript, ScriptCall)) {
    script.hosted(&mut FnHost(apply), UiScript::drain_calls);
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

    mod cast_reports;

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
        record_reports(&script);
        world.insert_non_send_resource(script);
        Frame { app, rx }
    }

    /// Every catalog line and cast-failure string defined as its own key, and a recorder of the
    /// lines shown (`UI_ERROR_MESSAGE`) in `LINES`, and of those lines and the cast's events in
    /// `REPORTS`, in the order they fire.
    fn record_reports(script: &UiScript) {
        let mut lua = String::new();
        for key in benilla_ui::messages::CATALOG
            .iter()
            .map(|r| r.key)
            .chain(crate::ui_action::CAST_FAIL_KEYS)
            .filter(|k| !k.is_empty())
        {
            lua.push_str(&format!("{key} = \"{key}\"\n"));
        }
        lua.push_str(
            r#"LINES, REPORTS = {}, {}
            local r = CreateFrame("Frame")
            for _, e in ipairs({"UI_ERROR_MESSAGE", "SPELLCAST_FAILED", "SPELLCAST_INTERRUPTED",
                    "SPELLCAST_STOP"}) do
                r:RegisterEvent(e)
            end
            r:SetScript("OnEvent", function()
                if event == "UI_ERROR_MESSAGE" then
                    table.insert(LINES, arg1)
                    table.insert(REPORTS, arg1)
                else
                    table.insert(REPORTS, event)
                end
            end)"#,
        );
        script.run(&lua).expect("the recorder installs");
    }

    /// The lines and cast events [`record_reports`] saw since the last read, in order.
    fn reports(f: &mut Frame) -> Vec<String> {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>("local r = REPORTS REPORTS = {} return r")
            .expect("the recorder reads")
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

    /// [`run`] with `lua` as a key's body, an input event of the protected-action gate
    /// (`[[0xb71290]+0xd8]`): the press a macro's `/script` line needs to cast.
    fn run_pressed(f: &mut Frame, lua: &str) {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run_binding(lua)
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
            ..Default::default()
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

    /// The keys of the error lines shown, each defined as its own key ([`record_reports`]).
    fn errors(f: &Frame) -> Vec<String> {
        f.app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return LINES")
            .expect("the recorder reads")
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
            // A player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PLAYER_CONTROLLED`): `CanAssist`
            // judges him on his own player's rungs, then the templates.
            with_factions(&mut f, selected_template, &[(2, 0x19), (FLAGS, 0x8)]);
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

    /// `TargetByName` (`0x489d60`) on a miss leaves the target alone and shows message `0x127`
    /// (`ERR_UNIT_NOT_FOUND`) for a name given (`0x489dbd`, tested on the raw string's first byte,
    /// so blanks count) or `0xb8` (`ERR_GENERIC_NO_TARGET`) for an empty one (`0x489dcf`), inside
    /// the call.
    #[test]
    fn target_by_name_that_finds_nobody_shows_the_references_line() {
        for (name, line) in [
            ("Nobody", "ERR_UNIT_NOT_FOUND"),
            ("   ", "ERR_UNIT_NOT_FOUND"),
            ("", "ERR_GENERIC_NO_TARGET"),
        ] {
            let mut f = frame(true);
            run(&mut f, &format!("TargetByName(\"{name}\")"));
            assert_eq!(errors(&f), vec![line], "{name:?}");
            assert_eq!(selected(&f), Some(OLD), "{name:?}: the target stays");
            assert_eq!(f.rx.try_iter().count(), 0, "{name:?}: nothing is sent");
        }
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

    /// `SetLootMethod`'s master name runs the resolver in mode 2 (`0x4e9418`, filter `0x493eca`):
    /// a held groupmate resolves through the candidate feed while `CanAssist` and alive, and a
    /// dead one is no candidate, so the name finds nobody and the binding sends nothing.
    #[test]
    fn set_loot_method_resolves_a_held_groupmate_only_while_alive() {
        use benilla_ui::script::{PartyMemberInfo, PartyRequest, PartyState};
        for (health, resolved) in [(100, true), (0, false)] {
            let mut f = frame(false);
            // OLD is a friendly player, our groupmate "Olly" in `party1`.
            with_factions(&mut f, FRIEND, &[(2, 0x19), (FLAGS, 0x8), (HEALTH, health)]);
            let world = f.app.world_mut();
            let mut names = world.resource_mut::<crate::names::NameCache>();
            names.insert_player(OLD, "Olly".into(), None);
            names.insert_player(ME, "Me".into(), None);
            world
                .run_system_once(crate::ui_unit::feed_name_candidates)
                .expect("the candidate feed runs as a one-shot system");
            let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_party(PartyState {
                members: vec![PartyMemberInfo {
                    name: "Olly".into(),
                    guid: OLD,
                }],
                leader_index: 0,
                leader_guid: ME,
                own_guid: ME,
                ..Default::default()
            });
            script.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: ME,
                    ..Default::default()
                }),
            );
            script.set_unit(
                "party1",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: OLD,
                    ..Default::default()
                }),
            );
            script
                .run(r#"SetLootMethod("master", "Ol")"#)
                .expect("the script runs");
            let sent = script.take_party_requests();
            let expected = resolved.then_some(PartyRequest::LootMethod {
                method: 2,
                master: OLD,
                threshold: 2,
            });
            assert_eq!(sent, expected.into_iter().collect::<Vec<_>>(), "{health}");
        }
    }

    /// Our own name resolves through the object pass at distance 0 (`0x493d30` measures from our
    /// own object, which the roster passes skip as held), and passes mode 2 while its health is
    /// above 0 (`0x493eca`): a ghost, at health 1, does; a corpse, at 0, does not.
    #[test]
    fn set_loot_method_resolves_our_own_name_through_the_feed() {
        use benilla_ui::script::{PartyRequest, PartyState};
        for (health, resolved) in [(100, true), (1, true), (0, false)] {
            let mut f = frame(false);
            with_factions(&mut f, FRIEND, &[]);
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (HEALTH, health),
                    (28, 100),
                    (FLAGS, 0x8),
                    (TEMPLATE, OURS),
                ])));
            world
                .resource_mut::<crate::names::NameCache>()
                .insert_player(ME, "Kelvin".into(), None);
            world
                .run_system_once(crate::ui_unit::feed_name_candidates)
                .expect("the candidate feed runs as a one-shot system");
            let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_party(PartyState {
                members: vec![benilla_ui::script::PartyMemberInfo {
                    name: "Olly".into(),
                    guid: OLD,
                }],
                leader_guid: ME,
                own_guid: ME,
                ..Default::default()
            });
            script.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: ME,
                    ..Default::default()
                }),
            );
            script
                .run(r#"SetLootMethod("master", "kel")"#)
                .expect("the script runs");
            let expected = resolved.then_some(PartyRequest::LootMethod {
                method: 2,
                master: ME,
                threshold: 2,
            });
            assert_eq!(
                script.take_party_requests(),
                expected.into_iter().collect::<Vec<_>>(),
                "health {health}"
            );
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
            assert_eq!(errors(&f), Vec::<String>::new(), "{lua}");
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

    /// `0x6131a0` stops a running attack only with `[0xb4b3e4]` set (`0x6131ce`); without control
    /// it falls to the start (`0x5ecb70`), which returns at the same flag (`0x5ecbc0`) before the
    /// stand, the swing and the sheath: the press neither stops nor swings.
    #[test]
    fn attack_without_control_neither_stops_nor_swings() {
        for engaged in [true, false] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, 100)]);
            let world = f.app.world_mut();
            world.init_resource::<crate::player::Player>();
            world.resource_mut::<crate::player::OwnControlLost>().0 = true;
            if engaged {
                let me = world.resource::<GuidIndex>().0[&ME];
                world
                    .entity_mut(me)
                    .insert(crate::creature_anim::Engaged(OLD));
            }
            run(&mut f, "AttackTarget()");
            assert_eq!(swings(&f), (vec![], false), "engaged {engaged}");
        }
    }

    /// TryCast's tail starts the swing of a cast that initiates combat through `0x6131a0`
    /// (`0x6e51b5`), whose start returns at a clear `[0xb4b3e4]`: the cast goes out, the swing
    /// does not.
    #[test]
    fn an_on_next_swing_cast_without_control_sends_no_swing() {
        const STRIKE: u32 = 78;
        for lost in [false, true] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, 100)]);
            with_book_spell(
                &mut f,
                STRIKE,
                "Strike",
                benilla_formats::SpellDisplay {
                    targets: 0x2,
                    attributes: 0x404,
                    ..Default::default()
                },
            );
            let world = f.app.world_mut();
            world.init_resource::<crate::player::Player>();
            world.resource_mut::<crate::player::OwnControlLost>().0 = lost;
            run(&mut f, r#"CastSpellByName("Strike")"#);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert!(
                sent.iter().any(
                    |c| matches!(c, ClientCommand::CastSpell { spell_id, .. } if *spell_id == STRIKE)
                ),
                "lost {lost}: the cast goes out: {sent:?}"
            );
            assert_eq!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::AttackSwing { guid } if *guid == OLD)),
                !lost,
                "lost {lost}: {sent:?}"
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
                    CreateMacro("heal", 1, 'CastSpellByName("Heal")', 1, nil)"#,
                )
                .expect("the macro is made");
            world
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        // The first account macro's id, one past the counter's start (`0x4eff90`).
                        action: 1,
                        kind: ACTION_KIND_MACRO,
                    },
                );
        }
        // The macro's line is a `RunScript` chunk, which casts only inside an input event: the
        // press is a key's body, and the line runs inside it in the reference.
        run_pressed(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// The macro's lines run inside the press (`0x4e6098`), so the input event that pressed it is
    /// still up for them; here the press has returned by the time the app runs them, so they run
    /// with it raised again. A press that was no input event (an addon's `OnUpdate`) hands them
    /// none, and a `/script` cast in them is refused, silently.
    #[test]
    fn a_macros_script_line_casts_only_when_its_press_was_an_input_event() {
        for (pressed, want) in [(true, vec![(HEAL, Some(OLD))]), (false, vec![])] {
            let mut f = frame(true);
            {
                let world = f.app.world_mut();
                let script = world.non_send_resource::<UiScript>();
                script
                    .run(
                        r#"local f = CreateFrame("Frame")
                        f:RegisterEvent("EXECUTE_CHAT_LINE")
                        f:SetScript("OnEvent", function() RunScript(arg1) end)
                        CreateMacro("heal", 1, 'CastSpellByName("Heal")', 1, nil)"#,
                    )
                    .expect("the macro is made");
                world
                    .resource_mut::<crate::ui_action::PlayerActions>()
                    .buttons
                    .insert(
                        0,
                        ActionButton {
                            slot: 0,
                            // The first account macro's id (`0x4eff90`).
                            action: 1,
                            kind: ACTION_KIND_MACRO,
                        },
                    );
            }
            if pressed {
                run_pressed(&mut f, "UseAction(1)");
            } else {
                run(&mut f, "UseAction(1)");
            }
            assert_eq!(casts(&f), want, "pressed in an input event: {pressed}");
        }
    }

    /// `UseAction` on a spell is `TryCastSpell`, whose gate is its first rung (`0x6e4bc1`): a macro's
    /// call outside an input event queues the press, which the app finds refused and sends
    /// nothing, where the client's own press casts. The same press on a macro slot still runs it.
    #[test]
    fn a_use_action_the_gate_refused_casts_nothing() {
        for (lua, want) in [
            ("UseAction(1)", vec![(HEAL, Some(OLD))]),
            (r#"RunScript("UseAction(1)")"#, vec![]),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: HEAL,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(casts(&f), want, "{lua}");
        }
    }

    /// Ghost Wolf: an aura with an `ActiveIconID`, which a second press cancels.
    const GHOST_WOLF: u32 = 2645;

    /// Ghost Wolf in the catalog, the book and the form bar, and up on us as a cancelable aura.
    fn with_ghost_wolf_up(f: &mut Frame, up: bool) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            active_icon_id: 1,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(GHOST_WOLF, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let me = world.resource::<GuidIndex>().0[&ME];
        let mut pairs = vec![(2, 0x19), (HEALTH, 100), (28, 100), (FLAGS, 0x8)];
        if up {
            // `UNIT_FIELD_AURA` 47 and the nibble-packed `UNIT_FIELD_AURAFLAGS` 95: occupied and
            // cancelable.
            pairs.extend([(47, GHOST_WOLF), (95, 0xb)]);
        }
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
                &pairs,
            )));
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Druid".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: GHOST_WOLF,
                name: "Ghost Wolf".into(),
                ..Default::default()
            }],
        });
        script.set_shapeshift_forms(vec![benilla_ui::script::ShapeshiftFormView {
            spell_id: GHOST_WOLF,
            name: "Ghost Wolf".into(),
            castable: true,
            ..Default::default()
        }]);
    }

    /// The `CMSG_CANCEL_AURA`s sent, as spell ids.
    fn cancels(f: &Frame) -> Vec<u32> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelAura { spell_id } => Some(spell_id),
                _ => None,
            })
            .collect()
    }

    /// `CastBookSpell` (`0x4b3300`) and `CastShapeshiftForm` (`0x4b4810`) cancel an active aura or
    /// form before `TryCast` (`0x4b3466`, `0x4b496b`, `0x4b497c`), so the gate's tag 2, which is
    /// `TryCast`'s first rung (`0x6e4bc1`), refuses only the cast: a macro's press outside an input
    /// event cancels the wolf all the same, and casts nothing when it is down.
    #[test]
    fn a_refused_book_or_form_cast_still_runs_its_cancels() {
        for lua in [
            r#"RunScript('CastSpellByName("Ghost Wolf")')"#,
            r#"RunScript("CastSpell(1, BOOKTYPE_SPELL)")"#,
            r#"RunScript("CastShapeshiftForm(1)")"#,
        ] {
            let mut f = frame(true);
            with_ghost_wolf_up(&mut f, true);
            run(&mut f, lua);
            assert_eq!(cancels(&f), vec![GHOST_WOLF], "{lua}: up, cancels");

            let mut f = frame(true);
            with_ghost_wolf_up(&mut f, false);
            run(&mut f, lua);
            assert_eq!(cancels(&f), Vec::<u32>::new(), "{lua}: down");
            assert_eq!(casts(&f), vec![], "{lua}: down, refused, casts nothing");
        }
        // The client's own call casts when the wolf is down.
        let mut f = frame(true);
        with_ghost_wolf_up(&mut f, false);
        run(&mut f, "CastShapeshiftForm(1)");
        assert_eq!(
            casts(&f).iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![GHOST_WOLF]
        );
    }

    /// The stock UI over the frame's world, with an insecure addon, `Hook`, whose handler of
    /// `PLAYER_TARGET_CHANGED` casts Heal.
    fn with_stock_ui_and_a_casting_hook(f: &mut Frame) {
        with_stock_ui(f, true);
    }

    /// The stock UI over the frame's world, and with `hook` the addon above.
    fn with_stock_ui(f: &mut Frame, hook: bool) {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut script, failures) = crate::ui_script::layer_tests::production_load_with(
            if hook { "tab-hook" } else { "tab-stock" },
            false,
            "",
            |root| {
                if !hook {
                    return;
                }
                let dir = root.join("Hook");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("Hook.toc"), "## Interface: 11200\nhook.lua\n").unwrap();
                std::fs::write(
                    dir.join("hook.lua"),
                    r#"local f = CreateFrame("Frame")
                       f:RegisterEvent("PLAYER_TARGET_CHANGED")
                       f:SetScript("OnEvent", function() CastSpellByName("Heal") end)"#,
                )
                .unwrap();
            },
        );
        assert!(failures.is_empty(), "{failures:#?}");
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
        f.app.world_mut().insert_non_send_resource(script);
    }

    /// Apply the queued calls, then the unit feed (which signals the selection's change), then the
    /// calls the handlers made.
    fn settle(f: &mut Frame) {
        let world = f.app.world_mut();
        world.run_system_once(apply_script_calls).unwrap();
        crate::ui_unit::run_feed_units_once(world);
        world.run_system_once(apply_script_calls).unwrap();
    }

    /// `SetSelection` signals `PLAYER_TARGET_CHANGED` inside `ExecuteBinding` (`0x4b7b02`-
    /// `0x4b7b11`), so an insecure addon's handler that casts passes after a TAB press. Here the
    /// unit feed signals it a step after the call is applied, with the call's input events raised
    /// again around it: the stock `TARGETNEARESTENEMY` binding moves the target, the addon's handler
    /// casts at the new one. A change no call made (the server's) hears it with none up, and its
    /// cast is refused.
    #[test]
    fn a_tab_targets_and_the_addons_target_handler_casts() {
        benilla_formats::wow_data_or_skip!();
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_stock_ui_and_a_casting_hook(&mut f);
        // The feed's first pass seats the selection it finds, with its world-enter events.
        settle(&mut f);
        f.rx.try_iter().for_each(drop);

        let pressed = f
            .app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .execute_binding("TARGETNEARESTENEMY", true)
            .expect("the binding runs");
        assert!(pressed);
        settle(&mut f);
        assert_eq!(selected(&f), Some(MOB), "TAB picked the enemy");
        assert_eq!(
            casts(&f),
            vec![(HEAL, Some(MOB))],
            "the handler's cast passed"
        );

        // The same edge with no call behind it: the server moves the target.
        f.app.world_mut().resource_mut::<Selection>().guid = Some(NEW);
        settle(&mut f);
        assert_eq!(
            casts(&f),
            vec![],
            "no input event up for the handler: refused"
        );
    }

    /// A refusal a call raises is signalled inside the call, under the input events it ran under,
    /// as the reference shows it inside the press: the line's handler runs with the key's event
    /// up, so a movement verb it calls passes the gate's tag 0 (`0x494b1a`, the client's own code
    /// with an input event running), and outside a press it does not.
    #[test]
    fn a_refusal_a_call_raises_carries_the_events_it_ran_under() {
        for (pressed, moved) in [(true, true), (false, false)] {
            let mut f = frame(false);
            with_factions(&mut f, FRIEND, &[]);
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(
                    r#"local h = CreateFrame("Frame")
                    h:RegisterEvent("UI_ERROR_MESSAGE")
                    h:SetScript("OnEvent", function() MoveForwardStart() end)"#,
                )
                .unwrap();
            if pressed {
                run_pressed(&mut f, "AttackTarget()");
            } else {
                run(&mut f, "AttackTarget()");
            }
            assert_eq!(
                errors(&f),
                vec!["ERR_NO_ATTACK_TARGET"],
                "pressed: {pressed}"
            );
            let inputs = f
                .app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_binding_input();
            assert_eq!(!inputs.is_empty(), moved, "pressed: {pressed}");
        }
    }

    /// The ladder reads the permit as an optional resource, so a world without it passes every
    /// cast; the client must register it, or the gate is silently off.
    #[test]
    fn the_client_registers_the_cast_permit() {
        let f = frame(false);
        assert!(f
            .app
            .world()
            .get_resource::<crate::spell::ScriptCastPermit>()
            .is_some());
    }

    /// The Attack slot is a cast too (`UseAction` → `TryCast`, `0x6e4c90`), behind the same first
    /// rung: a macro's press outside an input event swings at nothing, not even at a target
    /// already selected.
    #[test]
    fn a_use_action_on_attack_the_gate_refused_swings_at_nothing() {
        for (lua, want) in [
            ("UseAction(1)", vec![OLD]),
            (r#"RunScript("UseAction(1)")"#, vec![]),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::PlayerActions>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: crate::ui_action::SPELL_ATTACK,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(swings(&f), (want, false), "{lua}");
        }
    }

    /// The attack validator's pick of the nearest enemy asks the gate's tag 4 (`0x6130bc` →
    /// `0x493f9b`): refused, nothing is picked and the selection stands, so the friend selected is
    /// refused as the target and no swing goes out.
    #[test]
    fn an_attack_target_the_gate_refused_picks_no_enemy() {
        for (lua, selected_after, swung) in [
            ("AttackTarget()", MOB, vec![MOB]),
            (r#"RunScript("AttackTarget()")"#, OLD, vec![]),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            run(&mut f, lua);
            assert_eq!(selected(&f), Some(selected_after), "{lua}");
            assert_eq!(swings(&f), (swung, false), "{lua}");
        }
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
        assert_eq!(
            reports(&mut f),
            vec!["ERR_NO_ATTACK_TARGET"],
            "the pick's own line, which is no cast failure"
        );
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

    /// `CGItem::Use` casts an item's on-use spell through `TryCastSpell`, behind the gate's tag 2
    /// (`0x4fa430` and `0x4c7af0` → `0x5d8d00` → `0x5d9258` → `0x6e5a90` → `0x6e4bc1`). A bag
    /// click, a worn item's click and a bar press queue whatever the gate said, and the cast
    /// ladder refuses the refused ones: the client's own call sends `CMSG_USE_ITEM`, a macro's
    /// outside an input event sends nothing.
    #[test]
    fn a_refused_item_use_never_reaches_the_cast() {
        const HEAD_ITEM: u64 = 0x4000_0000_0000_0f08;
        let template = || {
            let mut t = crate::items::test_template("Trinket");
            t.spells = vec![benilla_protocol::messages::ItemSpellEntry {
                index: 0,
                spell_id: 4321,
                trigger: 0,
                charges: 0,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }];
            t.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
            t
        };
        let uses = |f: &Frame| {
            f.rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::UseItem { .. }))
                .count()
        };
        for lua in [
            "UseContainerItem(0, 1)",
            "UseAction(1)",
            "UseInventoryItem(1)",
        ] {
            for (call, want) in [(lua.to_string(), 1), (format!("RunScript({lua:?})"), 0)] {
                let mut f = frame(false);
                with_potion(&mut f, template());
                // The worn item: slot 1 of the doll, the head, a copy of the same trinket.
                let world = f.app.world_mut();
                let me = world.resource::<GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x19),
                        (22, 100),
                        (28, 100),
                        (46, 0x8),
                        (486, HEAD_ITEM as u32),
                        (487, (HEAD_ITEM >> 32) as u32),
                        (532, POTION as u32),
                        (533, (POTION >> 32) as u32),
                    ]),
                ));
                crate::items::test_spawn_item(
                    world,
                    HEAD_ITEM,
                    benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
                    false,
                );
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[1] = Some(benilla_ui::script::InvSlotView {
                    item_id: POTION_ENTRY,
                    guid: HEAD_ITEM,
                    count: 1,
                    use_spell: 4321,
                    ..Default::default()
                });
                world
                    .non_send_resource_mut::<UiScript>()
                    .set_inventory_slots(slots);
                let _ = f.rx.try_iter().count();
                run(&mut f, &call);
                assert_eq!(uses(&f), want, "{call}");
            }
        }
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
                name: None,
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
        // The refusal tests the partner cell first (`0x4bfd23`): the window is open.
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .open_trade_window();
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
        assert_eq!(reports(&mut f), vec!["ERR_CLIENT_LOCKED_OUT"]);

        // The spell is one the app fed as passing: the gate lets the use on to the cast.
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_control_bypass([4321].into());
        run(&mut f, "UseAction(1)");
        let shown = reports(&mut f);
        assert!(shown.is_empty(), "{shown:?}");
    }

    /// A heal's targeting cursor up: `TARGET_FLAG_UNIT` (0x2), which any unit binds, in the app's
    /// word and the VM's view of it.
    fn with_the_cursor_up(f: &mut Frame) {
        // The attempt that raised it filled the record with the player as its caster (`0x6e4fb1`).
        f.app
            .world_mut()
            .resource_mut::<crate::spell::PendingCast>()
            .fill_record();
        f.app
            .world_mut()
            .resource_mut::<crate::spell::SpellTargeting>()
            .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_spell_targeting(true);
    }

    fn cursor_is_up(f: &Frame) -> bool {
        f.app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .active()
    }

    /// The `CMSG_SET_SELECTION`s sent.
    fn selections(sent: &[ClientCommand]) -> Vec<u64> {
        sent.iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(*guid),
                _ => None,
            })
            .collect()
    }

    /// The units a selecting verb can find in the frame: `MOB` (an enemy, named, party1's target
    /// and the last enemy) beside `OLD` (selected) and `NEW` (party1, "Bob"), with `OLD` the last
    /// target.
    fn with_things_to_select(f: &mut Frame) {
        spawn_mob(f, &[]);
        let world = f.app.world_mut();
        let new = world.resource::<GuidIndex>().0[&NEW];
        world
            .entity_mut(new)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
                (16, MOB as u32),
            ])));
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(NEW, "Bob".into(), None);
        world.resource_mut::<crate::target::LastEnemy>().0 = Some(MOB);
        world.resource_mut::<Selection>().last = Some(NEW);
    }

    /// `SelectTarget` (`0x493540`) with the targeting cursor up binds the unit to the pending cast
    /// and returns without selecting (`0x4935ca`-`0x4935d5`), from every verb that calls it:
    /// `TargetUnit`, `AssistUnit` and `TargetLastEnemy` (`0x489a40`), `AssistByName`,
    /// `TargetLastTarget`, `TargetByName` (`0x489daa`) and `TargetNearestEnemy` (`0x4941e6`). The
    /// bind asks the gate's tag 2 (`0x6e5ba5`): a macro's line outside an input event is refused,
    /// and `SelectTarget` returns all the same, with the cursor still up and nothing selected.
    #[test]
    fn a_selecting_verb_with_the_cursor_up_casts_at_its_unit_and_selects_nothing() {
        for (lua, bound) in [
            (r#"TargetUnit("party1")"#, NEW),
            (r#"AssistUnit("party1")"#, MOB),
            ("TargetLastEnemy()", MOB),
            (r#"TargetByName("Red Affinity")"#, MOB),
            (r#"AssistByName("Bob")"#, MOB),
            ("TargetLastTarget()", NEW),
            ("TargetNearestEnemy()", MOB),
        ] {
            // Pressed: the call is the client's own inside an input event.
            let mut f = frame(true);
            with_things_to_select(&mut f);
            with_the_cursor_up(&mut f);
            run_pressed(&mut f, lua);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert!(
                matches!(sent.iter().find_map(|c| match c {
                    ClientCommand::CastSpell { spell_id, target } => Some((*spell_id, *target)),
                    _ => None,
                }), Some((HEAL, Some(g))) if g == bound),
                "{lua}: casts at {bound:#x}: {sent:?}"
            );
            assert_eq!(
                selections(&sent),
                Vec::<u64>::new(),
                "{lua}: selects nothing"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
            assert!(!cursor_is_up(&f), "{lua}: the bind emptied the word");

            // The same line as a macro's `/script`: tag 2 refuses the bind.
            let mut f = frame(true);
            with_things_to_select(&mut f);
            with_the_cursor_up(&mut f);
            run(&mut f, &format!("RunScript({lua:?})"));
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert_eq!(casts_of(&sent), vec![], "{lua}: refused, casts nothing");
            assert_eq!(
                selections(&sent),
                Vec::<u64>::new(),
                "{lua}: and selects nothing"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
            assert!(cursor_is_up(&f), "{lua}: the cursor stands");

            // With the cursor down the same verb selects, and casts nothing.
            if lua != "TargetNearestEnemy()" {
                let mut f = frame(true);
                with_things_to_select(&mut f);
                run_pressed(&mut f, lua);
                let sent: Vec<_> = f.rx.try_iter().collect();
                assert_eq!(casts_of(&sent), vec![], "{lua}: cursor down");
                assert_eq!(selected(&f), Some(bound), "{lua}: cursor down");
            }
        }
    }

    /// The `CMSG_CAST_SPELL`s among `sent`.
    fn casts_of(sent: &[ClientCommand]) -> Vec<(u32, Option<u64>)> {
        sent.iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => Some((*spell_id, *target)),
                _ => None,
            })
            .collect()
    }

    /// TAB with a heal's cursor up, through the stock `TARGETNEARESTENEMY` binding: the cycler picks
    /// the enemy, `SelectTarget` binds it (`0x4941e6` → `0x4935d5`) and the heal goes to it, the
    /// target where it was. A second TAB, the cursor down now, selects as ever.
    #[test]
    fn a_tab_with_a_heals_cursor_up_casts_at_the_tabbed_unit() {
        benilla_formats::wow_data_or_skip!();
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_stock_ui(&mut f, false);
        settle(&mut f);
        with_the_cursor_up(&mut f);
        f.rx.try_iter().for_each(drop);

        let tab = |f: &mut Frame| {
            let pressed = f
                .app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .execute_binding("TARGETNEARESTENEMY", true)
                .expect("the binding runs");
            assert!(pressed);
            settle(f);
            f.rx.try_iter().collect::<Vec<_>>()
        };
        let sent = tab(&mut f);
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(MOB))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD), "TAB left the target alone");
        assert!(!cursor_is_up(&f));

        let sent = tab(&mut f);
        assert_eq!(casts_of(&sent), vec![]);
        assert_eq!(selections(&sent), vec![MOB], "the cursor down, TAB selects");
    }

    /// A charmed player's `SelectTarget` returns before anything (`0x493589`-`0x49359c`): the bind
    /// does not happen with the cursor up, nor the selection with it down.
    #[test]
    fn a_charmed_player_neither_binds_nor_selects() {
        for cursor in [true, false] {
            let mut f = frame(true);
            let me = f.app.world().resource::<GuidIndex>().0[&ME];
            f.app.world_mut().entity_mut(me).insert(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (HEALTH, 100),
                    (28, 100),
                    // `UNIT_FIELD_CHARMEDBY`, a guid of two fields.
                    (10, 0x99),
                    (11, 0),
                ]),
            ));
            if cursor {
                with_the_cursor_up(&mut f);
            }
            run_pressed(&mut f, r#"TargetUnit("party1")"#);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert_eq!(casts_of(&sent), vec![], "cursor {cursor}");
            assert_eq!(selections(&sent), Vec::<u64>::new(), "cursor {cursor}");
            assert_eq!(selected(&f), Some(OLD), "cursor {cursor}");
            assert_eq!(cursor_is_up(&f), cursor, "cursor {cursor}");
        }
    }

    /// A deselect passes guid 0 and skips the bind (`0x4935ac`): `ClearTarget` with the cursor up
    /// clears the target and the cursor stands.
    #[test]
    fn clear_target_with_the_cursor_up_still_clears() {
        let mut f = frame(true);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "ClearTarget()");
        assert_eq!(selected(&f), None);
        assert!(cursor_is_up(&f));
        assert_eq!(casts(&f), vec![]);
    }

    /// The attack validator selects a unit twice over: the one it keeps (`0x613100`, when the
    /// selection is not friendly) and the nearest enemy it picks otherwise (`0x4941e6`). With the
    /// cursor up each is bound to the cast and the selection stays; the attack goes on past it.
    #[test]
    fn attack_target_with_the_cursor_up_binds_the_unit_it_would_select() {
        // A hostile selection is kept (`0x6130a3`): the bind is at it, and the swing follows.
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "AttackTarget()");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(OLD))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD));
        // The bind is inside the validator, ahead of the swing it validates.
        let at = |want: fn(&ClientCommand) -> bool| sent.iter().position(want);
        let (cast, swing) = (
            at(|c| matches!(c, ClientCommand::CastSpell { .. })),
            at(|c| matches!(c, ClientCommand::AttackSwing { guid: OLD })),
        );
        assert!(
            cast.is_some() && swing.is_some() && cast < swing,
            "the cast goes out before the swing: {sent:?}"
        );

        // A friendly one is dropped for the nearest enemy, which the bind takes.
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "AttackTarget()");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(MOB))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD), "the friend stays selected");
    }

    /// The selection writers outside the script calls (a loot window's select, `0x48f3a0`) hand the
    /// unit to the cursor too, and the next pass of the calls binds it.
    #[test]
    fn a_loot_windows_select_with_the_cursor_up_is_bound_by_the_next_pass() {
        let mut f = frame(true);
        spawn_mob(&mut f, &[(2, 0x9)]);
        with_the_cursor_up(&mut f);
        let world = f.app.world_mut();
        world
            .run_system_once(|mut select: crate::target::SelectCommit| select.select_unit(MOB))
            .expect("the select runs");
        assert_eq!(selected(&f), Some(OLD), "nothing selected");
        assert_eq!(casts(&f), vec![], "and nothing bound until the pass");
        f.app
            .world_mut()
            .run_system_once(apply_script_calls)
            .expect("the calls apply as a one-shot system");
        assert_eq!(casts(&f), vec![(HEAL, Some(MOB))]);
        assert_eq!(selected(&f), Some(OLD));
    }

    /// `CastBookSpell` (`0x4b3300`) asks `GetTargetingSpellId` (`0x6e48e0`) ahead of the cast and the
    /// gate: if the book spell is the one awaiting its target, the press stops targeting
    /// (`StopSpellTargeting`, `0x4b347d`) and casts nothing, a macro's refused one as well. A
    /// different spell's cursor is not its to stop: the cast goes and the cursor with it.
    #[test]
    fn a_book_cast_of_the_spell_awaiting_its_target_stops_targeting() {
        for lua in [
            r#"CastSpellByName("Heal")"#,
            "CastSpell(1, BOOKTYPE_SPELL)",
            r#"RunScript('CastSpellByName("Heal")')"#,
            r#"RunScript("CastSpell(1, BOOKTYPE_SPELL)")"#,
        ] {
            let mut f = frame(true);
            with_the_cursor_up(&mut f);
            run(&mut f, lua);
            assert!(!cursor_is_up(&f), "{lua}: the press stopped targeting");
            assert_eq!(
                reports(&mut f),
                vec!["SPELLCAST_FAILED"],
                "{lua}: as a cancel with its failed flag (`0x6e4900`), not a commit"
            );
            assert_eq!(casts(&f), vec![], "{lua}: and cast nothing");
        }

        // Heal's cursor, Ghost Wolf's press: nothing to stop on its account; it casts.
        let mut f = frame(true);
        with_ghost_wolf_up(&mut f, false);
        with_the_cursor_up(&mut f);
        run(&mut f, r#"CastSpellByName("Ghost Wolf")"#);
        assert_eq!(
            casts(&f).iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![GHOST_WOLF]
        );
    }

    // ── A cast inside the call (`0x4b4ab0` → `0x4b3300` → `0x6e5a90` → `0x6e4b60` → `0x6e5250`) ──

    /// Mark of the Wild: no `Targets`, `EffectImplicitTargetA` 21, so the arm's word is the assist
    /// bit 0x100 (spell_template 1126).
    const MOTW: u32 = 1126;
    /// Resurrection: `Targets` 0x8000, the dead friendly player's word, implicit target 0
    /// (spell_template 2006).
    const RESURRECTION: u32 = 2006;

    /// Book `name` as slot 1 over `display`.
    fn with_book_spell(
        f: &mut Frame,
        spell_id: u32,
        name: &str,
        display: benilla_formats::SpellDisplay,
    ) {
        let world = f.app.world_mut();
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(spell_id, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world
            .non_send_resource_mut::<UiScript>()
            .set_spellbook(SpellBookState {
                tabs: vec![SpellTabView {
                    name: "Book".into(),
                    texture: None,
                    offset: 0,
                    num_spells: 1,
                }],
                slots: vec![SpellSlotView {
                    spell_id,
                    name: name.into(),
                    ..Default::default()
                }],
            });
    }

    /// `party1` (`NEW`) a friendly player-controlled player, alive or dead.
    fn with_friendly_party1(f: &mut Frame, alive: bool) {
        let world = f.app.world_mut();
        let new = world.resource::<GuidIndex>().0[&NEW];
        world
            .entity_mut(new)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, if alive { 100 } else { 0 }),
                (28, 100),
                (FLAGS, 0x8),
                (TEMPLATE, FRIEND),
            ])));
    }

    /// Run `lua` as a key's body with the app's host lent, as the bindings pass runs one: every
    /// call applies inside its verb, and no drain runs after the chunk.
    fn run_hosted(f: &mut Frame, lua: &str) {
        let lua = lua.to_string();
        f.app
            .world_mut()
            .run_system_once(
                move |mut script: NonSendMut<UiScript>, mut host: ScriptCallHost| {
                    script
                        .hosted(&mut host, |s| s.run_binding(&lua))
                        .expect("the chunk runs");
                },
            )
            .expect("the hosted chunk runs as a one-shot system");
    }

    /// A global the chunk left, as a number; `None` for nil.
    fn global(f: &mut Frame, name: &str) -> Option<i64> {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Option<i64>>(&format!("return {name}"))
            .expect("the global reads")
    }

    /// FuBar_CorkFu's buff (`BuffTemplate.lua:156-160`): the cast arms inside `CastSpellByName`, so
    /// the next line's `SpellIsTargeting()` (`0x6e6cd0` → `0x6e48a0`) is 1, `SpellTargetUnit` binds
    /// the pending spell (`0x6e5b10` → `0x6e5b40`) and sends it, and the second test reads nil, all
    /// before the chunk returns.
    #[test]
    fn a_cast_arms_inside_the_call_and_the_next_line_binds_it() {
        let mut f = frame(true);
        // An enemy selected: the arm's candidate fails the assist bit, `autoSelfCast` is off, so
        // the cursor comes up (`0x6e50c8`).
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        run_hosted(
            &mut f,
            r#"CastSpellByName("Mark of the Wild")
            FIRST = SpellIsTargeting()
            if SpellIsTargeting() then SpellTargetUnit("party1") end
            SECOND = SpellIsTargeting()
            if SpellIsTargeting() then SpellStopTargeting() end"#,
        );
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(
            global(&mut f, "SECOND"),
            None,
            "bound and sent by the line after"
        );
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
        assert!(!cursor_is_up(&f));
        assert_eq!(selected(&f), Some(OLD), "the bind leaves the selection");
    }

    /// SmartRes (`SmartRes.lua:173-177`): `SpellTargetUnit(corpse)` right after the cast binds the
    /// dead groupmate, so `not SpellIsTargeting()` tells it the resurrection went out.
    #[test]
    fn a_resurrection_binds_the_dead_unit_named_on_the_next_line() {
        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, false);
        with_book_spell(
            &mut f,
            RESURRECTION,
            "Resurrection",
            benilla_formats::SpellDisplay {
                targets: 0x8000,
                ..Default::default()
            },
        );
        run_hosted(
            &mut f,
            r#"CastSpellByName("Resurrection", 0)
            ARMED = SpellIsTargeting()
            SpellTargetUnit("party1")
            if not SpellIsTargeting() then SENT = 1 end"#,
        );
        assert_eq!(global(&mut f, "ARMED"), Some(1));
        assert_eq!(global(&mut f, "SENT"), Some(1));
        assert_eq!(casts(&f), vec![(RESURRECTION, Some(NEW))]);
    }

    /// A cast `TryCastSpell` refuses never reaches the arm (`0x6e5038`), so the next line reads no
    /// targeting: a dead caster (`0x6e4c49`, 0x13) and the gate's tag 2 outside an input event, a
    /// macro's line (`0x6e4bc1`, `0x494a92`). The client's own call, alive, arms.
    #[test]
    fn a_refused_cast_does_not_arm() {
        const CAST: &str = r#"CastSpellByName("Mark of the Wild") ARMED = SpellIsTargeting()"#;
        const MACRO_LINE: &str =
            r#"RunScript('CastSpellByName("Mark of the Wild")') ARMED = SpellIsTargeting()"#;
        for (dead, lua, armed) in [
            (false, CAST, true),
            (true, CAST, false),
            (false, MACRO_LINE, false),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
            with_book_spell(
                &mut f,
                MOTW,
                "Mark of the Wild",
                benilla_formats::SpellDisplay {
                    implicit_target_a1: 21,
                    ..Default::default()
                },
            );
            if dead {
                let world = f.app.world_mut();
                let me = world.resource::<GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x19),
                        (HEALTH, 0),
                        (28, 100),
                        (FLAGS, 0x8),
                        (TEMPLATE, OURS),
                    ]),
                ));
            }
            let chunk = lua.to_string();
            f.app
                .world_mut()
                .run_system_once(
                    move |mut script: NonSendMut<UiScript>, mut host: ScriptCallHost| {
                        // Outside any input event: the macro line's gate refuses, and the client's
                        // own chunk passes it.
                        script
                            .hosted(&mut host, |s| s.run(&chunk))
                            .expect("the chunk runs");
                    },
                )
                .unwrap();
            assert_eq!(
                global(&mut f, "ARMED"),
                armed.then_some(1),
                "{lua}, dead {dead}"
            );
            assert_eq!(cursor_is_up(&f), armed, "{lua}, dead {dead}");
            assert_eq!(casts(&f), vec![], "{lua}, dead {dead}");
            // "You are dead" for the rung, and with nothing in flight its event (`0x6e224a`); the
            // gate refuses silently.
            let want: Vec<&str> = if dead {
                vec!["SPELL_FAILED_CASTER_DEAD", "SPELLCAST_FAILED"]
            } else {
                vec![]
            };
            assert_eq!(reports(&mut f), want, "{lua}, dead {dead}");
        }
    }

    /// `CURRENT_SPELL_CAST_CHANGED` (`0x4b3250`) is signalled inside the call that armed the
    /// cursor, three times: the slot's push (`0x6e5026`), the arm (`0x6e5333`) and the attempt's
    /// end (`0x6e51e3`); and inside the one that bound it, twice: `BindTarget`'s merge
    /// (`0x6e60cb`) and the send (`0x6e589d`). The next frame's feed signals neither again.
    #[test]
    fn current_spell_cast_changed_fires_inside_the_call_and_not_again_from_the_feed() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
                f:SetScript("OnEvent", function() FIRED = (FIRED or 0) + 1 end)"#,
            )
            .unwrap();
        let feed = |f: &mut Frame| {
            let world = f.app.world_mut();
            world
                .run_system_once(crate::spell::targeting::feed_targeting_to_vm)
                .unwrap();
            world
                .run_system_once(crate::ui_cast::feed_cast_reports)
                .unwrap();
        };
        run_hosted(
            &mut f,
            r#"CastSpellByName("Mark of the Wild") SEEN = FIRED"#,
        );
        assert_eq!(global(&mut f, "SEEN"), Some(3), "inside the arming call");
        feed(&mut f);
        assert_eq!(global(&mut f, "FIRED"), Some(3), "not again from the feed");
        run_hosted(&mut f, r#"SpellTargetUnit("party1") SEEN = FIRED"#);
        assert_eq!(global(&mut f, "SEEN"), Some(5), "inside the binding call");
        feed(&mut f);
        assert_eq!(global(&mut f, "FIRED"), Some(5), "not again from the feed");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// A key's body runs inside `CBindings::ExecuteBinding` (`0x4b7990`) with its input event up
    /// (`0x4b7b02`), and the bindings pass lends the host around it: SmartRes on a key casts, binds
    /// and reads the result inside the body, with no drain after it.
    #[test]
    fn a_bound_keys_body_casts_and_binds_inside_the_bindings_pass() {
        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, false);
        with_book_spell(
            &mut f,
            RESURRECTION,
            "Resurrection",
            benilla_formats::SpellDisplay {
                targets: 0x8000,
                ..Default::default()
            },
        );
        let world = f.app.world_mut();
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(
                "bind F SMARTRES\r\n",
            ));
            script.load_binding_set(1);
            script.register_bindings(
                &benilla_ui::bindings_xml::parse(
                    r#"<Bindings><Binding name="SMARTRES">
                        CastSpellByName("Resurrection", 0)
                        ARMED = SpellIsTargeting()
                        SpellTargetUnit("party1")
                        if not SpellIsTargeting() then SENT = 1 end
                    </Binding></Bindings>"#,
                )
                .expect("well-formed"),
            );
        }
        world.write_message(bevy::input::keyboard::KeyboardInput {
            key_code: KeyCode::KeyF,
            logical_key: bevy::input::keyboard::Key::Character("f".into()),
            state: bevy::input::ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        crate::bindings::dispatch_once(world);
        assert_eq!(global(&mut f, "ARMED"), Some(1));
        assert_eq!(global(&mut f, "SENT"), Some(1));
        assert_eq!(casts(&f), vec![(RESURRECTION, Some(NEW))]);
    }

    /// FuBar_CorkFu casts from its plugin's click: a button's `OnClick` run by the mouse raises the
    /// click event (`0x779275`), and the input pass lends the host around it, so the cast arms
    /// and the next line binds it inside the handler.
    #[test]
    fn a_clicks_handler_casts_and_binds_inside_the_input_pass() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        let world = f.app.world_mut();
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.set_screen_size(1024.0, 768.0);
            script
                .run(
                    r#"local b = CreateFrame("Button", "CorkButton")
                    b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(1024) b:SetHeight(768)
                    b:SetScript("OnClick", function()
                        CastSpellByName("Mark of the Wild")
                        FIRST = SpellIsTargeting()
                        if SpellIsTargeting() then SpellTargetUnit("party1") end
                        SECOND = SpellIsTargeting()
                    end)"#,
                )
                .expect("the button builds");
            script.resolve();
        }
        let mut window = Window {
            resolution: UVec2::new(1024, 768).into(),
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
        world.spawn((window, bevy::window::PrimaryWindow));
        for down in [true, false] {
            let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
            buttons.clear();
            if down {
                buttons.press(MouseButton::Left);
            } else {
                buttons.release(MouseButton::Left);
            }
            crate::ui_script::feed_input_once(world);
        }
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }
}
