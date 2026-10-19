//! The interface events of the field watches and the packet handlers that signal unit events
//! ([`crate::field_watch`]): each notice the packets wrote this frame fires its callback's
//! signals, in wire order, after every snapshot those events' handlers read is pushed
//! ([`FieldEventPushes`]) and the unit tokens are seated ([`crate::ui_aura::AuraEvents`]).

use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::field_watch::FieldNotice;

/// The feeds that push what a field event's handlers read: the unit tokens' snapshots with the XP
/// and rest values (`feed_units`), the group's and the pet's tokens, the pet's stats, the combat
/// stats of the player and of the pet, the honor block and the watched faction. The firing runs
/// after all of them, as the reference writes a whole block before its notify pass.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FieldEventPushes;

/// The read position in the notices, one for both of their readers: the firing while the in-game
/// interface is up and the drop while it is not, so each notice is taken exactly once, the frame
/// it is written.
#[derive(Resource, Default)]
pub(crate) struct FieldNoticeCursor(MessageCursor<FieldNotice>);

/// Fire this frame's field notices in the order the packets wrote them. Under the unit feed's
/// gate, so only while the in-game interface is up; [`drop_field_notices`] takes them otherwise.
pub(crate) fn fire_field_notices(
    script: Option<NonSendMut<UiScript>>,
    notices: Res<Messages<FieldNotice>>,
    mut cursor: ResMut<FieldNoticeCursor>,
    mut sink: crate::ui_action::MessageSink,
    // The quest log's watcher reads the slots.
    me: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    let fresh: Vec<FieldNotice> = cursor.0.read(&notices).copied().collect();
    // No VM, nobody to hear them: they are dropped, as below.
    let Some(mut script) = script else {
        return;
    };
    for notice in fresh {
        if notice == FieldNotice::QuestLog {
            if let Some(store) = me.iter().next() {
                script.quest_log_changed(crate::ui_quest_log::slot_views(&store.0));
            }
            continue;
        }
        fire(&mut script, &mut sink, notice);
    }
}

/// The field notices' second reader and its position: [`drop_field_notices`] under the exact
/// complement of the unit feed's gate, so a notice is fired or dropped the frame it is written,
/// never left to age in the buffer.
pub(super) fn add_notice_drop(app: &mut App) {
    app.add_message::<FieldNotice>()
        .init_resource::<FieldNoticeCursor>()
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
) {
    cursor.0.clear(&notices);
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
        // Fired by [`fire_field_notices`], which holds the slots.
        FieldNotice::QuestLog => {}
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

/// `0x5de4b0`: the event its registration names, `arg1 = "player"`.
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
        id::PLAYER_UPDATE_RESTING => "PLAYER_UPDATE_RESTING",
        id::PET_UI_UPDATE => "PET_UI_UPDATE",
        id::PLAYTIME_CHANGED => "PLAYTIME_CHANGED",
        _ => return None,
    })
}

/// The event-table name of an id `0x5dd8a0` registers `0x5de4b0` with.
fn player_event(id: u16) -> Option<&'static str> {
    Some(match id {
        149 => "UNIT_RESISTANCES",
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
        for id in [149, 189, 201, 372, 406, 523, 524] {
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
            .init_resource::<crate::sound::MessageSounds>();
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
                ]
            ),
            [
                "PLAYER_REGEN_DISABLED nil",
                "UI_INFO_MESSAGE on",
                "PET_ATTACK_STOP nil",
                "UI_INFO_MESSAGE off",
            ]
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_chat::ChatLog>()
                .pending_lines(),
            ["on, verbose", "off, verbose"]
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
                crate::ui_pet_doll::feed_pet_doll,
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
        ];
        assert_eq!(
            ahead, [true; 8],
            "feed_units, char, pet doll, honor, reputation, party, pet, pet stats"
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
            &edges,
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
                    r"Interface\FrameXML\UIParent.xml",
                    r"Interface\FrameXML\MoneyFrame.lua",
                    r"Interface\FrameXML\MoneyFrame.xml",
                    r"Interface\FrameXML\GameTooltip.xml",
                    r"Interface\FrameXML\UIPanelTemplates.lua",
                    r"Interface\FrameXML\UIPanelTemplates.xml",
                    r"Interface\FrameXML\OptionsFrameTemplates.xml",
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
