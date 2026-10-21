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
use std::collections::HashMap;

use benilla_ui::script::{MailInboxRow, MailState, ScriptValue, StationeryView, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{
    ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer, WorldEnterCascadeMessage,
};
use crate::query_cache::Ask;
use crate::query_cache::QueryCache;
use crate::script_calls::VmWith;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

mod net;
mod pending;
#[cfg(test)]
mod verb_tests;
mod verbs;

pub(crate) use pending::MailPending;
pub(crate) use verbs::MailVerbs;

/// `checked` mask bits (vmangos `Mail.h`): READ, RETURNED, COPIED.
const CHECKED_READ: u32 = 0x1;
const CHECKED_RETURNED: u32 = 0x2;
const CHECKED_COPIED: u32 = 0x4;
/// `MAIL_CHECK_MASK_COD_PAYMENT` (`byte[rec+0x148] & 8`): the money a COD taker paid; taking it
/// empties the mail (`0x4ad6b0`).
const CHECKED_COD_PAYMENT: u32 = 0x8;

/// `MAIL_STATIONERY_GM` (`Stationery.dbc` row 61): a GM row shows its stationery icon even when it
/// carries a package (`MailFrame.lua:108`).
const STATIONERY_GM: u32 = 61;

/// The `Stationery.dbc` catalog; without it every mail's texture is nil and no paper is usable.
#[derive(Resource)]
pub(crate) struct Stationery(pub(crate) benilla_formats::StationeryCatalog);

/// `MailTemplate.dbc`'s bodies by id, in the client's locale; without them a template mail's body
/// is nil, as for an id the store lacks (`0x4af229`).
#[derive(Resource)]
pub(crate) struct MailTemplates(pub(crate) std::collections::HashMap<u32, String>);

/// `Package.dbc`'s rows, in file order; without them `GetNumPackages()` answers 0.
#[derive(Resource)]
pub(crate) struct Packages(pub(crate) Vec<benilla_formats::PackageRow>);

/// The stores a row's icons and a non-player sender's name are read from.
#[derive(Clone, Copy, Default)]
pub(super) struct RowCatalogs<'a> {
    pub(super) packages: Option<&'a Packages>,
    pub(super) houses: Option<&'a crate::ui_auction::AuctionHouses>,
    pub(super) gameobjects: Option<&'a crate::go_templates::GameObjectTemplates>,
}

/// A package row as the engine answers it: the icon under `StringLookups.dbc` row 3's folder
/// (`0x4ae4ab`, the `"%s%s"` of folder and `\`, then the basename appended).
fn package_view(row: &benilla_formats::PackageRow) -> benilla_ui::script::PackageView {
    benilla_ui::script::PackageView {
        id: row.id,
        name: row.name.clone(),
        icon: format!("Interface\\Icons\\{}", row.icon),
        cost: row.cost,
    }
}

/// One `SMSG_SEND_MAIL_RESULT`, queued in arrival order for [`feed_mail`]. The handler (`0x4ad050`)
/// ends every arm (send, take money, take item, return, delete, each success and each refusal)
/// at one tail that lifts the locks and signals `MAIL_FAILED` (`0x4ad15f`-`0x4ad16a`), so every
/// result owes the event, success included; the stock handler only re-enables the send button.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct MailResult {
    /// The message key of the line the arm shows: a send's "mail sent" or a refusal other than an
    /// equip error, which goes through the equip-error table.
    pub(crate) line: Option<&'static str>,
    /// A `SEND` result and whether it succeeded: it lifts the send-pending flag (`0x4ad159`), and a
    /// success resets the compose tab (`0x4ad0ca`).
    pub(crate) send: Option<bool>,
    /// The 1-based row a take emptied and purged, owed a `CLOSE_INBOX_ITEM`.
    pub(crate) close_inbox: Option<u32>,
    /// The arm edited a held row (or removed it): its own `MAIL_INBOX_UPDATE`, ahead of the tail.
    pub(crate) inbox_update: bool,
}

/// The mailbox session: the open mailbox's guid, the inbox rows as the wire sent them, the
/// letter-body cache and the result queues. A close forgets the guid and the queues and keeps the
/// rows, as the reference's close does (`0x4acd71`); the world-enter init (`0x4acb10`) and a
/// disconnect clear everything.
#[derive(Resource, Default)]
pub(crate) struct MailOpen {
    pub(crate) mailbox: Option<u64>,
    /// Wire order, which is the 1-based display order.
    pub(crate) mails: Vec<MailListEntry>,
    /// Letter bodies by `item_text_id`, asked once.
    pub(crate) bodies: QueryCache<u32, String>,
    /// `MAIL_SHOW` owed: every mailbox use re-shows the window (`0x4acd10`).
    show_requested: bool,
    /// A list or send answer landed since the last feed: the VM's request lock clears.
    request_resolved: bool,
    /// Inbox list results parsed since the last feed, each owed a `MAIL_INBOX_UPDATE`
    /// (`0x4ad5c6`-`0x4ad5d6`), changed or not.
    list_results: u32,
    /// The mail results since the last feed, in arrival order.
    pub(crate) results: Vec<MailResult>,
    /// Each auction notice's subject cell (`[rec+0x14]`) as the list parse left it, by mail id; the
    /// row keeps the raw triplet, the copy (`[rec+0x150]`) every parse reads.
    pub(crate) auction_subjects: HashMap<u32, ListedSubject>,
}

/// An auction notice's shown subject after the list parse (`0x4ad580`-`0x4ad5b2`): rewritten
/// and masked when the notice's item was held then, else empty (`0x4acf3d`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListedSubject {
    pub(crate) text: String,
    /// The parse rewrote it. Else the cell stays empty until the item's template lands and its
    /// callback rewrites it (`0x4ad010` → `0x4ace40`), with no mask.
    pub(crate) rewritten: bool,
}

impl crate::query_cache::AskOnce for MailOpen {
    fn clear_pending(&mut self) {
        self.bodies.clear_pending();
    }
}

impl MailOpen {
    /// The open core's cell write (`0x4acd27`): the guid, keeping the rows whichever mailbox they
    /// came from, and `MAIL_SHOW` owed, as the reference fires it on every use. [`open_mailbox`]
    /// registers on the latch first.
    pub(crate) fn click(&mut self, mailbox: u64) {
        self.mailbox = Some(mailbox);
        self.show_requested = true;
    }

    /// A list or send answer: the VM's request lock clears at the next feed.
    pub(crate) fn resolve_request(&mut self) {
        self.request_resolved = true;
    }

    /// The read mark's record half (`0x4adcd0`), as `CMSG_MAIL_MARK_AS_READ` gets no reply: the
    /// READ bit (`0x4add62`), and days left above 3 lowered to 3 on a mail with no COD
    /// (`0x4add72`-`0x4add8e`).
    fn mark_read(&mut self, mail_id: u32) {
        if let Some(e) = self.mails.iter_mut().find(|e| e.message_id == mail_id) {
            e.checked |= CHECKED_READ;
            if e.cod == 0 && e.expire_days > 3.0 {
                e.expire_days = 3.0;
            }
        }
    }

    /// A client-side close, sending nothing as 1.12 does (`0x4acd71`): the guid goes, the rows
    /// and the request lock stay, so a re-open inside `CheckInbox`'s window re-announces them. A
    /// result already landed still lifts the lock and signals: `0x4ad050` tests no mailbox.
    pub(crate) fn clear(&mut self) {
        self.mailbox = None;
        self.show_requested = false;
    }

    /// The world-enter init (`0x4acb10`) and a disconnect: no mailbox and no rows.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.results.clear();
        self.mails.clear();
        self.auction_subjects.clear();
        self.bodies.clear();
        self.request_resolved = false;
        self.list_results = 0;
    }
}

/// The mailbox's cell `[0xb6ef88]`; `CloseMail` (`0x4aded0`) is the core's ordinary arm. The
/// server's `CheckMailBox` (`MailHandler.cpp:69`) also checks 5 yd on each mailbox `CMSG`.
impl LatchedWindow for MailOpen {
    const WINDOW: NpcWindow = NpcWindow::Mail;

    fn cell(&self) -> Option<u64> {
        self.mailbox
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_mail_close()
    }
}

/// A mailbox use (`0x5f6820` → `0x4acd10`): the latch first, with no test of the cell
/// (`0x4acd1c`), so a use of the open mailbox closes the window and opens it again; then the cell
/// (`0x4acd27`); the feed resets the compose tab and fires `MAIL_SHOW` (`0x4acd38`, `0x4acd42`).
pub(crate) fn open_mailbox(world: &mut World, mailbox: u64) {
    crate::ui_session::register(world, mailbox);
    world.resource_mut::<MailOpen>().click(mailbox);
}

