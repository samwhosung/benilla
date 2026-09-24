//! The WoW stdlib: positional `format`, the `getglobal` alias layer, and the
//! sandbox holes (`loadstring` text-only, dangerous globals removed).

use super::common::script;

// ── The positional format wrapper ───────────────────────────────────────────────────────────────

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
    // sequential still works (and via string.format too, which we patched)
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
    // mixing positional and sequential is an error (matches Blizzard erroring)
    let mixed_ok: bool = s
        .eval(r#"return pcall(format, "%1$s %s", "a", "b")"#)
        .unwrap();
    assert!(!mixed_ok, "mixed positional+sequential must error");
}

// ── getglobal / the alias layer ───────────────────────────────────────────────

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

// ── Sandbox holes ───────────────────────────────────────────────────────────────────────────────

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
    // `debugstack` survives the sandbox — and returns a REAL traceback, not the `""` this used to
    // assert. The stub was fine for addons that only DISPLAY it and wrong for the ones that PARSE
    // it: `FuBarPlugin-2.0.lua:752` finds each plugin's own folder in `debugstack(6, 1, 0)`, and
    // against `""` that returned nil and killed 20 corpus addons.
    let trace = s.eval::<String>("return debugstack()").unwrap();
    assert!(
        !trace.is_empty(),
        "debugstack must return a real traceback: {trace:?}"
    );
    // **Frames only — no `stack traceback:` header** (decision 2121). Line 1 IS a frame, which is
    // what `AceLibrary.lua:70` reads (`string.gsub(stack, "\n.*", "")` then
    // `".*\\(.*).lua:%d+: .*"`), and what `AceDB-2.0.lua:742` counts on when it skips exactly one
    // line to reach its caller's.
    assert!(
        !trace.starts_with("stack traceback"),
        "the reference has no header line: {trace:?}"
    );
    // A level far past the top of the stack is an empty string, never a raise — a caller that
    // guesses too deep still gets something it can `string.find` against.
    assert_eq!(s.eval::<String>("return debugstack(99)").unwrap(), "");
}

/// **AceDB-2.0's own capture, run for real across two chunks** (decision 2121).
///
/// `RegisterDB` reads the calling addon's folder out of `debugstack()` by skipping exactly one
/// line — its own frame — and taking the `\AddOns\<folder>\` out of the next. That only works if
/// line 1 is a frame; with mlua's `stack traceback:` header in front, the capture returned the
/// folder of whichever addon shipped the winning copy of the library, `RegisterDB` took its
/// already-loaded branch, and `db.raw` was bound to a fresh table at the addon's file scope —
/// before the SavedVariables chunk ran. Bartender2 then printed `Creating new DB` at every login.
///
/// The two chunks here are the real configuration: the library lives in one addon's folder, the
/// caller in another's.
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

/// `AceLibrary.lua:70`'s reader: the first line, whole, is a frame it can pull a file name out of.
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

/// `AceLibrary.lua:139`'s `argCheck` names the offending function with `"([`<].-['>])"` — a
/// BACKTICK opening it. Lua 5.4 writes `in function 'name'`, which that pattern cannot match; the
/// reference's 5.0 wording is `` in function `name' ``.
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

