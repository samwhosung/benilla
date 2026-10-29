//! The interface events of the field watches and the packet handlers that signal unit events
//! ([`crate::field_watch`]): the notice lane, whose notices fire their callbacks' signals in the
//! packets' order among the other lanes ([`crate::lanes`]), after every snapshot those events'
//! handlers read is pushed ([`FieldEventPushes`]) and the unit tokens are seated
//! ([`crate::ui_aura::UnitGuidPush`]), as the update handler's notify pass follows its two write
//! passes over every block of the packet (`0x465277` after `0x46520e`, `0x465222`). The unit
//! tokens' views the drain then steps back to each packet's state ([`super::views`]).

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};
use benilla_ui::strings::Arg;

use crate::field_watch::{FieldNotice, NoticeLane};
use crate::lanes::Until;

/// The feeds that push what a field event's handlers read: the player's inventory and ammo. The
/// lanes' drain runs after all of them, as the update handler writes every block of its packet
/// before its notify pass (`0x46520e`, `0x465222`, then `0x46522d`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FieldEventPushes;

/// What a field notice's firing reads.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct NoticeFiring<'w, 's> {
    sink: crate::ui_action::MessageSink<'w>,
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
    // The form table the bonus bar's offset reads (`0x4e4fc0`), and the player's store as of the
    // drain's place, whose form byte it pages by.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    player: Query<'w, 's, Entity, With<crate::net::SelfPlayer>>,
    views: super::views::ViewStores<'w, 's>,
    // The skill value watch's line reads the line's name and race-class row (`0x5de2f2`,
    // `0x5de340`).
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    // The aura watcher's cache and the store its packet left.
    auras: crate::ui_aura::PlayerAuraWatch<'w, 's>,
    // `SetPetGuid`'s trainer refresh, whose close needs the latch: a world turn after the notice.
    commands: Commands<'w, 's>,
    // `SetPetGuid`'s unit lookup of the new guid (`0x4bc8e4`).
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    // A callback's mark of the portrait list (`0x524e10`).
    marks: Option<ResMut<'w, crate::portrait::PortraitMarks>>,
    // The happiness watch's tables, which its run reads the bucket through.
    pet_stats: Option<Res<'w, crate::ui_pet_stats::PetStatTables>>,
    // The explored watch's setter rebuilds the landmark list the bits gate.
    landmarks: crate::ui_world_map::LandmarkReads<'w, 's>,
}

impl NoticeFiring<'_, '_> {
    /// One notice's signals; whether it left a world turn the lane's firing must yield to, so the
    /// turn runs before the next notice.
    fn fire(&mut self, script: &mut UiScript, notice: FieldNotice, trainer: Option<u64>) -> bool {
        if let FieldNotice::PetGuidSet { changed, guid } = notice {
            return self.pet_guid_set(script, changed, guid, trainer);
        }
        if let FieldNotice::PortraitMark { guid } = notice {
            if let Some(marks) = self.marks.as_mut() {
                marks.mark(guid);
            }
            return false;
        }
        if notice == FieldNotice::PlayerAuras {
            self.auras.run(script);
            return false;
        }
        if notice == FieldNotice::QuestLog {
            // The slots are read where the list is built, off the store as the packet left it.
            script.quest_log_changed();
            return false;
        }
        if let FieldNotice::MirrorTimer(edge) = notice {
            crate::ui_mirror::fire_edge(script, edge, self.spells.as_deref());
            return false;
        }
        if let FieldNotice::Tutorial { id } = notice {
            // `0x4b5390` with no delay, inside the watcher's run.
            let published = self.tutorials.as_ref().and_then(|t| t.lock().fire_now(id));
            if let Some(published) = published {
                crate::tutorial::fire_published(script, &self.sink.sounds, published);
            }
            return false;
        }
        // A bridge watch, so only a unit with token flags carries it (`0x60c4a0`).
        if let FieldNotice::Happiness { guid, old, new } = notice {
            if script.unit_has_field_watchers(guid)
                && crate::ui_pet_stats::happiness_moved(self.pet_stats.as_deref(), old, new)
            {
                signal_unit(script, guid, crate::field_watch::HAPPINESS_BRIDGE);
            }
            return false;
        }
        if let FieldNotice::Drunk { guid, old, new } = notice {
            drunk(script, &mut self.sink, guid, old, new);
            return false;
        }
        if notice == FieldNotice::ExploredZones {
            // The bytes as the watcher's packet left them, as the shapeshift offset reads the
            // form byte.
            let explored = self
                .player
                .iter()
                .next()
                .and_then(|e| self.views.get(e).ok())
                .map(|store| crate::ui_world_map::explored_of(&store.0));
            if let Some(explored) = explored {
                self.landmarks.explored_zones_changed(script, explored);
            }
            return false;
        }
        if notice == FieldNotice::SkillLines {
            script.rebuild_skills();
            return false;
        }
        if notice == FieldNotice::SkillMax {
            script.rebuild_trade_skill(&mut |_| {});
            script.rebuild_skills();
            return false;
        }
        if let FieldNotice::SkillValue {
            skill_id,
            old,
            shown,
        } = notice
        {
            if shown != u32::from(old) {
                script.rebuild_skills();
                self.skill_line(script, skill_id, old, shown);
            }
            return false;
        }
        if notice == FieldNotice::Professions {
            script.rebuild_trade_skill(&mut |_| {});
            return false;
        }
        if notice == FieldNotice::PetMacros {
            crate::ui_macro::derive_pet_followers(script);
            return false;
        }
        if notice == FieldNotice::Shapeshift {
            // `0x5de016` → `0x4b3250`: `0x4e5bf0`'s event, then its own (`0x4b325a`).
            script.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
            script.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
            // `0x5de01b` → `0x4e4fc0`: the offset stored, then the event, on every form change,
            // off the form byte as the watcher's packet left it.
            let me = self
                .player
                .iter()
                .next()
                .and_then(|e| self.views.get(e).ok());
            let offset = crate::ui_action::bonus_bar_offset(me, self.spells.as_deref());
            script.set_bonus_bar_offset(offset);
            script.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
            return false;
        }
        fire(script, &mut self.sink, notice);
        if let FieldNotice::Unit {
            guid,
            id: crate::field_watch::id::UNIT_HEALTH,
        } = notice
        {
            self.tooltip_health(script, guid);
        }
        false
    }
}

impl NoticeFiring<'_, '_> {
    /// The unit tooltip's own watcher on the health of the unit it shows (`0x529560`, which the
    /// render registers on the object, `0x52a0a8`), after the bridge's: `guid`'s health as its
    /// packet left it.
    fn tooltip_health(&self, script: &mut UiScript, guid: u64) {
        let store = self
            .index
            .as_ref()
            .and_then(|i| i.0.get(&guid))
            .and_then(|&e| self.views.get(e).ok());
        if let Some(store) = store {
            script.tooltip_unit_health(
                guid,
                store.0.unit_health().unwrap_or(0),
                store.0.unit_max_health().unwrap_or(0),
            );
        }
    }

    /// `SetPetGuid`'s signals in its order (`0x4bc847`-`0x4bc911`): on a change, `UNIT_PET`, then,
    /// with a pet trainer open and our player held, `TalkToTrainer` on it (`0x4bc8b2`), then, when
    /// the new guid resolves as a unit (`0x4bc8e4`) that `IsPlayerOrHunterPet` passes (`0x4bc8ef`),
    /// `PET_UI_CLOSE` (`0x4bc907`); then `PET_BAR_UPDATE`. `trainer` is the open pet trainer
    /// (`0x4bc860`-`0x4bc879`), whose close needs the latch, so with one the rest runs as a world
    /// turn after this notice.
    fn pet_guid_set(
        &mut self,
        script: &mut UiScript,
        changed: bool,
        guid: u64,
        trainer: Option<u64>,
    ) -> bool {
        if !changed {
            script.fire_event("PET_BAR_UPDATE", vec![]);
            return false;
        }
        script.fire_event("UNIT_PET", vec![super::player_token()]);
        let player = self.me.iter().next();
        let closes = guid != 0
            && self
                .index
                .as_ref()
                .and_then(|i| i.0.get(&guid))
                .and_then(|&e| self.stores.get(e).ok())
                .is_some_and(|unit| crate::ui_pet::is_player_or_hunter_pet(unit, guid, player));
        let Some(trainer) = trainer.filter(|_| player.is_some()) else {
            if closes {
                script.fire_event("PET_UI_CLOSE", vec![]);
            }
            script.fire_event("PET_BAR_UPDATE", vec![]);
            return false;
        };
        // At the notice's place: what the refresh's close signals nests under it.
        let at = crate::lanes::firing_now();
        self.commands.queue(move |world: &mut World| {
            let refresh = |world: &mut World| {
                crate::ui_trainer::talk_to_trainer(world, trainer);
                crate::ui_session::fire(world, |script| {
                    if closes {
                        script.fire_event("PET_UI_CLOSE", vec![]);
                    }
                    script.fire_event("PET_BAR_UPDATE", vec![]);
                });
            };
            match at {
                Some(at) => {
                    crate::lanes::firing(at, || refresh(world));
                }
                None => refresh(world),
            }
        });
        true
    }

