//! The state's lifetime and its handles: no cycle keeps a state alive, a callback's panic
//! leaves the state sound, a string read stays put, and another state's value is refused.

use std::cell::Cell;
use std::rc::Rc;

use benilla_mlua::{Function, Lua, Table, Value};

/// Sets its flag when dropped: kept in a state's app data, it says whether the state went.
struct Sentinel(Rc<Cell<bool>>);

impl Drop for Sentinel {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

/// A panic payload's text.
fn text(p: &(dyn std::any::Any + Send)) -> Option<&str> {
    p.downcast_ref::<&str>()
        .copied()
        .or_else(|| p.downcast_ref::<String>().map(String::as_str))
}

fn watched(lua: &Lua) -> Rc<Cell<bool>> {
    let gone = Rc::new(Cell::new(false));
    lua.set_app_data(Sentinel(gone.clone()));
    gone
}

#[test]
fn a_state_whose_closures_capture_its_handles_is_freed() {
    let lua = Lua::new();
    let gone = watched(&lua);
    let t = lua.create_table().unwrap();
    let s = lua.create_string("held").unwrap();
    let f = lua.create_function(|_, ()| Ok(1)).unwrap();
    let (ht, hs, hf) = (t.clone(), s.clone(), f.clone());
    let keep = lua
        .create_function(move |_, ()| {
            let _ = (&ht, &hs, &hf);
            Ok(())
        })
        .unwrap();
    lua.globals().set("keep", keep).unwrap();
    lua.globals().set("t", t).unwrap();
    drop((s, f));
    drop(lua);
    assert!(
        gone.get(),
        "the VM and its app data went with the last `Lua`"
    );
}

#[test]
fn a_handle_outliving_its_state_panics_on_use() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    drop(lua);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.raw_len()));
    assert_eq!(text(&*r.unwrap_err()), Some("Lua instance is destroyed"));
    // Dropping it touches nothing.
    drop(t);
}

fn panicking(lua: &Lua) -> Function {
    lua.create_function(|_, ()| -> benilla_mlua::Result<()> { panic!("callback broke") })
        .unwrap()
}

#[test]
fn a_callback_panic_is_a_lua_error_pcall_catches() {
    let lua = Lua::new();
    lua.globals().set("boom", panicking(&lua)).unwrap();
    let got: String = lua
        .load(
            "local ok, e = pcall(boom) return tostring(ok) .. ' ' .. type(e) .. ' ' .. tostring(e)",
        )
        .eval()
        .unwrap();
    assert_eq!(got, "false userdata callback broke");
    // The state still runs.
    assert_eq!(lua.load("return 1 + 1").eval::<i64>().unwrap(), 2);
}

/// A panic Lua caught and dropped is let go: its payload and text go once the collector frees
/// the error value, and all of a state's go with the state.
#[test]
fn a_caught_panic_is_let_go() {
    let lua = Lua::new();
    lua.globals().set("boom", panicking(&lua)).unwrap();
    lua.load("for i = 1, 100 do pcall(boom) end")
        .exec()
        .unwrap();
    assert_eq!(benilla_mlua::kept_panics(), 100);
    lua.gc_collect().unwrap();
    lua.load("Kept = {pcall(boom)}").exec().unwrap();
    assert_eq!(benilla_mlua::kept_panics(), 1);
    assert_eq!(
        lua.load("return tostring(Kept[2])")
            .eval::<String>()
            .unwrap(),
        "callback broke"
    );
    drop(lua);
    assert_eq!(benilla_mlua::kept_panics(), 0);
}

#[test]
fn an_uncaught_callback_panic_resumes_in_rust() {
    let lua = Lua::new();
    lua.globals().set("boom", panicking(&lua)).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lua.load("boom()").exec()));
    let payload = r.expect_err("the panic comes back out");
    assert_eq!(text(&*payload), Some("callback broke"));
    assert_eq!(lua.load("return 2 + 2").eval::<i64>().unwrap(), 4);
}

