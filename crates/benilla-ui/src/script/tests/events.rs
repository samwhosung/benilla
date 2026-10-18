//! Event registration and dispatch (`0x704d50`, `0x704f10`). Handlers read varargs through Lua
//! 5.0's implicit `arg` table: `...` is not a value in the 1.12 client's grammar.

use super::common::script;
use crate::script::*;

/// A handler gets no arguments (`0x705199` calls `lua_pcall` with zero); its inputs are the
/// `this`, `event` and `argN` globals.
#[test]
fn an_event_handler_gets_no_arguments_and_reads_the_globals() {
    let mut s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "EF")
        f:RegisterEvent("UNIT_HEALTH")
        f:SetScript("OnEvent", function(a, b, c)
            r_args = (a == nil and b == nil and c == nil)
            r_this = (this == EF)
            r_event = event
            r_arg1 = arg1
            r_arg2 = arg2
        end)
    "#,
    )
    .unwrap();

    s.fire_event(
        "UNIT_HEALTH",
        vec![ScriptValue::Str("player".into()), ScriptValue::Int(42)],
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    assert!(
        s.eval::<bool>("return r_args").unwrap(),
        "a declared parameter is nil: no positional argument is passed"
    );
    assert!(s.eval::<bool>("return r_this").unwrap());
    assert_eq!(s.eval::<String>("return r_event").unwrap(), "UNIT_HEALTH");
    assert_eq!(s.eval::<String>("return r_arg1").unwrap(), "player");
    assert_eq!(s.eval::<i64>("return r_arg2").unwrap(), 42);
}

#[test]
fn globals_are_restored_after_firing_nesting_safe() {
    let mut s = script();
    s.run(
        r#"
        this, event, arg1 = "outer_this", "outer_event", "outer_arg1"
        local f = CreateFrame("Frame", "NF")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function() end)
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![ScriptValue::Str("x".into())]);
    // `0x704f10` sets the globals for the handler and restores the prior values after.
    let (t, e, a): (String, String, String) = s.eval("return this, event, arg1").unwrap();
    assert_eq!(
        (t.as_str(), e.as_str(), a.as_str()),
        ("outer_this", "outer_event", "outer_arg1")
    );
}

#[test]
fn handler_errors_are_collected_not_panicked() {
    let mut s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "BoomF")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function() error("boom") end)
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    let errs = s.errors();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("boom"), "{errs:?}");
}

