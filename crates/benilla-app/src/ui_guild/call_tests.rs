//! The guild verbs through the whole client's host: each sends its packet, changes the roster's
//! order or answers its getters inside its call, as the reference's bindings do (`0x48aef0`,
//! `0x4d10d0`, `0x4d1cb0` → `0x4d0fb0`, `0x4d1e10` → `0x4d0f70`, `0x4d15e0`), and the roster's
//! status return reads the `CHAT_FLAG_*` globals at the call (`0x4d13ed`, `0x4d1404`).

use benilla_protocol::messages::{guild_presence, GuildRoster, GuildRosterMember};
use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;

struct Guild {
    app: App,
    rx: Receiver<ClientCommand>,
}

/// The whole client, headless, with a VM that counts `GUILD_ROSTER_UPDATE`, and our player in
/// guild 7, whose record is cached: the roster getters need both (`0x4d1200`).
fn client() -> Guild {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"UPDATES = 0
            local f = CreateFrame("Frame")
            f:RegisterEvent("GUILD_ROSTER_UPDATE")
            f:SetScript("OnEvent", function() UPDATES = UPDATES + 1 end)"#,
        )
        .expect("the counter installs");
    app.world_mut().insert_non_send_resource(script);
    // Fields 191 and 192: `PLAYER_GUILDID` and `PLAYER_GUILDRANK`.
    app.world_mut().spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (191, 7),
            (192, 0),
        ])),
    ));
    let mut guild = app.world_mut().resource_mut::<super::GuildState>();
    guild.join_for_test(7);
    guild.land_for_test(7, "Legacy");
    // The landing's own update (`0x4d1480`), fired and spent.
    let mut g = Guild { app, rx };
    fire(&mut g);
    run(&mut g, "UPDATES = 0");
    g
}

fn member(guid: u64, name: &str, presence: u8, rank_id: u32, level: u8) -> GuildRosterMember {
    GuildRosterMember {
        guid,
        presence,
        name: name.to_string(),
        rank_id,
        level,
        ..Default::default()
    }
}

/// `members` as `SMSG_GUILD_ROSTER`, its event fired, the counter reset.
fn roster(g: &mut Guild, members: Vec<GuildRosterMember>) {
    crate::net::handlers::dispatch(
        g.app.world_mut(),
        vec![SessionEvent::GuildRoster(GuildRoster {
            rank_rights: vec![0; 5],
            members,
            ..Default::default()
        })],
    );
    fire(g);
    run(g, "UPDATES = 0");
}

/// What the handlers raised, fired.
fn fire(g: &mut Guild) {
    super::lane::fire_for_test(g.app.world_mut());
}

fn run(g: &mut Guild, lua: &str) {
    with_hosted_vm(g.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(g: &mut Guild, lua: &str) -> T {
    with_hosted_vm(g.app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua))
        .expect("the chunk runs")
}

/// The packets sent since the last read, the name queries left out.
fn sent(g: &Guild) -> Vec<String> {
    g.rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.starts_with("NameQuery") && !c.starts_with("GuildQuery"))
        .collect()
}

/// Rank ids above level order: the default chain (`0x4d0a00`, slot `i` key `i`) sorts rank first,
/// the higher id first (`0x4d0d77`).
fn three_online(g: &mut Guild) {
    let on = guild_presence::ONLINE;
    roster(
        g,
        vec![
            member(1, "Alder", on, 2, 10),
            member(2, "Birch", on, 1, 30),
            member(3, "Cedar", on, 0, 20),
        ],
    );
}

/// `GuildInviteByName` sends at `0x48af64` and `GuildRoster` at `0x4d10d0`'s send, each inside
/// its call, so a chunk's packets leave in call order with `RandomRoll`'s (`0x48c832`).
#[test]
fn the_guild_verbs_send_inside_their_calls() {
    let mut g = client();
    run(
        &mut g,
        "GuildInviteByName('Bob') RandomRoll(1, 2) GuildRoster() GuildSetMOTD('hi') \
         GuildLeave()",
    );
    assert_eq!(
        sent(&g),
        [
            "GuildInvite { name: \"Bob\" }",
            "RandomRoll { min: 1, max: 2 }",
            "GuildRosterRequest",
            "GuildMotd { motd: \"hi\" }",
            "GuildLeave",
        ]
    );
}

