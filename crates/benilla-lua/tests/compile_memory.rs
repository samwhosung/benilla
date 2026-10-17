//! Compiling takes memory in proportion to the source: what hot reload keeps of a function's
//! identity (its key, each captured variable's declaration) is shared with the enclosing
//! function's and cut to a bounded name, a name built along a path of fields included, so no
//! source can make the compiler ask for more memory than the process has. This binary counts every allocation, so it holds this one test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to `System`; the counters only watch.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let live = LIVE.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(live, Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// The most memory live at once while `src` compiles, above what was live before.
fn peak_compiling(src: &str) -> usize {
    let mut heap = benilla_lua::heap::Heap::default();
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let p = benilla_lua::compile(src.as_bytes(), b"=t", &mut heap);
    assert!(p.is_ok(), "{:?}", p.err());
    PEAK.load(Relaxed) - base
}

#[test]
fn compile_memory_follows_the_source_not_its_nesting() {
    let long = "N".repeat(16 * 1024);
    let locals: Vec<String> = (1..=32).map(|i| format!("u{i}")).collect();
    let locals = locals.join(",");

    // A long-named function's 32 locals, as many as a closure may capture, captured by each
    // of 400 closures.
    let inner = format!("F = function() return {locals} end ").repeat(400);
    let captures = format!("function {long}() local {locals} {inner} end");
    // Functions nested 150 deep, each with a long name.
    let nested = format!(
        "{}{}",
        format!("function {long}() ").repeat(150),
        "end ".repeat(150)
    );

    // Constructors nested 150 deep under long field names, a function at each level: each
    // function's name is the whole path to it, as long as the source above it.
    let fields = format!(
        "T = {}{}",
        format!("{{f = function() end, {long} = ").repeat(150),
        "{}".to_string() + &"}".repeat(150)
    );

    for (what, src) in [
        ("captures", captures),
        ("nesting", nested),
        ("fields", fields),
    ] {
        let peak = peak_compiling(&src);
        assert!(
            peak < 16 * src.len() + (4 << 20),
            "{what}: {peak} bytes to compile {} bytes of source",
            src.len()
        );
    }
}
