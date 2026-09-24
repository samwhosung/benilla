//! RegisterEvent + fire_event via BOTH conventions (`0x704d50` / `0x704f10`).
//!
//! The handler's extra arguments are read through 5.0's implicit `arg` table, not `select(n, ...)`:
//! `...` as a value is not in this VM's grammar, because it is not in the 1.12
//! client's. The point of the test is unchanged — the same handler sees the legacy globals
//! (`this`, `event`, `arg1`) AND the positional arguments.

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
    // After firing, the prior global values must be restored (`0x704f10`'s set-then-restore).
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

/// The cross-frame dispatch ORDER law: the client's
/// per-event listener list is tail-appended (`0x7052d0`) and walked head-first (`0x703e50`) —
/// **FIFO: registration order = firing order**. Duplicate registration keeps the original
/// position (`0x702264` dup ret); unregister+re-register moves to the tail. The ZoneText frames
/// depend on this: both write PVPInfoTextString on one event — the last writer decides.
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

    // Duplicate registration keeps A's position (the client's dup early-ret).
    s.run("FA:RegisterEvent('E'); order = ''").unwrap();
    s.fire_event("E", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ABC");

    // Unregister + re-register moves B to the TAIL (the node is freed, the re-add appends).
    s.run("FB:UnregisterEvent('E'); FB:RegisterEvent('E'); order = ''")
        .unwrap();
    s.fire_event("E", vec![]);
    assert_eq!(s.eval::<String>("return order").unwrap(), "ACB");
}

/// **`HasScript` answers "can this widget CARRY that kind", not "does it have one set".**
///
/// That distinction is the verb's whole purpose, and every corpus caller depends on it: they ask
/// before hooking, precisely when nothing is set yet.
///
/// ```lua
/// if parent:HasScript("OnMouseDown") then          -- Tablet-2.0.lua:2409
///     local script = parent:GetScript("OnMouseDown")
///     parent:SetScript("OnMouseDown", function() … end)
/// end
/// ```
///
/// It was the top session-start blocker — 32 of 39 `attempt to call method` failures were this one
/// name — and implementing it took survivors from 41 to 69.
///
/// The known over-permission is asserted too, so it is a recorded divergence rather than a
/// discovery: our table is flat where the reference's is per widget type, so a plain Frame answers
/// true for a Button-only kind. Exact for the base kinds, which is what the corpus asks about.
#[test]
fn has_script_reports_the_kind_is_supported_not_that_one_is_set() {
    let s = script();
    s.run(r#"f = CreateFrame("Frame", "HasScriptProbe")"#)
        .unwrap();

    // True with NOTHING set — the case every caller is actually in.
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

    // Tablet's exact idiom, run end to end: guard, read the (absent) handler, install one, fire it.
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

    // The recorded divergence: flat table, so a Frame says true for a Button-only kind. The
    // reference says false. Pinned so making SCRIPT_KINDS per-type has to come here and decide.
    assert!(
        s.eval::<bool>(r#"return f:HasScript("OnClick")"#).unwrap(),
        "over-permissive by design today — see the comment at the binding"
    );
}

/// **The walk steps by a next saved BEFORE the handler runs** (`0x703ee8`): a
/// handler that unregisters ITSELF mid-dispatch cannot rob its successor. This is AceEvent-2.0's
/// fire-once idiom for `PLAYER_LOGIN`/`VARIABLES_LOADED` — its frame unregisters inside the
/// handler, and the index-walk this replaces skipped whichever addon registered right after it
/// (Bagnon_Forever's DB never initialized; the director's SaveBagData error dialogs).
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
    // The removal held: a second fire reaches only A and C.
    s.fire_event("E", vec![]);
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(log, vec!["WalkA", "WalkB", "WalkC", "WalkA", "WalkC"]);
}

/// A handler that unregisters the walk's SAVED next ends the dispatch there — the reference frees
/// that node and walks into zeroed links (an accident we render as a deterministic stop) — while a
/// frame registered mid-dispatch tail-appends and is still visited.
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

/// `Frame:RegisterAllEvents()` — the frame's `OnEvent` receives every event dispatched
/// (`0x774c20`, table `0x878ec0`, argc 1, arity 0), and `UnregisterAllEvents` clears that state
/// along with the per-event ones.
///
/// **The clearing half is the load-bearing one.** AceEvent-2.0 — shipped by 63 vanilla addons and
/// 6 of the top 20 — turns all-events on once (`AceEvent.frame:RegisterAllEvents()`), deliberately
/// stops calling `frame:UnregisterEvent(event)` while it is on, and gets back to per-event
/// registration by calling `frame:UnregisterAllEvents()` and then re-registering each event it
/// still wants. A `RegisterAllEvents` that survived that call would leave every Ace2 addon on the
/// whole event stream for the session.
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

    // Arity 0 — the verb answers nothing.
    assert_eq!(s.arity("All:RegisterAllEvents()").unwrap(), 0);

    // Anything dispatched now reaches it, including an event no name list could have enumerated.
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

    // Registering twice is a no-op, and a frame holding BOTH an all-events registration and a
    // RegisterEvent for the same event is one listener, not two — the same rule `RegisterEvent`'s
    // own `if not already in the list` holds one level down.
    s.run(r#"seen = {}; All:RegisterAllEvents(); All:RegisterEvent("UNIT_HEALTH")"#)
        .unwrap();
    s.fire_event("UNIT_HEALTH", vec![]);
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        1,
        "fired once, not twice"
    );

    // UnregisterAllEvents clears BOTH: the explicit UNIT_HEALTH registration and the all-events one.
    s.run("seen = {}; All:UnregisterAllEvents()").unwrap();
    for ev in ["UNIT_HEALTH", "PLAYER_LOGIN"] {
        s.fire_event(ev, vec![]);
    }
    assert_eq!(
        s.eval::<i64>("return table.getn(seen)").unwrap(),
        0,
        "the all-events registration does not outlive UnregisterAllEvents"
    );

    // …and the AceEvent path back: re-register the individual events it still wants.
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

/// An all-events frame dispatches AFTER the event's own listeners — where it would sit if the
/// registration were expanded into every per-event list, since it joined later than they did.
/// Cross-frame order is a law consumers depend on (the two ZoneText frames writing one FontString);
/// it does not stop being one because a listener asked for everything.
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