/// Each event's listener list is tail-appended (`0x7052d0`) and walked head-first (`0x703e50`);
/// a duplicate registration returns early (`0x702264`).
#[test]
fn events_fire_in_registration_order_fifo() {
    let mut s = script();
    s.run(
        r#"
        order = ""
        local a = CreateFrame("Frame", "FA")
        local b = CreateFrame("Frame", "FB")
        local c = CreateFrame("Frame", "FC")
        a:RegisterEvent("PLAYER_LOGIN"); b:RegisterEvent("PLAYER_LOGIN"); c:RegisterEvent("PLAYER_LOGIN")
        a:SetScript("OnEvent", function() order = order .. "A" end)
        b:SetScript("OnEvent", function() order = order .. "B" end)
        c:SetScript("OnEvent", function() order = order .. "C" end)
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ABC");

    // A duplicate registration keeps A's position.
    s.run("FA:RegisterEvent('PLAYER_LOGIN'); order = ''")
        .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ABC");

    // Unregistering frees B's node, so re-registering appends it at the tail.
    s.run("FB:UnregisterEvent('PLAYER_LOGIN'); FB:RegisterEvent('PLAYER_LOGIN'); order = ''")
        .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ACB");
}

/// `RegisterEvent` (`0x774a40` → `0x702140`) walks the in-game event table with an ASCII
/// case-folding compare (`0x64a4c0` → `0x414310`): a name in any case registers that event, the
/// handler's `event` is the table's spelling, and a name the table lacks matches nothing and
/// raises nothing. `UnregisterEvent` (`0x702280`) compares the same way.
#[test]
fn register_event_matches_the_table_case_insensitively_and_drops_unknown_names() {
    let mut s = script();
    s.run(
        r#"
        heard = {}
        f = CreateFrame("Frame", "CaseF")
        f:SetScript("OnEvent", function() table.insert(heard, event) end)
        f:RegisterEvent("player_login")
        f:RegisterEvent("Not_An_Event")
        g = CreateFrame("Frame", "CaseG")
        g:SetScript("OnEvent", function() table.insert(heard, "g:" .. event) end)
        g:RegisterEvent("UNIT_health")
        "#,
    )
    .expect("a name off the table raises nothing");
    s.fire_event("PLAYER_LOGIN", vec![]);
    s.fire_event("Not_An_Event", vec![]);
    s.fire_event("UNIT_HEALTH", vec![]);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["PLAYER_LOGIN", "g:UNIT_HEALTH"],
        "the table's spelling registers and is what the handler reads; the unknown name never fires"
    );

    s.run(r#"heard = {} f:UnregisterEvent("Player_Login")"#)
        .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert!(
        s.eval::<Vec<String>>("return heard").unwrap().is_empty(),
        "UnregisterEvent matches in any case too"
    );
}

/// Callers ask before hooking, when nothing is set yet (`Tablet-2.0.lua:2409`). Our kind table is
/// flat where the reference's is per widget type, so a Frame also answers true for `OnClick`.
#[test]
fn has_script_reports_the_kind_is_supported_not_that_one_is_set() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "HasScriptProbe")"#)
        .unwrap();

    assert!(
        s.eval::<bool>(r#"return f:HasScript("OnMouseDown")"#)
            .unwrap(),
        "a frame must report it can carry OnMouseDown before one is set"
    );
    assert!(
        !s.eval::<bool>(r#"return f:HasScript("OnNotARealScript")"#)
            .unwrap(),
        "an unknown kind is false, not true"
    );

    // Tablet's idiom, end to end.
    let fired: bool = s
        .eval(
            r#"
            RAN = false
            if f:HasScript("OnMouseDown") then
                local prev = f:GetScript("OnMouseDown")
                f:SetScript("OnMouseDown", function() RAN = true end)
            end
            f:GetScript("OnMouseDown")()
            return RAN
        "#,
        )
        .unwrap();
    assert!(fired, "the guarded hook must install and run");

    // The flat table: a Frame says true for a Button-only kind, where the reference says false.
    assert!(
        s.eval::<bool>(r#"return f:HasScript("OnClick")"#).unwrap(),
        "over-permissive by design today — see the comment at the binding"
    );
}

/// The walk saves the next node before the handler runs (`0x703ee8`); AceEvent-2.0's fire-once
/// idiom unregisters inside its own handler.
#[test]
fn a_self_unregistering_handler_does_not_rob_its_successor() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        for _, name in ipairs({"WalkA", "WalkB", "WalkC"}) do
            -- A body local: the loop's own variable is one for the whole loop, nil after it.
            local n = name
            local f = CreateFrame("Frame", n)
            f:RegisterEvent("PLAYER_LOGIN")
            f:SetScript("OnEvent", function()
                table.insert(log, n)
                if n == "WalkB" then WalkB:UnregisterEvent("PLAYER_LOGIN") end
            end)
        end
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["WalkA", "WalkB", "WalkC"],
        "the once-idiom's self-removal must not skip the next listener"
    );
    s.fire_event("PLAYER_LOGIN", vec![]);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["WalkA", "WalkB", "WalkC", "WalkA", "WalkC"]);
}

