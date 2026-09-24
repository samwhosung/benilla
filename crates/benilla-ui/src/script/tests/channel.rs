//! The joined-channel verbs `GetChannelName` and `GetChannelList` ([`crate::script::channel`])
//! and the guild-recruitment latch.

use super::common::script;

/// Two channels in join order, which is the numbering; deliberately not sorted.
fn joined() -> crate::script::UiScript {
    let mut s = script();
    s.set_joined_channels(vec![Some("World".into()), Some("Trade - City".into())]);
    s
}

/// The slot is the 1-based position in join order, not a DBC id or an alphabetical rank.
#[test]
fn a_joined_channel_answers_its_slot_name_and_instance() {
    let s = joined();
    let (id, name, instance): (i64, String, i64) = s.eval("return GetChannelName(2)").unwrap();
    assert_eq!(id, 2, "the 1-based slot in join order");
    assert_eq!(name, "Trade - City");
    assert_eq!(
        instance, 0,
        "instanceID is 0 on every vanilla emulator, and a NUMBER so a caller can compare it"
    );
}

/// Addons look up by name in any case: `_LazyPig` passes `"world"` for the server's `"World"`.
#[test]
fn the_name_form_resolves_case_insensitively_to_the_same_slot() {
    let s = joined();
    let by_name: i64 = s.eval("return GetChannelName('world')").unwrap();
    let by_index: i64 = s.eval("return GetChannelName(1)").unwrap();
    assert_eq!(by_name, 1);
    assert_eq!(by_name, by_index, "both directions must agree on the slot");

    let name: String = s
        .eval("local _, n = GetChannelName('TRADE - CITY') return n")
        .unwrap();
    assert_eq!(
        name, "Trade - City",
        "the answer is the JOINED spelling, not the caller's"
    );
}

/// Stock callers compare the first return numerically (`ChatFrame.lua:2114`, `:2232`).
#[test]
fn a_channel_that_is_not_joined_answers_the_number_zero_never_nil() {
    let s = joined();

    let id: i64 = s.eval("return GetChannelName('NoSuchChannel')").unwrap();
    assert_eq!(id, 0);

    let guard: bool = s
        .eval("local id = GetChannelName('NoSuchChannel') return id > 0")
        .unwrap();
    assert!(
        !guard,
        "the reference's `if ( channelNum > 0 )` must run and be false"
    );

    let out_of_range: bool = s
        .eval("local id = GetChannelName(99) return id > 0")
        .unwrap();
    assert!(!out_of_range, "an out-of-range index takes the same branch");

    let zero: bool = s
        .eval("local id = GetChannelName(0) return id > 0")
        .unwrap();
    assert!(
        !zero,
        "the client bounds-checks 1 <= n <= count, so 0 is not a slot"
    );
}

/// `ChatFrame.lua:2113` passes the `"1"` it cuts out of `/1`, which Lua coerces to a number.
#[test]
fn a_numeric_string_resolves_as_an_index_not_as_a_name() {
    let s = joined();
    let id: i64 = s.eval("return GetChannelName('2')").unwrap();
    assert_eq!(id, 2, "the `/2` slash-command path depends on this");
}

/// `ui_chat::feed` appends on the server's YOU_JOINED notice, never on the request.
#[test]
fn nothing_is_joined_before_the_server_confirms_it() {
    let s = script();
    let id: i64 = s.eval("return GetChannelName('World')").unwrap();
    assert_eq!(id, 0);
}

/// The shape its consumers pin: `FCFDropDown_LoadChannels` walks `for i=1, arg.n, 2` reading
/// `arg[i+1]` as the name, and `ChatLog.lua:424` packs it and spots an id by `type == "number"`.
#[test]
fn get_channel_list_is_a_flat_slot_name_vararg_in_join_order() {
    let s = joined();

    // Two pairs for two joined channels.
    assert_eq!(s.arity("GetChannelList()").unwrap(), 4);

    // Slot 1 is the first joined; alphabetically "Trade - City" would come first.
    let (s1, n1, s2, n2) = s
        .eval::<(i64, String, i64, String)>("return GetChannelList()")
        .unwrap();
    assert_eq!((s1, n1.as_str()), (1, "World"));
    assert_eq!((s2, n2.as_str()), (2, "Trade - City"));

    assert!(s
        .eval::<bool>(
            "local t = { GetChannelList() } \
             return table.getn(t) == 4 and type(t[1]) == 'number' and type(t[2]) == 'string'"
        )
        .unwrap());

    assert!(s
        .eval::<bool>("local i = GetChannelName('Trade - City') return i == 2")
        .unwrap());

    let empty = crate::script::tests::common::script();
    assert_eq!(empty.arity("GetChannelList()").unwrap(), 0);
}

