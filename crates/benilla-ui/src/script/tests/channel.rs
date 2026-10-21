//! The joined-channel verbs `GetChannelName` and `GetChannelList` ([`crate::script::channel`])
//! and the guild-recruitment latch.

use crate::script::{CallHost, CallTail, ChannelCommand, ScriptCall, UiScript};

/// A VM with no engine behind it: the channel verbs' calls are kept for the test to read.
fn script() -> UiScript {
    let mut s = super::common::script();
    s.keep_calls();
    s
}

/// A host that applies each channel call by naming it in the global `APPLIED`, so a handler can
/// tell whether the call was applied before it ran.
#[derive(Default)]
struct Recorder(Vec<ChannelCommand>);

impl CallHost for Recorder {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        if let ScriptCall::Channel(cmd) = call {
            if let ChannelCommand::Join { name, .. } | ChannelCommand::Leave { name, .. } = &cmd {
                script
                    .lua()
                    .globals()
                    .set("APPLIED", name.as_str())
                    .expect("the global sets");
            }
            self.0.push(cmd);
        }
        None
    }
}

/// Two channels in join order, which is the numbering; deliberately not sorted.
fn joined() -> UiScript {
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

/// A fresh VM holds no slot: the app's mirror and the join verbs fill it.
#[test]
fn a_fresh_vm_holds_no_channel() {
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

/// `SetGuildRecruitmentMode(1)` runs the cascade in place (`0x49ea70` → `0x49ea90`): the join
/// registers window 1's entry and is applied, then fires `UPDATE_CHAT_WINDOWS` (`0x49eb5f`), all
/// before the verb's own (`0x4a00a9`); a leave fires the same pair (`0x49eb21`); an owed run fires
/// the verb's alone. Neither passes the binding's reset flag.
#[test]
fn the_recruitment_cascade_runs_inside_the_verb() {
    use crate::script::RecruitmentPlan;
    let mut s = super::common::script();
    crate::script::tick::heard::listen(
        &s,
        &["UPDATE_CHAT_WINDOWS"],
        Some("tostring(APPLIED) .. ',' .. tostring((GetChatWindowChannels(1)))"),
    );
    let mut host = Recorder::default();
    let run = |s: &mut UiScript, host: &mut Recorder, lua: &str| {
        s.hosted(host, |s| s.run(lua)).unwrap();
        std::mem::take(&mut host.0)
    };

    s.set_guild_recruitment_plan(RecruitmentPlan::Join {
        name: "GuildRecruitment - City".into(),
        shortcut: "GuildRecruitment".into(),
        id: 25,
    });
    assert_eq!(
        run(&mut s, &mut host, "SetGuildRecruitmentMode(1)"),
        [ChannelCommand::Join {
            name: "GuildRecruitment - City".into(),
            password: String::new(),
            id: 25,
        }]
    );
    assert_eq!(
        crate::script::tick::heard::take(&s),
        [
            "UPDATE_CHAT_WINDOWS GuildRecruitment - City,GuildRecruitment",
            "UPDATE_CHAT_WINDOWS GuildRecruitment - City,GuildRecruitment"
        ],
        "the join is applied and window 1 holds the entry before either event"
    );
    assert!(!s.take_guild_recruitment_cascade(), "the run is spent");

    s.set_guild_recruitment_plan(RecruitmentPlan::Leave("GuildRecruitment - City".into()));
    assert_eq!(
        run(&mut s, &mut host, "SetGuildRecruitmentMode(1)"),
        [ChannelCommand::Leave {
            name: "GuildRecruitment - City".into(),
        }]
    );
    assert_eq!(crate::script::tick::heard::take(&s).len(), 2);

    // The retry owes the run; no player returns before anything (`0x49eac3`); an unnamed
    // channel still signals the cascade's own event before the verb's.
    for (plan, events, owed) in [
        (RecruitmentPlan::Retry, 1, true),
        (RecruitmentPlan::NoPlayer, 1, false),
        (RecruitmentPlan::Unnamed, 2, false),
    ] {
        s.set_guild_recruitment_plan(plan);
        assert!(run(&mut s, &mut host, "SetGuildRecruitmentMode(1)").is_empty());
        assert_eq!(crate::script::tick::heard::take(&s).len(), events);
        assert_eq!(s.take_guild_recruitment_cascade(), owed);
    }

    s.set_guild_recruitment_plan(RecruitmentPlan::Join {
        name: "GuildRecruitment - City".into(),
        shortcut: "GuildRecruitment".into(),
        id: 25,
    });
    assert!(run(&mut s, &mut host, "SetGuildRecruitmentMode(0)").is_empty());
    assert_eq!(
        crate::script::tick::heard::take(&s).len(),
        1,
        "mode 0 is the latch alone"
    );
}

/// A binding's join of the `GuildRecruitment` row calls `0x49ea70(0)` (`0x49ed3d`): it dirties
/// the file and, as mode 0, asks for no cascade; with the latch already 0 nothing moves.
#[test]
fn a_bindings_guild_recruitment_join_resets_the_latch() {
    let mut s = script();
    s.set_zone_channel_catalog(vec![crate::script::ZoneChannelRow {
        id: 25,
        shortcut: "GuildRecruitment".into(),
        resolved: Some("GuildRecruitment - City".into()),
        listed: true,
        guild_recruitment: true,
    }]);
    s.run("JoinChannelByName('GuildRecruitment')").unwrap();
    assert_eq!(
        s.eval::<f64>("return GetGuildRecruitmentMode()").unwrap(),
        0.0
    );
    assert!(s.take_guild_recruitment_change());
    assert!(!s.take_guild_recruitment_cascade());
    s.run("JoinChannelByName('GuildRecruitment')").unwrap();
    assert!(
        !s.take_guild_recruitment_change(),
        "already 0: nothing moved"
    );
}

/// `JoinChannelByName` (`0x49ff00`): a name that is neither string nor number raises the usage
/// text (`0x49ffe7`); a spaced name pushes nothing (`0x49ff5e`); a join pushes two values (the
/// channel id and the resolved name or nil, `0x49ffcc`, `0x49ffdd`) and sends; a `frameId` may be
/// a numeric string.
#[test]
fn join_channel_by_name_raises_pushes_two_values_or_nothing() {
    let mut s = script();
    let count = |s: &crate::script::UiScript, call: &str| -> i64 {
        s.eval(&format!("return (function(...) return arg.n end)({call})"))
            .unwrap()
    };
    for call in [
        "JoinChannelByName()",
        "JoinChannelByName(nil)",
        "JoinChannelByName({})",
        "JoinChannelByName(true)",
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(
            err.contains(r#"Usage: JoinChannelByName("name")"#),
            "{call}: {err}"
        );
    }
    // A spaced name, and an empty one: nothing at all, and nothing sent.
    assert_eq!(count(&s, "JoinChannelByName('two words')"), 0);
    assert_eq!(count(&s, "JoinChannelByName('')"), 0);
    assert!(s.take_channel_commands().is_empty());
    // A custom channel: two values, the second nil; a number is stringified.
    assert_eq!(count(&s, "JoinChannelByName('Guildies', 'pw', '2')"), 2);
    assert_eq!(count(&s, "JoinChannelByName(7)"), 2);
    let sent = s.take_channel_commands();
    assert_eq!(sent.len(), 2);
    // The numeric-string frame id registered the channel in window 2's list.
    assert!(s
        .eval::<bool>(
            "for _, n in ipairs({ GetChatWindowChannels(2) }) do if n == 'Guildies' then return true end end return false"
        )
        .unwrap());
}

/// A slot of the app's mirror: its name, its `ChatChannels.dbc` id, and whether it is in state 0.
fn slot(name: &str, id: u32, confirmed: bool) -> Option<crate::script::JoinedChannel> {
    Some(crate::script::JoinedChannel {
        name: name.into(),
        id,
        confirmed,
    })
}

/// Rows 1 and 2 as `ChatChannels.dbc` has them, composed for Elwynn Forest.
fn elwynn_rows() -> Vec<crate::script::ZoneChannelRow> {
    vec![
        crate::script::ZoneChannelRow {
            id: 1,
            shortcut: "General".into(),
            resolved: Some("General - Elwynn Forest".into()),
            listed: true,
            guild_recruitment: false,
        },
        crate::script::ZoneChannelRow {
            id: 2,
            shortcut: "Trade".into(),
            resolved: Some("Trade - Elwynn Forest".into()),
            listed: false,
            guild_recruitment: false,
        },
    ]
}

/// `GetChannelName` by number is `0x49bf30` after `__ftol` (`0x4a05ed`-`0x4a0609`), which keeps a
/// slot in state 0 only (`0x49bf5c`-`0x49bf68`); by name it is `0x49be90` (`0x4a0627`), which
/// tests no state.
#[test]
fn a_number_names_only_a_confirmed_slot_and_a_name_any() {
    let mut s = script();
    s.set_joined_channels(vec![
        slot("General - Elwynn Forest", 1, true),
        slot("Trade - Elwynn Forest", 2, false),
        slot("MyChan", 0, false),
    ]);
    let miss = (0, None, 0);
    for call in [
        "GetChannelName(2)",
        "GetChannelName('2')",
        "GetChannelName(3)",
        "GetChannelName(2.9)",
        "GetChannelName(-1)",
    ] {
        assert_eq!(
            s.eval::<(i64, Option<String>, i64)>(&format!("return {call}"))
                .unwrap(),
            miss,
            "{call}: a slot the server has not confirmed answers no number"
        );
    }
    assert_eq!(
        s.eval::<(i64, String)>("return GetChannelName(1.9)")
            .unwrap(),
        (1, "General - Elwynn Forest".to_string()),
        "the number truncates"
    );
    assert_eq!(
        s.eval::<i64>("return GetChannelName('Trade - Elwynn Forest')")
            .unwrap(),
        2,
        "the name finds the pending slot"
    );
    assert_eq!(s.eval::<i64>("return GetChannelName('mychan')").unwrap(), 3);
}

/// `GetChannelList` (`0x4a02d0`) skips every slot not in state 0 (`0x4a0302`-`0x4a030b`) and names
/// each by its row's Shortcut when its id names a row (`0x4a0323`-`0x4a034f`), else by its own
/// name (`0x4a0351`).
#[test]
fn the_channel_list_names_confirmed_slots_by_their_shortcut() {
    let mut s = script();
    s.set_zone_channel_catalog(elwynn_rows());
    s.set_joined_channels(vec![
        slot("General - Elwynn Forest", 1, true),
        slot("Trade - Elwynn Forest", 2, false),
        None,
        slot("MyChan", 0, true),
        slot("LocalDefense - Elwynn Forest", 22, true),
    ]);
    let list: Vec<mlua::Value> = s.eval("return {GetChannelList()}").unwrap();
    let got: Vec<String> = list
        .iter()
        .map(|v| match v {
            mlua::Value::Integer(i) => i.to_string(),
            mlua::Value::Number(n) => n.to_string(),
            mlua::Value::String(t) => t.to_str().unwrap().to_string(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        got,
        [
            "1",
            "General",
            "4",
            "MyChan",
            "5",
            "LocalDefense - Elwynn Forest"
        ],
        "the pending Trade slot is left out; a row's slot is its Shortcut, a custom one or one \
         whose id names no row its own name"
    );
}

/// `RemoveChatWindowChannel` sends a number through `0x49e880` (`0x4a12bf`) to `0x49be50`, which
/// answers a slot in state 0 only (`0x49be73`-`0x49be78`); with none it removes nothing
/// (`0x4a12c9`).
#[test]
fn remove_chat_window_channel_numbers_only_a_confirmed_slot() {
    let mut s = script();
    s.run("AddChatWindowChannel(1, 'MyChan')").unwrap();
    s.set_joined_channels(vec![slot("MyChan", 0, false)]);
    s.run("RemoveChatWindowChannel(1, '1')").unwrap();
    assert_eq!(
        s.chat_window_looks()[0].channels,
        vec![("MyChan".to_string(), 0)],
        "slot 1 is not confirmed: nothing is removed"
    );
    s.set_joined_channels(vec![slot("MyChan", 0, true)]);
    s.run("RemoveChatWindowChannel(1, '1')").unwrap();
    assert!(s.chat_window_looks()[0].channels.is_empty());
}

/// The join body stores a matched row's Shortcut as its second result (`0x49ecec`-`0x49ecf3`),
/// which `JoinChannelByName` returns (`0x49ffbc`-`0x49ffc5`); the packet carries the composed name
/// and the slot the row's id (`0x49ede8`).
#[test]
fn join_channel_by_name_answers_a_rows_shortcut() {
    let mut s = script();
    s.set_zone_channel_catalog(elwynn_rows());
    assert_eq!(
        s.eval::<(i64, String)>("return JoinChannelByName('general')")
            .unwrap(),
        (1, "General".to_string())
    );
    assert!(matches!(
        &s.take_channel_commands()[..],
        [ChannelCommand::Join { name, id: 1, .. }] if name == "General - Elwynn Forest"
    ));
}