/// Unregistering the walk's saved next ends the dispatch, as the reference frees that node and
/// walks into its zeroed links; a frame registered mid-dispatch tail-appends and is still visited.
#[test]
fn mid_dispatch_removal_of_the_next_stops_and_append_is_visited() {
    let mut s = script();
    s.run(
        r#"
        log = {}
        local function reg(n, body)
            local f = CreateFrame("Frame", n)
            f:RegisterEvent("PLAYER_LOGOUT")
            f:SetScript("OnEvent", function() table.insert(log, n); if body then body() end end)
            return f
        end
        reg("NxA", function()
            NxB:UnregisterEvent("PLAYER_LOGOUT")   -- kill the walk's saved next
        end)
        reg("NxB")
        reg("NxC")
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGOUT", vec![]);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["NxA"],
        "removing the saved next ends the dispatch"
    );

    s.run(
        r#"
        log = {}
        local function reg(n, body)
            local f = CreateFrame("Frame", n)
            f:RegisterEvent("PLAYER_ALIVE")
            f:SetScript("OnEvent", function() table.insert(log, n); if body then body() end end)
        end
        reg("ApA", function()
            local f = CreateFrame("Frame", "ApLate")
            f:RegisterEvent("PLAYER_ALIVE")
            f:SetScript("OnEvent", function() table.insert(log, "ApLate") end)
        end)
        reg("ApB")
    "#,
    )
    .unwrap();
    s.fire_event("PLAYER_ALIVE", vec![]);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["ApA", "ApB", "ApLate"],
        "a tail-append during dispatch is still visited this dispatch"
    );
}

/// `RegisterAllEvents` (`0x774c20`, table `0x878ec0`, argc 1, arity 0) registers the frame on every
/// event the VM's table holds (`0x7023e0`); an event off the table reaches no frame, as events
/// fire by id (`0x703e50`). `UnregisterAllEvents` clears it, which is how AceEvent-2.0 gets back to
/// per-event registration.
#[test]
fn register_all_events_takes_every_event_and_unregister_all_clears_it() {
    let mut s = script();
    s.run(
        r#"
        seen = {}
        All = CreateFrame("Frame", "AllEv")
        All:SetScript("OnEvent", function() table.insert(seen, event) end)
        "#,
    )
    .unwrap();

    assert_eq!(s.arity("All:RegisterAllEvents()").unwrap(), 0);

    for ev in [
        "PLAYER_LOGIN",
        "UNIT_HEALTH",
        "SOME_SERVER_EVENT_NOBODY_LISTED",
    ] {
        s.fire_event(ev, vec![]);
    }
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        2,
        "every tabled event dispatched, a name off the table reaching nothing"
    );
    assert_eq!(s.eval::<String>("return seen[2]").unwrap(), "UNIT_HEALTH");

    // All-events plus an explicit registration of the same event is still one listener.
    s.run(r#"seen = {}; All:RegisterAllEvents(); All:RegisterEvent("UNIT_HEALTH")"#)
        .unwrap();
    s.fire_event("UNIT_HEALTH", vec![]);
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        1,
        "fired once, not twice"
    );

    // UnregisterAllEvents clears both registrations.
    s.run("seen = {}; All:UnregisterAllEvents()").unwrap();
    for ev in ["UNIT_HEALTH", "PLAYER_LOGIN"] {
        s.fire_event(ev, vec![]);
    }
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        0,
        "the all-events registration does not outlive UnregisterAllEvents"
    );

    // AceEvent's way back: re-register the events it still wants.
    s.run(r#"All:RegisterEvent("UNIT_HEALTH")"#).unwrap();
    for ev in ["UNIT_HEALTH", "PLAYER_LOGIN"] {
        s.fire_event(ev, vec![]);
    }
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        1,
        "back to exactly one event"
    );
    assert!(s.take_errors().is_empty());
}

#[test]
fn an_all_events_listener_runs_in_registration_order_with_a_named_one() {
    let mut s = script();
    s.run(
        r#"
        order = {}
        Named = CreateFrame("Frame", "NamedEv")
        Named:SetScript("OnEvent", function() table.insert(order, "named") end)
        Named:RegisterEvent("PLAYER_LOGIN")

        Everything = CreateFrame("Frame", "EveryEv")
        Everything:SetScript("OnEvent", function() table.insert(order, "all") end)
        Everything:RegisterAllEvents()
        "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<String>("return order[1]").unwrap(), "named");
    assert_eq!(s.eval::<String>("return order[2]").unwrap(), "all");
    assert_eq!(s.eval::<i64>("return table.getn(order)").unwrap(), 2);

    // The other way round: the all-events frame registered first runs first.
    s.run(
        r#"
        order = {}
        Everything:UnregisterAllEvents()
        Named:UnregisterAllEvents()
        Everything:RegisterAllEvents()
        Named:RegisterEvent("PLAYER_LOGIN")
        "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return table.concat(order, ',')").unwrap(),
        "all,named",
        "one list per event, walked from the head (`0x703e50`)"
    );
}

