//! **What the host remembers about the VM, and how it forgets.**
//!
//! The UI VM does not live for the process — it is built at world entry and destroyed at the
//! character screen, once per login (the reference's `0x48fbf0` ↔ `0x490bd0`). That
//! makes every host-side "I already told the VM about this" a claim with an expiry date, and there
//! are two kinds of them:
//!
//! - **seeds** — a registry, a catalog, a keybinding set: pushed once, because pushing it is
//!   expensive and its content does not change. A `bool` latch, or a `PostStartup` system, which is
//!   the same latch written in the scheduler.
//! - **change memos** — "these are the action slots I last pushed", so an unchanged frame costs
//!   nothing. A `Local<FeedMemory>`, near-universally.
//!
//! Both are *correct* against one VM and *silently wrong* against the next, and the failure has no
//! error path: the push simply does not happen, and the window is empty. Session 2's action bar,
//! bags, spellbook, keybinds and macros all failed exactly this way, and one of them — the CVar
//! table — failed worse than empty: `save_config` composes `config.toml` from the VM's snapshot, so
//! an unseeded VM would have written the player's settings file back out **stripped**.
//!
//! [`VmMemo`] is the one mechanism for both. It keys the memory on
//! [`UiScript::session`] — a VM-side identity, not a host-side counter someone must remember to
//! bump — so a memory written against a dead VM cannot be read at all. The failure mode inverts:
//! forgetting to use it is visible (the memo is a plain `Local` again), while using it and being
//! wrong costs one redundant push per login.

use benilla_ui::script::UiScript;

/// A host-side memory about the UI VM — **valid only for the VM it was written against**.
///
/// Wrap the memo type and read it through [`VmMemo::get`]; against a VM other than the one that
/// last wrote it, the memo resets to `T::default()` first. The wrapped type is unchanged, so the
/// call sites keep their own shape:
///
/// ```ignore
/// mut memory: Local<VmMemo<FeedMemory>>,
/// // …
/// let memory = memory.get(&script);
/// if memory.pushed.get(&id) != Some(&slot) { /* push */ }
/// ```
pub(crate) struct VmMemo<T> {
    /// The VM this memory is about. `0` is "no VM" — [`UiScript::session`] hands out from 1, so a
    /// freshly defaulted memo matches nothing, and the first read of every session is a miss.
    session: u64,
    inner: T,
}

impl<T: Default> Default for VmMemo<T> {
    fn default() -> Self {
        Self {
            session: 0,
            inner: T::default(),
        }
    }
}

impl<T: Default> VmMemo<T> {
    /// The memory — **cleared first if this is a different VM** than the one that wrote it.
    pub(crate) fn get(&mut self, script: &UiScript) -> &mut T {
        self.get_for(Some(script))
    }

    /// [`VmMemo::get`] for a system that also runs with **no VM** — the character screen, where the
    /// session's Lua state does not exist (1290).
    ///
    /// "No VM" is a session in its own right, and it is session `0`: the memory resets once on the
    /// way into it and once on the way out, and holds in between. Reaching for a scratch default
    /// each frame instead would quietly turn the memo off for the whole glue phase.
    pub(crate) fn get_for(&mut self, script: Option<&UiScript>) -> &mut T {
        self.get_reset_for(script).0
    }

    /// [`VmMemo::get`], also reporting whether the memory RESET on this read — i.e. this is the
    /// first read against a new VM. A gated feed keys its "must run" on exactly
    /// this: with every input unchanged, a fresh VM still needs the full re-push, and the reset is
    /// the only signal that says so. The flag is true at most once per session per memo, so a gate
    /// that ORs it in costs nothing on the steady frames it exists to skip.
    pub(crate) fn get_reset(&mut self, script: &UiScript) -> (&mut T, bool) {
        self.get_reset_for(Some(script))
    }

    fn get_reset_for(&mut self, script: Option<&UiScript>) -> (&mut T, bool) {
        let now = script.map_or(0, UiScript::session);
        let reset = self.session != now;
        if reset {
            self.session = now;
            self.inner = T::default();
        }
        (&mut self.inner, reset)
    }
}

