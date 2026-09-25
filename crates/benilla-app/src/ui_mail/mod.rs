//! The mail feed: the app half of [`benilla_ui::script`]'s `mail` module.
//!
//! The mailbox window is client-side, with no `SMSG_SHOW_MAILBOX`: a mailbox right-click opens the
//! session and sends nothing (the MAILBOX use handler `0x5f6820` opens locally, never
//! `CMSG_GAMEOBJ_USE`), and the window's `MAIL_SHOW` handler calls `CheckInbox()`, the first
//! `CMSG_GET_MAIL_LIST` (`MailFrame.lua:42`).
//!
//! [`MailPending`] is the arrival layer behind `HasNewMail()` and the minimap icon, whose only
//! listener, `MiniMapMailFrame` (`Minimap.xml:278-289`), never reads the inbox. Reading mail clears
//! the icon because opening a letter arms a deferred refresh (`[0xb6efcc]`) and the mailbox close
//! re-asks the server.

use benilla_protocol::messages::{mail_error, mail_message_type, MailListEntry};
use bevy::prelude::*;

use benilla_ui::script::{MailInboxRow, MailState, ScriptValue, StationeryView, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{
    ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, Objects, SelfPlayer,
};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod net;
mod pending;

pub(crate) use pending::MailPending;

/// `checked` mask bits (vmangos `Mail.h`): READ, RETURNED, COPIED.
const CHECKED_READ: u32 = 0x1;
const CHECKED_RETURNED: u32 = 0x2;
const CHECKED_COPIED: u32 = 0x4;

/// `MAIL_STATIONERY_GM` (`Stationery.dbc` row 61): a GM row shows its stationery icon even when it
/// carries a package (`MailFrame.lua:108`).
const STATIONERY_GM: u32 = 61;

/// `CheckInbox`'s client-side rate limit on `CMSG_GET_MAIL_LIST`, in seconds (`0x4aeab0`).
const CHECK_INBOX_THROTTLE_SECS: f64 = 60.0;

/// The letter-row icon: the stationery's item icon (`Stationery.dbc` 41 → item 9311, display
/// 7798; 61 → item 18154, display 30658).
const STATIONERY_ICON_NORMAL: &str = "Interface\\Icons\\INV_Misc_Note_01";
const STATIONERY_ICON_GM: &str = "Interface\\Icons\\Mail_GMIcon";

/// The `Stationery.dbc` catalog; without it every mail gets the default backdrop.
#[derive(Resource)]
pub(crate) struct Stationery(pub(crate) benilla_formats::StationeryCatalog);

/// A `SEND` result queued for [`feed_mail`]. `MAIL_FAILED` fires on every one, success included:
/// the reference uses it as "the send resolved" (`0x4ad15f`), and the stock handler only
/// re-enables the button.
pub(crate) struct MailSendAck {
    pub(crate) ok: bool,
    /// The message key of a failure other than an equip error.
    pub(crate) refusal: Option<&'static str>,
}

/// The open mailbox session: the guid, the inbox rows as the wire sent them, the letter-body cache
/// and the result queues. Cleared on close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct MailOpen {
    pub(crate) mailbox: Option<u64>,
    /// Wire order, which is the 1-based display order.
    pub(crate) mails: Vec<MailListEntry>,
    /// Letter bodies by `item_text_id`, asked once.
    pub(crate) bodies: QueryCache<u32, String>,
    /// `MAIL_SHOW` owed: every mailbox use re-shows the window (`0x4acd10`).
    show_requested: bool,
    /// When the last `CMSG_GET_MAIL_LIST` went out, in `Time::elapsed_secs_f64`.
    last_list_query: Option<f64>,
    pub(crate) send_acks: Vec<MailSendAck>,
    /// 1-based rows a take emptied and purged, each owed a `CLOSE_INBOX_ITEM`.
    pub(crate) close_inbox: Vec<u32>,
    /// Take, return and delete refusals, as message keys.
    pub(crate) errors: Vec<&'static str>,
}

impl crate::query_cache::AskOnce for MailOpen {
    fn clear_pending(&mut self) {
        self.bodies.clear_pending();
    }
}