/// `SortGuildRoster` (`0x4d1cb0` → `0x4d0fb0`) re-sorts the array and signals
/// `GUILD_ROSTER_UPDATE` before it returns (`0x4d1022`), so the chunk's next line reads the new
/// order and the handlers have run; nothing announces it again.
#[test]
fn a_sort_reorders_and_signals_inside_its_call() {
    let mut g = client();
    three_online(&mut g);
    assert_eq!(
        eval::<String>(&mut g, "return (GetGuildRosterInfo(2))"),
        "Birch"
    );
    run(
        &mut g,
        "SortGuildRoster('level') SECOND = GetGuildRosterInfo(2) SEEN = UPDATES",
    );
    assert_eq!(eval::<String>(&mut g, "return SECOND"), "Cedar");
    assert_eq!(eval::<i64>(&mut g, "return SEEN"), 1);
    fire(&mut g);
    assert_eq!(eval::<i64>(&mut g, "return UPDATES"), 1, "announced once");
}

/// `SetGuildRosterShowOffline` stores `GetScriptFlagArg`'s integer (`0x4d1e17`) and `0x4d0f70`
/// compares it raw (`cmp ecx,[0xb73124]`): a 2 after a 1 is a change, which re-sorts and signals
/// in the call (`0x4d0f87`-`0x4d0f9f`); an unchanged value does nothing.
#[test]
fn show_offline_compares_the_integer_and_signals_inside_its_call() {
    let mut g = client();
    let on = guild_presence::ONLINE;
    roster(
        &mut g,
        vec![
            member(1, "Alder", on, 0, 10),
            member(2, "Birch", guild_presence::OFFLINE, 0, 30),
        ],
    );
    run(
        &mut g,
        "SetGuildRosterShowOffline(1) A = GetNumGuildMembers() \
         SetGuildRosterShowOffline(2) SetGuildRosterShowOffline(2) SEEN = UPDATES \
         SetGuildRosterShowOffline(0) B = GetNumGuildMembers()",
    );
    assert_eq!(eval::<i64>(&mut g, "return A"), 2);
    assert_eq!(eval::<i64>(&mut g, "return SEEN"), 2, "0 to 1, then 1 to 2");
    assert_eq!(eval::<i64>(&mut g, "return B"), 1);
    assert_eq!(
        eval::<Option<i64>>(
            &mut g,
            "SetGuildRosterShowOffline(2) return GetGuildRosterShowOffline()"
        ),
        Some(1),
        "the getter answers 1 or nil (`0x4d1e30`)"
    );
}

/// With no member held `0x4d0f70` stores the flag and returns before the sort and the signal
/// (`0x4d0f85`), and nothing later announces it.
#[test]
fn show_offline_on_an_empty_roster_signals_nothing() {
    let mut g = client();
    run(&mut g, "UPDATES = 0 SetGuildRosterShowOffline(1)");
    fire(&mut g);
    assert_eq!(eval::<i64>(&mut g, "return UPDATES"), 0);
    assert_eq!(
        eval::<Option<i64>>(&mut g, "return GetGuildRosterShowOffline()"),
        Some(1)
    );
}

/// `GetGuildRosterInfo`'s tenth return is `FrameScript_GetText` of `CHAT_FLAG_DND` for presence
/// bit 4 (`0x4d13ed`), else of `CHAT_FLAG_AFK` for bit 2 (`0x4d1404`), read at the call: an addon's
/// own string shows, and a missing one is the empty string (`0x703bf0`).
#[test]
fn the_roster_status_reads_the_chat_flag_globals_at_the_call() {
    let mut g = client();
    let on = guild_presence::ONLINE;
    roster(
        &mut g,
        vec![
            member(1, "Alder", on | guild_presence::AFK, 0, 10),
            member(2, "Birch", on | guild_presence::DND, 0, 20),
        ],
    );
    let status = |g: &mut Guild, name: &str| -> String {
        eval(
            g,
            &format!(
                "for i = 1, 2 do local n = GetGuildRosterInfo(i) \
                 if n == '{name}' then \
                 local _, _, _, _, _, _, _, _, _, status = GetGuildRosterInfo(i) \
                 return status end end"
            ),
        )
    };
    run(&mut g, "CHAT_FLAG_AFK = '<Away>' CHAT_FLAG_DND = nil");
    assert_eq!(status(&mut g, "Alder"), "<Away>");
    assert_eq!(status(&mut g, "Birch"), "");
    run(&mut g, "CHAT_FLAG_DND = '<Busy>'");
    assert_eq!(status(&mut g, "Birch"), "<Busy>");
}

