//! The mail bindings. The app pushes the open mailbox's inbox, already resolved from the wire
//! (`UiScript::set_mail`), and each verb past its guards hands the host its call ([`MailCall`]),
//! which sends inside it. Indices are 1-based, as `MailFrame.lua` uses them.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{self, flag};
use super::cursor;
use super::Model;

/// The GM stationery, `Stationery.dbc` row 61 (`0x4af2cb cmp [rec+0x11c],0x3d`).
const STATIONERY_GM: u32 = 61;

/// `CheckInbox`'s throttle window, 60 000 ms (`0x4aeb06`).
const CHECK_INBOX_WINDOW_SECS: f64 = 60.0;

/// One inbox row, resolved by the app from a wire `MailListEntry`; its 1-based index is its
/// position in [`MailState::inbox`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailInboxRow {
    /// The mail id (`[rec+0]`), which every request about the mail carries.
    pub mail_id: u32,
    /// The letter's item text id (`[rec+0x114]`), 0 for none: the key `GetInboxText` asks the
    /// body by (`0x4af1bf`).
    pub item_text_id: u32,
    /// The stationery id (`[rec+0x11c]`).
    pub stationery: u32,
    /// `packageIcon`: the package's icon for a package id the store holds, else the enclosed
    /// item's; `None` with neither, or while the item's template loads.
    pub package_icon: Option<String>,
    /// `stationeryIcon`: the paper's item's icon; `None` while its template loads.
    pub stationery_icon: Option<String>,
    /// `sender`: a player's name, or an auction house's, a creature's or a game object's by the
    /// sender id; `None` while in flight.
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
    /// A player's mail off the GM stationery (`0x4aefa5`-`0x4aefbc`), the Reply button's gate
    /// (`MailFrame.lua:281`).
    pub can_reply: bool,
    /// `stationery == 61`: a GM mail (vmangos `MAIL_STATIONERY_GM`, `Mail.h:85`).
    pub is_gm: bool,
    /// `GetInboxText`'s body, its text or its template's, `$`-expanded; `None` (nil) until the
    /// text lands, for an auction mail and for a mail with neither.
    pub body: Option<String>,
    /// The stationery's texture basename in the store (`[row+0x8]`), `None` (nil) for an id the
    /// store lacks (`0x4af27c`-`0x4af2a9`).
    pub stationery_texture: Option<String>,
    /// `isInvoice`: only an auction won (result 1) or sold (2), not an outbid or expiry notice
    /// (`0x4af2eb`).
    pub is_invoice: bool,
    /// `GetInboxInvoiceInfo`'s answer; `None`, the reference's miss, also while the body or the
    /// counterparty's name is in flight.
    pub invoice: Option<MailInvoice>,
    /// The enclosed item's template entry, 0 for none; the key `SetInboxItem`'s tooltip reads.
    pub item_id: u32,
    /// The enclosed item's random-suffix id (the wire `randomPropId`), 0 for none, which the
    /// reference's `SetInboxItem` hands the tooltip (`+0x424`); `item_name` carries its suffix.
    pub item_random_property_id: u32,
    /// The enclosed item's charges and `(current, max)` durability off the wire, which
    /// `SetInboxItem` writes into the tooltip's `+0x428` and `+0x438`/`+0x434` (`0x53566f`,
    /// `0x535699`, `0x53568d`).
    pub item_charges: i32,
    pub item_durability: (u32, u32),
    /// The enclosed item's name; `None` while its template is in flight.
    pub item_name: Option<String>,
    pub item_texture: Option<String>,
    pub item_quality: Option<u32>,
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

/// `SendMail` past its guards: what `CMSG_SEND_MAIL` carries (`0x4ae9be`-`0x4aea61`).
#[derive(Clone, Debug, PartialEq)]
pub struct MailSendRequest {
    pub target: String,
    pub subject: String,
    pub body: String,
    /// The selected `Stationery.dbc` id; never 0, as a send with none aborts (`0x4ae8dd`).
    pub stationery: u32,
    pub money: u32,
    pub cod: u32,
    /// The attached item's guid (`0xb6ef90`), 0 for none.
    pub item_guid: u64,
    /// The selected `Package.dbc` id, sent only with an attached item (`0x4ae8f3`).
    pub package: u32,
}

/// A mail verb past its binding's guards, which the host applies inside the call: its packet,
/// with the mailbox cell as it stands, before the verb returns.
#[derive(Clone, Debug, PartialEq)]
pub enum MailCall {
    /// `CheckInbox()` past its throttle (`0x4aeab0`): `CMSG_GET_MAIL_LIST` (`0x4aeb0b`).
    CheckInbox,
    /// `GetInboxText(index)` on a held row (`0x4af110`): an unread mail's read mark
    /// (`0x4af18e` → `0x4adcd0`), whose `MAIL_INBOX_UPDATE` (`0x4add9d`) runs its handlers.
    Open {
        index: u32,
        mail_id: u32,
        unread: bool,
    },
    /// `GetInboxText`'s body, after the mark: asked by the text id (`0x4af1bf`) and masked
    /// (`0x4af203`), the answer [`super::UiScript::answer_inbox_body`] leaves for the call.
    ReadBody {
        index: u32,
        mail_id: u32,
        text_id: u32,
    },
    /// A row verb past its refusals: its packet for `mail_id`, `TakeInboxTextItem`'s with the
    /// template id (`0x4afb4a`).
    Row {
        verb: MailRowVerb,
        mail_id: u32,
        template_id: u32,
    },
    /// `SendMail` past its guards: `CMSG_SEND_MAIL` (`0x4aea6c`).
    Send(MailSendRequest),
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

