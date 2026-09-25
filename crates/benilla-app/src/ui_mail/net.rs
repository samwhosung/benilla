//! The mailbox's packet handlers: they fill [`MailOpen`] and [`MailPending`] and send the wire
//! re-syncs; the events fire from [`super::feed_mail`].

use benilla_protocol::messages::{mail_action, mail_error, mail_message_type, MailListEntry};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{mail_refusal, MailOpen, MailPending, MailSendAck};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_items::{EquipError, EquipErrors};

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::MailList, on_mail_list)
        .net_handler(K::SendMailResult, on_send_mail_result)
        .net_handler(K::MailItemText, on_mail_item_text)
        .net_handler(K::ReceivedMail, on_received_mail)
        .net_handler(K::NextMailTime, on_next_mail_time)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_mail_list(In(ev): In<SessionEvent>, mut mail: ResMut<MailOpen>, commands: Res<NetCommands>) {
    if let SessionEvent::MailList { mails } = ev {
        mail_list(mails, &mut mail, &commands);
    }
}

fn on_send_mail_result(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    commands: Res<NetCommands>,
    mut equip_errors: ResMut<EquipErrors>,
) {
    if let SessionEvent::SendMailResult {
        mail_id,
        action,
        error,
        equip_error,
        item,
    } = ev
    {
        send_mail_result(
            mail_id,
            action,
            error,
            equip_error,
            item,
            &mut mail,
            &commands,
            &mut equip_errors,
        );
    }
}

fn on_mail_item_text(In(ev): In<SessionEvent>, mut mail: ResMut<MailOpen>) {
    if let SessionEvent::MailItemText { text_id, text } = ev {
        mail_item_text(text_id, text, &mut mail);
    }
}

fn on_received_mail(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mail: Res<MailOpen>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::ReceivedMail { seconds } = ev {
        received_mail(seconds, &mut pending, &mail, &commands);
    }
}

fn on_next_mail_time(In(ev): In<SessionEvent>, mut pending: ResMut<MailPending>) {
    if let SessionEvent::NextMailTime { seconds } = ev {
        next_mail_time(seconds, &mut pending);
    }
}

/// The open mailbox and the arrival countdown end with the session; the next world enter
/// re-queries.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
) {
    mail.clear_session();
    *pending = MailPending::default();
}

/// `SMSG_MAIL_LIST_RESULT` replaces the rows; an expired row (`expire_days <= 0`) is deleted with
/// `CMSG_MAIL_DELETE` and dropped, as the handler `0x4ad1b0` does. It never touches
/// [`MailPending`]: nothing on the inbox path writes the countdown `0x845eac`.
fn mail_list(mails: Vec<MailListEntry>, mail: &mut MailOpen, commands: &NetCommands) {
    let mailbox = mail.mailbox;
    mail.mails = mails
        .into_iter()
        .filter(|e| {
            if e.expire_days <= 0.0 {
                if let Some(mailbox) = mailbox {
                    let _ = commands.0.send(ClientCommand::MailDelete {
                        mailbox,
                        mail_id: e.message_id,
                    });
                }
                false
            } else {
                true
            }
        })
        .collect();
}

/// `MAIL_CHECK_MASK_COD_PAYMENT` (`byte[rec+0x148] & 8`): the money a COD taker paid; taking it
/// empties the mail (`0x4ad6b0`).
const CHECKED_COD_PAYMENT: u32 = 8;

/// Whether a take leaves the mail empty, which the client then deletes itself: `CMSG_MAIL_DELETE`,
/// then `CLOSE_INBOX_ITEM(index)`, then `MAIL_INBOX_UPDATE`. The money leg (`0x4ad6b0`) purges a
/// COD payment or an auction notice with no item. The item leg (`0x4ad7b0`) purges once the money
/// is gone and the mail is an auction notice or has neither a text id (`[rec+0x114]`) nor a mail
/// template id (`[rec+0x25c]`); this reads the text id alone, so it also purges a template mail
/// the reference keeps.
fn take_empties(entry: &MailListEntry, action: u32) -> bool {
    let auction = entry.message_type == mail_message_type::AUCTION;
    match action {
        mail_action::MONEY_TAKEN => {
            entry.checked & CHECKED_COD_PAYMENT != 0 || (auction && entry.item.is_none())
        }
        mail_action::ITEM_TAKEN => entry.money == 0 && (auction || entry.item_text_id == 0),
        _ => false,
    }
}

