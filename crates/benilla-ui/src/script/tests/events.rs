//! Event registration and dispatch (`0x704d50`, `0x704f10`). Handlers read varargs through Lua
//! 5.0's implicit `arg` table: `...` is not a value in the 1.12 client's grammar.

use super::common::script;
use crate::script::*;

#[test]
fn fire_event_both_conventions_in_one_handler() {
    let mut s = script();
    s.run(
        r#"
        local f = CreateFrame("Frame", "EF")
        f:RegisterEvent("UNIT_HEALTH")
        f:SetScript("OnEvent", function(self, event, ...)
            r_this_eq_self = (this == self)         -- legacy `this` global == modern `self`
            r_event_global = event                  -- modern `event` arg
            r_event_eq     = (event == _G.event)    -- == legacy `event` global
            r_arg1_eq      = (arg1 == arg[1])        -- legacy `arg1` global == the vararg table
            r_arg1         = arg1
            r_arg2         = arg[2]
        end)
    "#,
    )
    .unwrap();

    s.fire_event(
        "UNIT_HEALTH",
        vec![ScriptValue::Str("player".into()), ScriptValue::Int(42)],
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    assert!(s.eval::<bool>("return r_this_eq_self").unwrap());
    assert_eq!(
        s.eval::<String>("return r_event_global").unwrap(),
        "UNIT_HEALTH"
    );
    assert!(s.eval::<bool>("return r_event_eq").unwrap());
    assert!(s.eval::<bool>("return r_arg1_eq").unwrap());
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
        f:RegisterEvent("E")
        f:SetScript("OnEvent", function() end)
    "#,
    )
    .unwrap();
    s.fire_event("E", vec![ScriptValue::Str("x".into())]);
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
        f:RegisterEvent("E")
        f:SetScript("OnEvent", function() error("boom") end)
    "#,
    )
    .unwrap();
    s.fire_event("E", vec![]);
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
        a:RegisterEvent("E"); b:RegisterEvent("E"); c:RegisterEvent("E")
        a:SetScript("OnEvent", function() order = order .. "A" end)
        b:SetScript("OnEvent", function() order = order .. "B" end)
        c:SetScript("OnEvent", function() order = order .. "C" end)
    "#,
    )
    .unwrap();
    s.fire_event("E", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ABC");

    // A duplicate registration keeps A's position.
    s.run("FA:RegisterEvent('E'); order = ''").unwrap();
    s.fire_event("E", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ABC");

    // Unregistering frees B's node, so re-registering appends it at the tail.
    s.run("FB:UnregisterEvent('E'); FB:RegisterEvent('E'); order = ''")
        .unwrap();
    s.fire_event("E", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ACB");
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
        for _, n in ipairs({"WalkA", "WalkB", "WalkC"}) do
            local f = CreateFrame("Frame", n)
            f:RegisterEvent("E")
            f:SetScript("OnEvent", function()
                table.insert(log, n)
                if n == "WalkB" then WalkB:UnregisterEvent("E") end
            end)
        end
    "#,
    )
    .unwrap();
    s.fire_event("E", vec![]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["WalkA", "WalkB", "WalkC"],
        "the once-idiom's self-removal must not skip the next listener"
    );
    s.fire_event("E", vec![]);
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
            f:RegisterEvent("E2")
            f:SetScript("OnEvent", function() table.insert(log, n); if body then body() end end)
            return f
        end
        reg("NxA", function()
            NxB:UnregisterEvent("E2")   -- kill the walk's saved next
        end)
        reg("NxB")
        reg("NxC")
    "#,
    )
    .unwrap();
    s.fire_event("E2", vec![]);
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
            f:RegisterEvent("E3")
            f:SetScript("OnEvent", function() table.insert(log, n); if body then body() end end)
        end
        reg("ApA", function()
            local f = CreateFrame("Frame", "ApLate")
            f:RegisterEvent("E3")
            f:SetScript("OnEvent", function() table.insert(log, "ApLate") end)
        end)
        reg("ApB")
    "#,
    )
    .unwrap();
    s.fire_event("E3", vec![]);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["ApA", "ApB", "ApLate"],
        "a tail-append during dispatch is still visited this dispatch"
    );
}

/// `RegisterAllEvents` (`0x774c20`, table `0x878ec0`, argc 1, arity 0) sends every event to the
/// frame's `OnEvent`; `UnregisterAllEvents` clears it too, which is how AceEvent-2.0 gets back to
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
        3,
        "every event dispatched, whatever its name"
    );
    assert_eq!(
        s.eval::<String>("return seen[3]").unwrap(),
        "SOME_SERVER_EVENT_NOBODY_LISTED"
    );

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
fn an_all_events_listener_runs_after_the_events_own() {
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
}