/// Run `setup`, fire `PLAYER_LOGIN` and answer the comma-joined `log` it built.
fn login_log(s: &mut UiScript, setup: &str) -> String {
    s.run(setup).unwrap();
    s.run("log = {}").unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    s.eval::<String>("return table.concat(log, ',')").unwrap()
}

/// A frame whose `OnEvent` appends `name` to `log`.
const LOGGER: &str = r#"
    function Logger(name)
        local f = CreateFrame("Frame")
        f:SetScript("OnEvent", function() table.insert(log, name) end)
        return f
    end
"#;

/// `RegisterAllEvents` appends to every event's list like a named registration (`0x702440`), so
/// a named frame, an all-events frame and another named frame run in the order they registered.
#[test]
fn named_and_all_events_registrations_interleave_in_registration_order() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        log = {}
        A = Logger("A"); All = Logger("All"); B = Logger("B")
        A:RegisterEvent("PLAYER_LOGIN")
        All:RegisterAllEvents()
        B:RegisterEvent("PLAYER_LOGIN")
        "#,
    );
    assert_eq!(got, "A,All,B");
}

/// `UnregisterEvent` removes that event's node only (`0x702280`), an all-events frame's too;
/// registering again appends at the tail.
#[test]
fn unregister_event_removes_one_event_from_an_all_events_frame() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    s.run(
        r#"
        log = {}
        A = Logger("A"); All = Logger("All")
        A:RegisterEvent("PLAYER_LOGIN"); A:RegisterEvent("UNIT_HEALTH")
        All:RegisterAllEvents()
        All:UnregisterEvent("UNIT_HEALTH")
        "#,
    )
    .unwrap();
    s.fire_event("UNIT_HEALTH", vec![]);
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "A,A,All",
        "UNIT_HEALTH reaches only A; PLAYER_LOGIN still reaches both"
    );

    s.run(
        r#"
        log = {}
        All:UnregisterEvent("PLAYER_LOGIN"); All:RegisterEvent("PLAYER_LOGIN")
        A:UnregisterEvent("PLAYER_LOGIN"); A:RegisterEvent("PLAYER_LOGIN")
        "#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "All,A",
        "unregister then register goes last"
    );
}

/// Registering by name while already present keeps its place (`0x7021a8` → `0x702264`), and a
/// second `RegisterAllEvents` appends nothing (`0x7024c7`).
#[test]
fn a_repeated_register_all_events_keeps_its_place_and_fires_once() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        All = Logger("All"); B = Logger("B")
        All:RegisterAllEvents()
        B:RegisterEvent("PLAYER_LOGIN")
        All:RegisterAllEvents()
        All:RegisterEvent("PLAYER_LOGIN")
        "#,
    );
    assert_eq!(got, "All,B");
}

/// The walk takes each listener's successor before its call (`0x703ee8`): a frame appended by
/// `RegisterAllEvents` from the tail listener is not reached this signal, from an earlier one it
/// is reached at the tail.
#[test]
fn register_all_events_during_a_dispatch_is_reached_unless_from_the_tail() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        Late = Logger("Late")
        Tail = CreateFrame("Frame")
        Tail:SetScript("OnEvent", function() table.insert(log, "Tail"); Late:RegisterAllEvents() end)
        Tail:RegisterEvent("PLAYER_LOGIN")
        "#,
    );
    assert_eq!(got, "Tail", "the tail's append waits for the next signal");
    s.run("log = {}").unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        "Tail,Late"
    );

    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        Late = Logger("Late"); Last = Logger("Last")
        Head = CreateFrame("Frame")
        Head:SetScript("OnEvent", function() table.insert(log, "Head"); Late:RegisterAllEvents() end)
        Head:RegisterEvent("PLAYER_LOGIN")
        Last:RegisterEvent("PLAYER_LOGIN")
        "#,
    );
    assert_eq!(got, "Head,Last,Late");
}

