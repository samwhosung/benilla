//! The mail verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each packet with the mailbox cell as it stands
//! (`[0xb6ef88]`), and `GetInboxText`'s read mark and body ask, before the verb returns, as the
//! reference's bindings send in their own bodies (`0x4aeab0`, `0x4af110`, `0x4af7d0`, `0x4af8e0`,
//! `0x4afa60`, `0x4afba0`, `0x4afcd0`, `0x4ae800`).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{MailCall, MailRowVerb, UiScript};

use super::{MailOpen, MailPending};
use crate::net::{ClientCommand, NetCommands};

/// The state the mail verbs read and write: the session's rows and body cache, the deferred
/// refresh, and the socket.
#[derive(SystemParam)]
pub(crate) struct MailVerbs<'w> {
    /// Absent in a world with no mail session, which drops the calls.
    mail: Option<ResMut<'w, MailOpen>>,
    pending: Option<ResMut<'w, MailPending>>,
    commands: Res<'w, NetCommands>,
    /// The profanity mask a read letter's text runs through (`0x4af203`).
    filter: Option<Res<'w, crate::text_filter::TextFilter>>,
    switches: Option<Res<'w, crate::text_filter::TextFilterSwitches>>,
}

impl MailVerbs<'_> {
    /// Apply one mail call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: MailCall) {
        let Self {
            mail,
            pending,
            commands,
            filter,
            switches,
        } = self;
        let Some(mail) = mail.as_deref_mut() else {
            debug!("ui_mail: no mail session in this world; {call:?} dropped");
            return;
        };
        match call {
            MailCall::Open {
                index,
                mail_id,
                unread,
            } => {
                // The mark runs with no test of the mailbox, which a close leaves 0 (`0x4add25`).
                if unread {
                    let mailbox = mail.mailbox.unwrap_or(0);
                    let _ = commands
                        .0
                        .send(ClientCommand::MailMarkAsRead { mailbox, mail_id });
                    mail.mark_read(mail_id);
                    script.mark_mail_read(index);
                    // Only an unread mail arms the close's re-query (`0x4adda6`).
                    if let Some(pending) = pending.as_deref_mut() {
                        pending.arm_refresh();
                    }
                }
                return;
            }
            MailCall::ReadBody {
                index,
                mail_id,
                text_id,
            } => {
                mail.bodies.lookup(text_id, || {
                    let _ = commands
                        .0
                        .send(ClientCommand::ItemTextQuery { text_id, mail_id });
                });
                // The cached text, expanded, is masked on every read (`0x4af1f6`, `0x4af203` with
                // `dl` 0), so each read moves the mask's phase; a template's body is not.
                if let (Some(mut body), Some(filter)) =
                    (script.inbox_body(index), filter.as_deref())
                {
                    let enabled = switches.as_deref().is_some_and(|s| s.profanity);
                    filter.mask(enabled, &mut body);
                    script.answer_inbox_body(body);
                }
                return;
            }
            _ => {}
        }
        // The VM's guards found a mailbox the session has since forgotten: no packet goes, so the
        // locks the verb took go back.
        let Some(mailbox) = mail.mailbox else {
            if matches!(call, MailCall::Send(_)) {
                script.mail_send_resolved();
            }
            script.mail_request_resolved();
            return;
        };
        let command = match call {
            MailCall::Open { .. } | MailCall::ReadBody { .. } => return,
            MailCall::CheckInbox => ClientCommand::GetMailList { mailbox },
            MailCall::Row {
                verb,
                mail_id,
                template_id,
            } => match verb {
                MailRowVerb::TakeMoney => ClientCommand::MailTakeMoney { mailbox, mail_id },
                MailRowVerb::TakeItem => ClientCommand::MailTakeItem { mailbox, mail_id },
                MailRowVerb::Return => ClientCommand::MailReturnToSender { mailbox, mail_id },
                MailRowVerb::Delete => ClientCommand::MailDelete { mailbox, mail_id },
                MailRowVerb::TakeText => ClientCommand::MailCreateTextItem {
                    mailbox,
                    mail_id,
                    template_id,
                },
            },
            MailCall::Send(req) => ClientCommand::SendMail {
                mailbox,
                receiver: req.target,
                subject: req.subject,
                body: req.body,
                stationery: req.stationery,
                item_guid: req.item_guid,
                money: req.money,
                cod: req.cod,
                // The package rides only with an item (`0x4ae8f3`).
                package: if req.item_guid != 0 { req.package } else { 0 },
            },
        };
        let _ = commands.0.send(command);
    }
}
