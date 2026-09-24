//! The WoW stdlib: positional `format`, the bare-global aliases, the sandbox, `debugstack`,
//! `GetTime`, `time`/`date`, `RunScript` and the Lua 5.0 dialect.

use super::common::script;

// ── The positional format wrapper ──

#[test]
fn positional_format_reorders_and_mix_is_an_error() {
    let s = script();
    assert_eq!(
        s.eval::<String>(r#"return format("%2$s %1$s", "a", "b")"#)
            .unwrap(),
        "b a"
    );
    // width/precision travel with the positional spec
    assert_eq!(
        s.eval::<String>(r#"return format("%1$05d", 42)"#).unwrap(),
        "00042"
    );
    // sequential still works, through the patched `string.format` too
    assert_eq!(
        s.eval::<String>(r#"return string.format("%d-%s", 1, "x")"#)
            .unwrap(),
        "1-x"
    );
    // %% is preserved
    assert_eq!(
        s.eval::<String>(r#"return format("%1$d%%", 50)"#).unwrap(),
        "50%"
    );
    // mixing positional and sequential is an error, as in the reference
    let mixed_ok: bool = s
        .eval(r#"return pcall(format, "%1$s %s", "a", "b")"#)
        .unwrap();
    assert!(!mixed_ok, "mixed positional+sequential must error");
}

// ── getglobal and the alias layer ──

#[test]
fn stdlib_aliases_and_helpers() {
    let s = script();
    s.run(
        r#"
        -- getglobal on a named frame
        local f = CreateFrame("Frame", "GG")
        assert(getglobal("GG") == f)

        -- the bare-global aliases
        assert(strupper("ab") == "AB" and strlower("AB") == "ab")
        assert(strsub("hello", 2, 3) == "el")
        assert(strlen("hello") == 5)
        local t = {}
        tinsert(t, 10); tinsert(t, 20)
        assert(getn(t) == 2)
        tremove(t, 1)
        assert(t[1] == 20)

        -- and the six 2.0 names that are NOT here (decision 2146)
        assert(wipe == nil and tostringall == nil)
        assert(strsplit == nil and strjoin == nil and strconcat == nil and strtrim == nil)
    "#,
    )
    .unwrap();
}

// ── Sandbox holes ──

#[test]
fn sandbox_removes_dangerous_globals() {
    let s = script();
    let all_nil: bool = s
        .eval(
            r#"return io == nil and os == nil and package == nil and require == nil
               and dofile == nil and loadfile == nil and debug == nil"#,
        )
        .unwrap();
    assert!(all_nil);
    // `debugstack` survives the sandbox and returns a real traceback, which addons parse.
    let trace = s.eval::<String>("return debugstack()").unwrap();
    assert!(
        !trace.is_empty(),
        "debugstack must return a real traceback: {trace:?}"
    );
    // Frames only, no `stack traceback:` header: `AceLibrary.lua:70` reads line 1 as a frame and
    // `AceDB-2.0.lua:742` skips exactly one line to reach its caller.
    assert!(
        !trace.starts_with("stack traceback"),
        "the reference has no header line: {trace:?}"
    );
    // A level past the top of the stack is an empty string, never a raise.
    assert_eq!(s.eval::<String>("return debugstack(99)").unwrap(), "");
}

/// AceDB-2.0's `RegisterDB` skips one line of `debugstack()`, its own frame, and takes the caller's
/// `\AddOns\<folder>\` from the next; here the library and its caller load from different addons.
#[test]
fn acedbs_capture_names_the_calling_addon_not_the_librarys_owner() {
    let s = script();
    s.run_chunk_named(
        b"function BenillaProbeRegisterDB()
            return string.gsub(debugstack(), \".-\\n.-\\\\AddOns\\\\(.-)\\\\.*\", \"%1\")
          end",
        &crate::script::addon_chunk_name("AtlasLoot", "Libs\\AceDB-2.0\\AceDB-2.0.lua"),
    )
    .unwrap();
    s.run_chunk_named(
        b"BenillaProbeCaller = BenillaProbeRegisterDB()",
        &crate::script::addon_chunk_name("Bartender2", "Bartender2.lua"),
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return BenillaProbeCaller").unwrap(),
        "Bartender2",
        "the capture must name the CALLER's addon, not the library owner's"
    );
}

/// `AceLibrary.lua:70` pulls a file name out of the first line, a whole frame.
#[test]
fn the_first_debugstack_line_is_a_frame() {
    let s = script();
    s.run_chunk_named(
        b"function BenillaProbeFirstLine()
            local first = string.gsub(debugstack(), \"\\n.*\", \"\")
            return string.gsub(first, \".*\\\\(.*).lua:%d+: .*\", \"%1\")
          end",
        &crate::script::addon_chunk_name("Atlas", "Libs\\AceLibrary\\AceLibrary.lua"),
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return BenillaProbeFirstLine()").unwrap(),
        "AceLibrary",
        "line 1 is the calling function's own frame"
    );
}

/// The reference's Lua 5.0 writes a named frame as `` in function `name' ``, opening with a
/// backtick, which `AceLibrary.lua:139`'s `argCheck` pattern `` "([`<].-['>])" `` needs.
#[test]
fn a_named_frame_uses_the_5_0_backtick_quoting() {
    let s = script();
    let found: String = s
        .eval(
            "function BenillaProbeNamed()                 local _, _, f = string.find(debugstack(), \"([`<].-['>])\")                 return f or '<no match>'              end              local r = BenillaProbeNamed() return r",
        )
        .unwrap();
    assert_eq!(found, "`BenillaProbeNamed'");
}

/// The reference ends each frame with `"\n"` and writes no header (`0x703971`). It formats while
/// the level is `<= start + count1` (`0x703857`, `jbe`), then probes `getstack(level + count2)`; a
/// failed probe prints that level as an ordinary frame. So with `count1 = 1, count2 = 0` a
/// two-deep stack gives two frames and no marker, and a three-deep one a frame then `"...\n"`.
#[test]
fn debugstack_frames_end_in_newline_and_count1_does_not_clamp() {
    let s = script();
    let trace = s.eval::<String>("return debugstack()").unwrap();
    assert!(
        trace.ends_with('\n'),
        "the newline is pushed AFTER each frame: {trace:?}"
    );
    let two: String = s
        .eval(
            "function BenillaProbeDepth2() local r = debugstack(1, 1, 0) return r end \
             local r = BenillaProbeDepth2() return r",
        )
        .unwrap();
    let lines: Vec<&str> = two.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 2, "two frames, no marker: {two:?}");
    assert!(!two.contains("..."), "no elision at depth 2: {two:?}");
    let deep: String = s
        .eval(
            "function BenillaProbeC() local r = debugstack(1, 1, 0) return r end \
             function BenillaProbeB() local r = BenillaProbeC() return r end \
             function BenillaProbeA() local r = BenillaProbeB() return r end \
             local r = BenillaProbeA() return r",
        )
        .unwrap();
    let lines: Vec<&str> = deep.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 2, "one frame then the marker: {deep:?}");
    assert_eq!(lines[1], "...", "the marker is its own complete line");
}

/// An addon chunk is named as the reference names it, `@Interface\AddOns\<folder>\<file>`:
/// `FuBarPlugin-2.0.lua:752` finds its folder with a greedy `"\\AddOns\\(.*)\\"`, which captures
/// up to the last backslash, so the file must be in the name.
#[test]
fn an_addon_chunk_is_named_the_way_the_client_names_it() {
    let name = crate::script::addon_chunk_name("FuBar_BagFu", "FuBar_BagFu.lua");
    assert_eq!(name, "@Interface\\AddOns\\FuBar_BagFu\\FuBar_BagFu.lua");

    // FuBar's own pattern, against a traceback from a chunk loaded under that name.
    let s = script();
    s.run_chunk_named(
        b"function BenillaProbeFolder() return debugstack(1, 1, 0) end",
        &name,
    )
    .unwrap();
    let folder: String = s
        .eval(
            "local _, _, f = string.find(BenillaProbeFolder(), \"\\\\AddOns\\\\(.*)\\\\\") \
             return f or '<no match>'",
        )
        .unwrap();
    assert_eq!(
        folder, "FuBar_BagFu",
        "FuBarPlugin's own capture must yield the folder name"
    );

    // A nested path in the manifest keeps its separators as backslashes.
    assert_eq!(
        crate::script::addon_chunk_name("Big", "libs/Thing/Thing.lua"),
        "@Interface\\AddOns\\Big\\libs\\Thing\\Thing.lua"
    );
}

#[test]
fn loadstring_is_text_only_bytecode_rejected() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        -- valid source compiles
        local f = loadstring("return 1 + 1")
        assert(type(f) == "function" and f() == 2)
        -- bytecode is rejected: returns nil + error message
        local bc = string.dump(function() return 7 end)
        local g, err = loadstring(bc)
        return (g == nil) and (type(err) == "string")
    "#,
        )
        .unwrap();
    assert!(ok, "loadstring must reject bytecode");
}

// ── GetTime: the session clock ──

#[test]
fn gettime_starts_at_zero_and_tracks_tick() {
    let mut s = script();
    assert_eq!(s.eval::<f64>("return GetTime()").unwrap(), 0.0);
    s.tick(0.25);
    s.tick(0.25);
    let t = s.eval::<f64>("return GetTime()").unwrap();
    assert!((t - 0.5).abs() < 1e-6, "two 0.25s ticks = 0.5 (got {t})");
}

/// The rest of the engine's bare string and math globals, and the later-client names it lacks.
#[test]
fn the_rest_of_the_bare_globals() {
    let s = script();
    s.run(
        r#"
        -- string family
        assert(strbyte("A") == 65 and strchar(65) == "A")

        -- and the Era-only names 1187 added are GONE (decision 1189): the 5.0 client has no
        -- string.match/gmatch, and claiming otherwise misleads an addon that feature-detects.
        assert(strmatch == nil and gmatch == nil and strrev == nil)
        assert(strlenutf8 == nil and strcmputf8i == nil)
        assert(securecall == nil and hooksecurefunc == nil and issecure == nil)

        -- math family
        assert(exp(0) == 1 and log(1) == 0 and log10(100) == 2)
        assert(frexp(8) == 0.5 and ldexp(0.5, 4) == 8)

        -- the trig globals are DEGREE-based, inverses included — same family as the verified
        -- sin/cos, and what every addon rotation helper assumes.
        assert(math.abs(tan(45) - 1) < 1e-9)
        assert(math.abs(asin(1) - 90) < 1e-9)
        assert(math.abs(acos(1)) < 1e-9)
        assert(math.abs(atan(1) - 45) < 1e-9)
        assert(math.abs(atan2(1, 0) - 90) < 1e-9)
    "#,
    )
    .unwrap();
}

/// 1.12 runs Lua 5.0 (`0x811b30`, "Lua: Lua 5.0 Copyright..."); this VM is mlua's `lua51`, and the
/// 5.0 idioms vanilla addons use run on it, above all the implicit vararg `arg` table.
#[test]
fn the_lua_5_0_dialect_vanilla_addons_are_written_in_runs_here() {
    let s = script();
    s.run(
        r##"
        -- The implicit vararg table, 5.0's spelling of what 5.1 does with `...`.
        local function varargs(...) return arg.n, arg[1], arg[2] end
        local n, first, second = varargs("a", "b")
        assert(n == 2 and first == "a" and second == "b")

        -- The edge that used to be here is gone (decision 2101). `arg` was synthesized only for
        -- a vararg function that did NOT also mention `...` in its body, so mixing the two
        -- spellings in one function left `arg` nil. `...` as a value is no longer in the grammar
        -- at all, so nothing can clear the flag and EVERY vararg function has its `arg`.
        local function fixed_and_varargs(a, ...) return a, arg.n, arg[1] end
        local a, n, first = fixed_and_varargs("a", "b", "c")
        assert(a == "a" and n == 2 and first == "b")

        -- 5.0's table/string/math spellings, all of which 5.1 renamed.
        assert(table.getn({ 1, 2, 3 }) == 3)
        assert(string.gfind ~= nil)          -- 5.1 renamed this to string.gmatch
        assert(math.mod(7, 3) == 1)
        -- ...and the 5.1 OPERATORS 5.0 lacks are not in the grammar: `%`, `#`, and `...` as a
        -- value all fail to compile, exactly as they do on the 1.12 client (2101).
        assert(loadstring("return 7 % 3") == nil)
        assert(loadstring("return #({1})") == nil)
        assert(loadstring("return function(...) return ... end") == nil)
    "##,
    )
    .unwrap();
}

/// `time()` is wall-clock epoch seconds and `date()` formats them, both engine globals in the 1.12
/// `_G` (slots 34 and 33 of its base registry); addons save `time()` and compare across logins.
#[test]
fn time_is_epoch_seconds_and_date_formats_them() {
    let s = script();

    // A plausible wall clock, between 2020 and 2100, not GetTime's 0-based one.
    let now: i64 = s.eval("return time()").unwrap();
    assert!(
        (1_577_836_800..4_102_444_800).contains(&now),
        "time() must be wall-clock epoch seconds, got {now}"
    );

    // 1_000_000_000 = 2001-09-09 01:46:40 UTC, a Sunday, day 252 of the year.
    for (fmt, want) in [
        ("%Y-%m-%d", "2001-09-09"),
        ("%H:%M:%S", "01:46:40"),
        (
            "%A, %B %d, %Y - %H:%M",
            "Sunday, September 09, 2001 - 01:46",
        ),
        ("%a %b %y", "Sun Sep 01"),
        ("%I:%M %p", "01:46 AM"),
        ("%j", "252"),
        ("%w", "0"),
        ("%c", "Sun Sep  9 01:46:40 2001"),
        ("100%%", "100%"),
        // An unknown specifier is emitted verbatim rather than swallowed.
        ("%Q", "%Q"),
    ] {
        let got: String = s
            .eval(&format!(r#"return date("{fmt}", 1000000000)"#))
            .unwrap();
        assert_eq!(got, want, "date({fmt:?})");
    }

    // Bare `date()` is `%c` and must not raise; `Recap.lua:2690` calls it with no arguments.
    let bare: String = s.eval("return date()").unwrap();
    assert!(
        bare.len() > 10,
        "bare date() must format something: {bare:?}"
    );

    // A leap day, the civil conversion's edge case.
    let leap: String = s.eval(r#"return date("%Y-%m-%d %A", 951782400)"#).unwrap();
    assert_eq!(leap, "2000-02-29 Tuesday");
}

// ── RunScript ──

/// `RunScript` hands its string to `FrameScript_Execute` (`0x704cd0`) as both code and chunk name
/// (`0x48b9c7`, `0x48b9c9`), so `luaO_chunkid` (`0x6f5c40`) reports it as `[string "…"]`.
#[test]
fn a_runscript_chunk_is_named_by_its_own_source() {
    let mut s = script();
    // RunScript consumes the error, so read it off the recorded channel.
    s.run("RunScript(\"error('boom')\")").unwrap();
    let errs = s.take_errors();
    assert!(
        errs.iter()
            .any(|e| e.starts_with("[string \"error('boom')\"]:1: boom")),
        "the chunk names itself by its source: {errs:?}"
    );
}

/// A non-string argument returns silently (`0x48b98f` jumps to `0x48b9f3`), as does an empty
/// string (`0x48b9a1`); a number passes `lua_isstring` (`0x6f3510`) and runs as text. The function
/// never calls `luaL_error` (`0x6f4940`).
#[test]
fn runscript_swallows_a_bad_argument_instead_of_raising() {
    let mut s = script();
    s.run(
        "BenillaRan = 0
         RunScript(nil)
         RunScript({})
         RunScript(false)
         RunScript('')
         BenillaRan = 1",
    )
    .expect("a bad RunScript argument is a no-op, not a raise");
    assert_eq!(s.eval::<i64>("return BenillaRan").unwrap(), 1);
    assert!(
        s.take_errors().is_empty(),
        "a silent no-op records nothing either"
    );
    // A number is a string to `lua_isstring`, so it compiles, and `42` is not a statement.
    s.run("RunScript(42)").unwrap();
    assert!(
        s.take_errors()
            .iter()
            .any(|e| e.contains("[string \"42\"]")),
        "a number coerces and runs as its own text"
    );
}

/// A raise inside the snippet stays there: `0x704ae0` runs it under `lua_pcall` (`0x704b68`) with
/// the registry's error handler (`0x704afe`), and calls that handler for a compile failure too
/// (`0x704b42`).
#[test]
fn a_runscript_error_reaches_the_handler_and_not_the_caller() {
    let mut s = script();
    s.run(
        "BenillaAfter = 0
         RunScript('error(\"inner\")')
         RunScript('this is not lua')
         BenillaAfter = 1",
    )
    .expect("neither a runtime nor a compile error may propagate to the caller");
    assert_eq!(
        s.eval::<i64>("return BenillaAfter").unwrap(),
        1,
        "the caller's next statement still runs"
    );
    let errs = s.take_errors();
    assert_eq!(
        errs.len(),
        2,
        "both errors are recorded, not dropped: {errs:?}"
    );
    assert!(errs[0].contains("inner"), "{errs:?}");
    // mlua's `Display` prefix ("syntax error: ") is not something the reference writes.
    assert!(
        errs[1].starts_with("[string \"this is not lua\"]:1:"),
        "a compile failure is reported under the same name, undecorated: {errs:?}"
    );
}
