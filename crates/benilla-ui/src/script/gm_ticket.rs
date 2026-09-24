//! The GM trouble-ticket **Era API surface** — the seven globals `HelpFrame.lua`
//! is written against, and nothing else.
//!
//! All seven are engine bindings in 1.12 (`reference/1.12-globals.tsv` lists each as
//! `function`/`engine`), and the set is closed: the shipped Help window calls exactly
//! `GetGMTicketCategories`, `GetGMStatus`, `GetGMTicket`, `NewGMTicket`, `UpdateGMTicket`,
//! `DeleteGMTicket` and `Stuck`, and nothing else in the shipped UI calls any of them.
//!
//! **The five ticket verbs share ONE ordered queue** ([`GmTicketIntent`]), rather than a counter
//! each. Call order is the wire order in the real client, and a per-verb drain cannot preserve it:
//! with separate queues, `DeleteGMTicket(); GetGMTicket()` in a single chunk puts the *get* on the
//! wire first and the answer then describes the state before the delete. The shipped Help window
//! never issues two in one frame, so nothing on screen would ever have shown it — which is exactly
//! why it is worth getting right rather than leaving as a latent trap for the next addon, or for
//! our own probe (which hit it).
//!
//! A queue, not a set: **two calls are two sends**. That matters more here than for
//! [`super::binder`], because the ticket toast **re-polls `GetGMTicket()` every 10 minutes** — a
//! deduplicating drain would silently stop the poll. `Stuck()` keeps its own counter, because it
//! is not a ticket verb at all (see its registration below) and shares no ordering with them.
//!
//! **The category list is DBC data, pushed once.** `GetGMTicketCategories()` returns a flat
//! `id1, name1, id2, name2, …` vararg list, which `HelpFrameGM_UpdateCategories(...)` walks with
//! Lua 5.0's `arg`/`arg.n`. The ids are `GMTicketCategory.dbc`'s own and they are the values that
//! go on the wire, so this is an ordered list of pairs rather than an indexable table — see
//! [`benilla_formats::gm_ticket_category`] for why renumbering would misfile every ticket.
//!
//! **There is no `GetGMTicket` *getter*.** The reference's `GetGMTicket()` is a SEND, not a read:
//! the answer comes back as the `UPDATE_TICKET` event's arguments, and the window keeps whatever
//! it needs in its own frame fields. So nothing here holds a ticket snapshot — the same shape as
//! [`super::duel`] and [`super::binder`], and for the same reason.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One ticket verb the window called, in call order — the queue [`super::UiScript::take_gm_ticket_intents`]
/// drains. Ordered rather than counted so the wire sees what Lua did, in the order Lua did it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmTicketIntent {
    /// `GetGMTicket()` — `CMSG_GMTICKET_GETTICKET`.
    Ask,
    /// `GetGMStatus()` — `CMSG_GMTICKET_SYSTEMSTATUS`.
    AskStatus,
    /// `DeleteGMTicket()` — `CMSG_GMTICKET_DELETETICKET`.
    Delete,
    /// `NewGMTicket`/`UpdateGMTicket` — create or edit.
    Write(GmTicketWrite),
}

/// One `NewGMTicket` / `UpdateGMTicket` call, drained by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmTicketWrite {
    /// The `GMTicketCategory.dbc` id the window had selected — `HelpFrameOpenTicket.ticketType`,
    /// which is the clicked category row's own id.
    pub category: u32,
    /// The text as typed. The window's EditBox caps it at 500 letters; the engine's own cap is
    /// 1999, so nothing here truncates what the UI allowed.
    pub text: String,
    /// `true` for `NewGMTicket` (file), `false` for `UpdateGMTicket` (edit). Two different
    /// opcodes with two different answers, so the distinction survives to the app rather than
    /// being re-derived from whether a ticket is believed to exist — the reference picks the verb
    /// from its own `HelpFrameOpenTicket.hasTicket` flag and we must not second-guess it.
    pub is_new: bool,
}

impl super::UiScript {
    /// Publish the ticket categories `GetGMTicketCategories()` returns — the `GMTicketCategory.dbc`
    /// rows, in file order. Pushed once at load; the table is static client data.
    pub fn set_gm_ticket_categories(&mut self, categories: Vec<(u32, String)>) {
        self.model_mut().gm_ticket_categories = categories;
    }

    /// Drain every ticket verb the window called since the last drain, **in call order** — the
    /// module doc's reason. Each entry is exactly one packet.
    pub fn take_gm_ticket_intents(&mut self) -> Vec<GmTicketIntent> {
        std::mem::take(&mut self.model_mut().gm_ticket_intents)
    }

    /// Drain the `Stuck()` calls — each is one cast of spell 7355.
    pub fn take_stuck_casts(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().stuck_casts)
    }
}

/// Register the seven Help-window globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetGMTicketCategories() — the flat (id, name) vararg list, consumed by
    // `HelpFrameGM_UpdateCategories` as `arg[i]`/`arg[i+1]` pairs. Empty until the app has pushed
    // the DBC (a bare-XML harness, or a run with no client data): the window then simply paints no
    // category rows, which is the honest rendering of "we have no catalog" and cannot error.
    g.set(
        "GetGMTicketCategories",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let mut out = Vec::with_capacity(model.gm_ticket_categories.len() * 2);
            for (id, name) in &model.gm_ticket_categories {
                out.push(Value::Integer(i64::from(*id)));
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The three payload-free verbs. Each pushes onto the ONE ordered queue, so a chunk that calls
    // two of them sends two packets in the order it called them.
    //   GetGMTicket()    — "send me my ticket"; a send, answered as UPDATE_TICKET.
    //   GetGMStatus()    — "is the petition queue up?"; answered as UPDATE_GM_STATUS.
    //   DeleteGMTicket() — the HELP_TICKET_ABANDON_CONFIRM dialog's Yes.
    for (name, intent) in [
        ("GetGMTicket", GmTicketIntent::Ask),
        ("GetGMStatus", GmTicketIntent::AskStatus),
        ("DeleteGMTicket", GmTicketIntent::Delete),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .gm_ticket_intents
                    .push(intent.clone());
                Ok(())
            })?,
        )?;
    }

    // NewGMTicket(category, text) / UpdateGMTicket(category, text) — the window's Submit and its
    // Save Changes. Same signature, different opcode; the reference's own usage string is
    // `Usage: UpdateGMTicket(type, text)`, so (number, string) is the declared arity for both.
    for (name, is_new) in [("NewGMTicket", true), ("UpdateGMTicket", false)] {
        g.set(
            name,
            lua.create_function(move |lua, (category, text): (u32, String)| {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .gm_ticket_intents
                    .push(GmTicketIntent::Write(GmTicketWrite {
                        category,
                        text,
                        is_new,
                    }));
                Ok(())
            })?,
        )?;
    }

    // Stuck() — the Help window's "Auto-Unstuck". NOT a ticket verb despite living only in this
    // window: it casts spell 7355 "Stuck", whose SPELL_EFFECT_STUCK teleports the player to their
    // last safe position server-side. It is registered here because HelpFrame is its only caller
    // in the whole shipped UI, and a module for one binding would be worse than this note.
    g.set(
        "Stuck",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .stuck_casts += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}
