//! Global verbs that gate a string argument with `is-number-or-string` (`0x6f3510`) and raise their
//! own literal through `luaL_error` (`0x6f4940`) on a miss: a number passes as its text, while nil,
//! a missing argument, a boolean or a table raises the verb's line, never mlua's conversion text.

use super::common::script;
use crate::script::UiScript;

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
