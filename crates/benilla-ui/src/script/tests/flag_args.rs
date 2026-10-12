//! The flag arguments the reference reads through `GetBoolOrDefault` (`0x6f1c10`): absent takes the
//! binding's default, nil is false, a boolean is itself, a number is its truncation's low dword, a
//! string goes by its first byte and then by `on`/`off`/`enabled`/`disabled`, and a table or
//! function takes the default. Each binding below is one call site of the helper, with the default
//! its site pushes.

use super::common::script;
use crate::script::*;

/// Every argument form with the answer the helper gives it for `default`.
fn cases(default: bool) -> Vec<(&'static str, bool)> {
    vec![
        ("nil", false),
        ("true", true),
        ("false", false),
        ("0", false),
        ("0.5", false),
        ("-0.5", false),
        ("1", true),
        ("-1", true),
        ("2^32", false),
        ("2^32 + 1", true),
        ("1/0", false),
        (r#""0""#, false),
        (r#""1""#, true),
        (r#""0.5""#, false),
        (r#""9lives""#, true),
        (r#""f""#, false),
        (r#""False""#, false),
        (r#""n""#, false),
        (r#""no""#, false),
        (r#""t""#, true),
        (r#""True""#, true),
        (r#""y""#, true),
        (r#""yes""#, true),
        (r#""off""#, false),
        (r#""OFF""#, false),
        (r#""Disabled""#, false),
        (r#""on""#, true),
        (r#""Enabled""#, true),
        (r#""""#, default),
        (r#""zzz""#, default),
        (r#"" 0""#, default),
        (r#""-1""#, default),
        ("{}", default),
        ("function() end", default),
    ]
}

/// Drive one flag binding: `set(arg)` writes it, `get()` reads it back as a Lua boolean. Each case
/// starts from the opposite of its answer, so an ignored argument fails.
fn check(s: &UiScript, what: &str, set: &str, get: &str, default: bool) {
    let put = |arg: &str| {
        let call = set.replace("ARG", arg);
        s.run(&call)
            .unwrap_or_else(|e| panic!("{what}: {call}: {e}"));
    };
    let read = || s.eval::<bool>(&format!("return ({get}) == true")).unwrap();
    // The bare call takes the default.
    put("true");
    put("false");
    s.run(&set.replace(", ARG", "").replace("ARG", "")).unwrap();
    assert_eq!(read(), default, "{what}: no argument");
    for (arg, want) in cases(default) {
        put(if want { "false" } else { "true" });
        assert_eq!(read(), !want, "{what}: setup");
        put(arg);
        assert_eq!(read(), want, "{what}({arg})");
    }
}

/// The Frame methods, all default 1 (`0x775440` ... `0x77727e`).
#[test]
fn frame_flag_setters_read_get_bool_or_default_with_default_one() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F")"#).unwrap();
    for (set, get) in [
        ("SetToplevel", "IsToplevel"),
        ("SetMovable", "IsMovable"),
        ("SetResizable", "IsResizable"),
        ("SetClampedToScreen", "IsClampedToScreen"),
        ("EnableKeyboard", "IsKeyboardEnabled"),
        ("EnableMouse", "IsMouseEnabled"),
        ("EnableMouseWheel", "IsMouseWheelEnabled"),
    ] {
        // The predicates answer the number 1 or nil, never a boolean.
        check(
            &s,
            set,
            &format!("F:{set}(ARG)"),
            &format!("F:{get}() == 1"),
            true,
        );
    }
    // `SetUserPlaced` (`0x776a50`) refuses a frame that is neither movable nor resizable.
    s.run("F:SetMovable(true)").unwrap();
    check(
        &s,
        "SetUserPlaced",
        "F:SetUserPlaced(ARG)",
        "F:IsUserPlaced() == 1",
        true,
    );
}

/// `SetFading` on both message-frame classes (`0x7939a0`, `0x7950d0`), default 1.
#[test]
fn set_fading_reads_get_bool_or_default_with_default_one() {
    let s = script();
    s.run(
        r#"
        S = CreateFrame("ScrollingMessageFrame", "S")
        M = CreateFrame("MessageFrame", "M")
    "#,
    )
    .unwrap();
    for f in ["S", "M"] {
        check(
            &s,
            &format!("{f}:SetFading"),
            &format!("{f}:SetFading(ARG)"),
            &format!("{f}:GetFading() == 1"),
            true,
        );
    }
}

/// `SetTexCoordModifiesRect`, `SetDesaturated` (`0x79c10e`, `0x79c27e`) and `SetNonSpaceWrap`
/// (`0x79ea7e`), default 1.
#[test]
fn texture_and_fontstring_flags_read_get_bool_or_default_with_default_one() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame", "F")
        T = F:CreateTexture("T")
        FS = F:CreateFontString("FS")
    "#,
    )
    .unwrap();
    check(
        &s,
        "SetTexCoordModifiesRect",
        "T:SetTexCoordModifiesRect(ARG)",
        "T:GetTexCoordModifiesRect() == 1",
        true,
    );
    check(
        &s,
        "SetNonSpaceWrap",
        "FS:SetNonSpaceWrap(ARG)",
        "FS:CanNonSpaceWrap() == 1",
        true,
    );
    // Desaturation has no getter; read it off the region.
    let desat = || {
        let t: mlua::Table = s.lua().globals().get("T").unwrap();
        let rh = crate::script::region::region_handle_of(s.lua(), &t).unwrap();
        s.model_ref()
            .region_data
            .get(&rh)
            .is_some_and(|d| d.desaturated)
    };
    s.run("T:SetDesaturated(false) T:SetDesaturated()").unwrap();
    assert!(desat(), "SetDesaturated: no argument takes the default 1");
    for (arg, want) in cases(true) {
        s.run(&format!(
            "T:SetDesaturated({})",
            if want { "false" } else { "true" }
        ))
        .unwrap();
        s.run(&format!("T:SetDesaturated({arg})")).unwrap();
        assert_eq!(desat(), want, "SetDesaturated({arg})");
    }
}

/// `SetBackdrop`'s `tile` key (`0x7778a3`), default 0.
#[test]
fn backdrop_tile_reads_get_bool_or_default_with_default_zero() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F")"#).unwrap();
    for (arg, want) in cases(false) {
        if arg == "nil" {
            continue; // an absent key and a nil value are the same table slot
        }
        s.run(&format!("F:SetBackdrop({{ tile = {arg} }})"))
            .unwrap();
        assert_eq!(
            s.eval::<bool>("return F:GetBackdrop().tile == 1").unwrap(),
            want,
            "tile = {arg}"
        );
    }
    s.run("F:SetBackdrop({})").unwrap();
    assert!(!s.eval::<bool>("return F:GetBackdrop().tile == 1").unwrap());
}

/// The chat-window records (`0x4a1777` default 1, `0x4a1697` default 0).
#[test]
fn chat_window_flags_read_get_bool_or_default() {
    let s = script();
    // `GetChatWindowInfo` answers shown 7th and locked 8th, each 1 or nil; Lua 5.0 has no `select`.
    s.run(
        "function ChatShown() local _, _, _, _, _, _, v = GetChatWindowInfo(2) return v end \
         function ChatLocked() local _, _, _, _, _, _, _, v = GetChatWindowInfo(2) return v end",
    )
    .unwrap();
    check(
        &s,
        "SetChatWindowShown",
        "SetChatWindowShown(2, ARG)",
        "ChatShown() == 1",
        true,
    );
    check(
        &s,
        "SetChatWindowLocked",
        "SetChatWindowLocked(2, ARG)",
        "ChatLocked() == 1",
        false,
    );
}

/// The guild flags: show-offline (`0x4d1e17`, default 1), the rank checkbox (`0x4d20b7`, default 0).
#[test]
fn guild_flags_read_get_bool_or_default() {
    let s = script();
    check(
        &s,
        "SetGuildRosterShowOffline",
        "SetGuildRosterShowOffline(ARG)",
        "GetGuildRosterShowOffline() == 1",
        true,
    );
    let staged = || s.model_ref().guild_control.staged[0];
    s.run("GuildControlSetRankFlag(1, true) GuildControlSetRankFlag(1)")
        .unwrap();
    assert!(!staged(), "no argument takes the default 0");
    for (arg, want) in cases(false) {
        s.run(&format!(
            "GuildControlSetRankFlag(1, {})",
            if want { "false" } else { "true" }
        ))
        .unwrap();
        s.run(&format!("GuildControlSetRankFlag(1, {arg})"))
            .unwrap();
        assert_eq!(staged(), want, "GuildControlSetRankFlag(1, {arg})");
    }
}

/// `GetNumGuildMembers`' flag (`0x4d11a0`, default 0) counts the whole roster.
#[test]
fn get_num_guild_members_flag_counts_the_whole_roster() {
    let mut s = script();
    let member = |name: &str, online: bool| GuildMemberInfo {
        name: name.into(),
        online,
        ..Default::default()
    };
    s.set_guild(GuildState {
        in_guild: true,
        roster: vec![member("A", true), member("B", false), member("C", false)],
        num_members: 1,
        ..Default::default()
    });
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers()").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers(nil)").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers(0)").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers('0')").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers(1)").unwrap(), 3);
    assert_eq!(
        s.eval::<i64>("return GetNumGuildMembers('yes')").unwrap(),
        3
    );
    assert_eq!(s.eval::<i64>("return GetNumGuildMembers(true)").unwrap(), 3);
}

/// The request-queueing verbs, each reading one flag through the helper.
#[test]
fn queued_verbs_read_their_flags_through_get_bool_or_default() {
    let mut s = script();
    s.set_guild(GuildState::default());

    // SetWhoToUI(flag) 0x5ad877, default 0.
    for (arg, want) in cases(false) {
        s.run(&format!("SetWhoToUI({arg})")).unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SetWhoToUi(want)],
            "SetWhoToUI({arg})"
        );
    }
    s.run("SetWhoToUI()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::SetWhoToUi(false)]
    );

    // TargetByName / FollowByName exact-match flag, default 0 (`0x489d8e`, `0x489f14`).
    for (arg, want) in cases(false) {
        s.run(&format!("FollowByName('Bob', {arg})")).unwrap();
        assert_eq!(
            s.take_follow_requests(),
            vec![FollowRequest::ByName {
                name: "Bob".into(),
                exact: want
            }],
            "FollowByName('Bob', {arg})"
        );
        s.run(&format!("TargetByName('Bob', {arg})")).unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::TargetByName {
                name: "Bob".into(),
                exact: want
            }],
            "TargetByName('Bob', {arg})"
        );
    }

    // ConfirmReadyCheck(ready) 0x4bc0ad, default 0.
    for (arg, want) in cases(false) {
        s.run(&format!("ConfirmReadyCheck({arg})")).unwrap();
        assert_eq!(
            s.take_party_requests(),
            vec![PartyRequest::ReadyCheckAnswer(want)],
            "ConfirmReadyCheck({arg})"
        );
    }
    s.run("ConfirmReadyCheck()").unwrap();
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::ReadyCheckAnswer(false)]
    );

    // UseAction(action, checkCursor, onSelf) 0x4e717e, 0x4e716f, defaults 0.
    for (arg, want) in cases(false) {
        s.run(&format!("UseAction(1, 0, {arg})")).unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![ScriptCall::UseAction(ActionUse {
                action: 1,
                on_self: want,
                item_locks: Vec::new(),
            })],
            "UseAction(1, 0, {arg})"
        );
    }
}

/// `LoggingChat(flag)` (`0x49fe69`, default 1) sets whenever an argument is passed, so an explicit
/// nil clears; a bare call is the getter.
#[test]
fn logging_flags_read_get_bool_or_default_with_default_one() {
    let s = script();
    for name in ["LoggingChat", "LoggingCombat"] {
        for (arg, want) in cases(true) {
            s.run(&format!("{name}({})", if want { "false" } else { "true" }))
                .unwrap();
            s.run(&format!("{name}({arg})")).unwrap();
            assert_eq!(
                s.eval::<bool>(&format!("return {name}() == 1")).unwrap(),
                want,
                "{name}({arg})"
            );
        }
    }
}

/// `CreateMacro`'s `local` and `perCharacter` flags (`0x4f167b`, `0x4f168a`) and `EditMacro`'s
/// `local` (`0x4f19ad`), default 0; `EditMacro` always writes it.
#[test]
fn macro_flags_read_get_bool_or_default_with_default_zero() {
    let s = script();
    s.run("function MacroLocal(i) local _, _, _, v = GetMacroInfo(i) return v end")
        .unwrap();
    for (arg, want) in cases(false) {
        s.run(&format!(
            "i = CreateMacro('m', 1, '', {arg}) _, _, _, loc = GetMacroInfo(i)"
        ))
        .unwrap();
        assert_eq!(
            s.eval::<bool>("return loc == 1").unwrap(),
            want,
            "CreateMacro(name, icon, body, {arg})"
        );
        s.run("DeleteMacro(i)").unwrap();
    }
    s.run("i = CreateMacro('m', 1, '', true) EditMacro(i, nil, nil, 'body')")
        .unwrap();
    assert!(
        !s.eval::<bool>("return MacroLocal(i) == 1").unwrap(),
        "an omitted EditMacro `local` is stored as 0"
    );
    for (arg, want) in cases(false) {
        s.run(&format!("EditMacro(i, nil, nil, nil, {arg})"))
            .unwrap();
        assert_eq!(
            s.eval::<bool>("return MacroLocal(i) == 1").unwrap(),
            want,
            "EditMacro(i, nil, nil, nil, {arg})"
        );
    }
}

/// The GameTooltip wrap flag (`0x531811`, `0x531dd3`, `0x531b60`), default 0: a wrapped line pins
/// its left cell to the wrap width. A double line wraps only with an empty right side, as the line
/// core clears the flag otherwise (`0x530284`).
#[test]
fn tooltip_wrap_flags_read_get_bool_or_default_with_default_zero() {
    let s = script();
    s.run(r#"TT = CreateFrame("GameTooltip", "TT")"#).unwrap();
    let pinned = |line: u32| {
        let t: mlua::Table = s
            .lua()
            .globals()
            .get(format!("TTTextLeft{line}"))
            .expect("left line");
        let rh = crate::script::region::region_handle_of(s.lua(), &t).unwrap();
        s.model_ref()
            .region_data
            .get(&rh)
            .is_some_and(|d| d.size.is_some())
    };
    for (arg, want) in cases(false) {
        s.run("TT:ClearLines()").unwrap();
        s.run(&format!("TT:AddLine('line', 1, 1, 1, {arg})"))
            .unwrap();
        assert_eq!(pinned(1), want, "AddLine(text, r, g, b, {arg})");
        s.run(&format!("TT:SetText('line', 1, 1, 1, 1, {arg})"))
            .unwrap();
        assert_eq!(pinned(1), want, "SetText(text, r, g, b, a, {arg})");
        s.run(&format!(
            "TT:ClearLines() TT:AddDoubleLine('line', '', 1, 1, 1, 1, 1, 1, {arg})"
        ))
        .unwrap();
        assert_eq!(
            pinned(1),
            want,
            "AddDoubleLine(.., {arg}) with an empty right"
        );
        s.run(&format!(
            "TT:ClearLines() TT:AddDoubleLine('line', 'right', 1, 1, 1, 1, 1, 1, {arg})"
        ))
        .unwrap();
        assert!(!pinned(1), "a double line with a right side never wraps");
    }
    s.run("TT:ClearLines() TT:AddLine('line')").unwrap();
    assert!(!pinned(1), "no flag takes the default 0");
}