    /// The mail calls a harness's VM kept ([`Self::keep_calls`]), in call order.
    pub fn take_mail_calls(&mut self) -> Vec<MailCall> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::Mail(call) => Some(call.clone()),
            _ => None,
        })
    }

    /// The read mark's VM half (`0x4adcd0`), applied by the host after its packet: the row's read
    /// bit (`0x4add62`), its days left above 3 lowered to 3 when it carries no COD
    /// (`0x4add72`-`0x4add8e`), and `MAIL_INBOX_UPDATE` (`0x4add9d`).
    pub fn mark_mail_read(&mut self, index: u32) {
        super::tick::verb(&self.lua, |model| {
            let Some(row) = model.mail.as_mut().and_then(|m| {
                index
                    .checked_sub(1)
                    .and_then(|i| m.inbox.get_mut(i as usize))
            }) else {
                return;
            };
            row.was_read = true;
            if row.cod == 0 && row.days_left > 3.0 {
                row.days_left = 3.0;
            }
            model.raise("MAIL_INBOX_UPDATE", Vec::new());
        });
    }

    /// The body row `index` holds, for the host's mask of a read letter.
    pub fn inbox_body(&self, index: u32) -> Option<String> {
        let model = self.model_ref();
        row_at(&model, index as usize).and_then(|r| r.body)
    }

    /// The body the applying `GetInboxText` call answers, masked by the host (`0x4af203`).
    pub fn answer_inbox_body(&mut self, body: String) {
        self.model_mut().mail_body_answer = Some(body);
    }

    /// Whether the VM holds `state` as its inbox: a snapshot equal to what the calls already left
    /// there is no change for the scripts.
    pub fn holds_mail(&self, state: &Option<MailState>) -> bool {
        self.model_ref().mail == *state
    }

    /// Take the `CloseMail` flag; 1.12 has no close opcode, so the app only ends its mail session.
    pub fn take_mail_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_close)
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

/// `GetSendMailPrice()` (`0x4ae730`): 30 copper (`0x4ae756`); plus, for the selected paper in the
/// usable list (`0x4ae765`-`0x4ae774`) when the player carries none (`0x4ae77d`), its item's buy
/// price (`0x4ae7a0`); plus the selected package's cost (`0x4ae7b1`-`0x4ae7c6`); plus the money
/// enclosed (`0x4ae7c9`-`0x4ae7d2`), summed in 32 bits and answered unsigned (`0x4ae7d7`).
fn send_price(model: &Model) -> u32 {
    let paper = (model.mail_stationery != 0)
        .then(|| {
            model
                .mail_stationeries
                .iter()
                .find(|r| r.id == model.mail_stationery)
        })
        .flatten()
        .and_then(|r| r.cost)
        .unwrap_or(0);
    let package = (model.mail_package != 0)
        .then(|| {
            model
                .mail_packages
                .iter()
                .find(|p| p.id == model.mail_package)
        })
        .flatten()
        .map_or(0, |p| p.cost as u32);
    30u32
        .wrapping_add(paper)
        .wrapping_add(package)
        .wrapping_add(model.mail_send_money)
}

/// The five row verbs, each with its own refusal of the row before any send: silent, but for
/// TakeInboxItem's COD the purse cannot pay, which shows `ERR_NOT_ENOUGH_MONEY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailRowVerb {
    /// `TakeInboxItem`, `CMSG_MAIL_TAKE_ITEM` (`0x4af9d2`).
    TakeItem,
    /// `TakeInboxMoney`, `CMSG_MAIL_TAKE_MONEY` (`0x4af85c`).
    TakeMoney,
    /// `DeleteInboxItem`, `CMSG_MAIL_DELETE` (`0x4afd79`).
    Delete,
    /// `ReturnInboxItem`, `CMSG_MAIL_RETURN_TO_SENDER` (`0x4afc4d`).
    Return,
    /// `TakeInboxTextItem`, `CMSG_MAIL_CREATE_TEXT_ITEM` (`0x4afb08`).
    TakeText,
}

