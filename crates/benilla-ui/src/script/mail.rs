//! The mail bindings (decision 0544 P1/P2) — the Era-shaped mailbox surface, the same two-way seam
//! as [`super::merchant`]/[`super::loot`]: the app pushes a **mail snapshot**
//! ([`UiScript::set_mail`] — the inbox rows already resolved from the wire to
//! name/icon/sender/body by the app's item-template + name caches) and the Lua
//! `CheckInbox`/`TakeInboxItem`/`SendMail`/… calls queue outbound **intents** the app drains. The
//! engine holds no mail knowledge — a row is a header (sender/subject/money/COD/expiry/flags), an
//! optional enclosed item (id/name/icon/quality), an ask-once letter body, and a stationery
//! basename.
//!
//! ## The 5875 API shape (VERIFIED against the extracted `MailFrame.lua`)
//!
//! `index` is **1-based** everywhere; an out-of-range index answers `nil`/`0`. The inbox getters:
//! `GetInboxNumItems()`, `GetInboxHeaderInfo(index)` → the reference 13-tuple
//! `packageIcon, stationeryIcon, sender, subject, money, CODAmount, daysLeft, hasItem(count|nil),
//! wasRead, wasReturned, textCreated, canReply, isGM` (MailFrame.lua l.105/265/279),
//! `GetInboxText(index)` → `body, stationeryTexture, isTakeable, isInvoice` (l.292 — a body cache
//! miss queues the `CMSG_ITEM_TEXT_QUERY` fetch and returns `""`), `GetInboxItem(index)` →
//! `name, texture, count, quality, canUse` (l.379), `InboxItemCanDelete(index)` → 1/nil (the
//! delete-vs-return law, l.417/450). `HasNewMail()` reads the app-pushed login-scoped flag
//! ([`UiScript::set_has_new_mail`], decision 0544 P3) — `nil` (false) until the app's first
//! `MSG_QUERY_NEXT_MAIL_TIME` answer lands.
//!
//! The send tab: `GetSendMailItem()` → `name, texture, stackCount, quality` off the attached cursor
//! item (l.511), `GetSendMailPrice()` → the flat **30c** postage (vmangos's fee), `SendMail(target,
//! subject, body)` fires the send, `SetSendMailMoney`/`SetSendMailCOD(copper)` push the money/COD
//! amounts the app reads at send time, `ClickSendMailItemButton()` attaches (or detaches) the
//! cursor's held bag item as the send attachment (l.719). Intents `CheckInbox`, `TakeInboxItem`,
//! `TakeInboxMoney`, `DeleteInboxItem`, `ReturnInboxItem`, `CloseMail` are 1-based row indices /
//! flags the app maps to the wire.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{self, CursorPayload};
use super::Model;