/// **Every frame carries its own trailing `\n`, and `count1` bounds nothing on its own**
/// (decision 2121).
///
/// Stock Lua 5.0 pushes `"\n\t"` *before* each frame plus a header once; the reference pushes a
/// single `"\n"` after each frame and no header (`0x703971`). And the walk formats while the level
/// is `<= start + count1` — `0x703857` is `jbe`, unsigned ≤ — then probes
/// `getstack(level + count2)`: a probe that FAILS steps back and prints that level as an ordinary
/// frame. So with `count1 = 1, count2 = 0` a two-deep stack returns TWO frames and no marker, and a
/// three-deep one returns one frame plus `"...\n"` and nothing after. A client that clamped to
/// `count1` would diverge at exactly `depth == count1 + 1` — and that is
/// `FuBarPlugin-2.0.lua:752`'s `debugstack(6, 1, 0)`, whose capture is a GREEDY
/// `"\\AddOns\\(.*)\\"` reading the LAST path in the string.
#[test]
fn debugstack_frames_end_in_newline_and_count1_does_not_clamp() {
    let s = script();
    let trace = s.eval::<String>("return debugstack()").unwrap();
    assert!(
        trace.ends_with('\n'),
        "the newline is pushed AFTER each frame: {trace:?}"
    );
    // Two levels at or above `start`, `count1 = 1`: the elision probe fails, so the second frame
    // prints in full and no marker appears.
    let two: String = s
        .eval(
            "function BenillaProbeDepth2() local r = debugstack(1, 1, 0) return r end \
             local r = BenillaProbeDepth2() return r",
        )
        .unwrap();
    let lines: Vec<&str> = two.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 2, "two frames, no marker: {two:?}");
    assert!(!two.contains("..."), "no elision at depth 2: {two:?}");
    // Deeper: one frame, then the marker on its own line, and nothing after it (count2 = 0).
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