/// `GuildRosterSetPublicNote` (`0x4d15e0`) compares the new text with the stored note
/// (`0x64a480` at `0x4d1668`) and sends `CMSG_GUILD_SET_PUBLIC_NOTE` inside the call; it writes
/// nothing locally, so the roster keeps the old note until the server's next roster, and the same
/// text sends again.
#[test]
fn a_note_sends_inside_its_call_and_leaves_the_stored_note() {
    let mut g = client();
    let mut alder = member(1, "Alder", guild_presence::ONLINE, 0, 10);
    alder.public_note = "old".into();
    roster(&mut g, vec![alder]);
    run(&mut g, "GuildRosterSetPublicNote(1, 'new')");
    assert_eq!(
        sent(&g),
        ["GuildSetPublicNote { name: \"Alder\", note: \"new\" }"]
    );
    fire(&mut g);
    assert_eq!(
        eval::<String>(
            &mut g,
            "local _, _, _, _, _, _, note = GetGuildRosterInfo(1) return note"
        ),
        "old"
    );
    run(
        &mut g,
        "GuildRosterSetPublicNote(1, 'new') GuildRosterSetPublicNote(1, 'old')",
    );
    assert_eq!(
        sent(&g),
        ["GuildSetPublicNote { name: \"Alder\", note: \"new\" }"],
        "the stored text still differs from the first; the second matches it"
    );
}

/// Swap in a fresh VM as the lifecycle does, a `/reload`'s (`reload`) or a world entry's.
fn new_vm(g: &mut Guild, reload: bool) {
    let world = g.app.world_mut();
    super::fold_dying_vm_guild(world);
    let mut script = UiScript::new().expect("a VM");
    super::seed_vm_guild(world, &mut script, reload);
    world.insert_non_send_resource(script);
}

/// A world entry runs `InitGuildRoster` from `InitializeGame` (`0x48f5ea` → `0x4d0a00`), which
/// zeroes the request deadline (`0x4d0a41`) and seeds the sort chain afresh (`0x4d0a50`); a
/// `/reload` (`0x495664`, `0x495669`) runs neither, so both stand across it.
#[test]
fn a_login_resets_the_sort_and_the_request_deadline_and_a_reload_keeps_them() {
    let mut g = client();
    three_online(&mut g);
    run(&mut g, "SortGuildRoster('level') GuildRoster()");
    assert_eq!(sent(&g), ["GuildRosterRequest"]);
    let second = |g: &mut Guild| eval::<String>(g, "return (GetGuildRosterInfo(2))");

    new_vm(&mut g, true);
    assert_eq!(second(&mut g), "Cedar", "the reload keeps the level sort");
    run(&mut g, "GuildRoster()");
    assert!(sent(&g).is_empty(), "and the deadline");

    new_vm(&mut g, false);
    three_online(&mut g);
    assert_eq!(
        second(&mut g),
        "Birch",
        "the login's chain sorts by rank again"
    );
    run(&mut g, "GuildRoster()");
    assert_eq!(sent(&g), ["GuildRosterRequest"], "and allows the request");
}

/// `InitGuildRoster` also zeroes the member, online and rank counts (`0x4d0a13`-`0x4d0a1f`), so a
/// relog's VM holds no roster and no ranks until the server sends them; a `/reload`'s keeps both.
#[test]
fn a_login_empties_the_roster_and_the_ranks_and_a_reload_keeps_them() {
    let mut g = client();
    three_online(&mut g);
    let counts = |g: &mut Guild| -> (i64, i64) {
        eval(g, "return GetNumGuildMembers(), GuildControlGetNumRanks()")
    };
    assert_eq!(counts(&mut g), (3, 5));
    new_vm(&mut g, true);
    assert_eq!(counts(&mut g), (3, 5), "the reload keeps them");
    new_vm(&mut g, false);
    assert_eq!(
        counts(&mut g),
        (0, 0),
        "the login's InitGuildRoster zeroed them"
    );
}
