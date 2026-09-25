//! Mail wire messages. There is no `SMSG_SHOW_MAILBOX`: the client opens the window itself. Each
//! mailbox CMSG leads with the mailbox guid, range-checked (5 yd) by vmangos `CheckMailBox`.

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_f32_le, read_u32_le, read_u64_le, read_u8};

/// [`MailListEntry::message_type`]: each value fixes the form of the row's sender field.
pub mod mail_message_type {
    /// A player: the sender field is an 8-byte guid.
    pub const NORMAL: u8 = 0;
    /// An auction-house notice: the sender field is a u32 auction id.
    pub const AUCTION: u8 = 2;
    /// A creature: the sender field is a u32 creature entry.
    pub const CREATURE: u8 = 3;
    /// A GameObject: the sender field is a u32 GameObject entry.
    pub const GAMEOBJECT: u8 = 4;
    /// A pre-made item mail (a GM or starter mail): no sender field at all.
    pub const ITEM: u8 = 5;
}

/// `SMSG_SEND_MAIL_RESULT`'s `action` (vmangos `Mail.h` `MailResponseType`).
pub mod mail_action {
    pub const SEND: u32 = 0;
    pub const MONEY_TAKEN: u32 = 1;
    pub const ITEM_TAKEN: u32 = 2;
    pub const RETURNED: u32 = 3;
    pub const DELETED: u32 = 4;
    pub const MADE_PERMANENT: u32 = 5;
}

/// `SMSG_SEND_MAIL_RESULT`'s `error` (vmangos `Mail.h` `MailResponseResult`).
pub mod mail_error {
    pub const OK: u32 = 0;
    pub const EQUIP_ERROR: u32 = 1;
    pub const CANNOT_SEND_TO_SELF: u32 = 2;
    pub const NOT_ENOUGH_MONEY: u32 = 3;
    pub const RECIPIENT_NOT_FOUND: u32 = 4;
    pub const NOT_YOUR_TEAM: u32 = 5;
    pub const INTERNAL_ERROR: u32 = 6;
    pub const TRIAL_ACCOUNT: u32 = 14;
    pub const TOO_MANY_ATTACHMENTS: u32 = 15;
}

/// A mail's one attachment (`MAX_MAIL_ITEMS` is 1 in 1.12): a fixed-width block present on every
/// `SMSG_MAIL_LIST_RESULT` row, all zero when there is no item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailAttachment {
    pub entry: u32,
    pub perm_enchant: u32,
    pub random_prop_id: u32,
    pub suffix_factor: u32,
    pub count: u8,
    pub charges: u32,
    pub durability_max: u32,
    pub durability: u32,
}

/// One `SMSG_MAIL_LIST_RESULT` row (vmangos `HandleGetMailList`, `Handlers/MailHandler.cpp`).
/// `sender_guid` and `sender_id` are one wire field, whose form `message_type` picks.
#[derive(Debug, Clone, PartialEq)]
pub struct MailListEntry {
    pub message_id: u32,
    pub message_type: u8,
    pub sender_guid: Option<u64>,
    pub sender_id: Option<u32>,
    pub subject: String,
    /// `0` = no letter body to fetch; nonzero = a [`super::item_text_query`] key.
    pub item_text_id: u32,
    pub stationery: u32,
    pub item: Option<MailAttachment>,
    pub money: u32,
    pub cod: u32,
    /// vmangos `Mail.h` flags: READ 0x1, RETURNED 0x2, COPIED 0x4, COD_PAYMENT 0x8, HAS_BODY 0x10.
    pub checked: u32,
    pub expire_days: f32,
    pub mail_template_id: u32,
}

/// `CMSG_GET_MAIL_LIST` (vmangos `Server/Packets/Mail.cpp`): the mailbox guid.
pub fn get_mail_list(mailbox: u64) -> Vec<u8> {
    mailbox.to_le_bytes().to_vec()
}

/// `CMSG_SEND_MAIL` (vmangos `Server/Packets/Mail.cpp`): mailbox guid, receiver, subject and body
/// cstrings, `u32` stationery and package, item guid, `u32` money and COD, then 9 zero bytes
/// vmangos skips. It ignores stationery and package, storing `MAIL_STATIONERY_DEFAULT` (41).
pub fn send_mail(
    mailbox: u64,
    receiver: &str,
    subject: &str,
    body: &str,
    stationery: u32,
    package: u32,
    item_guid: u64,
    money: u32,
    cod: u32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        8 + receiver.len() + 1 + subject.len() + 1 + body.len() + 1 + 4 + 4 + 8 + 4 + 4 + 9,
    );
    out.extend_from_slice(&mailbox.to_le_bytes());
    out.extend_from_slice(receiver.as_bytes());
    out.push(0);
    out.extend_from_slice(subject.as_bytes());
    out.push(0);
    out.extend_from_slice(body.as_bytes());
    out.push(0);
    out.extend_from_slice(&stationery.to_le_bytes());
    out.extend_from_slice(&package.to_le_bytes());
    out.extend_from_slice(&item_guid.to_le_bytes());
    out.extend_from_slice(&money.to_le_bytes());
    out.extend_from_slice(&cod.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.push(0);
    out
}

fn mailbox_and_mail_id(mailbox: u64, mail_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&mailbox.to_le_bytes());
    body.extend_from_slice(&mail_id.to_le_bytes());
    body
}