/// **An addon's chunk is named the way the CLIENT names it**, because addons parse that name.
///
/// `FuBarPlugin-2.0.lua:752` is `string.find(debugstack(6, 1, 0), "\\AddOns\\(.*)\\")`, and it
/// feeds the capture into `format("Interface\\AddOns\\%s\\icon", folderName)`. Without a name
/// mlua defaults the chunk to the RUST caller location, that pattern misses, and every FuBar plugin
/// dies formatting a nil three frames from the cause.
///
/// The greedy `(.*)` is why the file has to be in the name too: it captures to the LAST backslash.
#[test]
fn an_addon_chunk_is_named_the_way_the_client_names_it() {
    let name = crate::script::addon_chunk_name("FuBar_BagFu", "FuBar_BagFu.lua");
    assert_eq!(name, "@Interface\\AddOns\\FuBar_BagFu\\FuBar_BagFu.lua");

    // FuBar's own pattern, run for real against a traceback from a chunk loaded under that name.
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

// ── GetTime: the session clock (decision 0137 — the reference cast bar anchors on it) ───────────

#[test]
fn gettime_starts_at_zero_and_tracks_tick() {
    let mut s = script();
    assert_eq!(s.eval::<f64>("return GetTime()").unwrap(), 0.0);
    s.tick(0.25);
    s.tick(0.25);
    let t = s.eval::<f64>("return GetTime()").unwrap();
    assert!((t - 0.5).abs() < 1e-6, "two 0.25s ticks = 0.5 (got {t})");
}

/// The rest of the bare globals Blizzard's own restricted scope enumerates (decision 1187).
///
/// The list is not from memory: `Blizzard_RestrictedAddOnEnvironment/RestrictedEnvironment.lua`
/// copies exactly these out of the engine, which is what distinguishes them from the table helpers
/// (`tContains`, `tInvert`, …) that FrameXML defines in Lua and we must therefore NOT supply.
///
/// The measured cost of their absence: Bagnon + BagBrother produced 94 load failures, 81 of them
/// `Cannot find a library instance of "…"`, because `strmatch` was nil so every LibStub-registering
/// library aborted before it could register.
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

/// **The Lua 5.0 dialect a vanilla addon is written in runs on our 5.1 VM** — measured, because
/// this question has been answered three different ways from memory.
///
/// 1.12 runs Lua 5.0 (byte-confirmed: `0x811b30 = "Lua: Lua 5.0 Copyright..."`);
/// we run 5.1 via mlua's `lua51`. Decision 1188 called that "the deepest divergence and it is
/// unresolved" and told the next session to test it; 1189 replied that 0068 had already closed it.
/// Meanwhile five of our own transcribed FrameXML files carried the opposite claim in a comment —
/// *"`LUA_COMPAT_VARARG` isn't shipped"* — each citing the others as precedent, so a false fact
/// propagated by citation without anyone re-running it.
///
/// It is shipped. This asserts the five idioms vanilla addon code actually uses, so the next
/// session reads a result instead of a recollection. **The implicit `arg` table is the one that
/// matters**: it is the difference between a vanilla addon's vararg functions working and every
/// one of them erroring at runtime, which is 1188's stated redirect-the-arc risk.
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

/// **`time()` is real epoch seconds and `date()` formats them** — engine globals in the 1.12
/// client's own `_G` (slots 34/33 of its base registry) that we lacked entirely, because the
/// sandbox strips `os`.
///
/// `time` was the top name in the session-start `attempt to call global` row. Every corpus site
/// persists it into SavedVariables and compares across sessions
/// (`FTC_Save[k].LastCheck = time()`), so a session-relative clock like `GetTime` would be wrong in
/// a way that only shows up on the SECOND login.
///
/// The formats asserted are the ones read off real call sites, against a FIXED epoch so the
/// conversion is checked rather than the machine's clock: 2001-09-09 01:46:40 UTC, a Sunday.
#[test]
fn time_is_epoch_seconds_and_date_formats_them() {
    let s = script();

    // A plausible wall clock: after 2020, before 2100. Pins that this is not GetTime's 0-based one.
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

    // Bare `date()` is `%c` (Recap.lua:2690 calls it with no arguments) and must not raise.
    let bare: String = s.eval("return date()").unwrap();
    assert!(
        bare.len() > 10,
        "bare date() must format something: {bare:?}"
    );

    // A leap day, because the civil conversion is where a date implementation goes wrong.
    let leap: String = s.eval(r#"return date("%Y-%m-%d %A", 951782400)"#).unwrap();
    assert_eq!(leap, "2000-02-29 Tuesday");
}

// ── RunScript, at the image's own contract (2136's "left open", closed) ──────────────────────────

/// `RunScript`'s chunk name is the SOURCE, not a label of ours.
///
/// `0x48b9c7 mov edx,eax` / `0x48b9c9 mov ecx,eax` hand `lua_tostring`'s one return to
/// `FrameScript_Execute 0x704cd0` as both the code and the name, so the name reaches
/// `luaL_loadbuffer` unprefixed and `luaO_chunkid 0x6f5c40` wraps it — `[string "…"]`. We used to
/// write `=[RunScript]`, whose leading `=` is chunkid's *print this verbatim* marker.
#[test]
fn a_runscript_chunk_is_named_by_its_own_source() {
    let mut s = script();
    // The error is consumed by RunScript (fact 3), so read it off the recorded channel rather
    // than off a raise.
    s.run("RunScript(\"error('boom')\")").unwrap();
    let errs = s.take_errors();
    assert!(
        errs.iter()
            .any(|e| e.starts_with("[string \"error('boom')\"]:1: boom")),
        "the chunk names itself by its source: {errs:?}"
    );
}

/// The three silent legs, and the raise that must NOT happen.
///
/// `lua_isstring 0x6f3510` is tag-based, so a number passes and runs as its own text; every other
/// type takes `0x48b98f je 0x48b9f3` to `xor eax,eax; ret`. An empty string takes the same exit at
/// `0x48b9a1`. There is no `luaL_error 0x6f4940` in the function at all.
#[test]
fn runscript_swallows_a_bad_argument_instead_of_raising() {
    let mut s = script();
    // Each of these must return normally AND leave the next statement running.
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
    // A number IS a string to `lua_isstring`, so it compiles — and `42` is not a statement.
    s.run("RunScript(42)").unwrap();
    assert!(
        s.take_errors()
            .iter()
            .any(|e| e.contains("[string \"42\"]")),
        "a number coerces and runs as its own text"
    );
}

/// A raise inside the snippet does not escape it: `0x704ae0` runs the chunk under
/// `lua_pcall(L, 0, 0, -2)` (`0x704b68`) with the registry's error handler pushed at `0x704afe`,
/// and pcalls that same handler for a *compile* failure (`0x704b42`). Both legs return 0 values.
///
/// This is the one with teeth: raising here let one bad macro abort whatever ran it.
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
    // mlua's `Display` category word ("syntax error: ") is not something the image ever writes.
    assert!(
        errs[1].starts_with("[string \"this is not lua\"]:1:"),
        "a compile failure is reported under the same name, undecorated: {errs:?}"
    );
}
