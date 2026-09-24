//! The Help window's seven engine globals ([`crate::script::gm_ticket`]).

use super::common::script;
use crate::script::{GmTicketIntent, GmTicketWrite, UiScript};

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
    let mut s = script();
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
    let mut s = script();
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
    let mut s = script();
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

/// The reference's usage string is `Usage: UpdateGMTicket(type, text)`. The binding queues any
/// text; the reference refuses an empty one before the send (`ERR_TICKET_NO_TEXT`, `0x5ef808`,
/// `0x5efae7`).
#[test]
fn an_empty_ticket_body_is_queued_not_swallowed() {
    let mut s = script();
    s.run("NewGMTicket(1, \"\")").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Write(GmTicketWrite {
            category: 1,
            text: String::new(),
            is_new: true,
        })]
    );
}