/// `CMSG_MAIL_TAKE_MONEY`: answered by a [`mail_action::MONEY_TAKEN`] result.
pub fn mail_take_money(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_TAKE_ITEM`: answered by an [`mail_action::ITEM_TAKEN`] result naming the item.
pub fn mail_take_item(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_MARK_AS_READ`: there is no reply; the caller sets the `checked` READ bit itself.
pub fn mail_mark_as_read(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_RETURN_TO_SENDER`: answered by a [`mail_action::RETURNED`] result.
pub fn mail_return_to_sender(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_DELETE`: answered by a [`mail_action::DELETED`] result.
pub fn mail_delete(mailbox: u64, mail_id: u32) -> Vec<u8> {
    mailbox_and_mail_id(mailbox, mail_id)
}

/// `CMSG_MAIL_CREATE_TEXT_ITEM`: mailbox, mail id and a `u32` template id, 0 for player mail.
/// Answered by a [`mail_action::MADE_PERMANENT`] result; on OK the mail's COPIED bit is set.
pub fn mail_create_text_item(mailbox: u64, mail_id: u32) -> Vec<u8> {
    let mut body = mailbox_and_mail_id(mailbox, mail_id);
    body.extend_from_slice(&0u32.to_le_bytes()); // mailTemplateId
    body
}

/// `CMSG_ITEM_TEXT_QUERY` (vmangos `Server/Packets/Mail.cpp`): `u32 textId, u32 mailId, u32 unk`.
/// The 1.12 client writes all three; vmangos only uses `textId`.
pub fn item_text_query(text_id: u32, mail_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&text_id.to_le_bytes());
    body.extend_from_slice(&mail_id.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // the unk vmangos never reads back
    body
}

/// `SMSG_MAIL_LIST_RESULT` (vmangos `HandleGetMailList`): `u8 count`, then [`MailListEntry`] rows.
pub(super) fn read_mail_list_result(r: &mut &[u8]) -> io::Result<Vec<MailListEntry>> {
    let count = read_u8(r)?;
    // vmangos stops the list at 254 (`MailHandler.cpp:765`, `mailsCount >= 254`).
    let mut mails = Vec::with_capacity(capacity_hint(count, 254));
    for _ in 0..count {
        let message_id = read_u32_le(r)?;
        let message_type = read_u8(r)?;
        let (sender_guid, sender_id) = match message_type {
            mail_message_type::NORMAL => (Some(read_u64_le(r)?), None),
            mail_message_type::AUCTION
            | mail_message_type::CREATURE
            | mail_message_type::GAMEOBJECT => (None, Some(read_u32_le(r)?)),
            // MAIL_ITEM (5) and anything else unrecognized: no sender bytes at all.
            _ => (None, None),
        };
        let subject = read_cstring(r)?;
        let item_text_id = read_u32_le(r)?;
        let _package = read_u32_le(r)?; // always 0 (vmangos hardcodes it)
        let stationery = read_u32_le(r)?;
        let entry = read_u32_le(r)?;
        let perm_enchant = read_u32_le(r)?;
        let random_prop_id = read_u32_le(r)?;
        let suffix_factor = read_u32_le(r)?;
        let count_stack = read_u8(r)?;
        let charges = read_u32_le(r)?;
        let durability_max = read_u32_le(r)?;
        let durability = read_u32_le(r)?;
        let item = (entry != 0).then_some(MailAttachment {
            entry,
            perm_enchant,
            random_prop_id,
            suffix_factor,
            count: count_stack,
            charges,
            durability_max,
            durability,
        });
        let money = read_u32_le(r)?;
        let cod = read_u32_le(r)?;
        let checked = read_u32_le(r)?;
        let expire_days = read_f32_le(r)?;
        let mail_template_id = read_u32_le(r)?;
        mails.push(MailListEntry {
            message_id,
            message_type,
            sender_guid,
            sender_id,
            subject,
            item_text_id,
            stationery,
            item,
            money,
            cod,
            checked,
            expire_days,
            mail_template_id,
        });
    }
    Ok(mails)
}

/// `SMSG_SEND_MAIL_RESULT` (vmangos `Server/Packets/Mail.cpp`): `u32 mailId, action, error`, then
/// `u32 equipError` on `EQUIP_ERROR`, or `u32 itemEntry, itemCount` on an OK `ITEM_TAKEN`.
#[allow(clippy::type_complexity)]
pub(super) fn read_send_mail_result(
    r: &mut &[u8],
) -> io::Result<(u32, u32, u32, Option<u32>, Option<(u32, u32)>)> {
    let mail_id = read_u32_le(r)?;
    let action = read_u32_le(r)?;
    let error = read_u32_le(r)?;
    let mut equip_error = None;
    let mut item = None;
    if error == mail_error::EQUIP_ERROR && !r.is_empty() {
        equip_error = Some(read_u32_le(r)?);
    } else if action == mail_action::ITEM_TAKEN && error == mail_error::OK && !r.is_empty() {
        item = Some((read_u32_le(r)?, read_u32_le(r)?));
    }
    Ok((mail_id, action, error, equip_error, item))
}

/// `SMSG_ITEM_TEXT_QUERY_RESPONSE` (vmangos `Server/Packets/Mail.cpp`): `u32 textId, cstr text`.
pub(super) fn read_item_text_query_response(r: &mut &[u8]) -> io::Result<(u32, String)> {
    Ok((read_u32_le(r)?, read_cstring(r)?))
}

/// `SMSG_RECEIVED_MAIL`: an `f32` delay in seconds until the mail shows as waiting; the 1.12
/// client reads a float (`0x4ad620`). vmangos writes `uint32(0)` (`SendNewMail`), bitwise `0.0`.
pub(super) fn read_received_mail(r: &mut &[u8]) -> io::Result<f32> {
    read_f32_le(r)
}

/// The `MSG_QUERY_NEXT_MAIL_TIME` reply (the request is an empty body on the same opcode): an
/// `f32`, `0.0` for unread mail waiting and `-86400.0` for none.
pub(super) fn read_query_next_mail_time(r: &mut &[u8]) -> io::Result<f32> {
    read_f32_le(r)
}