/// `SMSG_SEND_MAIL_RESULT`: a `SEND` result is queued for the feed; a successful take updates the
/// row and purges it when empty, and every successful take, return or delete re-lists the inbox.
fn send_mail_result(
    mail_id: u32,
    action: u32,
    error: u32,
    equip_error: Option<u32>,
    _item: Option<(u32, u32)>,
    mail: &mut MailOpen,
    commands: &NetCommands,
    equip_errors: &mut EquipErrors,
) {
    // ITEM_TAKEN's `(entry, count)` tail is unused; vmangos sends no `SMSG_ITEM_PUSH_RESULT` for a
    // mail take (`MailHandler.cpp:675`).
    if action == mail_action::SEND {
        if error == mail_error::EQUIP_ERROR {
            equip_errors.0.push(EquipError {
                reason: equip_error.unwrap_or(0) as u8,
                required_level: None,
                // The result carries no bag slot: 255 is the wire's "own array" sentinel, so
                // reason 16 names no container.
                bag_slot: 255,
            });
        }
        mail.send_acks.push(MailSendAck {
            ok: error == mail_error::OK,
            refusal: (error != mail_error::OK && error != mail_error::EQUIP_ERROR)
                .then(|| mail_refusal(error)),
        });
        return;
    }
    // A take/return/delete result.
    match error {
        mail_error::OK => {
            // The take clears the local row first (`[rec+0x140]` money, `[rec+0x120]` item), then
            // an emptied mail is purged: `CMSG_MAIL_DELETE`, then the feed's `CLOSE_INBOX_ITEM`.
            if let Some(pos) = mail.mails.iter().position(|e| e.message_id == mail_id) {
                match action {
                    mail_action::MONEY_TAKEN => mail.mails[pos].money = 0,
                    mail_action::ITEM_TAKEN => mail.mails[pos].item = None,
                    _ => {}
                }
                if take_empties(&mail.mails[pos], action) {
                    if let Some(mailbox) = mail.mailbox {
                        let _ = commands
                            .0
                            .send(ClientCommand::MailDelete { mailbox, mail_id });
                    }
                    mail.close_inbox.push(pos as u32 + 1);
                    mail.mails.remove(pos);
                }
            }
            // Re-sync the inbox: the taken money/item or removed row is gone server-side.
            if let Some(mailbox) = mail.mailbox {
                let _ = commands.0.send(ClientCommand::GetMailList { mailbox });
            }
        }
        mail_error::EQUIP_ERROR => equip_errors.0.push(EquipError {
            reason: equip_error.unwrap_or(0) as u8,
            required_level: None,
            bag_slot: 255,
        }),
        other => mail.errors.push(mail_refusal(other)),
    }
}

/// `SMSG_ITEM_TEXT_QUERY_RESPONSE`: the body lands in the cache and the feed repaints.
fn mail_item_text(text_id: u32, text: String, mail: &mut MailOpen) {
    mail.bodies.insert(text_id, Some(text));
}

/// `SMSG_RECEIVED_MAIL` (`0x4ad620`): with a mailbox open it arms the deferred refresh instead of
/// moving the icon.
///
/// Deviation: an open mailbox also re-lists at once, past `CheckInbox`'s 60 s throttle, because a
/// mail announced while the player stands there should show; the reference waits for the close.
fn received_mail(seconds: f32, pending: &mut MailPending, mail: &MailOpen, commands: &NetCommands) {
    pending.apply_received_mail(seconds, mail.mailbox.is_some());
    if let Some(mailbox) = mail.mailbox {
        let _ = commands.0.send(ClientCommand::GetMailList { mailbox });
    }
}

/// The `MSG_QUERY_NEXT_MAIL_TIME` reply: stored as sent, and `UPDATE_PENDING_MAIL` always fires
/// (`0x4ad5f0`, `0x4ad605`).
fn next_mail_time(seconds: f32, pending: &mut MailPending) {
    pending.apply_query_reply(seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_routes_the_arrival_and_the_session_end_to_the_mailbox() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<EquipErrors>()
            .insert_resource(NetCommands(tx));
        register(&mut app);

        app.world_mut().resource_mut::<MailOpen>().click(0x20);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::NextMailTime { seconds: 0.0 }],
        );
        assert!(app.world().resource::<MailPending>().has_new_mail());

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );
        assert!(!app.world().resource::<MailPending>().has_new_mail());
        assert_eq!(app.world().resource::<MailOpen>().mailbox, None);
    }
}
