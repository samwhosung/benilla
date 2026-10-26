//! Rust-driven tests of the Lua host, one module per subject; the shared `script()` fixture lives
//! in [`common`].

mod addon_argument_abi;
mod addon_index_space;
mod adopt;
mod anchor_slots;
mod anchors;
mod backdrop;
mod button;
mod button_fonts;
mod capture;
mod channel;
mod chat_bubble;
mod clear_points;
mod colour_args;
mod colour_bytes;
pub(in crate::script) mod common;
mod cooldown;
mod create_frame_template;
mod cvar_bindings;
mod dispatch_bench;
mod drag_pin;
mod drag_threshold;
mod dying_target;
mod end_to_end;
mod engine_updates;
mod european_text;
mod events;
mod flag_args;
mod font_links;
mod font_object;
mod frame_api;
mod frame_state;
mod generic_for;
mod glue_surface;
mod gm_ticket;
mod handler_prof;
mod hidden_defer;
mod host_state;
mod hover_publish;
mod hover_walk;
mod hyperlink;
mod index_usage;
mod input;
mod is_number_gates;
mod keep_rect;
mod keyboard;
mod layout_gate;
mod layout_mode;
mod layout_structure;
mod line_spacing;
mod measure;
mod message_justify;
mod minimap;
mod miss_tuples;
mod model_clock;
mod modelframe;
mod movable;
mod name_targets;
mod nameplate;
mod no_font;
mod number_text;
mod numeric_shape_gate;
mod object_model;
mod one_or_nil_methods;
mod open_layout;
mod protected_gate;
mod reference_surface;
mod region_map;
mod regions;
mod reputation_list;
mod resize_bounds;
mod screen_units;
mod script_lengths;
mod scrollframe;
mod set_font_args;
mod setparent;
mod shown_bit;
mod simplehtml;
mod size_changed;
mod skill_list;
mod slider;
mod statusbar;
mod stdlib;
mod string_usage_gates;
mod structure_queries;
mod talent;
mod taxi;
mod texcoord_font;
mod text_wrap;
mod texture_art;
mod tooltip;
mod tooltip_item;
mod tooltip_setter_flags;
mod tooltip_size;
mod tooltip_spell;
mod tooltip_unit;
mod tooltip_unit_status;
mod toplevel;
mod update_walk;
mod usage_lines;
mod usage_number_args;
mod usage_range_gates;
mod visibility;
mod wheel;
mod widget_args;
mod widget_surface;
mod world_press_hook;
mod worldframe;
mod worldmap;

#[test]
fn an_instruction_budget_turns_a_runaway_loop_into_an_error() {
    let s = crate::script::UiScript::new().unwrap();
    s.set_instruction_budget(5_000_000);
    let err = s
        .run("local i = 0 while true do i = i + 1 end")
        .expect_err("an unbounded loop must raise rather than run forever")
        .to_string();
    assert!(
        err.contains("instruction budget exhausted"),
        "the raise names itself so a report can tell it from an addon's own error: {err}"
    );
    assert!(
        s.instructions_used() >= 5_000_000,
        "the counter reports what was spent: {}",
        s.instructions_used()
    );

    assert_eq!(
        s.eval::<i64>("return 2 + 3").unwrap(),
        5,
        "the VM still evaluates after a budget raise"
    );

    // An ordinary chunk is untouched by a budget it never approaches.
    let t = crate::script::UiScript::new().unwrap();
    t.set_instruction_budget(200_000_000);
    t.run("BudgetOk = 0 for i = 1, 1000 do BudgetOk = BudgetOk + i end")
        .unwrap();
    assert_eq!(t.eval::<i64>("return BudgetOk").unwrap(), 500_500);
}

/// The budget's raise reaches the host past every `pcall` and `xpcall`, and runs no handler: a
/// loop under a protected call is stopped too.
#[test]
fn an_instruction_budget_is_caught_by_no_protected_call() {
    for src in [
        "while true do pcall(function() while true do end end) end",
        "xpcall(function() while true do end end, function() while true do end end)",
    ] {
        let s = crate::script::UiScript::new().unwrap();
        s.set_instruction_budget(5_000_000);
        let err = s.run(src).expect_err(src).to_string();
        assert!(err.contains("instruction budget exhausted"), "{src}: {err}");
    }
}