    /// `0x5de180`'s line after its rebuild (`0x5de345`-`0x5de41c`), for a line with a `SkillLine`
    /// row and a race-class row without flags `0x402` (`0x5de34a`-`0x5de35f`): over an old value
    /// of 0 `ERR_SKILL_GAINED_S` (`0x5de378`), else, on a shown value above 0,
    /// `ERR_SKILL_UP_SI` with it (`0x5de417`), each a `CHAT_MSG_SKILL` line shown in place
    /// (`0x496720`, the catalog rows' chat type 23).
    fn skill_line(&mut self, script: &mut UiScript, skill_id: u16, old: u16, shown: u32) {
        let line_id = u32::from(skill_id);
        let Some(catalog) = self.skill_lines.as_ref().map(|l| &l.catalog) else {
            return;
        };
        let Some(name) = catalog.line(line_id).map(|l| l.name.clone()) else {
            return;
        };
        let (race, class) = self.me.iter().next().map_or((0, 0), |s| {
            (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0))
        });
        if !catalog.announces_skill_ups(line_id, race, class) {
            return;
        }
        let (key, args): (&str, Vec<Arg<'_>>) = if old == 0 {
            ("ERR_SKILL_GAINED_S", vec![Arg::S(&name)])
        } else if shown > 0 {
            (
                "ERR_SKILL_UP_SI",
                vec![Arg::S(&name), Arg::D(i64::from(shown))],
            )
        } else {
            return;
        };
        let Some(template) = script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        let event = crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::ChatEventKind::Skill,
            benilla_ui::strings::fill(&template, &args),
        );
        self.sink.chat.show_now(script, &event);
    }
}

/// The notice lane's head ([`crate::lanes`]).
fn peek(world: &World) -> Option<crate::lanes::Stamp> {
    world.get_resource::<NoticeLane>()?.peek()
}

/// The VM and the host, with the open trainer `SetPetGuid` reads between notices.
type NoticeVm<'w, 's> =
    crate::script_calls::VmWith<'w, 's, Option<Res<'static, crate::ui_trainer::TrainerOpen>>>;

/// The notices `until` admits, in stamp order, each one's signals with the host lent; the lane's
/// firing stops after a notice whose firing stamped anything, so the drain takes what it pushed
/// next, or left a world turn, which runs as the firing returns. With no VM there is nobody to
/// hear them, and they are dropped, as below.
fn fire_lane(vm: &mut NoticeVm, lane: &mut NoticeLane, firing: &mut NoticeFiring, until: Until) {
    if vm.parts().0.is_none() {
        lane.clear();
        return;
    }
    while let Some((stamp, notice)) = lane.pop(until) {
        // An engine write, with no signal: the host's autorun bit.
        if notice == FieldNotice::AutorunClear {
            vm.engine_autorun_clear();
            continue;
        }
        if notice == FieldNotice::UsableRefresh {
            let ((), moved) = crate::lanes::firing(stamp, || vm.engine_usable_refresh());
            if moved {
                break;
            }
            continue;
        }
        if let FieldNotice::SpellSlots { spell } = notice {
            // Each slot resolved as the walk reaches it (`0x4e5df5`), so a listener's write to a
            // later slot is seen.
            let ((), moved) = crate::lanes::firing(stamp, || {
                for action in 1..=120 {
                    if !vm.engine_spell_slot_notified(action, spell) {
                        continue;
                    }
                    vm.run(|script| {
                        if let Some(script) = script {
                            script.fire_event(
                                "ACTIONBAR_SLOT_CHANGED",
                                vec![ScriptValue::Int(i64::from(action))],
                            );
                        }
                    });
                }
            });
            if moved {
                break;
            }
            continue;
        }
        if let FieldNotice::ActionSlotChanged { action } = notice {
            let ((), moved) = crate::lanes::firing(stamp, || {
                vm.engine_slot_written(action);
                vm.run(|script| {
                    if let Some(script) = script {
                        script.fire_event(
                            "ACTIONBAR_SLOT_CHANGED",
                            vec![ScriptValue::Int(i64::from(action))],
                        );
                    }
                })
            });
            if moved {
                break;
            }
            continue;
        }
        let trainer = match &notice {
            FieldNotice::PetGuidSet { changed: true, .. } => {
                crate::ui_trainer::open_pet_trainer(vm.own().as_deref())
            }
            _ => None,
        };
        let (yielded, moved) = vm.run(|script| match script {
            Some(script) => crate::lanes::firing(stamp, || firing.fire(script, notice, trainer)),
            None => (false, false),
        });
        if moved || yielded {
            break;
        }
    }
}

/// The notice lane's `fire_until`.
fn fire_notices(
    In(until): In<Until>,
    mut vm: NoticeVm,
    mut lane: ResMut<NoticeLane>,
    mut firing: NoticeFiring,
) {
    fire_lane(&mut vm, &mut lane, &mut firing, until);
}

/// `PLAYER_LEVEL_UP` (`0x5e413e`, event `0xc7`), its nine values pushed as `%d`: the message's
/// level, health and first power, 1 for a level of at least 10 (`0x5e407c`-`0x5e4081`), and its
/// five stats, the arguments `ChatFrame.lua`'s level-up block prints.
fn level_up(script: &mut UiScript, info: &benilla_protocol::messages::LevelUpInfo) {
    let level = info.level as i32;
    let int = |v: i32| ScriptValue::Int(i64::from(v));
    let [str, agi, sta, int_, spi] = info.stats;
    script.fire_event(
        "PLAYER_LEVEL_UP",
        vec![
            int(level),
            int(info.health),
            int(info.powers[0]),
            int(i32::from(level >= 10)),
            int(str),
            int(agi),
            int(sta),
            int(int_),
            int(spi),
        ],
    );
}

/// `SMSG_LEVELUP_INFO`'s arm of the notice handler `0x5e38c0`. Only while the active player is
/// held (`0x5e4054`-`0x5e4071`): each level tutorial the message's level reaches, in the arm's
/// order (`0x5e4086`-`0x5e410e`), then `PLAYER_LEVEL_UP` (`0x5e413e`), at the packet's place among
/// the other lanes. Nothing else signals the event: a level written to the descriptor does not.
pub(super) fn on_level_up(
    In(ev): In<benilla_protocol::SessionEvent>,
    me: Query<(), With<crate::net::SelfPlayer>>,
    mut lane: ResMut<NoticeLane>,
) {
    let benilla_protocol::SessionEvent::LevelUp(info) = ev else {
        return;
    };
    if me.is_empty() {
        return;
    }
    // The arm compares signed (`0x5e4089 jl`).
    let level = info.level as i32;
    lane.write_batch(
        crate::tutorial::LEVEL_TRIGGERS
            .iter()
            .filter(|&&(threshold, _)| level >= threshold)
            .map(|&(_, id)| FieldNotice::Tutorial { id }),
    );
    lane.write(FieldNotice::LevelUp(info));
}

/// Every notice waiting, fired alone, for a test that drives the notices without the chat lane.
#[cfg(test)]
pub(crate) fn fire_field_notices(
    mut vm: NoticeVm,
    mut lane: ResMut<NoticeLane>,
    mut firing: NoticeFiring,
) {
    let until = Until::all();
    while lane.peek().is_some_and(|s| until.admits(s)) {
        fire_lane(&mut vm, &mut lane, &mut firing, until);
    }
}

/// `0x5e2a90`'s run on the player `guid` whose drunk byte moved from `old` to `new`. On the active
/// player, `UNIT_LEVEL` for `"target"` when it can attack its target (`0x5e2afb`-`0x5e2b38`), as
/// `UnitLevel` lowers such a unit's level by the player's drunkenness (`0x518096`). Then each
/// message as a system chat line (`0x49a870`, type 0xa, `0x5e2c5d`): `DRUNK_MESSAGE_SELF%d` for
/// the active player (`0x5e2bf8`), else `DRUNK_MESSAGE_OTHER%d` filled with the player's name
/// (`0x5e2c09`, `0x609210`), the text the key names or empty (`0x703bf0`). The active player's
/// name-plate refresh and camera field-of-view bias (`0x605550`, `0x511250`) are not built.
fn drunk(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    guid: u64,
    old: u8,
    new: u8,
) {
    let me = script.is_active_player(guid);
    if me && script.player_can_attack_target() {
        script.fire_event("UNIT_LEVEL", vec![ScriptValue::Str("target".to_string())]);
    }
    // The name cache as the watcher runs, through the lent host (`0x609210`).
    let name = (!me).then(|| {
        benilla_ui::script::live::ask(script.lua(), &benilla_ui::script::PlayerNameQuery(guid))
            .flatten()
            .unwrap_or_else(|| script.unknown_object_text())
    });
    let lines: Vec<_> = drunk_messages(old, new)
        .into_iter()
        .map(|k| {
            let key = if me {
                format!("DRUNK_MESSAGE_SELF{k}")
            } else {
                format!("DRUNK_MESSAGE_OTHER{k}")
            };
            let template = script
                .lua()
                .globals()
                .get::<String>(key)
                .unwrap_or_default();
            let args: Vec<benilla_ui::strings::Arg<'_>> = name
                .as_deref()
                .map(benilla_ui::strings::Arg::S)
                .into_iter()
                .collect();
            crate::ui_action::Shown::unkeyed(
                benilla_ui::messages::MsgKind::Chat,
                benilla_ui::strings::fill(&template, &args),
            )
        })
        .collect::<Vec<_>>();
    // Each to the line function inside the watcher's run (`0x5e2c68` → `0x49a870`).
    for line in lines {
        crate::ui_action::show_now(script, sink, "ui_unit", line);
    }
}

