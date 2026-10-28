//! Party quest sharing: each member's verdict on a quest we shared, and the escort confirm a
//! member gets when someone starts a party-accept quest, with one server latch
//! (`Player::SetQuestShareInfo`) behind both. The receiving half needs nothing here: the receiver
//! gets a plain `SMSG_QUESTGIVER_QUEST_DETAILS` whose giver is the sharer (vmangos
//! `QuestHandler.cpp:454`), which [`crate::ui_quest`] tells from an NPC offer by the guid's type.
//!
//! Each verdict's line and the confirm's `QUEST_ACCEPT_CONFIRM(member, title)` go on the
//! questgiver lane at their packet ([`crate::ui_quest::QuestGiverLane`]); the stock popup's Yes
//! calls `ConfirmAcceptQuest()` (`UIParent.lua:353`) and No sends nothing.
//!
//! Deviation: a member's name not yet cached holds its verdict line for a bounded retry, shown at
//! the name's answer within [`NAME_WAIT_FRAMES`] and dropped after, and holds the escort confirm
//! with no bound until the name answers or another confirm replaces it, where the reference reads
//! its name cache (`0xc0e228` via `0x55f080`) with a null callback and drops the message on a
//! miss, because benilla's cache may not hold a party member yet.

use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
use bevy::prelude::*;

use crate::ui_action::UiError;
use crate::ui_quest::{QuestGiverEvent, QuestGiverLane};

/// Frames a held verdict line waits for its name: a name query is one round trip, and a guid
/// the server will not name would otherwise hold its line all session.
const NAME_WAIT_FRAMES: u32 = 120;

/// The share's waits on names: the verdicts on quests we pushed, and the escort confirm we owe an
/// answer, each line with the frame its packet came in.
#[derive(Resource, Default)]
pub(crate) struct QuestShare {
    /// `MSG_QUEST_PUSH_RESULT` lines whose member is not named yet, by member, in arrival order.
    verdicts: Vec<(u64, &'static str, u32)>,
    /// The confirm whose sender is not named yet, held with no bound; a second replaces the first,
    /// as the server keeps one latch.
    pending_confirm: Option<QuestConfirmAccept>,
}

/// Whether what came in at frame `at` still waits at frame `now`.
fn waiting(at: u32, now: u32) -> bool {
    now.wrapping_sub(at) < NAME_WAIT_FRAMES
}

impl QuestShare {
    /// Drops the lines that have waited out their bound.
    fn expire(&mut self, now: u32) {
        self.verdicts.retain(|&(_, _, at)| waiting(at, now));
    }

    /// On disconnect: the server's latch went with the session.
    pub(crate) fn clear_session(&mut self) {
        self.verdicts.clear();
        self.pending_confirm = None;
    }
}

/// The `ERR_QUEST_PUSH_*` key a verdict shows, named as vmangos `QuestDef.h:62-70` names them. The
/// reference's `0x276` arm (`0x5e4781`) maps 0..8 onto messages `0x181`-`0x189` in this order, all
/// `kind 0` (`0x487dc9`-`0x487e69`): `CHAT_MSG_SYSTEM` lines, never `UIErrorsFrame`. An unmapped
/// byte shows nothing.
fn verdict_message(msg: QuestShareMsg) -> Option<&'static str> {
    let key = match msg {
        QuestShareMsg::SHARING_QUEST => "ERR_QUEST_PUSH_SUCCESS_S",
        QuestShareMsg::CANT_TAKE_QUEST => "ERR_QUEST_PUSH_INVALID_S",
        QuestShareMsg::ACCEPT_QUEST => "ERR_QUEST_PUSH_ACCEPTED_S",
        QuestShareMsg::DECLINE_QUEST => "ERR_QUEST_PUSH_DECLINED_S",
        QuestShareMsg::TOO_FAR => "ERR_QUEST_PUSH_TOO_FAR_S",
        QuestShareMsg::BUSY => "ERR_QUEST_PUSH_BUSY_S",
        QuestShareMsg::LOG_FULL => "ERR_QUEST_PUSH_LOG_FULL_S",
        QuestShareMsg::HAVE_QUEST => "ERR_QUEST_PUSH_ONQUEST_S",
        QuestShareMsg::FINISH_QUEST => "ERR_QUEST_PUSH_ALREADY_DONE_S",
        _ => return None,
    };
    Some(key)
}

