//! The profiler charges time to the chunk that ran it, and the census charges tables to the
//! chunk that made them.

const SLEEP_MS: u64 = 2;

fn source_id(vm: &benilla_lua::Vm, name: &[u8]) -> usize {
    vm.heap
        .sources
        .iter()
        .position(|s| s.as_deref() == Some(name))
        .expect("a compiled chunk")
}

/// Time in a host function lands on the chunk that called it, and a loop's on the chunk that runs
/// it. Neither is checked against a wall clock's upper bound: a sleep takes at least its time.
#[test]
fn time_and_tables_land_on_the_chunk_that_spent_and_made_them() {
    let mut vm = benilla_lua::new_state();
    vm.register("Sleep", |_, _| {
        std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));
        Ok(0)
    });
    vm.exec(
        b"function Heavy() local s = 0 for i = 1, 200000 do s = s + i end return s end \
          function Maker() Kept = {} for i = 1, 500 do Kept[i] = {i} end end",
        b"@Interface\\AddOns\\Heavy\\Heavy.lua",
    )
    .unwrap();
    vm.exec(
        b"function Light() Sleep() end function Both() Light() return Heavy() end",
        b"@Interface\\AddOns\\Light\\Light.lua",
    )
    .unwrap();
    vm.set_profiling(true);
    vm.exec(b"for i = 1, 50 do Both() end Maker()", b"=driver")
        .unwrap();
    let p = vm.profile.as_ref().unwrap().clone();
    let heavy = source_id(&vm, b"@Interface\\AddOns\\Heavy\\Heavy.lua");
    let light = source_id(&vm, b"@Interface\\AddOns\\Light\\Light.lua");
    let ns = |i: usize| p.ns.get(i).copied().unwrap_or(0);
    println!("heavy {} ns, light {} ns", ns(heavy), ns(light));
    assert!(ns(light) >= 50 * SLEEP_MS * 1_000_000, "the sleeps' caller");
    assert!(ns(heavy) > 0);
    let census = vm.heap.census();
    assert!(
        census[heavy].tables >= 501,
        "Maker's tables: {:?}",
        census[heavy]
    );
    assert!(census[light].tables < 5, "{:?}", census[light]);
    // Off, nothing more is charged.
    vm.set_profiling(false);
    assert!(vm.profile.is_none());
}

/// A chunk nothing refers to any more gives its source slot back at a collection: `loadstring`
/// names a chunk by its own code, so code built in a loop would otherwise hold every name. A
/// new chunk takes a freed slot, and the profile and census still charge the live ones.
#[test]
fn chunk_names_nothing_refers_to_are_collected() {
    let mut vm = benilla_lua::new_state();
    vm.exec(
        b"Kept = loadstring('return 7') Made = {}",
        b"@Interface\\AddOns\\Kept\\Kept.lua",
    )
    .unwrap();
    vm.exec(
        b"for i = 1, 200000 do local f = loadstring('return ' .. i) f() end",
        b"=driver",
    )
    .unwrap();
    vm.full_gc();
    vm.heap.free_unreferenced_sources();
    let live: Vec<String> = vm
        .heap
        .sources_in_order()
        .into_iter()
        .map(|(_, s)| String::from_utf8_lossy(s).into_owned())
        .collect();
    assert_eq!(
        live,
        [
            "=[host]",
            "compat.lua",
            "@Interface\\AddOns\\Kept\\Kept.lua",
            "return 7",
        ],
        "the host, the compat chunk, the file whose functions and table live and the chunk a \
         global holds; back in the host, the driver chunk is charged nothing"
    );
    assert!(
        vm.heap.sources.len() < 20_000,
        "{} slots: collections ran while the loop made chunks",
        vm.heap.sources.len()
    );
    let kept = source_id(&vm, b"@Interface\\AddOns\\Kept\\Kept.lua");
    assert!(vm.heap.census()[kept].tables >= 1);
    // A freed slot is taken again, by a chunk compiled after the ones still live.
    let before = vm.heap.sources.len();
    vm.exec(b"return 1", b"=again").unwrap();
    assert_eq!(vm.heap.sources.len(), before);
    let order = vm.heap.sources_in_order();
    assert_eq!(&**order.last().unwrap().1, b"=again");
}

/// Code that calls into another chunk through a native (`pcall`, a `sort` comparator, a `gsub`
/// function, an `__index` metamethod) is charged its own work after the call returns: the
/// chunk running is the caller's again, for the profile and for the tables it makes.
#[test]
fn work_after_a_call_through_a_native_lands_on_the_caller() {
    for (name, call) in [
        ("pcall", "pcall(B)"),
        ("sort", "local t = {3, 1, 2} table.sort(t, BLess)"),
        ("gsub", "string.gsub('abc', '%w', B)"),
        ("__index", "local _ = BIndexed.missing"),
    ] {
        let mut vm = benilla_lua::new_state();
        vm.exec(
            b"function B() return 'x' end function BLess(a, b) return a < b end \
              BIndexed = setmetatable({}, {__index = function() return 1 end})",
            b"@Interface\\AddOns\\B\\B.lua",
        )
        .unwrap();
        let a = format!(
            "function A() {call} local s = 0 for i = 1, 300000 do s = s + i end \
             Made = {{}} for i = 1, 300 do Made[i] = {{}} end end"
        );
        vm.exec(a.as_bytes(), b"@Interface\\AddOns\\A\\A.lua")
            .unwrap();
        vm.set_profiling(true);
        vm.exec(b"A()", b"=driver").unwrap();
        let p = vm.profile.as_ref().unwrap().clone();
        let (ia, ib) = (
            source_id(&vm, b"@Interface\\AddOns\\A\\A.lua"),
            source_id(&vm, b"@Interface\\AddOns\\B\\B.lua"),
        );
        let ns = |i: usize| p.ns.get(i).copied().unwrap_or(0);
        assert!(
            ns(ia) > 10 * ns(ib),
            "{name}: A's loop is A's, A {} ns, B {} ns",
            ns(ia),
            ns(ib)
        );
        let census = vm.heap.census();
        assert!(census[ia].tables >= 300, "{name}: {:?}", census[ia]);
        assert!(census[ib].tables < 5, "{name}: {:?}", census[ib]);
    }
}

/// A chunk Lua compiles from a string names the chunk that compiled it, while that one lives.
#[test]
fn a_loadstring_chunk_knows_the_chunk_that_compiled_it() {
    let mut vm = benilla_lua::new_state();
    vm.exec(
        b"F = loadstring('return 1')",
        b"@Interface\\AddOns\\A\\A.lua",
    )
    .unwrap();
    let a = source_id(&vm, b"@Interface\\AddOns\\A\\A.lua") as u32;
    let f = source_id(&vm, b"return 1") as u32;
    assert_eq!(vm.heap.source_parent(f), Some(a));
    assert_eq!(vm.heap.source_parent(a), None, "the host compiled A");
}