/// The notice lane and its drop: [`drop_field_notices`] under the exact complement of the
/// drain's gate, so a notice is fired or dropped the frame it is written, never left to age.
pub(super) fn add_notice_lane(app: &mut App) {
    use crate::lanes::LaneApp;
    use crate::net::handlers::NetHandlerApp;
    app.init_resource::<NoticeLane>()
        .net_handler(benilla_protocol::SessionEventKind::LevelUp, on_level_up)
        .add_lane("field notices", peek, fire_notices, None)
        .add_systems(
            Update,
            drop_field_notices
                .in_set(crate::ui_script::UiFeed)
                .before(super::UnitFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// The notices written while no in-game interface is up, taken unfired, on purpose: there is no VM
/// to hear them, and the one that comes up later must not hear changes from before it. The
/// reference's interface exists before the world it watches, so it has no such window; the
/// snapshots the VM reads are pushed whole when it comes up, as they are at a `/reload`.
///
/// A follow stop's autorun clear goes too: with no VM no call touches the bit, so the steer's
/// release takes it clear ([`crate::player::CallAutorun`]), as it does where [`fire_lane`] drops
/// the lane for want of a VM.
pub(super) fn drop_field_notices(mut lane: ResMut<NoticeLane>) {
    lane.clear();
}

/// One callback's signals.
pub(crate) fn fire(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    notice: FieldNotice,
) {
    match notice {
        // A unit with no token flags carries no bridge (`0x60c4a0`).
        FieldNotice::Unit { guid, id } => {
            if script.unit_has_field_watchers(guid) {
                signal_unit(script, guid, id);
            }
        }
        FieldNotice::Signal { guid, id } => signal_unit(script, guid, id),
        FieldNotice::Player { id } => fire_player(script, id),
        FieldNotice::RestState { old, new } => {
            // `0x5de4e0`: the signal, then the line for the new state.
            script.fire_event("UPDATE_EXHAUSTION", vec![]);
            // Through `DisplayError` (`0x5de52c`), whose chat row shows it at once (`0x496842`).
            let line = super::rest_state_message(old, new)
                .and_then(|key| crate::ui_action::keyed_line(script, key));
            if let Some(line) = line {
                crate::ui_action::show_now(script, sink, "ui_unit", line);
            }
        }
        FieldNotice::CombatSkill => {
            // `0x5ed990`, in its order (`0x5ed99f`, `0x5ed9b3`, `0x5ed9c7`).
            script.fire_event("UNIT_ATTACK", vec![super::player_token()]);
            script.fire_event("UNIT_DEFENSE", vec![super::player_token()]);
            script.fire_event("UNIT_RANGED_ATTACK_POWER", vec![super::player_token()]);
        }
        FieldNotice::Weapon { slot } => {
            let event = match slot {
                // `0x5ed970` (`0x5deed9`, `0x5deee4`).
                15 | 16 => "UNIT_ATTACK",
                // `0x5deecb`.
                17 => "UNIT_RANGED_ATTACK_POWER",
                _ => return,
            };
            script.fire_event(event, vec![super::player_token()]);
            // The main hand and the ranged slot then signal the book (`0x5deeed`), which shows
            // their weapons' icons; the off hand returns first (`0x5deede`).
            if slot != 16 {
                script.fire_event("SPELLS_CHANGED", vec![]);
            }
        }
        FieldNotice::Global { id } => {
            if let Some(global) = global_event(id) {
                script.fire_event(global, vec![]);
            }
        }
        // `0x5ddffa`.
        FieldNotice::ComboPoints => script.fire_event("PLAYER_COMBO_POINTS", vec![]),
        // `SignalEvent2(0x1fe, "%d", tab + 1)` (`0x4b2b92`).
        FieldNotice::LearnedInTab { tab } => {
            script.fire_event("LEARNED_SPELL_IN_TAB", vec![ScriptValue::Int(tab)])
        }
        // `0x5dfa3f`: the cost as a number, which the dialog's `MoneyFrame_Update` takes.
        FieldNotice::TalentWipe { cost } => script.fire_event(
            "CONFIRM_TALENT_WIPE",
            vec![ScriptValue::Int(i64::from(cost))],
        ),
        // `0x4c7098`, the two changes as numbers: `ChatFrame.lua:1326` tests `arg2 > 0`.
        FieldNotice::CharacterPoints { talent, profession } => script.fire_event(
            "CHARACTER_POINTS_CHANGED",
            vec![
                ScriptValue::Int(i64::from(talent)),
                ScriptValue::Int(i64::from(profession)),
            ],
        ),
        FieldNotice::LevelUp(info) => level_up(script, &info),
        FieldNotice::PlayerControl { in_control } => {
            script.set_player_control(in_control);
            if in_control {
                script.fire_event("PLAYER_CONTROL_GAINED", vec![]);
            } else {
                script.fire_event("PLAYER_CONTROL_LOST", vec![]);
            }
        }
        // Caught by [`fire_lane`] before it reaches here: the refresh and the slot write's pair
        // run on the host's action states
        // ([`crate::script_calls::ScriptCallHost::engine_usable_refresh`]).
        FieldNotice::UsableRefresh
        | FieldNotice::ActionSlotChanged { .. }
        | FieldNotice::SpellSlots { .. } => {}
        // Fired by [`NoticeFiring::fire`], which holds the slots, the banks, the names, the form
        // table, the spell catalog, the aura cache, the skill lines and the pet's stat tables.
        FieldNotice::QuestLog
        | FieldNotice::ExploredZones
        | FieldNotice::Tutorial { .. }
        | FieldNotice::MirrorTimer(_)
        | FieldNotice::Drunk { .. }
        | FieldNotice::Happiness { .. }
        | FieldNotice::Shapeshift
        | FieldNotice::PlayerAuras
        | FieldNotice::SkillLines
        | FieldNotice::SkillMax
        | FieldNotice::SkillValue { .. }
        | FieldNotice::Professions
        | FieldNotice::PortraitMark { .. }
        | FieldNotice::PetMacros
        | FieldNotice::PetGuidSet { .. } => {}
        // Put on the trainer lane by the packet's handler ([`crate::ui_trainer::TrainerPasses`]).
        FieldNotice::TrainerPass
        | FieldNotice::AutoMoveStop
        | FieldNotice::AutorunClear
        | FieldNotice::InputTick
        | FieldNotice::FarSightStop { .. } => {}
        // Run at its place by the packet's handler ([`crate::net::objects`]), its signal written
        // there.
        FieldNotice::AreaSpiritHealerClear | FieldNotice::QuestAccepted { .. } => {}
        // `%d%d` (`0x4018d5`): the two counts as numbers, which `ChatFrame_DisplayTimePlayed`
        // prints (`ChatFrame.lua:1279`).
        FieldNotice::PlayedTime { total, level } => script.fire_event(
            "TIME_PLAYED_MSG",
            vec![
                ScriptValue::Int(i64::from(total)),
                ScriptValue::Int(i64::from(level)),
            ],
        ),
        // `%d` (`0x48f408`): the stock `UIParent.lua` hides the resurrect offers and prints
        // `DEATH_CORPSE_SKINNED`.
        FieldNotice::Skinned { free_repop } => script.fire_event(
            "PLAYER_SKINNED",
            vec![ScriptValue::Int(i64::from(free_repop))],
        ),
        FieldNotice::MinimapPing(ping) => ping.fire(script),
        FieldNotice::Revived => {
            script.unlock_all_items();
            script.fire_event("PLAYER_ALIVE", vec![]);
        }
        FieldNotice::Line { key } => {
            if let Some(line) = crate::ui_action::keyed_line(script, key) {
                crate::ui_action::show_now(script, sink, "ui_unit", line);
            }
        }
        FieldNotice::PvpPreference { on } => {
            let (toast, verbose) = super::pvp_announcement(on);
            // The verbose sentence is no catalog row, so it goes `unkeyed` to chat:
            // `Shown::keyed`'s unknown-key fallback would show it red.
            let lines = [
                crate::ui_action::keyed_line(script, toast),
                script
                    .lua()
                    .globals()
                    .get::<String>(verbose)
                    .ok()
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, t)
                    }),
            ];
            for line in lines.into_iter().flatten() {
                crate::ui_action::show_now(script, sink, "ui_unit", line);
            }
        }
    }
}

/// The drunk thresholds, the floats at `0x80af68`.
const DRUNK_THRESHOLDS: [f32; 3] = [0.01, 0.5, 0.9];

