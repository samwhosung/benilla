//! Programs the client compiles whose code generation leaves 5.1's own invariants unmet; a debug
//! build must compile and run them as the client does.

fn run(src: &str) -> String {
    let mut vm = benilla_lua::new_state();
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or("none".into(), |&v| vm.display(v)),
        Err(e) => vm.display(e),
    }
}

/// The client's `forlist` (`0x6fe540`) adjusts the values to its locals without freeing the
/// rest, so given more values than locals the loop's block opens with a register above them. A
/// local declared first in the body is then read from the register above the loop's variables,
/// where `OP_TFORLOOP` called the generator (`0x6f9344`): it holds the call's first result.
#[test]
fn a_generic_for_given_more_values_than_locals_compiles() {
    assert_eq!(
        run("for k in function() end, nil, nil, 42 do end return 7"),
        "7"
    );
    assert_eq!(
        run("local n = 0 for k in function(s, c) if c ~= 3 then return (c or 0) + 1 end end, nil, nil, 9, 8 do \
             do local y = k n = n + y end end return n"),
        "6"
    );
}

/// The same leaked register under a `return` in the loop's body: the values are returned from
/// the first free local (5.0's `retstat`), the call's result there, and a tail call still runs.
#[test]
fn a_return_in_a_generic_for_given_more_values_than_locals_compiles() {
    let f = "local function f(s, c) if not c then return 1 end end ";
    let r = |body: &str| run(&format!("{f}{body}"));
    assert_eq!(r("for k in f, nil, nil, 42 do return k, 5 end"), "1");
    assert_eq!(
        r("local function g(x) return 'g' .. tostring(x) end for k in f, nil, nil, 42 do return g(k) end"),
        "g1"
    );
}

/// In such a loop over `next`, the body's first locals read the registers from the call up: its
/// first result, the state the call was passed, the key `lua_next` wrote over the control, and
/// past what the call wrote, a value the list left or the nil a frame starts with, as the
/// client's `luaD_precall` clears every frame. With no more values than locals, nothing leaks.
#[test]
fn the_registers_above_a_generic_for_hold_what_the_client_leaves() {
    let t = "local t, o = {10, 20}, {} ";
    let r = |body: &str| run(&format!("{t}{body} return table.concat(o, ' ')"));
    assert_eq!(
        r("for k in next, t, nil, 42 do local y = k table.insert(o, type(y)) end"),
        "number number"
    );
    assert_eq!(
        r("for k in next, t, nil, 42, 43 do local y, z = k table.insert(o, type(z)) end"),
        "table table"
    );
    assert_eq!(
        r("for k in next, t, nil, 42, 43, 44 do local y, z, w = k table.insert(o, tostring(w)) end"),
        "1 2"
    );
    assert_eq!(
        r(
            "for k in next, t, nil, 42, 43, 44, 45, 46 do local y, z, w, u, q = k \
           table.insert(o, tostring(q)) end"
        ),
        "46 46"
    );
    assert_eq!(
        r("for k, v, w in next, t, nil, 42 do local y = k table.insert(o, tostring(y)) end"),
        "1 2"
    );
    // A generator that writes a register on one call and not on the next: the client's
    // `luaD_precall` cleared it, so the body reads nil the second time.
    assert_eq!(
        run("local o = {} \
             local function gen(s, c) if c == 0 then local a, b = 'v', 'w' return 1 end if c == 1 then return 2 end end \
             for k in gen, nil, 0, 1, 2, 3, 4, 5 do local y, z, w, u, q = k table.insert(o, tostring(q)) end \
             return table.concat(o, ' ')"),
        "w nil"
    );
}
