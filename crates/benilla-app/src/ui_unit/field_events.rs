//! The interface events of the field watches and the packet handlers that signal unit events
//! ([`crate::field_watch`]): each notice the packets wrote this frame fires its callback's
//! signals, in wire order, after every snapshot those events' handlers read is pushed
//! ([`FieldEventPushes`]) and the unit tokens are seated ([`crate::ui_aura::AuraEvents`]).

use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::field_watch::FieldNotice;

/// The feeds that push what a field event's handlers read: the unit tokens' snapshots with the XP
/// and rest values (`feed_units`), the group's and the pet's tokens, the pet's stats, the player's
/// combat stats and inventory, the inspected unit's slots, the honor block and the watched
/// faction. The firing runs
/// after all of them, as the reference writes a whole block before its notify pass.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FieldEventPushes;

/// The read position in the notices, one for both of their readers: the firing while the in-game
/// interface is up and the drop while it is not, so each notice is taken exactly once, the frame
/// it is written.
#[derive(Resource, Default)]
pub(crate) struct FieldNoticeCursor(MessageCursor<FieldNotice>);

/// The notices [`fire_field_notices`] left for the chat drain, each by its message id: each came
/// after a chat item the drain had yet to fire, in the order the packets queued them
/// ([`super::wire_order`]).
#[derive(Resource, Default)]
pub(crate) struct DeferredNotices(Vec<(usize, FieldNotice)>);

/// What a field notice's firing reads, and the notices left for the chat drain.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct NoticeFiring<'w, 's> {
    sink: crate::ui_action::MessageSink<'w>,
    // The drunk watcher names the player it runs on (`0x609210`).
    names: Res<'w, crate::names::NameCache>,
    // The quest log's watcher reads the slots.
    me: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    tutorials: Option<ResMut<'w, crate::tutorial::Tutorials>>,
    // `Option`, as the marks: a harness without the unit plugin defers nothing.
    deferred: Option<ResMut<'w, DeferredNotices>>,
    marks: Option<Res<'w, super::wire_order::WireMarks>>,
}

impl NoticeFiring<'_, '_> {
    /// One notice's signals.
    fn fire(&mut self, script: &mut UiScript, notice: FieldNotice) {
        if notice == FieldNotice::QuestLog {
            if let Some(store) = self.me.iter().next() {
                script.quest_log_changed(crate::ui_quest_log::slot_views(&store.0));
            }
            return;
        }
        if let FieldNotice::Tutorial { id } = notice {
            // `0x4b5390` with no delay, inside the watcher's run.
            if let Some(published) = self.tutorials.as_mut().and_then(|t| t.fire_now(id)) {
                crate::tutorial::fire_published(script, &self.sink.sounds, published);
            }
            return;
        }
        if let FieldNotice::Drunk { guid, old, new } = notice {
            drunk(script, &mut self.sink, &self.names, guid, old, new);
            return;
        }
        fire(script, &mut self.sink, notice);
    }

    /// The handler call that wrote notice `id` ([`super::wire_order::WireMarks::of_notice`]).
    fn call_of_notice(&self, id: usize) -> usize {
        self.marks.as_ref().map_or(0, |m| m.of_notice(id))
    }

    /// The handler call that queued chat item `seq`, on the notices' scale.
    pub(crate) fn call_of_chat(&self, seq: u64) -> usize {
        self.marks.as_ref().map_or(0, |m| m.of_chat(seq))
    }

    /// Fire the first waiting notice if a chat item of handler call `call` comes after it, as one
    /// of an earlier call or of the same one does; `None` fires it whatever its call. Whether one
    /// fired: one at a time, so the chat drain shows each one's line before the next signals.
    pub(crate) fn fire_next(&mut self, script: &mut UiScript, call: Option<usize>) -> bool {
        let Some(&(id, notice)) = self.deferred.as_ref().and_then(|d| d.0.first()) else {
            return false;
        };
        if call.is_some_and(|call| self.call_of_notice(id) > call) {
            return false;
        }
        if let Some(deferred) = self.deferred.as_mut() {
            deferred.0.remove(0);
        }
        self.fire(script, notice);
        true
    }

    /// Whether a notice waits for the chat drain.
    pub(crate) fn any_waiting(&self) -> bool {
        self.deferred.as_ref().is_some_and(|d| !d.0.is_empty())
    }

