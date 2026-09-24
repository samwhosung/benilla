//! The seven GM ticket globals `HelpFrame.lua` calls. The five ticket verbs share one ordered
//! queue, because call order is the reference's wire order, and two calls are two sends: the
//! ticket toast re-polls `GetGMTicket()` every 10 minutes. `GetGMTicket()` is a send, answered by
//! the `UPDATE_TICKET` event, so nothing here holds a ticket.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// One ticket verb the window called, drained in call order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmTicketIntent {
    /// `GetGMTicket()`: `CMSG_GMTICKET_GETTICKET`.
    Ask,
    /// `GetGMStatus()`: `CMSG_GMTICKET_SYSTEMSTATUS`.
    AskStatus,
    /// `DeleteGMTicket()`: `CMSG_GMTICKET_DELETETICKET`.
    Delete,
    /// `NewGMTicket` or `UpdateGMTicket`.
    Write(GmTicketWrite),
}

/// One `NewGMTicket` or `UpdateGMTicket` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmTicketWrite {
    /// The selected `GMTicketCategory.dbc` id (`HelpFrameOpenTicket.ticketType`).
    pub category: u32,
    /// The text as typed; the EditBox caps it at 500, under the reference's 1999.
    pub text: String,
    /// `NewGMTicket` or `UpdateGMTicket`, two opcodes: the window picks by its own `hasTicket`
    /// flag, and the app must not re-derive it.
    pub is_new: bool,
}

impl super::UiScript {
    /// The `GMTicketCategory.dbc` rows in file order, pushed once; the ids are the wire's values,
    /// so renumbering them would misfile every ticket.
    pub fn set_gm_ticket_categories(&mut self, categories: Vec<(u32, String)>) {
        self.model_mut().gm_ticket_categories = categories;
    }

    /// Drain the ticket verbs in call order, one packet each.
    pub fn take_gm_ticket_intents(&mut self) -> Vec<GmTicketIntent> {
        std::mem::take(&mut self.model_mut().gm_ticket_intents)
    }

    /// Drain the `Stuck()` calls, each one cast of spell 7355.
    pub fn take_stuck_casts(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().stuck_casts)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // A flat `id, name, ...` vararg list, walked in pairs by `HelpFrameGM_UpdateCategories`;
    // empty until the app pushes the DBC.
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

    // Sends: `GetGMTicket` is answered by `UPDATE_TICKET`, `GetGMStatus` by `UPDATE_GM_STATUS`.
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

    // Submit and Save Changes: one signature, `(type, text)` in the reference's usage string.
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

    // The Help window's Auto-Unstuck, not a ticket verb: it casts spell 7355, whose
    // `SPELL_EFFECT_STUCK` moves the player to a safe position server-side.
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