/// The close core's ordinary arm, `0x4acd50` with `cl = 0`: the compose tab's silent reset first
/// and always (`0x4acd55` → `0x4acdc0(0)`), then, with a mailbox open, the latch release
/// (`0x4acd85`), the cell cleared, `MAIL_CLOSED` (`0x4acda3`) and the tail (`0x4acda8`,
/// `0x4acdb1`): after a read, or an arrival while open, the next-mail-time query, stamping "no
/// mail".
pub(crate) fn close_core(world: &mut World) {
    crate::ui_session::with_script(world, |script| script.reset_compose_tab_silently());
    let Some(mailbox) = world.resource::<MailOpen>().mailbox else {
        return;
    };
    crate::ui_session::release(world, mailbox);
    world.resource_mut::<MailOpen>().clear();
    crate::ui_session::note_closed(world, NpcWindow::Mail);
    crate::ui_session::fire(world, |script| {
        script.mail_closed();
        script.fire_event("MAIL_CLOSED", vec![]);
    });
    let refresh = world
        .get_resource_mut::<MailPending>()
        .is_some_and(|mut p| p.take_refresh());
    if refresh {
        world.resource_mut::<MailPending>().on_query_sent();
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net.0.send(ClientCommand::QueryNextMailTime);
        }
    }
}

/// The leave-world sweep's mail leg (`0x490b1b` → `0x4acc00` → `0x4acd50(cl=1)`): the compose
/// tab's silent reset runs first and always (`0x4acd55` → `0x4acdc0(0)`), lifting the
/// attachment's lock in the VM the sweep runs in (`0x4acdd6`); then the mailbox is forgotten with
/// no `MAIL_CLOSED`, no interaction release and no refresh (`0x4acd5f`-`0x4acd6b`), the `cl`
/// arm the close button's `0x4acd71` skips to. The latch still names the mailbox, which the
/// sweep's interaction close then finds no window for.
pub(crate) fn close_mail_on_leave_world(mut vm: VmWith<ResMut<'static, MailOpen>>) {
    vm.run(|script| {
        if let Some(script) = script {
            script.reset_compose_tab_silently();
        }
    });
    vm.own().clear();
}

pub(crate) struct UiMailPlugin;

impl Plugin for UiMailPlugin {
    fn build(&self, app: &mut App) {
        crate::query_cache::register::<MailOpen>(app);
        net::register(app);
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .add_latched_window::<MailOpen>()
            .add_sweep_leg(SweepLeg::Mail, close_mail_on_leave_world)
            .add_systems(
                Update,
                (
                    // Feed, then drain after input, so each edge lands the same frame; the feed
                    // runs after `UnitFeed` so `SetInboxItem` reads the template.
                    feed_mail.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    drain_mail.in_set(crate::ui_script::UiDrain),
                    // The cascade's order: the time query, this, then the battlefield status.
                    send_query_next_mail_time_on_enter
                        .after(crate::net::send_query_time)
                        .in_set(UiFeed),
                ),
            );
    }
}