/// The budget counts Lua that a library function calls, as 5.1's count hook does: a `gsub`
/// replacement over three million characters runs four instructions a call on either VM
/// (`GETGLOBAL`, `ADD`, `SETGLOBAL`, `RETURN`), and a runaway one is stopped.
#[test]
fn the_instruction_budget_counts_lua_a_library_function_runs() {
    let s = crate::script::UiScript::new().unwrap();
    s.set_instruction_budget(u64::MAX);
    s.run("n = 0 string.gsub(string.rep('a', 3000000), 'a', function(c) n = n + 1 return c end)")
        .unwrap();
    assert_eq!(s.instructions_used(), 12_000_000);

    let s = crate::script::UiScript::new().unwrap();
    s.set_instruction_budget(5_000_000);
    let err = s
        .run("string.gsub(string.rep('a', 50000000), 'a', function(c) n = 1 return c end)")
        .expect_err("a library function's callbacks run inside the budget")
        .to_string();
    assert!(err.contains("instruction budget exhausted"), "{err}");
}

/// The hearth location's name; before the packet, or for a bind zone with no AreaTable row, the
/// `HOME_INN` text (`0x48db19`), never nil. `StaticPopup.lua:1742` formats it.
#[test]
fn get_bind_location_answers_the_pushed_name_and_the_inn_text_otherwise() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.eval::<()>(r#"HOME_INN = "your inn""#).unwrap();
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "your inn"
    );

    s.set_bind_location(Some("Stormwind City"));
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "Stormwind City"
    );
    assert_eq!(
        s.eval::<String>(r#"return "Bound: " .. GetBindLocation()"#)
            .unwrap(),
        "Bound: Stormwind City"
    );
    s.set_bind_location(Some("Ironforge"));
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "Ironforge"
    );
    s.set_bind_location(None);
    assert_eq!(
        s.eval::<String>("return GetBindLocation()").unwrap(),
        "your inn"
    );
}

/// `RequestTimePlayed()` is a call the host sends inside the verb; `TIME_PLAYED_MSG` carries the
/// answer.
#[test]
fn request_time_played_is_a_call_and_the_answer_arrives_as_an_event() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.keep_calls();

    assert_eq!(s.arity("RequestTimePlayed()").unwrap(), 0);
    // A count, not a latch: the packet is empty, so two asks in a frame are two sends.
    s.run("RequestTimePlayed() RequestTimePlayed()").unwrap();
    assert_eq!(
        s.take_played_time_asks(),
        3,
        "the eval above plus two more — each ask is its own CMSG_PLAYED_TIME"
    );
    assert_eq!(
        s.take_played_time_asks(),
        0,
        "the take empties the kept calls"
    );

    s.run(
        r#"
        TPSeen = nil
        local f = CreateFrame("Frame", "TPWatcher")
        f:RegisterEvent("TIME_PLAYED_MSG")
        f:SetScript("OnEvent", function() TPSeen = { arg1, arg2 } end)
        "#,
    )
    .unwrap();
    s.fire_event(
        "TIME_PLAYED_MSG",
        vec![
            crate::script::ScriptValue::Int(360_000),
            crate::script::ScriptValue::Int(7_200),
        ],
    );
    assert_eq!(
        s.eval::<(i64, i64)>("return TPSeen[1], TPSeen[2]").unwrap(),
        (360_000, 7_200),
        "total seconds played and seconds since the last level-up, in that order"
    );
}

/// `seterrorhandler`'s contract: an engine-caught script error goes to the chosen handler.
#[test]
fn a_chosen_error_handler_hears_engine_caught_errors_and_none_leaves_the_host_channel_alone() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "local f = CreateFrame('Frame') \
         f:RegisterEvent('PLAYER_ALIVE') \
         f:SetScript('OnEvent', function() error('boom from a handler') end)",
    )
    .unwrap();

    // With no handler set, as before FrameXML sets one, the dispatch adds nothing.
    s.fire_event("PLAYER_ALIVE", vec![]);
    let errors = s.take_errors();
    assert_eq!(
        errors.iter().filter(|e| e.contains("boom")).count(),
        1,
        "one error, once: {errors:?}"
    );

    // A chosen handler gets the message, and the host channel still records it.
    s.run("caught = {} seterrorhandler(function(msg) table.insert(caught, msg) end)")
        .unwrap();
    s.fire_event("PLAYER_ALIVE", vec![]);
    assert!(
        s.eval::<String>("return caught[1]")
            .unwrap()
            .contains("boom from a handler"),
        "the chosen handler received the engine-caught error"
    );
    assert_eq!(
        s.take_errors()
            .iter()
            .filter(|e| e.contains("boom"))
            .count(),
        1,
        "and the host channel recorded it too"
    );
}