/// The numbers of the drunk messages a move of the byte from `old` to `new` prints, in order, over
/// the drunk fractions of the two bytes (`0x5e2acc`-`0x5e2aeb`, `0x5e2b5e`-`0x5e2b7c`). Threshold
/// k, from the lowest (`0x5e2b6f`), is crossed rising when `old < t <= new` (`0x5e2b8b`,
/// `0x5e2b9b`) and falling when `new < t <= old` (`0x5e2bab`, `0x5e2bbf`); a rise prints k + 2 and
/// skips threshold k + 1 (`0x5e2bd0`-`0x5e2bdc`), a fall prints k + 1.
fn drunk_messages(old: u8, new: u8) -> Vec<u8> {
    let (old, new) = (
        crate::player::drunk::fraction(old),
        crate::player::drunk::fraction(new),
    );
    let mut out = Vec::new();
    let mut k = 0;
    while k < DRUNK_THRESHOLDS.len() {
        let t = DRUNK_THRESHOLDS[k];
        let rising = old < t && new >= t;
        let falling = old >= t && new < t;
        if rising || falling {
            if new > old {
                k += 1;
            }
            out.push(k as u8 + 1);
        }
        k += 1;
    }
    out
}

/// A unit notice's signals into a bare VM, for the stock-file tests that drive the frames off the
/// notices: [`fire`]'s two unit arms.
#[cfg(test)]
pub(crate) fn fire_unit_notice(script: &mut UiScript, notice: FieldNotice) {
    match notice {
        FieldNotice::Unit { guid, id } => {
            if script.unit_has_field_watchers(guid) {
                signal_unit(script, guid, id);
            }
        }
        FieldNotice::Signal { guid, id } => signal_unit(script, guid, id),
        other => panic!("not a unit notice: {other:?}"),
    }
}

/// `0x515e50(guid, id)`: the event `id` names, once per token naming the unit (`0x515c50`), `arg1`
/// the token (`0x515e7a`); nothing for a unit no token names.
fn signal_unit(script: &mut UiScript, guid: u64, id: u16) {
    let Some(event) = unit_event(id) else {
        return;
    };
    for token in script.unit_tokens_of(guid) {
        script.fire_event(event, vec![ScriptValue::Str(token)]);
    }
}

/// A [`FieldNotice::Player`]: the event its id names, `arg1 = "player"`.
fn fire_player(script: &mut UiScript, id: u16) {
    let Some(event) = player_event(id) else {
        return;
    };
    script.fire_event(event, vec![super::player_token()]);
}

/// The event-table name of an id a [`FieldNotice::Unit`] or [`FieldNotice::Signal`] carries, as
/// the static initializers write the table at `0x51ab30`-`0x51bb17`: every bridge id
/// ([`crate::field_watch::BRIDGE_IDS`]), the power events a power type reaches, and the fixed ids
/// of the other producers. An id past them, as a power type the server never sends would reach,
/// signals nothing here.
fn unit_event(id: u16) -> Option<&'static str> {
    Some(match id {
        0 | 2 => "UNIT_PET",
        16 => "UNIT_HEALTH",
        17 | 167 | 174 => "UNIT_MANA",
        18 => "UNIT_RAGE",
        19 => "UNIT_FOCUS",
        20 => "UNIT_ENERGY",
        21 => "UNIT_HAPPINESS",
        22 => "UNIT_MAXHEALTH",
        23 => "UNIT_MAXMANA",
        24 => "UNIT_MAXRAGE",
        25 => "UNIT_MAXFOCUS",
        26 => "UNIT_MAXENERGY",
        27 => "UNIT_MAXHAPPINESS",
        28 => "UNIT_LEVEL",
        29 => "UNIT_FACTION",
        30 => "UNIT_DISPLAYPOWER",
        40 => "UNIT_FLAGS",
        41 | 107 => "UNIT_AURA",
        120 | 121 => "UNIT_ATTACK_SPEED",
        122 | 165 | 166 => "UNIT_RANGEDDAMAGE",
        128..=131 => "UNIT_DAMAGE",
        132 => "UNIT_LOYALTY",
        135 | 136 => "UNIT_PET_EXPERIENCE",
        137 => "UNIT_DYNAMIC_FLAGS",
        143 => "UNIT_PET_TRAINING_POINTS",
        144..=148 => "UNIT_STATS",
        149..=155 => "UNIT_RESISTANCES",
        159..=161 => "UNIT_ATTACK_POWER",
        162..=164 => "UNIT_RANGED_ATTACK_POWER",
        183 => "UNIT_NAME_UPDATE",
        184 => "UNIT_PORTRAIT_UPDATE",
        185 => "UNIT_MODEL_CHANGED",
        186 => "UNIT_INVENTORY_CHANGED",
        187 => "UNIT_CLASSIFICATION_CHANGED",
        345 => "PLAYER_GUILD_UPDATE",
        407 => "PLAYER_FLAGS_CHANGED",
        522 => "UNIT_QUEST_LOG_CHANGED",
        _ => return None,
    })
}

/// The event-table name of an id a [`FieldNotice::Global`] carries.
fn global_event(id: u16) -> Option<&'static str> {
    use crate::field_watch::id;
    Some(match id {
        id::PLAYER_REGEN_DISABLED => "PLAYER_REGEN_DISABLED",
        id::PLAYER_REGEN_ENABLED => "PLAYER_REGEN_ENABLED",
        id::AUTOFOLLOW_END => "AUTOFOLLOW_END",
        id::PET_ATTACK_START => "PET_ATTACK_START",
        id::PET_ATTACK_STOP => "PET_ATTACK_STOP",
        id::PLAYER_GUILD_UPDATE => "PLAYER_GUILD_UPDATE",
        id::TABARD_CANSAVE_CHANGED => "TABARD_CANSAVE_CHANGED",
        id::MERCHANT_UPDATE => "MERCHANT_UPDATE",
        id::UPDATE_FACTION => "UPDATE_FACTION",
        id::PLAYER_UNGHOST => "PLAYER_UNGHOST",
        id::PLAYER_UPDATE_RESTING => "PLAYER_UPDATE_RESTING",
        id::PET_UI_UPDATE => "PET_UI_UPDATE",
        id::PLAYTIME_CHANGED => "PLAYTIME_CHANGED",
        id::PLAYER_MONEY => "PLAYER_MONEY",
        id::SPELLS_CHANGED => "SPELLS_CHANGED",
        id::UPDATE_SHAPESHIFT_FORMS => "UPDATE_SHAPESHIFT_FORMS",
        id::PET_BAR_UPDATE => "PET_BAR_UPDATE",
        id::AREA_SPIRIT_HEALER_IN_RANGE => "AREA_SPIRIT_HEALER_IN_RANGE",
        id::AREA_SPIRIT_HEALER_OUT_OF_RANGE => "AREA_SPIRIT_HEALER_OUT_OF_RANGE",
        id::PLAYER_CAMPING => "PLAYER_CAMPING",
        id::PLAYER_QUITING => "PLAYER_QUITING",
        id::LOGOUT_CANCEL => "LOGOUT_CANCEL",
        id::INSPECT_HONOR_UPDATE => "INSPECT_HONOR_UPDATE",
        id::PLAYERBANKBAGSLOTS_CHANGED => "PLAYERBANKBAGSLOTS_CHANGED",
        id::PLAYER_FARSIGHT_FOCUS_CHANGED => "PLAYER_FARSIGHT_FOCUS_CHANGED",
        id::LANGUAGE_LIST_CHANGED => "LANGUAGE_LIST_CHANGED",
        id::LOCALPLAYER_PET_RENAMED => "LOCALPLAYER_PET_RENAMED",
        id::UPDATE_LFG_TYPES => "UPDATE_LFG_TYPES",
        _ => return None,
    })
}