#[test]
fn a_panic_in_a_fork_leaves_the_template_sound() {
    let template = Lua::new();
    template.globals().set("x", 7).unwrap();
    template
        .globals()
        .set("boom", panicking(&template))
        .unwrap();
    let fork = template.fork();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fork.load("boom()").exec()));
    assert!(r.is_err());
    let caught: bool = fork.load("return not pcall(boom)").eval().unwrap();
    assert!(caught);
    drop(fork);
    // The template reads its own VM, not the dropped fork's.
    assert_eq!(template.globals().get::<i64>("x").unwrap(), 7);
    for _ in 0..1000 {
        template.create_string("filler").unwrap();
    }
    assert_eq!(template.globals().get::<i64>("x").unwrap(), 7);
}

#[test]
fn a_string_read_stays_put_while_the_heap_grows() {
    let lua = Lua::new();
    let s = lua.create_string("short sentinel").unwrap();
    let long = lua
        .create_string("a sentinel longer than the inline limit")
        .unwrap();
    let (b, l) = (s.as_bytes(), long.to_str().unwrap());
    let keep: Vec<_> = (0..10_000)
        .map(|i| lua.create_string(format!("s{i}")).unwrap())
        .collect();
    lua.gc_collect().unwrap();
    assert_eq!(&*b, b"short sentinel");
    assert_eq!(&*l, "a sentinel longer than the inline limit");
    drop(keep);
}

#[test]
fn another_states_value_is_refused() {
    let a = Lua::new();
    let b = Lua::new();
    let foreign: Table = a.create_table().unwrap();
    foreign.set("x", 1).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.globals().set("foreign", foreign.clone())
    }));
    assert_eq!(
        text(&*r.unwrap_err()),
        Some("Lua instance passed Value created from a different main Lua state")
    );
    // A fork is another state too, outside a callback.
    let fork = a.fork();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fork.globals().set("t", Value::Table(foreign.clone()))
    }));
    assert!(r.is_err());
    assert_eq!(
        b.load("return foreign").eval::<Value>().unwrap(),
        Value::Nil
    );
}

/// A freed host closure's captures drop after the collection, as host code on the VM: a
/// destructor that uses the state reaches it soundly, and runs.
#[test]
fn a_destructor_run_by_the_collector_can_use_the_state() {
    struct D(Lua, Rc<Cell<bool>>);
    impl Drop for D {
        fn drop(&mut self) {
            let f = self.0.create_function(|_, ()| Ok(5)).unwrap();
            assert_eq!(f.call::<i64>(()).unwrap(), 5);
            self.1.set(true);
        }
    }
    let lua = Lua::new();
    let ran = Rc::new(Cell::new(false));
    let d = D(lua.clone(), ran.clone());
    let f = lua
        .create_function(move |_, ()| {
            let _ = &d;
            Ok(())
        })
        .unwrap();
    drop(f);
    lua.gc_collect().unwrap();
    assert!(ran.get(), "the destructor ran");
    // And from a collection the interpreter starts itself.
    let ran = Rc::new(Cell::new(false));
    let d = D(lua.clone(), ran.clone());
    let f = lua
        .create_function(move |_, ()| {
            let _ = &d;
            Ok(())
        })
        .unwrap();
    drop(f);
    lua.load("for i = 1, 20000 do local t = {} end")
        .exec()
        .unwrap();
    assert!(
        ran.get(),
        "the destructor ran inside the interpreter's collection"
    );
}