    /// Leave `notice` for the chat drain; `false` with nowhere to leave it.
    fn defer(&mut self, id: usize, notice: FieldNotice) -> bool {
        match self.deferred.as_mut() {
            Some(deferred) => {
                deferred.0.push((id, notice));
                true
            }
            None => false,
        }
    }
}

/// Fire this frame's field notices in the order the packets wrote them. A notice a packet wrote
/// after a chat item still queued, and every notice after it, waits for the chat drain, which
/// fires them in their packets' order among its items ([`super::wire_order`]). Under the unit
/// feed's gate, so only while the in-game interface is up; [`drop_field_notices`] takes them
/// otherwise.
pub(crate) fn fire_field_notices(
    mut hosted_vm: crate::script_calls::HostedVm,
    notices: Res<Messages<FieldNotice>>,
    mut cursor: ResMut<FieldNoticeCursor>,
    mut firing: NoticeFiring,
) {
    hosted_vm.run(|script| {
        let fresh: Vec<(FieldNotice, usize)> = cursor
            .0
            .read_with_id(&notices)
            .map(|(notice, id)| (*notice, id.id))
            .collect();
        // No VM, nobody to hear them: they are dropped, as below.
        let Some(script) = script else {
            return;
        };
        // The chat drain's first item, by the handler call that queued it.
        let chat_first = firing
            .sink
            .chat
            .first_live_seq()
            .map(|seq| firing.call_of_chat(seq));
        for (notice, id) in fresh {
            let waits =
                firing.any_waiting() || chat_first.is_some_and(|c| c < firing.call_of_notice(id));
            if waits && firing.defer(id, notice) {
                continue;
            }
            firing.fire(script, notice);
        }
    })
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
    names: &crate::names::NameCache,
    guid: u64,
    old: u8,
    new: u8,
) {
    let me = script.is_active_player(guid);
    if me && script.player_can_attack_target() {
        script.fire_event("UNIT_LEVEL", vec![ScriptValue::Str("target".to_string())]);
    }
    let name = (!me).then(|| {
        names
            .resolve_player(guid, crate::query_cache::Ask::Read)
            .map_or_else(|| script.unknown_object_text(), str::to_string)
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
        .collect();
    crate::ui_action::show_messages(script, sink, "ui_unit", lines);
}

/// The field notices' second reader and its position: [`drop_field_notices`] under the exact
/// complement of the unit feed's gate, so a notice is fired or dropped the frame it is written,
/// never left to age in the buffer.
pub(super) fn add_notice_drop(app: &mut App) {
    app.add_message::<FieldNotice>()
        .init_resource::<FieldNoticeCursor>()
        .init_resource::<DeferredNotices>()
        .init_resource::<super::wire_order::WireMarks>()
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
pub(super) fn drop_field_notices(
    notices: Res<Messages<FieldNotice>>,
    mut cursor: ResMut<FieldNoticeCursor>,
    deferred: Option<ResMut<DeferredNotices>>,
) {
    cursor.0.clear(&notices);
    if let Some(mut deferred) = deferred {
        deferred.0.clear();
    }
}

/// One callback's signals.
fn fire(script: &mut UiScript, sink: &mut crate::ui_action::MessageSink, notice: FieldNotice) {
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
            let line = super::rest_state_message(old, new)
                .and_then(|key| crate::ui_action::keyed_line(script, key));
            crate::ui_action::show_messages(script, sink, "ui_unit", line);
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
        }
        FieldNotice::Global { id } => {
            if let Some(global) = global_event(id) {
                script.fire_event(global, vec![]);
            }
        }
        // Fired by [`fire_field_notices`], which holds the slots, the banks and the names.
        FieldNotice::QuestLog | FieldNotice::Tutorial { .. } | FieldNotice::Drunk { .. } => {}
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
            crate::ui_action::show_messages(script, sink, "ui_unit", lines.into_iter().flatten());
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
        id::PET_ATTACK_START => "PET_ATTACK_START",
        id::PET_ATTACK_STOP => "PET_ATTACK_STOP",
        id::PLAYER_GUILD_UPDATE => "PLAYER_GUILD_UPDATE",
        id::TABARD_CANSAVE_CHANGED => "TABARD_CANSAVE_CHANGED",
        id::PLAYER_UNGHOST => "PLAYER_UNGHOST",
        id::PLAYER_UPDATE_RESTING => "PLAYER_UPDATE_RESTING",
        id::PET_UI_UPDATE => "PET_UI_UPDATE",
        id::PLAYTIME_CHANGED => "PLAYTIME_CHANGED",
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
    /// the table is set, but `UNIT_HAPPINESS`'s, which the pet stats feed fires.
    #[test]
    fn the_bridge_ids_are_every_named_field_of_the_table() {
        let want: std::collections::BTreeSet<u16> = event_table()
            .into_keys()
            .filter(|&id| id < 0xb6 && id != 21)
            .collect();
        let have: std::collections::BTreeSet<u16> =
            crate::field_watch::BRIDGE_IDS.into_iter().collect();
        assert_eq!(have, want);
    }

    /// A world whose VM records every event the notices fire, with `guids` seated.
    fn world(guids: benilla_ui::script::UnitGuids) -> App {
        let mut app = App::new();
        app.add_message::<FieldNotice>()
            .init_resource::<FieldNoticeCursor>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::names::NameCache>();
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
        app
    }

    fn fired(app: &mut App, notices: &[FieldNotice]) -> Vec<String> {
        app.world_mut()
            .resource_mut::<Messages<FieldNotice>>()
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
                "UPDATE_EXHAUSTION player",
            ]
        );
    }

    /// A global notice fires its event bare, once, whatever unit raised it; the PvP preference
    /// shows its toast for the new state, then its verbose sentence as a chat line.
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
                "PET_ATTACK_STOP nil",
                "UI_INFO_MESSAGE off",
                "PLAYER_UNGHOST nil",
            ]
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines(),
            ["on, verbose", "off, verbose"]
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
        struct Attackable;
        impl benilla_ui::script::UnitRelations for Attackable {
            fn holds(&self, guid: u64) -> bool {
                guid == MOB || guid == ME
            }
            fn reaction(&self, _: u64, _: u64) -> Option<u8> {
                None
            }
            fn can_attack(&self, a: u64, b: u64) -> Option<bool> {
                Some(a == ME && b == MOB)
            }
            fn can_assist(&self, _: u64, _: u64) -> Option<bool> {
                None
            }
            fn can_cooperate(&self, _: u64, _: u64) -> Option<bool> {
                None
            }
            fn is_pvp(&self, _: u64) -> Option<bool> {
                None
            }
        }
        let mut app = world(benilla_ui::script::UnitGuids {
            target: MOB,
            ..seated()
        });
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_player(OTHER, "Bob".into(), None);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_unit_relations(Some(std::rc::Rc::new(Attackable)));
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
        let chat = |app: &mut App| -> Vec<String> {
            let lines = app
                .world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines();
            app.world_mut()
                .insert_resource(crate::ui_chat::ChatLog::default());
            lines
        };
        let drunk = |guid, old, new| FieldNotice::Drunk { guid, old, new };
        assert_eq!(fired(&mut app, &[drunk(ME, 0, 95)]), ["UNIT_LEVEL target"]);
        assert_eq!(chat(&mut app), ["self 2", "self 4"]);
        // Another player: no signal, its name in the line.
        assert!(fired(&mut app, &[drunk(OTHER, 95, 0)]).is_empty());
        assert_eq!(
            chat(&mut app),
            ["Bob other 1", "Bob other 2", "Bob other 3"]
        );
        // A target the player cannot attack: the lines alone.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit_guids(&seated());
        assert!(fired(&mut app, &[drunk(ME, 60, 40)]).is_empty());
        assert_eq!(chat(&mut app), ["self 2"]);
    }

    /// A watcher's tutorial fires inside its run (`0x4b5390` with delay 0): the `TutorialPopup` cue
    /// and `TUTORIAL_TRIGGER` with the number plus one, ahead of the next notice's event, once per
    /// fire-once bit.
    #[test]
    fn a_watchers_tutorial_fires_in_its_place_once() {
        let mut app = world(seated());
        app.init_resource::<crate::tutorial::Tutorials>();
        app.world_mut()
            .resource_mut::<crate::tutorial::Tutorials>()
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

    /// The two readers share one position: what the drop took, the firing never fires.
    #[test]
    fn a_dropped_notice_never_fires_later() {
        let mut app = world(seated());
        app.world_mut()
            .resource_mut::<Messages<FieldNotice>>()
            .write(FieldNotice::Player { id: 523 });
        app.world_mut().run_system_once(drop_field_notices).unwrap();
        assert_eq!(
            fired(&mut app, &[FieldNotice::Player { id: 524 }]),
            ["PLAYER_PVP_RANK_CHANGED player"]
        );
    }

    /// The drop's own registration: while no in-game interface is up it takes each notice the
    /// frame it is written; once one is up it leaves them to the firing.
    #[test]
    fn the_drop_runs_exactly_while_no_interface_is_up() {
        let mut app = App::new();
        add_notice_drop(&mut app);
        let unread = |app: &mut App, notice: FieldNotice| -> usize {
            app.world_mut()
                .resource_mut::<Messages<FieldNotice>>()
                .write(notice);
            app.update();
            app.world_mut()
                .resource_scope(|world, cursor: Mut<FieldNoticeCursor>| {
                    cursor.0.len(world.resource::<Messages<FieldNotice>>())
                })
        };
        // No state yet: the login screen's side of the gate.
        assert_eq!(unread(&mut app, FieldNotice::Player { id: 189 }), 0);
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        assert_eq!(
            unread(&mut app, FieldNotice::Player { id: 189 }),
            1,
            "the interface is up: the firing's to take"
        );
        // An entry load pending is the interface not up yet.
        app.insert_resource(crate::ui_script::PendingEntryUiLoad);
        assert_eq!(unread(&mut app, FieldNotice::Player { id: 406 }), 0);
    }

    /// A self player seated in a world that runs `feed_units` and the honor feed, its VM
    /// recording what `lua` registers.
    fn feed_world(pairs: &[(u16, u32)], lua: &str) -> (App, Entity) {
        use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
        let mut app = App::new();
        app.add_message::<crate::net::FieldChanged>()
            .add_message::<FieldNotice>()
            .init_resource::<FieldNoticeCursor>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<super::super::UnitFeedState>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<crate::ui_honor::HonorFeedState>()
            .init_resource::<crate::ui_honor::InspectHonor>()
            .init_resource::<crate::ui_inspect::InspectTarget>();
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

    /// `PLAYER_TARGET_CHANGED` re-shows the inspect window on the new target
    /// (`Blizzard_InspectUI.lua:29-31`), whose doll reads its slots inside the call
    /// (`InspectPaperDollFrame.lua:57-79`): the aura feed seats the new target's guid, the inspect
    /// feed pushes its slots, then the event fires. Both edges are declared, not left to
    /// registration order.
    #[test]
    fn a_new_targets_slots_are_pushed_before_its_target_event() {
        use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};

        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !c.systems.values().any(|s| s.name.contains("benilla_app::")) {
            eprintln!("skipped: this build carries no type names");
            return;
        }
        let one = |suffix: &str| {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        // The aura feed is private to its module, so its side is read by name off the graph.
        let (aura, inspect, fire) = (
            one("::ui_aura::feed_auras"),
            one("::ui_inspect::feed_inspect"),
            one("::ui_unit::fire_unit_events"),
        );
        assert_eq!(
            [
                c.dependencies.contains(&(aura, inspect)),
                c.dependencies.contains(&(inspect, fire)),
            ],
            [true; 2],
            "aura seat, inspect push"
        );
    }

    /// A target held for the first time answers inside its own `PLAYER_TARGET_CHANGED`, whose
    /// inspect handler re-shows the window (`Blizzard_InspectUI.lua:29-31`): `CanInspect` reads
    /// the new target's object (`0x48a1b0` → `0x4944a0`) and so do the slot getters (`0x4c8328`),
    /// so its reach and its slots are in the VM before the event. The frame runs its feeds in the
    /// order the client schedules them.
    #[test]
    fn a_new_targets_reach_and_slots_answer_inside_its_target_event() {
        use crate::game_plugins::schedule_tests::headless_client;
        use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
        use crate::target::relations::fixture::{factions, HUMAN};
        use crate::target::Selection;
        use crate::test_support::runs_before;
        use benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR;
        use benilla_protocol::messages::ObjectFields;
        const OTHER: u64 = 0x1002;
        const HELM: u32 = 7366;
        /// `OBJECT_FIELD_TYPE` and `UNIT_FIELD_FACTIONTEMPLATE`.
        const OBJECT_TYPE: u16 = 2;
        const TEMPLATE: u16 = 35;

        let mut client = headless_client();
        let inspect_first = runs_before(
            &mut client,
            crate::ui_inspect::feed_inspect,
            super::super::fire_unit_events,
        );
        let reach_first = runs_before(
            &mut client,
            super::super::feed_unit_reach,
            super::super::fire_unit_events,
        );
        drop(client);

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::client_state::ClientState::InWorld)
            .init_resource::<Selection>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<super::super::UnitFeedState>()
            .init_resource::<crate::ui_inspect::InspectTarget>()
            .init_resource::<crate::ui_inspect::InspectFeedState>()
            .init_resource::<crate::items::Items>()
            .insert_resource(crate::net::SelfGuid(Some(ME)))
            .insert_resource(factions())
            .insert_resource(NetCommands(tx))
            .add_plugins(crate::ui_aura::UiAuraPlugin);
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(HELM, Some(crate::items::test_template("Helm")));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_TARGET_CHANGED")
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, tostring(CanInspect("target")) .. " "
                        .. tostring(GetInventoryItemLink("target", 1)))
                end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);

        // We at the origin; a Human 3 yards off wearing the helm, whom no token names yet.
        let mut spawn = |guid: u64, pairs: &[(u16, u32)], yards: f32| {
            let e = app
                .world_mut()
                .spawn((
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(pairs)),
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let me = spawn(ME, &[(OBJECT_TYPE, 0x19), (TEMPLATE, HUMAN)], 0.0);
        let other = spawn(
            OTHER,
            &[
                (OBJECT_TYPE, 0x19),
                (TEMPLATE, HUMAN),
                (FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2, HELM),
            ],
            3.0,
        );
        app.world_mut().entity_mut(me).insert(SelfPlayer);

        // One frame of the feeds after the aura feed's seat, in the client's order.
        let feeds = |app: &mut App, target_changed: bool| {
            app.update();
            if target_changed {
                app.world_mut().resource_scope(
                    |world, mut feed: Mut<super::super::UnitFeedState>| {
                        let script = world.non_send_resource::<UiScript>();
                        feed.vm.get(script).pending.target_changed = true;
                    },
                );
            }
            let world = app.world_mut();
            if inspect_first {
                world
                    .run_system_once(crate::ui_inspect::feed_inspect)
                    .unwrap();
            }
            if reach_first {
                world
                    .run_system_once(super::super::feed_unit_reach)
                    .unwrap();
            }
            world
                .run_system_once(super::super::fire_unit_events)
                .unwrap();
            if !inspect_first {
                world
                    .run_system_once(crate::ui_inspect::feed_inspect)
                    .unwrap();
            }
            if !reach_first {
                world
                    .run_system_once(super::super::feed_unit_reach)
                    .unwrap();
            }
        };
        feeds(&mut app, false);
        app.insert_resource(Selection {
            target: Some(other),
            guid: Some(OTHER),
            ..Default::default()
        });
        feeds(&mut app, true);

        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
        assert!(
            matches!(seen.as_slice(), [one] if one.starts_with("1 ") && one.contains("[Helm]")),
            "{seen:?}"
        );
    }

    /// A values block with new XP: `PLAYER_XP_UPDATE`'s handler reads the new `UnitXP`, because
    /// the schedule runs every push the field events' handlers read ahead of the firing.
    #[test]
    fn the_xp_handler_reads_the_xp_its_block_wrote() {
        const XP: u16 = 716;
        let mut headless = crate::game_plugins::schedule_tests::headless_client();
        let ahead = [
            crate::test_support::runs_before(
                &mut headless,
                super::super::feed_units,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_char::feed_char,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_honor::feed_honor,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_reputation::feed_reputation,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_party::feed_party,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_pet::feed_pet_unit,
                fire_field_notices,
            ),
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_pet_stats::feed_pet_stats,
                fire_field_notices,
            ),
            // The inspected unit's slots, which the bridge's `UNIT_INVENTORY_CHANGED` repaints.
            crate::test_support::runs_before(
                &mut headless,
                crate::ui_inspect::feed_inspect,
                fire_field_notices,
            ),
        ];
        assert_eq!(
            ahead, [true; 8],
            "feed_units, char, honor, reputation, party, pet, pet stats, inspect"
        );

        // In that order: the block lands, the push, the firing.
        let (mut app, me) = feed_world(
            &[(XP, 10), (XP + 1, 100)],
            r#"SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("PLAYER_XP_UPDATE")
               f:SetScript("OnEvent", function()
                   table.insert(SEEN, event .. " " .. UnitXP("player"))
               end)"#,
        );
        app.world_mut()
            .run_system_once(super::super::feed_units)
            .unwrap();
        let mut edges = Vec::new();
        let fields = {
            let mut store = app
                .world_mut()
                .get_mut::<crate::net::ObjectStore>(me)
                .unwrap();
            store.0.merge_diff(
                benilla_protocol::ObjectFields::from_pairs(&[(XP, 30)]),
                |i, o, n| edges.push((i, o, n)),
            );
            store.0.clone()
        };
        let notices = crate::field_watch::values(
            &crate::field_watch::Around {
                me: Some(ME),
                creature_lookup: &|_| false,
                others: &Vec::new,
                pet_bar: 0,
            },
            ME,
            benilla_protocol::messages::ObjectType::Player,
            &fields,
            &crate::field_watch::Notified::new(crate::field_watch::Sent::Fields(&[XP]), &edges),
        );
        app.world_mut()
            .resource_mut::<Messages<FieldNotice>>()
            .write_batch(notices);
        app.world_mut()
            .run_system_once(super::super::feed_units)
            .unwrap();
        app.world_mut().run_system_once(fire_field_notices).unwrap();
        assert_eq!(seen(&mut app), ["PLAYER_XP_UPDATE 30"]);
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
            app.add_systems(Update, super::super::feed_units);
            app.update();
            assert_eq!(seen(&mut app), heard, "flags {flags:#x}: the arrival");
            let mut reloaded = UiScript::new().unwrap();
            reloaded.run(spy).unwrap();
            reloaded.set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                ..Default::default()
            });
            app.insert_non_send_resource(reloaded);
            app.update();
            assert_eq!(
                seen(&mut app),
                ["PLAYER_ENTERING_WORLD"],
                "flags {flags:#x}: the reload"
            );
        }
    }

    /// `PLAYER_ENTERING_WORLD`'s handler reads the honor block, since the schedule pushes it ahead
    /// of the unit feed that fires the cascade: `HonorFrame_Update(1)` reads the weekly values
    /// only then (`HonorFrame.lua:10-11`).
    #[test]
    fn the_honor_block_is_pushed_before_the_world_enter_events() {
        const SESSION_KILLS: u16 = 1250;
        let mut headless = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut headless,
            crate::ui_honor::feed_honor,
            super::super::feed_units
        ));

        // In that order: the push, then the cascade.
        let (mut app, _) = feed_world(
            &[(SESSION_KILLS, 3)],
            r#"SEEN = {}
               local f = CreateFrame("Frame")
               f:RegisterEvent("PLAYER_ENTERING_WORLD")
               f:SetScript("OnEvent", function()
                   table.insert(SEEN, event .. " " .. GetPVPSessionStats())
               end)"#,
        );
        app.world_mut()
            .run_system_once(crate::ui_honor::feed_honor)
            .unwrap();
        app.world_mut()
            .run_system_once(super::super::feed_units)
            .unwrap();
        assert_eq!(seen(&mut app), ["PLAYER_ENTERING_WORLD 3"]);
    }

    /// The login in the server's order: the faction list lands before the player's create (vmangos
    /// `SendInitialPacketsBeforeAddToMap`, `Player.cpp:19256`), so the list's first push and the
    /// world-enter cascade share the create's frame. The cascade's rebuild signals
    /// `UPDATE_FACTION` after `PLAYER_ENTERING_WORLD` (`0x4909d3`), the player seated, so
    /// `ReputationWatchBar_Update` reads the level and a 60 trades the XP bar for the max-level art
    /// (`ReputationFrame.lua:236-239`), a 59 keeps it (`:241-243`). Skips without client data.
    #[test]
    fn the_world_enter_rebuild_sets_the_xp_bar_by_the_seated_level() {
        benilla_formats::wow_data_or_skip!();
        const WATCHED: u16 = 1261;
        const XP: u16 = 716;
        let mut headless = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::test_support::runs_before(
            &mut headless,
            crate::ui_reputation::feed_reputation,
            super::super::feed_units
        ));

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
            app.world_mut()
                .run_system_once(crate::ui_reputation::feed_reputation)
                .unwrap();
            app.world_mut()
                .run_system_once(super::super::feed_units)
                .unwrap();
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
}