/// The quest-share packet handlers, each showing what it raised on the questgiver lane where it
/// runs.
mod net {
    use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use bevy::diagnostic::FrameCount;

    use super::{verdict_message, QuestShare};
    use crate::names::NameCache;
    use crate::net::{NetCommands, NetHandlerApp};
    use crate::query_cache::Ask;
    use crate::ui_quest::QuestGiverLane;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestPushResult, on_push_result)
            .net_handler(K::QuestConfirmAccept, on_confirm_accept)
            .net_handler(K::PlayerName, on_player_name)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The name cache, read as the module's deviation asks it, and the frame a wait starts at.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(super) struct Names<'w> {
        names: Res<'w, NameCache>,
        commands: Res<'w, NetCommands>,
        frame: Res<'w, FrameCount>,
    }

    impl Names<'_> {
        /// The member's name, asked for on a miss (the module's deviation).
        fn name(&self, guid: u64) -> Option<String> {
            self.names
                .resolve_player(guid, Ask::Event(&self.commands))
                .map(str::to_string)
        }
    }

    /// `MSG_QUEST_PUSH_RESULT` (`0x5e4781`): an unmapped verdict shows nothing; a named member's
    /// line shows at the packet, an unnamed one's waits for the name.
    fn on_push_result(
        In(ev): In<SessionEvent>,
        names: Names,
        mut share: ResMut<QuestShare>,
        mut lane: ResMut<QuestGiverLane>,
    ) {
        if let SessionEvent::QuestPushResult { member, msg } = ev {
            quest_push_result(member, msg, &names, &mut share, &mut lane);
        }
    }

    /// `SMSG_QUEST_CONFIRM_ACCEPT` (`0x5019a0`): the quest id stored at once (`0x5019c7`), then
    /// `QUEST_ACCEPT_CONFIRM` with the sender's name and the title (`0x5019fe`).
    fn on_confirm_accept(
        In(ev): In<SessionEvent>,
        names: Names,
        mut share: ResMut<QuestShare>,
        mut lane: ResMut<QuestGiverLane>,
        mut giver: ResMut<crate::ui_quest::QuestGiver>,
    ) {
        if let SessionEvent::QuestConfirmAccept(c) = ev {
            // The quest `ConfirmAcceptQuest` answers, written at the packet (`0x5019c7`) and
            // never cleared.
            giver.confirm = c.quest_id;
            quest_confirm_accept(c, &names, &mut share, &mut lane);
        }
    }

    /// The name cache's answer for a member a line or the confirm waits on: shown at the
    /// answer's packet with the name, a line only inside its bound; dropped with a refusal.
    fn on_player_name(
        In(ev): In<SessionEvent>,
        frame: Res<FrameCount>,
        mut share: ResMut<QuestShare>,
        mut lane: ResMut<QuestGiverLane>,
    ) {
        if let SessionEvent::PlayerName { guid, name, .. } = ev {
            share.name_answered(guid, &name, frame.0, &mut lane);
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut share: ResMut<QuestShare>) {
        share.clear_session();
    }

    fn quest_push_result(
        member: u64,
        msg: QuestShareMsg,
        names: &Names,
        share: &mut QuestShare,
        lane: &mut QuestGiverLane,
    ) {
        debug!(
            "net: quest push result — member {member:#x} verdict {}",
            msg.0
        );
        let Some(key) = verdict_message(msg) else {
            debug!("net: unmapped push verdict {} — no line", msg.0);
            return;
        };
        share.expire(names.frame.0);
        match names.name(member) {
            Some(name) => super::show_verdict(key, name, lane),
            None => share.verdicts.push((member, key, names.frame.0)),
        }
    }

    fn quest_confirm_accept(
        c: QuestConfirmAccept,
        names: &Names,
        share: &mut QuestShare,
        lane: &mut QuestGiverLane,
    ) {
        debug!(
            "net: quest confirm accept — quest {} ({:?}) from {:#x}",
            c.quest_id, c.title, c.sender
        );
        match names.name(c.sender) {
            Some(name) => super::show_confirm(name, c.title, lane),
            None => share.pending_confirm = Some(c),
        }
    }
}

