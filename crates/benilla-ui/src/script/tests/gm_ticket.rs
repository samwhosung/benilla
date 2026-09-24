//! The Help window's seven engine globals ([`crate::script::gm_ticket`]).
//!
//! Two claims carry the whole feature and each has a test named after it: the category list is
//! **flat pairs consumed by Lua 5.0 varargs** (that is what `HelpFrameGM_UpdateCategories` reads),
//! and the four payload-free verbs **count** rather than latch (that is what keeps the window's
//! 10-minute ticket poll alive).

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

/// **`GetGMTicketCategories()` returns a FLAT (id, name) vararg list**, which the shipped window
/// walks as `arg[i]` / `arg[i+1]` pairs with `arg.n` — the Lua 5.0 convention. A table here, or a
/// list of names alone, would leave every category button unlabelled or unclickable.
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
    // The THIRD pair is at 5/6 — pairs, not a parallel pair of lists.
    assert_eq!((id3, name3.as_str()), (3, "Guild"));
}

/// **The ids are the DBC's, not indices.** `HelpFrameGM_UpdateCategories` stores `arg[index]` as
/// both `button.key` (an index into `HELPFRAME_FRAMES`) and `button.ticketType` (what goes on the
/// wire), so a renumbered list would misfile every ticket under the wrong heading.
#[test]
fn the_ids_are_the_dbc_ids_not_list_positions() {
    let mut s = script();
    // A deliberately gappy, unsorted catalog — nothing may re-index it.
    s.set_gm_ticket_categories(vec![(10, "Character".into()), (4, "Item".into())]);
    let (a, b): (i64, i64) = s
        .eval("local f = function(...) return arg[1], arg[3] end return f(GetGMTicketCategories())")
        .unwrap();
    assert_eq!((a, b), (10, 4), "ids and order pass through untouched");
}

/// No catalog is not an error: the window paints no category rows and cannot raise. This is the
/// bare-XML harness, and a run with no client data.
#[test]
fn an_absent_catalog_returns_nothing_rather_than_erroring() {
    let s = script();
    let n: i64 = s
        .eval("local f = function(...) return arg.n end return f(GetGMTicketCategories())")
        .unwrap();
    assert_eq!(n, 0);
}

/// **The verbs queue, they do not latch.** Two `GetGMTicket()` calls are two packets, because the
/// ticket toast re-polls every 10 minutes from `TicketStatus_OnUpdate` — a latch would collapse the
/// second poll into the first and the window would look hung.
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
    // Drained means drained — the next frame must not re-send.
    assert!(s.take_gm_ticket_intents().is_empty());
    assert_eq!(s.take_stuck_casts(), 0);
}

/// **Call order is wire order.** A chunk that abandons and then re-asks must reach the server in
/// that order — reversed, the get answers with the state *before* the delete and the window is
/// told it still has the ticket it just abandoned. Per-verb drains cannot express this, which is
/// why there is one queue.
#[test]
fn the_queue_preserves_call_order_across_different_verbs() {
    let mut s = script();
    s.run("DeleteGMTicket() GetGMTicket()").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Delete, GmTicketIntent::Ask],
        "delete first, exactly as Lua called them"
    );

    // And the other way round, so the test cannot pass on a fixed per-verb ordering that merely
    // happens to match one of the two cases.
    s.run("GetGMTicket() DeleteGMTicket()").unwrap();
    assert_eq!(
        s.take_gm_ticket_intents(),
        vec![GmTicketIntent::Ask, GmTicketIntent::Delete]
    );

    // Writes interleave with the bare verbs on the same queue.
    s.run("NewGMTicket(1, \"a\") GetGMTicket()").unwrap();
    assert!(matches!(
        s.take_gm_ticket_intents().as_slice(),
        [GmTicketIntent::Write(_), GmTicketIntent::Ask]
    ));
}

/// **The window's verb survives to the app.** `NewGMTicket` and `UpdateGMTicket` share a signature
/// but not an opcode, and the shipped window picks between them from its own `hasTicket` flag —
/// so the choice is carried, never re-derived from what the client believes.
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

/// The Era arity is `(number, string)` — the reference's own usage string is
/// `Usage: UpdateGMTicket(type, text)`. An empty ticket is the *window's* refusal to make, not
/// ours: the binding queues whatever it is handed, and the engine's own emptiness check sits
/// upstream of the send.
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