/// The mail init (`0x4acb10`, called at `0x4909f6`) at each world-enter cascade, which a login, a
/// cross-map worldport and a `/reload` all run: the mailbox, the rows, the list throttle
/// (`0x4acb7d`) and the request lock (`0x4acb82`) start over, every stationery's item is looked up
/// with a callback (`0x4acbba`), then it stamps "no mail" (`0x4acb87`) and sends
/// `MSG_QUERY_NEXT_MAIL_TIME` (`0x4acbdc`), after the time query and before the battlefield status.
pub(crate) fn send_query_next_mail_time_on_enter(
    mut vm: VmWith<MailSession>,
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    commands: Res<NetCommands>,
    items: Option<Res<Items>>,
    stationery: Option<Res<Stationery>>,
) {
    vm.turn(|script, (mut mail, mut pending)| {
        if cascades.read().next().is_some() {
            mail.clear_session();
            if let Some(script) = script {
                script.reset_mail_world_state();
            }
            if let Some(items) = items.as_deref() {
                for row in stationery.iter().flat_map(|s| s.0.rows()) {
                    items.template(row.item, 0, Ask::Event(&commands));
                }
            }
            pending.on_query_sent();
            let _ = commands.0.send(ClientCommand::QueryNextMailTime);
        }
    })
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

/// An auction notice's subject as `0x4ace70` writes it once the notice's item is held
/// (`0x4acf04`): the outcome's `AUCTION_*_MAIL_SUBJECT` (`0x4acfec`) with the item's name and
/// its roll's suffix (`0x4acf5d` → `0x5d8b00`), `""`
/// when the string is missing or empty (`0x4acf99`-`0x4acfbd`). `None` for a subject that is not
/// the triplet, and for an item not held, which the lookup asks for (`0x4acf38`).
pub(super) fn auction_subject(
    raw: &str,
    items: &Items,
    props: Option<&crate::items::RandomProperties>,
    ask: Ask<'_>,
    get_text: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let notice = invoice::parse_subject(raw)?;
    let template = items.template(notice.entry, 0, ask)?;
    let name = &crate::items::item_display_name(&template.name, notice.random_property, props);
    Some(
        invoice::subject_key(notice.result)
            .and_then(get_text)
            .filter(|t| !t.is_empty())
            .map(|t| t.replace("%s", name))
            .unwrap_or_default(),
    )
}

/// One wire row as the Lua-facing row; anything still in flight stays `None` until it lands. The
/// caches are read as `ask` says: the reference's `GetInboxHeaderInfo` (sender `0x4aedf6`, item
/// `0x4aecc7`), its subject parse at the list (`0x4acf38`) and `GetInboxInvoiceInfo`'s name
/// (`0x4af4b7`) all look up with a callback.
fn resolve_row(
    entry: &MailListEntry,
    bodies: &QueryCache<u32, String>,
    // What the list parse left in an auction notice's subject.
    listed: Option<&ListedSubject>,
    items: &Items,
    icons: Option<&ItemDisplays>,
    stationery: Option<&Stationery>,
    templates: Option<&MailTemplates>,
    catalogs: RowCatalogs,
    names: &NameCache,
    ask: Ask<'_>,
    rolls: crate::items::RollCatalogs,
    macros: &crate::npc_text::MacroContext,
    // The VM's GlobalStrings, for the auction subject: the strings are the install's, never ours.
    get_text: &dyn Fn(&str) -> Option<String>,
) -> MailInboxRow {
    // The GM flag is the stationery alone (`0x4aeda1`); the reply flag is a player's mail off it,
    // returned or not (`0x4aefa5`-`0x4aefbc`), which `MailFrame.lua:281` reads.
    let is_gm = entry.stationery == STATIONERY_GM;
    let returned = entry.checked & CHECKED_RETURNED != 0;
    let player_mail = entry.message_type == mail_message_type::NORMAL;
    let can_reply = player_mail && !is_gm;

    let (item_id, item_roll, item_count, item_name, item_texture, item_quality) = match &entry.item
    {
        Some(att) => {
            let template = items.template(att.entry, 0, ask);
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
    // The block's charges and `(current, max)` durability (`0x53566f`, `0x535699`, `0x53568d`).
    let (item_charges, item_durability) = entry.item.as_ref().map_or((0, (0, 0)), |att| {
        (att.charges as i32, (att.durability, att.durability_max))
    });

    // A player's GM letter is from `GM_EMAIL_NAME`, `""` when the string is missing
    // (`0x4aedbd`-`0x4aedcb`, `0x703bf0`); any other player's mail from its sender's name.
    // A non-player sender is named from its store by the sender id (`[rec+0x10]`, jump table
    // `0x4af068`): an auction house's name (`0x4aee11`), a creature's (`0x4aee48`) and a game
    // object's (`0x4aee78`), the last two asked with no guid.
    let sender_id = entry.sender_id.unwrap_or(0);
    let sender = match entry.message_type {
        mail_message_type::NORMAL if is_gm => Some(get_text("GM_EMAIL_NAME").unwrap_or_default()),
        mail_message_type::NORMAL => entry
            .sender_guid
            .and_then(|g| names.resolve_player(g, ask).map(str::to_string)),
        mail_message_type::AUCTION => catalogs
            .houses
            .and_then(|h| h.0.get(sender_id))
            .map(|h| h.name.clone()),
        mail_message_type::CREATURE => names
            .resolve_creature(sender_id, 0, ask)
            .map(str::to_string),
        mail_message_type::GAMEOBJECT => catalogs
            .gameobjects
            .and_then(|g| g.resolve_name(sender_id, ask))
            .map(str::to_string),
        _ => None,
    };

    // The package's icon when the row carries an id the store holds (`[rec+0x118]`,
    // `0x4aec76`-`0x4aec9c`), else the item's (`[rec+0x120]`, `0x4aeca6`-`0x4aecd8`), GM letter
    // or not.
    let package_icon = catalogs
        .packages
        .filter(|_| (entry.package as i32) > 0)
        .and_then(|p| p.0.iter().find(|r| r.id == entry.package))
        .map(|r| package_view(r).icon)
        .or_else(|| item_texture.clone());
    // The paper's item's icon (`[rec+0x11c]` → the store row's item, `0x4aed25`-`0x4aed74`), nil
    // for a paper the store lacks or an item not yet held.
    let stationery_icon = stationery
        .and_then(|s| s.0.rows().iter().find(|r| r.id == entry.stationery))
        .and_then(|r| items.template(r.item, 0, ask))
        .and_then(|t| crate::ui_items::item_icon(icons, t.display_info_id));

    // The store's texture for the id, none for one it lacks (`0x4af27c`-`0x4af2a9`).
    let stationery_texture = stationery
        .and_then(|s| s.0.texture_of(entry.stationery))
        .map(str::to_string);

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
    // The invoice names it with the roll's suffix (`0x4af430` → `0x5d8b00`).
    let notice_item_name = auction.and_then(|a| {
        items
            .template(a.entry, 0, ask)
            .map(|t| crate::items::item_display_name(&t.name, a.random_property, rolls.props))
    });
    // The list parse's rewrite stands, masked; else the template's callback rewrites the raw
    // triplet once the item lands, unmasked, and until then the list's masked triplet shows.
    let subject = match listed {
        Some(l) if l.rewritten => l.text.clone(),
        _ => auction
            .and_then(|_| auction_subject(&entry.subject, items, rolls.props, ask, get_text))
            .or_else(|| listed.map(|l| l.text.clone()))
            .unwrap_or_else(|| entry.subject.clone()),
    };
    let auction_invoice = auction
        .filter(|a| matches!(a.result, auction_mail::WON | auction_mail::SOLD))
        .and_then(|a| {
            let seller = a.result == auction_mail::SOLD;
            let n = invoice::parse_body(raw_body?, seller)?;
            // An unresolved counterparty fills in when its name lands, as in the reference.
            let player_name = names.resolve_player(n.player_guid, ask)?.to_string();
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
        mail_id: entry.message_id,
        item_text_id: entry.item_text_id,
        stationery: entry.stationery,
        package_icon,
        stationery_icon,
        sender,
        subject,
        money: entry.money,
        cod: entry.cod,
        days_left: entry.expire_days,
        item_count,
        was_read: entry.checked & CHECKED_READ != 0,
        was_returned: returned,
        text_created: entry.checked & CHECKED_COPIED != 0,
        cod_payment: entry.checked & CHECKED_COD_PAYMENT != 0,
        message_type: entry.message_type,
        mail_template_id: entry.mail_template_id,
        can_reply,
        is_gm,
        body: letter_body(entry, raw_body, templates, macros),
        stationery_texture,
        // Won and sold notices only, not every auction mail: the reference's `isInvoice` reads
        // what its subject parser persists for those two alone.
        is_invoice: auction
            .is_some_and(|a| matches!(a.result, auction_mail::WON | auction_mail::SOLD)),
        invoice: auction_invoice,
        item_id,
        item_random_property_id: item_roll,
        item_charges,
        item_durability,
        item_name,
        item_texture,
        item_quality,
    }
}

/// `GetInboxText`'s body (`0x4af193`-`0x4af26e`): with a text id, its cached text, nil until it
/// lands (`0x4af1c9`) and for an auction mail (`0x4af1cf`); with none, the template's body in the
/// client's locale (`0x4af217`-`0x4af24e`), nil for an id the store lacks (`0x4af229`); with
/// neither, nil. Either is expanded for the local player (`0x4af1f6`, `0x4af261`).
fn letter_body(
    entry: &MailListEntry,
    raw_body: Option<&String>,
    templates: Option<&MailTemplates>,
    macros: &crate::npc_text::MacroContext,
) -> Option<String> {
    let text = if entry.item_text_id != 0 {
        raw_body.filter(|_| entry.message_type != mail_message_type::AUCTION)
    } else if entry.mail_template_id != 0 {
        templates.and_then(|t| t.0.get(&entry.mail_template_id))
    } else {
        None
    };
    text.map(|b| crate::npc_text::substitute(b, macros))
}

/// The mail session and its arrival state, which the call host writes too ([`MailVerbs`]): a system
/// holding them runs its Lua as an engine turn ([`VmWith`]).
type MailSession = (ResMut<'static, MailOpen>, ResMut<'static, MailPending>);

/// The usable stationery list (`0x4ad970`): rows always available (`Flags & 1`) or whose item the
/// player carries in bags (`0x622270`), with a cached template, in the comparator `0x4ada90`'s
/// order: carried first (`0x4adab5`-`0x4adae7`), then the lower buy price, signed (`0x4adb81`), then
/// the name, ASCII case folded (`SStrCmpI` at `0x4adba6`). A carried paper has no `cost`. The templates are only read (`0x4ada51`): the mail init asks
/// for them ([`send_query_next_mail_time_on_enter`]).
pub(super) fn stationeries(
    catalog: &Stationery,
    self_q: &Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
) -> Vec<StationeryView> {
    let store = self_q.iter().next().map(|(s, _)| s);
    let mut out: Vec<((bool, i32, String), StationeryView)> = Vec::new();
    for row in catalog.0.rows() {
        let Some(t) = items.template(row.item, 0, Ask::Read).cloned() else {
            continue;
        };
        let carried = store.is_some_and(|s| {
            crate::ui_items::count_of(
                &s.0,
                objects,
                row.item,
                crate::ui_items::InventoryScope::DEFAULT,
            ) > 0
        });
        if row.flags & 1 == 0 && !carried {
            continue;
        }
        out.push((
            (!carried, t.buy_price as i32, t.name.to_ascii_lowercase()),
            StationeryView {
                id: row.id,
                name: t.name.clone(),
                icon: crate::ui_items::item_icon(icons, t.display_info_id).unwrap_or_default(),
                cost: (!carried).then_some(t.buy_price),
                texture: row.texture.clone(),
            },
        ));
    }
    // The id breaks what the comparator leaves equal, which its sort (`0x73f727`) leaves in no
    // order the reference fixes.
    out.sort_by(|(a, v), (b, w)| a.cmp(b).then(v.id.cmp(&w.id)));
    out.into_iter().map(|(_, v)| v).collect()
}

fn snapshot(
    mail: &MailOpen,
    items: &Items,
    icons: Option<&ItemDisplays>,
    stationery: Option<&Stationery>,
    templates: Option<&MailTemplates>,
    catalogs: RowCatalogs,
    names: &NameCache,
    ask: Ask<'_>,
    rolls: crate::items::RollCatalogs,
    macros: &crate::npc_text::MacroContext,
    get_text: &dyn Fn(&str) -> Option<String>,
) -> Option<MailState> {
    Some(MailState {
        open: mail.mailbox.is_some(),
        inbox: mail
            .mails
            .iter()
            .map(|e| {
                resolve_row(
                    e,
                    &mail.bodies,
                    mail.auction_subjects.get(&e.message_id),
                    items,
                    icons,
                    stationery,
                    templates,
                    catalogs,
                    names,
                    ask,
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
    packages: Option<Res<'w, Packages>>,
    /// The package rows went to this VM.
    packages_pushed: Local<'s, crate::ui_script::VmMemo<bool>>,
    closes: CoreCloses<'w, 's>,
    unit_names: crate::ui_unit::NameTables<'w>,
    templates: Option<Res<'w, MailTemplates>>,
    houses: Option<Res<'w, crate::ui_auction::AuctionHouses>>,
    gameobjects: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    /// The diff fired `MAIL_INBOX_UPDATE` last frame, which the stock inbox answers with its
    /// getters, as it does the reference's cache callbacks' (`0x4af080`, `0x4af0a0` on status 1).
    inbox_updated: Local<'s, crate::ui_script::VmMemo<bool>>,
}

/// `0x4ad050`'s order for each result: the arm's message line and a send's compose reset
/// (`0x4ad0ca`), the arm's own events, a purged row's `CLOSE_INBOX_ITEM(index)` (money `0x4ad772`,
/// item `0x4ad87d`) and its `MAIL_INBOX_UPDATE` (`0x4ad7a3`, `0x4ad8ae`, `0x4ad8fc`, `0x4ad966`),
/// then the tail: the send-pending flag (`0x4ad159`) and the request lock (`0x4ad164`) lift, so a
/// `MAIL_FAILED` handler can send the next request, and `MAIL_FAILED` on every path (`0x4ad15f`).
/// Returns the `MAIL_INBOX_UPDATE`s fired.
fn announce_results(
    script: &mut UiScript,
    results: Vec<MailResult>,
    mut show: impl FnMut(&mut UiScript, Option<crate::ui_action::Shown>),
) -> u32 {
    let mut updates = 0;
    for result in results {
        let line = result
            .line
            .and_then(|key| crate::ui_action::keyed_line(script, key));
        show(script, line);
        if result.send == Some(true) {
            script.reset_compose_tab();
        }
        if let Some(index) = result.close_inbox {
            script.fire_event("CLOSE_INBOX_ITEM", vec![ScriptValue::Int(i64::from(index))]);
        }
        if result.inbox_update {
            updates += 1;
            script.fire_event("MAIL_INBOX_UPDATE", vec![]);
        }
        if result.send.is_some() {
            script.mail_send_resolved();
        }
        script.mail_request_resolved();
        script.fire_event("MAIL_FAILED", vec![]);
    }
    updates
}

/// The list parse's tail: the request lock clears (`0x4ad5d0`), then `MAIL_INBOX_UPDATE`
/// (`0x4ad5d6`), once per result.
fn announce_lists(script: &mut UiScript, results: u32) {
    for _ in 0..results {
        script.mail_request_resolved();
        script.fire_event("MAIL_INBOX_UPDATE", vec![]);
    }
}

fn feed_mail(
    mut vm: VmWith<MailSession>,
    objects: Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    stationery: Option<Res<Stationery>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    time: Res<Time>,
    mut last: crate::ui_script::WindowMemo<Option<MailState>>,
    mut last_mailbox: crate::ui_script::WindowMemo<Option<u64>>,
    mut last_has_new_mail: Local<crate::ui_script::VmMemo<bool>>,
    // The local player, the letter body's `$`-macro subject.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
    extras: MailFeedExtras,
) {
    // The session is read and written first, with no Lua running; then the host is lent around
    // the pushes and signals, so each handler's verb applies inside its call and reads the request
    // lock as the reference's tail leaves it at that point (`0x4ad164`, `0x4ad5d0`).
    let (Some(script), set) = vm.parts() else {
        return;
    };
    let MailFeedExtras {
        props,
        enchants,
        mut sink,
        stationeries: mut last_stationeries,
        packages,
        mut packages_pushed,
        mut closes,
        unit_names,
        templates,
        houses,
        gameobjects,
        mut inbox_updated,
    } = extras;
    let rolls = crate::items::RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    let last = last.get(script);
    let last_mailbox = last_mailbox.get(script);
    let last_has_new_mail = last_has_new_mail.get(script);
    if closes.since_last(script, NpcWindow::Mail) {
        *last = None;
        *last_mailbox = None;
    }
    let inbox_updated = inbox_updated.get(script);

    let (fresh, opened, mailbox, show_requested, lists, results, resolved, has_new_mail, notify) = {
        let (mut mail, mut pending) = set.p0();
        // `None` until the player is named; the diff re-substitutes an open letter when it lands.
        let subject = crate::npc_text::player_identity(&self_q, &names, unit_names.get());
        let macros = crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states: &states,
        };
        // A close is the core's, which told the VM; the leave-world sweep's and a disconnect's
        // tell it nothing.
        let opened = last_mailbox.is_none() && mail.mailbox.is_some();
        // The frames the stock inbox calls its getters on: `MAIL_SHOW` and the
        // `MAIL_INBOX_UPDATE` of a list or a result arm, with the list's subject parse
        // (`0x4ad5a0`) besides, and the frame after the diff's own `MAIL_INBOX_UPDATE`.
        let rows_event = opened
            || mail.show_requested
            || mail.list_results > 0
            || mail.results.iter().any(|r| r.inbox_update)
            || std::mem::take(inbox_updated);
        let get_text = |key: &str| script.lua().globals().get::<String>(key).ok();
        let fresh = snapshot(
            &mail,
            &items,
            icons.as_deref(),
            stationery.as_deref(),
            templates.as_deref(),
            RowCatalogs {
                packages: packages.as_deref(),
                houses: houses.as_deref(),
                gameobjects: gameobjects.as_deref(),
            },
            &names,
            Ask::at(rows_event, &commands),
            rolls,
            &macros,
            &get_text,
        );
        // `HasNewMail()` is pushed on change, but `UPDATE_PENDING_MAIL` fires only at the
        // reference's three sites, so a query's "no mail" stamp does not move the icon until the
        // reply lands.
        pending.step(time.delta_secs());
        (
            fresh,
            opened,
            mail.mailbox,
            std::mem::take(&mut mail.show_requested),
            std::mem::take(&mut mail.list_results),
            std::mem::take(&mut mail.results),
            std::mem::take(&mut mail.request_resolved),
            pending.has_new_mail(),
            pending.take_notify(),
        )
    };
    // The usable stationery is asked ahead of any mailbox so the list is whole when
    // `SendMailFrame_Reset` selects row 1 on show; an empty list leaves every send unsent. Only
    // with a player: an ask sent before the socket exists is dropped and never re-asked.
    let usable = stationery
        .as_deref()
        .filter(|_| !self_q.is_empty())
        .map(|catalog| stationeries(catalog, &self_q, &objects, &items, icons.as_deref()))
        .unwrap_or_default();

    let mut host = set.p1();
    crate::script_calls::lent(script, host.as_mut(), |script| {
        if let Some(packages) = packages.as_deref() {
            let pushed = packages_pushed.get(script);
            if !*pushed {
                *pushed = true;
                script.set_mail_packages(packages.0.iter().map(package_view).collect());
            }
        }
        let changed = fresh != *last;
        // What the calls left in the VM already (a read mark, `0x4add62`) is no change to it.
        let news = changed && !script.holds_mail(&fresh);
        if changed {
            script.set_mail(fresh.clone());
        }
        // The results read the rows as their arms left them (`0x4ad6e8`, `0x4ad7e8`, `0x4ad933`).
        let arm_updates = announce_results(script, results, |script, line| {
            crate::ui_action::show_messages(script, &mut sink, "ui_mail", line)
        });
        let memo = last_stationeries.get(script);
        if *memo != usable {
            *memo = usable.clone();
            script.set_mail_stationeries(usable);
        }
        if opened {
            // The open core (`0x4acd10`) resets the compose tab before `MAIL_SHOW`, the stationery
            // selection with it (`0x4ace07`), which `MAIL_SHOW`'s reset re-picks as row 1; the
            // reset's `MAIL_SEND_SUCCESS`, page-turn sound and all, is the reference's too.
            script.reset_compose_tab();
            script.fire_event("MAIL_SHOW", vec![]);
        } else if mailbox.is_some() {
            // A re-click re-shows the open window; `MAIL_SHOW`'s `CheckInbox` refreshes the list.
            if show_requested {
                script.fire_event("MAIL_SHOW", vec![]);
            }
            // A change no result announced. The reference signals at its cache callbacks: an
            // auction subject's item (`0x4ad010`), a sender name (`0x4af080`), an item template
            // (`0x4af0a0`), a letter body (`0x4af340`) and an invoice name (`0x4af5b0`). This
            // snapshot diff fires on more than those sites: any row field that moved without a
            // result, such as a name the cache resolved before the listener asked. A list or an
            // arm that fired this frame already showed the rows as they stand.
            if news && lists == 0 && arm_updates == 0 {
                script.fire_event("MAIL_INBOX_UPDATE", vec![]);
                *inbox_updated = true;
            }
        }
        // Every list result ends its parse in the event (`0x4ad5c6`-`0x4ad5d6`), changed or not,
        // and whether or not the window is up.
        announce_lists(script, lists);
        // A result a same-frame close dropped still lifts the lock (the flag outlives the close).
        if resolved {
            script.mail_request_resolved();
        }
        *last = fresh;
        *last_mailbox = mailbox;
        if has_new_mail != *last_has_new_mail {
            script.set_has_new_mail(has_new_mail);
            *last_has_new_mail = has_new_mail;
        }
        if notify {
            script.fire_event("UPDATE_PENDING_MAIL", vec![]);
        }
    })
}

/// `CloseMail` into the close core, which sends nothing; every other mail verb applies inside its
/// call ([`MailVerbs`]).
fn drain_mail(mut hosted_vm: crate::script_calls::HostedVm, mut world_commands: Commands) {
    hosted_vm.run(|script| {
        // With no session a stray close still runs the core, which finds no cell (`0x4acd7c`).
        if script.is_some_and(|script| script.take_mail_close()) {
            crate::ui_session::queue(&mut world_commands, close_core);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::MailAttachment;
    use benilla_ui::script::{MailCall, MailRowVerb};

    /// `0x4ad9fc` finds the paper with `0x622270` at mask 0: a copy worn in a slot or in the
    /// keyring is carried (no cost line), one in the bank is not (the paper costs its price).
    #[test]
    fn a_paper_worn_or_in_the_keyring_is_carried_and_one_in_the_bank_is_not() {
        use bevy::ecs::system::RunSystemOnce;
        const PAPER: u32 = 9_311;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut items = Items::default();
        let mut paper = crate::items::test_template("Blank Paper");
        paper.buy_price = 40;
        items.insert_template(PAPER, Some(paper));
        app.init_resource::<crate::net::GuidIndex>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx));
        crate::items::test_spawn_item(
            app.world_mut(),
            0x4000_0000_0000_0001,
            benilla_protocol::ObjectFields::from_pairs(&[(3, PAPER), (14, 1)]),
            false,
        );
        let player = app
            .world_mut()
            .spawn((
                SelfPlayer,
                crate::net::Guid(7),
                ObjectStore(Default::default()),
            ))
            .id();
        let catalog = std::sync::Arc::new(Stationery(
            benilla_formats::StationeryCatalog::from_rows(vec![benilla_formats::StationeryRow {
                id: 5,
                item: PAPER,
                texture: "PAPER".into(),
                flags: 0,
            }]),
        ));
        let mut costs = |slot: u16| -> Vec<Option<u32>> {
            let lo = 486 + 2 * slot;
            app.world_mut().entity_mut(player).insert(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (lo, 0x4000_0000_0000_0001u64 as u32),
                    (lo + 1, 0x4000_0000),
                ]),
            ));
            let catalog = std::sync::Arc::clone(&catalog);
            app.world_mut()
                .run_system_once(
                    move |self_q: Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
                          objects: Objects,
                          items: Res<Items>| {
                        stationeries(&catalog, &self_q, &objects, &items, None)
                            .into_iter()
                            .map(|v| v.cost)
                            .collect::<Vec<_>>()
                    },
                )
                .unwrap()
        };
        assert_eq!(costs(23), [None], "the backpack, as before");
        assert_eq!(costs(17), [None], "worn");
        assert_eq!(costs(81), [None], "the keyring");
        assert!(
            costs(39).is_empty(),
            "the bank is outside mask 0, and the paper is not free"
        );
    }

    /// The usable list's order is the comparator `0x4ada90`'s: a carried paper first
    /// (`0x4adab5`-`0x4adae7`), then the lower buy price (`0x4adb81`-`0x4adb89`), then the name,
    /// case-insensitively (`0x4adba6`).
    #[test]
    fn the_usable_papers_sort_carried_first_then_by_price_then_by_name() {
        use bevy::ecs::system::RunSystemOnce;
        const PACK_SLOT_1: u16 = 486 + 2 * 23;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut items = Items::default();
        for (entry, name, price) in [(1, "Zeta", 10), (2, "alpha", 10), (3, "Mid", 50)] {
            let mut t = crate::items::test_template(name);
            t.buy_price = price;
            items.insert_template(entry, Some(t));
        }
        app.init_resource::<crate::net::GuidIndex>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx));
        crate::items::test_spawn_item(
            app.world_mut(),
            0x4000_0000_0000_0003,
            benilla_protocol::ObjectFields::from_pairs(&[(3, 3), (14, 1)]),
            false,
        );
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Guid(7),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (PACK_SLOT_1, 0x4000_0000_0000_0003u64 as u32),
                (PACK_SLOT_1 + 1, 0x4000_0000),
            ])),
        ));
        let row = |id: u32, item: u32, flags: u32| benilla_formats::StationeryRow {
            id,
            item,
            texture: format!("T{id}"),
            flags,
        };
        let catalog = Stationery(benilla_formats::StationeryCatalog::from_rows(vec![
            row(11, 1, 1),
            row(12, 2, 1),
            row(13, 3, 0),
        ]));
        let order = app
            .world_mut()
            .run_system_once(
                move |self_q: Query<(&ObjectStore, &crate::net::Guid), With<SelfPlayer>>,
                      objects: Objects,
                      items: Res<Items>| {
                    stationeries(&catalog, &self_q, &objects, &items, None)
                        .into_iter()
                        .map(|v| v.id)
                        .collect::<Vec<_>>()
                },
            )
            .unwrap();
        assert_eq!(order, [13, 12, 11]);
    }

    /// `GetPackageInfo`'s icon is the basename under `Interface\Icons` (`0x4ae4c6`, `0x4ae4eb`).
    #[test]
    fn a_package_row_answers_its_icon_under_the_icon_folder() {
        let view = package_view(&benilla_formats::PackageRow {
            id: 2,
            icon: "INV_BOX_04".into(),
            cost: 10,
            name: "Test Package".into(),
        });
        assert_eq!(view.icon, "Interface\\Icons\\INV_BOX_04");
        assert_eq!(
            (view.id, view.cost, view.name.as_str()),
            (2, 10, "Test Package")
        );
    }

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

    pub(super) fn entry(
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
            package: 0,
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
        m.show_requested = false;
        // The same mailbox keeps the rows, and re-requests the show.
        m.click(0x100);
        assert_eq!(m.mails.len(), 1);
        assert!(m.show_requested);
        // Another mailbox keeps them too: the open core stores only the guid (`0x4acd27`).
        m.click(0x200);
        assert_eq!(m.mails.len(), 1);
    }

    /// The read mark's record half (`0x4adcd0`): the READ bit (`0x4add62`), and days left above 3
    /// lowered to 3 (`0x4add81`-`0x4add8e`), not on a COD mail (`0x4add72`) nor below 3.
    #[test]
    fn the_read_mark_lowers_the_days_left_of_a_mail_with_no_cod() {
        let mut m = MailOpen::default();
        let mut cod = entry(43, Some(0xA), 0, 10.0);
        cod.cod = 100;
        m.mails = vec![entry(42, Some(0xA), 0, 30.0), cod, entry(44, None, 0, 2.5)];
        for id in [42, 43, 44, 45] {
            m.mark_read(id);
        }
        let marked: Vec<(u32, f32)> = m
            .mails
            .iter()
            .map(|e| (e.checked & CHECKED_READ, e.expire_days))
            .collect();
        assert_eq!(marked, [(1, 3.0), (1, 10.0), (1, 2.5)]);
    }

    /// The world-enter init (`0x4acb10`), which a `/reload` and a worldport reach as well as a
    /// login, drops the rows and the mailbox and lifts the request lock and the list throttle
    /// (`0x4acb7d`, `0x4acb82`); nothing else resets them.
    #[test]
    fn the_world_enter_init_resets_the_rows_lock_and_throttle() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .add_message::<WorldEnterCascadeMessage>()
            .add_systems(Update, send_query_next_mail_time_on_enter);
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script.set_mail(Some(MailState {
            open: true,
            ..Default::default()
        }));
        script.run("CheckInbox()").unwrap();
        assert_eq!(script.take_mail_calls(), [MailCall::CheckInbox]);
        app.insert_non_send_resource(script);
        {
            let mut mail = app.world_mut().resource_mut::<MailOpen>();
            mail.click(0x20);
            mail.mails.push(entry(1, Some(0xA), 0, 30.0));
        }
        let check = |app: &mut App| {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.run("CheckInbox()").unwrap();
            script.take_mail_calls() == [MailCall::CheckInbox]
        };
        assert!(!check(&mut app), "the lock holds");
        app.update();
        assert!(!check(&mut app), "no cascade, no reset");
        app.world_mut().write_message(WorldEnterCascadeMessage);
        app.update();
        let mail = app.world().resource::<MailOpen>();
        assert!(mail.mails.is_empty() && mail.mailbox.is_none());
        assert!(check(&mut app), "the lock and the throttle start over");
    }

    /// A close forgets the mailbox and keeps the rows (`0x4acd71`); the world-enter init and a
    /// disconnect clear them (`0x4acb10`).
    #[test]
    fn a_close_keeps_the_rows_and_the_session_end_clears_them() {
        let mut m = MailOpen::default();
        m.click(0x1);
        m.mails.push(entry(1, Some(0xA), 0, 30.0));
        m.bodies.insert(7, "hi".into());
        m.clear();
        assert!(m.mailbox.is_none());
        assert_eq!(m.mails.len(), 1);
        assert!(!m.bodies.is_empty());
        m.clear_session();
        assert!(m.mails.is_empty());
        assert!(m.bodies.is_empty());
    }

    /// No GlobalStrings in a unit test: every key misses.
    fn no_strings(_key: &str) -> Option<String> {
        None
    }

    #[test]
    fn resolve_row_derives_the_reply_law() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let bodies = QueryCache::default();

        // A plain letter from a player: replyable.
        let from_player = entry(1, Some(0xA), 0, 30.0);
        let row = resolve_row(
            &from_player,
            &bodies,
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_reply);
        assert_eq!(row.stationery_texture, None, "no store, no texture");

        // A player mail still carrying money: replyable.
        let mut with_money = entry(4, Some(0xA), 0, 30.0);
        with_money.money = 500;
        let row = resolve_row(
            &with_money,
            &bodies,
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_reply);

        // A returned mail: replyable still (`0x4aefa5`-`0x4aefbc` test no returned bit).
        let returned = entry(2, Some(0xA), CHECKED_RETURNED, 30.0);
        let row = resolve_row(
            &returned,
            &bodies,
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(row.can_reply);

        // A creature's mail: not replyable.
        let mut system = entry(3, None, 0, 30.0);
        system.message_type = mail_message_type::CREATURE;
        let row = resolve_row(
            &system,
            &bodies,
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert!(!row.can_reply);
    }

    /// `GetInboxHeaderInfo`'s reply flag is a player's mail off the GM stationery
    /// (`0x4aefa5`-`0x4aefbc`), with no returned test; a player's GM letter names its sender
    /// `GM_EMAIL_NAME` (`0x4aedbd`-`0x4aedcb`, jump table `0x4af068`).
    #[test]
    fn a_gm_letter_is_from_gm_email_name_and_a_returned_mail_can_be_answered() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let bodies = QueryCache::default();
        let strings = |key: &str| (key == "GM_EMAIL_NAME").then(|| "Customer Support".to_string());
        let row_of = |e: &MailListEntry| {
            resolve_row(
                e,
                &bodies,
                None,
                &items,
                None,
                None,
                None,
                RowCatalogs::default(),
                &names,
                Ask::New(&commands),
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &strings,
            )
        };
        let mut gm = entry(1, Some(0xA), 0, 30.0);
        gm.stationery = STATIONERY_GM;
        let row = row_of(&gm);
        assert_eq!(
            (row.sender.as_deref(), row.can_reply, row.is_gm),
            (Some("Customer Support"), false, true)
        );
        let returned = entry(2, Some(0xA), CHECKED_RETURNED, 30.0);
        assert!(row_of(&returned).can_reply);
    }

    /// `GetInboxText`'s texture is nil for a stationery id the store lacks (`0x4af284`-`0x4af2a9`).
    #[test]
    fn a_stationery_the_store_lacks_has_no_texture() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let bodies = QueryCache::default();
        let catalog = Stationery(benilla_formats::StationeryCatalog::from_rows(vec![
            benilla_formats::StationeryRow {
                id: 41,
                item: 9311,
                texture: "STATIONERYTEST".into(),
                flags: 1,
            },
        ]));
        let texture = |id: u32| {
            let mut e = entry(1, Some(0xA), 0, 30.0);
            e.stationery = id;
            resolve_row(
                &e,
                &bodies,
                None,
                &items,
                None,
                Some(&catalog),
                None,
                RowCatalogs::default(),
                &names,
                Ask::New(&commands),
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &no_strings,
            )
            .stationery_texture
        };
        assert_eq!(texture(41).as_deref(), Some("STATIONERYTEST"));
        assert_eq!(texture(77), None);
    }

    /// `GetInboxHeaderInfo`'s first icon is the package's when the row carries a package id the
    /// store holds (`[rec+0x118]`, `0x4aec76`-`0x4aec9c`), else its item's (`[rec+0x120]`,
    /// `0x4aeca6`-`0x4aecd8`), whatever the paper; the second is the paper's item's icon
    /// (`[rec+0x11c]` → the store row's item, `0x4aed25`-`0x4aed74`), nil for a paper the store
    /// lacks or an item not yet held.
    #[test]
    fn the_header_icons_read_the_package_and_the_paper_item() {
        use benilla_formats::{ItemDisplay, ItemDisplayCatalog};
        let states = crate::world_state::WorldStates::default();
        let mut items = Items::default();
        for (item, display) in [(9311, 5), (18154, 6), (2589, 7)] {
            let mut t = crate::items::test_template("Paper");
            t.display_info_id = display;
            items.insert_template(item, Some(t));
        }
        let icons = ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(
            std::collections::HashMap::from([5, 6, 7].map(|id| {
                (
                    id,
                    ItemDisplay {
                        icon: Some(format!("ICON{id}")),
                        ..Default::default()
                    },
                )
            })),
        ));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let bodies = QueryCache::default();
        let row = |id, item| benilla_formats::StationeryRow {
            id,
            item,
            texture: "STATIONERYTEST".into(),
            flags: 1,
        };
        let catalog = Stationery(benilla_formats::StationeryCatalog::from_rows(vec![
            row(41, 9311),
            row(61, 18154),
            row(42, 9999),
        ]));
        let packages = Packages(vec![benilla_formats::PackageRow {
            id: 2,
            icon: "INV_BOX_04".into(),
            cost: 10,
            name: "Gift".into(),
        }]);
        let icons_of = |stationery: u32, package: u32, item: bool| {
            let mut e = entry(1, Some(0xA), 0, 30.0);
            (e.stationery, e.package) = (stationery, package);
            if item {
                e.item = Some(MailAttachment {
                    entry: 2589,
                    perm_enchant: 0,
                    random_prop_id: 0,
                    suffix_factor: 0,
                    count: 1,
                    charges: 0,
                    durability_max: 0,
                    durability: 0,
                });
            }
            let r = resolve_row(
                &e,
                &bodies,
                None,
                &items,
                Some(&icons),
                Some(&catalog),
                None,
                RowCatalogs {
                    packages: Some(&packages),
                    ..Default::default()
                },
                &names,
                Ask::New(&commands),
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &no_strings,
            );
            (r.package_icon, r.stationery_icon)
        };
        let s = |v: &str| Some(v.to_string());
        let gift = s("Interface\\Icons\\INV_BOX_04");
        assert_eq!(icons_of(41, 2, true), (gift.clone(), s("ICON5")));
        assert_eq!(icons_of(41, 2, false), (gift, s("ICON5")), "no item asked");
        assert_eq!(
            icons_of(41, 3, true),
            (s("ICON7"), s("ICON5")),
            "no such package"
        );
        assert_eq!(
            icons_of(61, 0, true),
            (s("ICON7"), s("ICON6")),
            "a GM row's item"
        );
        assert_eq!(icons_of(61, 0, false), (None, s("ICON6")));
        assert_eq!(icons_of(77, 0, false), (None, None), "no such paper");
        assert_eq!(icons_of(42, 0, false), (None, None), "its item not held");
    }

    /// `GetInboxHeaderInfo` names a non-player sender from its store by the row's sender id
    /// (`[rec+0x10]`, jump table `0x4af068`): an auction house's `AuctionHouse.dbc` name
    /// (`0x4aee11`), a creature's (`0x4aee48`) and a game object's (`0x4aee78`) cached names, each
    /// nil until held.
    #[test]
    fn a_non_player_sender_is_named_from_its_store() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        names.insert_creature(
            3,
            Some(crate::names::CreatureRecord {
                name: "Innkeeper Farley".into(),
                subname: None,
                creature_type: 7,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                pet_spell_list: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }),
        );
        let mut gameobjects = crate::go_templates::GameObjectTemplates::default();
        gameobjects.insert(4, 19, "Mailbox".into(), &[0; 24]);
        let houses = crate::ui_auction::AuctionHouses(
            benilla_formats::AuctionHouseCatalog::from_houses(std::collections::HashMap::from([(
                1,
                benilla_formats::AuctionHouseInfo {
                    faction: 12,
                    deposit_percent: 15,
                    cut_percent: 5,
                    name: "Stormwind Auction House".into(),
                },
            )])),
        );
        let bodies = QueryCache::default();
        let sender = |message_type: u8, sender_id: u32| {
            let mut e = entry(1, None, 0, 30.0);
            (e.message_type, e.sender_id) = (message_type, Some(sender_id));
            resolve_row(
                &e,
                &bodies,
                None,
                &items,
                None,
                None,
                None,
                RowCatalogs {
                    houses: Some(&houses),
                    gameobjects: Some(&gameobjects),
                    ..Default::default()
                },
                &names,
                Ask::New(&commands),
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &no_strings,
            )
            .sender
        };
        assert_eq!(
            sender(mail_message_type::AUCTION, 1).as_deref(),
            Some("Stormwind Auction House")
        );
        assert_eq!(
            sender(mail_message_type::CREATURE, 3).as_deref(),
            Some("Innkeeper Farley")
        );
        assert_eq!(
            sender(mail_message_type::GAMEOBJECT, 4).as_deref(),
            Some("Mailbox")
        );
        let _ = rx.try_iter().count();
        assert_eq!(sender(mail_message_type::AUCTION, 2), None);
        assert_eq!(sender(mail_message_type::CREATURE, 30), None);
        assert_eq!(sender(mail_message_type::GAMEOBJECT, 40), None);
        let asked: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            asked.len(),
            2,
            "the creature and the object are asked: {asked:?}"
        );
    }

    /// An auction notice names its item as `0x5d8b00` does, with the roll's suffix, in the
    /// subject (`0x4acf5d`) and the invoice (`0x4af430`, from `[rec+0x250]` and `[rec+0x254]`).
    #[test]
    fn an_auction_notice_names_its_item_with_the_suffix() {
        let states = crate::world_state::WorldStates::default();
        let mut items = Items::default();
        items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
        let props = crate::items::RandomProperties(
            benilla_formats::RandomPropertyCatalog::from_rows(std::collections::HashMap::from([(
                7,
                benilla_formats::RandomProperty {
                    suffix: "of the Bear".into(),
                    ..Default::default()
                },
            )])),
        );
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        names.insert_player(0x42, "Bob".into(), None);
        let mut bodies = QueryCache::default();
        bodies.insert(99, "0000000000000042:500:0".into());
        let mut e = entry(1, None, 0, 30.0);
        (e.message_type, e.sender_id) = (mail_message_type::AUCTION, Some(1));
        (e.subject, e.item_text_id) = ("2589:7:1".into(), 99);
        let strings =
            |key: &str| (key == "AUCTION_WON_MAIL_SUBJECT").then(|| "Auction won: %s".to_string());
        let row = resolve_row(
            &e,
            &bodies,
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs {
                props: Some(&props),
                enchants: None,
            },
            &no_macros(&states),
            &strings,
        );
        assert_eq!(row.subject, "Auction won: Linen Cloth of the Bear");
        assert_eq!(
            row.invoice.map(|i| i.item_name).as_deref(),
            Some("Linen Cloth of the Bear")
        );
    }

    #[test]
    fn resolve_row_reads_the_item_and_body_caches() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let names = NameCache::default();
        let mut bodies = QueryCache::default();
        bodies.insert(99, "the letter body".into());

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
            None,
            &items,
            None,
            None,
            None,
            RowCatalogs::default(),
            &names,
            Ask::New(&commands),
            crate::items::RollCatalogs::NONE,
            &no_macros(&states),
            &no_strings,
        );
        assert_eq!(row.item_id, 2589);
        assert_eq!(row.item_count, 5);
        assert_eq!(row.body.as_deref(), Some("the letter body"));
        assert_eq!(row.item_text_id, e.item_text_id);
        // The item template has not answered yet.
        assert!(row.item_name.is_none());
    }

    /// The feed and the sweep's mail leg, the feed's other inputs at rest.
    fn sweep_mail_app() -> App {
        use crate::ui_script::SweepApp;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<Time>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx))
            .add_sweep_leg(SweepLeg::Mail, close_mail_on_leave_world)
            .add_systems(Update, feed_mail);
        app
    }

    const EVENTS: [&str; 2] = ["MAIL_SHOW", "MAIL_CLOSED"];
    const MAILBOX: u64 = 0xF110_0000_0000_0B0C;

    /// A `/reload` at an open mailbox: the mail leg (`0x490b1b` → `0x4acd50(cl=1)`) forgets it
    /// with no `MAIL_CLOSED`, and the new VM shows nothing.
    #[test]
    fn a_reload_forgets_the_mailbox_silently_and_the_new_vm_shows_nothing() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        let mut app = sweep_mail_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.world_mut().resource_mut::<MailOpen>().click(MAILBOX);
        app.update();
        assert_eq!(seen_in(&app), ["MAIL_SHOW"]);

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(
            seen(&dying),
            ["MAIL_SHOW"],
            "no `MAIL_CLOSED` on the `cl` arm"
        );
        assert_eq!(app.world().resource::<MailOpen>().mailbox, None);

        app.update();
        assert!(
            seen_in(&app).is_empty(),
            "the new VM has no mailbox to show"
        );
    }

    /// A worldport's surviving VM: the leg's silence holds, and the feed adds no `MAIL_CLOSED`.
    #[test]
    fn a_worldport_forgets_the_mailbox_without_mail_closed() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let mut app = sweep_mail_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.world_mut().resource_mut::<MailOpen>().click(MAILBOX);
        app.update();
        sweep_in_place(&mut app);
        app.update();
        app.update();
        assert_eq!(seen_in(&app), ["MAIL_SHOW"]);
    }

    /// A `feed_mail` app whose listener logs the mail events the stock frame reacts to.
    fn feed_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<Time>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, feed_mail);
        let mut script = UiScript::new().unwrap();
        // No host here: the verbs' calls are kept.
        script.keep_calls();
        script
            .run(
                r#"LOG = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_INBOX_UPDATE")
                f:SetScript("OnEvent", function() table.insert(LOG, event) end)"#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn logged(app: &mut App) -> i64 {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<i64>("return getn(LOG)")
            .unwrap()
    }

    /// The inbox reads its rows' caches at the frames the stock frame calls `GetInboxHeaderInfo`
    /// on, which looks the sender and the item up with a callback (`0x4aedf6`, `0x4aecc7`):
    /// `MAIL_SHOW` and a list's `MAIL_INBOX_UPDATE`. The frames between never ask a refusal; the
    /// next list asks it, once.
    #[test]
    fn a_refused_sender_or_item_is_asked_at_the_next_list_never_by_a_frame() {
        let (mut app, rx) = feed_app();
        let commands = NetCommands(crossbeam_channel::unbounded().0);
        let mut row = entry(1, Some(0x77), 0, 30.0);
        row.item = Some(benilla_protocol::messages::MailAttachment {
            entry: 900,
            perm_enchant: 0,
            random_prop_id: 0,
            suffix_factor: 0,
            count: 1,
            charges: 0,
            durability_max: 0,
            durability: 0,
        });
        app.world_mut().resource_mut::<MailOpen>().click(0x20);
        super::net::mail_list(
            vec![row.clone()],
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        let asked = crate::query_cache::queries(&rx);
        assert_eq!(asked.len(), 2, "the sender and the item, once: {asked:?}");
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x77, String::new(), None);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(900, None);
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
        super::net::mail_list(
            vec![row],
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            2,
            "the next list asks both again"
        );
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
    }

    /// A mail with a template id and no text id takes its body from `MailTemplate.dbc`
    /// (`0x4af217`-`0x4af26e`), and its letter can be copied (`0x4af2c0`); an auction mail's text
    /// is no body (`0x4af1cf`), whatever its notice.
    #[test]
    fn a_template_letter_reads_its_body_from_the_template() {
        let (mut app, _rx) = feed_app();
        app.insert_resource(MailTemplates(
            [(87, "Thanks for the fish.".to_string())].into(),
        ));
        let mut template = entry(1, Some(0x77), 0, 30.0);
        template.mail_template_id = 87;
        // An outbid notice (result 0), which no invoice parse marks.
        let mut notice = entry(2, None, 0, 30.0);
        notice.message_type = mail_message_type::AUCTION;
        notice.subject = "2589:0:0".into();
        notice.item_text_id = 9;
        {
            let mut mail = app.world_mut().resource_mut::<MailOpen>();
            mail.click(0x20);
            mail.mails = vec![template, notice];
            mail.bodies.insert(9, "Outbid.".into());
        }
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .keep_calls();
        app.update();
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let (body, takeable): (Option<String>, bool) = script
            .eval("local b, _, t = GetInboxText(1) return b, t == 1")
            .unwrap();
        assert_eq!(body.as_deref(), Some("Thanks for the fish."));
        assert!(takeable);
        let notice: Option<String> = script.eval("return (GetInboxText(2))").unwrap();
        assert_eq!(notice, None);
    }

    /// A sender's name landing moves the row, and the diff's `MAIL_INBOX_UPDATE` stands for the
    /// reference's callback (`0x4af080`): the stock inbox answers it with `GetInboxHeaderInfo`,
    /// whose item lookup asks a refused item again (`0x4aecc7`), once, at the frame after.
    #[test]
    fn a_landed_sender_makes_the_next_frame_ask_a_refused_item_again() {
        let (mut app, rx) = feed_app();
        let commands = NetCommands(crossbeam_channel::unbounded().0);
        let mut row = entry(1, Some(0x77), 0, 30.0);
        row.item = Some(benilla_protocol::messages::MailAttachment {
            entry: 900,
            perm_enchant: 0,
            random_prop_id: 0,
            suffix_factor: 0,
            count: 1,
            charges: 0,
            durability_max: 0,
            durability: 0,
        });
        app.world_mut().resource_mut::<MailOpen>().click(0x20);
        super::net::mail_list(
            vec![row],
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 2);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(900, None);
        app.update();
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "a refusal waits"
        );
        let before = logged(&mut app);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x77, "Bob".into(), None);
        app.update();
        assert_eq!(logged(&mut app), before + 1, "the diff fires the update");
        app.update();
        let asked = crate::query_cache::queries(&rx);
        assert!(
            matches!(
                asked.as_slice(),
                [ClientCommand::ItemQuery { entry: 900, .. }]
            ),
            "the frame after asks the refused item: {asked:?}"
        );
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
    }

    /// The mail init looks every stationery's item up with a callback (`0x4acbba`); the list the
    /// feed builds only reads (`0x4ada51`).
    #[test]
    fn the_stationery_items_are_asked_at_the_world_enter_init_only() {
        let (mut app, rx) = feed_app();
        app.add_message::<WorldEnterCascadeMessage>()
            .insert_resource(Stationery(benilla_formats::StationeryCatalog::from_rows(
                vec![benilla_formats::StationeryRow {
                    id: 5,
                    item: 9_311,
                    texture: "PAPER".into(),
                    flags: 1,
                }],
            )))
            .add_systems(Update, send_query_next_mail_time_on_enter);
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Guid(7),
            ObjectStore(Default::default()),
        ));
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty(), "no frame asks");
        app.world_mut().write_message(WorldEnterCascadeMessage);
        app.update();
        assert!(matches!(
            crate::query_cache::queries(&rx)[..],
            [ClientCommand::ItemQuery { entry: 9_311, .. }]
        ));
    }

    /// Every list result ends in `MAIL_INBOX_UPDATE` (`0x4ad5c6`-`0x4ad5d6`), changed or not, so an
    /// addon that called `CheckInbox` hears the answer whatever it held.
    #[test]
    fn every_list_result_signals_the_inbox_update() {
        let (mut app, _rx) = feed_app();
        app.world_mut().resource_mut::<MailOpen>().click(0x20);
        app.update();
        assert_eq!(logged(&mut app), 0, "opening announces no list");
        let commands = NetCommands(crossbeam_channel::unbounded().0);
        let list = || vec![entry(1, Some(0xA), 0, 30.0)];

        net::mail_list(
            list(),
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        assert_eq!(logged(&mut app), 1, "the first list");
        net::mail_list(
            list(),
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        assert_eq!(logged(&mut app), 2, "the same list again is still a result");
        app.update();
        assert_eq!(logged(&mut app), 2, "and one result is one signal");

        // Two results in one frame signal twice.
        for _ in 0..2 {
            net::mail_list(
                list(),
                &mut app.world_mut().resource_mut::<MailOpen>(),
                &commands,
                |_| false,
                None,
                |_| None,
                |_| {},
            );
        }
        app.update();
        assert_eq!(logged(&mut app), 4);

        // A take edits its row in place and fires its arm's own event (`0x4ad7a3`), once: the
        // row's change is not announced a second time, and no list is asked for.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut equip = crate::ui_items::EquipErrors::default();
        let mut paid = entry(1, Some(0xA), 0, 30.0);
        paid.money = 50;
        net::mail_list(
            vec![paid],
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &commands,
            |_| false,
            None,
            |_| None,
            |_| {},
        );
        app.update();
        assert_eq!(logged(&mut app), 5);
        net::send_mail_result(
            1,
            benilla_protocol::messages::mail_action::MONEY_TAKEN,
            mail_error::OK,
            None,
            None,
            &mut app.world_mut().resource_mut::<MailOpen>(),
            &NetCommands(tx),
            &mut equip,
            Default::default(),
        );
        app.update();
        assert_eq!(logged(&mut app), 6, "the arm's event, once");
        assert!(rx.try_iter().next().is_none(), "nothing asks for the list");
    }

    /// The row verbs a harness's VM kept since the last take, in call order.
    fn row_verbs(script: &mut UiScript) -> Vec<MailRowVerb> {
        script
            .take_mail_calls()
            .into_iter()
            .filter_map(|call| match call {
                MailCall::Row { verb, .. } => Some(verb),
                _ => None,
            })
            .collect()
    }

    /// The tail lifts the request lock before `MAIL_FAILED` (`0x4ad164`, `0x4ad16a`), so a handler
    /// of the event can send the next request: the pacing the event exists for.
    #[test]
    fn a_mail_failed_handler_can_send_the_next_request() {
        let mut script = paid_mail_script();
        script
            .run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_FAILED")
                f:SetScript("OnEvent", function() DeleteInboxItem(1) end)
                TakeInboxMoney(1)"#,
            )
            .unwrap();
        assert_eq!(
            row_verbs(&mut script),
            [MailRowVerb::TakeMoney],
            "the first request took the lock"
        );
        announce_results(&mut script, vec![MailResult::default()], |_, _| {});
        assert_eq!(
            row_verbs(&mut script),
            [MailRowVerb::Delete],
            "the handler's request got through"
        );
    }

    fn paid_mail_script() -> UiScript {
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        // A system mail with money, which a delete takes (`0x4afd57`).
        let paid = MailInboxRow {
            money: 5,
            message_type: 1,
            ..Default::default()
        };
        script.set_mail(Some(MailState {
            open: true,
            inbox: vec![paid],
        }));
        script
    }

    /// The lock [0xb6efc8] stays held through the arm and its `MAIL_INBOX_UPDATE`; it clears only
    /// at the tail (`0x4ad164`), so an inbox-update handler's request is refused (`0x4af90c`)
    /// while the `MAIL_FAILED` handler's goes out.
    #[test]
    fn the_arm_events_run_with_the_request_lock_held() {
        let mut script = paid_mail_script();
        script
            .run(
                r#"local u = CreateFrame("Frame")
                u:RegisterEvent("MAIL_INBOX_UPDATE")
                u:SetScript("OnEvent", function() DeleteInboxItem(1) end)
                local f = CreateFrame("Frame")
                f:RegisterEvent("MAIL_FAILED")
                f:SetScript("OnEvent", function() TakeInboxMoney(1) end)
                TakeInboxMoney(1)"#,
            )
            .unwrap();
        assert_eq!(row_verbs(&mut script), [MailRowVerb::TakeMoney]);
        let result = MailResult {
            inbox_update: true,
            ..Default::default()
        };
        announce_results(&mut script, vec![result], |_, _| {});
        assert_eq!(
            row_verbs(&mut script),
            [MailRowVerb::TakeMoney],
            "the arm's own event's delete is refused, the tail's take is not"
        );
    }

    /// A list parse clears the lock before its event (`0x4ad5d0`, `0x4ad5d6`).
    #[test]
    fn a_list_results_event_runs_with_the_lock_lifted() {
        let mut script = paid_mail_script();
        script
            .run(
                r#"local u = CreateFrame("Frame")
                u:RegisterEvent("MAIL_INBOX_UPDATE")
                u:SetScript("OnEvent", function() DeleteInboxItem(1) end)
                TakeInboxMoney(1)"#,
            )
            .unwrap();
        assert_eq!(row_verbs(&mut script), [MailRowVerb::TakeMoney]);
        announce_lists(&mut script, 1);
        assert_eq!(row_verbs(&mut script), [MailRowVerb::Delete]);
    }

    /// `0x4ad050` tests no mailbox: a result queued when the window closes still fires its events.
    #[test]
    fn a_close_keeps_the_queued_results_and_the_session_end_drops_them() {
        let mut m = MailOpen::default();
        m.click(0x1);
        m.results.push(MailResult::default());
        m.clear();
        assert_eq!(m.results.len(), 1);
        // The leave-world sweep's mail leg (`0x4acd50`, `cl=1`) forgets the mailbox the same way
        // and touches no pending result.
        m.click(0x1);
        m.clear();
        assert_eq!(m.mailbox, None);
        assert_eq!(m.results.len(), 1);
        m.clear_session();
        assert!(m.results.is_empty());
    }

    /// Through the whole feed: the system's `resolve_request` flag must not lift the lock ahead
    /// of the arm's events (`0x4ad6b0` holds `[0xb6efc8]` until `0x4ad164`).
    #[test]
    fn the_feed_keeps_the_lock_through_the_arm_events() {
        let (mut app, _rx) = feed_app();
        let mut row = entry(1, Some(0xA), 0, 30.0);
        row.money = 5;
        row.message_type = mail_message_type::AUCTION;
        {
            let mut mail = app.world_mut().resource_mut::<MailOpen>();
            mail.click(0x20);
            mail.mails.push(row);
        }
        app.update();
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script
                .run(
                    r#"DELETES = 0
                    local u = CreateFrame("Frame")
                    u:RegisterEvent("MAIL_INBOX_UPDATE")
                    u:SetScript("OnEvent", function() DeleteInboxItem(1) end)
                    TakeInboxMoney(1)"#,
                )
                .unwrap();
            assert_eq!(row_verbs(&mut script), [MailRowVerb::TakeMoney]);
        }
        {
            let mut mail = app.world_mut().resource_mut::<MailOpen>();
            mail.resolve_request();
            mail.results.push(MailResult {
                inbox_update: true,
                ..Default::default()
            });
        }
        app.update();
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert!(row_verbs(&mut script).is_empty(), "refused under the lock");
        // The lock came back by the end of the feed.
        script.run("DeleteInboxItem(1)").unwrap();
        assert_eq!(row_verbs(&mut script), [MailRowVerb::Delete]);
    }
}
