//! The census lists: what a call's apply or an engine turn does while a handler it signalled is
//! held ([`benilla_ui::script::census`]), as `SignalEvent` (`0x703e50`) runs every listener before
//! the verb goes on.
//!
//! The census only counts; the reference says whether a count is a defect. No call writes after a
//! handler the reference runs ahead of the write, and none sends while a handler it signalled is
//! held.

/// The members every call's epilogue lends after its signals and publishes through: the cast
/// ladder's permit, the reports and the cast views it commits, the selection's and the cursor's
/// deferred steps. The epilogue writes nothing the call did not move, but it takes them `&mut`,
/// which the census cannot tell from a write, so [`writes_after`] leaves them out.
const EPILOGUE: &[&str] = &["cast", "reports", "cast_views", "select", "cursor"];

/// The census since the last take counts no send made while a handler was held.
pub(crate) fn shows_no_send() {
    let counted = benilla_ui::script::census::take();
    assert!(counted.is_empty(), "{counted:?}");
}

/// The `kind:member` pairs that wrote after a held handler since the last take, sorted: the
/// lends that began after the signal, without the epilogue's members ([`EPILOGUE`]) and a turn's
/// line. A lend that raised the signal itself is not here: the census cannot order its writes
/// against the signal.
pub(crate) fn writes_after() -> Vec<String> {
    let mut keys: Vec<String> = benilla_ui::script::census::take_writes()
        .into_iter()
        .filter(|(_, writes)| writes.after > 0)
        .filter_map(|(key, _)| {
            let (head, member) = key.rsplit_once(':')?;
            if EPILOGUE.contains(&member) {
                return None;
            }
            let head = match head.rsplit_once(':') {
                Some((file, line)) if head.starts_with("turn@") && line.parse::<u32>().is_ok() => {
                    file
                }
                _ => head,
            };
            Some(format!("{head}:{member}"))
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}
