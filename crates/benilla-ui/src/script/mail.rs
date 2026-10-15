//! The mail bindings. The app pushes the open mailbox's inbox, already resolved from the wire
//! (`UiScript::set_mail`), and the Lua verbs queue intents the app drains and sends. Indices are
//! 1-based, as `MailFrame.lua` uses them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor;
use super::Model;
use crate::script::binding_abi::Idx;

/// `CheckInbox`'s throttle window, 60 000 ms (`0x4aeb06`).
const CHECK_INBOX_WINDOW_SECS: f64 = 60.0;

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
    /// `checked & COD_PAYMENT(0x8)`: the money a COD sale brought back.
    pub cod_payment: bool,
    /// The wire's message type (`[rec+0x4]`): 0 a player's mail, else a system sender.
    pub message_type: u8,
    /// The `MailTemplate.dbc` id (`[rec+0x25c]`), 0 for none: a body `TakeInboxTextItem` can copy.
    pub mail_template_id: u32,
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

/// The inbox as the client holds it, pushed whole by the app. The rows outlive a close, as the
/// reference's do (`0x4acd71` clears only the mailbox guid); `open` is that guid's presence
/// (`[0xb6ef88]`), which every mail verb tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub inbox: Vec<MailInboxRow>,
    pub open: bool,
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

/// One `Package.dbc` row, as `GetPackageInfo` answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageView {
    /// The row id, which `SelectPackage` stores and `CMSG_SEND_MAIL` carries.
    pub id: u32,
    /// The localized name (field 3 on).
    pub name: String,
    /// The icon as a full path: `StringLookups.dbc` row 3's folder, `\` and field 1
    /// (`0x4ae4ab`-`0x4ae4eb`).
    pub icon: String,
    /// Field 2, in copper.
    pub cost: i32,
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
    /// The selected `Package.dbc` id, sent only with an attached item (`0x4ae8f3`).
    pub package: u32,
}

/// `0x4acdc0`'s clears: the attachment, whose lock it lifts (`0x495420`, firing an argless
/// `ITEM_LOCK_CHANGED` at `0x495455`), then money, COD, stationery and package. Returns the
/// lifted attachment's guid, so the caller unlocks it.
fn clear_compose(model: &mut Model) -> Option<u64> {
    // The reset lifts the send-pending flag too (`0x4ace02`).
    model.mail_send_pending = false;
    let lock = model.mail_send_item.take().map(|it| it.guid);
    model.mail_send_money = 0;
    model.mail_send_cod = 0;
    model.mail_stationery = 0;
    model.mail_package = 0;
    lock
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open mailbox's inbox snapshot.
    pub fn set_mail(&mut self, state: Option<MailState>) {
        self.model_mut().mail = state;
    }

    /// The close core zeroed the mailbox (`0x4acd8f`): the verbs find none from here, ahead of
    /// the snapshot the next feed pushes.
    pub fn mail_closed(&mut self) {
        if let Some(mail) = self.model_mut().mail.as_mut() {
            mail.open = false;
        }
    }

    /// Take the `CheckInbox` flag; the app answers it with `CMSG_GET_MAIL_LIST`.
    pub fn take_mail_check_inbox(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_check_inbox)
    }

    /// A list or send answer landed: the request lock clears (`0x4ad5d0`, `0x4ad164`).
    pub fn mail_request_resolved(&mut self) {
        self.model_mut().mail_busy = false;
    }

    /// A send's own result (`action` SEND) lifts the send-pending flag (`0x4ad159`).
    pub fn mail_send_resolved(&mut self) {
        self.model_mut().mail_send_pending = false;
    }

    /// The world-enter init (`0x4acb10`, from the cascade at `0x4909f6`, which a `/reload` and a
    /// worldport reach as well as a login): the list throttle (`0x4acb7d`) and the request lock
    /// (`0x4acb82`) start over.
    pub fn reset_mail_world_state(&mut self) {
        let mut model = self.model_mut();
        model.mail_list_stamp = None;
        model.mail_busy = false;
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
            package: model.mail_package,
        })
    }

    /// The reference's compose-tab reset (`0x4acdc0(1)`), run on mailbox open and a sent mail:
    /// clear the attachment (its lock lifted), money, COD, stationery and package, then fire
    /// `SEND_MAIL_MONEY_CHANGED`, `SEND_MAIL_COD_CHANGED` and `MAIL_SEND_SUCCESS` in that order.
    /// The events must follow the clear, as `SendMailFrame_Reset` re-reads `GetSendMailItem` and
    /// selects stationery row 1. `MAIL_SEND_SUCCESS` means the form is clean: opening a mailbox
    /// fires it.
    pub fn reset_compose_tab(&mut self) {
        let lock = clear_compose(&mut self.model_mut());
        // Fired now, not queued: the order against the caller's `MAIL_SHOW`/`MAIL_FAILED` matters.
        if let Some(guid) = lock {
            self.unlock_item(guid);
        }
        // One literal call each, as the event census (`ui_script::reference_ui`) reads them.
        self.fire_event("SEND_MAIL_MONEY_CHANGED", Vec::new()); // 0x4ace14
        self.fire_event("SEND_MAIL_COD_CHANGED", Vec::new()); // 0x4ace1e
        self.fire_event("MAIL_SEND_SUCCESS", Vec::new()); // 0x4ace28
    }

    /// The close core's silent reset (`0x4acdc0(0)`, from `0x4acd50`): the same clears, and only
    /// the attachment's `ITEM_LOCK_CHANGED`, which `0x495420` fires on either leg.
    pub fn reset_compose_tab_silently(&mut self) {
        let lock = clear_compose(&mut self.model_mut());
        if let Some(guid) = lock {
            self.unlock_item(guid);
        }
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

    /// Push `Package.dbc`'s rows, in file order.
    pub fn set_mail_packages(&mut self, packages: Vec<PackageView>) {
        self.model_mut().mail_packages = packages;
    }

    /// Push `HasNewMail()`'s answer, independent of any open mailbox. The app sets it before each
    /// `UPDATE_PENDING_MAIL`, on which the minimap icon re-reads it (`Minimap.xml:278-289`).
    pub fn set_has_new_mail(&mut self, has: bool) {
        self.model_mut().has_new_mail = has;
    }
}