/// The event-table name of an id `0x5dd8a0` registers `0x5de4b0` with, or another callback
/// signals with `"player"` ([`FieldNotice::Player`]).
fn player_event(id: u16) -> Option<&'static str> {
    Some(match id {
        149 => "UNIT_RESISTANCES",
        186 => "UNIT_INVENTORY_CHANGED",
        189 => "PLAYER_XP_UPDATE",
        201 => "PLAYER_DAMAGE_DONE_MODS",
        347 => "PLAYERBANKSLOTS_CHANGED",
        372 => "UPDATE_FACTION",
        406 => "UPDATE_EXHAUSTION",
        523 => "PLAYER_PVP_KILLS_CHANGED",
        524 => "PLAYER_PVP_RANK_CHANGED",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x07;
    const PET: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0001;

    /// The event table, id to name (`reference/events.tsv`).
    fn event_table() -> std::collections::HashMap<u16, String> {
        let tsv = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/events.tsv");
        let text = std::fs::read_to_string(tsv).expect("reference/events.tsv");
        text.lines()
            .skip(1)
            .filter_map(|l| {
                let mut f = l.split('\t');
                Some((f.next()?.parse().ok()?, f.next()?.to_string()))
            })
            .collect()
    }

    /// Every id a unit notice can carry fires the event the reference's table names for it, and
    /// every bridge id names one.
    #[test]
    fn each_id_fires_the_event_the_table_names() {
        let table = event_table();
        for id in 0..=600 {
            if let Some(event) = unit_event(id) {
                assert_eq!(Some(event), table.get(&id).map(String::as_str), "id {id}");
            }
        }
        for id in crate::field_watch::BRIDGE_IDS {
            assert!(unit_event(id).is_some(), "bridge id {id}");
        }
        for id in 0..=600 {
            if let Some(event) = global_event(id) {
                assert_eq!(
                    Some(event),
                    table.get(&id).map(String::as_str),
                    "global id {id}"
                );
            }
        }
        for id in [149, 186, 189, 201, 372, 406, 523, 524] {
            assert_eq!(
                player_event(id),
                table.get(&id).map(String::as_str),
                "player id {id}"
            );
        }
    }

    /// The bridge watches every field below `0xb6` (`0x51bbb0`'s bound, `0x51bc1d`) whose row in
    /// the table is set.
    #[test]
    fn the_bridge_ids_are_every_named_field_of_the_table() {
        let want: std::collections::BTreeSet<u16> =
            event_table().into_keys().filter(|&id| id < 0xb6).collect();
        let have: std::collections::BTreeSet<u16> =
            crate::field_watch::BRIDGE_IDS.into_iter().collect();
        assert_eq!(have, want);
    }

    /// A world whose VM records every event the notices fire, with `guids` seated.
    fn world(guids: benilla_ui::script::UnitGuids) -> App {
        let mut app = App::new();
        app.init_resource::<crate::field_watch::NoticeLane>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::names::NameCache>();
        spy(&mut app, guids);
        app
    }

    /// [`world`] in the whole client's, whose notices fire with the call host lent, as a getter
    /// reads the name caches through it.
    fn hosted_world(guids: benilla_ui::script::UnitGuids) -> App {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        // The selection the getters read through the host.
        app.world_mut()
            .resource_mut::<crate::target::Selection>()
            .guid = (guids.target != 0).then_some(guids.target);
        spy(&mut app, guids);
        app
    }

    /// The VM that records every event the notices fire, with `guids` seated.
    fn spy(app: &mut App, guids: benilla_ui::script::UnitGuids) {
        let mut script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterAllEvents()
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        script.set_unit_guids(&guids);
        app.insert_non_send_resource(script);
    }

    fn fired(app: &mut App, notices: &[FieldNotice]) -> Vec<String> {
        app.world_mut()
            .resource_mut::<crate::field_watch::NoticeLane>()
            .write_batch(notices.iter().copied());
        app.world_mut().run_system_once(fire_field_notices).unwrap();
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        let out = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        out
    }

    fn seated() -> benilla_ui::script::UnitGuids {
        benilla_ui::script::UnitGuids {
            player: ME,
            pet: PET,
            target: ME,
            mouseover: MOB,
            ..Default::default()
        }
    }

    /// `SetPetGuid` with no pet trainer open signals inside its notice (`0x4bc847`-`0x4bc911`):
    /// `UNIT_PET` and `PET_BAR_UPDATE` ahead of the next notice's events, with no world turn.
    #[test]
    fn a_pet_change_with_no_trainer_signals_inside_its_notice() {
        let mut app = world(seated());
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::PetGuidSet {
                        changed: true,
                        guid: 0
                    },
                    FieldNotice::Global {
                        id: crate::field_watch::id::PLAYER_REGEN_DISABLED
                    }
                ]
            ),
            [
                "UNIT_PET player",
                "PET_BAR_UPDATE nil",
                "PLAYER_REGEN_DISABLED nil"
            ]
        );
    }

    /// `UNIT_INVENTORY_CHANGED` from its three kinds of producer: the visible-item bridge
    /// (`0x51bd70`) and the record set-up (`0x5d85fe`) for each token of the player, the
    /// ammunition and slot paths for `"player"` alone (`0x4c7110`, `0x4c72e0`, `0x4c8509`).
    #[test]
    fn the_inventory_event_reaches_each_token_or_the_player() {
        let mut app = world(seated());
        let id = crate::field_watch::id::UNIT_INVENTORY_CHANGED;
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Unit { guid: ME, id },
                    FieldNotice::Signal { guid: ME, id },
                    FieldNotice::Player { id }
                ]
            ),
            [
                "UNIT_INVENTORY_CHANGED player",
                "UNIT_INVENTORY_CHANGED target",
                "UNIT_INVENTORY_CHANGED player",
                "UNIT_INVENTORY_CHANGED target",
                "UNIT_INVENTORY_CHANGED player"
            ]
        );
    }

    /// The bridge fires once per token naming the unit, in the fan-out's order; a unit with no
    /// token flags fires nothing, the mouseover alone included. Another producer's signal needs
    /// no watcher of the unit's, so it reaches the mouseover's token too; an id past the table's
    /// unit events fires nothing.
    #[test]
    fn the_bridge_fires_once_per_token_naming_the_unit() {
        let mut app = world(seated());
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Unit { guid: ME, id: 144 },
                    FieldNotice::Unit { guid: ME, id: 145 },
                    FieldNotice::Unit { guid: PET, id: 128 },
                    FieldNotice::Unit { guid: MOB, id: 144 },
                    FieldNotice::Signal { guid: MOB, id: 183 },
                    FieldNotice::Signal { guid: PET, id: 167 },
                    FieldNotice::Signal { guid: PET, id: 300 },
                ]
            ),
            [
                "UNIT_STATS player",
                "UNIT_STATS target",
                "UNIT_STATS player",
                "UNIT_STATS target",
                "UNIT_DAMAGE pet",
                "UNIT_NAME_UPDATE mouseover",
                "UNIT_MANA pet",
            ]
        );
    }

    /// The portrait drain's pair for one unit (`0x524de4`, `0x524df1`): `UNIT_PORTRAIT_UPDATE` for
    /// every token naming it, then `UNIT_MODEL_CHANGED` for every token, each with the token as
    /// `arg1` (`0x515e7a`).
    #[test]
    fn the_portrait_pair_fires_each_event_for_every_token_in_turn() {
        let mut app = world(seated());
        let (portrait, model) = (
            crate::field_watch::id::UNIT_PORTRAIT_UPDATE,
            crate::field_watch::id::UNIT_MODEL_CHANGED,
        );
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Signal {
                        guid: ME,
                        id: portrait
                    },
                    FieldNotice::Signal {
                        guid: ME,
                        id: model
                    },
                ]
            ),
            [
                "UNIT_PORTRAIT_UPDATE player",
                "UNIT_PORTRAIT_UPDATE target",
                "UNIT_MODEL_CHANGED player",
                "UNIT_MODEL_CHANGED target",
            ]
        );
    }

    /// A mark of the portrait list fires nothing itself: it queues the GUID for the drain.
    #[test]
    fn a_portrait_mark_queues_the_guid_and_fires_nothing() {
        let mut app = world(seated());
        app.init_resource::<crate::portrait::PortraitMarks>();
        assert_eq!(
            fired(&mut app, &[FieldNotice::PortraitMark { guid: PET }]),
            Vec::<String>::new()
        );
        assert_eq!(
            app.world()
                .resource::<crate::portrait::PortraitMarks>()
                .pending(),
            [PET]
        );
    }

    /// Each callback's own signals, in the order the notices came.
    #[test]
    fn each_callback_fires_its_own_signals_in_notice_order() {
        let mut app = world(seated());
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::RestState { old: 1, new: 2 },
                    FieldNotice::Player { id: 189 },
                    FieldNotice::CombatSkill,
                    FieldNotice::Weapon { slot: 16 },
                    FieldNotice::Weapon { slot: 17 },
                    FieldNotice::Player { id: 406 },
                ]
            ),
            [
                "UPDATE_EXHAUSTION nil",
                "PLAYER_XP_UPDATE player",
                "UNIT_ATTACK player",
                "UNIT_DEFENSE player",
                "UNIT_RANGED_ATTACK_POWER player",
                "UNIT_ATTACK player",
                "UNIT_RANGED_ATTACK_POWER player",
                // The ranged slot's book signal (`0x5deeed`); the off hand's returns first.
                "SPELLS_CHANGED nil",
                "UPDATE_EXHAUSTION player",
            ]
        );
    }

    /// A global notice fires its event bare, once, whatever unit raised it; the PvP preference
    /// shows its toast for the new state, then its verbose sentence as a chat line, whose
    /// `CHAT_MSG_SYSTEM` fires inside the watcher's run, its line function called directly
    /// (`0x5eeb5d` → `0x49a870`).
    #[test]
    fn a_global_notice_fires_bare_and_the_pvp_preference_speaks() {
        let mut app = world(seated());
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"
                ERR_PVP_TOGGLE_ON = "on"
                ERR_PVP_TOGGLE_OFF = "off"
                PVP_TOGGLE_ON_VERBOSE = "on, verbose"
                PVP_TOGGLE_OFF_VERBOSE = "off, verbose"
            "#,
            )
            .unwrap();
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Global { id: 190 },
                    FieldNotice::PvpPreference { on: true },
                    FieldNotice::Global { id: 335 },
                    FieldNotice::PvpPreference { on: false },
                    FieldNotice::Global { id: 300 },
                    FieldNotice::Global { id: 401 },
                ]
            ),
            [
                "PLAYER_REGEN_DISABLED nil",
                "UI_INFO_MESSAGE on",
                "CHAT_MSG_SYSTEM on, verbose",
                "PET_ATTACK_STOP nil",
                "UI_INFO_MESSAGE off",
                "CHAT_MSG_SYSTEM off, verbose",
                "PLAYER_UNGHOST nil",
            ]
        );
        assert!(
            app.world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines()
                .is_empty(),
            "the lines fired in the watcher's run, not from the chat lane"
        );
    }

    /// `0x5e2a90`'s thresholds, the floats 0.01, 0.5 and 0.9 at `0x80af68` against
    /// `min(byte, 100) · 0.01` as a single (`0x5e2acc`-`0x5e2aeb`, `0x5e2b5e`-`0x5e2b7c`): a rise
    /// across threshold k prints message k + 2 and skips threshold k + 1 (`0x5e2bd5`-`0x5e2bdc`), a
    /// fall prints k + 1, in threshold order.
    #[test]
    fn the_drunk_messages_follow_the_thresholds_crossed() {
        for (old, new, want) in [
            (0, 1, &[2][..]),
            (1, 0, &[1]),
            (0, 49, &[2]),
            (0, 50, &[2]),
            (49, 50, &[3]),
            (50, 49, &[2]),
            (89, 90, &[4]),
            (90, 89, &[3]),
            (0, 95, &[2, 4]),
            (95, 0, &[1, 2, 3]),
            (30, 95, &[3]),
            (60, 60, &[]),
            (100, 255, &[]),
        ] {
            assert_eq!(drunk_messages(old, new), want, "{old} -> {new}");
        }
    }

    /// The drunk watcher's run (`0x5e2a90`): on the active player, `UNIT_LEVEL` for `"target"` when
    /// the player can attack its target (`0x5e2afb`-`0x5e2b38`); then, on any player, each message
    /// as a system chat line (`0x49a870`, type 0xa): `DRUNK_MESSAGE_SELF%d` for the active player
    /// (`0x5e2bf8`), else `DRUNK_MESSAGE_OTHER%d` filled with the player's name (`0x5e2c09`,
    /// `0x609210`).
    #[test]
    fn the_drunk_watcher_signals_the_target_and_speaks() {
        const OTHER: u64 = 0x08;
        let mut app = hosted_world(benilla_ui::script::UnitGuids {
            target: MOB,
            ..seated()
        });
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_player(OTHER, "Bob".into(), None);
        // A Human us and an Orc mob, which the host's relation functions hold attackable.
        {
            use crate::target::relations::fixture::{creature, factions, player, store_of};
            use crate::target::relations::fixture::{HUMAN, ORC};
            let world = app.world_mut();
            world.insert_resource(factions());
            let me = world
                .spawn((
                    crate::net::SelfPlayer,
                    crate::net::Guid(ME),
                    store_of(&player(ME, HUMAN)),
                ))
                .id();
            let mob = world
                .spawn((crate::net::Guid(MOB), store_of(&creature(MOB, ORC))))
                .id();
            let mut index = world.resource_mut::<crate::net::GuidIndex>();
            index.0.insert(ME, me);
            index.0.insert(MOB, mob);
        }
        {
            let script = app.world_mut().non_send_resource_mut::<UiScript>();
            script
                .run(
                    r#"
                    for k = 1, 4 do
                        setglobal("DRUNK_MESSAGE_SELF" .. k, "self " .. k)
                        setglobal("DRUNK_MESSAGE_OTHER" .. k, "%s other " .. k)
                    end
                "#,
                )
                .unwrap();
        }
        let drunk = |guid, old, new| FieldNotice::Drunk { guid, old, new };
        assert_eq!(
            fired(&mut app, &[drunk(ME, 0, 95)]),
            [
                "UNIT_LEVEL target",
                "CHAT_MSG_SYSTEM self 2",
                "CHAT_MSG_SYSTEM self 4"
            ]
        );
        // Another player: no signal, its name in the line.
        assert_eq!(
            fired(&mut app, &[drunk(OTHER, 95, 0)]),
            [
                "CHAT_MSG_SYSTEM Bob other 1",
                "CHAT_MSG_SYSTEM Bob other 2",
                "CHAT_MSG_SYSTEM Bob other 3"
            ]
        );
        // A target the player cannot attack: the lines alone.
        app.world_mut()
            .resource_mut::<crate::target::Selection>()
            .guid = Some(seated().target);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit_guids(&seated());
        assert_eq!(
            fired(&mut app, &[drunk(ME, 60, 40)]),
            ["CHAT_MSG_SYSTEM self 2"]
        );
        assert!(
            app.world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines()
                .is_empty(),
            "each line fired in the watcher's run (`0x5e2c68`)"
        );
    }

    /// A watcher's tutorial fires inside its run (`0x4b5390` with delay 0): the `TutorialPopup` cue
    /// and `TUTORIAL_TRIGGER` with the number plus one, ahead of the next notice's event, once per
    /// fire-once bit.
    #[test]
    fn a_watchers_tutorial_fires_in_its_place_once() {
        let mut app = world(seated());
        app.init_resource::<crate::tutorial::Tutorials>();
        app.world_mut()
            .resource::<crate::tutorial::Tutorials>()
            .lock()
            .apply_flags(&[0u8; 8]);
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Tutorial { id: 0x1d },
                    FieldNotice::Global { id: 405 },
                    FieldNotice::Tutorial { id: 0x1d },
                ]
            ),
            ["TUTORIAL_TRIGGER 30", "PLAYER_UPDATE_RESTING nil"]
        );
        assert_eq!(
            app.world()
                .resource::<crate::sound::MessageSounds>()
                .queued_cues(),
            ["TutorialPopup"]
        );
    }

    /// A skill watch's rebuild signals `SKILL_LINES_CHANGED` (`0x4d2fec`); the value watch then
    /// speaks its line (`0x5de365`-`0x5de417`): `ERR_SKILL_GAINED_S` over an old value of 0, else
    /// `ERR_SKILL_UP_SI` with the shown value, the permanent bonus included, both as
    /// `CHAT_MSG_SKILL`; a line whose race-class flags carry `0x402` stays silent (`0x5de358`).
    #[test]
    fn the_skill_notices_rebuild_the_list_and_speak() {
        let mut app = world(seated());
        app.insert_resource(crate::ui_spellbook::SkillLines {
            catalog: benilla_formats::SkillLineCatalog::from_spell_lines([])
                .with_line_names([(164, "Blacksmithing"), (762, "Riding")])
                .with_race_class_flags([(164, 0), (762, 0x2)]),
        });
        // A human warrior: race and class 1 (`UNIT_FIELD_BYTES_0`).
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(36, 0x0101)])),
        ));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"ERR_SKILL_GAINED_S = "You have gained the %s skill."
                ERR_SKILL_UP_SI = "Your skill in %s has increased to %d.""#,
            )
            .unwrap();
        assert_eq!(
            fired(&mut app, &[FieldNotice::SkillLines]),
            ["SKILL_LINES_CHANGED nil"]
        );
        let value = |skill_id, old, shown| FieldNotice::SkillValue {
            skill_id,
            old,
            shown,
        };
        assert_eq!(
            fired(&mut app, &[value(164, 0, 1)]),
            [
                "SKILL_LINES_CHANGED nil",
                "CHAT_MSG_SKILL You have gained the Blacksmithing skill."
            ]
        );
        assert_eq!(
            fired(&mut app, &[value(164, 1, 17)]),
            [
                "SKILL_LINES_CHANGED nil",
                "CHAT_MSG_SKILL Your skill in Blacksmithing has increased to 17."
            ]
        );
        assert_eq!(
            fired(&mut app, &[value(762, 75, 150)]),
            ["SKILL_LINES_CHANGED nil"]
        );
    }

    /// The two readers share one position: what the drop took, the firing never fires.
    #[test]
    fn a_dropped_notice_never_fires_later() {
        let mut app = world(seated());
        app.world_mut()
            .resource_mut::<crate::field_watch::NoticeLane>()
            .write(FieldNotice::Player { id: 523 });
        app.world_mut().run_system_once(drop_field_notices).unwrap();
        assert_eq!(
            fired(&mut app, &[FieldNotice::Player { id: 524 }]),
            ["PLAYER_PVP_RANK_CHANGED player"]
        );
    }

    /// The lane's registration: while no in-game interface is up the drop takes each notice the
    /// frame it is written, unheard; once one is up the lanes' drain fires it.
    #[test]
    fn the_drop_runs_exactly_while_no_interface_is_up() {
        let mut app = world(seated());
        add_notice_lane(&mut app);
        let heard = |app: &mut App, notice: FieldNotice| -> Vec<String> {
            app.world_mut()
                .resource_mut::<crate::field_watch::NoticeLane>()
                .write(notice);
            app.update();
            assert_eq!(
                app.world()
                    .resource::<crate::field_watch::NoticeLane>()
                    .iter()
                    .count(),
                0,
                "fired or dropped the frame it is written"
            );
            seen(app)
        };
        // No state yet: the login screen's side of the gate.
        assert!(heard(&mut app, FieldNotice::Player { id: 189 }).is_empty());
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        assert_eq!(
            heard(&mut app, FieldNotice::Player { id: 189 }),
            ["PLAYER_XP_UPDATE player"],
            "the interface is up: the drain fires it"
        );
        // An entry load pending is the interface not up yet.
        app.insert_resource(crate::ui_script::PendingEntryUiLoad);
        assert!(heard(&mut app, FieldNotice::Player { id: 406 }).is_empty());
    }

    /// A self player seated in a world that seats the four tokens' copies, its VM
    /// recording what `lua` registers.
    fn feed_world(pairs: &[(u16, u32)], lua: &str) -> (App, Entity) {
        use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
        let mut app = App::new();
        app.add_message::<crate::net::FieldChanged>()
            .init_resource::<crate::field_watch::NoticeLane>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let mut script = UiScript::new().unwrap();
        script.run(lua).unwrap();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            ..Default::default()
        });
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(ME),
                ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(pairs)
                        .into_created(benilla_protocol::messages::ObjectType::Player),
                ),
            ))
            .id();
        (app, me)
    }

    fn seen(app: &mut App) -> Vec<String> {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        let out = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        out
    }

    /// A frame's feeds, then our create's arrival item fired (`0x5dec7b` → `0x5dea50`).
    fn arrive(app: &mut App) {
        app.update();
        crate::ui_world_enter::push_arrival(app.world_mut());
        crate::ui_world_enter::fire_lane(app.world_mut());
    }

    /// A frame's feeds, then a `/reload`'s cascade fired (`0x490168`).
    fn reenter(app: &mut App) {
        app.update();
        app.world_mut()
            .get_resource_or_init::<crate::ui_world_enter::ArrivalLane>()
            .push(crate::ui_world_enter::Entry::Reload);
        crate::ui_world_enter::fire_lane(app.world_mut());
    }

    /// Every feed whose push a field event's handlers read runs ahead of the drain that fires it,
    /// as the update handler writes every block of its packet before its notify pass (`0x46520e`,
    /// `0x465222`, then `0x46522d`).
    #[test]
    fn the_field_event_pushes_run_ahead_of_the_drain() {
        let mut headless = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut headless,
            crate::ui_char::feed_char,
            crate::lanes::fire_in_order,
        ));
    }

    /// The active player's arrival runs its ghost state (`0x5dea50` → `0x5deb2a` → `0x5de9c0`)
    /// ahead of the world-enter cascade (`0x5deb60`): alive, it signals `PLAYER_UNGHOST`
    /// (`0x5dea2a`) before `PLAYER_ENTERING_WORLD`; as a ghost its leg is the Death tutorial and no
    /// event. A `/reload`'s cascade (`0x490168`) runs no arrival, so its fresh VM hears
    /// `PLAYER_ENTERING_WORLD` alone.
    #[test]
    fn an_alive_arrival_signals_player_unghost_ahead_of_the_cascade() {
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_UNGHOST")
            f:RegisterEvent("PLAYER_ENTERING_WORLD")
            f:SetScript("OnEvent", function() table.insert(SEEN, event) end)"#;
        const PLAYER_FLAGS: u16 = 190;
        for (flags, heard) in [
            (0, &["PLAYER_UNGHOST", "PLAYER_ENTERING_WORLD"][..]),
            (0x10, &["PLAYER_ENTERING_WORLD"][..]),
        ] {
            let (mut app, _) = feed_world(&[(PLAYER_FLAGS, flags)], spy);
            app.add_systems(Update, super::super::seat_tokens);
            arrive(&mut app);
            assert_eq!(seen(&mut app), heard, "flags {flags:#x}: the arrival");
            let mut reloaded = UiScript::new().unwrap();
            reloaded.run(spy).unwrap();
            reloaded.set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                ..Default::default()
            });
            app.insert_non_send_resource(reloaded);
            reenter(&mut app);
            assert_eq!(
                seen(&mut app),
                ["PLAYER_ENTERING_WORLD"],
                "flags {flags:#x}: the reload"
            );
        }
    }

    /// Every world entry, a `/reload`'s included, refreshes the bonus bar after the entering
    /// event (`0x490979` → `0x4e4f90` → `0x4e4fc0`): the offset set from our form, 0 with none,
    /// then `UPDATE_BONUS_ACTIONBAR` whether or not the offset moved (`0x4e5018`).
    #[test]
    fn every_world_entry_signals_the_bonus_bar_after_entering() {
        const BYTES_1: u16 = 138;
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_ENTERING_WORLD")
            f:RegisterEvent("UPDATE_BONUS_ACTIONBAR")
            f:SetScript("OnEvent", function()
                if event == "UPDATE_BONUS_ACTIONBAR" then
                    table.insert(SEEN, event .. " " .. GetBonusBarOffset())
                else
                    table.insert(SEEN, event)
                end
            end)"#;
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.forms.insert(
            1,
            benilla_formats::ShapeshiftForm {
                bonus_bar: 1,
                ..Default::default()
            },
        );
        for (form, offset) in [(0u32, 0), (1, 1)] {
            let (mut app, _) = feed_world(&[(BYTES_1, form << 16)], spy);
            app.world_mut().insert_resource(crate::ui_action::Spells {
                forms: spells.forms.clone(),
                ..crate::ui_action::Spells::empty_for_tests()
            });
            app.add_systems(Update, super::super::seat_tokens);
            arrive(&mut app);
            let heard = [
                "PLAYER_ENTERING_WORLD".to_string(),
                format!("UPDATE_BONUS_ACTIONBAR {offset}"),
            ];
            assert_eq!(seen(&mut app), heard, "form {form}: the login");
            let reloaded = UiScript::new().unwrap();
            reloaded.run(spy).unwrap();
            app.insert_non_send_resource(reloaded);
            reenter(&mut app);
            assert_eq!(seen(&mut app), heard, "form {form}: the reload");
        }
    }

    /// The login in the server's order: the faction list lands before the player's create (vmangos
    /// `SendInitialPacketsBeforeAddToMap`, `Player.cpp:19256`), so the list's first build and the
    /// world-enter cascade share the create's frame. The cascade's rebuild signals
    /// `UPDATE_FACTION` after `PLAYER_ENTERING_WORLD` (`0x4909d3`), the player seated, so
    /// `ReputationWatchBar_Update` reads the level and a 60 trades the XP bar for the max-level art
    /// (`ReputationFrame.lua:236-239`), a 59 keeps it (`:241-243`). Skips without client data.
    #[test]
    fn the_world_enter_rebuild_sets_the_xp_bar_by_the_seated_level() {
        benilla_formats::wow_data_or_skip!();
        const WATCHED: u16 = 1261;
        const XP: u16 = 716;
        for (level, xp_bar) in [(60, false), (59, true)] {
            let (mut app, _) = feed_world(
                &[
                    (benilla_protocol::field::FIELD_UNIT_LEVEL, level),
                    (WATCHED, u32::MAX),
                    // `PLAYER_XP` and `PLAYER_NEXT_LEVEL_XP`: a 60 on vmangos reads 0 / 217400.
                    (XP, 0),
                    (XP + 1, 217_400),
                    // `PLAYER_BYTES_2` byte 3, rest state 2, normal: `ExhaustionTick_Update`
                    // compares `GetRestState()` at `PLAYER_ENTERING_WORLD`.
                    (194, 2 << 24),
                ],
                "",
            );
            app.insert_resource(crate::target::Factions::from_catalog(
                benilla_formats::FactionCatalog::from_rows(Default::default(), Default::default()),
            ));
            {
                let s = app.world().non_send_resource::<UiScript>();
                for file in [
                    r"Interface\FrameXML\GlobalStrings.lua",
                    r"Interface\FrameXML\Fonts.xml",
                    // `TEXT` (`BasicControls.xml:24`), which the popups' file reads.
                    r"Interface\FrameXML\BasicControls.xml",
                    r"Interface\FrameXML\UIParent.xml",
                    r"Interface\FrameXML\MoneyFrame.lua",
                    r"Interface\FrameXML\MoneyFrame.xml",
                    r"Interface\FrameXML\GameTooltip.xml",
                    r"Interface\FrameXML\UIPanelTemplates.lua",
                    r"Interface\FrameXML\UIPanelTemplates.xml",
                    r"Interface\FrameXML\OptionsFrameTemplates.xml",
                    // `UIParent`'s `PLAYER_UNGHOST` arm, which an alive arrival signals ahead of
                    // the cascade, hides the resurrect popups (`UIParent.lua:251-257`).
                    r"Interface\FrameXML\StaticPopup.xml",
                    r"Interface\FrameXML\TextStatusBar.lua",
                    r"Interface\FrameXML\TextStatusBar.xml",
                    r"Interface\FrameXML\MainMenuBar.xml",
                    r"Interface\FrameXML\Cooldown.xml",
                    r"Interface\FrameXML\ActionButtonTemplate.xml",
                    r"Interface\FrameXML\ActionBarFrame.xml",
                    r"Interface\FrameXML\ReputationFrame.xml",
                    r"Interface\FrameXML\BonusActionBarFrame.xml",
                ] {
                    crate::ui_script::test_ui::load_ui(s, file);
                }
            }
            app.add_systems(Update, super::super::seat_tokens);
            arrive(&mut app);
            let s = app.world().non_send_resource::<UiScript>();
            let shown = |frame: &str| {
                s.eval::<bool>(&format!("return {frame}:IsShown() ~= nil"))
                    .unwrap()
            };
            assert_eq!(shown("MainMenuExpBar"), xp_bar, "level {level}: the XP bar");
            assert_eq!(
                shown("MainMenuBarMaxLevelBar"),
                !xp_bar,
                "level {level}: the max-level art"
            );
            assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
        }
    }

    /// The cascade's pet step (`0x4909c9` → `0x4bc6f0`), after the bonus bar and before the
    /// reputation rebuild: `SetPetGuid(0, 0)` (`0x4bc6fd`), whose change signals
    /// `UNIT_PET("player")` (`0x4bc84f`) and which always signals `PET_BAR_UPDATE` (`0x4bc90c`),
    /// then `CMSG_REQUEST_PET_INFO` (`0x4bc738`) when our charm, else our summon, is set
    /// (`0x4bc721`-`0x4bc736`). Each handler reads the bar already gone. A `/reload` with a pet up
    /// takes the change too: the guid outlives the interface.
    #[test]
    fn the_world_entry_drops_the_pet_bar_and_asks_for_it_again() {
        use crate::net::ClientCommand;
        /// `UNIT_FIELD_SUMMON`.
        const SUMMON: u16 = 8;
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UPDATE_BONUS_ACTIONBAR")
            f:RegisterEvent("UNIT_PET")
            f:RegisterEvent("PET_BAR_UPDATE")
            f:RegisterEvent("UPDATE_FACTION")
            f:SetScript("OnEvent", function()
                table.insert(SEEN, event .. " " .. tostring(arg1) .. " " .. tostring(PetHasActionBar()))
            end)"#;
        for summoned in [true, false] {
            let pairs: Vec<(u16, u32)> = if summoned {
                vec![(SUMMON, PET as u32), (SUMMON + 1, (PET >> 32) as u32)]
            } else {
                vec![]
            };
            let (mut app, _) = feed_world(&pairs, spy);
            let (tx, rx) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            app.init_resource::<crate::ui_pet::PetBar>()
                .init_resource::<crate::ui_pet::PetAttackLatch>();
            if summoned {
                app.world_mut()
                    .resource_mut::<crate::ui_pet::PetBar>()
                    .spells
                    .pet_guid = PET;
                app.world_mut()
                    .resource_mut::<crate::ui_pet::PetAttackLatch>()
                    .0 = Some(PET);
            }
            app.add_systems(Update, super::super::seat_tokens);
            arrive(&mut app);
            let mut heard = vec!["UPDATE_BONUS_ACTIONBAR nil nil"];
            if summoned {
                heard.push("UNIT_PET player nil");
            }
            heard.extend([
                "PET_BAR_UPDATE nil nil",
                "UPDATE_FACTION nil nil",
                "UPDATE_FACTION nil nil",
            ]);
            assert_eq!(seen(&mut app), heard, "summoned {summoned}");
            assert_eq!(
                app.world()
                    .resource::<crate::ui_pet::PetBar>()
                    .spells
                    .pet_guid,
                0
            );
            assert_eq!(
                app.world().resource::<crate::ui_pet::PetAttackLatch>().0,
                None
            );
            let asked = rx
                .try_iter()
                .filter(|c| matches!(c, ClientCommand::RequestPetInfo))
                .count();
            assert_eq!(asked, usize::from(summoned), "summoned {summoned}");
        }
    }

    /// `EnterWorldUI`'s action bar refresh (`0x490979` → `0x4e4f90`): the usable refresh
    /// `0x4e5c00`, whose `ACTIONBAR_UPDATE_USABLE` (`0x4e5c53`) comes after the minimap's set-up
    /// (`0x49096f`) and before the bonus bar's `UPDATE_BONUS_ACTIONBAR` (`0x4e4f95` → `0x4e4fc0`)
    /// and the pet reset (`0x4909c9`), inside the cascade. With no pair changed it signals nothing.
    #[test]
    fn the_world_entry_signals_the_usable_refresh_inside_the_cascade() {
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PLAYER_ENTERING_WORLD", "ACTIONBAR_UPDATE_USABLE",
                    "UPDATE_BONUS_ACTIONBAR", "PET_BAR_UPDATE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(SEEN, event .. " " .. tostring(arg1) .. " nil")
            end)"#;
        let (mut app, _) = feed_world(&[], spy);
        app.add_systems(Update, super::super::seat_tokens);
        arrive(&mut app);
        assert_eq!(
            seen(&mut app),
            [
                "PLAYER_ENTERING_WORLD nil nil",
                "UPDATE_BONUS_ACTIONBAR nil nil",
                "PET_BAR_UPDATE nil nil"
            ],
            "no pair changed"
        );
        let mut script = UiScript::new().unwrap();
        script.run(spy).unwrap();
        script.fire_world_enter_action_bar(0, true);
        script.resolve();
        assert_eq!(
            script.eval::<Vec<String>>("return SEEN").unwrap(),
            [
                "ACTIONBAR_UPDATE_USABLE nil nil",
                "UPDATE_BONUS_ACTIONBAR nil nil"
            ],
            "a changed pair signals ahead of the bonus bar"
        );
    }

    /// The cascade's buff cache set-up (`0x490983` → `0x4e4100`): its rebuild's
    /// `PLAYER_AURAS_CHANGED` (`0x4e437f`) after the action bar's refresh (`0x490979`) and before
    /// the pet reset (`0x4909c9`), inside the cascade, with the cache already holding our auras.
    #[test]
    fn the_world_entry_rebuilds_the_buff_cache_inside_the_cascade() {
        /// `UNIT_FIELD_AURA` and `UNIT_FIELD_AURAFLAGS`.
        const AURA: u16 = 47;
        const AURAFLAGS: u16 = 95;
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"PLAYER_ENTERING_WORLD", "UPDATE_BONUS_ACTIONBAR",
                    "PLAYER_AURAS_CHANGED", "PET_BAR_UPDATE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(SEEN, event .. " " .. tostring(GetPlayerBuff(0, "HELPFUL")) .. " nil")
            end)"#;
        let (mut app, _) = feed_world(&[(AURA, 1126), (AURAFLAGS, 0x9)], spy);
        app.init_resource::<crate::ui_aura::PlayerAuraCache>();
        app.add_systems(Update, super::super::seat_tokens);
        arrive(&mut app);
        assert_eq!(
            seen(&mut app),
            [
                "PLAYER_ENTERING_WORLD -1 nil",
                "UPDATE_BONUS_ACTIONBAR -1 nil",
                "PLAYER_AURAS_CHANGED 0 nil",
                "PET_BAR_UPDATE 0 nil",
            ]
        );
    }

    /// The cascade's `SetPetGuid(0, 0)` stores the zero guid (`0x4bc822`, `0x4bc827`) before its
    /// `UNIT_PET` (`0x4bc84f`), and `HasPetUI` resolves the pet from that guid (`0x4be67e`-
    /// `0x4be690`): a `/reload` with a pet out hears `UNIT_PET` with `HasPetUI()` nil. The pet's
    /// happiness field moves at neither the VM's first push nor the bar's return, so neither signals
    /// `UNIT_HAPPINESS`, which only its field's watch raises.
    #[test]
    fn the_world_entry_pet_reset_signals_unit_pet_with_no_pet_ui() {
        /// `UNIT_FIELD_BYTES_0` (class in byte 1) and `UNIT_FIELD_PETNUMBER`.
        const BYTES_0: u16 = 36;
        const PETNUMBER: u16 = 139;
        let spy = r#"SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_PET")
            f:RegisterEvent("UNIT_HAPPINESS")
            f:SetScript("OnEvent", function()
                local ui, hunter = HasPetUI()
                table.insert(SEEN, event .. " " .. tostring(ui) .. " " .. tostring(hunter))
            end)"#;
        let (mut app, me) = feed_world(&[(BYTES_0, 3 << 8)], spy);
        app.init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::ui_pet::PetAttackLatch>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(crate::net::SelfGuid(Some(ME)));
        let pet = app
            .world_mut()
            .spawn((
                crate::net::Guid(PET),
                crate::net::ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[(PETNUMBER, 7)])
                        .into_created(benilla_protocol::messages::ObjectType::Unit),
                ),
            ))
            .id();
        {
            let mut index = app.world_mut().resource_mut::<crate::net::GuidIndex>();
            index.0.insert(ME, me);
            index.0.insert(PET, pet);
        }
        app.world_mut()
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = PET;
        app.add_systems(Update, super::super::seat_tokens);
        reenter(&mut app);
        assert_eq!(seen(&mut app), ["UNIT_PET nil nil"], "the reload's entry");
        // `SMSG_PET_SPELLS` brings the same pet back.
        app.world_mut()
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = PET;
        app.update();
        assert!(seen(&mut app).is_empty(), "the pet's return");
    }
}