/// A verdict's line, a `CHAT_MSG_SYSTEM` line with the member's name.
fn show_verdict(key: &'static str, name: String, lane: &mut QuestGiverLane) {
    lane.push(QuestGiverEvent::Line(UiError::s(key, name)));
}

/// `QUEST_ACCEPT_CONFIRM`: the player first, then the title (`%s%s`, `0x5019fe`).
fn show_confirm(name: String, title: String, lane: &mut QuestGiverLane) {
    lane.push(QuestGiverEvent::AcceptConfirm { name, title });
}

impl QuestShare {
    /// The name cache answered `guid` with `name` at frame `now`, empty for a refusal: what
    /// waited on it shows in arrival order, or goes; a line that waited out its bound is gone.
    fn name_answered(&mut self, guid: u64, name: &str, now: u32, lane: &mut QuestGiverLane) {
        self.expire(now);
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.verdicts)
            .into_iter()
            .partition(|&(member, _, _)| member == guid);
        self.verdicts = rest;
        let confirm = self.pending_confirm.take_if(|c| c.sender == guid);
        if name.is_empty() {
            return;
        }
        for (_, key, _) in mine {
            show_verdict(key, name.to_string(), lane);
        }
        if let Some(c) = confirm {
            show_confirm(name.to_string(), c.title, lane);
        }
    }
}

/// Owns [`QuestShare`] and its handlers; nothing here is bound to the questgiver window.
pub(crate) struct QuestSharePlugin;

