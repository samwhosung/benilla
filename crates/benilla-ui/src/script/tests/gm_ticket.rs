//! The Help window's seven engine globals ([`crate::script::gm_ticket`]).

use super::common::script;
use crate::script::{GmTicketIntent, GmTicketWrite, UiScript};

/// A VM with the active player in the world, which the ticket writes need (`0x48ca29`,
/// `0x48cab9`).
fn in_world() -> UiScript {
    let mut s = script();
    s.set_unit(
        "player",
        Some(crate::script::UnitState {
            exists: true,
            ..Default::default()
        }),
    );
    s
}

/// The catalog as the app pushes it: `GMTicketCategory.dbc`'s own ids and order.
fn with_categories() -> UiScript {
    let mut s = script();
    s.set_gm_ticket_categories(vec![
        (1, "Stuck".into()),
        (2, "Behavior/Harassment".into()),
        (3, "Guild".into()),
    ]);
    s
}

/// The stock window walks it as `arg[i]`, `arg[i+1]` pairs up to `arg.n`.
#[test]
fn the_categories_come_back_as_a_flat_id_name_vararg_list() {
    let s = with_categories();
    let n: i64 = s
        .eval("local f = function(...) return arg.n end return f(GetGMTicketCategories())")
        .unwrap();
    assert_eq!(n, 6, "three categories = six varargs");

    let (id1, name1, id3, name3): (i64, String, i64, String) = s
        .eval(
            "local f = function(...) return arg[1], arg[2], arg[5], arg[6] end \
             return f(GetGMTicketCategories())",
        )
        .unwrap();
    assert_eq!((id1, name1.as_str()), (1, "Stuck"));
    assert_eq!((id3, name3.as_str()), (3, "Guild"));
}

/// `HelpFrameGM_UpdateCategories` stores the id as `button.key` and as the wire `ticketType`.
#[test]
fn the_ids_are_the_dbc_ids_not_list_positions() {
    let mut s = script();
    // Gappy and unsorted on purpose.
    s.set_gm_ticket_categories(vec![(10, "Character".into()), (4, "Item".into())]);
    let (a, b): (i64, i64) = s
        .eval("local f = function(...) return arg[1], arg[3] end return f(GetGMTicketCategories())")
        .unwrap();
    assert_eq!((a, b), (10, 4), "ids and order pass through untouched");
}

/// The bare-XML harness and a run with no client data have no catalog.
#[test]
fn an_absent_catalog_returns_nothing_rather_than_erroring() {
    let s = script();
    let n: i64 = s
        .eval("local f = function(...) return arg.n end return f(GetGMTicketCategories())")
        .unwrap();
    assert_eq!(n, 0);
}

/// Two `GetGMTicket()` calls are two packets: `TicketStatus_OnUpdate` re-polls every 10 minutes.
#[test]
fn the_payload_free_verbs_queue_rather_than_latch() {
    let mut s = in_world();
    s.keep_calls();
    s.run("GetGMTicket() GetGMTicket() GetGMStatus() DeleteGMTicket() Stuck() Stuck() Stuck()")
        .unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![
            GmTicketIntent::Ask,
            GmTicketIntent::Ask,
            GmTicketIntent::AskStatus,
            GmTicketIntent::Delete,
        ]
    );
    assert_eq!(s.take_stuck_casts(), 3, "Stuck is not a ticket verb");
    assert!(s.take_gm_ticket_intents().is_empty());
    assert_eq!(s.take_stuck_casts(), 0);
}

/// Call order is wire order: reversed, the get would answer with the ticket just deleted.
#[test]
fn the_queue_preserves_call_order_across_different_verbs() {
    let mut s = in_world();
    s.keep_calls();
    s.run("DeleteGMTicket() GetGMTicket()").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Delete, GmTicketIntent::Ask],
        "delete first, exactly as Lua called them"
    );

    // The other way round, so a fixed per-verb order cannot pass.
    s.run("GetGMTicket() DeleteGMTicket()").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Ask, GmTicketIntent::Delete]
    );

    s.run("NewGMTicket(1, \"a\") GetGMTicket()").unwrap();
    assert!(matches!(
        s.take_gm_ticket_intents().as_slice(),
        [GmTicketIntent::Write(_), GmTicketIntent::Ask]
    ));
}

