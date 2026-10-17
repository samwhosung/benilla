//! Hot reload: new code for a chunk's functions, swapped into the closures that already exist.
//! Nothing reruns, so every table, upvalue and registration the chunk made stays; a handler a
//! frame holds, a hook in another addon's table, a callback in a list all run the new code from
//! their next call. A frame already executing finishes on the old code.

use std::collections::HashMap;
use std::rc::Rc;

use crate::heap::{FuncObj, LuaFunc};
use crate::proto::{Key, Proto};
use crate::vm::Vm;
use crate::SyntaxError;

/// What a reload did, by function key.
#[derive(Debug, Default)]
pub struct Report {
    /// Live closures now running new code.
    pub closures: usize,
    /// Functions whose live closures were swapped.
    pub swapped: Vec<String>,
    /// Functions left on the old code, a full reload's work: their captured variables changed,
    /// or their key is numbered and the count of functions sharing its name changed, so it may
    /// name another function now.
    pub reshaped: Vec<String>,
    /// Functions only the new source has; they exist once their definition runs again. A
    /// function the old source had without a live closure is none of these.
    pub added: Vec<String>,
    /// Functions only the old source had, with live closures still on the old code.
    pub removed: Vec<String>,
}

// A key's sibling count is a `Cell`, and no part of its hash or equality.
#[allow(clippy::mutable_key_type)]
fn walk(p: &Rc<Proto>, out: &mut HashMap<Key, Rc<Proto>>) {
    out.insert(p.key.clone(), p.clone());
    for q in &p.protos {
        walk(q, out);
    }
}

fn show(k: &Key) -> String {
    k.to_string()
}

impl Vm {
    /// Compile `src` as the chunk `chunkname` and swap each of its functions into the live
    /// closures of the same function. A syntax error leaves everything as it was.
    #[allow(clippy::mutable_key_type)] // as `walk`
    pub fn hot_reload(&mut self, chunkname: &[u8], src: &[u8]) -> Result<Report, SyntaxError> {
        let root = crate::compile::compile(src, chunkname, &mut self.heap)?;
        // A constant the compiler could not make: `not enough memory`, as `Vm::load` answers.
        if self.heap.take_oom() {
            return Err(SyntaxError(b"not enough memory".to_vec()));
        }
        self.heap.charge(root.tree_bytes());
        let mut fresh = HashMap::new();
        walk(&root, &mut fresh);
        let mut report = Report::default();
        let mut seen: HashMap<Key, bool> = HashMap::new();
        // Replaced code may still be running in a frame; it is kept until no Lua runs.
        if self.frames_empty() {
            self.retired.clear();
        }
        let mut retired = Vec::new();
        // The keys the old source had, from what its live closures carry.
        let mut old_keys: Vec<Rc<[Key]>> = Vec::new();
        for f in self.heap.funcs_mut() {
            let FuncObj::Lua(LuaFunc {
                proto,
                upvals,
                shape,
                ..
            }) = f
            else {
                continue;
            };
            // The main function ran once; it is not swapped.
            if &*proto.source != chunkname || proto.key.is_root() {
                continue;
            }
            if !old_keys.iter().any(|k| Rc::ptr_eq(k, &proto.chunk_keys)) {
                old_keys.push(proto.chunk_keys.clone());
            }
            let key = proto.key.clone();
            let Some(new) = fresh.get(&key) else {
                seen.entry(key).or_insert(false);
                continue;
            };
            // The new function's upvalues, by the declaration each captures, from the closure's
            // existing ones; none when the key may now name another function.
            let remap: Option<Vec<u32>> = if !new.key.same_counts(&proto.key) {
                None
            } else {
                new.upval_decls
                    .iter()
                    .map(|d| {
                        proto
                            .upval_decls
                            .iter()
                            .position(|o| o == d)
                            .map(|i| upvals[i])
                    })
                    .collect()
            };
            match remap {
                Some(ups) => {
                    *shape = crate::heap::Shape::of(new);
                    let old_proto = std::mem::replace(proto, new.clone());
                    let old_ups = std::mem::replace(upvals, crate::heap::Upvals::new(ups));
                    retired.push((old_proto, old_ups));
                    report.closures += 1;
                    seen.insert(key, true);
                }
                None => {
                    report.reshaped.push(show(&key));
                    seen.entry(key).or_insert(false);
                }
            }
        }
        self.retired.extend(retired);
        for (k, swapped) in &seen {
            if *swapped {
                report.swapped.push(show(k));
            } else if !fresh.contains_key(k) {
                report.removed.push(show(k));
            }
        }
        for k in fresh.keys() {
            let had = old_keys.iter().any(|ks| ks.contains(k));
            if !k.is_root() && !seen.contains_key(k) && !had {
                report.added.push(show(k));
            }
        }
        report.swapped.sort();
        report.reshaped.sort();
        report.reshaped.dedup();
        report.added.sort();
        report.removed.sort();
        Ok(report)
    }
}
