//! The mailbox's packet handlers (decision 0544 P1/P2/P3; in the net handler table since 2306) —
//! the inbox/body/send-result kinds fill the [`MailOpen`] session the feed reads, the arrival
//! pair feeds [`MailPending`] (`HasNewMail()`/the minimap icon). The `UiScript` events these
//! ultimately drive are fired by [`super::feed_mail`] (the feed owns the VM), so nothing here
//! touches it: the handlers only mutate resources + send the wire re-syncs.

use benilla_protocol::messages::{mail_action, mail_error, mail_message_type, MailListEntry};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{mail_refusal, MailOpen, MailPending, MailSendAck};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_items::{EquipError, EquipErrors};

/// Register the mailbox's handlers — called from [`super::UiMailPlugin`]. One per kind, plus the
/// session-end listener.
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

/// An open mailbox dies with the socket, and the arrival countdown is login-scoped (decision
/// 0544 P3): a fresh login re-queries `MSG_QUERY_NEXT_MAIL_TIME` at world-enter, so nothing
/// carries over across a reconnect. A listener on the session end (a second handler on
/// `Disconnected`, after the bridge's own teardown, `net::session::on_disconnected`).
fn on_session_end(
    In(_): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
) {
    mail.clear_session();
    *pending = MailPending::default();
}

/// `SessionEvent::MailList` (`SMSG_MAIL_LIST_RESULT`) — replace the session's rows + fire the inbox
/// repaint (via the feed's diff). The inbox handler auto-purges expired mail: any row whose timer ran
/// out (`expire_days <= 0`) is deleted server-side (`CMSG_MAIL_DELETE`) and dropped here
/// (`0x4ad1b0`).
///
/// **It does not touch [`MailPending`]** — and that is a positive fact, not an
/// omission. This arm used to clear the countdown when the surviving list had
/// nothing unread, on the inferred grounds that "checking your mail clears the icon" had to be the
/// list's doing. A full write-xref of the countdown float `0x845eac` says otherwise: nothing on the
/// inbox path writes it. The icon clears because **opening a letter arms the deferred-refresh flag
/// and the mailbox *close* re-asks the server** — modelled in [`super`], where the close
/// edge lives.
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

/// `MAIL_CHECK_MASK_COD_PAYMENT` — the `checked` bit (`byte[rec+0x148] & 8`) that marks a mail as
/// the money a COD taker paid; taking that money empties the mail (`0x4ad6b0`, 1970).
const CHECKED_COD_PAYMENT: u32 = 8;

/// The two take legs' "this mail is now empty" decision, off the client's own bytes
/// (1970): a take that empties the mail sends `CMSG_MAIL_DELETE`
/// itself, then `CLOSE_INBOX_ITEM(index)`, then `MAIL_INBOX_UPDATE`. The money leg (`0x4ad6b0`)
/// purges iff the mail is a COD payment, or an auction notice with no item; the item leg
/// (`0x4ad7b0`) purges iff the money is gone and the mail is an auction notice or carries no
/// letter text. A plain letter you took the money from stays open, husk and all — that is the
/// reference too (the stock `OpenMailFrame_OnHide` deletes a copied, emptied letter on close).
///
/// The item leg's second conjunct reads `[rec+0x114] == 0 && [rec+0x25c] == 0`; `+0x114` is the
/// letter's text id and `+0x25c` a second no-text field (inferred to be the
/// fetched body). This takes the text id alone, which can only purge a mail the client also would
/// when that second field is zero whenever the first is.
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

/// `SessionEvent::SendMailResult` (`SMSG_SEND_MAIL_RESULT`) — route per action/error (decision 0544
/// P2). action == SEND queues a [`MailSendAck`] for the feed (MAIL_FAILED always, MAIL_SEND_SUCCESS
/// on OK, else the red error line). A successful take applies to the local row and, when it
/// empties the mail, deletes it the way the client does ([`take_empties`]); every successful
/// take/return/delete then re-syncs the inbox with a fresh `CMSG_GET_MAIL_LIST` (the reference
/// client's inbox-refresh moment); an EQUIP_ERROR routes to the existing inventory-error surface;
/// any other failure surfaces the red error line.
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
    // ITEM_TAKEN's OK tail carries (entry, count) for a "received" line; vmangos does NOT also send
    // SMSG_ITEM_PUSH_RESULT for a mail take, and no mail-received-line precedent exists yet, so we do
    // nothing extra with it here (the GetMailList re-sync below reflects the emptied row). [_item]
    if action == mail_action::SEND {
        if error == mail_error::EQUIP_ERROR {
            equip_errors.0.push(EquipError {
                reason: equip_error.unwrap_or(0) as u8,
                required_level: None,
                // SMSG_SEND_MAIL_RESULT carries only the code, never a bag slot. 255 is the
                // wire's own "the player's own array" sentinel — reason 16's substitution
                // correctly declines to name a container it was never told about.
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
            // The take landed on the local row first (the client clears `[rec+0x140]` /
            // `[rec+0x120]` before deciding), and an emptied mail is purged client-side:
            // `CMSG_MAIL_DELETE`, then `CLOSE_INBOX_ITEM(index)` from the feed (1970).
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

/// `SessionEvent::MailItemText` (`SMSG_ITEM_TEXT_QUERY_RESPONSE`) — land the letter body in the
/// ask-once cache + clear its pending flag; the feed repaints (MAIL_INBOX_UPDATE) on the change.
fn mail_item_text(text_id: u32, text: String, mail: &mut MailOpen) {
    mail.bodies.insert(text_id, Some(text));
}

/// `SessionEvent::ReceivedMail` (`SMSG_RECEIVED_MAIL`) — mail just arrived. `seconds` is the wire's
/// delay float (vmangos always sends `0.0` = "now"); it runs the countdown's set-value ladder,
/// which takes the **busy** branch when a mailbox window is open — arming the deferred refresh
/// instead of moving the icon under the player's nose (`0x4ad620`).
///
/// The list re-sync is ours, not the reference's, and stays: a server push bypasses `CheckInbox`'s
/// 60 s client-side throttle (decision 0544 P3), so a mail arriving while you stand at the mailbox
/// shows up. The reference reaches the same place by its close-time re-query; leaving a mail you
/// were just told about invisible for up to a minute is the worse client, and this costs one packet.
fn received_mail(seconds: f32, pending: &mut MailPending, mail: &MailOpen, commands: &NetCommands) {
    pending.apply_received_mail(seconds, mail.mailbox.is_some());
    if let Some(mailbox) = mail.mailbox {
        let _ = commands.0.send(ClientCommand::GetMailList { mailbox });
    }
}

/// `SessionEvent::NextMailTime` (`MSG_QUERY_NEXT_MAIL_TIME`'s reply, one `f32`) — store the
/// server's float verbatim and signal `UPDATE_PENDING_MAIL` **unconditionally** (`0x4ad5f0`,
/// signal site `0x4ad605`). `0.0` = mail waiting now, negative (vmangos always sends
/// `-86400.0`) = none, a positive value counts down per frame in `crate::ui_mail`'s `feed_mail` and
/// flips `HasNewMail()` true as it lands inside ε.
fn next_mail_time(seconds: f32, pending: &mut MailPending) {
    pending.apply_query_reply(seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End to end through the real registration: the wire's "mail is waiting" reaches the
    /// countdown, a mailbox click survives until the session ends, and the session end — the
    /// bridge's own teardown beside this listener — resets both.
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