/// Same signature, different opcodes; the stock window picks one from its own `hasTicket` flag.
#[test]
fn new_and_update_are_distinguishable_and_keep_their_order() {
    let mut s = in_world();
    s.keep_calls();
    s.run("NewGMTicket(4, \"My sword vanished.\") UpdateGMTicket(4, \"Still gone.\")")
        .unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![
            GmTicketIntent::Write(GmTicketWrite {
                category: 4,
                text: "My sword vanished.".into(),
                is_new: true,
            }),
            GmTicketIntent::Write(GmTicketWrite {
                category: 4,
                text: "Still gone.".into(),
                is_new: false,
            }),
        ]
    );
}

/// Both writes gate the type with `is-number` and the text with `is-number-or-string`
/// (`0x48c9d8`, `0x48c9e8`) and raise their own line (`0x842acc`, `0x842aec`); the type is
/// `_ftol`ed with no range check (`0x48c9fe`), so `-1` is sent as its 32-bit pattern and `"2.9"`
/// as 2.
#[test]
fn ticket_writes_raise_their_usage_line_and_take_any_number_type() {
    let mut s = in_world();
    s.keep_calls();
    for (verb, line) in [
        ("NewGMTicket", "Usage: NewGMTicket(type, text)"),
        ("UpdateGMTicket", "Usage: UpdateGMTicket(type, text)"),
    ] {
        for args in ["", "1", "nil, 'x'", "{}, 'x'", "1, true", "1, {}"] {
            let e = s.run(&format!("{verb}({args})")).unwrap_err().to_string();
            assert!(e.contains(line), "{verb}({args}) -> {e}");
        }
    }
    assert!(s.take_gm_ticket_intents().is_empty());
    s.run("NewGMTicket(-1, 'x') UpdateGMTicket('2.9', 5)")
        .unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![
            GmTicketIntent::Write(GmTicketWrite {
                category: u32::MAX,
                text: "x".into(),
                is_new: true,
            }),
            GmTicketIntent::Write(GmTicketWrite {
                category: 2,
                text: "5".into(),
                is_new: false,
            }),
        ]
    );
}

/// An empty text is refused inside the call and sends nothing: a new ticket tests the copied
/// text's first byte (`0x5ef802`-`0x5ef80a`), an update likewise (`0x5efae7`), and each raises
/// `ERR_TICKET_NO_TEXT` through `DisplayError` (`push 0x157`, `0x5ef80c`/`0x5efaeb`).
#[test]
fn an_empty_ticket_body_shows_no_text_and_sends_nothing() {
    use crate::script::ui_errors::shown::seen_in_call;
    let mut s = in_world();
    for verb in ["NewGMTicket", "UpdateGMTicket"] {
        let (seen, host) = seen_in_call(&mut s, &format!("{verb}(1, \"\")"));
        assert_eq!(seen, "ERR_TICKET_NO_TEXT", "{verb}");
        assert!(host.calls.is_empty(), "{verb} sends nothing");
    }
    // A one-character text goes out, with no line.
    let (seen, host) = seen_in_call(&mut s, "NewGMTicket(1, \"x\")");
    assert_eq!(seen, "");
    assert_eq!(host.calls.len(), 1);
}

/// With no active player the writes do nothing at all, an empty text included: the bindings find
/// the player before they call the sender (`0x48ca29`-`0x48ca30`, `0x48cab9`-`0x48cac0`), and the
/// sender looks again (`0x5ef79f`-`0x5ef7a6`).
#[test]
fn a_ticket_write_with_no_player_does_nothing() {
    use crate::script::ui_errors::shown::seen_in_call;
    let mut s = script();
    for chunk in [
        "NewGMTicket(1, \"stuck\")",
        "UpdateGMTicket(1, \"stuck\")",
        "NewGMTicket(1, \"\")",
    ] {
        let (seen, host) = seen_in_call(&mut s, chunk);
        assert_eq!(seen, "", "{chunk} shows nothing");
        assert!(host.calls.is_empty(), "{chunk} sends nothing");
    }
}

/// With no active player `GetGMTicket` and `DeleteGMTicket` send nothing: each finds the player
/// (`0x468550`, then `0x468460` with mask 0x10) and skips the sender on null (`0x48c9bd`,
/// `0x48cb8d`). `GetGMStatus` has no such gate (`0x48cfc0` builds message 0x21a straight away).
#[test]
fn ticket_get_and_delete_with_no_player_send_nothing() {
    let mut s = script();
    s.keep_calls();
    s.run("GetGMTicket() DeleteGMTicket() GetGMStatus()")
        .unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::AskStatus],
        "only the status ask goes out"
    );
}