/// `GetGuildRecruitmentMode` (`0x4a0040`) always pushes its int global as a number; it boots at 1
/// from a `.data` initialiser (`raw 0x443608`), the value `UIOptionsFrame_SetDefaults` sets.
#[test]
fn the_guild_recruitment_mode_boots_auto_and_answers_a_number() {
    let s = script();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0,
        "the reference's own .data initialiser, not a BSS zero"
    );
    assert!(s
        .eval::<bool>("return type(GetGuildRecruitmentMode()) == 'number'")
        .unwrap());
    s.run("SetGuildRecruitmentMode(0)").unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );
}

/// `0x4a0060` raises `Usage:` unless `lua_isnumber` (`0x6f34d0`), truncates through `__ftol`
/// (`0x40a2b0`), and raises `invalid mode` outside `0 <= mode < 2`; success pushes no values.
#[test]
fn the_guild_recruitment_setter_gates_its_argument_the_way_the_reference_does() {
    let s = script();

    s.run(r#"SetGuildRecruitmentMode("0")"#).unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );

    // 1.7 truncates to a legal 1.
    s.run("SetGuildRecruitmentMode(1.7)").unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0
    );

    for bad in ["", "nil", r#""AUTO""#, "{}", "true"] {
        let e = s
            .run(&format!("SetGuildRecruitmentMode({bad})"))
            .expect_err(&format!("SetGuildRecruitmentMode({bad}) must raise"));
        assert!(
            e.to_string().contains("Usage: SetGuildRecruitmentMode"),
            "{bad}: {e}"
        );
    }
    for bad in ["-1", "2", "-1.7", "37"] {
        let e = s
            .run(&format!("SetGuildRecruitmentMode({bad})"))
            .expect_err(&format!("SetGuildRecruitmentMode({bad}) must raise"));
        assert!(e.to_string().contains("invalid mode"), "{bad}: {e}");
    }

    // None of the refused calls moved the latch.
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        1.0
    );
    assert_eq!(
        s.arity("SetGuildRecruitmentMode(0)").unwrap(),
        0,
        "0x4a0060 returns `xor eax,eax` — zero values, not a nil"
    );
}

/// `0x49ea70` stores the latch and jumps into the cascade `0x49ea90` whenever the new value is 1,
/// moved or not; `0x4a00a4`/`0x4a00a9` fire `UPDATE_CHAT_WINDOWS` on every successful call.
#[test]
fn the_setter_asks_for_the_cascade_on_one_and_fires_update_chat_windows() {
    let mut s = script();
    s.run(
        r#"
        n = 0
        local f = CreateFrame("Frame", "GRF")
        f:RegisterEvent("UPDATE_CHAT_WINDOWS")
        f:SetScript("OnEvent", function() n = n + 1 end)
    "#,
    )
    .unwrap();
    assert!(!s.take_guild_recruitment_cascade(), "nothing asked yet");

    // Boots at 1; a Set(1) that moves nothing still asks for the cascade.
    s.run("SetGuildRecruitmentMode(1)").unwrap();
    assert!(s.take_guild_recruitment_cascade());
    assert!(!s.take_guild_recruitment_cascade(), "drained");
    assert!(
        !s.take_guild_recruitment_change(),
        "…and the file is not dirtied by a no-move"
    );

    s.run("SetGuildRecruitmentMode(0)").unwrap();
    assert!(
        !s.take_guild_recruitment_cascade(),
        "mode 0 is the latch alone"
    );
    assert!(
        s.take_guild_recruitment_change(),
        "the file is dirtied by the move"
    );

    // A refused call fires nothing.
    s.run("SetGuildRecruitmentMode(2)").unwrap_err();
    s.tick(0.016);
    assert_eq!(
        s.eval::<i64>("return n").unwrap(),
        2,
        "one UPDATE_CHAT_WINDOWS per successful call: Set(1), Set(0); the raise fired none"
    );
}

/// A manual join or leave of `GuildRecruitment` calls `0x49ea70(0)` (`0x49ed3d`, `0x49ef8f`): it
/// dirties the file and, as mode 0, asks for no cascade.
#[test]
fn a_manual_guild_recruitment_verb_resets_the_latch() {
    let mut s = script();
    assert!(s.reset_guild_recruitment_mode(), "1 → 0 moved");
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );
    assert!(s.take_guild_recruitment_change());
    assert!(!s.take_guild_recruitment_cascade());
    assert!(
        !s.reset_guild_recruitment_mode(),
        "already 0: nothing moved"
    );
    assert!(!s.take_guild_recruitment_change());
}