/// The guards every mail request opens with: no request in flight (`[0xb6efc8]`, `0x4aeab6`,
/// `0x4ae830`, `0x4af800`, `0x4af90c`, `0x4afa8e`, `0x4afbce`, `0x4afcfe`) and an open mailbox
/// (`[0xb6ef88]`).
fn mail_ready(model: &Model) -> bool {
    !model.mail_busy && model.mail.as_ref().is_some_and(|m| m.open)
}

/// The five row verbs, each with its own refusal of the row before any send: silent, but for
/// TakeInboxItem's COD the purse cannot pay, which shows `ERR_NOT_ENOUGH_MONEY`.
#[derive(Clone, Copy)]
enum RowVerb {
    TakeItem,
    TakeMoney,
    Delete,
    Return,
    TakeText,
}

impl RowVerb {
    /// `Ok` when the reference sends for `row`, else the refusal's message key, if it shows one.
    fn sends(self, model: &Model, row: &MailInboxRow) -> Result<(), Option<&'static str>> {
        // An item (`[rec+0x120]`, `0x4af96d`), then the player's object (`0x4af992`) and the purse
        // to pay its COD (`[rec+0x144]` against the coinage, `0x4af9c5`); either miss shows
        // `ERR_NOT_ENOUGH_MONEY` (`0x4afa4c`, `push 0x25`).
        if let RowVerb::TakeItem = self {
            if row.item_id == 0 {
                return Err(None);
            }
            let player = model.units_by_lower.contains_key("player");
            if !player || model.money < u64::from(row.cod) {
                return Err(Some("ERR_NOT_ENOUGH_MONEY"));
            }
            return Ok(());
        }
        // `checked`: RETURNED 0x2, COPIED 0x4, COD_PAYMENT 0x8 (`[rec+0x148]`).
        let returned_or_cod = row.was_returned || row.cod_payment;
        let sends = match self {
            RowVerb::TakeItem => true,
            // Money (`[rec+0x140]`, `0x4af85a`).
            RowVerb::TakeMoney => row.money != 0,
            // A body or a template to copy (`[rec+0x114]`, `[rec+0x25c]`, `0x4afae7`), not yet
            // copied (`& 4`, `0x4afafb`).
            RowVerb::TakeText => (row.has_body || row.mail_template_id != 0) && !row.text_created,
            // A player's mail (type 0, `0x4afc27`) with an item or money (`0x4afc30`), neither
            // returned nor a COD payment (`& 0xa`, `0x4afc44`).
            RowVerb::Return => {
                row.message_type == 0 && (row.item_id != 0 || row.money != 0) && !returned_or_cod
            }
            // Returned or a COD payment, not a player's, or empty (`0x4afd57`-`0x4afd77`).
            RowVerb::Delete => {
                returned_or_cod || row.message_type != 0 || (row.item_id == 0 && row.money == 0)
            }
        };
        if sends {
            Ok(())
        } else {
            Err(None)
        }
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