/// One inbox row, resolved by the app from a wire `MailListEntry` (decision 0544). Plain data — its
/// 1-based order in the window is its position in [`MailState::inbox`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailInboxRow {
    /// The enclosed item's icon (`GetInboxHeaderInfo`'s `packageIcon`); `None` when the mail carries
    /// no item, or while its template answer is in flight.
    pub package_icon: Option<String>,
    /// The letter icon (`stationeryIcon`) — the app derives it from the stationery id/GM flag; the
    /// row shows this when there's no package (or it's GM mail).
    pub stationery_icon: Option<String>,
    /// The sender name (`sender`), resolved through the name cache; `None` while in flight or for a
    /// non-player sender (the Lua shows `UNKNOWN`).
    pub sender: Option<String>,
    pub subject: String,
    pub money: u32,
    pub cod: u32,
    /// Days remaining before the mail is deleted/returned (`daysLeft`, the wire `expire_days`).
    pub days_left: f32,
    /// The enclosed item's stack count (`GetInboxHeaderInfo`'s `hasItem` returns this, or `nil` when
    /// `0`). `0` = no item.
    pub item_count: u32,
    /// `checked & READ(0x1)`.
    pub was_read: bool,
    /// `checked & RETURNED(0x2)`.
    pub was_returned: bool,
    /// `checked & COPIED(0x4)` — the letter body was materialized as a text item (`textCreated`).
    pub text_created: bool,
    /// `sender_guid.is_some() && !was_returned` — the Reply button gate (l.281).
    pub can_reply: bool,
    /// `stationery == 61` — a GM mail (l.108).
    pub is_gm: bool,
    /// The letter body (`GetInboxText`'s first return); `None` = not fetched yet (the getter returns
    /// `""` and queues the ask-once `CMSG_ITEM_TEXT_QUERY` when [`Self::has_body`]).
    pub body: Option<String>,
    /// The stationery texture basename (`GetInboxText`'s second return) — the Lua wraps it as
    /// `Interface\Stationery\<basename>1`/`2` for the open-letter backdrop.
    pub stationery_texture: String,
    /// `GetInboxText`'s fourth return. **Narrower than "an auction mail"**: the reference answers
    /// `1` iff the record carries the three fields its subject parser persisted (`[rec+0x250] != 0`,
    /// `0x4af2eb`), and the parser persists them *only* for result code 1 (won) or 2 (sold). An
    /// outbid or expiry notice is an auction mail that is **not** an invoice (decision 1527).
    pub is_invoice: bool,
    /// The auction invoice this mail carries, once it can be answered — `GetInboxInvoiceInfo`'s
    /// seven values. `None` is the reference's **miss tail**, and it covers four different states
    /// that all read the same from Lua: not an auction mail at all; an auction mail whose result
    /// code is not won(1)/sold(2) (an outbid or expiry notice carries no body and no invoice); the
    /// body not fetched yet; or the counterparty's name still in flight.
    pub invoice: Option<MailInvoice>,
    /// The mail carries a fetchable letter body (`item_text_id != 0`) — gates the body ask.
    pub has_body: bool,
    /// The enclosed item's template entry (`0` = none) — the shared item-tooltip store's key
    /// (`GameTooltip:SetInboxItem`) and `GetInboxItem`'s identity.
    pub item_id: u32,
    /// The enclosed item's **random-suffix roll** (`SMSG_MAIL_LIST`'s per-item `randomPropId`) —
    /// what the hover resolves its enchant lines from, and the id whose suffix
    /// [`Self::item_name`] already carries. `0` = unrolled. The reference's `SetInboxItem` writes
    /// exactly this into the tooltip's `+0x424` (decision 1547).
    pub item_random_property_id: u32,
    /// The enclosed item's name (`GetInboxItem`'s first return); `None` while the template is in
    /// flight.
    pub item_name: Option<String>,
    /// The enclosed item's icon; `None` while in flight.
    pub item_texture: Option<String>,
    /// The enclosed item's quality (0..6); `None` while in flight.
    pub item_quality: Option<u32>,
    /// `InboxItemCanDelete` — the delete-vs-return law (`!can_reply`: a not-yet-returned mail from a
    /// player is returnable, everything else is deletable — l.417/450).
    pub can_delete: bool,
}

/// One auction mail's invoice — what `GetInboxInvoiceInfo(index)` hands back, already parsed.
///
/// The invoice is **TEXT**, not wire data: the auction house writes the numbers into the mail's
/// subject and body and the client `sscanf`s them back out (`GetInboxInvoiceInfo 0x4af360`).
/// The app owns that parse; this is its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailInvoice {
    /// `true` → the literal token `"seller"` (your auction sold), `false` → `"buyer"` (you won).
    /// A bare ASCII token in the reference too, not a GlobalString — the Lua compares it as one.
    pub seller: bool,
    /// The auctioned item's name, composed from the subject's item entry.
    pub item_name: String,
    /// The counterparty: who bought it (seller invoice) or who sold it (buyer invoice).
    pub player_name: String,
    /// The winning bid. `bid == buyout` is how the window knows it was a buyout rather than a bid.
    pub bid: u32,
    pub buyout: u32,
    /// Seller invoices only (`0` on a buyer invoice — the body carries three fields, not five).
    pub deposit: u32,
    /// The auction house's cut, seller invoices only.
    pub consignment: u32,
}

