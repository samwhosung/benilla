//! The census list: the call kinds that still send while a handler they signalled is held
//! ([`benilla_ui::script::census`]), as `SignalEvent` (`0x703e50`) runs every listener before the
//! verb goes on. A kind made faithful leaves the list with its test, and none joins it.

/// The call kinds that still send while a handler they signalled waits, each with its test:
/// - `Loot`: the sweep's money line (`0x4c2170` → `0x49a870`) waits behind its rows' stores
///   (`ui_loot`'s `a_sweep_sends_its_rows_ahead_of_its_held_money_line`);
/// - `UseAction`: the bar's equip, whose cursor clear signals (`0x494bb6`) ahead of the
///   `AutoEquipCursorItem` send (`a_bar_equip_sends_ahead_of_its_held_cursor_signals`).
pub(crate) const SENDS_AHEAD: &[&str] = &["Loot", "UseAction"];

/// The census since the last take shows `kind` alone, which is on [`SENDS_AHEAD`]: once it shows
/// nothing, `kind` is faithful, and leaves the list with the test that called this.
pub(crate) fn shows_only(kind: &str) {
    let counted = benilla_ui::script::census::take();
    assert!(SENDS_AHEAD.contains(&kind), "{kind} is not on the list");
    assert_eq!(
        counted.keys().map(String::as_str).collect::<Vec<_>>(),
        [kind],
        "{counted:?}"
    );
}
