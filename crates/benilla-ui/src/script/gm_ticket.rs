//! The seven GM ticket globals `HelpFrame.lua` calls. The five ticket verbs send inside their
//! calls, through the host ([`GmTicketCall`]), so call order is the wire order, and two calls are
//! two sends: the ticket toast re-polls `GetGMTicket()` every 10 minutes. `GetGMTicket()` is a
//! send, answered by the `UPDATE_TICKET` event, so nothing here holds a ticket.

use mlua::{Lua, MultiValue, Value};

use super::Model;

/// A GM verb the host applies inside its call ([`super::ScriptCall::GmTicket`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmTicketCall {
    /// A ticket verb's packet.
    Ticket(GmTicketIntent),
    /// `Stuck()` (`0x489370`) past the gate: `CastSpellAsActivePlayer` (`0x6e5a90`) on its spell.
    Stuck,
}

/// One ticket verb the window called, sent in call order.
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
    /// The text as typed, never empty (the verb refuses one); the EditBox caps it at 500, under
    /// the reference's 1999.
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

    /// The ticket verbs a harness's VM kept ([`Self::keep_calls`]), in call order, one packet
    /// each.
    pub fn take_gm_ticket_intents(&mut self) -> Vec<GmTicketIntent> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::GmTicket(GmTicketCall::Ticket(intent)) => Some(intent.clone()),
            _ => None,
        })
    }

    /// The `Stuck()` calls a harness's VM kept, each one cast of spell 7355.
    #[cfg(test)]
    pub fn take_stuck_casts(&mut self) -> u32 {
        self.take_calls_like(&super::ScriptCall::GmTicket(GmTicketCall::Stuck)) as u32
    }
}

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: GmTicketCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::GmTicket(call))
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
    // With no active player `GetGMTicket` and `DeleteGMTicket` call nothing (`0x48c9bd`,
    // `0x48cb8d`); `GetGMStatus` sends regardless (`0x48cfc0`).
    for (name, intent, needs_player) in [
        ("GetGMTicket", GmTicketIntent::Ask, true),
        ("GetGMStatus", GmTicketIntent::AskStatus, false),
        ("DeleteGMTicket", GmTicketIntent::Delete, true),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                if needs_player
                    && !super::battlefield_queue::player_exists(
                        lua,
                        &lua.app_data_ref::<Model>().expect("model app_data"),
                    )
                {
                    return Ok(());
                }
                submit(lua, GmTicketCall::Ticket(intent.clone()))
            })?,
        )?;
    }

    // Submit and Save Changes: one signature, `(type, text)` in the reference's usage string. An
    // empty text sends nothing and shows `ERR_TICKET_NO_TEXT` (`push 0x157; call 0x496720`): the
    // senders test the copied text's first byte, a new ticket at `0x5ef802`-`0x5ef80a` and an
    // update at `0x5efadf`-`0x5efae9`. Stock `HelpFrame.lua:478-486` leaves the check to them.
    for (name, is_new, usage) in [
        ("NewGMTicket", true, "Usage: NewGMTicket(type, text)"),
        ("UpdateGMTicket", false, "Usage: UpdateGMTicket(type, text)"),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, (category, text): (Value, Value)| {
                // `is-number` on the type and `is-number-or-string` on the text, else the verb's
                // usage line (`0x842acc`, `0x842aec`; `0x48ca40`, `0x48cad0`). The type is
                // `_ftol`ed with no range check (`0x48c9fe`), so a negative one is sent.
                let category = super::binding_abi::index_arg32(lua, category, usage)?;
                let text = super::binding_abi::string_arg(lua, text, usage)?;
                // With no active player the binding calls nothing (`0x48ca29`-`0x48ca30`,
                // `0x48cab9`-`0x48cac0`).
                if !super::battlefield_queue::player_exists(
                    lua,
                    &lua.app_data_ref::<Model>().expect("model app_data"),
                ) {
                    return Ok(());
                }
                if text.is_empty() {
                    return super::calls::display_error(lua, "ERR_TICKET_NO_TEXT");
                }
                submit(
                    lua,
                    GmTicketCall::Ticket(GmTicketIntent::Write(GmTicketWrite {
                        category,
                        text,
                        is_new,
                    })),
                )
            })?,
        )?;
    }

    // The Help window's Auto-Unstuck, not a ticket verb: it casts spell 7355, whose
    // `SPELL_EFFECT_STUCK` moves the player to a safe position server-side.
    g.set(
        "Stuck",
        lua.create_function(|lua, ()| {
            // `0x48937c` → `TryCastSpell`, behind the gate's tag 2 (`0x6e4bc1`).
            if super::protected::gate(lua, super::protected::Tag::CAST) {
                submit(lua, GmTicketCall::Stuck)?;
            }
            Ok(())
        })?,
    )?;

    Ok(())
}
