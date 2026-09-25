//! The mail bindings. The app pushes the open mailbox's inbox, already resolved from the wire
//! (`UiScript::set_mail`), and the Lua verbs queue intents the app drains and sends. Indices are
//! 1-based, as `MailFrame.lua` uses them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// One inbox row, resolved by the app from a wire `MailListEntry`; its 1-based index is its
/// position in [`MailState::inbox`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailInboxRow {
    /// `packageIcon`, the enclosed item's icon; `None` with no item or while its template loads.
    pub package_icon: Option<String>,
    pub stationery_icon: Option<String>,
    /// `sender`, from the name cache; `None` while in flight or for a non-player sender.
    pub sender: Option<String>,
    pub subject: String,
    pub money: u32,
    pub cod: u32,
    pub days_left: f32,
    /// The enclosed stack's count, answered as `hasItem`; 0 (nil) is no item.
    pub item_count: u32,
    /// `checked & READ(0x1)`.
    pub was_read: bool,
    /// `checked & RETURNED(0x2)`.
    pub was_returned: bool,
    /// `checked & COPIED(0x4)`: the letter was copied to an item (`textCreated`).
    pub text_created: bool,
    /// `sender_guid.is_some() && !was_returned`, the Reply button's gate (`MailFrame.lua:281`).
    pub can_reply: bool,
    /// `stationery == 61`: a GM mail (vmangos `MAIL_STATIONERY_GM`, `Mail.h:85`).
    pub is_gm: bool,
    /// The letter body; `None` until fetched, while `GetInboxText` answers `""`.
    pub body: Option<String>,
    pub stationery_texture: String,
    /// `isInvoice`: only an auction won (result 1) or sold (2), not an outbid or expiry notice
    /// (`0x4af2eb`).
    pub is_invoice: bool,
    /// `GetInboxInvoiceInfo`'s answer; `None`, the reference's miss, also while the body or the
    /// counterparty's name is in flight.
    pub invoice: Option<MailInvoice>,
    /// `item_text_id != 0`: the mail has a letter body to fetch.
    pub has_body: bool,
    /// The enclosed item's template entry, 0 for none; the key `SetInboxItem`'s tooltip reads.
    pub item_id: u32,
    /// The enclosed item's random-suffix id (the wire `randomPropId`), 0 for none, which the
    /// reference's `SetInboxItem` hands the tooltip (`+0x424`); `item_name` carries its suffix.
    pub item_random_property_id: u32,
    /// The enclosed item's name; `None` while its template is in flight.
    pub item_name: Option<String>,
    pub item_texture: Option<String>,
    pub item_quality: Option<u32>,
    /// `InboxItemCanDelete`: `!can_reply`, as a player's unreturned mail is returned instead
    /// (`MailFrame.lua:417`, `:450`).
    pub can_delete: bool,
}

/// One auction invoice, `GetInboxInvoiceInfo`'s answer, parsed by the app from the mail's subject
/// and body text as the reference `sscanf`s them (`0x4af360`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailInvoice {
    /// `"seller"` (your auction sold) or `"buyer"` (you won), literal tokens, not global strings.
    pub seller: bool,
    pub item_name: String,
    /// The counterparty: who bought it (seller invoice) or who sold it (buyer invoice).
    pub player_name: String,
    /// The winning bid; the window reads `bid == buyout` as a buyout (`MailFrame.lua:307`).
    pub bid: u32,
    pub buyout: u32,
    /// Seller invoices only; 0 on a buyer's, whose body carries three fields, not five.
    pub deposit: u32,
    /// The auction house's cut, seller invoices only.
    pub consignment: u32,
}

/// The open mailbox's inbox, pushed whole by the app; `set_mail(None)` means no mailbox is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub inbox: Vec<MailInboxRow>,
}

/// One usable stationery in the send tab's picker; the app builds the list from `Stationery.dbc`
/// and the bags, sorted by price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationeryView {
    /// The `Stationery.dbc` row id, which `SelectStationery` stores and `CMSG_SEND_MAIL` carries.
    pub id: u32,
    pub name: String,
    /// The item's icon as a full `Interface\Icons\` path, which `MailFrame.lua:671` sets as is.
    pub icon: String,
    /// The item's BuyPrice in copper; `None` (nil) when the player carries one.
    pub cost: Option<u32>,
    /// The bare texture basename that `GetSelectedStationeryTexture` answers.
    pub texture: String,
}

