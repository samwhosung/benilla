//! The mailbox session: the app half of [`benilla_ui::script`]'s `mail` module. The getters read
//! it through the host at the call ([`live`]), and what its packets, its open and close cores and
//! its record callbacks signal fires on the mail lane at their place ([`lane`]).
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
use benilla_ui::script::ItemRead;
use bevy::prelude::*;
use std::collections::HashMap;

use benilla_ui::script::{MailInboxRow, ScriptValue, StationeryView, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::query_cache::QueryCache;
use crate::script_calls::VmWith;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

mod lane;
mod live;
mod net;
mod pending;
mod stationery;
#[cfg(test)]
mod verb_tests;
mod verbs;

pub(crate) use lane::{MailEvent, MailLane};
pub(crate) use live::MailLive;
pub(crate) use pending::MailPending;
pub(crate) use stationery::{count_papers_on_enter, StationeryRebuild, UsableStationery};
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

/// One `SMSG_SEND_MAIL_RESULT`, queued on the mail lane at its packet. The handler (`0x4ad050`)
/// ends every arm (send, take money, take item, return, delete, each success and each refusal)
/// at one tail that lifts the locks and signals `MAIL_FAILED` (`0x4ad15f`-`0x4ad16a`), so every
/// result owes the event, success included; the stock handler only re-enables the send button.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct MailResult {
    /// The message key of the line the arm shows: a send's "mail sent", or a refusal's, an equip
    /// error's through the equip-error table (`0x4ad12e` → `0x622630`).
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
/// letter-body cache and the getters' name and body callbacks. A close forgets the guid and keeps
/// the rows, as the reference's close does (`0x4acd71`); the world-enter init (`0x4acb10`) and a
/// disconnect clear everything but the bodies.
#[derive(Resource, Default)]
pub(crate) struct MailOpen {
    pub(crate) mailbox: Option<u64>,
    /// Wire order, which is the 1-based display order.
    pub(crate) mails: Vec<MailListEntry>,
    /// Letter bodies by `item_text_id`, asked once.
    pub(crate) bodies: QueryCache<u32, String>,
    /// The callbacks the getters' name and body lookups that missed keep, one per call, in the
    /// order they asked; the item lookups keep theirs on the item cache
    /// ([`crate::items::RecordCallback::Mail`]).
    waits: Vec<MailWait>,
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

/// What a getter's name or body lookup that missed waits on, its callback signalling
/// `MAIL_INBOX_UPDATE` for a found answer: a player's name (`0x4af080`, `0x4af5b0`), a creature's
/// (`0x4af080`), a game object's (`0x4af080`) or a letter's text (`0x4af340`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailWait {
    Player(u64),
    Creature(u32),
    Object(u32),
    Body(u32),
}

impl MailOpen {
    /// The open core's cell write (`0x4acd27`): the guid, keeping the rows whichever mailbox they
    /// came from. [`open_mailbox`] registers on the latch first.
    pub(crate) fn click(&mut self, mailbox: u64) {
        self.mailbox = Some(mailbox);
    }

    /// A getter's lookup of `key` missed: its callback waits for the answer.
    pub(crate) fn wait(&mut self, key: MailWait) {
        self.waits.push(key);
    }

    /// `key` answered, `found` or refused: each callback waiting on it runs and goes, in the order
    /// kept, a found one's `MAIL_INBOX_UPDATE` on the lane; a refusal's signals nothing.
    pub(crate) fn answered(&mut self, key: MailWait, found: bool, lane: &mut MailLane) {
        self.waits.retain(|&wait| {
            if wait != key {
                return true;
            }
            if found {
                lane.push(MailEvent::InboxUpdate);
            }
            false
        });
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
    }

    /// The world-enter init (`0x4acb10`): no mailbox and no rows. The bodies stay, asks
    /// included: the item-text store is saved (`0x554eb0`), and neither the init (it touches no
    /// cache) nor the world entry (`0x555811`) clears it.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.mails.clear();
        self.auction_subjects.clear();
    }

    /// A disconnect: the session's rows as [`Self::clear_session`], with the callbacks that waited
    /// on any ask. The body asks that died with the socket are cleared by [`crate::wdb`].
    pub(crate) fn clear_disconnected(&mut self) {
        self.clear_session();
        self.waits.clear();
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
}

/// A mailbox use (`0x5f6820` → `0x4acd10`): the latch first, with no test of the cell
/// (`0x4acd1c`), so a use of the open mailbox closes the window and opens it again; then the cell
/// (`0x4acd27`); then the compose tab's reset with its events and `MAIL_SHOW` (`0x4acd38`,
/// `0x4acd42`), in place, or behind what the lane still holds ([`lane::step`]).
pub(crate) fn open_mailbox(world: &mut World, mailbox: u64) {
    let waits = lane::waits(world);
    crate::ui_session::register(world, mailbox);
    world.resource_mut::<MailOpen>().click(mailbox);
    lane::step(world, waits, MailEvent::Show);
}

/// The close core's ordinary arm, `0x4acd50` with `cl = 0`: the compose tab's silent reset first
/// and always (`0x4acd55` → `0x4acdc0(0)`), then, with a mailbox open, the latch release
/// (`0x4acd85`), the cell cleared, `MAIL_CLOSED` (`0x4acda3`) and the tail (`0x4acda8`,
/// `0x4acdb1`): after a read, or an arrival while open, the next-mail-time query, stamping "no
/// mail". The latch, the cell and the tail run here; the reset and the signal wait behind what the
/// lane still holds, or at a packet behind the earlier packets' signals ([`lane::step`]).
pub(crate) fn close_core(world: &mut World) {
    let waits = lane::waits(world);
    lane::step(world, waits, MailEvent::Reset);
    let Some(mailbox) = world.resource::<MailOpen>().mailbox else {
        return;
    };
    crate::ui_session::release(world, mailbox);
    world.resource_mut::<MailOpen>().clear();
    lane::step(world, waits, MailEvent::Closed);
    lane::step(world, waits, MailEvent::Refresh);
}

/// The leave-world sweep's mail leg (`0x490b1b` → `0x4acc00` → `0x4acd50(cl=1)`): the compose
/// tab's silent reset runs first and always (`0x4acd55` → `0x4acdc0(0)`), lifting the
/// attachment's lock in the VM the sweep runs in (`0x4acdd6`); then the mailbox is forgotten with
/// no `MAIL_CLOSED`, no interaction release and no refresh (`0x4acd5f`-`0x4acd6b`), the `cl`
/// arm the close button's `0x4acd71` skips to. The latch still names the mailbox, which the
/// sweep's interaction close then finds no window for. Then each paper's wait is cancelled
/// (`0x4acc15`-`0x4acc2d`).
pub(crate) fn close_mail_on_leave_world(
    mut vm: VmWith<(
        ResMut<'static, MailOpen>,
        Option<Res<'static, crate::items::Items>>,
    )>,
) {
    vm.run(|script| {
        if let Some(script) = script {
            script.reset_compose_tab_silently();
        }
    });
    let (mut mail, items) = vm.own();
    mail.clear();
    if let Some(items) = items {
        UsableStationery::cancel_waits(&items);
    }
}

pub(crate) struct UiMailPlugin;

impl Plugin for UiMailPlugin {
    fn build(&self, app: &mut App) {
        lane::add_mail_lane(app);
        net::register(app);
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<UsableStationery>()
            .add_latched_window::<MailOpen>()
            .add_sweep_leg(SweepLeg::Mail, close_mail_on_leave_world)
            .add_systems(
                Update,
                // In the frame time, after the input records and the packets, as the world
                // frame's update steps it (`0x4818d0`), behind its walk-away watch
                // (`0x4818c0`) and that close's flush, with the in-game interface up.
                step_pending_mail
                    .after(benilla_world::schedule::WorldStage::Net)
                    .after(crate::ui_session::watch_interaction_range)
                    .before(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

/// The world frame's step of the pending-mail time (`0x4818d0` → `0x4ade60`): a crossing under
/// the epsilon signals `UPDATE_PENDING_MAIL` (`0x4adeba`), on the lane behind the frame's packets.
fn step_pending_mail(
    time: Res<Time>,
    mut pending: ResMut<MailPending>,
    mut lane: ResMut<MailLane>,
) {
    if pending.step(time.delta_secs()) {
        lane.push(MailEvent::Pending);
    }
}

/// The mail init (`0x4acb10`, called at `0x4909f6`) in each world-enter cascade's place
/// ([`crate::ui_world_enter`]), which a login, a cross-map worldport and a `/reload` all run: the
/// mailbox, the rows, the list throttle (`0x4acb7d`) and the request lock (`0x4acb82`) start over,
/// every stationery's item is looked up with a callback (`0x4acbba`,
/// [`stationery::count_papers_on_enter`], its leg just ahead), then it stamps "no mail"
/// (`0x4acb87`) and sends `MSG_QUERY_NEXT_MAIL_TIME` (`0x4acbdc`), after the time query and before
/// the battlefield status.
pub(crate) fn send_query_next_mail_time_on_enter(
    mut vm: VmWith<MailSession>,
    commands: Res<NetCommands>,
) {
    vm.turn(|script, (mut mail, mut pending)| {
        mail.clear_session();
        if let Some(script) = script {
            script.reset_mail_world_state();
        }
        pending.on_query_sent();
        let _ = commands.0.send(ClientCommand::QueryNextMailTime);
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

    let (
        item_id,
        item_roll,
        item_count,
        item_name,
        item_texture,
        item_quality,
        item_inventory_type,
    ) = match &entry.item {
        Some(att) => {
            // The row's lookup, whose miss asks; then what `GetInboxItem` reads of the record as
            // it runs, read the same way.
            let _ = items.template(att.entry, 0, ask);
            let roll = benilla_ui::script::ItemRoll {
                property: att.random_prop_id,
                ..Default::default()
            };
            let live =
                crate::ui_items::live::LiveItem::record(att.entry, roll, items, icons, rolls);
            (
                att.entry,
                att.random_prop_id,
                u32::from(att.count),
                live.name(),
                live.texture(),
                live.quality(),
                live.inventory_type(),
            )
        }
        None => (0, 0, 0, None, None, None, 0),
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
    // The invoice names it with the roll's suffix (`0x4af430` → `0x5d8b00`), "" while the cache
    // lacks it: `0x5d8b00` clears its buffer first (`0x5d8b25`) and the verb never tests it.
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
    // The subject parse keeps its triplet for a won or sold notice alone (`0x4acfc1`-`0x4acfdd`),
    // and the invoice and `isInvoice` both need its item entry (`0x4af3e8`, `0x4af2eb`).
    let invoice_notice = auction
        .filter(|a| a.entry != 0 && matches!(a.result, auction_mail::WON | auction_mail::SOLD));
    let auction_invoice = invoice_notice.and_then(|a| {
        let seller = a.result == auction_mail::SOLD;
        let n = invoice::parse_body(raw_body?, seller);
        // An unresolved counterparty is the verb's one miss past the body (`0x4af4b7`-
        // `0x4af4c1`), filled in when its name lands; a guid the body left 0 is null at once,
        // asked of no one, as every cache's zero key is (`0x55f0a5`-`0x55f0ac`).
        let player_name = names.resolve_player(n.player_guid, ask)?.to_string();
        Some(benilla_ui::script::MailInvoice {
            seller,
            item_name: notice_item_name.unwrap_or_default(),
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
        // the item entry its subject parser persists for those two alone.
        is_invoice: invoice_notice.is_some(),
        invoice: auction_invoice,
        item_id,
        item_random_property_id: item_roll,
        item_charges,
        item_durability,
        item_name,
        item_texture,
        item_quality,
        item_inventory_type,
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
/// player's `store` carries (`0x622270`, mask 0), with a cached template, in the comparator
/// `0x4ada90`'s order: carried first (`0x4adab5`-`0x4adae7`), then the lower buy price, signed
/// (`0x4adb81`), then the name, ASCII case folded (`SStrCmpI` at `0x4adba6`). A carried paper has
/// no `cost`. The templates are only read (`0x4ada51`): the mail init asks for them
/// ([`stationery::count_papers_on_enter`]). Only the rebuild runs it ([`StationeryRebuild`]).
pub(super) fn stationeries(
    catalog: &Stationery,
    store: Option<&benilla_protocol::ObjectFields>,
    objects: &(impl crate::ui_items::ItemObjects + ?Sized),
    items: &Items,
    icons: Option<&ItemDisplays>,
) -> Vec<StationeryView> {
    let mut out: Vec<((bool, i32, String), StationeryView)> = Vec::new();
    for row in catalog.0.rows() {
        let Some(t) = items.template(row.item, 0, Ask::Read).cloned() else {
            continue;
        };
        let carried = store.is_some_and(|s| {
            crate::ui_items::count_of(
                s,
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

/// `0x4ad050`'s order for each result: the arm's message line and a send's compose reset
/// (`0x4ad0ca`), the arm's own events, a purged row's `CLOSE_INBOX_ITEM(index)` (money `0x4ad772`,
/// item `0x4ad87d`) and its `MAIL_INBOX_UPDATE` (`0x4ad7a3`, `0x4ad8ae`, `0x4ad8fc`, `0x4ad966`),
/// then the tail: the send-pending flag (`0x4ad159`) and the request lock (`0x4ad164`) lift, so a
/// `MAIL_FAILED` handler can send the next request, and `MAIL_FAILED` on every path (`0x4ad15f`).
fn announce_results(
    script: &mut UiScript,
    results: Vec<MailResult>,
    mut show: impl FnMut(&mut UiScript, Option<crate::ui_action::Shown>),
) {
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
            script.fire_event("MAIL_INBOX_UPDATE", vec![]);
        }
        if result.send.is_some() {
            script.mail_send_resolved();
        }
        script.mail_request_resolved();
        script.fire_event("MAIL_FAILED", vec![]);
    }
}

/// The list parse's tail: the request lock clears (`0x4ad5d0`), then `MAIL_INBOX_UPDATE`
/// (`0x4ad5d6`), once per result.
fn announce_lists(script: &mut UiScript, results: u32) {
    for _ in 0..results {
        script.mail_request_resolved();
        script.fire_event("MAIL_INBOX_UPDATE", vec![]);
    }
}

/// What a mail getter's lookups read and keep, inside its call
/// ([`benilla_ui::script::RecordAsk::Mail`]): the row's keys off the session, the caches that ask a
/// miss, and the callbacks a miss keeps.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct MailLookups<'w> {
    /// Each absent in a harness without it, which asks nothing.
    mail: Option<ResMut<'w, MailOpen>>,
    items: Option<Res<'w, Items>>,
    names: Option<Res<'w, NameCache>>,
    gameobjects: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    stationery: Option<Res<'w, Stationery>>,
    packages: Option<Res<'w, Packages>>,
}

impl MailLookups<'_> {
    /// `lookup` on the row of `mail_id`, as its getter makes it: each lookup asks a miss, a
    /// refused key again (`0x556aa0`), and keeps its callback, one per call. A row gone since the
    /// read asks nothing.
    pub(crate) fn look_up(
        &mut self,
        net: &NetCommands,
        mail_id: u32,
        lookup: benilla_ui::script::MailLookup,
    ) {
        use benilla_ui::script::MailLookup as L;
        let (Some(mail), Some(items), Some(names)) = (
            self.mail.as_deref_mut(),
            self.items.as_deref(),
            self.names.as_deref(),
        ) else {
            return;
        };
        let Some(entry) = mail.mails.iter().find(|e| e.message_id == mail_id).cloned() else {
            return;
        };
        // The item cache with `0x4af0a0`, owner 0 (`0x4aecb0`-`0x4aecc7`); entry 0 is no key.
        let item = |entry: u32| {
            if entry != 0 && items.template(entry, 0, Ask::Event(net)).is_none() {
                items.wait(entry, crate::items::RecordCallback::Mail);
            }
        };
        match lookup {
            L::Header => {
                // A package id the store holds names the icon, and the item is not looked up
                // (`0x4aec76`-`0x4aeca4`).
                let package = (entry.package as i32) > 0
                    && self
                        .packages
                        .as_deref()
                        .is_some_and(|p| p.0.iter().any(|r| r.id == entry.package));
                if !package {
                    item(entry.item.as_ref().map_or(0, |a| a.entry));
                }
                // The paper's item, for a paper the store holds (`0x4aed25`-`0x4aed63`).
                if let Some(row) = self
                    .stationery
                    .as_deref()
                    .and_then(|s| s.0.rows().iter().find(|r| r.id == entry.stationery))
                {
                    item(row.item);
                }
                // The sender by type (`0x4aedb6`): a GM letter's is a string (`0x4aedbd`).
                let sender_id = entry.sender_id.unwrap_or(0);
                let wait = match entry.message_type {
                    mail_message_type::NORMAL if entry.stationery != STATIONERY_GM => entry
                        .sender_guid
                        .filter(|&g| g != 0)
                        .filter(|&g| names.resolve_player(g, Ask::Event(net)).is_none())
                        .map(MailWait::Player),
                    mail_message_type::CREATURE => (sender_id != 0
                        && names
                            .resolve_creature(sender_id, 0, Ask::Event(net))
                            .is_none())
                    .then_some(MailWait::Creature(sender_id)),
                    mail_message_type::GAMEOBJECT => (sender_id != 0
                        && self
                            .gameobjects
                            .as_deref()
                            .is_some_and(|g| g.resolve_name(sender_id, Ask::Event(net)).is_none()))
                    .then_some(MailWait::Object(sender_id)),
                    _ => None,
                };
                mail.waits.extend(wait);
            }
            L::Item => item(entry.item.as_ref().map_or(0, |a| a.entry)),
            // The counterparty's guid off the cached body (`0x4af471`, `0x4af495`), a zero one no
            // key (`0x55f0a5`).
            L::InvoiceName => {
                // A won or sold notice's, whose subject kept its item (`0x4af3e8`).
                let notice = (entry.message_type == mail_message_type::AUCTION)
                    .then(|| invoice::parse_subject(&entry.subject))
                    .flatten()
                    .filter(|a| {
                        a.entry != 0 && matches!(a.result, auction_mail::WON | auction_mail::SOLD)
                    });
                let guid = notice
                    .and_then(|a| {
                        let body = mail.bodies.get(entry.item_text_id)?;
                        Some(invoice::parse_body(body, a.result == auction_mail::SOLD).player_guid)
                    })
                    .unwrap_or(0);
                if guid != 0 && names.resolve_player(guid, Ask::Event(net)).is_none() {
                    mail.waits.push(MailWait::Player(guid));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ObjectStore, Objects, SelfPlayer};
    use benilla_protocol::messages::MailAttachment;
    use benilla_ui::script::{MailCall, MailRowVerb, MailState};

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
                        stationeries(
                            &catalog,
                            self_q.iter().next().map(|(s, _)| &s.0),
                            &objects,
                            &items,
                            None,
                        )
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
                    stationeries(
                        &catalog,
                        self_q.iter().next().map(|(s, _)| &s.0),
                        &objects,
                        &items,
                        None,
                    )
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
        m.mails.push(entry(1, Some(0xA), 0, 30.0));
        // The same mailbox keeps the rows.
        m.click(0x100);
        assert_eq!(m.mails.len(), 1);
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
            .insert_resource(NetCommands(tx));
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
        app.world_mut()
            .run_system_cached(send_query_next_mail_time_on_enter)
            .expect("the mail init runs");
        let mail = app.world().resource::<MailOpen>();
        assert!(mail.mails.is_empty() && mail.mailbox.is_none());
        assert!(check(&mut app), "the lock and the throttle start over");
    }

    /// A close forgets the mailbox and keeps the rows (`0x4acd71`); the world-enter init and a
    /// disconnect clear them (`0x4acb10`), and keep the saved item-text store.
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
        assert!(!m.bodies.is_empty());
    }

    /// The world-enter init (`0x4acb10`) touches no cache: a body ask in flight stands (the
    /// item-text store is saved), and the mail's own disconnect leaves it to [`crate::wdb`].
    #[test]
    fn the_world_enter_init_and_the_mails_disconnect_keep_a_body_ask() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut m = MailOpen::default();
        let ask = |m: &MailOpen| m.bodies.read(7, Ask::Event(&commands), |_| {}).is_none();
        assert!(ask(&m));
        assert!(m.bodies.is_pending(7));
        m.clear_session();
        assert!(m.bodies.is_pending(7), "the init leaves the ask");
        m.clear_disconnected();
        assert!(
            m.bodies.is_pending(7),
            "the store's clear is the saved stores'"
        );
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
        let props = crate::items::RandomProperties::new(
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

    /// An invoice whose item the cache does not hold names it "": `0x5d8b00` clears its buffer
    /// first (`0x5d8b25`) and `0x4af360` never tests it. Only a counterparty name the lookup
    /// cannot answer (`0x4af4b7`-`0x4af4c1`) is the miss.
    #[test]
    fn an_invoice_names_an_uncached_item_as_empty() {
        let states = crate::world_state::WorldStates::default();
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut bodies = QueryCache::default();
        bodies.insert(99, "0000000000000042:500:0".into());
        let mut e = entry(1, None, 0, 30.0);
        (e.message_type, e.sender_id) = (mail_message_type::AUCTION, Some(1));
        (e.subject, e.item_text_id) = ("2589:0:1".into(), 99);
        let row = |names: &NameCache| {
            resolve_row(
                &e,
                &bodies,
                None,
                &items,
                None,
                None,
                None,
                RowCatalogs::default(),
                names,
                Ask::New(&commands),
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &no_strings,
            )
        };
        let mut names = NameCache::default();
        assert!(row(&names).invoice.is_none(), "the buyer's name is out");
        names.insert_player(0x42, "Bob".into(), None);
        let invoice = row(&names).invoice.expect("the invoice, its item out");
        assert_eq!(
            (
                invoice.item_name.as_str(),
                invoice.player_name.as_str(),
                invoice.bid
            ),
            ("", "Bob", 500)
        );
    }

    /// One auction notice's row with `subject` and `body` (text id 99), its item cached and the
    /// player 0x42 named Bob: the invoice, `isInvoice` and the name queries the read sent.
    fn invoice_row(
        subject: &str,
        body: &str,
    ) -> (Option<benilla_ui::script::MailInvoice>, bool, usize) {
        let states = crate::world_state::WorldStates::default();
        let mut items = Items::default();
        items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut names = NameCache::default();
        names.insert_player(0x42, "Bob".into(), None);
        let mut bodies = QueryCache::default();
        bodies.insert(99, body.into());
        let mut e = entry(1, None, 0, 30.0);
        (e.message_type, e.sender_id) = (mail_message_type::AUCTION, Some(1));
        (e.subject, e.item_text_id) = (subject.into(), 99);
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
        let asked = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::NameQuery { .. }))
            .count();
        (row.invoice, row.is_invoice, asked)
    }

    /// The invoice's item entry `[rec+0x250]` must be set (`0x4af3e8`-`0x4af3ee`), as must
    /// `GetInboxText`'s `isInvoice` (`0x4af2eb`-`0x4af2f5`): a `0:x:1` subject has neither.
    #[test]
    fn an_invoice_needs_its_item_entry() {
        let (invoice, is_invoice, _) = invoice_row("0:0:1", "42:500:600");
        assert_eq!((invoice, is_invoice), (None, false));
        let (invoice, is_invoice, _) = invoice_row("2589:0:1", "42:500:600");
        assert!(invoice.is_some() && is_invoice);
    }

    /// The body is `sscanf`ed (`0x73fb5d` at `0x4af471` and `0x4af495`) over outputs zeroed first
    /// (`0x4af440`-`0x4af44f`), and its count is never tested (`0x4af476 jmp 0x4af49d`): a partial
    /// body answers the fields it parsed and zeros. A body whose guid does not parse leaves it 0,
    /// which the name cache answers null at once, asking nothing (`0x55f0a5`-`0x55f0ac`).
    #[test]
    fn a_partial_invoice_body_answers_what_it_parsed() {
        let (invoice, _, _) = invoice_row("2589:0:2", "42:500:x:9:9");
        let invoice = invoice.expect("a seller invoice from a partial body");
        assert_eq!(
            (
                invoice.player_name.as_str(),
                invoice.bid,
                invoice.buyout,
                invoice.deposit,
                invoice.consignment
            ),
            ("Bob", 500, 0, 0, 0)
        );
        let (invoice, _, _) = invoice_row("2589:0:1", "42");
        assert_eq!(
            invoice.map(|i| (i.bid, i.buyout)),
            Some((0, 0)),
            "the guid alone"
        );
        let (invoice, is_invoice, asked) = invoice_row("2589:0:1", "garbage");
        assert_eq!((invoice, is_invoice, asked), (None, true, 0));
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

    /// The sweep's mail leg, with the session.
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
            .add_sweep_leg(SweepLeg::Mail, close_mail_on_leave_world);
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
        open_mailbox(app.world_mut(), MAILBOX);
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

    /// A worldport's surviving VM: the leg's silence holds, and nothing adds a `MAIL_CLOSED`.
    #[test]
    fn a_worldport_forgets_the_mailbox_without_mail_closed() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm, sweep_in_place};
        let mut app = sweep_mail_app();
        app.insert_non_send_resource(spied_vm(&EVENTS));
        open_mailbox(app.world_mut(), MAILBOX);
        sweep_in_place(&mut app);
        app.update();
        app.update();
        assert_eq!(seen_in(&app), ["MAIL_SHOW"]);
    }

    /// A mail with a template id and no text id takes its body from `MailTemplate.dbc`
    /// (`0x4af217`-`0x4af26e`); an auction mail's text is no body (`0x4af1cf`), whatever its
    /// notice.
    #[test]
    fn a_template_letter_reads_its_body_from_the_template() {
        let states = crate::world_state::WorldStates::default();
        let templates = MailTemplates([(87, "Thanks for the fish.".to_string())].into());
        let mut template = entry(1, Some(0x77), 0, 30.0);
        template.mail_template_id = 87;
        // An outbid notice (result 0), which no invoice parse marks.
        let mut notice = entry(2, None, 0, 30.0);
        notice.message_type = mail_message_type::AUCTION;
        notice.subject = "2589:0:0".into();
        notice.item_text_id = 9;
        let mut bodies = QueryCache::default();
        bodies.insert(9, "Outbid.".to_string());
        let body = |e: &MailListEntry| {
            resolve_row(
                e,
                &bodies,
                None,
                &Items::default(),
                None,
                None,
                Some(&templates),
                RowCatalogs::default(),
                &NameCache::default(),
                Ask::Read,
                crate::items::RollCatalogs::NONE,
                &no_macros(&states),
                &no_strings,
            )
            .body
        };
        assert_eq!(body(&template).as_deref(), Some("Thanks for the fish."));
        assert_eq!(body(&notice), None);
    }

    /// The mail init looks every stationery's item up with a callback (`0x4acbba`); no frame asks,
    /// and the list the getters read only reads (`0x4ada51`).
    #[test]
    fn the_stationery_items_are_asked_at_the_world_enter_init_only() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<MailOpen>()
            .init_resource::<MailPending>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(Stationery(benilla_formats::StationeryCatalog::from_rows(
                vec![benilla_formats::StationeryRow {
                    id: 5,
                    item: 9_311,
                    texture: "PAPER".into(),
                    flags: 1,
                }],
            )));
        app.world_mut().spawn((
            SelfPlayer,
            crate::net::Guid(7),
            ObjectStore(Default::default()),
        ));
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty(), "no frame asks");
        app.world_mut()
            .run_system_cached(count_papers_on_enter)
            .expect("the mail init's papers run");
        app.world_mut()
            .run_system_cached(send_query_next_mail_time_on_enter)
            .expect("the mail init runs");
        assert!(matches!(
            crate::query_cache::queries(&rx)[..],
            [ClientCommand::ItemQuery { entry: 9_311, .. }]
        ));
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
}