#[test]
fn a_widget_handler_error_reaches_the_chosen_error_handler() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "caught = {} seterrorhandler(function(m) table.insert(caught, m) end) \
         local b = CreateFrame('Button') \
         b:SetScript('OnClick', function() error('click boom') end) \
         b:Click() \
         local sl = CreateFrame('Slider') \
         sl:SetMinMaxValues(0, 10) \
         sl:SetScript('OnValueChanged', function() error('slide boom') end) \
         sl:SetValue(5)",
    )
    .unwrap();
    let caught = s
        .eval::<String>("return table.concat(caught, ' | ')")
        .unwrap();
    assert!(caught.contains("click boom"), "OnClick: {caught}");
    assert!(caught.contains("slide boom"), "OnValueChanged: {caught}");
}

/// 1.12 calls the error handler with the one value the failed call raised (`0x703b40`: the
/// handler pushed, inserted below the message, `lua_call` 1 argument): the bare Lua message, no
/// `runtime error: ` word and no traceback, which only benilla's own log carries.
#[test]
fn the_error_handler_receives_the_bare_lua_message() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "caught = {} seterrorhandler(function(m) table.insert(caught, m) end) \
         local f = CreateFrame('Frame') \
         f:RegisterEvent('PLAYER_ALIVE') \
         f:SetScript('OnEvent', function() this:NoSuchMethod() end) \
         local b = CreateFrame('Button') \
         b:SetScript('OnClick', function() local t = nil; t.x = 1 end) \
         b:Click() \
         local c = CreateFrame('Button') \
         c:SetScript('OnClick', function() SetPortraitTexture() end) \
         c:Click()",
    )
    .unwrap();
    s.fire_event("PLAYER_ALIVE", vec![]);
    let caught: Vec<String> = s.eval("return caught").unwrap();
    assert!(caught.len() >= 3, "{caught:?}");
    for m in &caught {
        assert!(
            !m.contains("runtime error") && !m.contains("stack traceback") && !m.contains('\n'),
            "bare message, one line: {m:?}"
        );
    }
    for want in [
        "attempt to call method `NoSuchMethod' (a nil value)",
        ":1: attempt to index local `t' (a nil value)",
    ] {
        assert!(
            caught
                .iter()
                .any(|m| m.starts_with("[string \"") && m.ends_with(want)),
            "{want}: {caught:?}"
        );
    }
    // The host log keeps the decorated text.
    assert!(s.take_errors().iter().any(|e| e.contains("runtime error")));
}

/// `fire_global` carries `UPDATE_FACTION` from the faction-header verbs.
#[test]
fn a_self_unregistering_listener_does_not_rob_its_successor_on_the_internal_dispatch() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        r#"log = ""
        local a = CreateFrame("Frame") a:RegisterEvent("UPDATE_FACTION")
        a:SetScript("OnEvent", function() log = log .. "a" this:UnregisterEvent("UPDATE_FACTION") end)
        local b = CreateFrame("Frame") b:RegisterEvent("UPDATE_FACTION")
        b:SetScript("OnEvent", function() log = log .. "b" end)
        ExpandFactionHeader(0)"#,
    )
    .unwrap();
    assert_eq!(s.eval::<String>("return log").unwrap(), "ab");
}

/// Every runner calls its script under the error function (`0x704e4f`-`0x704e79`), and Lua 5.0
/// runs an error function at the raise: the handler sees the failing handler's `this`, `event` and
/// `arg1` (`this` is put back only at `0x704ea7`), its frames are on the stack for `debugstack`,
/// and it runs before the event's next listener.
#[test]
fn the_error_handler_runs_at_the_raise_inside_the_failing_handler() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run_chunk_named(
        b"order = '' seen = nil stack = ''\n\
          seterrorhandler(function(m)\n\
            order = order .. 'h'\n\
            seen = (this and this:GetName() or 'nil') .. ' ' .. tostring(event) .. ' ' .. tostring(arg1)\n\
            stack = debugstack()\n\
          end)\n\
          function FailingListener()\n\
            error('listener fault')\n\
          end\n\
          local a = CreateFrame('Frame', 'FirstListener')\n\
          a:RegisterEvent('CHAT_MSG_SAY')\n\
          a:SetScript('OnEvent', function() FailingListener() end)\n\
          local b = CreateFrame('Frame', 'NextListener')\n\
          b:RegisterEvent('CHAT_MSG_SAY')\n\
          b:SetScript('OnEvent', function() order = order .. 'b' end)\n",
        "@Interface\\AddOns\\Probe\\probe.lua",
    )
    .unwrap();
    s.fire_event(
        "CHAT_MSG_SAY",
        vec![crate::script::ScriptValue::Str("hello".into())],
    );
    assert_eq!(s.eval::<String>("return order").unwrap(), "hb");
    assert_eq!(
        s.eval::<String>("return seen").unwrap(),
        "FirstListener CHAT_MSG_SAY hello"
    );
    let stack = s.eval::<String>("return stack").unwrap();
    assert!(
        stack.contains("probe.lua:8: in function `FailingListener'"),
        "the failing function is on the stack: {stack}"
    );
    // The host log keeps its row.
    assert!(s.take_errors().iter().any(|e| e.contains("listener fault")));
}

