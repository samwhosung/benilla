//! The guild roster as a listener reads it, packet by packet. Each guild packet's handler writes
//! the roster module's cells and signals inside itself: the roster message (`0x4d0ad0`) ends in
//! `GUILD_ROSTER_UPDATE` (`0x4d1160`), the guild event (`0x5e7180`) signals `GUILD_MOTD`
//! (`0x5e728d`) and then the update with arg1 (`0x5e74c4`), and the invitation (`0x5e6f20`)
//! signals `GUILD_INVITE_REQUEST` (`0x48f45c`). The getters read the cells at the call:
//! `GetNumGuildMembers` (`0x4d1190`), `GetGuildRosterInfo` (`0x4d1200`) with the guild's record
//! looked up at each call (`0x560e30`), `GetGuildRosterMOTD` (`0x4d11f0`), `GuildControlGetRankName`
//! (`0x4d1e90`), `GetGuildInfo` (`0x4c9330`). So two guild packets in one frame signal twice, and
//! each listener reads its own packet's roster.

use benilla_protocol::messages::{
    guild_event, guild_presence, GuildEventNotice, GuildQueryResponse, GuildRoster,
    GuildRosterMember,
};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// Our guild and the rank we hold in it.
const GUILD: u32 = 7;
const RANK: u32 = 1;
/// `PLAYER_GUILDID` and `PLAYER_GUILDRANK`.
const GUILD_ID: u16 = 191;
const GUILD_RANK: u16 = 192;
/// `UNIT_FIELD_HEALTH`, `UNIT_FIELD_MAXHEALTH`.
const HEALTH: u16 = 22;
const MAX_HEALTH: u16 = 28;

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the snapshots alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// The guild queries sent since the last call, by guild id.
    fn queries(&self) -> Vec<u32> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::GuildQuery { guild_id } => Some(guild_id),
                _ => None,
            })
            .collect()
    }
}

fn member(guid: u64, name: &str, rank_id: u32, presence: u8) -> GuildRosterMember {
    GuildRosterMember {
        guid,
        presence,
        name: name.to_string(),
        rank_id,
        level: 60,
        ..Default::default()
    }
}

fn roster(motd: &str, members: Vec<GuildRosterMember>) -> SessionEvent {
    SessionEvent::GuildRoster(GuildRoster {
        motd: motd.to_string(),
        rank_rights: vec![0xffff, 0x0010, 0x0000],
        members,
        ..Default::default()
    })
}

fn record(guild_id: u32, name: &str) -> SessionEvent {
    let mut rank_names: [String; 10] = Default::default();
    for (slot, rank) in rank_names.iter_mut().zip(["Master", "Officer", "Member"]) {
        *slot = rank.to_string();
    }
    SessionEvent::GuildQueryResponse(GuildQueryResponse {
        guild_id,
        name: name.to_string(),
        rank_names,
        ..Default::default()
    })
}

fn event(event: u8, params: &[&str]) -> SessionEvent {
    SessionEvent::GuildEvent(GuildEventNotice {
        event,
        params: params.iter().map(|p| (*p).to_string()).collect(),
        guid: None,
    })
}

/// The client in the world as [`ME`], in [`GUILD`] at [`RANK`], and a spy whose line for each
/// guild event carries what its listener read: the roster's size, its first row's name, the
/// message of the day and the first rank's name. The guild's record is not cached, and `twat` is
/// on the profanity list.
fn client() -> Client {
    let mut c = world();
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (HEALTH, 100),
            (MAX_HEALTH, 100),
            (GUILD_ID, GUILD),
            (GUILD_RANK, RANK),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen();
    // The asks the player's create left in flight (`0x5dec19`) are forgotten, as a world entry
    // forgets them: the getters' own lookups are what these tests watch.
    c.app
        .world_mut()
        .resource_mut::<crate::ui_guild::GuildState>()
        .drop_pending_for_test();
    c.queries();
    c
}

/// [`client`] before our player is created.
fn world() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"GUILD_ROSTER_UPDATE", "GUILD_MOTD", "GUILD_INVITE_REQUEST"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1) .. ":" .. tostring(arg2)
                    .. ":" .. GetNumGuildMembers(true) .. ":" .. tostring((GetGuildRosterInfo(1)))
                    .. ":" .. GetGuildRosterMOTD() .. ":" .. tostring(GuildControlGetRankName(1)))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    app.insert_resource(crate::text_filter::TextFilter::new(
        &[benilla_formats::FilterPattern {
            id: 1,
            pattern: "twat".into(),
        }],
        &[],
    ));
    Client { app, inbound, sent }
}

