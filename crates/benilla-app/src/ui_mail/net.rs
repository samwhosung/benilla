//! The mailbox's packet handlers: they fill [`MailOpen`] and [`MailPending`], send the wire
//! requests and queue their signals on the mail lane ([`MailLane`]) inside themselves, the name,
//! creature, game object and text answers' callbacks included.

use benilla_protocol::messages::{mail_action, mail_error, mail_message_type, MailListEntry};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{
    mail_refusal, MailEvent, MailLane, MailOpen, MailPending, MailResult, MailWait,
    CHECKED_COD_PAYMENT, CHECKED_COPIED, CHECKED_RETURNED, MAIL_SENT_KEY,
};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::query_cache::Ask;

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::MailList, on_mail_list)
        .net_handler(K::SendMailResult, on_send_mail_result)
        .net_handler(K::MailItemText, on_mail_item_text)
        .net_handler(K::ReceivedMail, on_received_mail)
        .net_handler(K::NextMailTime, on_next_mail_time)
        .net_handler(K::PlayerName, on_player_name)
        .net_handler(K::CreatureName, on_creature_name)
        .net_handler(K::GameObjectInfo, on_object_answer)
        .net_handler(K::GameObjectUnknown, on_object_answer)
        .net_handler(K::Disconnected, on_session_end);
}

/// The name cache's answer runs the getters' callbacks waiting on the guid (`0x4af080`,
/// `0x4af5b0`), a refusal with status 0, which signals nothing ([`MailOpen::answered`]).
fn on_player_name(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
) {
    if let SessionEvent::PlayerName { guid, name, .. } = ev {
        mail.answered(MailWait::Player(guid), !name.is_empty(), &mut lane);
    }
}

/// The creature cache's answer runs the sender callbacks waiting on its entry (`0x4af080`).
fn on_creature_name(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
) {
    if let SessionEvent::CreatureName { entry, name, .. } = ev {
        mail.answered(MailWait::Creature(entry), name.is_some(), &mut lane);
    }
}

/// The game object cache's answer, or its refusal, runs the sender callbacks waiting on its entry
/// (`0x4af080`).
fn on_object_answer(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
) {
    let (entry, found) = match ev {
        SessionEvent::GameObjectInfo { entry, .. } => (entry, true),
        SessionEvent::GameObjectUnknown { entry } => (entry, false),
        _ => return,
    };
    mail.answered(MailWait::Object(entry), found, &mut lane);
}

/// What the list handler reads beside the session: the papers it defaults a row to, the item
/// templates and GlobalStrings an auction notice's subject is written from, and the profanity
/// mask it runs on each subject.
#[derive(bevy::ecs::system::SystemParam)]
struct ListReads<'w> {
    stationery: Option<Res<'w, super::Stationery>>,
    usable: Option<Res<'w, super::UsableStationery>>,
    items: Res<'w, crate::items::Items>,
    filter: Option<Res<'w, crate::text_filter::TextFilter>>,
    switches: Option<Res<'w, crate::text_filter::TextFilterSwitches>>,
    script: Option<NonSend<'w, benilla_ui::script::UiScript>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
}

fn on_mail_list(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
    commands: Res<NetCommands>,
    social: Res<crate::ui_social::SocialState>,
    reads: ListReads,
) {
    if let SessionEvent::MailList { mails } = ev {
        // The usable list's first paper as last rebuilt (`[[0xb6ee30]]`, `0x4ad559`-`0x4ad56d`).
        let first_paper = reads
            .stationery
            .as_ref()
            .and(reads.usable.as_deref())
            .and_then(|u| u.list().first())
            .map(|v| v.id);
        let enabled = reads.switches.as_deref().is_some_and(|s| s.profanity);
        let get_text = |key: &str| {
            reads
                .script
                .as_deref()
                .and_then(|s| crate::ui_chat::combat::global_string(s, key))
        };
        mail_list(
            mails,
            &mut mail,
            &commands,
            |guid| social.is_ignored(guid),
            first_paper,
            |raw| {
                let written = super::auction_subject(
                    raw,
                    &reads.items,
                    reads.props.as_deref(),
                    Ask::Event(&commands),
                    &get_text,
                );
                // A notice whose item the cache misses keeps the callback `0x4ad010`, which
                // rewrites the subject and signals when the record lands (`0x4acf06`).
                if let Some(notice) = super::invoice::parse_subject(raw) {
                    if notice.entry != 0
                        && reads.items.template(notice.entry, 0, Ask::Read).is_none()
                    {
                        reads
                            .items
                            .wait(notice.entry, crate::items::RecordCallback::Mail);
                    }
                }
                written
            },
            |subject| {
                if let Some(filter) = reads.filter.as_deref() {
                    filter.mask(enabled, subject);
                }
            },
        );
        // The parse's tail: the lock and `MAIL_INBOX_UPDATE` (`0x4ad5c3`-`0x4ad5d6`).
        lane.push(MailEvent::List);
    }
}