/// A drained `SendMail` intent, which the app turns into `CMSG_SEND_MAIL`.
#[derive(Clone, Debug, PartialEq)]
pub struct MailSendRequest {
    pub target: String,
    pub subject: String,
    pub body: String,
    /// The selected `Stationery.dbc` id; never 0, as a send with none aborts (`0x4ae8dd`).
    pub stationery: u32,
    pub money: u32,
    pub cod: u32,
    /// The attached item's `(bag, 1-based slot)`; the app resolves its guid when the send fires,
    /// as the reference re-reads the slot then.
    pub item: Option<(i64, u32)>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open mailbox's inbox snapshot.
    pub fn set_mail(&mut self, state: Option<MailState>) {
        self.model_mut().mail = state;
    }

    /// Take the `CheckInbox` flag; the app answers it with `CMSG_GET_MAIL_LIST`.
    pub fn take_mail_check_inbox(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_check_inbox)
    }

    /// Drain the 1-based indices `GetInboxText` opened; the app marks each read, fetches its body.
    pub fn take_mail_opens(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_opens)
    }

    /// Drain the 1-based `TakeInboxItem` row picks.
    pub fn take_mail_take_items(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_take_items)
    }

    /// Drain the 1-based `TakeInboxMoney` row picks.
    pub fn take_mail_take_money(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_take_money)
    }

    /// Drain the 1-based `DeleteInboxItem` row picks.
    pub fn take_mail_deletes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_deletes)
    }

    /// Drain the 1-based `ReturnInboxItem` row picks.
    pub fn take_mail_returns(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_returns)
    }

    /// Drain the 1-based `TakeInboxTextItem` row picks, each a `CMSG_MAIL_CREATE_TEXT_ITEM`.
    pub fn take_mail_take_texts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_take_texts)
    }

    /// Take the `CloseMail` flag; 1.12 has no close opcode, so the app only ends its mail session.
    pub fn take_mail_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_close)
    }

    /// Drain the `SendMail` intent with the current money, COD and attachment. The attachment
    /// stays until [`Self::reset_compose_tab`], so a failed send keeps it.
    pub fn take_mail_send(&mut self) -> Option<MailSendRequest> {
        let mut model = self.model_mut();
        let (target, subject, body) = model.mail_send.take()?;
        let item = model.mail_send_item.as_ref().map(|it| (it.bag, it.slot));
        Some(MailSendRequest {
            target,
            subject,
            body,
            stationery: model.mail_stationery,
            money: model.mail_send_money,
            cod: model.mail_send_cod,
            item,
        })
    }

    /// The reference's compose-tab reset (`0x4acdc0`): clear the attachment, money and COD, then
    /// fire `SEND_MAIL_MONEY_CHANGED`, `SEND_MAIL_COD_CHANGED` and `MAIL_SEND_SUCCESS` in that
    /// order. The events must follow the clear, as `SendMailFrame_Reset` re-reads
    /// `GetSendMailItem`. `MAIL_SEND_SUCCESS` means the form is clean: opening a mailbox fires it.
    pub fn reset_compose_tab(&mut self) {
        {
            let mut model = self.model_mut();
            model.mail_send_item = None;
            model.mail_send_money = 0;
            model.mail_send_cod = 0;
        }
        // Fired now, not queued: the order against the caller's `MAIL_SHOW`/`MAIL_FAILED` matters.
        // One literal call each, as the event census (`ui_script::reference_ui`) reads them.
        self.fire_event("SEND_MAIL_MONEY_CHANGED", Vec::new()); // 0x4ace14
        self.fire_event("SEND_MAIL_COD_CHANGED", Vec::new()); // 0x4ace1e
        self.fire_event("MAIL_SEND_SUCCESS", Vec::new()); // 0x4ace28
    }

    /// `SendMail`'s abort when the attached item left its slot (`ERR_ITEM_NOT_FOUND`, no packet,
    /// `MAIL_SEND_INFO_UPDATE` at `0x4ae98d`): drop the attachment, keep the money and COD.
    pub fn drop_send_mail_item(&mut self) {
        let mut model = self.model_mut();
        if model.mail_send_item.take().is_some() {
            model
                .pending_events
                .push(("MAIL_SEND_INFO_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Push the usable stationery list, in the picker's order.
    pub fn set_mail_stationeries(&mut self, list: Vec<StationeryView>) {
        self.model_mut().mail_stationeries = list;
    }

    /// Clear the stationery selection, as the reference does on mailbox open and close
    /// (`0x4ace07`).
    pub fn clear_stationery(&mut self) {
        self.model_mut().mail_stationery = 0;
    }

    /// Push `HasNewMail()`'s answer, independent of any open mailbox. The app sets it before each
    /// `UPDATE_PENDING_MAIL`, on which the minimap icon re-reads it (`Minimap.xml:278-289`).
    pub fn set_has_new_mail(&mut self, has: bool) {
        self.model_mut().has_new_mail = has;
    }
}

/// A clone of the inbox row at a 1-based index.
fn row_at(model: &Model, index: usize) -> Option<MailInboxRow> {
    model
        .mail
        .as_ref()
        .and_then(|m| index.checked_sub(1).and_then(|n| m.inbox.get(n)))
        .cloned()
}

/// Register the mail globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetInboxNumItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.mail.as_ref().map_or(0, |m| m.inbox.len()) as i64)
        })?,
    )?;

    // GetInboxHeaderInfo(index): the 13 values `MailFrame.lua:105` reads; nil out of range.
    g.set(
        "GetInboxHeaderInfo",
        lua.create_function(|lua, index: usize| {
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, index)
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let opt_str = |s: &Option<String>| -> mlua::Result<Value> {
                Ok(match s {
                    Some(v) => Value::String(lua.create_string(v)?),
                    None => Value::Nil,
                })
            };
            // packageIcon only for a non-GM mail with an item (`MailFrame.lua:108`).
            let package_icon = if r.item_id != 0 && !r.is_gm {
                opt_str(&r.package_icon)?
            } else {
                Value::Nil
            };
            Ok(MultiValue::from_vec(vec![
                package_icon,
                opt_str(&r.stationery_icon)?,
                opt_str(&r.sender)?,
                Value::String(lua.create_string(&r.subject)?),
                Value::Integer(i64::from(r.money)),
                Value::Integer(i64::from(r.cod)),
                Value::Number(f64::from(r.days_left)),
                if r.item_count > 0 {
                    Value::Integer(i64::from(r.item_count))
                } else {
                    Value::Nil
                },
                flag(r.was_read),
                flag(r.was_returned),
                flag(r.text_created),
                flag(r.can_reply),
                flag(r.is_gm),
            ]))
        })?,
    )?;

    // GetInboxText(index): body, stationeryTexture, isTakeable, isInvoice (`MailFrame.lua:292`);
    // reading a mail queues its open. isTakeable with `not textCreated` gates the copy button
    // (`MailFrame.lua:366`), the pair vmangos requires (`MailHandler.cpp:867`).
    g.set(
        "GetInboxText",
        lua.create_function(|lua, index: usize| {
            let row = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let row = row_at(&model, index);
                if row.is_some() {
                    // Queued once per drain.
                    let i = index as u32;
                    if !model.mail_opens.contains(&i) {
                        model.mail_opens.push(i);
                    }
                }
                row
            };
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                // An invoice's body is nil (`0x4af1cf`): its text is the auction house's
                // bookkeeping, which only `GetInboxInvoiceInfo` parses.
                if r.is_invoice {
                    Value::Nil
                } else {
                    Value::String(lua.create_string(r.body.as_deref().unwrap_or(""))?)
                },
                Value::String(lua.create_string(&r.stationery_texture)?),
                flag(r.has_body),
                flag(r.is_invoice),
            ]))
        })?,
    )?;

    // GetInboxInvoiceInfo(index) (`MailFrame.lua:302`): always seven values (`0x4af5a1`), a miss
    // being three nils and four zeros (`0x4af559`).
    g.set(
        "GetInboxInvoiceInfo",
        lua.create_function(|lua, index: usize| {
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, index)
            };
            let miss = || {
                MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(0),
                ])
            };
            let Some(inv) = row.and_then(|r| r.invoice) else {
                return Ok(miss());
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(if inv.seller { "seller" } else { "buyer" })?),
                Value::String(lua.create_string(&inv.item_name)?),
                Value::String(lua.create_string(&inv.player_name)?),
                Value::Integer(i64::from(inv.bid)),
                Value::Integer(i64::from(inv.buyout)),
                Value::Integer(i64::from(inv.deposit)),
                Value::Integer(i64::from(inv.consignment)),
            ]))
        })?,
    )?;

    // GetInboxItem(index): name, texture, count, quality, canUse (`MailFrame.lua:379`); canUse is
    // the shared item-usable gate, true while the template is in flight.
    g.set(
        "GetInboxItem",
        lua.create_function(|lua, index: usize| {
            let (row, usable) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let row = row_at(&model, index);
                let usable = row
                    .as_ref()
                    .is_none_or(|r| super::item_stats::item_usable_by_id(&model, r.item_id));
                (row, usable)
            };
            // Five values on every path; with no item, `nil, nil, 0, 0, nil` (`0x4af5d0`).
            let Some(r) = row.filter(|r| r.item_id != 0) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Nil,
                ]));
            };
            let name = match &r.item_name {
                Some(n) => Value::String(lua.create_string(n)?),
                None => Value::Nil,
            };
            let texture = match &r.item_texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            // A number on every path, as in `GetSendMailItem`.
            let quality = Value::Integer(i64::from(r.item_quality.unwrap_or(0)));
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(r.item_count.max(1))),
                quality,
                flag(usable),
            ]))
        })?,
    )?;

    g.set(
        "InboxItemCanDelete",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(row_at(&model, index).is_some_and(|r| r.can_delete)))
        })?,
    )?;

    g.set(
        "HasNewMail",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.has_new_mail))
        })?,
    )?;

    g.set(
        "CheckInbox",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .mail_check_inbox = true;
            Ok(())
        })?,
    )?;

    for (name, field) in [
        ("TakeInboxItem", 0u8),
        ("TakeInboxMoney", 1),
        ("DeleteInboxItem", 2),
        ("ReturnInboxItem", 3),
        ("TakeInboxTextItem", 4),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: u32| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                match field {
                    0 => model.mail_take_items.push(index),
                    1 => model.mail_take_money.push(index),
                    2 => model.mail_deletes.push(index),
                    3 => model.mail_returns.push(index),
                    _ => model.mail_take_texts.push(index),
                }
                Ok(())
            })?,
        )?;
    }

    g.set(
        "CloseMail",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .mail_close = true;
            Ok(())
        })?,
    )?;

    g.set(
        "SendMail",
        lua.create_function(|lua, (target, subject, body): (String, String, String)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `SendMail` (`0x4ae800`) aborts silently, no packet and no return value, on money
            // with COD or on COD with no attached item.
            if model.mail_send_money != 0 && model.mail_send_cod != 0 {
                return Ok(());
            }
            if model.mail_send_cod != 0 && model.mail_send_item.is_none() {
                return Ok(());
            }
            // The same silent abort with no stationery selected (`0x4ae8dd`).
            if model.mail_stationery == 0 {
                return Ok(());
            }
            model.mail_send = Some((target, subject, body));
            Ok(())
        })?,
    )?;

    // SetSendMailMoney(copper) (`0x4ae0f0`): a non-number raises; more than the purse shows
    // ERR_NOT_ENOUGH_MONEY and answers nil; else it stores, fires SEND_MAIL_MONEY_CHANGED and
    // answers 1, which `StaticPopup.lua:257` branches on.
    g.set(
        "SetSendMailMoney",
        lua.create_function(|lua, copper: Value| {
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetSendMailMoney(amount)",
            )? as u32;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if u64::from(n) > model.money {
                model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
                return Ok(Value::Nil);
            }
            model.mail_send_money = n;
            model
                .pending_events
                .push(("SEND_MAIL_MONEY_CHANGED".to_string(), Vec::new()));
            Ok(Value::Integer(1))
        })?,
    )?;

    // GetSendMailMoney() and GetSendMailCOD() (`0x4ae150`, `0x4ae1c0`): the stored amounts.
    g.set(
        "GetSendMailMoney",
        lua.create_function(|lua, ()| {
            Ok(i64::from(
                lua.app_data_ref::<Model>()
                    .expect("model app_data")
                    .mail_send_money,
            ))
        })?,
    )?;
    g.set(
        "GetSendMailCOD",
        lua.create_function(|lua, ()| {
            Ok(i64::from(
                lua.app_data_ref::<Model>()
                    .expect("model app_data")
                    .mail_send_cod,
            ))
        })?,
    )?;

    // SetSendMailCOD(copper) (`0x4ae180`): a non-number raises; with no attached item nothing
    // happens; there is no purse or sign check and no return value.
    g.set(
        "SetSendMailCOD",
        lua.create_function(|lua, copper: Value| {
            let n = crate::script::binding_abi::number_arg(
                lua,
                copper,
                "Usage: SetSendMailCOD(amount)",
            )? as u32;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.mail_send_item.is_none() {
                return Ok(());
            }
            model.mail_send_cod = n;
            model
                .pending_events
                .push(("SEND_MAIL_COD_CHANGED".to_string(), Vec::new()));
            Ok(())
        })?,
    )?;

    // GetSendMailItem(): name, texture, stackCount, quality (`MailFrame.lua:511`). The reference
    // answers `nil, nil, 0, 0` both with nothing attached and before the template loads
    // (`0x4ae590`). It answers quality -1 for an item whose InventoryType is 0 (`0x4ae6bf`); this
    // answers the cached quality, or 0.
    g.set(
        "GetSendMailItem",
        lua.create_function(|lua, ()| {
            // A whole-stack pickup records no count, so the size comes from the source slot.
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.mail_send_item.clone().map(|it| {
                    let count = cursor::held_count(&model, &it);
                    (it, count)
                })
            };
            let Some((it, count)) = item else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            let name = cursor::item_link_name(it.link.as_deref());
            let name = if name.is_empty() {
                Value::Nil
            } else {
                Value::String(lua.create_string(&name)?)
            };
            let texture = match &it.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            // A number on every path, as in the reference.
            let quality = Value::Integer(i64::from(it.quality.unwrap_or(0)));
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(count)),
                quality,
            ]))
        })?,
    )?;

    // GetSendMailPrice(): 30 copper, vmangos's fee (`MailHandler.cpp:247`). The reference's is 30
    // (`0x4ae756`) plus an uncarried stationery's price, the package's and the enclosed money
    // (`0x4ae7a0`-`0x4ae7d2`); those are not added here.
    g.set(
        "GetSendMailPrice",
        lua.create_function(|_, ()| Ok(Value::Integer(30)))?,
    )?;

    g.set(
        "ClickSendMailItemButton",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_send_mail_item(&mut model);
            Ok(())
        })?,
    )?;

    // ── The stationery family ──
    // The reference rebuilds the list (`0x4ad970`) in `GetNumStationeries` (`0x4ae1f0`), on world
    // entry and on the last item-query answer; the app recomputes it every frame.

    g.set(
        "GetNumStationeries",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.mail_stationeries.len() as i64)
        })?,
    )?;

    // `GetStationeryInfo(index)` (`0x4ae230`): a non-number raises its Usage; three values
    // always, three nils out of range.
    g.set(
        "GetStationeryInfo",
        lua.create_function(|lua, index: Value| {
            let index = crate::script::binding_abi::number_arg(
                lua,
                index,
                "Usage: GetStationeryInfo(index)",
            )?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let row = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.mail_stationeries.get(i));
            let Some(row) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&row.name)?),
                Value::String(lua.create_string(&row.icon)?),
                row.cost
                    .map_or(Value::Nil, |c| Value::Integer(i64::from(c))),
            ]))
        })?,
    )?;

    // `SelectStationery(index)` (`0x4ae380`): out of range is not an error, it stores 0.
    g.set(
        "SelectStationery",
        lua.create_function(|lua, index: Value| {
            let index = crate::script::binding_abi::number_arg(
                lua,
                index,
                "Usage: SelectStationery(index)",
            )?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.mail_stationery = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.mail_stationeries.get(i))
                .map_or(0, |row| row.id);
            Ok(())
        })?,
    )?;

    // `GetSelectedStationeryTexture()` (`0x4ae3f0`): nil for no selection or an unknown id.
    g.set(
        "GetSelectedStationeryTexture",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let id = model.mail_stationery;
            let tex = (id != 0)
                .then(|| model.mail_stationeries.iter().find(|r| r.id == id))
                .flatten();
            Ok(match tex {
                Some(row) => Value::String(lua.create_string(&row.texture)?),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}

/// `ClickSendMailItemButton`: the send slot as a cursor drop target, firing `CURSOR_UPDATE` and
/// `ITEM_LOCK_CHANGED` like a bag slot.
fn click_send_mail_item(model: &mut Model) {
    match model.cursor.take() {
        // A held bag item attaches; it stays in its bag slot until the send.
        Some(CursorPayload::Item(item)) => {
            let (bag, slot) = (item.bag, item.slot);
            // An earlier attachment is overwritten, as in the reference; its slot has no lock.
            model.mail_send_item = Some(item);
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, bag, slot);
            // `MAIL_SEND_INFO_UPDATE` (`0x4ae0de`): the stock tab re-reads the item and postage.
            model
                .pending_events
                .push(("MAIL_SEND_INFO_UPDATE".to_string(), Vec::new()));
        }
        None => {
            if let Some(item) = model.mail_send_item.take() {
                let (bag, slot) = (item.bag, item.slot);
                model.cursor = Some(CursorPayload::Item(item));
                cursor::queue_cursor_update(model);
                cursor::queue_lock_changed(model, bag, slot);
                model
                    .pending_events
                    .push(("MAIL_SEND_INFO_UPDATE".to_string(), Vec::new()));
            }
        }
        Some(other) => {
            model.cursor = Some(other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A send needs a stationery: select the default, as the stock `SendMailFrame_Reset` does.
    fn select_default_stationery(s: &mut UiScript) {
        s.set_mail_stationeries(vec![super::StationeryView {
            id: 41,
            name: "Default Stationery".into(),
            icon: "Interface\\Icons\\INV_Letter_15".into(),
            cost: Some(0),
            texture: "STATIONERYTEST".into(),
        }]);
        s.run("SelectStationery(1)").unwrap();
    }

    fn row(item_id: u32) -> MailInboxRow {
        MailInboxRow {
            package_icon: (item_id != 0).then(|| "Interface\\Icons\\INV_Misc_Bag_08".to_string()),
            stationery_icon: Some("Interface\\Icons\\INV_Letter_15".to_string()),
            sender: Some("Thrall".into()),
            subject: "Warchief's orders".into(),
            money: 5000,
            cod: 0,
            days_left: 29.5,
            item_count: if item_id != 0 { 3 } else { 0 },
            was_read: false,
            was_returned: false,
            text_created: false,
            can_reply: true,
            is_gm: false,
            body: Some("Lok'tar.".into()),
            stationery_texture: "STATIONERYTEST".into(),
            is_invoice: false,
            invoice: None,
            has_body: true,
            item_id,
            item_name: (item_id != 0).then(|| "Linen Cloth".to_string()),
            item_texture: (item_id != 0)
                .then(|| "Interface\\Icons\\INV_Fabric_Linen_01".to_string()),
            item_quality: (item_id != 0).then_some(1),
            can_delete: false, // a player's unreturned mail is returned, not deleted
            item_random_property_id: 0,
        }
    }

    fn state() -> MailState {
        MailState {
            inbox: vec![row(2589), row(0)],
        }
    }

    #[test]
    fn the_invoice_answers_seven_values_or_a_three_nil_four_zero_miss() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.inbox[0].is_invoice = true;
        st.inbox[0].invoice = Some(MailInvoice {
            seller: true,
            item_name: "Linen Cloth".into(),
            player_name: "Twowarrior".into(),
            bid: 10_000,
            buyout: 10_000,
            deposit: 25,
            consignment: 500,
        });
        s.set_mail(Some(st));

        assert_eq!(s.arity("GetInboxInvoiceInfo(1)").unwrap(), 7);
        let vals: Vec<String> = (1..=7)
            .map(|i| {
                let discards = "_, ".repeat(i - 1);
                s.eval::<String>(&format!(
                    "local {discards}v = GetInboxInvoiceInfo(1) return tostring(v)"
                ))
                .unwrap()
            })
            .collect();
        assert_eq!(
            vals,
            [
                "seller",
                "Linen Cloth",
                "Twowarrior",
                "10000",
                "10000",
                "25",
                "500"
            ],
            "a seller invoice, in the reference's own order"
        );

        // Row 2 has no invoice, and index 99 is off the end.
        for idx in [2, 99] {
            assert_eq!(
                s.arity(&format!("GetInboxInvoiceInfo({idx})")).unwrap(),
                7,
                "the miss is still seven values"
            );
            let tail: Vec<String> = (1..=7)
                .map(|i| {
                    let discards = "_, ".repeat(i - 1);
                    s.eval::<String>(&format!(
                        "local {discards}v = GetInboxInvoiceInfo({idx}) return tostring(v)"
                    ))
                    .unwrap()
                })
                .collect();
            assert_eq!(tail, ["nil", "nil", "nil", "0", "0", "0", "0"]);
        }
    }

    #[test]
    fn an_invoice_has_no_letter_body() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.inbox[0].is_invoice = true;
        st.inbox[0].body = Some("6C:10000:10000:25:500".into());
        s.set_mail(Some(st));

        assert_eq!(
            s.arity("GetInboxText(1)").unwrap(),
            4,
            "four values, invoice or not"
        );
        assert_eq!(
            s.eval::<String>("return tostring((GetInboxText(1)))")
                .unwrap(),
            "nil",
            "the bookkeeping is never handed back as a letter body"
        );
        assert_eq!(
            s.eval::<String>("return tostring((GetInboxText(2)))")
                .unwrap(),
            "Lok'tar."
        );
    }

    #[test]
    fn a_buyer_invoice_is_the_literal_token_buyer() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.inbox[0].is_invoice = true;
        st.inbox[0].invoice = Some(MailInvoice {
            seller: false,
            item_name: "Small Blue Pouch".into(),
            player_name: "Onewarrior".into(),
            bid: 9_000,
            buyout: 10_000,
            deposit: 0,
            consignment: 0,
        });
        s.set_mail(Some(st));
        assert_eq!(
            s.eval::<String>("return (GetInboxInvoiceInfo(1))").unwrap(),
            "buyer"
        );
    }

    #[test]
    fn inbox_header_reads_the_reference_tuple() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetInboxNumItems()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetInboxHeaderInfo(1) == nil")
            .unwrap());

        s.set_mail(Some(state()));
        assert_eq!(s.eval::<i64>("return GetInboxNumItems()").unwrap(), 2);

        let (pkg, sta, sender, subject, money, cod, days): (
            String,
            String,
            String,
            String,
            i64,
            i64,
            f64,
        ) = s
            .eval("local a,b,c,d,e,f,g = GetInboxHeaderInfo(1)\nreturn a,b,c,d,e,f,g")
            .unwrap();
        assert_eq!(pkg, "Interface\\Icons\\INV_Misc_Bag_08");
        assert_eq!(sta, "Interface\\Icons\\INV_Letter_15");
        assert_eq!(
            (sender.as_str(), subject.as_str()),
            ("Thrall", "Warchief's orders")
        );
        assert_eq!((money, cod), (5000, 0));
        assert!((days - 29.5).abs() < 1e-3);
        assert!(s
            .eval::<bool>(
                "local a,b,c,d,e,f,g, has, read, ret, tc, reply, gm = GetInboxHeaderInfo(1)\n\
                 return has == 3 and read == nil and ret == nil and tc == nil and reply == 1 and gm == nil",
            )
            .unwrap());

        assert!(s
            .eval::<bool>(
                "local pkg, sta, s, subj, m, c, d, has = GetInboxHeaderInfo(2)\n\
                 return pkg == nil and has == nil",
            )
            .unwrap());
    }

    #[test]
    fn get_inbox_text_returns_body_and_queues_open() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        let (body, tex, takeable, invoice): (String, String, Value, Value) =
            s.eval("return GetInboxText(1)").unwrap();
        assert_eq!(body, "Lok'tar.");
        assert_eq!(tex, "STATIONERYTEST");
        assert_eq!(takeable, Value::Integer(1)); // has a body to fetch
        assert!(matches!(invoice, Value::Nil));
        // The open is queued once per drain.
        s.run("GetInboxText(1)").unwrap();
        assert_eq!(s.take_mail_opens(), vec![1]);
        assert!(s.take_mail_opens().is_empty(), "drained");
    }

    #[test]
    fn get_inbox_item_and_can_delete() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        let (name, _tex, count, quality, canuse): (String, String, i64, i64, i64) =
            s.eval("return GetInboxItem(1)").unwrap();
        assert_eq!(
            (name.as_str(), count, quality, canuse),
            ("Linen Cloth", 3, 1, 1)
        );
        assert!(s.eval::<bool>("return GetInboxItem(2) == nil").unwrap());
        // A player's unreturned mail is returnable, not deletable.
        assert!(s
            .eval::<bool>("return InboxItemCanDelete(1) == nil")
            .unwrap());
    }

    #[test]
    fn intents_queue_and_drain() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        s.run("CheckInbox()").unwrap();
        assert!(s.take_mail_check_inbox());
        assert!(!s.take_mail_check_inbox(), "drained");

        s.run("TakeInboxItem(1) TakeInboxMoney(1) DeleteInboxItem(2) ReturnInboxItem(1) TakeInboxTextItem(1)")
            .unwrap();
        assert_eq!(s.take_mail_take_items(), vec![1]);
        assert_eq!(s.take_mail_take_money(), vec![1]);
        assert_eq!(s.take_mail_deletes(), vec![2]);
        assert_eq!(s.take_mail_returns(), vec![1]);
        assert_eq!(s.take_mail_take_texts(), vec![1]);
        assert!(s.take_mail_take_texts().is_empty(), "drained");

        s.run("CloseMail()").unwrap();
        assert!(s.take_mail_close());
    }

    #[test]
    fn send_folds_money_cod_and_returns_true_from_setmoney() {
        let mut s = UiScript::new().unwrap();
        s.set_money(5_000);
        assert!(s.take_mail_send().is_none());
        assert!(s
            .eval::<bool>("return SetSendMailMoney(9999) == nil")
            .unwrap());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);
        assert!(s
            .eval::<bool>("return SetSendMailMoney(1234) == 1")
            .unwrap());
        // COD with no attached item: no store, no event.
        s.run("SetSendMailCOD(50)").unwrap();
        assert_eq!(s.eval::<i64>("return GetSendMailCOD()").unwrap(), 0);
        select_default_stationery(&mut s);
        s.run("SendMail('Jaina', 'Hi', 'body text')").unwrap();
        let req = s.take_mail_send().expect("a send was queued");
        assert_eq!(req.target, "Jaina");
        assert_eq!(req.subject, "Hi");
        assert_eq!(req.body, "body text");
        assert_eq!((req.money, req.cod), (1234, 0));
        assert_eq!(req.item, None);
        assert!(s.take_mail_send().is_none(), "drained");
        assert_eq!(s.eval::<i64>("return GetSendMailPrice()").unwrap(), 30);
        assert!(
            s.run("SetSendMailMoney(nil)").is_err(),
            "a non-number raises"
        );
        assert!(s.run("SetSendMailCOD({})").is_err());
    }

    #[test]
    fn attach_from_cursor_and_get_send_item() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("local n = GetSendMailItem()\nreturn n == nil")
            .unwrap());

        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 5,
            item_id: 2589,
            texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
            link: Some("|cff...|Hitem:2589|h[Linen Cloth]|h|r".into()),
            count: Some(7),
            quality: Some(1),
            equip_slots: Vec::new(),
        }));
        s.run("ClickSendMailItemButton()").unwrap();
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
        let (name, _tex, count, quality): (String, String, i64, i64) =
            s.eval("return GetSendMailItem()").unwrap();
        assert_eq!((name.as_str(), count, quality), ("Linen Cloth", 7, 1));

        select_default_stationery(&mut s);
        s.run("SendMail('Alt', 'stuff', '')").unwrap();
        assert_eq!(s.take_mail_send().unwrap().item, Some((0, 5)));

        // Clicking with an empty cursor detaches.
        s.run("ClickSendMailItemButton()").unwrap();
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s
            .eval::<bool>("local n = GetSendMailItem()\nreturn n == nil")
            .unwrap());
    }

    #[test]
    fn clear_send_item_resets_the_form() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 1,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));
        s.run("ClickSendMailItemButton()").unwrap();
        s.run("SetSendMailMoney(99) SetSendMailCOD(5)").unwrap();
        s.reset_compose_tab();
        select_default_stationery(&mut s);
        s.run("SendMail('x','y','z')").unwrap();
        let req = s.take_mail_send().unwrap();
        assert_eq!((req.money, req.cod, req.item), (0, 0, None));
    }

    #[test]
    fn clearing_the_mail_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        s.set_mail(None);
        assert_eq!(s.eval::<i64>("return GetInboxNumItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return HasNewMail() == nil").unwrap());
    }

    #[test]
    fn has_new_mail_reads_the_pushed_flag() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return HasNewMail() == nil").unwrap());
        s.set_has_new_mail(true);
        assert!(s.eval::<bool>("return HasNewMail() == 1").unwrap());
        s.set_has_new_mail(false);
        assert!(s.eval::<bool>("return HasNewMail() == nil").unwrap());
    }
}