impl VmMemo<bool> {
    /// **True exactly once per VM** — the seed shape: `if seeded.claim(&script) { …push the
    /// registry… }`.
    pub(crate) fn claim(&mut self, script: &UiScript) -> bool {
        let done = self.get(script);
        if *done {
            false
        } else {
            *done = true;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole mechanism rests on: a memo written against one VM is not readable
    /// from the next, and a seed claimed in one session is claimable again in the one after.
    #[test]
    fn a_memo_does_not_survive_the_vm_it_was_written_against() {
        let first = UiScript::new().expect("VM");
        let second = UiScript::new().expect("VM");
        assert_ne!(
            first.session(),
            second.session(),
            "every VM gets its own identity"
        );

        let mut memo: VmMemo<Vec<u32>> = VmMemo::default();
        memo.get(&first).push(7);
        assert_eq!(memo.get(&first), &vec![7], "…and keeps it within one VM");
        assert!(
            memo.get(&second).is_empty(),
            "a memo about the previous VM reads as empty against the next one"
        );
        assert!(
            memo.get(&first).is_empty(),
            "and moving back does not resurrect it — the memory is gone, not shelved"
        );
    }

    /// The gate half (1439): `get_reset` reports the reset exactly once per session flip — the
    /// one frame a gated feed must run with every other input unchanged.
    #[test]
    fn get_reset_reports_each_session_flip_once() {
        let first = UiScript::new().expect("VM");
        let second = UiScript::new().expect("VM");
        let mut memo: VmMemo<u32> = VmMemo::default();

        let (m, reset) = memo.get_reset(&first);
        assert!(reset, "the first read of a session is the reset");
        *m = 7;
        let (m, reset) = memo.get_reset(&first);
        assert!(!reset, "steady frames read quietly");
        assert_eq!(*m, 7);
        let (m, reset) = memo.get_reset(&second);
        assert!(reset, "a new VM resets again");
        assert_eq!(*m, 0, "…and the memory is gone with the old one");
    }

    #[test]
    fn a_seed_is_claimed_once_per_vm() {
        let first = UiScript::new().expect("VM");
        let second = UiScript::new().expect("VM");
        let mut seeded = VmMemo::<bool>::default();

        assert!(seeded.claim(&first), "the first VM needs seeding");
        assert!(!seeded.claim(&first), "…and only once");
        assert!(seeded.claim(&second), "the next VM needs it again");
        assert!(!seeded.claim(&second));
    }

    /// **The rule, enforced instead of remembered.**
    ///
    /// A feed that memoizes what it pushed into the VM and does *not* key that memo on the session
    /// fails in the one way nothing catches: it pushes nothing into the new VM, the window is
    /// empty, and no error is raised anywhere. There is no runtime signal to test for — which is
    /// exactly why the check has to be structural.
    ///
    /// So: a `Local<…>` in the parameter list of a system that also takes the VM must be a
    /// [`VmMemo`], or be named in [`EXEMPT`] with the reason it is not host-memory-about-the-VM.
    /// Adding a feed the ordinary way passes; adding one with a bare `Local` fails here, at the
    /// line, before it can reach a login.
    #[test]
    fn a_local_in_a_system_that_holds_the_vm_is_keyed_on_the_session() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for file in rust_files(&src) {
            let text = std::fs::read_to_string(&file).expect("readable source");
            if !text.contains("UiScript") {
                continue; // no VM in this file, so no memory about one
            }
            let rel = file
                .strip_prefix(&src)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            for params in fn_parameter_lists(&text) {
                if !params.contains("UiScript") {
                    continue;
                }
                for (name, ty) in locals_in(params) {
                    if ty.contains("VmMemo") || EXEMPT.contains(&(rel.as_str(), name.as_str())) {
                        continue;
                    }
                    offenders.push(format!("{rel}: `{name}: Local<{ty}>`"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these memos outlive the VM they are about — wrap them in \
             `crate::ui_script::VmMemo<…>` and read them through `.get(&script)`, or add them to \
             `EXEMPT` with the reason they are not memory about the VM (decision 1290):\n  {}",
            offenders.join("\n  ")
        );
    }

    /// `Local`s that live in a system holding the VM but are **not memory about the VM**, each with
    /// the reason. Keyed `(path under src/, parameter name)`.
    const EXEMPT: &[(&str, &str)] = &[
        // A raster fact (the screen seam), not a VM push. The measurer beside it deliberately
        // re-seats off `!script.has_text_measurer()` — it interrogates the VM instead of a memo,
        // which is the same guarantee arrived at the other way.
        ("ui_script/extract/mod.rs", "last_seam"),
        // The plate driver's anti-overlap scratch (decision 2148 put the VM in that system's
        // hands). It is cleared at the top of every run and rebuilt from this frame's plates —
        // a reused allocation, not memory: nothing in it survives the call it was filled in, so
        // a new VM has nothing to stale.
        ("vplates.rs", "bucket"),
        // The window's `scale_factor` beside it — the other term a measure is
        // only correct under, since a logical height becomes an integer DEVICE-pixel raster size.
        // A fact about the window, not about the VM; it gates the same re-seat `last_seam` does,
        // and a fresh VM re-seats on `!has_text_measurer()` regardless.
        ("ui_script/extract/mod.rs", "last_dpi"),
        // Pushed unconditionally every frame; there is nothing remembered to go stale.
        ("ui_script/mod.rs", "smoothed"),
        // A fact about the CHAT ROSTER, not about the VM: whether the player has joined a
        // zone-dependent defense channel (the world-state readout's `Type == 1` gate). It is
        // recomputed whenever `ChannelState` changes and is the same answer either side of a
        // login; the push it feeds is memoized beside it in a real `VmMemo`, which is what
        // re-seats the new VM.
        ("world_state_ui.rs", "defense_channel"),
        // The cursor systems, both platform arms: these track the OS cursor and an `NSCursor` raw
        // pointer. The OS keeps that state across the VM's death and rebirth, so re-seating them at
        // a login would re-assert a cursor nothing changed. (`last_set` is the key we last handed
        // the window — the same fact under the same name on both arms.)
        ("cursor.rs", "was_looking"),
        ("cursor.rs", "rects_disabled"),
        ("cursor.rs", "decode_failed"),
        ("cursor.rs", "last_set"),
        ("cursor.rs", "last_ptr"),
    ];

    use crate::test_support::{fn_parameter_lists, rust_files};

    /// Every `name: Local<Ty>` in a parameter list, as `(name, Ty)` — angle-bracket matched, so a
    /// nested generic comes back whole.
    fn locals_in(params: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (i, _) in params.match_indices("Local<") {
            let open = i + "Local<".len();
            let mut depth = 1usize;
            let mut close = None;
            for (j, c) in params[open..].char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(close) = close else { continue };
            // Walk back over `: ` and `mut ` to the parameter's own name.
            let head = params[..i].trim_end().trim_end_matches(':').trim_end();
            let name = head
                .rsplit(|c: char| c == ',' || c == '(' || c.is_whitespace())
                .find(|s| !s.is_empty() && *s != "mut")
                .unwrap_or("<unnamed>");
            out.push((name.to_string(), params[open..close].trim().to_string()));
        }
        out
    }
}