fn on_send_mail_result(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::SendMailResult {
        mail_id,
        action,
        error,
        equip_error,
        item,
    } = ev
    {
        let result = send_mail_result(
            mail_id,
            action,
            error,
            equip_error,
            item,
            &mut mail,
            &commands,
        );
        lane.push(MailEvent::Result(result));
    }
}

/// The text cache's answer: the body lands, then the callbacks `GetInboxText` kept on it signal
/// (`0x4af340`).
fn on_mail_item_text(
    In(ev): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut lane: ResMut<MailLane>,
) {
    if let SessionEvent::MailItemText { text_id, text } = ev {
        mail_item_text(text_id, text, &mut mail);
        mail.answered(MailWait::Body(text_id), true, &mut lane);
    }
}

fn on_received_mail(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mut lane: ResMut<MailLane>,
    mail: Res<MailOpen>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::ReceivedMail { seconds } = ev {
        if received_mail(seconds, &mut pending, &mail, &commands) {
            lane.push(MailEvent::Pending);
        }
    }
}

fn on_next_mail_time(
    In(ev): In<SessionEvent>,
    mut pending: ResMut<MailPending>,
    mut lane: ResMut<MailLane>,
) {
    if let SessionEvent::NextMailTime { seconds } = ev {
        next_mail_time(seconds, &mut pending);
        lane.push(MailEvent::Pending);
    }
}

/// The open mailbox and the arrival countdown end with the session; the next world enter
/// re-queries.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut mail: ResMut<MailOpen>,
    mut pending: ResMut<MailPending>,
) {
    mail.clear_disconnected();
    *pending = MailPending::default();
}