/// The open mailbox's inbox snapshot. Pushed whole by the app; `None` means no mailbox is open (the
/// window is closed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailState {
    pub inbox: Vec<MailInboxRow>,
}

/// One usable stationery, as the send tab's picker lists it (`GetNumStationeries` /
/// `GetStationeryInfo`, 1970) — the app computes the usable set off `Stationery.dbc` and the
/// player's bags, sorted by price, and pushes it whole ([`super::UiScript::set_mail_stationeries`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationeryView {
    /// The `Stationery.dbc` row id — what `SelectStationery` stores and `CMSG_SEND_MAIL` carries.
    pub id: u32,
    /// The stationery item's name (`GetStationeryInfo`'s first return).
    pub name: String,
    /// The item's icon as a FULL path (`Interface\Icons\…`) — `GetStationeryInfo`'s second
    /// return is used raw by `SetTexture` (MailFrame.lua l.671), unlike the bare basename below.
    pub icon: String,
    /// The item's BuyPrice in copper, or `None` when the player already carries the item
    /// (`GetStationeryInfo`'s third return is nil then).
    pub cost: Option<u32>,
    /// The bare texture basename (`GetSelectedStationeryTexture`), wrapped by the Lua as
    /// `STATIONERY_PATH..texture.."1"/"2"`.
    pub texture: String,
}

/// A drained `SendMail` intent — the app resolves the attachment `(bag, slot)` to an item guid and
/// builds `CMSG_SEND_MAIL` from this (decision 0544 P2).
#[derive(Clone, Debug, PartialEq)]
pub struct MailSendRequest {
    pub target: String,
    pub subject: String,
    pub body: String,
    /// The selected `Stationery.dbc` id — `CMSG_SEND_MAIL`'s sixth field (never 0 here: a send with
    /// no selection aborts before it is queued, `0x4ae8dd`).
    pub stationery: u32,
    /// Money to enclose (copper) — `SetSendMailMoney`'s last value.
    pub money: u32,
    /// COD amount (copper) — `SetSendMailCOD`'s last value; `0` when not a COD send.
    pub cod: u32,
    /// The attached bag item's `(live-API bag, 1-based slot)`; `None` = no attachment. The app
    /// resolves it to the wire's `u64 itemGuid` at send time (the reference re-reads the bag slot
    /// when the send fires, so the resolve is lazy).
    pub item: Option<(i64, u32)>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open mailbox's inbox snapshot.
    pub fn set_mail(&mut self, state: Option<MailState>) {
        self.model_mut().mail = state;
    }