    // GetInboxHeaderInfo(index): the 13 values `MailFrame.lua:105` reads. Out of range (or index
    // 0, which `OpenMail_Update` asks after a letter closes) the reference still returns 13
    // (`0x4aeff0`..`0x4af05e`): four nils, 0 for money, COD amount and days left, six nils.
    g.set(
        "GetInboxHeaderInfo",
        lua.create_function(|lua, Idx(index): Idx| {
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, index)
            };
            let Some(r) = row else {
                let mut miss = vec![Value::Nil; 4];
                miss.extend(vec![Value::Integer(0); 3]);
                miss.extend(vec![Value::Nil; 6]);
                return Ok(MultiValue::from_vec(miss));
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
                // hasItem is the constant 1.0 for a nonzero item entry (`0x4aef38`), not the stack.
                flag(r.item_id != 0),
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
        lua.create_function(|lua, Idx(index): Idx| {
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
            // A miss pushes four nils (`0x4af311`..`0x4af334`).
            let Some(r) = row else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 4]));
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
        lua.create_function(|lua, Idx(index): Idx| {
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
        lua.create_function(|lua, Idx(index): Idx| {
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

    // CheckInbox() (`0x4aeab0`): nothing while a request is in flight (`0x4aeac2`) or with no
    // mailbox (`0x4aead8`). Within 60 s of the last list query it re-announces the held list,
    // signalling `MAIL_INBOX_UPDATE` in place (`0x4aeaf8`); otherwise it queues the query, stamps
    // the window and takes the lock (`0x4aeb06`-`0x4aeb18`), and the answer announces the list.
    g.set(
        "CheckInbox",
        lua.create_function(|lua, ()| {
            super::tick::verb(lua, |model| {
                if !mail_ready(model) {
                    return;
                }
                if model.mail_list_stamp.is_some_and(|t| model.now < t) {
                    model.raise("MAIL_INBOX_UPDATE", Vec::new());
                    return;
                }
                model.mail_list_stamp = Some(model.now + CHECK_INBOX_WINDOW_SECS);
                model.mail_busy = true;
                model.mail_check_inbox = true;
            });
            Ok(())
        })?,
    )?;

    // The row verbs: the index must be a number, else the usage raises. A request in flight, no
    // mailbox, an index past the held rows or the verb's own refusal of the row returns at once
    // and sends nothing; a send takes the request lock (`0x4af8aa`, `0x4afa1f`, `0x4afb68`,
    // `0x4afc9e`, `0x4afdca`), which only a mail result or a list result lifts.
    for (name, verb, usage) in [
        (
            "TakeInboxItem",
            RowVerb::TakeItem,
            "Usage: TakeInboxItem(index)",
        ),
        (
            "TakeInboxMoney",
            RowVerb::TakeMoney,
            "Usage: TakeInboxMoney(index)",
        ),
        (
            "DeleteInboxItem",
            RowVerb::Delete,
            "Usage: DeleteInboxItem(index)",
        ),
        (
            "ReturnInboxItem",
            RowVerb::Return,
            "Usage: ReturnInboxItem(index)",
        ),
        (
            "TakeInboxTextItem",
            RowVerb::TakeText,
            "Usage: TakeInboxTextItem(index)",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let n = super::binding_abi::number_arg(lua, index, usage)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                if !mail_ready(&model) {
                    return Ok(());
                }
                let Some(index) = u32::try_from(n).ok().filter(|&i| i >= 1) else {
                    return Ok(());
                };
                let Some(row) = row_at(&model, index as usize) else {
                    return Ok(());
                };
                if let Err(refusal) = verb.sends(&model, &row) {
                    if let Some(key) = refusal {
                        model.push_ui_error(key);
                    }
                    return Ok(());
                }
                match verb {
                    RowVerb::TakeItem => model.mail_take_items.push(index),
                    RowVerb::TakeMoney => model.mail_take_money.push(index),
                    RowVerb::Delete => model.mail_deletes.push(index),
                    RowVerb::Return => model.mail_returns.push(index),
                    RowVerb::TakeText => model.mail_take_texts.push(index),
                }
                model.mail_busy = true;
                Ok(())
            })?,
        )?;
    }

    g.set(
        "CloseMail",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.mail_close = true;
            // The close core zeroes the mailbox inside the verb (`0x4aded0` → `0x4acd8f`), so a
            // mail verb later in the same chunk finds none.
            if let Some(mail) = model.mail.as_mut() {
                mail.open = false;
            }
            Ok(())
        })?,
    )?;

    g.set(
        "SendMail",
        lua.create_function(|lua, (target, subject, body): (Value, Value, Value)| {
            // The recipient must be a string (`0x4ae810`); the subject and body read as "" when
            // they are not (`0x4ae8a1`, `0x4ae8d0`).
            let target = super::binding_abi::string_arg(
                lua,
                target,
                "Usage: SendMail(target, subject, body)",
            )?;
            let text = |v: &Value| super::binding_abi::optional_string(lua, v).unwrap_or_default();
            let (subject, body) = (text(&subject), text(&body));
            super::tick::verb(lua, |model| {
                // `SendMail` (`0x4ae800`) aborts silently, no packet and no return value: a request
                // in flight or no mailbox (`0x4ae839`, `0x4ae852`), a send awaiting its result
                // (`0x4ae85e`), an empty recipient or subject (`0x4ae875`, `0x4ae8a9`), no
                // stationery (`0x4ae8dd`).
                if !mail_ready(model)
                    || model.mail_send_pending
                    || target.is_empty()
                    || subject.is_empty()
                    || model.mail_stationery == 0
                {
                    return;
                }
                // An attached item whose object is gone (`0x4ae92e` resolves its guid):
                // `ERR_ITEM_NOT_FOUND` (equip error 0x17, `0x4ae957`), the attachment dropped and
                // `MAIL_SEND_INFO_UPDATE` (`0x4ae98d`), no packet. Checked before the money
                // (`0x4ae917` ahead of `0x4ae99e`). The attachment is locked, so it cannot have moved.
                if let Some(item) = model.mail_send_item.as_ref() {
                    let held = model
                        .containers
                        .get(&item.bag)
                        .and_then(|c| c.slots.get(&item.slot))
                        .is_some_and(|s| s.guid == item.guid);
                    if !held {
                        model.push_ui_error("ERR_ITEM_NOT_FOUND");
                        model.mail_send_item = None;
                        model.raise("MAIL_SEND_INFO_UPDATE", Vec::new());
                        return;
                    }
                }
                // Money with COD, or COD with no item (`0x4ae99e`-`0x4ae9b8`).
                if model.mail_send_money != 0 && model.mail_send_cod != 0 {
                    return;
                }
                if model.mail_send_cod != 0 && model.mail_send_item.is_none() {
                    return;
                }
                model.mail_send = Some((target, subject, body));
                // The send raises the pending flag and takes the request lock (`0x4aea76`,
                // `0x4aea7b`); a send the app finds without its item gives the lock back, as that
                // check precedes the reference's write.
                model.mail_send_pending = true;
                model.mail_busy = true;
            });
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
            Ok(super::tick::verb(lua, |model| {
                if u64::from(n) > model.money {
                    model.push_ui_error("ERR_NOT_ENOUGH_MONEY");
                    return Value::Nil;
                }
                // The write (`0x4adc39`), then the signal (`0x4adc44`).
                model.mail_send_money = n;
                model.raise("SEND_MAIL_MONEY_CHANGED", Vec::new());
                Value::Integer(1)
            }))
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
            super::tick::verb(lua, |model| {
                if model.mail_send_item.is_some() {
                    // The write (`0x4adc7d`), then the signal (`0x4adc88`).
                    model.mail_send_cod = n;
                    model.raise("SEND_MAIL_COD_CHANGED", Vec::new());
                }
            });
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
            click_send_mail_item(lua);
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

    // ClearSendMail() (`0x4adee0`): the compose reset with its events (`0x4acdc0(1)`), fired in
    // place in its order: the attachment's unlock (`0x495455`), then `0x4ace19`, `0x4ace23` and
    // `0x4ace2d`; zero returns.
    g.set(
        "ClearSendMail",
        lua.create_function(|lua, ()| {
            super::tick::verb(lua, |model| {
                if let Some(guid) = clear_compose(model) {
                    cursor::unlock_item(model, guid);
                }
                model.raise("SEND_MAIL_MONEY_CHANGED", Vec::new());
                model.raise("SEND_MAIL_COD_CHANGED", Vec::new());
                model.raise("MAIL_SEND_SUCCESS", Vec::new());
            });
            Ok(MultiValue::new())
        })?,
    )?;

    // GetNumPackages() (`0x4ae430`): `Package.dbc`'s raw row count, unfiltered.
    g.set(
        "GetNumPackages",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.mail_packages.len() as i64)
        })?,
    )?;

    // GetPackageInfo(index) (`0x4ae450`) → name, icon, cost for a 1-based row; out of range it
    // pushes nil, nil, 0 (`0x4ae525`), and a non-number raises.
    g.set(
        "GetPackageInfo",
        lua.create_function(|lua, index: Value| {
            let index =
                crate::script::binding_abi::number_arg(lua, index, "Usage: GetPackageInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let row = usize::try_from(index.wrapping_sub(1))
                .ok()
                .and_then(|i| model.mail_packages.get(i));
            Ok(match row {
                Some(row) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&row.name)?),
                    Value::String(lua.create_string(&row.icon)?),
                    Value::Integer(i64::from(row.cost)),
                ]),
                None => MultiValue::from_vec(vec![Value::Nil, Value::Nil, Value::Integer(0)]),
            })
        })?,
    )?;

    // SelectPackage(index) (`0x4ae550`): stores the 1-based row's id, or 0 out of range
    // (`0x4adc90`); a non-number raises. Zero returns.
    g.set(
        "SelectPackage",
        lua.create_function(|lua, index: Value| {
            let index =
                crate::script::binding_abi::number_arg(lua, index, "Usage: SelectPackage(index)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.mail_package = usize::try_from(index.wrapping_sub(1))
                .ok()
                .and_then(|i| model.mail_packages.get(i))
                .map_or(0, |row| row.id);
            Ok(MultiValue::new())
        })?,
    )?;

    Ok(())
}