impl MailOpen {
    /// A mailbox right-click. A different mailbox resets the rows and the throttle; the same one
    /// only re-shows, as the reference fires `MAIL_SHOW` on every use.
    pub(crate) fn click(&mut self, mailbox: u64) {
        if self.mailbox != Some(mailbox) {
            self.mailbox = Some(mailbox);
            self.mails.clear();
            self.last_list_query = None;
        }
        self.show_requested = true;
    }

    fn mail_id_at(&self, index_1based: u32) -> Option<u32> {
        index_1based
            .checked_sub(1)
            .and_then(|i| self.mails.get(i as usize))
            .map(|e| e.message_id)
    }

    /// Sets a row's READ bit locally: `CMSG_MAIL_MARK_AS_READ` gets no reply.
    fn mark_read(&mut self, index_1based: u32) {
        if let Some(e) = index_1based
            .checked_sub(1)
            .and_then(|i| self.mails.get_mut(i as usize))
        {
            e.checked |= CHECKED_READ;
        }
    }

    /// A client-side close, sending nothing as 1.12 does; a re-open re-lists.
    pub(crate) fn clear(&mut self) {
        self.mailbox = None;
        self.mails.clear();
        self.bodies.clear();
        self.show_requested = false;
        self.last_list_query = None;
        self.send_acks.clear();
        self.errors.clear();
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// The range guard ([`crate::ui_session`]) closes the window, the `CloseMail` clear, past the
/// client's own interaction gate (`0x493230`, 5.5556 yd); the server's `CheckMailBox`
/// (`MailHandler.cpp:69`) checks 5 yd on each mailbox `CMSG`.
impl NpcSession for MailOpen {
    fn npc(&self) -> Option<u64> {
        self.mailbox
    }

    fn close(&mut self) {
        self.clear();
    }
}

pub(crate) struct UiMailPlugin;

impl Plugin for UiMailPlugin {
    fn build(&self, app: &mut App) {
        crate::query_cache::register::<MailOpen>(app);
        net::register(app);
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .add_systems(
                Update,
                (
                    // Range-close, feed, then drain after input, so each edge lands the same
                    // frame; the feed runs after `UnitFeed` so `SetInboxItem` reads the template.
                    close_npc_session_out_of_range::<MailOpen>.before(feed_mail),
                    feed_mail.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    drain_mail.after(UiInput),
                    send_query_next_mail_time_on_enter,
                ),
            );
    }
}

/// `MSG_QUERY_NEXT_MAIL_TIME` at every world enter, from the mail module's init in the world-enter
/// cascade (`0x4908c0`). The init stamps "no mail" first, so the icon goes dark across a loading
/// screen until the reply.
fn send_query_next_mail_time_on_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    commands: Res<NetCommands>,
    mut pending: ResMut<MailPending>,
) {
    if entered.read().next().is_some() {
        pending.on_query_sent();
        let _ = commands.0.send(ClientCommand::QueryNextMailTime);
    }
}

/// The message key a `SMSG_SEND_MAIL_RESULT` error shows: the handler `0x4ad050` indexes
/// `[0x4ad1a0 + error]` into the nine-arm jump table at `0x4ad17c`, and codes 6 to 13 and past 15
/// share `ERR_MAIL_DATABASE_ERROR` (`0x4ad147`). `OK` shows [`MAIL_SENT_KEY`] and `EQUIP_ERROR`
/// goes through the equip-error table (`0x4ad12e` → `0x622630`).
pub(crate) fn mail_refusal(error: u32) -> &'static str {
    match error {
        mail_error::CANNOT_SEND_TO_SELF => "ERR_MAIL_TO_SELF", // 0x164
        mail_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // 0x25, voiced (speech 0x28)
        mail_error::RECIPIENT_NOT_FOUND => "ERR_MAIL_TARGET_NOT_FOUND", // 0x165
        mail_error::NOT_YOUR_TEAM => "ERR_PLAYER_WRONG_FACTION", // 0xff
        mail_error::TRIAL_ACCOUNT => "ERR_RESTRICTED_ACCOUNT", // 0x1be
        mail_error::TOO_MANY_ATTACHMENTS => "ERR_MAIL_REACHED_CAP", // 0x1c4
        // INTERNAL_ERROR (6) and every other code (`0x4ad147`).
        _ => "ERR_MAIL_DATABASE_ERROR", // 0x166
    }
}