impl Plugin for QuestSharePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<QuestShare>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::SessionEvent;
    use bevy::diagnostic::FrameCount;

    fn push(member: u64, msg: QuestShareMsg) -> SessionEvent {
        SessionEvent::QuestPushResult { member, msg }
    }

    fn named(guid: u64, name: &str) -> SessionEvent {
        SessionEvent::PlayerName {
            guid,
            name: name.into(),
            race: 1,
            gender: 0,
            class: 1,
        }
    }

    /// A named member's verdict and confirm go on the questgiver lane inside their packets' handlers;
    /// an unnamed one's ask once and wait for the name's answer, which shows them inside its own
    /// handler (the module's deviation); a refusal drops them, and the next packet asks again.
    #[test]
    fn a_verdict_shows_at_its_packet_or_at_its_names_answer() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(crate::net::NetCommands(tx))
            .init_resource::<crate::names::NameCache>()
            .init_resource::<QuestGiverLane>()
            .init_resource::<crate::ui_quest::QuestGiver>()
            .init_resource::<FrameCount>()
            .init_resource::<QuestShare>();
        net::register(&mut app);
        let world = app.world_mut();
        let taken = |world: &mut World| world.resource_mut::<QuestGiverLane>().take();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(0x9, "Mate".into(), None);
        let before = crate::lanes::stamp();
        crate::net::handlers::dispatch(world, vec![push(0x9, QuestShareMsg::SHARING_QUEST)]);
        let stamped = world.resource::<QuestGiverLane>().stamped();
        assert!(
            matches!(stamped.as_slice(), [(at, _)] if before < *at),
            "on the lane at its packet: {stamped:?}"
        );
        assert_eq!(
            taken(world),
            [QuestGiverEvent::Line(UiError::s(
                "ERR_QUEST_PUSH_SUCCESS_S",
                "Mate"
            ))]
        );
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "a hit asks nothing"
        );

        let confirm = SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
            quest_id: 5,
            title: "Escort".into(),
            sender: 0xa,
        });
        crate::net::handlers::dispatch(
            world,
            vec![push(0xa, QuestShareMsg::DECLINE_QUEST), confirm.clone()],
        );
        assert!(taken(world).is_empty(), "both wait for the name");
        assert_eq!(crate::query_cache::queries(&rx).len(), 1, "asked once");
        assert_eq!(world.resource::<crate::ui_quest::QuestGiver>().confirm, 5);
        crate::net::handlers::dispatch(world, vec![named(0xa, "Pal")]);
        assert_eq!(
            taken(world),
            [
                QuestGiverEvent::Line(UiError::s("ERR_QUEST_PUSH_DECLINED_S", "Pal")),
                QuestGiverEvent::AcceptConfirm {
                    name: "Pal".into(),
                    title: "Escort".into()
                },
            ]
        );

        crate::net::handlers::dispatch(world, vec![push(0xb, QuestShareMsg::BUSY)]);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(0xb, String::new(), None);
        crate::net::handlers::dispatch(world, vec![named(0xb, "")]);
        crate::net::handlers::dispatch(world, vec![named(0xb, "Late")]);
        assert!(taken(world).is_empty(), "a refusal drops the line");
        crate::net::handlers::dispatch(world, vec![push(0xb, QuestShareMsg::BUSY)]);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next packet asks again"
        );
    }

    /// A line whose name answers inside the bound shows, and one answered at the bound shows
    /// nothing; the confirm has no bound and fires at its name's answer however late (the
    /// module's deviation).
    #[test]
    fn a_line_answered_after_the_wait_shows_nothing_and_a_confirm_still_fires() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(crate::net::NetCommands(tx))
            .init_resource::<crate::names::NameCache>()
            .init_resource::<QuestGiverLane>()
            .init_resource::<crate::ui_quest::QuestGiver>()
            .init_resource::<FrameCount>()
            .init_resource::<QuestShare>();
        net::register(&mut app);
        let world = app.world_mut();
        let taken = |world: &mut World| world.resource_mut::<QuestGiverLane>().take();
        let at = |world: &mut World, frame: u32| world.resource_mut::<FrameCount>().0 = frame;
        let confirm = |sender| {
            SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
                quest_id: 5,
                title: "Escort".into(),
                sender,
            })
        };

        at(world, 10);
        crate::net::handlers::dispatch(world, vec![push(0xa, QuestShareMsg::BUSY), confirm(0xa)]);
        at(world, 10 + NAME_WAIT_FRAMES - 1);
        crate::net::handlers::dispatch(world, vec![named(0xa, "Pal")]);
        assert_eq!(
            taken(world).len(),
            2,
            "the last frame of the wait shows both"
        );

        at(world, 200);
        crate::net::handlers::dispatch(world, vec![push(0xb, QuestShareMsg::BUSY), confirm(0xb)]);
        at(world, 200 + NAME_WAIT_FRAMES);
        crate::net::handlers::dispatch(world, vec![named(0xb, "Late")]);
        assert_eq!(
            taken(world),
            [QuestGiverEvent::AcceptConfirm {
                name: "Late".into(),
                title: "Escort".into()
            }],
            "the line after its wait shows nothing; the confirm still fires"
        );
        assert!(
            world.resource::<QuestShare>().verdicts.is_empty(),
            "no line is held past the bound"
        );
    }

    #[test]
    fn every_wire_verdict_has_a_key_and_an_unknown_has_none() {
        for v in 0u8..=8 {
            assert!(
                verdict_message(QuestShareMsg(v)).is_some(),
                "verdict {v} unmapped"
            );
        }
        assert!(verdict_message(QuestShareMsg(9)).is_none());
        assert!(verdict_message(QuestShareMsg(0xFF)).is_none());
    }

    #[test]
    fn verdict_keys_are_distinct() {
        let mut keys: Vec<&str> = (0u8..=8)
            .filter_map(|v| verdict_message(QuestShareMsg(v)))
            .collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate ERR_QUEST_PUSH_* key");
    }

    #[test]
    fn every_verdict_key_resolves_and_names_a_member() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for raw in 0u8..=8 {
            let key = verdict_message(QuestShareMsg(raw)).expect("mapped");
            let text = g(key).unwrap_or_default();
            assert!(!text.is_empty(), "{key} (verdict {raw}) missing");
            assert!(text.contains("%s"), "{key} names no member: {text:?}");
        }

        // The opener the sharer sees on the click, and a decline's answer.
        let line = |raw: u8| {
            let key = verdict_message(QuestShareMsg(raw)).unwrap();
            crate::ui_action::ui_error_text(&UiError::s(key, "Mate"), &g)
        };
        assert_eq!(
            line(QuestShareMsg::SHARING_QUEST.0).as_deref(),
            Some("Sharing quest with Mate...")
        );
        assert_eq!(
            line(QuestShareMsg::DECLINE_QUEST.0).as_deref(),
            Some("Mate has declined your quest")
        );
    }
}