/// A frame registered by name and for all events holds one node per event (`0x702264`), so
/// unregistering the event in its handler leaves it fired once.
#[test]
fn a_frame_both_named_and_all_events_fires_once_and_unregisters_its_node() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        Both = CreateFrame("Frame")
        Both:SetScript("OnEvent", function()
            table.insert(log, "Both")
            Both:UnregisterEvent("PLAYER_LOGIN")
        end)
        Both:RegisterEvent("PLAYER_LOGIN")
        Both:RegisterAllEvents()
        "#,
    );
    assert_eq!(got, "Both");
    s.run("log = {}").unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return table.concat(log, ',')").unwrap(),
        ""
    );
}

/// `event` is set once before the walk and restored after it (`0x703ea4`, `0x703f04`); the
/// per-listener runner (`0x704d50`) never writes it, so one handler's assignment reaches the next.
#[test]
fn event_is_set_once_per_walk_and_restored_after() {
    let mut s = script();
    s.run(LOGGER).unwrap();
    let got = login_log(
        &mut s,
        r#"
        event = "outer"
        First = CreateFrame("Frame")
        First:SetScript("OnEvent", function()
            table.insert(log, event)
            event = "changed"
        end)
        Second = CreateFrame("Frame")
        Second:SetScript("OnEvent", function() table.insert(log, event) end)
        First:RegisterEvent("PLAYER_LOGIN")
        Second:RegisterEvent("PLAYER_LOGIN")
        "#,
    );
    assert_eq!(got, "PLAYER_LOGIN,changed");
    assert_eq!(s.eval::<String>("return event").unwrap(), "outer");
}

/// The world-enter cascade (`0x4908c0`) fires `PLAYER_LOGIN` (`0x490959`) only while a load's arm
/// (`0x49011d`) is unspent, clearing it (`0x49095e`), then `PLAYER_ENTERING_WORLD` (`0x49096a`)
/// every time, inside its own sound bracket (`0x4908d5` to `0x490a56`).
#[test]
fn the_world_enter_cascade_spends_the_login_arm_once_and_plays_nothing() {
    let mut s = script();
    s.run(
        r#"
        order = ""
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_LOGIN")
        f:RegisterEvent("PLAYER_ENTERING_WORLD")
        f:SetScript("OnEvent", function()
            order = order .. event .. " "
            PlaySound("igMainMenuOpen")
        end)
        "#,
    )
    .unwrap();
    s.fire_world_enter();
    assert_eq!(
        s.eval::<String>("return order").unwrap(),
        "PLAYER_ENTERING_WORLD ",
        "no load armed it"
    );
    s.run(r#"order = """#).unwrap();
    s.arm_player_login();
    s.fire_world_enter();
    s.fire_world_enter();
    assert_eq!(
        s.eval::<String>("return order").unwrap(),
        "PLAYER_LOGIN PLAYER_ENTERING_WORLD PLAYER_ENTERING_WORLD ",
        "the arm is one-shot"
    );
    assert!(
        s.take_sounds().is_empty(),
        "the cascade's handlers are silent"
    );
    s.run(r#"PlaySound("igMainMenuOpen")"#).unwrap();
    assert_eq!(
        s.take_sounds().len(),
        1,
        "and the bracket closed behind them"
    );
}

/// The cascade collects in full before its events (`0x490941`-`0x490946` → `0x703bc0`, the
/// threshold set to 0 at `0x703bd6`), so a `PLAYER_LOGIN` handler's `gcinfo()` counts no garbage
/// the load left.
#[test]
fn the_world_enter_cascade_collects_before_player_login() {
    let mut s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_LOGIN")
        f:SetScript("OnEvent", function() AtLogin = gcinfo() end)
        Junk = {}
        for i = 1, 20000 do Junk[i] = { i } end
        Junk = nil
        Before = gcinfo()
        "#,
    )
    .unwrap();
    s.arm_player_login();
    s.fire_world_enter();
    let (before, at_login) = s.eval::<(i64, i64)>("return Before, AtLogin").unwrap();
    assert!(
        at_login + 256 < before,
        "the load's garbage is collected: {before} KB, then {at_login} KB at PLAYER_LOGIN"
    );
}
