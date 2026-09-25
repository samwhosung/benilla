//! Host-side memory about the UI VM, which lives for one login: the reference builds it at world
//! entry and destroys it at the character screen (`0x48fbf0`, `0x490bd0`). A seed or change memo
//! kept past its VM silently skips the push into the next one; [`VmMemo`] keys it on
//! [`UiScript::session`], so against a new VM it reads as fresh.

use benilla_ui::script::UiScript;

/// A host-side memo valid only for the VM that wrote it: [`VmMemo::get`] resets it to
/// `T::default()` when read against another.
pub(crate) struct VmMemo<T> {
    /// The VM this memory is about; 0 is no VM, as [`UiScript::session`] counts from 1.
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
    /// The memory, cleared first against a different VM.
    pub(crate) fn get(&mut self, script: &UiScript) -> &mut T {
        self.get_for(Some(script))
    }

    /// [`VmMemo::get`] for a system that also runs with no VM, at the character screen; no VM is
    /// session 0, so the memo holds there too.
    pub(crate) fn get_for(&mut self, script: Option<&UiScript>) -> &mut T {
        self.get_reset_for(script).0
    }

    /// [`VmMemo::get`], plus whether this read reset it: a gated feed must re-push into a fresh VM
    /// even when every input is unchanged.
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
    /// True exactly once per VM, for a seed pushed once per login.
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

    /// A `Local` in a system that takes the VM must be a [`VmMemo`] or be listed in [`EXEMPT`]: a
    /// stale memo fails silently, so the check is structural.
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
             `EXEMPT` with the reason they are not memory about the VM:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// `Local`s in VM-holding systems that are not memory about the VM, keyed
    /// `(path under src/, parameter name)`.
    const EXEMPT: &[(&str, &str)] = &[
        // A raster fact; a fresh VM re-seats its measurer on `!has_text_measurer()` anyway.
        ("ui_script/extract/mod.rs", "last_seam"),
        // The plate driver's anti-overlap scratch, cleared at the top of every run.
        ("vplates.rs", "bucket"),
        // The window's scale factor, which gates the same re-seat as `last_seam`.
        ("ui_script/extract/mod.rs", "last_dpi"),
        // Pushed unconditionally every frame; there is nothing remembered to go stale.
        ("ui_script/mod.rs", "smoothed"),
        // A fact about the chat roster (a defense channel joined); its push has its own `VmMemo`.
        ("world_state_ui.rs", "defense_channel"),
        // The cursor systems on both platforms track OS cursor state, which outlives the VM.
        ("cursor.rs", "was_looking"),
        ("cursor.rs", "rects_disabled"),
        ("cursor.rs", "decode_failed"),
        ("cursor.rs", "last_set"),
        ("cursor.rs", "last_ptr"),
    ];

    use crate::test_support::{fn_parameter_lists, rust_files};

    /// Every `name: Local<Ty>` in a parameter list as `(name, Ty)`, nested generics whole.
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