    /// Whether `CheckInbox` was called since the last drain (and clear the flag) — the app maps it to
    /// a fresh `CMSG_GET_MAIL_LIST`.
    pub fn take_mail_check_inbox(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_check_inbox)
    }

    /// Drain the 1-based indices of mails **opened** (`GetInboxText` was called on them) since the
    /// last drain — the app marks each read and fetches its body ask-once.
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

    /// Drain the 1-based `TakeInboxTextItem` row picks — the letter button's "permanent copy"
    /// verb; the app maps each to `CMSG_MAIL_CREATE_TEXT_ITEM`.
    pub fn take_mail_take_texts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().mail_take_texts)
    }

    /// Whether `CloseMail` was called since the last drain (and clear the flag). vanilla's
    /// client-side close sends no packet — the app clears its local mail session.
    pub fn take_mail_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().mail_close)
    }

    /// Drain the `SendMail` intent, if one was queued since the last drain — folds in the last
    /// `SetSendMailMoney`/`SetSendMailCOD` amounts and the attached item's `(bag, slot)`. The
    /// attachment is left in place (a failed send keeps it; [`Self::reset_compose_tab`] drops it
    /// on success).
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

    /// **`0x4acdc0(1)` — the client's compose-tab reset**, whole. Zeroes the send tab's attachment
    /// (`0xb6ef90/94`), money (`0xb6efa4`) and COD (`0xb6efa8`) globals and then **tail-fires its
    /// three events**, in this order: `SEND_MAIL_MONEY_CHANGED`, `SEND_MAIL_COD_CHANGED`,
    /// `MAIL_SEND_SUCCESS` (`@0x4ace14/1e/28`).
    ///
    /// **The fire belongs to the reset, and that is the whole point of this shape.** Both call
    /// sites used to fire `MAIL_SEND_SUCCESS` themselves, and the send-result one fired it
    /// *before* clearing — so the stock `SendMailFrame_Reset` ran while `GetSendMailItem()` still
    /// answered with the item that had just been sent, and its own `SendMailFrame_Update()` tail
    /// put the item's name straight back into the subject box and its texture back on
    /// `SendMailPackageButton`. Nothing re-ran the update after the clear landed, so a sent letter
    /// left its subject and its icon sitting in the form (director's report, decision 2145).
    ///
    /// `MAIL_SEND_SUCCESS` is **overloaded** — it means "the compose form is now clean", not "a
    /// mail was sent" — opening a mailbox fires it too.
    pub fn reset_compose_tab(&mut self) {
        {
            let mut model = self.model_mut();
            model.mail_send_item = None;
            model.mail_send_money = 0;
            model.mail_send_cod = 0;
        }
        // Fired here, immediately, rather than queued on `pending_events`: this is a host-side
        // edge (the mail system driving the VM), not a Lua binding queueing work for the next
        // tick, and its order against the caller's own `MAIL_SHOW`/`MAIL_FAILED` is the law.
        // Spelled out one call each — the three are a fixed sequence at three addresses, and the
        // chain-file event census (`ui_script::reference_ui`) reads producers as literals.
        self.fire_event("SEND_MAIL_MONEY_CHANGED", Vec::new()); // 0x4ace14
        self.fire_event("SEND_MAIL_COD_CHANGED", Vec::new()); // 0x4ace1e
        self.fire_event("MAIL_SEND_SUCCESS", Vec::new()); // 0x4ace28
    }

    /// Drop the attachment alone — `SendMail`'s attached-item-gone abort (`ERR_ITEM_NOT_FOUND`, no
    /// packet, `MAIL_SEND_INFO_UPDATE` at `0x4ae98d`): the bag slot no longer holds the item the
    /// send tab shows. The money and COD amounts stay, as the reference leaves them.
    pub fn drop_send_mail_item(&mut self) {
        let mut model = self.model_mut();
        if model.mail_send_item.take().is_some() {
            model
                .pending_events
                .push(("MAIL_SEND_INFO_UPDATE".to_string(), Vec::new()));
        }
    }

    /// Push the usable stationery list, in the picker's order (1970; see [`StationeryView`]).
    pub fn set_mail_stationeries(&mut self, list: Vec<StationeryView>) {
        self.model_mut().mail_stationeries = list;
    }

    /// Clear the stationery selection — the client does it at `0x4ace07` on BOTH opening and
    /// closing the mailbox, which is why the stock `SendMailFrame_Reset` re-selects row 1 on show.
    pub fn clear_stationery(&mut self) {
        self.model_mut().mail_stationery = 0;
    }

    /// Push `HasNewMail()`'s answer (decision 0544 P3) — login-scoped, independent of whether a
    /// mailbox window is open. The app computes this off its `MSG_QUERY_NEXT_MAIL_TIME`/
    /// `SMSG_RECEIVED_MAIL`-fed countdown and calls this on every value change, right before firing
    /// `UPDATE_PENDING_MAIL` (the reference `MiniMapMailFrame`'s only listened event, Minimap.xml
    /// l.278-289 — its `OnEvent` just re-reads `HasNewMail()` and shows/hides).
    pub fn set_has_new_mail(&mut self, has: bool) {
        self.model_mut().has_new_mail = has;
    }
}

