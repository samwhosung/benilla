//! The mailbox's packet handlers: they fill [`MailOpen`] and [`MailPending`] and send the wire
//! requests; the events fire from [`super::feed_mail`].

use benilla_protocol::messages::{mail_action, mail_error, mail_message_type, MailListEntry};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{
    mail_refusal, MailOpen, MailPending, MailResult, CHECKED_COD_PAYMENT, CHECKED_COPIED,
    MAIL_SENT_KEY,
};
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
pub(super) fn mail_list(mails: Vec<MailListEntry>, mail: &mut MailOpen, commands: &NetCommands) {
    // The answer clears the request lock (`0x4ad5d0`).
    mail.resolve_request();
    // The parse ends in `MAIL_INBOX_UPDATE` (`0x4ad5c6`).
    mail.list_results += 1;
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

/// Whether a take leaves the mail empty, which the client then deletes itself: `CMSG_MAIL_DELETE`,
/// then `CLOSE_INBOX_ITEM(index)`. The money leg (`0x4ad6b0`) purges a COD payment or an auction
/// notice with no item. The item leg (`0x4ad7b0`) purges once the money is gone and the mail is an
/// auction notice or has neither a text id (`[rec+0x114]`, `0x4ad7fd`) nor a mail template id
/// (`[rec+0x25c]`, `0x4ad809`), so a quest or event reward mail stays.
fn take_empties(entry: &MailListEntry, action: u32) -> bool {
    let auction = entry.message_type == mail_message_type::AUCTION;
    match action {
        mail_action::MONEY_TAKEN => {
            entry.checked & CHECKED_COD_PAYMENT != 0 || (auction && entry.item.is_none())
        }
        mail_action::ITEM_TAKEN => {
            entry.money == 0
                && (auction || (entry.item_text_id == 0 && entry.mail_template_id == 0))
        }
        _ => false,
    }
}

/// `SMSG_SEND_MAIL_RESULT` (`0x4ad050`). A `SEND` result is queued for the feed. A successful take,
/// return, delete or copy edits the held rows in place, as its arm does, and owes the feed that
/// arm's own `MAIL_INBOX_UPDATE`: the money leg (`0x4ad6b0`) and the item leg (`0x4ad7b0`) clear
/// the field and purge an emptied mail with `CMSG_MAIL_DELETE` and `CLOSE_INBOX_ITEM`, keeping the
/// row until the delete's own result; the return and delete arm (`0x4ad910`) removes the record;
/// the copy arm (`0x4ad8c0`) sets the COPIED bit. A mail the rows do not hold ends the arm
/// silently. Nothing asks the server for the list again.
pub(super) fn send_mail_result(
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
    // mail take (`MailHandler.cpp:675`). Every result lifts the request lock at the handler's
    // common tail (`0x4ad164`), whatever its action.
    mail.resolve_request();
    // Every result owes the feed its tail `MAIL_FAILED`, whatever its action (`0x4ad15f`).
    let mut result = MailResult::default();
    if action == mail_action::SEND {
        if error == mail_error::EQUIP_ERROR {
            equip_errors.0.push(EquipError {
                unlocks: [0, 0],
                reason: equip_error.unwrap_or(0) as u8,
                required_level: None,
                // The result carries no bag slot: 255 is the wire's "own array" sentinel, so
                // reason 16 names no container.
                bag_slot: 255,
            });
        }
        result.send = Some(error == mail_error::OK);
        result.line = match error {
            mail_error::OK => Some(MAIL_SENT_KEY),
            mail_error::EQUIP_ERROR => None,
            other => Some(mail_refusal(other)),
        };
        mail.results.push(result);
        return;
    }
    // A take/return/delete/copy result.
    match error {
        mail_error::OK => {
            if let Some(pos) = mail.mails.iter().position(|e| e.message_id == mail_id) {
                result.inbox_update = true;
                match action {
                    mail_action::MONEY_TAKEN | mail_action::ITEM_TAKEN => {
                        // The take clears the local field first (`[rec+0x140]` money,
                        // `[rec+0x120]` item), then an emptied mail is purged.
                        if action == mail_action::MONEY_TAKEN {
                            mail.mails[pos].money = 0;
                        } else {
                            mail.mails[pos].item = None;
                        }
                        if take_empties(&mail.mails[pos], action) {
                            // Sent whether or not a mailbox is open (`0x4ad74d`); a closed one
                            // has no guid, and the server drops it.
                            let mailbox = mail.mailbox.unwrap_or(0);
                            let _ = commands
                                .0
                                .send(ClientCommand::MailDelete { mailbox, mail_id });
                            result.close_inbox = Some(pos as u32 + 1);
                        }
                    }
                    mail_action::RETURNED | mail_action::DELETED => {
                        mail.mails.remove(pos);
                    }
                    mail_action::MADE_PERMANENT => mail.mails[pos].checked |= CHECKED_COPIED,
                    _ => result.inbox_update = false,
                }
            }
        }
        mail_error::EQUIP_ERROR => equip_errors.0.push(EquipError {
            unlocks: [0, 0],
            reason: equip_error.unwrap_or(0) as u8,
            required_level: None,
            bag_slot: 255,
        }),
        other => result.line = Some(mail_refusal(other)),
    }
    mail.results.push(result);
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
/// Its list answer also releases the request lock (`0x4ad5d0`), so a take in flight when it lands
/// loses its lock and a second request can go out before the take's result.
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

    /// Every `SMSG_SEND_MAIL_RESULT` lifts the request lock at the handler's common tail
    /// (`0x4ad164`): a take, a return and a delete as well as a send, refused or not.
    #[test]
    fn every_mail_result_lifts_the_request_lock() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut equip = EquipErrors::default();
        for (action, error) in [
            (mail_action::SEND, mail_error::OK),
            (mail_action::MONEY_TAKEN, mail_error::OK),
            (mail_action::ITEM_TAKEN, mail_error::INTERNAL_ERROR),
            (mail_action::RETURNED, mail_error::OK),
            (mail_action::MADE_PERMANENT, mail_error::OK),
            (mail_action::DELETED, mail_error::INTERNAL_ERROR),
        ] {
            let mut mail = MailOpen::default();
            mail.click(0x20);
            send_mail_result(
                7, action, error, None, None, &mut mail, &commands, &mut equip,
            );
            assert!(mail.request_resolved, "action {action} error {error}");
        }
    }

    /// `0x4ad050` ends every arm at one tail that signals `MAIL_FAILED` (`0x4ad15f`): a send, a
    /// take, a return and a delete, each success and each refusal, once per result and in arrival
    /// order, a purged row's `CLOSE_INBOX_ITEM` (`0x4ad772`) ahead of its own.
    #[test]
    fn every_mail_result_signals_mail_failed_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut equip = EquipErrors::default();
        let mut mail = MailOpen::default();
        mail.click(0x20);
        let mut cod = super::super::tests::entry(7, Some(0xA), 0, 30.0);
        cod.checked = CHECKED_COD_PAYMENT;
        mail.mails.push(cod);
        for (action, error) in [
            (mail_action::MONEY_TAKEN, mail_error::OK),
            (mail_action::RETURNED, mail_error::INTERNAL_ERROR),
            (mail_action::DELETED, mail_error::OK),
            (mail_action::ITEM_TAKEN, mail_error::EQUIP_ERROR),
            (mail_action::SEND, mail_error::OK),
        ] {
            send_mail_result(
                7, action, error, None, None, &mut mail, &commands, &mut equip,
            );
        }
        assert_eq!(mail.results.len(), 5, "one per result");

        let mut script = benilla_ui::script::UiScript::new().unwrap();
        script
            .run(
                r#"log = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_FAILED")
                f:RegisterEvent("CLOSE_INBOX_ITEM")
                f:RegisterEvent("MAIL_INBOX_UPDATE")
                f:SetScript("OnEvent", function()
                    table.insert(log, event .. (arg1 and (":" .. arg1) or ""))
                end)"#,
            )
            .unwrap();
        super::super::announce_results(&mut script, std::mem::take(&mut mail.results), |_, _| {});
        assert_eq!(
            script
                .eval::<String>("return table.concat(log, ' ')")
                .unwrap(),
            "CLOSE_INBOX_ITEM:1 MAIL_INBOX_UPDATE MAIL_FAILED MAIL_FAILED \
             MAIL_INBOX_UPDATE MAIL_FAILED MAIL_FAILED MAIL_FAILED"
        );
    }

    fn attachment() -> benilla_protocol::messages::MailAttachment {
        benilla_protocol::messages::MailAttachment {
            entry: 2589,
            perm_enchant: 0,
            random_prop_id: 0,
            suffix_factor: 0,
            count: 3,
            charges: 0,
            durability_max: 0,
            durability: 0,
        }
    }

    fn sent_commands(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<(&'static str, u32)> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::MailDelete { mail_id, .. } => ("delete", mail_id),
                ClientCommand::GetMailList { .. } => ("list", 0),
                _ => ("other", 0),
            })
            .collect()
    }

    fn result(
        mail: &mut MailOpen,
        id: u32,
        action: u32,
    ) -> crossbeam_channel::Receiver<ClientCommand> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut equip = EquipErrors::default();
        send_mail_result(
            id,
            action,
            mail_error::OK,
            None,
            None,
            mail,
            &NetCommands(tx),
            &mut equip,
        );
        rx
    }

    /// The take arms edit the held row in place and ask the server for nothing (`0x4ad6e8`,
    /// `0x4ad7e8`): no `CMSG_GET_MAIL_LIST` follows a take, a return or a delete, and each arm
    /// owes its own `MAIL_INBOX_UPDATE`.
    #[test]
    fn a_take_edits_its_row_in_place_and_asks_for_no_list() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut equip = EquipErrors::default();
        let mut mail = MailOpen::default();
        mail.click(0x20);
        let mut row = super::super::tests::entry(7, Some(0xA), 0, 30.0);
        row.money = 90;
        row.item = Some(attachment());
        mail.mails.push(row);
        for (action, id) in [
            (mail_action::MONEY_TAKEN, 7),
            (mail_action::ITEM_TAKEN, 7),
            (mail_action::MADE_PERMANENT, 7),
        ] {
            send_mail_result(
                id,
                action,
                mail_error::OK,
                None,
                None,
                &mut mail,
                &commands,
                &mut equip,
            );
        }
        let row = &mail.mails[0];
        assert_eq!((row.money, row.item.is_none()), (0, true));
        assert_ne!(row.checked & CHECKED_COPIED, 0, "the copy arm sets COPIED");
        assert!(mail.results.iter().all(|r| r.inbox_update));
        // A plain player's mail with no text id: the item leg purges it, once, and keeps the row.
        assert_eq!(sent_commands(&rx), [("delete", 7)], "no list request");
        assert_eq!(mail.results[1].close_inbox, Some(1));
        assert_eq!(
            mail.mails.len(),
            1,
            "the row stays until the delete's own result"
        );

        for action in [mail_action::DELETED, mail_action::RETURNED] {
            let mut m = MailOpen::default();
            m.click(0x20);
            m.mails
                .push(super::super::tests::entry(7, Some(0xA), 0, 30.0));
            m.mails
                .push(super::super::tests::entry(8, Some(0xA), 0, 30.0));
            let rx = result(&mut m, 7, action);
            assert!(sent_commands(&rx).is_empty(), "and asks for nothing");
            assert_eq!(m.mails.len(), 1, "action {action} removes the record");
            assert_eq!(m.mails[0].message_id, 8);
            assert!(m.results[0].inbox_update);
            // A mail the rows do not hold ends the arm silently.
            result(&mut m, 99, action);
            assert!(!m.results[1].inbox_update);
        }
    }

    /// A mail with a mail template id (a quest or event reward) is kept when its item is taken
    /// (`0x4ad809`), as one with a text id is (`0x4ad7fd`); an auction notice still goes.
    #[test]
    fn taking_the_item_of_a_template_mail_keeps_the_letter() {
        let attach = Some(attachment());
        for (template, text, kind, purged) in [
            (1141, 0, mail_message_type::NORMAL, false),
            (0, 55, mail_message_type::NORMAL, false),
            (0, 0, mail_message_type::NORMAL, true),
            (1141, 55, mail_message_type::AUCTION, true),
        ] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut mail = MailOpen::default();
            mail.click(0x20);
            let mut row = super::super::tests::entry(7, None, 0, 30.0);
            row.item = attach;
            row.mail_template_id = template;
            row.item_text_id = text;
            row.message_type = kind;
            mail.mails.push(row);
            let mut equip = EquipErrors::default();
            send_mail_result(
                7,
                mail_action::ITEM_TAKEN,
                mail_error::OK,
                None,
                None,
                &mut mail,
                &NetCommands(tx),
                &mut equip,
            );
            assert_eq!(
                sent_commands(&rx).is_empty(),
                !purged,
                "template {template} text {text} kind {kind}"
            );
            assert_eq!(mail.results[0].close_inbox.is_some(), purged);
        }
    }

    /// A purge sends `CMSG_MAIL_DELETE` whether or not a mailbox is open (`0x4ad74d`).
    #[test]
    fn a_purge_sends_its_delete_with_no_mailbox_open() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut mail = MailOpen::default();
        let mut cod = super::super::tests::entry(7, Some(0xA), 0, 30.0);
        cod.checked = CHECKED_COD_PAYMENT;
        mail.mails.push(cod);
        let mut equip = EquipErrors::default();
        send_mail_result(
            7,
            mail_action::MONEY_TAKEN,
            mail_error::OK,
            None,
            None,
            &mut mail,
            &NetCommands(tx),
            &mut equip,
        );
        assert_eq!(sent_commands(&rx), [("delete", 7)]);
        assert_eq!(mail.results[0].close_inbox, Some(1));
    }
}