/// The yellow info line (id `0x167`) a successful `SEND` shows before the form reset (`0x4ad0b1`,
/// `0x4acdc0`); a successful take, return or delete says nothing.
pub(crate) const MAIL_SENT_KEY: &str = "ERR_MAIL_SENT";

mod invoice;

use invoice::auction_mail;

/// One wire row as the Lua-facing row; anything still in flight stays `None` until it lands.
fn resolve_row(
    entry: &MailListEntry,
    bodies: &QueryCache<u32, String>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    stationery: Option<&Stationery>,
    names: &NameCache,
    commands: &NetCommands,
    rolls: crate::items::RollCatalogs,
    macros: &crate::npc_text::MacroContext,
    // The VM's GlobalStrings, for the auction subject: the strings are the install's, never ours.
    get_text: &dyn Fn(&str) -> Option<String>,
) -> MailInboxRow {
    let is_gm = entry.stationery == STATIONERY_GM;
    let returned = entry.checked & CHECKED_RETURNED != 0;
    // Reply only to an unreturned mail from a player (`MailFrame.lua:281` reads it).
    let can_reply = entry.sender_guid.is_some() && !returned;
    // An unreturned player mail still carrying money or an item returns rather than deletes; the
    // reference's `InboxItemCanDelete` predicate is untraced.
    let can_delete =
        returned || entry.sender_guid.is_none() || (entry.item.is_none() && entry.money == 0);

    let (item_id, item_roll, item_count, item_name, item_texture, item_quality) = match &entry.item
    {
        Some(att) => {
            let template = items.template(att.entry, 0, commands);
            let name = template.map(|t| rolls.name(&t.name, att.random_prop_id));
            let quality = template.map(|t| t.quality);
            let display_id = template.map(|t| t.display_info_id).unwrap_or(0);
            let texture = icons
                .and_then(|i| i.catalog.get(display_id))
                .and_then(|d| d.icon.clone());
            (
                att.entry,
                att.random_prop_id,
                u32::from(att.count),
                name,
                texture,
                quality,
            )
        }
        None => (0, 0, 0, None, None, None),
    };

    let sender = entry
        .sender_guid
        .and_then(|g| names.resolve(g, commands).map(str::to_string));

    let stationery_texture = stationery
        .map(|s| s.0.texture(entry.stationery).to_string())
        .unwrap_or_else(|| benilla_formats::StationeryCatalog::DEFAULT_TEXTURE.to_string());

    // The raw body: the invoice parses it before any `$`-macro expansion.
    let raw_body = (entry.item_text_id != 0)
        .then(|| bodies.get(entry.item_text_id))
        .flatten();

    // ── The auction house's mail (`0x4ace70`, `0x4af360`) ──
    // Every notice gets its displayed subject; only a won or sold one has an invoice, once its
    // body is fetched.
    let auction = (entry.message_type == mail_message_type::AUCTION)
        .then(|| invoice::parse_subject(&entry.subject))
        .flatten();
    // The notice's item comes from the subject: a sold notice names an item it does not carry.
    let notice_item_name = auction
        .and_then(|a| items.template(a.entry, 0, commands))
        .map(|t| t.name.clone());
    // Until the item template lands, the raw triplet stands.
    let subject = match (auction, &notice_item_name) {
        (Some(a), Some(name)) => invoice::subject_key(a.result)
            .and_then(get_text)
            .map(|t| t.replace("%s", name))
            .unwrap_or_else(|| entry.subject.clone()),
        _ => entry.subject.clone(),
    };
    let auction_invoice = auction
        .filter(|a| matches!(a.result, auction_mail::WON | auction_mail::SOLD))
        .and_then(|a| {
            let seller = a.result == auction_mail::SOLD;
            let n = invoice::parse_body(raw_body?, seller)?;
            // An unresolved counterparty fills in when its name lands, as in the reference.
            let player_name = names.resolve(n.player_guid, commands)?.to_string();
            Some(benilla_ui::script::MailInvoice {
                seller,
                item_name: notice_item_name.clone()?,
                player_name,
                bid: n.bid,
                buyout: n.buyout,
                deposit: n.deposit,
                consignment: n.consignment,
            })
        });

    MailInboxRow {
        package_icon: item_texture.clone(),
        stationery_icon: Some(
            if is_gm {
                STATIONERY_ICON_GM
            } else {
                STATIONERY_ICON_NORMAL
            }
            .to_string(),
        ),
        sender,
        subject,
        money: entry.money,
        cod: entry.cod,
        days_left: entry.expire_days,
        item_count,
        was_read: entry.checked & CHECKED_READ != 0,
        was_returned: returned,
        text_created: entry.checked & CHECKED_COPIED != 0,
        can_reply,
        is_gm,
        // The body runs the `$`-macro expander for the local player, as `GetInboxText` does.
        body: raw_body.map(|b| crate::npc_text::substitute(b, macros)),
        stationery_texture,
        // Won and sold notices only, not every auction mail: the reference's `isInvoice` reads
        // what its subject parser persists for those two alone.
        is_invoice: auction
            .is_some_and(|a| matches!(a.result, auction_mail::WON | auction_mail::SOLD)),
        invoice: auction_invoice,
        has_body: entry.item_text_id != 0,
        item_id,
        item_random_property_id: item_roll,
        item_name,
        item_texture,
        item_quality,
        can_delete,
    }
}