/// `SMSG_MAIL_LIST_RESULT` (`0x4ad1b0`) replaces the rows, whatever their days left: the handler
/// reads them (`0x4ad414`) and never tests them. A player's mail from an ignored sender
/// (`0x4ad47f`) that is neither returned nor a COD payment (`0x4ad496`) is answered at once and
/// dropped (`0x4ad51d`): `CMSG_MAIL_RETURN_TO_SENDER` when it holds an item or money (`0x4ad4bf`,
/// `0x4ad4c7`), else `CMSG_MAIL_DELETE` (`0x4ad4cf`), with the mailbox cell as it stands, 0 when
/// none is open (`0x4ad4e3`). Every other row with no positive stationery id takes `first_paper`,
/// the usable list's first (`0x4ad551`-`0x4ad56f`). An auction notice's subject is written by
/// `rewrite` when its item is held, else emptied (`0x4ad580`-`0x4ad5a0`, `0x4acf3d`), and every
/// row's shown subject is then
/// masked in list order (`0x4ad5b2`, `dl` 0); a notice keeps its raw triplet beside it
/// ([`super::ListedSubject`]). It never touches [`MailPending`]: nothing on the inbox path writes
/// the countdown `0x845eac`.
///
/// Deviation: with no usable paper the reference reads the first record of an empty list through
/// a null pointer (`0x4ad561`-`0x4ad56d`, `mov eax,[eax]` with `eax` 0) and faults; benilla keeps
/// the row's wire id, so the inbox still opens.
pub(super) fn mail_list(
    mails: Vec<MailListEntry>,
    mail: &mut MailOpen,
    commands: &NetCommands,
    is_ignored: impl Fn(u64) -> bool,
    first_paper: Option<u32>,
    mut rewrite: impl FnMut(&str) -> Option<String>,
    mut mask: impl FnMut(&mut String),
) {
    let mailbox = mail.mailbox.unwrap_or(0);
    let mut auction_subjects = std::collections::HashMap::new();
    mail.mails = mails
        .into_iter()
        .filter(|e| {
            let answered = e.message_type == mail_message_type::NORMAL
                && is_ignored(e.sender_guid.unwrap_or(0))
                && e.checked & (CHECKED_RETURNED | CHECKED_COD_PAYMENT) == 0;
            if answered {
                let mail_id = e.message_id;
                let _ = commands.0.send(if e.item.is_some() || e.money != 0 {
                    ClientCommand::MailReturnToSender { mailbox, mail_id }
                } else {
                    ClientCommand::MailDelete { mailbox, mail_id }
                });
            }
            !answered
        })
        .map(|mut e| {
            // The id is read signed (`0x4ad551 cmp [ecx+0x11c],0; jg`).
            if (e.stationery as i32) <= 0 {
                if let Some(id) = first_paper {
                    e.stationery = id;
                }
            }
            if e.message_type == mail_message_type::AUCTION {
                let written = rewrite(&e.subject);
                let rewritten = written.is_some();
                // An item not held, or a subject the parse cannot name, leaves the cell empty
                // (`0x4acf3d`, `0x4acfbd`).
                let mut text = written.unwrap_or_default();
                mask(&mut text);
                auction_subjects.insert(e.message_id, super::ListedSubject { text, rewritten });
            } else {
                mask(&mut e.subject);
            }
            e
        })
        .collect();
    mail.auction_subjects = auction_subjects;
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

/// `SMSG_SEND_MAIL_RESULT` (`0x4ad050`), its signals the returned result's. A successful take,
/// return, delete or copy edits the held rows in place, as its arm does, and owes that arm's own
/// `MAIL_INBOX_UPDATE`: the money leg (`0x4ad6b0`) and the item leg (`0x4ad7b0`) clear
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
) -> MailResult {
    // ITEM_TAKEN's `(entry, count)` tail is unused; vmangos sends no `SMSG_ITEM_PUSH_RESULT` for a
    // mail take (`MailHandler.cpp:675`). Every result lifts the request lock and signals
    // `MAIL_FAILED` at the handler's common tail (`0x4ad164`, `0x4ad15f`), whatever its action.
    let mut result = MailResult::default();
    if action == mail_action::SEND {
        result.send = Some(error == mail_error::OK);
        result.line = match error {
            mail_error::OK => Some(MAIL_SENT_KEY),
            // The equip-error table's line, with no fill: the result carries no bag slot and no
            // level, and unlocks nothing.
            mail_error::EQUIP_ERROR => Some(crate::ui_items::equip_error_key(
                equip_error.unwrap_or(0) as u8,
            )),
            other => Some(mail_refusal(other)),
        };
        return result;
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
        mail_error::EQUIP_ERROR => {
            result.line = Some(crate::ui_items::equip_error_key(
                equip_error.unwrap_or(0) as u8
            ))
        }
        other => result.line = Some(mail_refusal(other)),
    }
    result
}

/// `SMSG_ITEM_TEXT_QUERY_RESPONSE`: the body lands in the cache.
fn mail_item_text(text_id: u32, text: String, mail: &mut MailOpen) {
    mail.bodies.insert(text_id, text);
}