#[cfg(test)]
mod stationery_tests {
    use super::StationeryView;
    use crate::script::UiScript;

    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_mail_stationeries(vec![
            StationeryView {
                id: 41,
                name: "Default Stationery".into(),
                icon: "Interface\\Icons\\INV_Letter_15".into(),
                cost: Some(0),
                texture: "STATIONERYTEST".into(),
            },
            StationeryView {
                id: 64,
                name: "Valentine Stationery".into(),
                icon: "Interface\\Icons\\INV_ValentinesCard02".into(),
                cost: None,
                texture: "STATIONERY_VAL".into(),
            },
        ]);
        s
    }

    #[test]
    fn the_stationery_family_answers_like_the_client() {
        let s = seated();
        assert_eq!(s.eval::<i64>("return GetNumStationeries()").unwrap(), 2);
        assert_eq!(
            s.eval::<(String, String, i64)>("return GetStationeryInfo(1)")
                .unwrap(),
            (
                "Default Stationery".into(),
                "Interface\\Icons\\INV_Letter_15".into(),
                0
            )
        );
        assert!(
            s.eval::<bool>("local n, t, c = GetStationeryInfo(\"2\") return n == \"Valentine Stationery\" and c == nil")
                .unwrap(),
            "a carried paper costs nil; a numeric string is an index"
        );
        assert!(
            s.eval::<bool>(
                "local n, t, c = GetStationeryInfo(3) return n == nil and t == nil and c == nil"
            )
            .unwrap(),
            "past the end: three nils"
        );
        for bad in [
            "GetStationeryInfo()",
            "GetStationeryInfo(\"x\")",
            "SelectStationery(nil)",
        ] {
            let err = s.run(bad).expect_err(bad).to_string();
            assert!(err.contains("Usage: "), "{bad}: {err}");
        }
        assert!(s
            .eval::<bool>("return GetSelectedStationeryTexture() == nil")
            .unwrap());
        s.run("SelectStationery(2)").unwrap();
        assert_eq!(
            s.eval::<String>("return GetSelectedStationeryTexture()")
                .unwrap(),
            "STATIONERY_VAL"
        );
        s.run("SelectStationery(9)").unwrap();
        assert!(
            s.eval::<bool>("return GetSelectedStationeryTexture() == nil")
                .unwrap(),
            "out of range is a deselect, not an error"
        );
    }

    #[test]
    fn a_send_needs_a_stationery_and_carries_its_id() {
        let mut s = seated();
        s.run("SendMail(\"Bob\", \"hi\", \"body\")").unwrap();
        assert!(
            s.take_mail_send().is_none(),
            "no selection: silently nothing"
        );
        s.run("SelectStationery(1) SendMail(\"Bob\", \"hi\", \"body\")")
            .unwrap();
        assert_eq!(s.take_mail_send().map(|r| r.stationery), Some(41));
        s.clear_stationery();
        s.run("SendMail(\"Bob\", \"hi\", \"body\")").unwrap();
        assert!(s.take_mail_send().is_none());
    }
}
