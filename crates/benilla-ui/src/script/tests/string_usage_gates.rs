//! Global verbs that gate a string argument with `is-number-or-string` (`0x6f3510`) and raise their
//! own literal through `luaL_error` (`0x6f4940`) on a miss: a number passes as its text, while nil,
//! a missing argument, a boolean or a table raises the verb's line, never mlua's conversion text.

use super::common::script;
use crate::script::{ChannelCommand, UiScript};

fn raised(s: &UiScript, call: &str) -> String {
    s.eval::<String>(&format!(
        "local ok, e = pcall(function() {call} end) \
         if ok then return \"<no raise>\" end return tostring(e)"
    ))
    .unwrap()
}

/// Each call raises its line; every miss is listed.
fn assert_raises(s: &UiScript, cases: &[(&str, &str)]) {
    let misses: Vec<String> = cases
        .iter()
        .filter_map(|(call, line)| {
            let got = raised(s, call);
            (!got.contains(line)).then(|| format!("`{call}` must raise `{line}`, got: {got}"))
        })
        .collect();
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}

/// `GetChannelName` `0x4a0693` (`0x844cf0`), `GetChatTypeIndex` `0x4a0a9c` (`0x844d54`), `DoEmote`
/// `0x49fe24` (`0x844bc0`), `DropItemOnUnit` `0x48d975` (`0x842cd4`), `FollowUnit` `0x489ea7`
/// (`0x8426b4`), `FollowByName` `0x489fd5` (`0x8426d0`), `GetAuctionItemInfo` `0x4cf270`
/// (`0x849448`) and `GetAuctionItemLink` `0x4cf44d` (`0x849474`).
#[test]
fn chat_follow_and_auction_verbs_raise_their_usage_line() {
    let mut s = script();
    s.keep_calls();
    const CHANNEL: &str = "Usage: GetChannelName([channelIndex] or [channelName])";
    const EMOTE: &str = r#"Usage: DoEmote("emote"[, "target"])"#;
    const DROP: &str = r#"Usage: DropItemOnUnit("unit")"#;
    const INFO: &str = r#"Usage: GetAuctionItemInfo("type", index)"#;
    const LINK: &str = r#"Usage: GetAuctionItemLink("type", index)"#;
    assert_raises(
        &s,
        &[
            ("GetChannelName()", CHANNEL),
            ("GetChannelName(nil)", CHANNEL),
            ("GetChannelName(true)", CHANNEL),
            ("GetChannelName({})", CHANNEL),
            ("GetChatTypeIndex()", "Usage: GetChatTypeIndex(type)"),
            ("GetChatTypeIndex({})", "Usage: GetChatTypeIndex(type)"),
            ("DoEmote()", EMOTE),
            ("DoEmote(false, 'Bob')", EMOTE),
            ("DropItemOnUnit()", DROP),
            ("DropItemOnUnit({})", DROP),
            ("FollowUnit(nil)", r#"Usage: FollowUnit("unit")"#),
            ("FollowUnit(true)", r#"Usage: FollowUnit("unit")"#),
            ("FollowByName()", r#"Usage: FollowByName("name")"#),
            ("FollowByName({}, 1)", r#"Usage: FollowByName("name")"#),
            ("GetAuctionItemInfo(nil, 1)", INFO),
            ("GetAuctionItemInfo(true, 1)", INFO),
            ("GetAuctionItemInfo('list', nil)", INFO),
            ("GetAuctionItemLink(nil, 1)", LINK),
            ("GetAuctionItemLink({}, 1)", LINK),
        ],
    );
    // A string or a number passes the gate.
    s.run(
        r#"
        local n, name, inst = GetChannelName("General")
        assert(n == 0 and name == nil and inst == 0)
        assert(GetChannelName(3) == 0)
        assert(GetChatTypeIndex(5) == 0)
        DoEmote("")
        DoEmote(5)
        local info = { GetAuctionItemInfo(1, 1) }
        assert(info[3] == 1 and info[4] == -1)
        assert(GetAuctionItemLink("list", 1) == nil)
    "#,
    )
    .unwrap();
    let emotes = s.take_emote_requests();
    assert_eq!(emotes.len(), 1, "only DoEmote(5) queues: {emotes:?}");
    assert_eq!(emotes[0].token, "5");
}

/// The channel admin helpers `0x4a00f0` and `0x4a04b0` gate each argument (`0x4a0105`, `0x4a04c6`,
/// `0x4a04da`) and raise `Usage: %s("channel")` (`0x844c7c`) or `Usage: %s("channel", "name")`
/// (`0x844cd0`) with the verb's name; `SetChannelPassword` raises its own line (`0x844ca0`).
#[test]
fn channel_admin_verbs_raise_their_usage_line_and_send_strings_as_given() {
    let mut s = script();
    s.keep_calls();
    const PASSWORD: &str = r#"Usage: SetChannelPassword("name", "password")"#;
    assert_raises(
        &s,
        &[
            (
                "ListChannelByName()",
                r#"Usage: ListChannelByName("channel")"#,
            ),
            (
                "DisplayChannelOwner(nil)",
                r#"Usage: DisplayChannelOwner("channel")"#,
            ),
            (
                "ChannelModerate({})",
                r#"Usage: ChannelModerate("channel")"#,
            ),
            (
                "ChannelToggleAnnouncements(true)",
                r#"Usage: ChannelToggleAnnouncements("channel")"#,
            ),
            (
                "ChannelBan('General')",
                r#"Usage: ChannelBan("channel", "name")"#,
            ),
            (
                "ChannelKick(nil, 'Bob')",
                r#"Usage: ChannelKick("channel", "name")"#,
            ),
            (
                "SetChannelOwner('General', {})",
                r#"Usage: SetChannelOwner("channel", "name")"#,
            ),
            (
                "ChannelUnban()",
                r#"Usage: ChannelUnban("channel", "name")"#,
            ),
            ("SetChannelPassword('General')", PASSWORD),
            ("SetChannelPassword(true, 'pw')", PASSWORD),
        ],
    );
    assert!(
        s.take_channel_commands().is_empty(),
        "a raise sends nothing"
    );
    // "" and padding go on as given; a number is its text (`ParseChannelKey` reads it later).
    s.run(
        r#"
        ListChannelByName("")
        ChannelBan(" General ", " Bob")
        SetChannelPassword(1, "")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.take_channel_commands(),
        vec![
            ChannelCommand::List {
                name: String::new()
            },
            ChannelCommand::Ban {
                name: " General ".into(),
                player: " Bob".into()
            },
            ChannelCommand::SetPassword {
                name: "1".into(),
                password: String::new()
            },
        ]
    );
}

/// `GetGuildInfo` raises `Usage: GetGuildInfo("unit")` (`0x848ca0`, `0x4c935b`),
/// `GetNumAuctionItems` `Usage: GetNumAuctionItems("type")` (`0x849424`, `0x4ced37`), and
/// `GetInventorySlotInfo` takes its unknown name's raise (`0x848894`, `0x4c81ca` → `0x4c822c`).
#[test]
fn guild_slot_and_auction_count_verbs_raise_their_own_text() {
    let s = script();
    const GUILD: &str = r#"Usage: GetGuildInfo("unit")"#;
    const SLOT: &str = "Invalid inventory slot in GetInventorySlotInfo";
    const COUNT: &str = r#"Usage: GetNumAuctionItems("type")"#;
    assert_raises(
        &s,
        &[
            ("GetGuildInfo()", GUILD),
            ("GetGuildInfo(nil)", GUILD),
            ("GetGuildInfo({})", GUILD),
            ("GetInventorySlotInfo()", SLOT),
            ("GetInventorySlotInfo(true)", SLOT),
            ("GetInventorySlotInfo('NoSlot')", SLOT),
            ("GetNumAuctionItems()", COUNT),
            ("GetNumAuctionItems({})", COUNT),
        ],
    );
    s.run(
        r#"
        local name, rank, index = GetGuildInfo("player")
        assert(name == nil and rank == nil and index == 0)
        assert(GetInventorySlotInfo("headslot") == 1)
        local batch, total = GetNumAuctionItems(5)
        assert(batch == 0 and total == 0)
    "#,
    )
    .unwrap();
}

/// The guild verbs raise their own usage line on a non-string: `GuildUninviteByName` `0x48b03b`,
/// `GuildPromoteByName` `0x48b0db`, `GuildDemoteByName` `0x48b17b`, `GuildSetLeaderByName`
/// `0x48b24f`, `GuildSetMOTD` `0x48b30f`, `SetGuildInfoText` `0x4d2397`, `GuildControlSaveRank`
/// `0x4d20e8`, `GuildControlAddRank` `0x4d2228` and the two roster notes `0x4d16e3`, `0x4d1803`.
#[test]
fn guild_verbs_raise_their_usage_line() {
    let s = script();
    assert_raises(
        &s,
        &[
            (
                "GuildUninviteByName(nil)",
                r#"Usage: GuildUninviteByName("name")"#,
            ),
            (
                "GuildPromoteByName()",
                r#"Usage: GuildPromoteByName("name")"#,
            ),
            (
                "GuildDemoteByName({})",
                r#"Usage: GuildDemoteByName("name")"#,
            ),
            (
                "GuildSetLeaderByName(true)",
                r#"Usage: GuildSetLeaderByName("name")"#,
            ),
            ("GuildSetMOTD()", r#"Usage: GuildSetMOTD("message")"#),
            ("SetGuildInfoText(nil)", "Usage: SetGuildInfoText(text)"),
            (
                "GuildControlSaveRank()",
                "Usage: GuildControlSaveRank(name)",
            ),
            (
                "GuildControlAddRank({})",
                "Usage: GuildControlAddRank(name)",
            ),
            (
                "GuildRosterSetPublicNote(1)",
                "Usage: GuildRosterSetPublicNote(index, note)",
            ),
            (
                "GuildRosterSetPublicNote('x', 'note')",
                "Usage: GuildRosterSetPublicNote(index, note)",
            ),
            (
                "GuildRosterSetOfficerNote(nil, 'note')",
                "Usage: GuildRosterSetOfficerNote(index, note)",
            ),
        ],
    );
}

/// `GuildInviteByName` (`0x48aef0`) shows ERR_COMMAND_NEEDS_TARGET for a non-string, nil or ""
/// (`0x48af06`, `0x48af1c`, `0x48af20` → `push 0x132`) and raises nothing, as an empty
/// ADD_GUILDMEMBER box calls it with "" (`StaticPopup.lua:847`). The other names go on as given,
/// and `GuildSetLeaderByName` sends the packet for "" too (`0x48b220`).
#[test]
fn guild_invite_without_a_name_shows_needs_target_and_names_go_as_given() {
    use crate::script::ui_errors::shown::seen_in_call;
    use crate::script::{GuildCall, ScriptCall};
    let mut s = script();
    for call in [
        "GuildInviteByName()",
        "GuildInviteByName(nil)",
        "GuildInviteByName('')",
        "GuildInviteByName({})",
    ] {
        let (seen, host) = seen_in_call(&mut s, call);
        assert_eq!(seen, "ERR_COMMAND_NEEDS_TARGET", "{call}");
        assert!(host.calls.is_empty(), "{call} sends nothing");
    }
    let (seen, host) = seen_in_call(
        &mut s,
        "GuildInviteByName(' ') GuildUninviteByName('') GuildPromoteByName(' Bob') \
         GuildSetLeaderByName('') GuildSetMOTD('')",
    );
    assert_eq!(seen, "");
    let guild: Vec<GuildCall> = host
        .calls
        .into_iter()
        .filter_map(|c| match c {
            ScriptCall::Guild(g) => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(
        guild,
        vec![
            GuildCall::Invite(" ".into()),
            GuildCall::Uninvite(String::new()),
            GuildCall::Promote(" Bob".into()),
            GuildCall::SetLeader(String::new()),
            GuildCall::SetMotd(String::new()),
        ]
    );
}

/// `NotifyInspect` (`0x48a320`) raises `Usage: NotifyInspect(unit)` for a non-string (`0x842724`,
/// `0x48a38b`), returns with no player object (`0x48a35a`), and then hands the token to the
/// resolver (`0x48a36a` → `0x515970`), which raises `Unknown unit name: %s` (`0x850cc4`) for one
/// it does not know, a number as text included.
#[test]
fn notify_inspect_raises_its_usage_line_then_gates_on_the_player_then_the_token() {
    let mut s = script();
    s.keep_calls();
    const USAGE: &str = "Usage: NotifyInspect(unit)";
    assert_raises(
        &s,
        &[
            ("NotifyInspect()", USAGE),
            ("NotifyInspect(nil)", USAGE),
            ("NotifyInspect({})", USAGE),
        ],
    );
    // No player: even an unknown token returns quietly and sends nothing.
    s.run(r#"NotifyInspect("bogus") NotifyInspect("target")"#)
        .unwrap();
    assert_eq!(s.take_calls_like(&notify("bogus")), 0);
    assert_eq!(s.take_calls_like(&notify("target")), 0);
    s.set_unit(
        "player",
        Some(crate::script::UnitState {
            exists: true,
            ..Default::default()
        }),
    );
    assert_raises(
        &s,
        &[
            (r#"NotifyInspect("bogus")"#, "Unknown unit name: bogus"),
            ("NotifyInspect(5)", "Unknown unit name: 5"),
        ],
    );
    s.run(r#"NotifyInspect("target")"#).unwrap();
    assert_eq!(s.take_calls_like(&notify("target")), 1);
}

fn notify(token: &str) -> crate::script::ScriptCall {
    crate::script::ScriptCall::Inspect(crate::script::InspectCall::Notify(token.into()))
}

/// `GuildSetLeaderByName` (`0x48b190`) writes a name only when it is neither empty (`0x48b1e1`) nor
/// `target` in any case (`SStrCmpI` against `0x8428d8`, `0x48b1fc`), as `/gleader` passes the word
/// when a player is targeted (`ChatFrame.lua:650-657`); the packet goes either way (`0x48b220`).
#[test]
fn guild_set_leader_sends_no_name_for_target_in_any_case() {
    use crate::script::ui_errors::shown::seen_in_call;
    use crate::script::{GuildCall, ScriptCall};
    let mut s = script();
    let (_, host) = seen_in_call(
        &mut s,
        "GuildSetLeaderByName('target') GuildSetLeaderByName('TaRgEt') GuildSetLeaderByName('Bob') \
         GuildSetLeaderByName(' target') GuildSetLeaderByName('targets')",
    );
    let guild: Vec<GuildCall> = host
        .calls
        .into_iter()
        .filter_map(|c| match c {
            ScriptCall::Guild(g) => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(
        guild,
        vec![
            GuildCall::SetLeader(String::new()),
            GuildCall::SetLeader(String::new()),
            GuildCall::SetLeader("Bob".into()),
            GuildCall::SetLeader(" target".into()),
            GuildCall::SetLeader("targets".into()),
        ]
    );
}
