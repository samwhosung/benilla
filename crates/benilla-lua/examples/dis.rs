//! Prints the bytecode of a chunk and its functions: `cargo run --example dis -- '<lua>'`.
fn dump(p: &benilla_lua::proto::Proto, depth: usize) {
    let pad = "  ".repeat(depth);
    println!(
        "{pad}function {} (params {}, maxstack {})",
        p.key, p.nparams, p.maxstack
    );
    for (i, op) in p.code.iter().enumerate() {
        println!("{pad}  {i:3} {op:?}");
    }
    for q in &p.protos {
        dump(q, depth + 1);
    }
}
fn main() {
    let src = std::env::args().nth(1).unwrap();
    let mut heap = benilla_lua::heap::Heap::default();
    let p = benilla_lua::compile(src.as_bytes(), b"=dis", &mut heap).unwrap();
    dump(&p, 0);
}