/// Fetch a cloned inbox row for a 1-based index, or `None` (out of range / no mailbox open).
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

    // GetInboxNumItems() → the number of inbox rows (0 when no mailbox is open).
    g.set(
        "GetInboxNumItems",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.mail.as_ref().map_or(0, |m| m.inbox.len()) as i64)
        })?,
    )?;

    // GetInboxHeaderInfo(index) → the 13-tuple (see the module doc). An out-of-range index answers
    // nil (the reference callers only read within GetInboxNumItems, but the null shape is faithful).
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
            // packageIcon shows only for a non-GM mail that carries an item (MailFrame.lua l.108).
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
                // hasItem: the stack count, or nil when there's no item.
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

    // GetInboxText(index) → body, stationeryTexture, isTakeable, isInvoice (MailFrame.lua l.292).
    // Opening a mail is the ask-once trigger: queue the open intent (the app marks it read + fetches
    // the body when has_body). A cache miss returns "" until the body lands and a fresh snapshot
    // arrives. isTakeable = the mail carries a fetchable body (`item_text_id != 0`) — with
    // `not textCreated` it gates the letter button (the "permanent copy" verb, ref l.366; vmangos
    // `HandleMailCreateTextItem` requires the same pair: an `itemTextId` and no COPIED bit).
    g.set(
        "GetInboxText",
        lua.create_function(|lua, index: usize| {
            let row = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let row = row_at(&model, index);
                if row.is_some() {
                    // Ask-once dedup: don't re-queue an already-pending open this drain window.
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
                // **An invoice's body is `nil`, and that is an explicit carve-out in the reference,
                // not an accident** (`0x4af1cf cmp [esi+0x4],2` / `je` → `lua_pushnil`, decision
                // 1527). The auction house's raw `<guid>:<n>:<n>:…` bookkeeping really does sit in
                // the shared text cache — `GetInboxInvoiceInfo` `sscanf`s exactly that string — but
                // this binding refuses to hand it back. Two bindings, one cache, opposite purposes.
                // It is what lets MailFrame lay the invoice pane over the letter page without the
                // bookkeeping showing through above it.
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

    // GetInboxInvoiceInfo(index) → invoiceType, itemName, playerName, bid, buyout, deposit,
    // consignment (MailFrame.lua l.302). **SEVEN values, always** — the reference's own `mov eax,7`
    // (`0x4af5a1`), with the miss tail `nil, nil, nil, 0, 0, 0, 0` (`0x4af559`): three nils then
    // four zeros. MailFrame.lua guards on the third
    // (`if playerName then`), so the shape of the miss is what keeps the invoice pane hidden —
    // returning one bare nil instead would leave the four numeric destructures nil and the pane's
    // arithmetic would run on them.
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

    // GetInboxItem(index) → name, texture, count, quality, canUse (MailFrame.lua l.379). name/quality
    // are nil while the item-template answer is in flight; a mail with no item answers all-nil name.
    // canUse is the shared item-usable gate over the enclosed item's template (merchant's isUsable
    // path) — usable while the template is still in flight (the null-record skip).
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
            // **Five values on every path**, and the empty leg is `(nil, nil, 0, 0, nil)` —
            // `GetInboxItem 0x4af5d0`, the sibling of the `GetSendMailItem` block above (decision
            // 2129). This answered ONE value, which a caller
            // destructuring five reads as four nils — and the reference's own row
            // (`(nil,nil,number,number,nil) | …`) has no one-value alternative at all.
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
            // A number on every path, like the send tab's.
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

    // InboxItemCanDelete(index) → 1/nil (MailFrame.lua l.417/450): the delete-vs-return law.
    g.set(
        "InboxItemCanDelete",
        lua.create_function(|lua, index: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(row_at(&model, index).is_some_and(|r| r.can_delete)))
        })?,
    )?;

    // HasNewMail() → 1/nil off the app's login-scoped countdown flag (decision 0544 P3; see
    // `UiScript::set_has_new_mail`'s doc for the wire behind it).
    g.set(
        "HasNewMail",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.has_new_mail))
        })?,
    )?;

    // CheckInbox() — flag the inbox-refresh intent (the app sends CMSG_GET_MAIL_LIST).
    g.set(
        "CheckInbox",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .mail_check_inbox = true;
            Ok(())
        })?,
    )?;

    // The five 1-based row-pick intents (take item / take money / delete / return / copy letter).
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

    // CloseMail() — client-side close (no packet): flag it so the app clears its mail session.
    g.set(
        "CloseMail",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .mail_close = true;
            Ok(())
        })?,
    )?;

    // SendMail(target, subject, body) — queue the send; the app folds in money/COD + the attachment.
    g.set(
        "SendMail",
        lua.create_function(|lua, (target, subject, body): (String, String, String)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `SendMail 0x4ae800` (decision 1965): money and COD are exclusive AT SEND TIME — both
            // set is a silent abort, no packet — and a COD with no attached item aborts the same
            // way. 0 returns on every path, so Lua cannot tell.
            if model.mail_send_money != 0 && model.mail_send_cod != 0 {
                return Ok(());
            }
            if model.mail_send_cod != 0 && model.mail_send_item.is_none() {
                return Ok(());
            }
            // No stationery selected — `0x4ae8dd je`: the send silently aborts, no packet, 0
            // returns (1970). The selection is cleared on mailbox open and close, and the stock
            // `SendMailFrame_Reset` re-selects row 1, so a send from the stock tab always has one.
            if model.mail_stationery == 0 {
                return Ok(());
            }
            model.mail_send = Some((target, subject, body));
            Ok(())
        })?,
    )?;

    // SetSendMailMoney(copper) → 1 (the SEND_MONEY popup's OnAccept gates SendMail on the truthy
    // return, StaticPopup.lua l.252). Stores the enclose-money amount the app reads at send time.
    // SetSendMailMoney(copper) — `0x4ae0f0` → `0x4adbe0` (decision 1965): a non-number RAISES;
    // more than the purse (unsigned) shows ERR_NOT_ENOUGH_MONEY and answers nil; else the store,
    // SEND_MAIL_MONEY_CHANGED, and the number 1 — StaticPopup.lua l.257 branches on that value.
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

    // GetSendMailMoney() / GetSendMailCOD() → the two amounts as stored (`0x4ae150` / `0x4ae1c0`,
    // one number each — `reference/1.12-shapes.tsv`): what the stock money kit's SEND_MAIL and
    // SEND_MAIL_COD types display (1962).
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

    // SetSendMailCOD(copper) — store the COD amount the app reads at send time (MailFrame.lua l.497).
    // SetSendMailCOD(copper) — `0x4ae180` → `0x4adc70` (decision 1965): a non-number RAISES;
    // without an attached item nothing happens at all — no store, no event, no message — and
    // there is no affordability or sign check; 0 returns on every path.
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

    // GetSendMailItem() → name, texture, stackCount, quality off the attached cursor item
    // (MailFrame.lua l.511).
    //
    // **The empty leg is `(nil, nil, 0, 0)`** — four values, and slots 3 and 4 are NUMBERS. Read at
    // `0x4ae590`: `0x4ae6d3`/`0x4ae6da` push nil, then two `push 0; push 0; lua_pushnumber` pairs
    // at `0x4ae6df`/`0x4ae6ea`, `eax = 4` (decision 2129). All three of the reference's failure
    // guards share that one block, so "nothing attached" and "the item template has not loaded
    // yet" are indistinguishable to a script.
    //
    // The `1` this used to push in slot 3 was borrowed from the wrong binding: it is
    // `GetAuctionSellItemInfo 0x4ce590`'s empty leg (`1.0`/`-1.0`), not this one's.
    //
    // **Still not faithful, and stated rather than implied:** on the LOADED leg the reference reads
    // quality from `[rec+0x1c]` gated on `[rec+0x2c] != 0` = InventoryType, so a **non-equippable**
    // item answers quality `-1` (`0x4ae6bf`). We do not carry an inventory type here, so an item
    // whose quality we have not cached answers 0 rather than -1.
    g.set(
        "GetSendMailItem",
        lua.create_function(|lua, ()| {
            // The stack count is read while the model is borrowed: a whole-stack pickup records
            // no count of its own, so the true size comes back from the source slot.
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
            // A number on every path — the reference's row has no nil alternative in this slot.
            let quality = Value::Integer(i64::from(it.quality.unwrap_or(0)));
            Ok(MultiValue::from_vec(vec![
                name,
                texture,
                Value::Integer(i64::from(count)),
                quality,
            ]))
        })?,
    )?;

    // GetSendMailPrice() → the flat 30c postage (vmangos's fee; the reference reads a C constant).
    g.set(
        "GetSendMailPrice",
        lua.create_function(|_, ()| Ok(Value::Integer(30)))?,
    )?;

    // ClickSendMailItemButton() — attach the cursor's held bag item as the send attachment, or, with
    // an empty cursor and a filled slot, pick the attachment back onto the cursor (MailFrame.lua
    // l.719, mirroring pickup_container_item's swap). A spell/action cursor is refused (no-op).
    g.set(
        "ClickSendMailItemButton",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            click_send_mail_item(&mut model);
            Ok(())
        })?,
    )?;

    // ── The stationery family (1970) ──
    // The list is the app's (`Stationery.dbc` × the player's bags × the template cache); the
    // client rebuilds it from `GetNumStationeries` (`0x4ae202` → `0x4ad970`), on world enter and
    // on the last item-query answer — the app's per-frame recompute covers all three moments.

    // `GetNumStationeries()` — 0 args, 1 number: the usable count.
    g.set(
        "GetNumStationeries",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.mail_stationeries.len() as i64)
        })?,
    )?;

    // `GetStationeryInfo(index)` — shape A (`lua_isnumber`, truncate), raising its Usage on a
    // non-number; 1-based, unsigned bound; exactly 3 returns on every exit: name, the FULL icon
    // path, BuyPrice in copper — cost nil when the player carries the item; 3 nils out of range.
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

    // `SelectStationery(index)` — shape A, raising its Usage on a non-number; 0 returns. In range
    // it stores the row's DBC id; **out of range is not an error — it writes 0**, a deselect.
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

    // `GetSelectedStationeryTexture()` — 0 args, 1 return: the BARE `Stationery.dbc` texture name
    // of the selection; nil for no selection (id 0) or an id the table does not carry.
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