/// A client whose guild record is cached.
fn named() -> Client {
    let mut c = client();
    c.frame(vec![vec![record(GUILD, "Legacy")]]);
    c.seen();
    c
}

/// Two roster messages in one frame: each handler writes the cells and signals inside itself, so
/// the update fires twice, and each listener reads its own message's roster, the second's
/// message of the day and its first row, the higher rank first (`0x4d0d77`).
#[test]
fn two_roster_packets_signal_twice_each_as_its_packet_left_the_roster() {
    let on = guild_presence::ONLINE;
    let mut c = named();
    c.frame(vec![
        vec![roster("one", vec![member(1, "Alder", 1, on)])],
        vec![roster(
            "two",
            vec![member(1, "Alder", 1, on), member(2, "Birch", 2, on)],
        )],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:one:Master",
            "GUILD_ROSTER_UPDATE:nil:nil:2:Birch:two:Master",
        ]
    );
}

/// The message-of-the-day event signals `GUILD_MOTD` with the text and then the update with arg1
/// (`0x5e728d`, `0x5e74c4`), between two roster messages: the event's text goes to its listener
/// and is stored nowhere (`0xb72f00` is the roster message's alone, `0x4d0b6d`), so
/// `GetGuildRosterMOTD` reads the roster message's.
#[test]
fn a_motd_event_between_two_rosters_reads_its_own_text() {
    let on = guild_presence::ONLINE;
    let mut c = named();
    c.frame(vec![
        vec![roster("one", vec![member(1, "Alder", 1, on)])],
        vec![event(guild_event::MOTD, &["two"])],
        vec![roster("three", vec![member(1, "Alder", 1, on)])],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:one:Master",
            "GUILD_MOTD:two:nil:1:Alder:one:Master",
            "GUILD_ROSTER_UPDATE:1:nil:1:Alder:one:Master",
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:three:Master",
        ]
    );
}