/// `SMSG_RECEIVED_MAIL` (`0x4ad620`): with a mailbox open it arms the deferred refresh instead of
/// moving the icon. Whether it signals `UPDATE_PENDING_MAIL` (`0x4ad66b`).
///
/// Deviation: an open mailbox also re-lists at once, past `CheckInbox`'s 60 s throttle, because a
/// mail announced while the player stands there should show; the reference waits for the close.
/// Its list answer also releases the request lock (`0x4ad5d0`), so a take in flight when it lands
/// loses its lock and a second request can go out before the take's result.
fn received_mail(
    seconds: f32,
    pending: &mut MailPending,
    mail: &MailOpen,
    commands: &NetCommands,
) -> bool {
    let signals = pending.apply_received_mail(seconds, mail.mailbox.is_some());
    if let Some(mailbox) = mail.mailbox {
        let _ = commands.0.send(ClientCommand::GetMailList { mailbox });
    }
    signals
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
            .init_resource::<MailLane>()
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

    /// `0x4ad050` ends every arm at one tail that signals `MAIL_FAILED` (`0x4ad15f`): a send, a
    /// take, a return and a delete, each success and each refusal, once per result and in arrival
    /// order, a purged row's `CLOSE_INBOX_ITEM` (`0x4ad772`) ahead of its own.
    #[test]
    fn every_mail_result_signals_mail_failed_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut mail = MailOpen::default();
        mail.click(0x20);
        let mut cod = super::super::tests::entry(7, Some(0xA), 0, 30.0);
        cod.checked = CHECKED_COD_PAYMENT;
        mail.mails.push(cod);
        let mut results = Vec::new();
        for (action, error) in [
            (mail_action::MONEY_TAKEN, mail_error::OK),
            (mail_action::RETURNED, mail_error::INTERNAL_ERROR),
            (mail_action::DELETED, mail_error::OK),
            (mail_action::ITEM_TAKEN, mail_error::EQUIP_ERROR),
            (mail_action::SEND, mail_error::OK),
        ] {
            results.push(send_mail_result(
                7, action, error, None, None, &mut mail, &commands,
            ));
        }

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
        super::super::announce_results(&mut script, results, |_, _| {});
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
    ) -> (MailResult, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let result = send_mail_result(
            id,
            action,
            mail_error::OK,
            None,
            None,
            mail,
            &NetCommands(tx),
        );
        (result, rx)
    }

    /// The take arms edit the held row in place and ask the server for nothing (`0x4ad6e8`,
    /// `0x4ad7e8`): no `CMSG_GET_MAIL_LIST` follows a take, a return or a delete, and each arm
    /// owes its own `MAIL_INBOX_UPDATE`.
    #[test]
    fn a_take_edits_its_row_in_place_and_asks_for_no_list() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut mail = MailOpen::default();
        mail.click(0x20);
        let mut row = super::super::tests::entry(7, Some(0xA), 0, 30.0);
        row.money = 90;
        row.item = Some(attachment());
        mail.mails.push(row);
        let results: Vec<MailResult> = [
            (mail_action::MONEY_TAKEN, 7),
            (mail_action::ITEM_TAKEN, 7),
            (mail_action::MADE_PERMANENT, 7),
        ]
        .into_iter()
        .map(|(action, id)| {
            send_mail_result(id, action, mail_error::OK, None, None, &mut mail, &commands)
        })
        .collect();
        let row = &mail.mails[0];
        assert_eq!((row.money, row.item.is_none()), (0, true));
        assert_ne!(row.checked & CHECKED_COPIED, 0, "the copy arm sets COPIED");
        assert!(results.iter().all(|r| r.inbox_update));
        // A plain player's mail with no text id: the item leg purges it, once, and keeps the row.
        assert_eq!(sent_commands(&rx), [("delete", 7)], "no list request");
        assert_eq!(results[1].close_inbox, Some(1));
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
            let (first, rx) = result(&mut m, 7, action);
            assert!(sent_commands(&rx).is_empty(), "and asks for nothing");
            assert_eq!(m.mails.len(), 1, "action {action} removes the record");
            assert_eq!(m.mails[0].message_id, 8);
            assert!(first.inbox_update);
            // A mail the rows do not hold ends the arm silently.
            let (missing, _) = result(&mut m, 99, action);
            assert!(!missing.inbox_update);
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
            let result = send_mail_result(
                7,
                mail_action::ITEM_TAKEN,
                mail_error::OK,
                None,
                None,
                &mut mail,
                &NetCommands(tx),
            );
            assert_eq!(
                sent_commands(&rx).is_empty(),
                !purged,
                "template {template} text {text} kind {kind}"
            );
            assert_eq!(result.close_inbox.is_some(), purged);
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
        let result = send_mail_result(
            7,
            mail_action::MONEY_TAKEN,
            mail_error::OK,
            None,
            None,
            &mut mail,
            &NetCommands(tx),
        );
        assert_eq!(sent_commands(&rx), [("delete", 7)]);
        assert_eq!(result.close_inbox, Some(1));
    }
}