/// `OnUpdate` runs through the same runner (`0x704f10`), with `arg1` the elapsed time.
#[test]
fn an_on_update_error_reaches_the_handler_with_its_this_and_arg1() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "seen = nil \
         seterrorhandler(function(m) seen = this:GetName() .. ' ' .. arg1 .. ' ' .. m end) \
         local f = CreateFrame('Frame', 'Ticker') \
         f:SetScript('OnUpdate', function() error('tick fault', 0) end)",
    )
    .unwrap();
    s.tick(0.5);
    assert_eq!(
        s.eval::<String>("return seen").unwrap(),
        "Ticker 0.5 tick fault"
    );
}

/// A file chunk runs under the error function too (`0x704ae0`, `0x704b68`): the handler hears
/// its error inside the run, with the file's frame on the stack. A file that fails to compile
/// goes to the error function directly (`0x704b42`), before the run answers its error.
#[test]
fn a_file_chunks_error_reaches_the_handler_as_it_runs() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "heard = {} seterrorhandler(function(m) table.insert(heard, m .. '|' .. debugstack()) end)",
    )
    .unwrap();
    s.run_chunk_named(
        b"local x = 1\nerror('file fault')\n",
        "@Interface\\AddOns\\Probe\\file.lua",
    )
    .expect_err("the file raised");
    s.run_chunk_named(b"x = = 1\n", "@Interface\\AddOns\\Probe\\bad.lua")
        .expect_err("the file failed to compile");
    let heard: Vec<String> = s.eval("return heard").unwrap();
    assert_eq!(heard.len(), 2, "{heard:?}");
    assert!(
        heard[0].starts_with("Interface\\AddOns\\Probe\\file.lua:2: file fault|")
            && heard[0].contains("file.lua:2: in main chunk"),
        "{heard:?}"
    );
    assert!(
        heard[1].starts_with("Interface\\AddOns\\Probe\\bad.lua:1: "),
        "{heard:?}"
    );
    assert!(
        !heard[1].contains("bad.lua:1: in"),
        "no failing frame: {heard:?}"
    );
}

/// The handler runs as the pcall's error function, unprotected (`0x703b6d`): its own error runs
/// it again for that error (`luaG_errormsg`, `0x6fc780`) until `luaD_call` throws status 5 at
/// depth 225 (`0x6f65cf`), and that is what the pcall answers. The next fire runs as before.
#[test]
fn an_error_handler_that_errors_runs_again_until_error_in_error_handling() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        "heard = {} \
         seterrorhandler(function(m) table.insert(heard, m) error('the handler is broken too') end) \
         local f = CreateFrame('Frame') \
         f:RegisterEvent('PLAYER_ALIVE') \
         f:SetScript('OnEvent', function() error('original fault') end)",
    )
    .unwrap();
    s.fire_event("PLAYER_ALIVE", vec![]);
    let errors = s.take_errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("original fault — error handler failed: error in error handling"),
        "{errors:?}"
    );
    let heard: Vec<String> = s.eval("return heard").unwrap();
    assert!(heard[0].ends_with("original fault"), "{heard:?}");
    assert!(heard.len() > 2, "{}", heard.len());
    // `luaD_call` raises `C stack overflow` at depth 200 (`0x6f65bf`), and the handler hears that too.
    assert!(
        heard[1..]
            .iter()
            .all(|m| m.ends_with("the handler is broken too") || m == "C stack overflow"),
        "{heard:?}"
    );
    s.run("heard = {}").unwrap();
    s.fire_event("PLAYER_ALIVE", vec![]);
    let again: Vec<String> = s.eval("return heard").unwrap();
    assert_eq!(again.len(), heard.len(), "the same depth again");
}