impl MailRowVerb {
    /// `Ok` when the reference sends for `row`, else the refusal's message key, if it shows one.
    fn sends(self, model: &Model, row: &MailInboxRow) -> Result<(), Option<&'static str>> {
        // An item (`[rec+0x120]`, `0x4af96d`), then the player's object (`0x4af992`) and the purse
        // to pay its COD (`[rec+0x144]` against the coinage, `0x4af9c5`); either miss shows
        // `ERR_NOT_ENOUGH_MONEY` (`0x4afa4c`, `push 0x25`).
        if let MailRowVerb::TakeItem = self {
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
            MailRowVerb::TakeItem => true,
            // Money (`[rec+0x140]`, `0x4af85a`).
            MailRowVerb::TakeMoney => row.money != 0,
            // A body or a template to copy (`[rec+0x114]`, `[rec+0x25c]`, `0x4afae7`), not yet
            // copied (`& 4`, `0x4afafb`).
            MailRowVerb::TakeText => {
                (row.item_text_id != 0 || row.mail_template_id != 0) && !row.text_created
            }
            // A player's mail (type 0, `0x4afc27`) with an item or money (`0x4afc30`), neither
            // returned nor a COD payment (`& 0xa`, `0x4afc44`).
            MailRowVerb::Return => {
                row.message_type == 0 && (row.item_id != 0 || row.money != 0) && !returned_or_cod
            }
            // Returned or a COD payment, not a player's, or empty (`0x4afd57`-`0x4afd77`).
            MailRowVerb::Delete => {
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetInboxHeaderInfo(index)` (`0x8463b8`, `0x4aebee`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetInboxHeaderInfo(index)")?;
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
            // A COD payment's subject goes through `COD_PAYMENT` on any mail type, `""` when the
            // string is missing (`[rec+0x148] & 8`, `0x4aeeb0`-`0x4aeee9`, `0x703bf0`).
            let subject = if r.cod_payment {
                lua.globals()
                    .get::<Option<String>>("COD_PAYMENT")?
                    .unwrap_or_default()
                    .replacen("%s", &r.subject, 1)
            } else {
                r.subject.clone()
            };
            Ok(MultiValue::from_vec(vec![
                // The row's package or item icon, GM letter or not (`0x4aec76`-`0x4aecf8`).
                opt_str(&r.package_icon)?,
                opt_str(&r.stationery_icon)?,
                opt_str(&r.sender)?,
                Value::String(lua.create_string(&subject)?),
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

    // GetInboxText(index): body, stationeryTexture, isTakeable, isInvoice (`MailFrame.lua:292`),
    // read after the call marks the mail and asks for its body.
    g.set(
        "GetInboxText",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetInboxText(index)` (`0x8463dc`, `0x4af142`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetInboxText(index)")?;
            let read = |lua: &Lua| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                row_at(&model, index)
            };
            // A miss pushes four nils (`0x4af311`..`0x4af334`).
            let Some(r) = read(lua) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 4]));
            };
            let index = index as u32;
            submit(
                lua,
                MailCall::Open {
                    index,
                    mail_id: r.mail_id,
                    unread: !r.was_read,
                },
            )?;
            // The body is read after the mark's event, whose handlers may read the letter
            // themselves (`0x4add9d` ahead of `0x4af1bf`), so nothing runs between this call's
            // answer and its take.
            let answer = if r.item_text_id != 0 {
                let answer = |lua: &Lua| {
                    std::mem::take(
                        &mut lua
                            .app_data_mut::<Model>()
                            .expect("model app_data")
                            .mail_body_answer,
                    )
                };
                // An answer a call that waited for the host left is no answer to this one.
                answer(lua);
                submit(
                    lua,
                    MailCall::ReadBody {
                        index,
                        mail_id: r.mail_id,
                        text_id: r.item_text_id,
                    },
                )?;
                answer(lua)
            } else {
                None
            };
            // The row as the call left it.
            let Some(r) = read(lua) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 4]));
            };
            // The body, nil on every miss (`0x4af275`).
            let body = match answer.as_ref().or(r.body.as_ref()) {
                Some(b) => Value::String(lua.create_string(b)?),
                None => Value::Nil,
            };
            // A text or a template (`0x4af2ae`-`0x4af2c0`), not yet copied (`0x4af2c2`), off the
            // GM stationery 61 (`0x4af2cb`).
            let takeable = (r.item_text_id != 0 || r.mail_template_id != 0)
                && !r.text_created
                && r.stationery != STATIONERY_GM;
            Ok(MultiValue::from_vec(vec![
                body,
                match &r.stationery_texture {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                },
                flag(takeable),
                flag(r.is_invoice),
            ]))
        })?,
    )?;

    // GetInboxInvoiceInfo(index) (`MailFrame.lua:302`): always seven values (`0x4af5a1`), a miss
    // being three nils and four zeros (`0x4af559`).
    g.set(
        "GetInboxInvoiceInfo",
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetInboxInvoiceInfo(index)` (`0x84642c`, `0x4af38e`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetInboxInvoiceInfo(index)")?;
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: GetInboxItem(index)` (`0x846450`, `0x4af5fe`).
            let index = binding_abi::index_arg(lua, index, "Usage: GetInboxItem(index)")?;
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
        lua.create_function(|lua, index: Value| {
            // `is-number` else `Usage: InboxItemCanDelete(index)` (`0x846508`, `0x4afe11`).
            let index = binding_abi::index_arg(lua, index, "Usage: InboxItemCanDelete(index)")?;
            // `DeleteInboxItem`'s own rule (`0x4afe4c`-`0x4afe66`), with none of its guards on
            // the lock or the mailbox.
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let can = row_at(&model, index)
                .is_some_and(|r| MailRowVerb::Delete.sends(&model, &r).is_ok());
            Ok(flag(can))
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
    // signalling `MAIL_INBOX_UPDATE` in place (`0x4aeaf8`); otherwise it sends the query, stamps
    // the window and takes the lock (`0x4aeb06`-`0x4aeb18`), and the answer announces the list.
    g.set(
        "CheckInbox",
        lua.create_function(|lua, ()| {
            let query = super::tick::verb(lua, |model| {
                if !mail_ready(model) {
                    return false;
                }
                if model.mail_list_stamp.is_some_and(|t| model.now < t) {
                    model.raise("MAIL_INBOX_UPDATE", Vec::new());
                    return false;
                }
                model.mail_list_stamp = Some(model.now + CHECK_INBOX_WINDOW_SECS);
                model.mail_busy = true;
                true
            });
            if query {
                submit(lua, MailCall::CheckInbox)?;
            }
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
            MailRowVerb::TakeItem,
            "Usage: TakeInboxItem(index)",
        ),
        (
            "TakeInboxMoney",
            MailRowVerb::TakeMoney,
            "Usage: TakeInboxMoney(index)",
        ),
        (
            "DeleteInboxItem",
            MailRowVerb::Delete,
            "Usage: DeleteInboxItem(index)",
        ),
        (
            "ReturnInboxItem",
            MailRowVerb::Return,
            "Usage: ReturnInboxItem(index)",
        ),
        (
            "TakeInboxTextItem",
            MailRowVerb::TakeText,
            "Usage: TakeInboxTextItem(index)",
        ),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, index: Value| {
                let n = super::binding_abi::number_arg(lua, index, usage)?;
                let call = {
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
                    model.mail_busy = true;
                    MailCall::Row {
                        verb,
                        mail_id: row.mail_id,
                        template_id: row.mail_template_id,
                    }
                };
                submit(lua, call)
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
            let send = super::tick::verb(lua, |model| {
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
                    return None;
                }
                // An attached item whose object is gone (`0x4ae92e` resolves its guid):
                // `ERR_ITEM_NOT_FOUND` (equip error 0x17, `0x4ae957`), the attachment dropped and
                // `MAIL_SEND_INFO_UPDATE` (`0x4ae98d`), no packet. Checked before the money
                // (`0x4ae917` ahead of `0x4ae99e`). The attachment is locked, so it cannot have
                // moved.
                if let Some(item) = model.mail_send_item.as_ref() {
                    if !holds(model, item) {
                        model.push_ui_error("ERR_ITEM_NOT_FOUND");
                        model.mail_send_item = None;
                        model.raise("MAIL_SEND_INFO_UPDATE", Vec::new());
                        return None;
                    }
                }
                // Money with COD, or COD with no item (`0x4ae99e`-`0x4ae9b8`).
                if model.mail_send_money != 0 && model.mail_send_cod != 0 {
                    return None;
                }
                if model.mail_send_cod != 0 && model.mail_send_item.is_none() {
                    return None;
                }
                // The send raises the pending flag and takes the request lock (`0x4aea76`,
                // `0x4aea7b`).
                model.mail_send_pending = true;
                model.mail_busy = true;
                Some(MailSendRequest {
                    target,
                    subject,
                    body,
                    stationery: model.mail_stationery,
                    money: model.mail_send_money,
                    cod: model.mail_send_cod,
                    item_guid: model.mail_send_item.as_ref().map_or(0, |it| it.guid),
                    package: model.mail_package,
                })
            });
            match send {
                Some(req) => submit(lua, MailCall::Send(req)),
                None => Ok(()),
            }
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

    g.set(
        "GetSendMailPrice",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(send_price(&model)))
        })?,
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

/// Hand `call` to the host, which applies it before this returns ([`super::calls::submit`]).
fn submit(lua: &Lua, call: MailCall) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::Mail(call))
}

/// The error `ClickSendMailItemButton` refuses a held item with (`0x4adf38`-`0x4ae042`), read off
/// the item's slot and its cache record (`0x55ba30`, no callback), before any swap; none when the
/// slot no longer holds it, as an item the cursor's guid finds no object for passes
/// (`0x4adf3a`). In order: bound (`0x5da2c0`) is `ERR_MAIL_QUEST_ITEM` for quest bonding 4 or 5
/// (`0x4adf75`-`0x4adf7f`) and `ERR_MAIL_BOUND_ITEM` otherwise, an uncached record included
/// (`0x4adf6d`, `0x4adf97`); record flags conjured `0x2` (`0x4adfbd`), an area (`0x5da500`, record
/// `+0x1c8`) or a map (`0x5da570`, `+0x1cc`) is `ERR_MAIL_CONJURED_ITEM`; a container holding an
/// item is `ERR_MAIL_BAG` (`0x4ae019`-`0x4ae042`).
fn attach_refusal(model: &Model, item: &cursor::CursorItem) -> Option<&'static str> {
    let (bound, seats) = if item.bag == cursor::EQUIPMENT_BAG {
        let s = model
            .inv_slot("player", item.slot as usize)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.occupied_seats)
    } else {
        let s = model
            .containers
            .get(&item.bag)?
            .slots
            .get(&item.slot)
            .filter(|s| s.guid == item.guid)?;
        (s.already_bound, s.occupied_seats)
    };
    let record = model.item_templates.get(&item.item_id);
    /// `BIND_QUEST_ITEM` and `BIND_QUEST_ITEM1`.
    const QUEST_BONDING: [u32; 2] = [4, 5];
    if bound {
        return Some(
            if record.is_some_and(|r| QUEST_BONDING.contains(&r.bonding)) {
                "ERR_MAIL_QUEST_ITEM"
            } else {
                "ERR_MAIL_BOUND_ITEM"
            },
        );
    }
    if record.is_some_and(|r| r.flags & 0x2 != 0 || r.area != 0 || r.map != 0) {
        return Some("ERR_MAIL_CONJURED_ITEM");
    }
    seats.is_some_and(|n| n > 0).then_some("ERR_MAIL_BAG")
}

