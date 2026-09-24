//! `GetChannelName` — the joined-channel lookup ([`crate::script::channel`]).
//!
//! The verb is small; the contract around it is not, and every test here is named after one claim.
//! The load-bearing one is [`a_channel_that_is_not_joined_answers_the_number_zero_never_nil`]: the
//! reference's own callers and the corpus both compare the first return numerically, so a nil there
//! does not degrade — it raises, at four call sites that work today.

use super::common::script;

/// Two channels joined in order, the way `ui_chat::feed` mirrors them after the server's
/// YOU_JOINED. Join ORDER is the whole numbering law, so the fixture is deliberately not sorted.
fn joined() -> crate::script::UiScript {
    let mut s = script();
    s.set_joined_channels(vec![Some("World".into()), Some("Trade - City".into())]);
    s
}

/// **A joined channel answers its 1-based slot, its name, and an instanceID.** The slot is the
/// position in JOIN order — not a DBC id, not an alphabetical rank.
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

/// **The name form resolves, case-insensitively, and yields the same slot as the index form.**
/// Half the corpus sites look up by name (`GetChannelName("world")`, `GetChannelName("Trade -
/// City")`), and `_LazyPig` passes `"world"` for a channel the server names `"World"`.
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

/// **Not joined ⇒ the NUMBER 0. Never nil.** This is the trap, and it is the only way this verb can
/// break working code rather than merely fail to help it.
///
/// Verified from both sides. The reference compares it numerically at `ChatFrame.lua:2114`
/// (`if ( channelNum > 0 )`) and `l.2232` (`if ( channelNum <= 0 ) then return end`); so does the
/// corpus at `_LazyPig/LazyPig.lua:1996` (`if id > 0 then`). A nil first return turns every one of
/// those into "attempt to compare nil with number" — a loud failure in code that works today.
///
/// Asserted by DOING the comparison, not by inspecting the type, because the comparison is the
/// thing that must not raise.
#[test]
fn a_channel_that_is_not_joined_answers_the_number_zero_never_nil() {
    let s = joined();

    let id: i64 = s.eval("return GetChannelName('NoSuchChannel')").unwrap();
    assert_eq!(id, 0);

    // The reference's own guard shape, run for real: it must evaluate, not raise.
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

/// **A numeric STRING resolves as a number**, because `ChatFrame.lua:2113` hands one straight in:
/// it `gsub`s `/1` down to `"1"` and calls `GetChannelName(channel)`. On the real client Lua's own
/// coercion makes that work; ours must not treat it as a channel literally named "1".
#[test]
fn a_numeric_string_resolves_as_an_index_not_as_a_name() {
    let s = joined();
    let id: i64 = s.eval("return GetChannelName('2')").unwrap();
    assert_eq!(id, 2, "the `/2` slash-command path depends on this");
}

/// **Empty until the server confirms a join.** `ui_chat::feed` appends on the server's YOU_JOINED
/// notice, never on the request, so a session that has asked and not been answered has no slots —
/// and the verb must still answer 0 rather than raise on an empty list.
#[test]
fn nothing_is_joined_before_the_server_confirms_it() {
    let s = script();
    let id: i64 = s.eval("return GetChannelName('World')").unwrap();
    assert_eq!(id, 0);
}

/// **`GetChannelList()` is a FLAT vararg of (slot, name) pairs in join order** — not a table, and
/// not name-first.
///
/// No single reference address pins the shape; two independent consumers do, and the
/// test asserts what each of them relies on. The reference's `FCFDropDown_LoadChannels` walks
/// `for i=1, arg.n, 2` and reads `arg[i+1]` as the name, so the pair order is (slot, name) and the
/// stride is 2. `ChatLog.lua:424` packs it with `{ GetChannelList() }` and tests
/// `type(value) == "number"` to spot an id, so the values must interleave rather than nest.
#[test]
fn get_channel_list_is_a_flat_slot_name_vararg_in_join_order() {
    let s = joined();

    // Exactly two pairs for two joined channels — the arity `FCFDropDown_LoadChannels` steps over.
    assert_eq!(s.arity("GetChannelList()").unwrap(), 4);

    // Pair order and join order, both at once: slot 1 is the FIRST joined, not the alphabetical
    // first ("Trade - City" would sort ahead of "World").
    let (s1, n1, s2, n2) = s
        .eval::<(i64, String, i64, String)>("return GetChannelList()")
        .unwrap();
    assert_eq!((s1, n1.as_str()), (1, "World"));
    assert_eq!((s2, n2.as_str()), (2, "Trade - City"));

    // ChatLog's own walk: packed, the odd entries are numbers and the even ones are names.
    assert!(s
        .eval::<bool>(
            "local t = { GetChannelList() } \
             return table.getn(t) == 4 and type(t[1]) == 'number' and type(t[2]) == 'string'"
        )
        .unwrap());

    // The slot agrees with GetChannelName's, which is the point of sharing the numbering.
    assert!(s
        .eval::<bool>("local i = GetChannelName('Trade - City') return i == 2")
        .unwrap());

    // No channels joined is ZERO returns, so `{ GetChannelList() }` is an empty table rather than
    // a table of nils — every consumer above already handles that shape.
    let empty = crate::script::tests::common::script();
    assert_eq!(empty.arity("GetChannelList()").unwrap(), 0);
}

/// **The guild-recruitment latch boots at AUTO and round-trips as a NUMBER**.
///
/// `GetGuildRecruitmentMode 0x4a0040` is 23 bytes and one path — `fild` the int global,
/// `lua_pushnumber`, `mov eax,1`, `ret` — so it has no nil leg at all, and
/// `UIOptionsFrame_Load`'s `== 1` would read a nil as a silent "not auto". The boot value is 1
/// from a `.data` initialiser (`raw 0x443608` = `01 00 00 00`), corroborated by
/// `UIOptionsFrame_SetDefaults`'s `SetGuildRecruitmentMode(1)` and by all 33 `chat-cache.txt`
/// files the reference client itself wrote in this repo's install.
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

/// **The setter is shape A — it RAISES rather than swallowing a bad argument**.
///
/// `0x4a0060` gates on `lua_isnumber 0x6f34d0` (so a numeric STRING passes) and otherwise
/// `luaL_error`s `Usage: SetGuildRecruitmentMode(mode)`; it then truncates toward zero through
/// `__ftol 0x40a2b0` and range-gates `0 <= mode < 2`, raising
/// `SetGuildRecruitmentMode: invalid mode` outside it. Most 1.12 numeric bindings swallow a nil
/// as 0.0 — this one does not, and a client that guessed the common shape would turn an addon's
/// own bug into silence.
///
/// Success pushes **0 values**, not nil.
#[test]
fn the_guild_recruitment_setter_gates_its_argument_the_way_the_reference_does() {
    let s = script();

    // A numeric string is a number to `lua_isnumber`.
    s.run(r#"SetGuildRecruitmentMode("0")"#).unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );

    // Truncation toward zero, not rounding: 1.7 is a legal 1.
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

    // …and none of the refused calls moved the latch.
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

/// **The setter is not inert**. `0x49ea70` stores the latch and tail-jumps into
/// the cascade `0x49ea90` on the new value alone — `Set(1)` raises the app's cue whether or not
/// the value moved; `Set(0)` never does — and `0x4a00a4`/`0x4a00a9` fire `UPDATE_CHAT_WINDOWS`
/// on every successful call, before any cascade fires it again.
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

    // Boots at 1; a Set(1) that moves nothing still asks — the reference's store is
    // unconditional and the jump reads only `ecx == 1`.
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

/// **A manual join or leave of `GuildRecruitment` forces the latch to 0** — `0x49ed3d`/
/// `0x49ef8f`, `call 0x49ea70(0)`. A player gesture, so it dirties the file; and mode 0 is the
/// latch alone, so it asks for no cascade.
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