/// The usable stationery list (`0x4ad970`): rows always available (`Flags & 1`) or whose item the
/// player carries in bags (`0x622270`), with a cached template, sorted by buy price (`0x4ada90`).
/// A carried paper has no `cost`.
fn stationeries(
    catalog: &Stationery,
    self_q: &Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Vec<StationeryView> {
    let store = self_q.iter().next().map(|(s, _)| s);
    let mut out: Vec<(u32, StationeryView)> = Vec::new();
    for row in catalog.0.rows() {
        let Some(t) = items.template(row.item, 0, commands).cloned() else {
            continue;
        };
        let carried = store.is_some_and(|s| {
            crate::ui_items::count_of(
                &s.0,
                objects,
                row.item,
                crate::ui_items::InventoryScope::CARRIED,
            ) > 0
        });
        if row.flags & 1 == 0 && !carried {
            continue;
        }
        out.push((
            t.buy_price,
            StationeryView {
                id: row.id,
                name: t.name.clone(),
                icon: crate::ui_items::item_icon(icons, t.display_info_id).unwrap_or_default(),
                cost: (!carried).then_some(t.buy_price),
                texture: row.texture.clone(),
            },
        ));
    }
    out.sort_by_key(|(price, v)| (*price, v.id));
    out.into_iter().map(|(_, v)| v).collect()
}

fn snapshot(
    mail: &MailOpen,
    items: &Items,
    icons: Option<&ItemDisplays>,
    stationery: Option<&Stationery>,
    names: &NameCache,
    commands: &NetCommands,
    rolls: crate::items::RollCatalogs,
    macros: &crate::npc_text::MacroContext,
    get_text: &dyn Fn(&str) -> Option<String>,
) -> Option<MailState> {
    mail.mailbox?;
    Some(MailState {
        inbox: mail
            .mails
            .iter()
            .map(|e| {
                resolve_row(
                    e,
                    &mail.bodies,
                    items,
                    icons,
                    stationery,
                    names,
                    commands,
                    rolls,
                    macros,
                    get_text,
                )
            })
            .collect(),
    })
}

/// The feed's remaining parameters, bundled under Bevy's 16-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
struct MailFeedExtras<'w, 's> {
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    sink: crate::ui_action::MessageSink<'w>,
    stationeries: Local<'s, crate::ui_script::VmMemo<Vec<StationeryView>>>,
}

fn feed_mail(
    script: Option<NonSendMut<UiScript>>,
    mut mail: ResMut<MailOpen>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    stationery: Option<Res<Stationery>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut pending: ResMut<MailPending>,
    time: Res<Time>,
    mut last: Local<crate::ui_script::VmMemo<Option<MailState>>>,
    mut last_mailbox: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut last_has_new_mail: Local<crate::ui_script::VmMemo<bool>>,
    // The local player, the letter body's `$`-macro subject.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
    extras: MailFeedExtras,
) {
    let Some(mut script) = script else {
        return;
    };
    let MailFeedExtras {
        props,
        enchants,
        mut sink,
        stationeries: mut last_stationeries,
    } = extras;
    let rolls = crate::items::RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    let last = last.get(&script);
    let last_mailbox = last_mailbox.get(&script);
    let last_has_new_mail = last_has_new_mail.get(&script);

    // Take, return and delete refusals, on the surface and voice their message row names.
    let refusals: Vec<_> = std::mem::take(&mut mail.errors)
        .into_iter()
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_mail", refusals);
    // A purged row fires `CLOSE_INBOX_ITEM(index)` before the list's `MAIL_INBOX_UPDATE`, as both
    // take legs do (money `0x4ad772` before `0x4ad7a3`, item `0x4ad87d` before `0x4ad8ae`).
    for index in std::mem::take(&mut mail.close_inbox) {
        script.fire_event("CLOSE_INBOX_ITEM", vec![ScriptValue::Int(i64::from(index))]);
    }
    // `0x4ad050`'s order: the message line, then on a successful send the compose reset
    // (`0x4acdc0`) with its three events, then `MAIL_FAILED` on every path (`0x4ad15f`).
    for ack in std::mem::take(&mut mail.send_acks) {
        let key = if ack.ok {
            Some(MAIL_SENT_KEY)
        } else {
            ack.refusal
        };
        let line = key.and_then(|k| crate::ui_action::keyed_line(&script, k));
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_mail", line);
        if ack.ok {
            script.reset_compose_tab();
        }
        script.fire_event("MAIL_FAILED", vec![]);
    }

    // `None` until the player is named; the diff re-substitutes an open letter when it lands.
    let subject = crate::npc_text::player_identity(&self_q, &names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: subject.as_ref(),
        states: &states,
    };
    let fresh = {
        // Scoped: the resolver borrows the VM, and `set_mail` below needs it back.
        let get_text = |key: &str| script.lua().globals().get::<String>(key).ok();
        snapshot(
            &mail,
            &items,
            icons.as_deref(),
            stationery.as_deref(),
            &names,
            &commands,
            rolls,
            &macros,
            &get_text,
        )
    };
    let opened = last_mailbox.is_none() && mail.mailbox.is_some();
    let closed = last_mailbox.is_some() && mail.mailbox.is_none();
    let show_requested = std::mem::take(&mut mail.show_requested);
    let changed = fresh != *last;
    if changed {
        script.set_mail(fresh.clone());
    }
    // The usable stationery is asked ahead of any mailbox so the list is whole when
    // `SendMailFrame_Reset` selects row 1 on show; an empty list leaves every send unsent. Only
    // with a player: an ask sent before the socket exists is dropped and never re-asked.
    let usable = stationery
        .as_deref()
        .filter(|_| !self_q.is_empty())
        .map(|catalog| {
            stationeries(
                catalog,
                &self_q,
                &objects,
                &items,
                icons.as_deref(),
                &commands,
            )
        })
        .unwrap_or_default();
    let memo = last_stationeries.get(&script);
    if *memo != usable {
        *memo = usable.clone();
        script.set_mail_stationeries(usable);
    }
    if opened {
        // The selection clears on open and close (`0x4ace07`); `MAIL_SHOW`'s reset picks row 1.
        script.clear_stationery();
        // The open core (`0x4acd10`) resets the compose tab before `MAIL_SHOW`; the reset's
        // `MAIL_SEND_SUCCESS`, page-turn sound and all, is the reference's too.
        script.reset_compose_tab();
        script.fire_event("MAIL_SHOW", vec![]);
    } else if closed {
        script.clear_stationery();
        script.fire_event("MAIL_CLOSED", vec![]);
        // The close core's tail (`0x4acdad`, `0x4acdb1`): after a read, or an arrival while open,
        // re-ask the server, stamping "no mail". `CloseMail` and the range close both land here.
        if pending.take_refresh() {
            pending.on_query_sent();
            let _ = commands.0.send(ClientCommand::QueryNextMailTime);
        }
    } else if mail.mailbox.is_some() {
        // A re-click re-shows the open window; `MAIL_SHOW`'s `CheckInbox` refreshes the list.
        if show_requested {
            script.fire_event("MAIL_SHOW", vec![]);
        }
        if changed {
            script.fire_event("MAIL_INBOX_UPDATE", vec![]);
        }
    }
    *last = fresh;
    *last_mailbox = mail.mailbox;

    // `HasNewMail()` is pushed on change, but `UPDATE_PENDING_MAIL` fires only at the reference's
    // three sites, so a query's "no mail" stamp does not move the icon until the reply lands.
    pending.step(time.delta_secs());
    let has_new_mail = pending.has_new_mail();
    if has_new_mail != *last_has_new_mail {
        script.set_has_new_mail(has_new_mail);
        *last_has_new_mail = has_new_mail;
    }
    if pending.take_notify() {
        script.fire_event("UPDATE_PENDING_MAIL", vec![]);
    }
}

/// The Lua intents out as the mail `CMSG`s; `CloseMail` is a local clear that sends nothing.
fn drain_mail(
    script: Option<NonSendMut<UiScript>>,
    mut mail: ResMut<MailOpen>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    time: Res<Time>,
    mut pending: ResMut<MailPending>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(mailbox) = mail.mailbox else {
        // No session: still honor a stray close (idempotent) and drop everything else.
        if script.take_mail_close() {
            mail.clear();
        }
        let _ = script.take_mail_check_inbox();
        let _ = script.take_mail_opens();
        let _ = script.take_mail_send();
        return;
    };

    // `CheckInbox`: the list query, throttled to one per 60 s (`0x4aeab0`).
    if script.take_mail_check_inbox() {
        let now = time.elapsed_secs_f64();
        if mail
            .last_list_query
            .is_none_or(|t| now - t >= CHECK_INBOX_THROTTLE_SECS)
        {
            mail.last_list_query = Some(now);
            let _ = commands.0.send(ClientCommand::GetMailList { mailbox });
        }
    }

    // An opened mail: mark read, then ask for the body, in `0x4af110`'s order.
    for index in script.take_mail_opens() {
        let Some((mail_id, text_id, read)) = index
            .checked_sub(1)
            .and_then(|i| mail.mails.get(i as usize))
            .map(|e| (e.message_id, e.item_text_id, e.checked & CHECKED_READ != 0))
        else {
            continue;
        };
        // Every open arms the refresh, read or not: `GetInboxText` calls the mark-as-read sender
        // unconditionally, which sets `[0xb6efcc]` (`0x4adda6`).
        pending.arm_refresh();
        if !read {
            mail.mark_read(index);
            let _ = commands
                .0
                .send(ClientCommand::MailMarkAsRead { mailbox, mail_id });
        }
        if text_id != 0 {
            mail.bodies.get_or_ask(text_id, || {
                let _ = commands
                    .0
                    .send(ClientCommand::ItemTextQuery { text_id, mail_id });
            });
        }
    }

    for index in script.take_mail_take_items() {
        if let Some(mail_id) = mail.mail_id_at(index) {
            let _ = commands
                .0
                .send(ClientCommand::MailTakeItem { mailbox, mail_id });
        }
    }
    for index in script.take_mail_take_money() {
        if let Some(mail_id) = mail.mail_id_at(index) {
            let _ = commands
                .0
                .send(ClientCommand::MailTakeMoney { mailbox, mail_id });
        }
    }
    for index in script.take_mail_deletes() {
        if let Some(mail_id) = mail.mail_id_at(index) {
            let _ = commands
                .0
                .send(ClientCommand::MailDelete { mailbox, mail_id });
        }
    }
    for index in script.take_mail_returns() {
        if let Some(mail_id) = mail.mail_id_at(index) {
            let _ = commands
                .0
                .send(ClientCommand::MailReturnToSender { mailbox, mail_id });
        }
    }
    for index in script.take_mail_take_texts() {
        if let Some(mail_id) = mail.mail_id_at(index) {
            let _ = commands
                .0
                .send(ClientCommand::MailCreateTextItem { mailbox, mail_id });
        }
    }

    // `SendMail`: the attachment's slot is read at send time, as in the reference.
    if let Some(req) = script.take_mail_send() {
        let item_guid = req.item.and_then(|(bag, slot)| {
            self_q.iter().next().and_then(|s| {
                crate::ui_items::slot_guid(&s.0, bag, (slot.max(1) - 1) as u8, &objects)
            })
        });
        if req.item.is_some() && item_guid.is_none() {
            // The item left its slot: `ERR_ITEM_NOT_FOUND`, no packet, the attachment dropped and
            // `MAIL_SEND_INFO_UPDATE` fired (`0x4ae98d`).
            mail.errors.push("ERR_ITEM_NOT_FOUND");
            script.drop_send_mail_item();
        } else {
            let _ = commands.0.send(ClientCommand::SendMail {
                mailbox,
                receiver: req.target,
                subject: req.subject,
                body: req.body,
                stationery: req.stationery,
                item_guid: item_guid.unwrap_or(0),
                money: req.money,
                cod: req.cod,
            });
        }
    }

    if script.take_mail_close() {
        mail.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::MailAttachment;

    /// The result table at `0x4ad17c` (indexed at `0x4ad1a0`), asserted as the message ids its
    /// arms push.
    #[test]
    fn the_mail_result_table_is_the_references_own() {
        let id = |key: &str| {
            benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a catalog row"))
                .id
        };
        for (code, want) in [
            (mail_error::CANNOT_SEND_TO_SELF, 0x164u16),
            (mail_error::NOT_ENOUGH_MONEY, 0x25),
            (mail_error::RECIPIENT_NOT_FOUND, 0x165),
            (mail_error::NOT_YOUR_TEAM, 0xff),
            (mail_error::INTERNAL_ERROR, 0x166),
            (mail_error::TRIAL_ACCOUNT, 0x1be),
            (mail_error::TOO_MANY_ATTACHMENTS, 0x1c4),
        ] {
            assert_eq!(id(mail_refusal(code)), want, "mail error {code}");
        }
        // `ja 0x4ad147` and the index table's `08` run: 7..13 and past 15 share the database error.
        for code in [7, 8, 13, 16, 99, u32::MAX] {
            assert_eq!(id(mail_refusal(code)), 0x166, "mail error {code}");
        }
        // A success shows on the yellow info line.
        assert_eq!(id(MAIL_SENT_KEY), 0x167);
        assert_eq!(
            benilla_ui::messages::by_key(MAIL_SENT_KEY)
                .expect("row")
                .kind,
            benilla_ui::messages::MsgKind::Info
        );
    }

    /// A subject-less macro context: no row test uses a `$` token.
    fn no_macros(states: &crate::world_state::WorldStates) -> crate::npc_text::MacroContext<'_> {
        crate::npc_text::MacroContext {
            subject: None,
            states,
        }
    }

    fn entry(
        message_id: u32,
        sender_guid: Option<u64>,
        checked: u32,
        expire_days: f32,
    ) -> MailListEntry {
        MailListEntry {
            message_id,
            message_type: mail_message_type::NORMAL,
            sender_guid,
            sender_id: None,
            subject: "s".into(),
            item_text_id: 0,
            stationery: 41,
            item: None,
            money: 0,
            cod: 0,
            checked,
            expire_days,
            mail_template_id: 0,
        }
    }

    #[test]
    fn click_opens_and_reclick_keeps_rows() {
        let mut m = MailOpen::default();
        m.click(0x100);
        assert_eq!(m.mailbox, Some(0x100));
        assert!(m.show_requested);
        m.mails.push(entry(1, Some(0xA), 0, 30.0));
        m.last_list_query = Some(5.0);
        m.show_requested = false;
        // The same mailbox keeps the rows and throttle, and re-requests the show.
        m.click(0x100);
        assert_eq!(m.mails.len(), 1);
        assert_eq!(m.last_list_query, Some(5.0));
        assert!(m.show_requested);
        // A different mailbox resets the session.
        m.click(0x200);
        assert!(m.mails.is_empty());
        assert_eq!(m.last_list_query, None);
    }

    #[test]
    fn mail_id_and_mark_read() {
        let mut m = MailOpen::default();
        m.click(0x1);
        m.mails.push(entry(42, Some(0xA), 0, 30.0));
        m.mails.push(entry(43, None, CHECKED_READ, 10.0));
        assert_eq!(m.mail_id_at(1), Some(42));
        assert_eq!(m.mail_id_at(2), Some(43));
        assert_eq!(m.mail_id_at(3), None);
        assert_eq!(m.mail_id_at(0), None);
        assert_eq!(m.mails[0].checked & CHECKED_READ, 0);
        m.mark_read(1);
        assert_eq!(m.mails[0].checked & CHECKED_READ, CHECKED_READ);
    }

    #[test]
    fn clear_closes_the_window() {
        let mut m = MailOpen::default();
        m.click(0x1);
        m.mails.push(entry(1, Some(0xA), 0, 30.0));
        m.bodies.insert(7, Some("hi".into()));
        m.clear();
        assert!(m.mailbox.is_none());
        assert!(m.mails.is_empty());
        assert!(m.bodies.is_empty());
    }

    /// No GlobalStrings in a unit test: every key misses.
    fn no_strings(_key: &str) -> Option<String> {
        None
    }

    #[test]
    fn resolve_row_derives_the_reply_and_delete_law() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let bodies = QueryCache::default();

        // A plain letter from a player: replyable and deletable.
        let from_player = entry(1, Some(0xA), 0, 30.0);
        let row = resolve_row(
            &from_player,
            &bodies,
            &items,
            None,
            None,
            &names,
            &commands,
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_reply);
        assert!(row.can_delete);
        assert_eq!(row.stationery_texture, "STATIONERYTEST");

        // A player mail still carrying money returns, so it is not deletable; so does an item.
        let mut with_money = entry(4, Some(0xA), 0, 30.0);
        with_money.money = 500;
        let row = resolve_row(
            &with_money,
            &bodies,
            &items,
            None,
            None,
            &names,
            &commands,
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_reply);
        assert!(!row.can_delete);

        // A returned mail: not replyable, deletable.
        let returned = entry(2, Some(0xA), CHECKED_RETURNED, 30.0);
        let row = resolve_row(
            &returned,
            &bodies,
            &items,
            None,
            None,
            &names,
            &commands,
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(!row.can_reply);
        assert!(row.can_delete);

        // A system mail, with no sender guid: deletable.
        let system = entry(3, None, 0, 30.0);
        let row = resolve_row(
            &system,
            &bodies,
            &items,
            None,
            None,
            &names,
            &commands,
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_delete);
    }

    #[test]
    fn resolve_row_reads_the_item_and_body_caches() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let mut bodies = QueryCache::default();
        bodies.insert(99, Some("the letter body".into()));

        let mut e = entry(1, Some(0xA), 0, 30.0);
        e.item_text_id = 99;
        e.item = Some(MailAttachment {
            entry: 2589,
            perm_enchant: 0,
            random_prop_id: 0,
            suffix_factor: 0,
            count: 5,
            charges: 0,
            durability_max: 0,
            durability: 0,
        });
        let row = resolve_row(
            &e,
            &bodies,
            &items,
            None,
            None,
            &names,
            &commands,
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert_eq!(row.item_id, 2589);
        assert_eq!(row.item_count, 5);
        assert_eq!(row.body.as_deref(), Some("the letter body"));
        assert!(row.has_body);
        // The item template has not answered yet.
        assert!(row.item_name.is_none());
    }
}