/// A destructor the interpreter's own collection runs may grow the value stack, moving it: the
/// frame that was running reads and writes its registers where they now are.
#[test]
fn a_destructor_that_moves_the_stack_leaves_the_running_frame_sound() {
    struct Grow(Lua);
    impl Drop for Grow {
        fn drop(&mut self) {
            let deep = "local function r(n) if n > 0 then return 1 + r(n - 1) end return 0 end \
                        return r(4000)";
            assert_eq!(self.0.load(deep).eval::<i64>().unwrap(), 4000);
        }
    }
    let lua = Lua::new();
    let g = Grow(lua.clone());
    let f = lua
        .create_function(move |_, ()| {
            let _ = &g;
            Ok(())
        })
        .unwrap();
    drop(f);
    // A table a pass, so a collection runs at a `NewTable` with the loop's registers live.
    // Registers written through a stale pointer corrupt the heap, which can hang the loop: a
    // watchdog fails the test instead.
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watched = done.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if !watched.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("the loop after the stack moved never ended");
            std::process::abort();
        }
    });
    let src = "local s = 0 for i = 1, 100000 do local t = {} local x = i s = s + x end return s";
    assert_eq!(lua.load(src).eval::<i64>().unwrap(), 5_000_050_000);
    done.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// A value whose conversion collects garbage, as a conversion that runs Lua may.
struct Collects(f64);

impl benilla_mlua::IntoLua for Collects {
    fn into_lua(self, lua: &Lua) -> benilla_mlua::Result<Value> {
        lua.gc_collect()?;
        // Strings made after the collection take the ids it freed.
        for i in 0..64 {
            lua.create_string(format!("filler {i}"))?;
        }
        Ok(Value::Number(self.0))
    }
}

#[test]
fn a_key_made_for_a_set_survives_the_value_conversion() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    t.set("a key no one else holds", Collects(7.0)).unwrap();
    t.raw_set(String::from("another such key"), Collects(8.0))
        .unwrap();
    let keys: Vec<String> = t
        .clone()
        .pairs::<String, f64>()
        .map(|kv| kv.unwrap().0)
        .collect();
    assert!(
        keys.contains(&"a key no one else holds".to_string()),
        "{keys:?}"
    );
    assert!(keys.contains(&"another such key".to_string()), "{keys:?}");
    assert_eq!(t.get::<f64>("a key no one else holds").unwrap(), 7.0);
    assert_eq!(t.raw_get::<f64>("another such key").unwrap(), 8.0);
}

#[test]
fn a_value_handed_over_by_value_survives_a_collecting_newindex() {
    let lua = Lua::new();
    // The collection frees what nothing roots; the new tables take the freed slots.
    let t: Table = lua
        .load(
            "return setmetatable({}, {__newindex = function(t, k, v) \
             collectgarbage() for i = 1, 64 do local _ = {} end rawset(t, k, v) end})",
        )
        .eval()
        .unwrap();
    let fresh = lua.create_table().unwrap();
    fresh.set("inner", 5).unwrap();
    t.set(lua.create_string("a fresh key").unwrap(), fresh)
        .unwrap();
    let back: Table = t.raw_get("a fresh key").unwrap();
    assert_eq!(back.get::<i64>("inner").unwrap(), 5);
}

/// An error that carries a Lua value prints, `{:?}` and `Display`, after its state is gone: the
/// value waits out of the error, which is `Send` and `Sync` as mlua's is.
#[test]
fn an_error_outliving_its_state_prints() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<benilla_mlua::Error>();
    let e = {
        let lua = Lua::new();
        lua.load("error({})").exec().unwrap_err()
    };
    assert!(format!("{e:?}").contains("LuaValue"));
    assert!(e.to_string().starts_with("runtime error: "));
    let e2 = e.clone();
    drop(e);
    assert!(format!("{e2:?}").contains("LuaValue"));
}

/// The value an error carries comes back to its own state, and goes when the error does.
#[test]
fn an_error_carries_its_value_back_to_its_state() {
    let lua = Lua::new();
    let t = lua.create_table().unwrap();
    lua.globals().set("E", t.clone()).unwrap();
    let e = lua.load("error(E)").exec().unwrap_err();
    match e.lua_value(&lua) {
        Some(Value::Table(v)) => assert!(v.equals(&t).unwrap()),
        other => panic!("{other:?}"),
    }
    let other = Lua::new();
    assert!(
        e.lua_value(&other).is_none(),
        "another state's value stays its own"
    );
}
