//! Times each workload in this interpreter. `cargo run --release -p benilla-lua --example bench`.

use std::time::Instant;

const WORKLOADS: &[(&str, &str)] = &[
    ("calls (fib 27)", "local function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end return fib(27)"),
    ("numeric loop", "local s = 0 for i = 1, 5000000 do s = s + i * 2 - 1 end return s"),
    ("table array", "local t = {} for i = 1, 1000000 do t[i] = i end local s = 0 for i = 1, 1000000 do s = s + t[i] end return s"),
    ("table hash (string keys)", "local t = {} for i = 1, 200000 do t['k' .. i] = i end local s = 0 for k, v in pairs(t) do s = s + v end return s"),
    ("table hash (number keys)", "local t = {} for i = 1, 200000 do t[i * 7919] = i end local s = 0 for k, v in pairs(t) do s = s + t[k] end for i = 1, 200000 do s = s + t[i * 7919] end return s"),
    ("pow (global __pow)", "local s = 0 for i = 1, 1000000 do s = s + i ^ 0.5 end return s"),
    ("method calls (OOP)", "local C = {} C.__index = C function C:add(x) self.v = self.v + x return self end local o = setmetatable({v = 0}, C) for i = 1, 1000000 do o:add(1) end return o.v"),
    ("closures", "local s = 0 for i = 1, 300000 do local f = function(x) return x + i end s = s + f(1) end return s"),
    ("string ops", "local n = 0 for i = 1, 100000 do local s = 'item' .. i .. ':' .. (i * 2) local a, b, k, v = string.find(s, '(%a+)(%d+)') n = n + string.len(s) + string.len(string.upper(k)) end return n"),
    ("gsub/format", "local n = 0 for i = 1, 50000 do local s = string.format('%d-%s-%5.2f', i, 'x', i / 3) s = string.gsub(s, '%-', '+') n = n + string.len(s) end return n"),
    ("tinsert/sort", "local t = {} for i = 1, 100000 do table.insert(t, math.mod(i * 7919, 100003)) end table.sort(t) return t[1]"),
];

fn time<F: FnMut()>(mut f: F) -> f64 {
    // Best of three.
    (0..3)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::MAX, f64::min)
}

fn main() {
    // `bench <name>`: that workload alone, five times, for a profiler.
    if let Some(only) = std::env::args().nth(1) {
        // `noop` measures the fixed cost: a fresh state and a trivial chunk.
        // `-e <chunk>`: any chunk, five times.
        let custom = std::env::args().nth(2).filter(|_| only == "-e");
        let src = if let Some(c) = custom.as_deref() {
            c
        } else if only == "noop" {
            "return 0"
        } else {
            WORKLOADS
                .iter()
                .find(|(n, _)| n.contains(&only))
                .expect("a workload name")
                .1
        };
        for _ in 0..5 {
            let mut vm = benilla_lua::new_state();
            vm.exec(src.as_bytes(), b"=bench").expect("runs");
        }
        return;
    }
    println!("{:<28} {:>10}", "workload", "ms");
    for (name, src) in WORKLOADS {
        // One state, the chunk compiled once outside the clock, the best of three runs in that
        // warm state.
        let mut vm = benilla_lua::new_state();
        let f = vm.load(src.as_bytes(), b"=bench").expect("compiles");
        let fr = vm.root(f);
        let ms = time(|| {
            let f = vm.rooted(fr);
            vm.pcall_value(f, &[], -1).expect("runs");
        });
        println!("{name:<28} {ms:>10.1}");
    }
}