/// `ClickSendMailItemButton` (`0x4adef0`): the send slot as a cursor drop target, attaching the
/// held item and putting an earlier attachment back on the cursor ([`cursor::click_attach_slot`]).
/// A held item stays in its bag slot, locked, until the send. A changed attachment signals
/// `MAIL_SEND_INFO_UPDATE` (`0x4ae0de`), whose stock handler re-reads the item and postage.
fn click_send_mail_item(lua: &Lua) {
    if cursor::click_attach_slot(lua, |model| &mut model.mail_send_item) {
        super::tick::verb(lua, |model| {
            model.raise("MAIL_SEND_INFO_UPDATE", Vec::new());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// `CheckInbox` (`0x4aeab0`): no mailbox or a request in flight does nothing; the first
    /// check queues the query and takes the lock; once answered, a check inside the 60 s window
    /// re-announces the held list with `MAIL_INBOX_UPDATE` in place (`0x4aeaf8`), and past it
    /// queries again.
    #[test]
    fn check_inbox_throttles_to_an_in_place_reannounce() {
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        heard::listen(&s, &["MAIL_INBOX_UPDATE"], None);
        s.run("CheckInbox()").unwrap();
        assert!(!s.take_mail_check_inbox(), "no mailbox, no query");

        s.set_mail(Some(MailState::default()));
        s.run("CheckInbox()").unwrap();
        assert!(!s.take_mail_check_inbox(), "rows held, no mailbox open");
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        s.run("CheckInbox()").unwrap();
        assert!(s.take_mail_check_inbox());
        s.run("CheckInbox()").unwrap();
        assert!(
            !s.take_mail_check_inbox(),
            "the lock holds until the answer"
        );
        assert!(heard::take(&s).is_empty());

        s.mail_request_resolved();
        s.tick(30.0);
        s.run("CheckInbox()").unwrap();
        assert!(!s.take_mail_check_inbox());
        assert_eq!(heard::take(&s), ["MAIL_INBOX_UPDATE"]);

        s.tick(31.0);
        s.run("CheckInbox()").unwrap();
        assert!(
            s.take_mail_check_inbox(),
            "the window ran out: a fresh query"
        );
        assert!(heard::take(&s).is_empty());
    }

    /// The compose verbs fire in place, each after its write (`0x4adc39` then `0x4adc44`), and
    /// `ClearSendMail` after all its clears (`0x4acddf`-`0x4ace0c`, then `0x4ace19`, `0x4ace23`,
    /// `0x4ace2d`): every handler reads the stored amount before the next Lua line runs.
    #[test]
    fn the_compose_verbs_fire_their_events_in_place() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000);
        crate::script::tick::heard::listen(
            &s,
            &[
                "SEND_MAIL_MONEY_CHANGED",
                "SEND_MAIL_COD_CHANGED",
                "MAIL_SEND_SUCCESS",
            ],
            Some("GetSendMailMoney()"),
        );
        let seen: Vec<String> = s
            .eval("SetSendMailMoney(250) local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(seen, ["SEND_MAIL_MONEY_CHANGED 250"]);
        let seen: Vec<String> = s
            .eval("ClearSendMail() local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(
            seen,
            [
                "SEND_MAIL_MONEY_CHANGED 0",
                "SEND_MAIL_COD_CHANGED 0",
                "MAIL_SEND_SUCCESS 0"
            ]
        );
        assert!(s.model_ref().pending_events.is_empty());
    }

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
            cod_payment: false,
            message_type: 0,
            mail_template_id: 0,
        }
    }

    fn state() -> MailState {
        MailState {
            inbox: vec![row(2589), row(0)],
            open: true,
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
                 return has == 1 and read == nil and ret == nil and tc == nil and reply == 1 and gm == nil",
            )
            .unwrap());

        assert!(s
            .eval::<bool>(
                "local pkg, sta, s, subj, m, c, d, has = GetInboxHeaderInfo(2)\n\
                 return pkg == nil and has == nil",
            )
            .unwrap());
    }

    /// A row that does not exist still answers the full tuple: `GetInboxHeaderInfo` 13 values,
    /// four nils, `0` money, COD and days left, six nils (`0x4aeff0`..`0x4af05e`), so a closed
    /// letter's `GetInboxHeaderInfo(0)` can be compared with `CODAmount > 0` (`MailFrame.lua:399`);
    /// `GetInboxText` four nils (`0x4af311`..`0x4af334`).
    #[test]
    fn a_missing_inbox_row_answers_the_references_miss_tuples() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        for index in [0, 3, 99] {
            assert_eq!(
                s.arity(&format!("GetInboxHeaderInfo({index})")).unwrap(),
                13,
                "header {index}"
            );
            assert_eq!(
                s.arity(&format!("GetInboxText({index})")).unwrap(),
                4,
                "text {index}"
            );
            assert_eq!(
                s.eval::<String>(&format!(
                    "local t = {{GetInboxHeaderInfo({index})}} \
                     local k = '' \
                     for i = 1, 13 do k = k .. type(t[i]) .. ',' end \
                     local _, _, _, _, m, c, d = GetInboxHeaderInfo({index}) \
                     return k .. m .. ',' .. c .. ',' .. d .. ',' .. tostring(c > 0)"
                ))
                .unwrap(),
                "nil,nil,nil,nil,number,number,number,nil,nil,nil,nil,nil,nil,0,0,0,false",
                "header {index}"
            );
            assert!(
                s.eval::<bool>(&format!(
                    "local a, b, c, d = GetInboxText({index}) \
                     return a == nil and b == nil and c == nil and d == nil"
                ))
                .unwrap(),
                "text {index}"
            );
        }
        assert!(
            s.take_mail_opens().is_empty(),
            "a miss opens nothing for the app to fetch"
        );
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

    /// The mail verbs share one request lock (`[0xb6efc8]`): each returns at once while it is
    /// held, and each send takes it (`0x4aeb18`, `0x4af8aa`, `0x4afa1f`, `0x4afb68`, `0x4afc9e`,
    /// `0x4afdca`, `0x4aea7b`) until a result lifts it.
    #[test]
    fn the_mail_verbs_share_one_request_lock() {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(crate::script::UnitState::default()));
        let mut st = state();
        // An empty player's mail: deletable (`0x4afd65`-`0x4afd77`).
        st.inbox[1].money = 0;
        s.set_mail(Some(st));
        let all = "TakeInboxItem(1) TakeInboxMoney(1) DeleteInboxItem(2) ReturnInboxItem(1) \
                   TakeInboxTextItem(1) CheckInbox()";
        s.run(all).unwrap();
        assert_eq!(s.take_mail_take_items(), vec![1], "the first send");
        assert!(s.take_mail_take_money().is_empty(), "the rest wait on it");
        assert!(!s.take_mail_check_inbox());
        for (verb, take) in [
            (
                "TakeInboxMoney(1)",
                UiScript::take_mail_take_money as fn(&mut UiScript) -> Vec<u32>,
            ),
            ("DeleteInboxItem(2)", UiScript::take_mail_deletes),
            ("ReturnInboxItem(1)", UiScript::take_mail_returns),
            ("TakeInboxTextItem(1)", UiScript::take_mail_take_texts),
        ] {
            s.mail_request_resolved();
            s.run(&format!("{verb} {verb}")).unwrap();
            assert_eq!(take(&mut s).len(), 1, "{verb}: one send, then the lock");
        }
        s.mail_request_resolved();
        s.run("CheckInbox() CheckInbox()").unwrap();
        assert!(s.take_mail_check_inbox());
        assert!(!s.take_mail_check_inbox(), "drained");

        s.mail_request_resolved();
        select_default_stationery(&mut s);
        s.tick(61.0);
        s.run("SendMail('Jaina', 'Hi', 'body') CheckInbox()")
            .unwrap();
        assert!(s.take_mail_send().is_some());
        assert!(!s.take_mail_check_inbox(), "the send holds the lock");

        let err = s.run("TakeInboxItem('x')").unwrap_err().to_string();
        assert!(err.contains("Usage: TakeInboxItem(index)"), "{err}");

        s.run("CloseMail()").unwrap();
        assert!(s.take_mail_close());
    }

    /// `SendMail` (`0x4ae800`): an empty recipient or subject returns (`0x4ae875`, `0x4ae8a9`)
    /// while a missing body reads as ""; a sent mail raises the send-pending flag (`0x4aea76`),
    /// which a list result does not lift but the send's own result does (`0x4ad159`), as does
    /// the compose reset (`0x4ace02`).
    #[test]
    fn send_mail_waits_on_its_own_result() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        select_default_stationery(&mut s);
        s.run("SendMail('', 'Hi', 'b') SendMail('Bob', '', 'b') SendMail('Bob', nil, 'b')")
            .unwrap();
        assert!(s.take_mail_send().is_none(), "no recipient or no subject");
        s.run("SendMail('Bob', 'Hi')").unwrap();
        assert_eq!(s.take_mail_send().map(|r| r.body), Some(String::new()));

        s.mail_request_resolved();
        s.run("SendMail('Bob', 'Again', 'b')").unwrap();
        assert!(s.take_mail_send().is_none(), "the first send still pends");
        s.mail_send_resolved();
        s.run("SendMail('Bob', 'Again', 'b')").unwrap();
        assert!(s.take_mail_send().is_some());

        s.mail_request_resolved();
        s.reset_compose_tab();
        select_default_stationery(&mut s);
        s.run("SendMail('Bob', 'Third', 'b')").unwrap();
        assert!(s.take_mail_send().is_some(), "the compose reset lifted it");
    }

    /// The attached item is checked before money and COD (`0x4ae917` ahead of `0x4ae99e`), by its
    /// object (`0x4ae92e`): with another item of the same entry in its slot, the send shows
    /// `ERR_ITEM_NOT_FOUND`, drops the attachment and signals `MAIL_SEND_INFO_UPDATE`
    /// (`0x4ae98d`) even with money and COD both set.
    #[test]
    fn send_mail_checks_its_item_before_the_money() {
        use crate::script::cursor::two_item_bag;
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_money(10_000);
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        s.set_container(0, Some(two_item_bag()));
        s.run("PickupContainerItem(0, 1) ClickSendMailItemButton() SetSendMailMoney(5) SetSendMailCOD(5)")
            .unwrap();
        select_default_stationery(&mut s);
        let mut other = two_item_bag();
        other.slots.get_mut(&1).unwrap().guid += 0x1000;
        s.set_container(0, Some(other));
        let _ = s.take_ui_errors();
        heard::listen(&s, &["MAIL_SEND_INFO_UPDATE"], None);
        s.run("SendMail('Bob', 'Hi', 'b')").unwrap();
        assert!(s.take_mail_send().is_none());
        assert_eq!(s.take_ui_errors(), vec!["ERR_ITEM_NOT_FOUND"]);
        assert_eq!(heard::take(&s), ["MAIL_SEND_INFO_UPDATE"]);
        assert!(s.eval::<bool>("return GetSendMailItem() == nil").unwrap());
    }

    /// `CloseMail` zeroes the mailbox inside the verb (`0x4aded0` → `0x4acd8f`): a mail verb
    /// later in the same chunk finds none.
    #[test]
    fn close_mail_closes_before_the_next_verb() {
        let mut s = UiScript::new().unwrap();
        s.set_mail(Some(state()));
        s.run("CloseMail() CheckInbox()").unwrap();
        assert!(s.take_mail_close());
        assert!(!s.take_mail_check_inbox());
    }

    /// Each row verb refuses a row the reference does not send for, and then takes no lock:
    /// money to take (`0x4af85a`), an item and the purse for its COD (`0x4af96d`, `0x4af9c5`), a
    /// body not yet copied (`0x4afae7`, `0x4afafb`), a player's unreturned mail with something
    /// in it to return (`0x4afc27`-`0x4afc44`), and for delete the reverse (`0x4afd57`-`0x4afd77`).
    #[test]
    fn each_row_verb_refuses_a_row_it_cannot_send_for() {
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(crate::script::UnitState::default()));
        let mut empty = row(0);
        empty.money = 0;
        empty.has_body = false;
        let mut cod = row(2589);
        cod.cod = 100;
        let mut copied = row(0);
        copied.text_created = true;
        let mut returned = row(2589);
        returned.was_returned = true;
        s.set_mail(Some(MailState {
            inbox: vec![empty, cod, copied, returned, row(2589)],
            open: true,
        }));
        s.run(
            "TakeInboxMoney(1) TakeInboxItem(1) TakeInboxTextItem(1) ReturnInboxItem(1) \
             TakeInboxItem(2) TakeInboxTextItem(3) ReturnInboxItem(4) DeleteInboxItem(5) \
             TakeInboxItem(9)",
        )
        .unwrap();
        assert!(s.take_mail_take_money().is_empty());
        assert!(s.take_mail_take_items().is_empty());
        assert!(s.take_mail_take_texts().is_empty());
        assert!(s.take_mail_returns().is_empty());
        assert!(s.take_mail_deletes().is_empty());
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_NOT_ENOUGH_MONEY"],
            "the unpayable COD shows its refusal (`0x4afa4c`); the rest are silent"
        );
        // None of them took the lock: a deletable empty mail still goes.
        s.run("DeleteInboxItem(1)").unwrap();
        assert_eq!(s.take_mail_deletes(), vec![1]);
        s.mail_request_resolved();
        s.set_money(100);
        s.run("TakeInboxItem(2)").unwrap();
        assert_eq!(
            s.take_mail_take_items(),
            vec![2],
            "the COD is affordable now"
        );
        // With no player object the take refuses the same way (`0x4af992`).
        s.mail_request_resolved();
        s.set_unit("player", None);
        s.run("TakeInboxItem(5)").unwrap();
        assert!(s.take_mail_take_items().is_empty());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);
    }

    #[test]
    fn send_folds_money_cod_and_returns_true_from_setmoney() {
        let mut s = UiScript::new().unwrap();
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
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
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        assert!(s
            .eval::<bool>("local n = GetSendMailItem()\nreturn n == nil")
            .unwrap());

        // The bag still holds it at the send (`0x4ae92e`).
        s.set_container(
            0,
            Some(crate::script::ContainerState {
                name: None,
                num_slots: 16,
                slots: [(
                    5,
                    crate::script::ContainerSlot {
                        item_id: 2589,
                        guid: 0x102,
                        count: 7,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            }),
        );
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            item_class: None,
            guid: 0x102,
            bar_placeable: true,
            consumable: false,
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
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            item_class: None,
            guid: 0x103,
            bar_placeable: true,
            consumable: false,
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

    /// `ClearSendMail()` (`0x4adee0`) is the compose reset with its events, the attachment's lock
    /// first, then the three in `0x4acdc0`'s order; stationery and package clear with the rest.
    #[test]
    fn clear_send_mail_resets_the_form_and_fires_its_events() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = UiScript::new().unwrap();
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        select_default_stationery(&mut s);
        s.set_mail_packages(vec![PackageView {
            id: 2,
            name: "Test Package".into(),
            icon: "Interface\\Icons\\INV_BOX_04".into(),
            cost: 10,
        }]);
        s.run("SelectPackage(1)").unwrap();
        s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
            item_class: None,
            guid: 0x104,
            bar_placeable: true,
            consumable: false,
            bag: 0,
            slot: 3,
            item_id: 1,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        }));
        s.run("ClickSendMailItemButton() SetSendMailMoney(99) SetSendMailCOD(5)")
            .unwrap();
        s.tick(0.0);
        s.run(
            r#"FIRED = {}
               local f = CreateFrame("Frame")
               for _, e in { "ITEM_LOCK_CHANGED", "SEND_MAIL_MONEY_CHANGED",
                             "SEND_MAIL_COD_CHANGED", "MAIL_SEND_SUCCESS" } do
                   f:RegisterEvent(e)
               end
               f:SetScript("OnEvent", function() table.insert(FIRED, event) end)"#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>("local n = function(...) return arg.n end return n(ClearSendMail())")
                .unwrap(),
            0
        );
        s.tick(0.0);
        assert_eq!(
            s.eval::<Vec<String>>("return FIRED").unwrap(),
            [
                "ITEM_LOCK_CHANGED",
                "SEND_MAIL_MONEY_CHANGED",
                "SEND_MAIL_COD_CHANGED",
                "MAIL_SEND_SUCCESS"
            ]
        );
        assert!(s
            .eval::<bool>("return GetSendMailItem() == nil and GetSendMailMoney() == 0")
            .unwrap());
        s.run("SendMail('x', 'y', 'z')").unwrap();
        assert!(s.take_mail_send().is_none(), "the stationery cleared too");
        select_default_stationery(&mut s);
        s.run("SendMail('x', 'y', 'z')").unwrap();
        assert_eq!(s.take_mail_send().map(|r| r.package), Some(0));
    }

    /// `Package.dbc`'s one shipped row, through the three package verbs (`0x4ae430`, `0x4ae450`,
    /// `0x4ae550`).
    #[test]
    fn packages_list_describe_and_select_by_row() {
        let mut s = UiScript::new().unwrap();
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        assert_eq!(s.eval::<i64>("return GetNumPackages()").unwrap(), 0);
        s.set_mail_packages(vec![PackageView {
            id: 2,
            name: "Test Package".into(),
            icon: "Interface\\Icons\\INV_BOX_04".into(),
            cost: 10,
        }]);
        assert_eq!(s.eval::<i64>("return GetNumPackages()").unwrap(), 1);
        let (name, icon, cost): (String, String, i64) = s.eval("return GetPackageInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), icon.as_str(), cost),
            ("Test Package", "Interface\\Icons\\INV_BOX_04", 10)
        );
        for miss in ["0", "2", "-1"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local a, b, c = GetPackageInfo({miss}) return a == nil and b == nil and c == 0"
                ))
                .unwrap(),
                "{miss}"
            );
        }
        for call in ["GetPackageInfo()", "SelectPackage('x')"] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains("Usage: "), "{call}: {err}");
        }

        select_default_stationery(&mut s);
        s.run("SelectPackage(1) SendMail('x', 'y', 'z')").unwrap();
        assert_eq!(s.take_mail_send().map(|r| r.package), Some(2));
        s.mail_request_resolved();
        s.mail_send_resolved();
        s.run("SelectPackage(5) SendMail('x', 'y', 'z')").unwrap();
        assert_eq!(
            s.take_mail_send().map(|r| r.package),
            Some(0),
            "out of range stores 0"
        );
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

    /// The attach clears with `SetCursorItem(0, …)` (`0x4ae0d2`): no unlock and no
    /// `ITEM_LOCK_CHANGED`, so the attached item stays locked in its bag and cannot be picked up
    /// again, until the compose reset lifts the lock with its event (`0x4acdd6`).
    #[test]
    fn an_attached_item_keeps_its_lock_until_it_leaves_the_slot() {
        use crate::script::cursor::{bag_lock_probe, two_item_bag};
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(two_item_bag()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        heard::listen(
            &s,
            &["ITEM_LOCK_CHANGED", "CURSOR_UPDATE"],
            Some(&bag_lock_probe(1)),
        );
        s.run("ClickSendMailItemButton()").unwrap();
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE 1"]);
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(
            s.cursor_payload().is_none(),
            "a locked item is not picked up"
        );
        assert!(heard::take(&s).is_empty());
        s.reset_compose_tab();
        assert_eq!(heard::take(&s), ["ITEM_LOCK_CHANGED nil"]);
    }

    /// An occupied attach slot puts the old attachment back on the cursor through
    /// `SetCursorItem(0, …)` (`0x4ae05a`-`0x4ae0d4`): its inner clear's `CURSOR_UPDATE` reads the
    /// emptied cursor, the set's the old item, and both items stay locked. An empty hand takes the
    /// attachment back the same way. With nothing held and nothing attached the click is
    /// `ClearCursor(1,1)` (`0x4ae08b`), which drops any other payload and signals nothing more.
    #[test]
    fn an_attach_onto_an_occupied_slot_puts_the_old_item_on_the_cursor() {
        use crate::script::cursor::{
            bag_lock_probe, install_kind_probe, two_item_bag, CursorMacro, CursorPayload,
        };
        use crate::script::tick::heard;
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(two_item_bag()));
        install_kind_probe(&s);
        s.run("PickupContainerItem(0, 1) ClickSendMailItemButton() PickupContainerItem(0, 2)")
            .unwrap();
        heard::listen(
            &s,
            &[
                "ACTIONBAR_HIDEGRID",
                "ACTIONBAR_SHOWGRID",
                "CURSOR_UPDATE",
                "ITEM_LOCK_CHANGED",
                "MAIL_SEND_INFO_UPDATE",
            ],
            Some("CursorKind() .. ' ' .. tostring(GetSendMailItem())"),
        );
        s.run("ClickSendMailItemButton()").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "ACTIONBAR_HIDEGRID item Minor Healing Potion",
                "CURSOR_UPDATE none Minor Healing Potion",
                "ACTIONBAR_SHOWGRID item Minor Healing Potion",
                "CURSOR_UPDATE item Minor Healing Potion",
                "MAIL_SEND_INFO_UPDATE item Minor Healing Potion"
            ]
        );
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(1));
        for slot in [1, 2] {
            let probe = bag_lock_probe(slot);
            assert!(s.eval::<bool>(&format!("return {probe} == 1")).unwrap());
        }

        // An empty hand takes the attachment back.
        s.run("ClearCursor()").unwrap();
        heard::take(&s);
        s.run("ClickSendMailItemButton()").unwrap();
        assert_eq!(
            heard::take(&s),
            [
                "CURSOR_UPDATE none nil",
                "ACTIONBAR_SHOWGRID item nil",
                "CURSOR_UPDATE item nil",
                "MAIL_SEND_INFO_UPDATE item nil"
            ]
        );
        assert_eq!(s.cursor_item().map(|i| i.slot), Some(2));

        // Nothing held and nothing attached: a plain clear, here of a held macro.
        s.run("ClearCursor()").unwrap();
        s.set_cursor_for_test(CursorPayload::Macro(CursorMacro {
            index: 1,
            texture: None,
        }));
        heard::take(&s);
        s.run("ClickSendMailItemButton()").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE none nil"]);
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
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        s.run("SendMail(\"Bob\", \"hi\", \"body\")").unwrap();
        assert!(
            s.take_mail_send().is_none(),
            "no selection: silently nothing"
        );
        s.run("SelectStationery(1) SendMail(\"Bob\", \"hi\", \"body\")")
            .unwrap();
        assert_eq!(s.take_mail_send().map(|r| r.stationery), Some(41));
        s.mail_request_resolved();
        s.reset_compose_tab_silently();
        s.run("SendMail(\"Bob\", \"hi\", \"body\")").unwrap();
        assert!(s.take_mail_send().is_none());
    }
}
