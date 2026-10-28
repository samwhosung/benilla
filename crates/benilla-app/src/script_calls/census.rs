//! The census lists: what a call's apply or an engine turn does while a handler it signalled is
//! held ([`benilla_ui::script::census`]), as `SignalEvent` (`0x703e50`) runs every listener before
//! the verb goes on. A kind made faithful leaves its list with its test, and none joins one.
//!
//! The census only counts; the reference says whether a count is a defect. A send counted under a
//! kind whose signal follows the send in the reference (the held handler then runs after the
//! packet too) is in order and sits on [`SENDS_IN_ORDER`], and a write counted after a signal sits
//! on [`WRITES_AFTER`] when the reference writes it ahead of the signal.

/// The call kinds the census counts that the reference orders the same way, each with its test:
/// - `Item`: an accepted pending swap sends `CMSG_SWAP_INV_ITEM` (`0x5e0fdb`), then clears the
///   cursor (`0x5e0fe7`), so the clear's handlers run after the packet, as they do held
///   (`an_accepted_swap_sends_before_its_held_clear_signals`).
/// - `UseAction`: the bar's equip signals `SetCursorItem`'s clear and put (`0x494bb6`, `0x494c54`)
///   in place, then sends (`0x5e1757`) and clears the cursor (`0x5e1763`), so that clear's handlers
///   run after the packet, as they do held
///   (`a_bar_equip_signals_its_cursor_set_ahead_of_its_send`).
pub(crate) const SENDS_IN_ORDER: &[&str] = &["Item", "UseAction"];

/// The `kind:member` pairs that write state a held handler can read after the signal, where the
/// reference writes it ahead of the signal; a turn's key is `turn@file:member`, its line left
/// out. Each with its test:
/// - `ClearTarget:action_states` and `turn@script_calls.rs:action_states`: a selection's cast
///   reports (`StopAttack`, `0x493a08`) signal ahead of its usable refresh (`0x493a69`), but the
///   reports wait for the point and the bar's states are written first
///   (`a_teardowns_strike_cancel_report_waits_behind_the_bars_usable_refresh`,
///   `an_engine_deselects_strike_cancel_report_waits_behind_the_bars_usable_refresh`).
pub(crate) const WRITES_AFTER: &[&str] = &[
    "ClearTarget:action_states",
    "turn@script_calls.rs:action_states",
];

/// The members every call's epilogue lends after its signals and publishes through: the cast
/// ladder's permit, the reports and the cast views it commits, the selection's and the cursor's
/// deferred steps. The epilogue writes nothing the call did not move, but it takes them `&mut`,
/// which the census cannot tell from a write, so [`writes_after`] leaves them out.
const EPILOGUE: &[&str] = &["cast", "reports", "cast_views", "select", "cursor"];

/// The census since the last take shows `kind` alone, which is on [`SENDS_IN_ORDER`].
pub(crate) fn shows_in_order(kind: &str) {
    let counted = benilla_ui::script::census::take();
    assert!(SENDS_IN_ORDER.contains(&kind), "{kind} is not on the list");
    assert_eq!(
        counted.keys().map(String::as_str).collect::<Vec<_>>(),
        [kind],
        "{counted:?}"
    );
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

/// The writes since the last take show `key` alone, which is on [`WRITES_AFTER`]: once they show
/// nothing, `key` is faithful, and leaves the list with the test that called this.
pub(crate) fn shows_writes_only(key: &str) {
    assert!(WRITES_AFTER.contains(&key), "{key} is not on the list");
    assert_eq!(writes_after(), [key]);
}
