//! The point of the experiment: the whole state is plain data, so a clone is a snapshot and an
//! assignment back is a restore.

fn run(vm: &mut benilla_lua::Vm, src: &str) -> String {
    match vm.exec(src.as_bytes(), b"=t") {
        Ok(v) => v.first().map_or(String::new(), |&v| vm.display(v)),
        Err(e) => format!("<raised> {}", vm.display(e)),
    }
}

#[test]
fn a_clone_is_a_snapshot_and_restoring_it_undoes_everything_after() {
    let mut vm = benilla_lua::new_state();
    run(
        &mut vm,
        "Counter = 0 Registry = {} function Bump() Counter = Counter + 1 return Counter end \
         local hidden = 'upvalue' function Peek() return hidden end",
    );
    let snap = vm.clone();
    // Everything an addon can do after the snapshot: globals, tables, metatables, closures,
    // upvalues, hooks of existing functions, the random generator.
    run(
        &mut vm,
        "Bump() Bump() Registry.x = 1 setmetatable(Registry, {__index = function() return 'meta' end}) \
         local orig = Bump Bump = function() return 'hooked' end strfind = nil math.random()",
    );
    assert_eq!(run(&mut vm, "return Counter .. ',' .. tostring(Bump()) .. ',' .. tostring(Registry.y) .. ',' .. type(strfind)"), "2,hooked,meta,nil");
    vm = snap.clone();
    assert_eq!(
        run(&mut vm, "return Counter .. ',' .. Bump() .. ',' .. tostring(Registry.y) .. ',' .. type(strfind) .. ',' .. Peek()"),
        "0,1,nil,function,upvalue"
    );
    // The snapshot itself is untouched by what ran on its restored copy.
    let mut again = snap;
    assert_eq!(run(&mut again, "return Counter"), "0");
}

/// A snapshot of a large state is the whole of it. The time it takes is printed, not asserted:
/// a wall clock on a shared machine makes no test.
#[test]
fn a_snapshot_of_a_large_state_is_whole() {
    let mut vm = benilla_lua::new_state();
    run(
        &mut vm,
        "Data = {} for i = 1, 100000 do local j = i Data[i] = {id = i, name = 'item' .. i, f = function() return j end} end",
    );
    let counts = vm.heap.counts();
    let t = std::time::Instant::now();
    let snap = vm.clone();
    let clone_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "snapshot of {} strings, {} tables, {} functions: {clone_ms:.1} ms",
        counts[0], counts[1], counts[2]
    );
    let mut snap = snap;
    assert_eq!(snap.heap.counts(), counts);
    run(&mut vm, "Data = nil collectgarbage()");
    assert_eq!(
        run(&mut snap, "return Data[100000].name .. Data[77777].f()"),
        "item10000077777"
    );
}

/// `math.random` draws from the VM's own generator: a clone continues the sequence on its own,
/// and reseeding one leaves the other.
#[test]
fn a_clone_draws_its_own_random_sequence() {
    let mut a = benilla_lua::new_state();
    let draw = |vm: &mut benilla_lua::Vm| -> Vec<f64> {
        (0..4)
            .map(|_| {
                vm.exec(b"r = math.random(1000)", b"=t").unwrap();
                vm.get_global("r").as_num().unwrap()
            })
            .collect()
    };
    a.exec(b"math.randomseed(7)", b"=t").unwrap();
    let mut b = a.clone();
    let from_a = draw(&mut a);
    b.exec(b"math.randomseed(99)", b"=t").unwrap();
    let mut c = a.clone();
    assert_eq!(
        draw(&mut c),
        draw(&mut a),
        "a clone continues where it was taken"
    );
    let mut a2 = benilla_lua::new_state();
    a2.exec(b"math.randomseed(7)", b"=t").unwrap();
    assert_eq!(
        draw(&mut a2),
        from_a,
        "the sequence follows the seed, not other VMs"
    );
}