/// Two invitations in one frame signal the popup twice, each with its inviter and guild
/// (`0x5e6f20` → `0x48f450`); a roster message that carries a new message of the day signals
/// no `GUILD_MOTD` of its own.
#[test]
fn each_invitation_signals_its_own_popup() {
    let mut c = named();
    c.frame(vec![
        vec![SessionEvent::GuildInvite {
            inviter: "Tigole".into(),
            guild: "Legacy".into(),
        }],
        vec![SessionEvent::GuildInvite {
            inviter: "Furor".into(),
            guild: "Fury".into(),
        }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_INVITE_REQUEST:Tigole:Legacy:0:nil::nil",
            "GUILD_INVITE_REQUEST:Furor:Fury:0:nil::nil",
        ]
    );
}

/// The roster getters look the guild's record up from the player's guild id at each call and fail
/// without it (`0x4d1291`, `0x4d1412`; `0x4d1f3e`, `0x4d1f80`; `0x4c93d2`): the nil row
/// `nil, nil, 0, 0, nil, nil, nil, nil, nil, ""`, a nil rank name, with the query sent once while
/// it is pending; the record landing fires the bare update (`0x4d1480`).
#[test]
fn a_roster_getter_fails_and_asks_until_the_guilds_record_lands() {
    let on = guild_presence::ONLINE;
    let mut c = client();
    c.frame(vec![vec![roster("one", vec![member(1, "Alder", 1, on)])]]);
    assert_eq!(
        c.seen(),
        ["GUILD_ROSTER_UPDATE:nil:nil:1:nil:one:nil"],
        "the row and the rank name are nil without the record"
    );
    assert_eq!(c.queries(), [GUILD], "and the call asked");
    let row: String = c.eval(
        true,
        "local function count(...) return arg.n end \
         local t = {GetGuildRosterInfo(1)} \
         return table.concat({tostring(t[1]), tostring(t[3]), tostring(t[4]), tostring(t[10]), \
         count(GetGuildRosterInfo(1))}, ',')",
    );
    assert_eq!(row, "nil,0,0,,10");
    assert!(c.queries().is_empty(), "the ask is pending: not sent again");

    // The record lands: its callback runs once for each call that missed (`0x4d1480`, `0x4d148c`
    // from `0x556f91`): the listener's two getters, and the two calls of the chunk above.
    c.frame(vec![vec![record(GUILD, "Legacy")]]);
    assert_eq!(
        c.seen(),
        ["GUILD_ROSTER_UPDATE:nil:nil:1:Alder:one:Master"; 4]
    );
    let rank: String = c.eval(true, "local _, rank = GetGuildRosterInfo(1) return rank");
    assert_eq!(rank, "Officer", "the member's rank name from the record");
}

/// A refused guild is asked again by the next call (`0x556f35`, then `0x556aa0`), and by nothing
/// else: the refusal itself signals nothing (every guild callback ignores status 0).
#[test]
fn a_refused_guild_is_asked_again_by_the_next_call() {
    let on = guild_presence::ONLINE;
    let mut c = client();
    c.frame(vec![vec![roster("one", vec![member(1, "Alder", 1, on)])]]);
    c.seen();
    assert_eq!(
        c.queries(),
        [GUILD],
        "the listener's getters ask once between them"
    );
    c.frame(vec![vec![record(GUILD, "")]]);
    assert!(c.seen().is_empty(), "the refusal signals nothing");
    assert!(c.queries().is_empty(), "nothing asks a refusal on its own");
    c.eval::<()>(true, "GetGuildRosterInfo(1)");
    assert_eq!(c.queries(), [GUILD], "the next call asks again");
}

/// `GetGuildInfo` for the player reads the descriptor's guild and rank and the record
/// (`0x4c9330`): the name, the rank's name and its index; a miss asks and answers `nil, nil, 0`.
#[test]
fn get_guild_info_reads_the_players_descriptor_and_the_record() {
    let mut c = client();
    let info = |c: &mut Client| -> String {
        c.eval(
            true,
            "local a, b, n = GetGuildInfo('player') return tostring(a) .. ',' .. tostring(b) \
             .. ',' .. tostring(n)",
        )
    };
    assert_eq!(info(&mut c), "nil,nil,0");
    assert_eq!(c.queries(), [GUILD], "the lookup asked");
    c.frame(vec![vec![record(GUILD, "Legacy")]]);
    assert_eq!(info(&mut c), "Legacy,Officer,1");
    // The descriptor moves: the next call reads the new rank.
    c.frame(vec![vec![values(ME, &[(GUILD_RANK, 2)])]]);
    assert_eq!(info(&mut c), "Legacy,Member,2");
}

/// The player's standing reads the descriptor and the rank's rights (`0x516de0`, `0x516e40`,
/// `0x4d19a0`): in a guild at rank 1, whose rights hold Invite and not Promote.
#[test]
fn the_players_standing_reads_the_descriptor_and_the_rank_rights() {
    let mut c = named();
    c.frame(vec![vec![roster("one", vec![])]]);
    let standing: String = c.eval(
        true,
        "return tostring(IsInGuild()) .. ',' .. tostring(IsGuildLeader()) .. ',' \
         .. tostring(CanGuildInvite()) .. ',' .. tostring(CanGuildPromote())",
    );
    assert_eq!(standing, "1,nil,1,nil");
    // Rank 0 is the guild master.
    c.frame(vec![vec![values(ME, &[(GUILD_RANK, 0)])]]);
    let standing: String = c.eval(
        true,
        "return tostring(IsGuildLeader()) .. ',' .. tostring(CanGuildPromote())",
    );
    assert_eq!(standing, "1,1");
    // Out of the guild every predicate and every rank flag answers nil.
    c.frame(vec![vec![values(ME, &[(GUILD_ID, 0)])]]);
    let standing: String = c.eval(
        true,
        "local function count(...) return arg.n end \
         GuildControlSetRank(1) local t = {GuildControlGetRankFlags()} \
         return tostring(IsInGuild()) .. ',' .. tostring(CanGuildInvite()) .. ',' \
         .. tostring(t[1]) .. ',' .. count(GuildControlGetRankFlags())",
    );
    assert_eq!(standing, "nil,nil,nil,13");
}

/// `GetGuildRosterLastOnline` answers four nils for an online member (`0x4d1502` → `0x4d15b2`)
/// and for a row past the roster, the decomposed time for an offline one (`0x4d1508`).
#[test]
fn last_online_is_nil_for_an_online_member() {
    let mut c = named();
    let mut gone = member(2, "Birch", 2, guild_presence::OFFLINE);
    gone.last_online_days = 400.5;
    c.frame(vec![vec![roster(
        "one",
        vec![member(1, "Alder", 1, guild_presence::ONLINE), gone],
    )]]);
    let last: Vec<String> = c.eval(
        true,
        "SetGuildRosterShowOffline(1)
         local function f(...)
             local t = {}
             for i = 1, arg.n do t[i] = tostring(arg[i]) end
             return table.concat(t, ',')
         end
         return {f(GetGuildRosterLastOnline(1)), f(GetGuildRosterLastOnline(2)),
                 f(GetGuildRosterLastOnline(9))}",
    );
    // Birch (rank 2) sorts first, offline: 400.5 days is 1 year, 1 month, 5 days, 12 hours.
    assert_eq!(last, ["1,1,5,12", "nil,nil,nil,nil", "nil,nil,nil,nil"]);
}

/// `/ginfo`'s answer prints its two lines inside its packet (`0x5e6fb0`), between the events of
/// the packets around it.
#[test]
fn the_ginfo_lines_print_at_their_packets_place() {
    let on = guild_presence::ONLINE;
    let mut c = named();
    c.eval::<()>(
        false,
        "GUILD_NAME_TEMPLATE = 'Guild: %s' GUILD_INFO_TEMPLATE = '%d/%d/%d %d %d'
         local f = CreateFrame('Frame')
         f:RegisterEvent('CHAT_MSG_SYSTEM')
         f:SetScript('OnEvent', function() table.insert(Seen, 'CHAT:' .. arg1) end)",
    );
    c.frame(vec![
        vec![roster("one", vec![member(1, "Alder", 1, on)])],
        vec![SessionEvent::GuildInfo(
            benilla_protocol::messages::GuildInfo {
                name: "Legacy".into(),
                created_day: 3,
                created_month: 4,
                created_year: 2005,
                member_count: 9,
                account_count: 8,
            },
        )],
        vec![roster("two", vec![member(1, "Alder", 1, on)])],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:one:Master",
            "CHAT:Guild: Legacy",
            "CHAT:4/3/2005 9 8",
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:two:Master",
        ]
    );
}

/// The guild event's message of the day is masked as it lands (`0x5e7277`, `dl = 0`: each text
/// advances the mask index), the roster's at its parse (`0x4d0b71`, `dl = 1`: a text masked
/// before is answered from the cache with the index where it was).
#[test]
fn the_message_of_the_day_is_masked_as_it_lands() {
    let mut c = named();
    c.frame(vec![vec![event(guild_event::MOTD, &["you twat"])]]);
    assert!(
        c.seen()[0].starts_with("GUILD_MOTD:you !@#$:"),
        "the event's text, masked, to its listener"
    );
    let motd = |c: &mut Client| -> String { c.eval(true, "return GetGuildRosterMOTD()") };
    c.frame(vec![vec![roster("twat", vec![])]]);
    assert_eq!(motd(&mut c), "%^&*", "a new text takes the next tokens");
    c.frame(vec![vec![roster("twat", vec![])]]);
    assert_eq!(
        motd(&mut c),
        "%^&*",
        "the same text again: the cache, no new tokens"
    );
    c.seen();
    c.frame(vec![vec![event(guild_event::MOTD, &["twat"])]]);
    let seen = c.seen();
    assert!(
        seen[0].starts_with("GUILD_MOTD:!@#$:"),
        "the index stood at the cycle's start: {seen:?}"
    );
}

/// The rank names the getters return are masked (`0x4d12fd`, `0x4d1f62`, `0x4c9402`, `dl = 1`),
/// and one text masked before comes back the same from the cache, whichever getter asks.
#[test]
fn a_profane_rank_name_comes_back_masked_and_the_same_each_time() {
    let on = guild_presence::ONLINE;
    let mut c = client();
    let mut rank_names: [String; 10] = Default::default();
    rank_names[1] = "twat".into();
    c.frame(vec![
        vec![SessionEvent::GuildQueryResponse(GuildQueryResponse {
            guild_id: GUILD,
            name: "Legacy".into(),
            rank_names,
            ..Default::default()
        })],
        vec![roster("one", vec![member(1, "Alder", 1, on)])],
    ]);
    let names: Vec<String> = c.eval(
        true,
        "local _, a = GetGuildRosterInfo(1) local _, b = GetGuildInfo('player') \
         return {a, GuildControlGetRankName(2), b}",
    );
    assert_eq!(names, ["!@#$", "!@#$", "!@#$"]);
}

/// A member's notes are masked by the first read of their list (`0x4d09d0`, `dl = 1`, public
/// note then officer note) and keep what that read left: the mark clears (`0x4d09f1`), so a list
/// read with `profanityFilter` off stays plain after it turns on, and one read with it on stays
/// masked, until the next roster message brings a new mark.
#[test]
fn a_guild_members_notes_are_masked_once_by_their_first_read() {
    let on = guild_presence::ONLINE;
    let noted = |public: &str, officer: &str| {
        let mut m = member(1, "Alder", 1, on);
        m.public_note = public.into();
        m.officer_note = officer.into();
        m
    };
    let notes = |c: &mut Client| -> Vec<String> {
        c.eval(
            true,
            "local _, _, _, _, _, _, a, b = GetGuildRosterInfo(1) return {a, b}",
        )
    };
    let mut c = named();
    c.frame(vec![vec![roster("one", vec![noted("twat", "you twat")])]]);
    assert_eq!(notes(&mut c), ["!@#$", "you %^&*"], "both notes, in order");
    assert_eq!(notes(&mut c), ["!@#$", "you %^&*"], "the same again");

    c.app
        .world_mut()
        .resource_mut::<crate::text_filter::TextFilterSwitches>()
        .profanity = false;
    assert_eq!(
        notes(&mut c),
        ["!@#$", "you %^&*"],
        "the mark is cleared: a list read once is not read again"
    );
    c.frame(vec![vec![roster("two", vec![noted("twat", "you twat")])]]);
    assert_eq!(
        notes(&mut c),
        ["twat", "you twat"],
        "a new list, read with the filter off"
    );
    c.app
        .world_mut()
        .resource_mut::<crate::text_filter::TextFilterSwitches>()
        .profanity = true;
    assert_eq!(notes(&mut c), ["twat", "you twat"], "and it stays plain");
    c.frame(vec![vec![roster("three", vec![noted("twat", "you twat")])]]);
    assert_eq!(
        notes(&mut c),
        ["!@#$", "you %^&*"],
        "a third list, read with it on: the cached masks of the same texts"
    );
}

/// `GetGuildRosterLastOnline` has no player gate (`0x4d14a0`), where `GetGuildRosterInfo` fails
/// with none (`0x4d125f`).
#[test]
fn last_online_answers_with_no_player_and_the_row_does_not() {
    let mut c = world();
    let mut gone = member(2, "Birch", 2, guild_presence::OFFLINE);
    gone.last_online_days = 400.5;
    c.frame(vec![vec![roster("one", vec![gone])]]);
    let got: Vec<String> = c.eval(
        true,
        "SetGuildRosterShowOffline(1) local a, b, c, d = GetGuildRosterLastOnline(1) \
         return {tostring(a), tostring(b), tostring(c), tostring(d), tostring((GetGuildRosterInfo(1)))}",
    );
    assert_eq!(got, ["1", "1", "5", "12", "nil"]);
}

/// Each roster listener reads its own message's roster, and the friends the packets ahead of it
/// left: a friend list between the two rosters, which `GetNumFriends` reads at the call
/// (`0x5ad000`), counts none for the first and Ann for the second.
#[test]
fn each_roster_listener_reads_its_own_span() {
    let on = guild_presence::ONLINE;
    let mut c = named();
    let ann = super::unit_view_tests::one_friend(&mut c.app);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("GUILD_ROSTER_UPDATE")
        g:SetScript("OnEvent", function() table.insert(Seen, "friends " .. GetNumFriends()) end)"#,
    );
    c.frame(vec![
        vec![roster("one", vec![member(1, "Alder", 1, on)])],
        vec![ann],
        vec![roster(
            "two",
            vec![member(1, "Alder", 1, on), member(2, "Birch", 2, on)],
        )],
    ]);
    assert_eq!(
        c.seen(),
        [
            "GUILD_ROSTER_UPDATE:nil:nil:1:Alder:one:Master",
            "friends 0",
            "GUILD_ROSTER_UPDATE:nil:nil:2:Birch:two:Master",
            "friends 1",
        ]
    );
}