/// Whether the attachment's slot, a bag's or the doll's, still holds its object: the send
/// resolves the guid and tests its owner (`0x4ae92e`-`0x4ae955`), and a locked item cannot move.
fn holds(model: &Model, item: &cursor::CursorItem) -> bool {
    if item.bag == cursor::EQUIPMENT_BAG {
        model
            .inv_slot("player", item.slot as usize)
            .is_some_and(|s| s.guid == item.guid)
    } else {
        model
            .containers
            .get(&item.bag)
            .and_then(|c| c.slots.get(&item.slot))
            .is_some_and(|s| s.guid == item.guid)
    }
}

/// `ClickSendMailItemButton` (`0x4adef0`): the send slot as a cursor drop target, attaching the
/// held item past its refusals ([`attach_refusal`]) and putting an earlier attachment back on
/// the cursor ([`cursor::click_attach_slot`]).
/// A held item stays in its bag slot, locked, until the send. A changed attachment signals
/// `MAIL_SEND_INFO_UPDATE` (`0x4ae0de`), whose stock handler re-reads the item and postage.
fn click_send_mail_item(lua: &Lua) {
    let refusal = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match &model.cursor {
            Some(cursor::CursorPayload::Item(item)) => attach_refusal(&model, item),
            _ => None,
        }
    };
    if let Some(key) = refusal {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .push_ui_error(key);
        return;
    }
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

    /// A VM with no call host, keeping its calls for the takes below.
    pub(super) fn kept() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.keep_calls();
        s
    }

    /// Whether a `CheckInbox` call was kept since the last take.
    fn checks(s: &mut UiScript) -> bool {
        !s.take_calls_where(|c| {
            matches!(c, crate::script::ScriptCall::Mail(MailCall::CheckInbox)).then_some(())
        })
        .is_empty()
    }

    /// The mail ids of the kept `verb` calls.
    pub(super) fn rows(s: &mut UiScript, verb: MailRowVerb) -> Vec<u32> {
        s.take_calls_where(|c| match c {
            crate::script::ScriptCall::Mail(MailCall::Row {
                verb: v, mail_id, ..
            }) if *v == verb => Some(*mail_id),
            _ => None,
        })
    }

    /// The first kept `SendMail` call.
    pub(super) fn sends(s: &mut UiScript) -> Option<MailSendRequest> {
        s.take_calls_where(|c| match c {
            crate::script::ScriptCall::Mail(MailCall::Send(req)) => Some(req.clone()),
            _ => None,
        })
        .into_iter()
        .next()
    }

    /// The rows the kept `GetInboxText` calls opened.
    fn opens(s: &mut UiScript) -> Vec<u32> {
        s.take_calls_where(|c| match c {
            crate::script::ScriptCall::Mail(MailCall::Open { index, .. }) => Some(*index),
            _ => None,
        })
    }

    /// `CheckInbox` (`0x4aeab0`): no mailbox or a request in flight does nothing; the first
    /// check queues the query and takes the lock; once answered, a check inside the 60 s window
    /// re-announces the held list with `MAIL_INBOX_UPDATE` in place (`0x4aeaf8`), and past it
    /// queries again.
    #[test]
    fn check_inbox_throttles_to_an_in_place_reannounce() {
        use crate::script::tick::heard;
        let mut s = kept();
        heard::listen(&s, &["MAIL_INBOX_UPDATE"], None);
        s.run("CheckInbox()").unwrap();
        assert!(!checks(&mut s), "no mailbox, no query");

        s.set_mail(Some(MailState::default()));
        s.run("CheckInbox()").unwrap();
        assert!(!checks(&mut s), "rows held, no mailbox open");
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        s.run("CheckInbox()").unwrap();
        assert!(checks(&mut s));
        s.run("CheckInbox()").unwrap();
        assert!(!checks(&mut s), "the lock holds until the answer");
        assert!(heard::take(&s).is_empty());

        s.mail_request_resolved();
        s.tick(30.0);
        s.run("CheckInbox()").unwrap();
        assert!(!checks(&mut s));
        assert_eq!(heard::take(&s), ["MAIL_INBOX_UPDATE"]);

        s.tick(31.0);
        s.run("CheckInbox()").unwrap();
        assert!(checks(&mut s), "the window ran out: a fresh query");
        assert!(heard::take(&s).is_empty());
    }

    /// The compose verbs fire in place, each after its write (`0x4adc39` then `0x4adc44`), and
    /// `ClearSendMail` after all its clears (`0x4acddf`-`0x4ace0c`, then `0x4ace19`, `0x4ace23`,
    /// `0x4ace2d`): every handler reads the stored amount before the next Lua line runs.
    #[test]
    fn the_compose_verbs_fire_their_events_in_place() {
        let mut s = kept();
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
            mail_id: 7,
            item_text_id: 1,
            stationery: 41,
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
            stationery_texture: Some("STATIONERYTEST".into()),
            is_invoice: false,
            invoice: None,
            item_id,
            item_name: (item_id != 0).then(|| "Linen Cloth".to_string()),
            item_texture: (item_id != 0)
                .then(|| "Interface\\Icons\\INV_Fabric_Linen_01".to_string()),
            item_quality: (item_id != 0).then_some(1),
            item_random_property_id: 0,
            item_charges: 0,
            item_durability: (0, 0),
            cod_payment: false,
            message_type: 0,
            mail_template_id: 0,
        }
    }

    /// Two rows whose mail ids are their indices.
    fn state() -> MailState {
        let (mut first, mut second) = (row(2589), row(0));
        (first.mail_id, second.mail_id) = (1, 2);
        MailState {
            inbox: vec![first, second],
            open: true,
        }
    }

    #[test]
    fn the_invoice_answers_seven_values_or_a_three_nil_four_zero_miss() {
        let mut s = kept();
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
        let mut s = kept();
        let mut st = state();
        // The app resolves an auction mail's body to none (`0x4af1cf`).
        (st.inbox[0].is_invoice, st.inbox[0].message_type) = (true, 2);
        st.inbox[0].body = None;
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
        let mut s = kept();
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
        let mut s = kept();
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

    /// A COD payment's header subject goes through `COD_PAYMENT` (`[rec+0x148] & 8`,
    /// `0x4aeeb0`-`0x4aeee9`), whatever the mail's type; any other row's is pushed as held.
    #[test]
    fn a_cod_payments_subject_goes_through_cod_payment() {
        let mut s = kept();
        s.run("COD_PAYMENT = 'COD Payment: %s'").unwrap();
        let mut paid = row(0);
        (paid.mail_id, paid.cod_payment, paid.message_type) = (1, true, 2);
        let mut plain = row(0);
        plain.mail_id = 2;
        s.set_mail(Some(MailState {
            inbox: vec![paid, plain],
            open: true,
        }));
        let subjects: (String, String) = s
            .eval(
                "local _, _, _, a = GetInboxHeaderInfo(1) local _, _, _, b = GetInboxHeaderInfo(2) \
                 return a, b",
            )
            .unwrap();
        assert_eq!(
            subjects,
            (
                "COD Payment: Warchief's orders".to_string(),
                "Warchief's orders".to_string()
            )
        );
    }

    /// The header's first icon is the row's, a GM letter's and an item-less row's alike: the
    /// binding pushes what `0x4aec76`-`0x4aecf8` found, with no GM or item gate.
    #[test]
    fn the_header_pushes_the_rows_package_icon_ungated() {
        let mut s = kept();
        let mut gm = row(2589);
        (gm.mail_id, gm.is_gm, gm.can_reply) = (1, true, false);
        let mut boxed = row(0);
        boxed.mail_id = 2;
        boxed.package_icon = Some("Interface\\Icons\\INV_BOX_04".into());
        s.set_mail(Some(MailState {
            inbox: vec![gm, boxed],
            open: true,
        }));
        let icons: (String, String) = s
            .eval("return GetInboxHeaderInfo(1), GetInboxHeaderInfo(2)")
            .unwrap();
        assert_eq!(
            icons,
            (
                "Interface\\Icons\\INV_Misc_Bag_08".to_string(),
                "Interface\\Icons\\INV_BOX_04".to_string()
            )
        );
    }

    /// A row that does not exist still answers the full tuple: `GetInboxHeaderInfo` 13 values,
    /// four nils, `0` money, COD and days left, six nils (`0x4aeff0`..`0x4af05e`), so a closed
    /// letter's `GetInboxHeaderInfo(0)` can be compared with `CODAmount > 0` (`MailFrame.lua:399`);
    /// `GetInboxText` four nils (`0x4af311`..`0x4af334`).
    #[test]
    fn a_missing_inbox_row_answers_the_references_miss_tuples() {
        let mut s = kept();
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
            opens(&mut s).is_empty(),
            "a miss opens nothing for the app to fetch"
        );
    }

    #[test]
    fn get_inbox_text_returns_body_and_calls_for_the_open() {
        let mut s = kept();
        s.set_mail(Some(state()));
        let (body, tex, takeable, invoice): (String, String, Value, Value) =
            s.eval("return GetInboxText(1)").unwrap();
        assert_eq!(body, "Lok'tar.");
        assert_eq!(tex, "STATIONERYTEST");
        assert_eq!(takeable, Value::Integer(1)); // has a body to fetch
        assert!(matches!(invoice, Value::Nil));
        // Each read is its own call; the host marks only an unread mail.
        s.run("GetInboxText(1)").unwrap();
        assert_eq!(opens(&mut s), [1, 1]);
        assert!(opens(&mut s).is_empty(), "taken");
    }

    #[test]
    fn get_inbox_item_and_can_delete() {
        let mut s = kept();
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
        let mut s = kept();
        s.set_unit("player", Some(crate::script::UnitState::default()));
        let mut st = state();
        // An empty player's mail: deletable (`0x4afd65`-`0x4afd77`).
        st.inbox[1].money = 0;
        s.set_mail(Some(st));
        let all = "TakeInboxItem(1) TakeInboxMoney(1) DeleteInboxItem(2) ReturnInboxItem(1) \
                   TakeInboxTextItem(1) CheckInbox()";
        s.run(all).unwrap();
        assert_eq!(
            rows(&mut s, MailRowVerb::TakeItem),
            vec![1],
            "the first send"
        );
        assert!(
            rows(&mut s, MailRowVerb::TakeMoney).is_empty(),
            "the rest wait on it"
        );
        assert!(!checks(&mut s));
        for (verb, kind) in [
            ("TakeInboxMoney(1)", MailRowVerb::TakeMoney),
            ("DeleteInboxItem(2)", MailRowVerb::Delete),
            ("ReturnInboxItem(1)", MailRowVerb::Return),
            ("TakeInboxTextItem(1)", MailRowVerb::TakeText),
        ] {
            s.mail_request_resolved();
            s.run(&format!("{verb} {verb}")).unwrap();
            assert_eq!(
                rows(&mut s, kind).len(),
                1,
                "{verb}: one send, then the lock"
            );
        }
        s.mail_request_resolved();
        s.run("CheckInbox() CheckInbox()").unwrap();
        assert!(checks(&mut s));
        assert!(!checks(&mut s), "drained");

        s.mail_request_resolved();
        select_default_stationery(&mut s);
        s.tick(61.0);
        s.run("SendMail('Jaina', 'Hi', 'body') CheckInbox()")
            .unwrap();
        assert!(sends(&mut s).is_some());
        assert!(!checks(&mut s), "the send holds the lock");

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
        let mut s = kept();
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        select_default_stationery(&mut s);
        s.run("SendMail('', 'Hi', 'b') SendMail('Bob', '', 'b') SendMail('Bob', nil, 'b')")
            .unwrap();
        assert!(sends(&mut s).is_none(), "no recipient or no subject");
        s.run("SendMail('Bob', 'Hi')").unwrap();
        assert_eq!(sends(&mut s).map(|r| r.body), Some(String::new()));

        s.mail_request_resolved();
        s.run("SendMail('Bob', 'Again', 'b')").unwrap();
        assert!(sends(&mut s).is_none(), "the first send still pends");
        s.mail_send_resolved();
        s.run("SendMail('Bob', 'Again', 'b')").unwrap();
        assert!(sends(&mut s).is_some());

        s.mail_request_resolved();
        s.reset_compose_tab();
        select_default_stationery(&mut s);
        s.run("SendMail('Bob', 'Third', 'b')").unwrap();
        assert!(sends(&mut s).is_some(), "the compose reset lifted it");
    }

    /// The attached item is checked before money and COD (`0x4ae917` ahead of `0x4ae99e`), by its
    /// object (`0x4ae92e`): with another item of the same entry in its slot, the send shows
    /// `ERR_ITEM_NOT_FOUND`, drops the attachment and signals `MAIL_SEND_INFO_UPDATE`
    /// (`0x4ae98d`) even with money and COD both set.
    #[test]
    fn send_mail_checks_its_item_before_the_money() {
        use crate::script::cursor::two_item_bag;
        use crate::script::tick::heard;
        let mut s = kept();
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
        assert!(sends(&mut s).is_none());
        assert_eq!(s.take_ui_errors(), vec!["ERR_ITEM_NOT_FOUND"]);
        assert_eq!(heard::take(&s), ["MAIL_SEND_INFO_UPDATE"]);
        assert!(s.eval::<bool>("return GetSendMailItem() == nil").unwrap());
    }

    /// The send finds its attachment by the object and its owner (`0x4ae92e`-`0x4ae955`), wherever
    /// the player holds it: an item attached off the doll sends.
    #[test]
    fn send_mail_sends_an_item_attached_off_the_doll() {
        let mut s = kept();
        s.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        let mut doll: crate::script::InventorySlots = Default::default();
        doll[16] = Some(crate::script::InvSlotView {
            item_id: 7000,
            guid: 0x716,
            count: 1,
            ..Default::default()
        });
        s.set_inventory_slots(doll);
        s.run("PickupInventoryItem(16) ClickSendMailItemButton()")
            .unwrap();
        assert!(s.model_ref().mail_send_item.is_some(), "attached");
        select_default_stationery(&mut s);
        let _ = s.take_ui_errors();
        s.run("SendMail('Bob', 'Hi', 'b')").unwrap();
        assert!(s.take_ui_errors().is_empty());
        assert_eq!(sends(&mut s).map(|r| r.item_guid), Some(0x716));
    }

    /// `CloseMail` zeroes the mailbox inside the verb (`0x4aded0` → `0x4acd8f`): a mail verb
    /// later in the same chunk finds none.
    #[test]
    fn close_mail_closes_before_the_next_verb() {
        let mut s = kept();
        s.set_mail(Some(state()));
        s.run("CloseMail() CheckInbox()").unwrap();
        assert!(s.take_mail_close());
        assert!(!checks(&mut s));
    }

    /// Each row verb refuses a row the reference does not send for, and then takes no lock:
    /// money to take (`0x4af85a`), an item and the purse for its COD (`0x4af96d`, `0x4af9c5`), a
    /// body not yet copied (`0x4afae7`, `0x4afafb`), a player's unreturned mail with something
    /// in it to return (`0x4afc27`-`0x4afc44`), and for delete the reverse (`0x4afd57`-`0x4afd77`).
    #[test]
    fn each_row_verb_refuses_a_row_it_cannot_send_for() {
        let mut s = kept();
        s.set_unit("player", Some(crate::script::UnitState::default()));
        let mut empty = row(0);
        empty.money = 0;
        empty.item_text_id = 0;
        let mut cod = row(2589);
        cod.cod = 100;
        let mut copied = row(0);
        copied.text_created = true;
        let mut returned = row(2589);
        returned.was_returned = true;
        let mut inbox = vec![empty, cod, copied, returned, row(2589)];
        for (id, r) in (1..).zip(&mut inbox) {
            r.mail_id = id;
        }
        s.set_mail(Some(MailState { inbox, open: true }));
        s.run(
            "TakeInboxMoney(1) TakeInboxItem(1) TakeInboxTextItem(1) ReturnInboxItem(1) \
             TakeInboxItem(2) TakeInboxTextItem(3) ReturnInboxItem(4) DeleteInboxItem(5) \
             TakeInboxItem(9)",
        )
        .unwrap();
        assert!(rows(&mut s, MailRowVerb::TakeMoney).is_empty());
        assert!(rows(&mut s, MailRowVerb::TakeItem).is_empty());
        assert!(rows(&mut s, MailRowVerb::TakeText).is_empty());
        assert!(rows(&mut s, MailRowVerb::Return).is_empty());
        assert!(rows(&mut s, MailRowVerb::Delete).is_empty());
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_NOT_ENOUGH_MONEY"],
            "the unpayable COD shows its refusal (`0x4afa4c`); the rest are silent"
        );
        // None of them took the lock: a deletable empty mail still goes.
        s.run("DeleteInboxItem(1)").unwrap();
        assert_eq!(rows(&mut s, MailRowVerb::Delete), vec![1]);
        s.mail_request_resolved();
        s.set_money(100);
        s.run("TakeInboxItem(2)").unwrap();
        assert_eq!(
            rows(&mut s, MailRowVerb::TakeItem),
            vec![2],
            "the COD is affordable now"
        );
        // With no player object the take refuses the same way (`0x4af992`).
        s.mail_request_resolved();
        s.set_unit("player", None);
        s.run("TakeInboxItem(5)").unwrap();
        assert!(rows(&mut s, MailRowVerb::TakeItem).is_empty());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);
    }

    #[test]
    fn send_folds_money_cod_and_returns_true_from_setmoney() {
        let mut s = kept();
        // An open mailbox: the send verbs test it (`0x4ae852`).
        s.set_mail(Some(crate::script::MailState {
            open: true,
            ..Default::default()
        }));
        s.set_money(5_000);
        assert!(sends(&mut s).is_none());
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
        let req = sends(&mut s).expect("a send was queued");
        assert_eq!(req.target, "Jaina");
        assert_eq!(req.subject, "Hi");
        assert_eq!(req.body, "body text");
        assert_eq!((req.money, req.cod), (1234, 0));
        assert_eq!(req.item_guid, 0);
        assert!(sends(&mut s).is_none(), "drained");
        // The postage and the money still enclosed, until the send's result resets the form.
        assert_eq!(s.eval::<i64>("return GetSendMailPrice()").unwrap(), 1264);
        assert!(
            s.run("SetSendMailMoney(nil)").is_err(),
            "a non-number raises"
        );
        assert!(s.run("SetSendMailCOD({})").is_err());
    }

    #[test]
    fn attach_from_cursor_and_get_send_item() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = kept();
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
        assert_eq!(sends(&mut s).unwrap().item_guid, 0x102);

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
        let mut s = kept();
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
        let req = sends(&mut s).unwrap();
        assert_eq!((req.money, req.cod, req.item_guid), (0, 0, 0));
    }

    /// `ClearSendMail()` (`0x4adee0`) is the compose reset with its events, the attachment's lock
    /// first, then the three in `0x4acdc0`'s order; stationery and package clear with the rest.
    #[test]
    fn clear_send_mail_resets_the_form_and_fires_its_events() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        let mut s = kept();
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
        assert!(sends(&mut s).is_none(), "the stationery cleared too");
        select_default_stationery(&mut s);
        s.run("SendMail('x', 'y', 'z')").unwrap();
        assert_eq!(sends(&mut s).map(|r| r.package), Some(0));
    }

    /// `Package.dbc`'s one shipped row, through the three package verbs (`0x4ae430`, `0x4ae450`,
    /// `0x4ae550`).
    #[test]
    fn packages_list_describe_and_select_by_row() {
        let mut s = kept();
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
        assert_eq!(sends(&mut s).map(|r| r.package), Some(2));
        s.mail_request_resolved();
        s.mail_send_resolved();
        s.run("SelectPackage(5) SendMail('x', 'y', 'z')").unwrap();
        assert_eq!(
            sends(&mut s).map(|r| r.package),
            Some(0),
            "out of range stores 0"
        );
    }

    #[test]
    fn clearing_the_mail_empties_it() {
        let mut s = kept();
        s.set_mail(Some(state()));
        s.set_mail(None);
        assert_eq!(s.eval::<i64>("return GetInboxNumItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return HasNewMail() == nil").unwrap());
    }

    #[test]
    fn has_new_mail_reads_the_pushed_flag() {
        let mut s = kept();
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
        let mut s = kept();
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
        let mut s = kept();
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
            id: 1,
            texture: None,
        }));
        heard::take(&s);
        s.run("ClickSendMailItemButton()").unwrap();
        assert!(s.cursor_payload().is_none());
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE none nil"]);
    }

    /// `GetInboxText`'s third return (`0x4af2ae`-`0x4af2e6`): 1 for a text id or a template id
    /// (`0x4af2b6`, `0x4af2c0`), nil for a letter already copied (`0x4af2c2`) or on the GM
    /// stationery 61 (`0x4af2cb`).
    #[test]
    fn is_takeable_needs_a_text_or_a_template_uncopied_off_gm_paper() {
        let mut s = kept();
        s.keep_calls();
        let mut template = row(0);
        template.item_text_id = 0;
        template.mail_template_id = 87;
        let mut copied = row(0);
        copied.text_created = true;
        let mut gm = row(0);
        (gm.stationery, gm.is_gm) = (61, true);
        let mut neither = row(0);
        neither.item_text_id = 0;
        s.set_mail(Some(MailState {
            inbox: vec![row(0), template, copied, gm, neither],
            open: true,
        }));
        let takeable: Vec<bool> = (1..=5)
            .map(|i| {
                s.eval(&format!("local _, _, t = GetInboxText({i}) return t == 1"))
                    .unwrap()
            })
            .collect();
        assert_eq!(takeable, [true, true, false, false, false]);
    }

    /// `GetInboxText`'s body is nil on every miss (`0x4af275`): a text not yet in the cache
    /// (`0x4af1c9`), and a mail with neither a text id nor a template id (`0x4af21f`).
    #[test]
    fn an_unfetched_or_absent_body_is_nil() {
        let mut s = kept();
        s.keep_calls();
        let mut unfetched = row(0);
        unfetched.body = None;
        let mut neither = row(0);
        (neither.item_text_id, neither.body) = (0, None);
        s.set_mail(Some(MailState {
            inbox: vec![row(0), unfetched, neither],
            open: true,
        }));
        let bodies: Vec<Option<String>> = (1..=3)
            .map(|i| s.eval(&format!("return (GetInboxText({i}))")).unwrap())
            .collect();
        assert_eq!(bodies, [Some("Lok'tar.".to_string()), None, None]);
    }

    /// `InboxItemCanDelete` (`0x4afe00`) is `DeleteInboxItem`'s own rule: returned or a COD
    /// payment (`0x4afe4c test [rec+0x148],0xa`), not a player's mail (`0x4afe55`), or holding
    /// neither item nor money (`0x4afe5c`, `0x4afe66`); a bad index is nil (`0x4afe3d`).
    #[test]
    fn can_delete_is_the_delete_verbs_own_rule() {
        let mut s = kept();
        let mut cod_payment = row(0);
        cod_payment.cod_payment = true;
        let mut returned = row(2589);
        returned.was_returned = true;
        let mut auction = row(2589);
        auction.message_type = 2;
        let mut empty = row(0);
        empty.money = 0;
        s.set_mail(Some(MailState {
            inbox: vec![row(2589), row(0), cod_payment, returned, auction, empty],
            open: true,
        }));
        let can: Vec<bool> = (1..=7)
            .map(|i| {
                s.eval(&format!("return InboxItemCanDelete({i}) == 1"))
                    .unwrap()
            })
            .collect();
        assert_eq!(can, [false, false, true, true, true, true, false]);
    }

    /// `GetSendMailPrice` (`0x4ae730`): 30 copper (`0x4ae756`), plus the selected paper's price
    /// when the player carries none (`0x4ae77d`, `0x4ae7a0`), plus the selected package's cost
    /// (`0x4ae7c6`), plus the money enclosed (`0x4ae7c9`-`0x4ae7d2`).
    #[test]
    fn the_send_price_sums_postage_paper_package_and_money() {
        let mut s = kept();
        s.set_money(10_000);
        let paper = |id: u32, cost: Option<u32>| StationeryView {
            id,
            name: format!("Paper {id}"),
            icon: "Interface\\Icons\\INV_Letter_15".into(),
            cost,
            texture: "STATIONERYTEST".into(),
        };
        s.set_mail_stationeries(vec![
            paper(41, Some(0)),
            paper(42, Some(25)),
            paper(43, None),
        ]);
        s.set_mail_packages(vec![PackageView {
            id: 2,
            name: "Test Package".into(),
            icon: "Interface\\Icons\\INV_BOX_04".into(),
            cost: 10,
        }]);
        let price =
            |lua: &str| -> i64 { s.eval(&format!("{lua} return GetSendMailPrice()")).unwrap() };
        assert_eq!(price(""), 30, "no paper selected (`0x4ae75b`)");
        assert_eq!(price("SelectStationery(2)"), 55);
        assert_eq!(price("SelectStationery(3)"), 30, "a carried paper is free");
        assert_eq!(price("SelectPackage(1)"), 40);
        assert_eq!(price("SetSendMailMoney(500)"), 540);
    }

    /// `ClickSendMailItemButton` refuses the held item off its live record before any swap
    /// (`0x4adf38`-`0x4ae042`), in the reference's order: bound (`0x5da2c0`) is
    /// `ERR_MAIL_QUEST_ITEM` for quest bonding 4 or 5 (`0x4adf75`-`0x4adf7f`) and
    /// `ERR_MAIL_BOUND_ITEM` otherwise (`0x4adf97`); conjured (`0x4adfbd`) or bound to an area or a
    /// map (`0x4adfe2`, `0x4adfed`) is `ERR_MAIL_CONJURED_ITEM`; a container holding an item is
    /// `ERR_MAIL_BAG` (`0x4ae042`). A refused item stays on the cursor.
    #[test]
    fn the_send_slot_refuses_bound_conjured_zoned_and_filled_bag_items() {
        use crate::script::cursor::{CursorItem, CursorPayload};
        use crate::script::ItemTemplateView;
        // `(bound, filled seats, record as (flags, bonding, area, map))`, and what the click shows.
        type Case = (bool, Option<u32>, Option<(u32, u32, u32, u32)>);
        let attach = |(bound, seats, record): Case| -> (Vec<&'static str>, bool) {
            let mut s = kept();
            s.set_mail(Some(MailState {
                open: true,
                ..Default::default()
            }));
            if let Some((flags, bonding, area, map)) = record {
                s.set_item_template(
                    2589,
                    ItemTemplateView {
                        flags,
                        bonding,
                        area,
                        map,
                        ..Default::default()
                    },
                );
            }
            let slot = crate::script::ContainerSlot {
                item_id: 2589,
                guid: 0x102,
                count: 1,
                already_bound: bound,
                occupied_seats: seats,
                ..Default::default()
            };
            s.set_container(
                0,
                Some(crate::script::ContainerState {
                    name: None,
                    num_slots: 16,
                    slots: [(5, slot)].into_iter().collect(),
                }),
            );
            s.model_mut().cursor = Some(CursorPayload::Item(CursorItem {
                item_class: None,
                guid: 0x102,
                bar_placeable: false,
                consumable: false,
                bag: 0,
                slot: 5,
                item_id: 2589,
                texture: None,
                link: None,
                count: Some(1),
                quality: Some(1),
                equip_slots: Vec::new(),
            }));
            s.run("ClickSendMailItemButton()").unwrap();
            let attached = s.model_ref().mail_send_item.is_some();
            (s.take_ui_errors(), attached)
        };
        let refused = |key| (vec![key], false);
        assert_eq!(
            attach((true, None, Some((0, 4, 0, 0)))),
            refused("ERR_MAIL_QUEST_ITEM")
        );
        assert_eq!(
            attach((true, None, Some((0, 5, 0, 0)))),
            refused("ERR_MAIL_QUEST_ITEM")
        );
        assert_eq!(
            attach((true, None, Some((0, 1, 0, 0)))),
            refused("ERR_MAIL_BOUND_ITEM")
        );
        assert_eq!(
            attach((true, None, None)),
            refused("ERR_MAIL_BOUND_ITEM"),
            "an uncached record is no quest item (`0x4adf6d`)"
        );
        assert_eq!(
            attach((true, None, Some((0x2, 1, 0, 0)))),
            refused("ERR_MAIL_BOUND_ITEM"),
            "bound is asked first"
        );
        let conjured = "ERR_MAIL_CONJURED_ITEM";
        assert_eq!(
            attach((false, None, Some((0x2, 0, 0, 0)))),
            refused(conjured)
        );
        assert_eq!(
            attach((false, None, Some((0, 0, 12, 0)))),
            refused(conjured)
        );
        assert_eq!(
            attach((false, None, Some((0, 0, 0, 33)))),
            refused(conjured)
        );
        assert_eq!(
            attach((false, Some(1), Some((0, 0, 0, 0)))),
            refused("ERR_MAIL_BAG")
        );
        assert_eq!(
            attach((false, Some(0), Some((0, 0, 0, 0)))),
            (Vec::new(), true)
        );
        assert_eq!(
            attach((false, None, None)),
            (Vec::new(), true),
            "nothing known refuses"
        );
    }
}

#[cfg(test)]
mod stationery_tests {
    use super::tests::{kept, sends};
    use super::StationeryView;
    use crate::script::UiScript;

    fn seated() -> UiScript {
        let mut s = kept();
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
        assert!(sends(&mut s).is_none(), "no selection: silently nothing");
        s.run("SelectStationery(1) SendMail(\"Bob\", \"hi\", \"body\")")
            .unwrap();
        assert_eq!(sends(&mut s).map(|r| r.stationery), Some(41));
        s.mail_request_resolved();
        s.reset_compose_tab_silently();
        s.run("SendMail(\"Bob\", \"hi\", \"body\")").unwrap();
        assert!(sends(&mut s).is_none());
    }
}