/// The attach/detach swap of `ClickSendMailItemButton` — the mail send slot as a cursor drop
/// target (decision 0216's payload rails). Every `model.cursor` mutation is followed by
/// [`cursor::queue_cursor_update`]/[`cursor::queue_lock_changed`] so `CURSOR_UPDATE`/
/// `ITEM_LOCK_CHANGED` fire, exactly like a bag slot.
fn click_send_mail_item(model: &mut Model) {
    match model.cursor.take() {
        // Cursor holds a bag item → attach it (the item leaves nothing in the bag until send).
        Some(CursorPayload::Item(item)) => {
            let (bag, slot) = (item.bag, item.slot);
            // If a different item was already attached, it returns to nowhere client-side (the
            // reference simply overwrites); its slot was never locked, so nothing to unlock.
            model.mail_send_item = Some(item);
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, bag, slot);
            // The attachment changed — `MAIL_SEND_INFO_UPDATE` at `0x4ae0de` (0 args, 1970); the
            // stock tab re-reads `GetSendMailItem` and the postage on it.
            model
                .pending_events
                .push(("MAIL_SEND_INFO_UPDATE".to_string(), Vec::new()));
        }
        // Empty cursor, a filled slot → pick the attachment back onto the cursor (detach).
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
        // A spell/action payload is refused — put it back untouched (container.rs's refuse arm).
        Some(other) => {
            model.cursor = Some(other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A send needs a stationery selected (1970): seat the default and select it, the way the
    /// stock `SendMailFrame_Reset` does on MAIL_SHOW.
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
            can_delete: false, // from a player, not returned → returnable (not deletable)
            item_random_property_id: 0,
        }
    }

    fn state() -> MailState {
        MailState {
            inbox: vec![row(2589), row(0)],
        }
    }

    /// `GetInboxInvoiceInfo` answers **seven values, always** — the reference's own `mov eax,7`.
    /// The miss tail is the part that matters and the part that is easy to get wrong: three nils
    /// then four ZEROS, not one bare nil. MailFrame.lua destructures all seven unguarded and only
    /// then tests the third, so a short return leaves its arithmetic running on nils.
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

        // Row 2 carries no invoice — and neither does an index off the end.
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

    /// `GetInboxText` returns **four** values always, and for an INVOICE the first is `nil`.
    /// That is the reference's own carve-out (`0x4af1cf cmp [esi+0x4],2` → `lua_pushnil`), and it
    /// is what lets MailFrame lay the invoice pane over the letter page: the auction house's raw
    /// bookkeeping is in the text cache — `GetInboxInvoiceInfo` parses exactly that string — and
    /// this binding refuses to hand it back (decision 1527).
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
        // Row 2 is an ordinary letter and keeps its text.
        assert_eq!(
            s.eval::<String>("return tostring((GetInboxText(2)))")
                .unwrap(),
            "Lok'tar."
        );
    }

    /// A BUYER invoice says so with the bare ASCII token the Lua compares against — not a
    /// GlobalString, not localized.
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

        // Row 1 (has item): the leading strings + money/COD/days, then the flags — asserted in two
        // smaller evals so the harness never destructures the whole 13-tuple in one Rust type.
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
        // hasItem = count 3, wasRead nil, wasReturned nil, textCreated nil, canReply 1, isGM nil.
        assert!(s
            .eval::<bool>(
                "local a,b,c,d,e,f,g, has, read, ret, tc, reply, gm = GetInboxHeaderInfo(1)\n\
                 return has == 3 and read == nil and ret == nil and tc == nil and reply == 1 and gm == nil",
            )
            .unwrap());

        // Row 2 (no item): hasItem nil, packageIcon nil.
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
        assert_eq!(takeable, Value::Integer(1)); // has a fetchable body
        assert!(matches!(invoice, Value::Nil));
        // Opening queued the row for the mark-read/body ask; deduped within the drain window.
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
        // Row 2 has no item → nil.
        assert!(s.eval::<bool>("return GetInboxItem(2) == nil").unwrap());
        // Row 1 is from a player, not returned → returnable, NOT deletable.
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
        // The purse gate (1965): more than the purse answers nil and names the refusal.
        assert!(s
            .eval::<bool>("return SetSendMailMoney(9999) == nil")
            .unwrap());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);
        assert!(s
            .eval::<bool>("return SetSendMailMoney(1234) == 1")
            .unwrap());
        // A COD with no attached item is completely silent: no store, no event (1965).
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
        // Nothing attached → nil name, stack 1.
        assert!(s
            .eval::<bool>("local n = GetSendMailItem()\nreturn n == nil")
            .unwrap());

        // Put a bag item on the cursor, then click the send slot: it attaches.
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
        // Cursor is now empty; the send item reads the attachment.
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
        let (name, _tex, count, quality): (String, String, i64, i64) =
            s.eval("return GetSendMailItem()").unwrap();
        assert_eq!((name.as_str(), count, quality), ("Linen Cloth", 7, 1));

        // A send folds in the attachment's (bag, slot).
        select_default_stationery(&mut s);
        s.run("SendMail('Alt', 'stuff', '')").unwrap();
        assert_eq!(s.take_mail_send().unwrap().item, Some((0, 5)));

        // Clicking again with an empty cursor detaches (back onto the cursor).
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

    /// The four verbs, shape by shape (`GetNumStationeries 0x4ae1f0`, `GetStationeryInfo 0x4ae230`,
    /// `SelectStationery 0x4ae380`, `GetSelectedStationeryTexture 0x4ae3f0`): the count; the info
    /// triple with a nil cost for a carried paper and three nils past the end; a numeric string
    /// passes the number gate and a non-number raises the Usage; the selection stores the DBC id,
    /// out of range deselects, and the texture is the bare basename or nil.
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

    /// A send with no stationery selected aborts silently — no request, no error; with one
    /// selected the request carries the DBC id. Opening/closing the mailbox clears it.
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
