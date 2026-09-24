//! The app-side **mail feed** (decision 0544 P1/P2) — the inward half of the mail seam around
//! [`benilla_ui::script`]'s `mail` module, the mailbox twin of [`crate::ui_merchant`]'s vendor seam.
//!
//! The mailbox window is entirely client-side (there is no `SMSG_SHOW_MAILBOX` on 5875): a
//! right-click on a MAILBOX GameObject ([`crate::target`]) opens the session here — it stores the
//! mailbox guid and requests the window's show; it sends **no** packet (the MAILBOX use handler
//! `0x5f6820` overrides the shared use-sender to a local open — never `CMSG_GAMEOBJ_USE`).
//! The window's own `MAIL_SHOW` handler then calls `CheckInbox()`, whose intent this module drains
//! into the first `CMSG_GET_MAIL_LIST` — one request, Lua-driven, exactly the reference flow
//! (MailFrame.lua l.42).
//!
//! The mailbox's packet handlers ([`net`]) fill [`MailOpen`] from the wire
//! (`SMSG_MAIL_LIST_RESULT` → the inbox rows; `SMSG_ITEM_TEXT_QUERY_RESPONSE` → the body cache;
//! `SMSG_SEND_MAIL_RESULT` → the send/take result queues). Each frame [`feed_mail`] resolves each
//! wire [`MailListEntry`] to a Lua-facing [`MailInboxRow`] (sender via the ask-once name cache, item
//! name/quality/icon via the ask-once item-template cache + `ItemDisplayInfo.dbc`, body from the
//! cache, stationery basename from `Stationery.dbc`), pushes the snapshot
//! ([`benilla_ui::script::UiScript::set_mail`]), and fires the events the reference Lua drives —
//! `MAIL_SHOW` on open, `MAIL_INBOX_UPDATE` on a list/body change, `MAIL_CLOSED` on clear, and the
//! send-result trio (see [`feed_mail`]). [`drain_mail`] pulls the Lua intents back out into the mail
//! `CMSG`s. The standardized NPC-session range guard ([`crate::ui_session`]) client-side-closes the
//! window when the player leaves the mailbox's interaction range (the client's generic
//! interaction-target range gate `0x493230` — past it every mail `CMSG` silently fails its
//! `CheckMailBox`, `MailHandler.cpp:65`).
//!
//! **The arrival layer ([`MailPending`], decisions 0544 P3 / 0904 / 0913).** `HasNewMail()` mirrors
//! the real client's own model: a countdown float (`0x845eac`) read as **`|countdown| < ε`** —
//! *near zero*, not `<= 0` (the sign matters, decision 0904: vmangos's "no mail" reply is
//! `-86400.0`, which a `<= 0` predicate reads as "you have mail" at every login). vmangos only ever
//! sends `0.0` ("waiting now") or `-86400.0` ("none") — never a positive value — but the countdown
//! is implemented in full since that's the client's real model, not a two-value special case.
//!
//! **The icon's whole life is that float**, because `MiniMapMailFrame` is the *only* listener of
//! `UPDATE_PENDING_MAIL` in all of 1.12 FrameXML and its handler is a bare idempotent
//! `if HasNewMail() then Show() else Hide()` (`Minimap.xml` l.278-289) — it never listens for
//! `MAIL_INBOX_UPDATE`/`MAIL_CLOSED` despite the icon logically depending on the inbox state. So
//! **checking your mail clears the icon by a route that never touches the inbox** (decision 0913):
//! opening a letter arms the deferred-refresh flag ([`MailPending::arm_refresh`], the reference's
//! `[0xb6efcc]`), and the mailbox *close* re-asks the server — the sender stamping the countdown to
//! [`MAIL_TIME_NO_MAIL`] and the reply settling it. The same stamp-then-ask runs at every
//! world-enter ([`send_query_next_mail_time_on_enter`]), which is why the reference icon also blinks
//! off across a loading screen.
//!
//! [`feed_mail`] steps the float every frame ([`MailPending::step`]), keeps
//! [`benilla_ui::script::UiScript::set_has_new_mail`] current, and fires `UPDATE_PENDING_MAIL` at
//! the three sites the reference fires it (see [`MailPending::notify`]).

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

/// `checked` mask bits (VERIFIED vmangos `Mail.h`, decision 0544): READ, RETURNED, COPIED.
const CHECKED_READ: u32 = 0x1;
const CHECKED_RETURNED: u32 = 0x2;
const CHECKED_COPIED: u32 = 0x4;

/// `MAIL_STATIONERY_GM` (`Stationery.dbc` row 61) — `isGM` is `stationery == 61` (MailFrame.lua
/// l.108: a GM row shows its stationery icon even when it carries a package).
const STATIONERY_GM: u32 = 61;

/// The `CheckInbox` → `CMSG_GET_MAIL_LIST` client-side rate limit, in seconds (`0x4aeab0`: the
/// real client drops repeat inbox queries inside this window).
const CHECK_INBOX_THROTTLE_SECS: f64 = 60.0;

/// The letter-row icon by kind (the wire carries only a stationery id; the reference derives the
/// row's `stationeryIcon` from it). VERIFIED via the data chain `Stationery.dbc` → stationery
/// item (41 → 9311 "Default Stationery", 61 → 18154 "Blizzard Stationery") → the item's
/// `ItemDisplayInfo` icon (displays 7798 / 30658) — and the default's pixels match the reference
/// screenshot's red-stamped letter (`INV_Letter_15`, our old guess, is the tan open envelope).
const STATIONERY_ICON_NORMAL: &str = "Interface\\Icons\\INV_Misc_Note_01";
const STATIONERY_ICON_GM: &str = "Interface\\Icons\\Mail_GMIcon";

/// The `Stationery.dbc` id → texture-basename catalog, loaded with the entity catalogs
/// ([`crate::entities`]). Optional resource — absent, every mail falls to the default backdrop
/// ([`benilla_formats::StationeryCatalog::DEFAULT_TEXTURE`]) and the window still works.
#[derive(Resource)]
pub(crate) struct Stationery(pub(crate) benilla_formats::StationeryCatalog);

/// One `SMSG_SEND_MAIL_RESULT` for `action == SEND`, queued by the net bridge for [`feed_mail`] to
/// fire (the feed owns the `UiScript`, the apply site doesn't). `MAIL_FAILED` fires on **every**
/// SEND result — success included — because the real client overloads it as "send resolved, unblock
/// the UI" (`0x4ad15f`; MailFrame.lua's `MAIL_FAILED` handler only re-enables the button).
/// `MAIL_SEND_SUCCESS` fires additionally when `error == OK`.
pub(crate) struct MailSendAck {
    pub(crate) ok: bool,
    /// The message key for a non-OK, non-equip send failure (else `None`).
    pub(crate) refusal: Option<&'static str>,
}

/// The open mailbox, filled by the packet handlers ([`net`]) and read by
/// [`feed_mail`]. Holds the mailbox guid and the inbox rows exactly as the wire delivered them
/// (`SMSG_MAIL_LIST_RESULT`), the ask-once letter-body cache, and the send-result queues; the feed
/// resolves each row to a display row and the drain maps a clicked 1-based row to its wire mail id.
/// Cleared on a client-side close and on disconnect.
#[derive(Resource, Default)]
pub(crate) struct MailOpen {
    /// The mailbox whose window is open; `None` = no mailbox open.
    pub(crate) mailbox: Option<u64>,
    /// The inbox rows (wire order = 1-based display order).
    pub(crate) mails: Vec<MailListEntry>,
    /// Ask-once letter-body cache: `item_text_id` → body text, through [`QueryCache`] (2288).
    pub(crate) bodies: QueryCache<u32, String>,
    /// Fire `MAIL_SHOW` next feed — set on every mailbox click (the reference opens the window per
    /// use, `0x4acd10`). Consumed by [`feed_mail`].
    show_requested: bool,
    /// Session-scoped throttle: the `Time::elapsed_secs_f64` of the last `CMSG_GET_MAIL_LIST`, or
    /// `None` when never queried this session (a fresh open queries immediately).
    last_list_query: Option<f64>,
    /// SEND-action results the net bridge queued for the feed to fire ([`MailSendAck`]).
    pub(crate) send_acks: Vec<MailSendAck>,
    /// 1-based inbox rows a take just emptied and purged ([`net`]) — the feed fires
    /// `CLOSE_INBOX_ITEM(index)` for each, ahead of the `MAIL_INBOX_UPDATE` the purge causes.
    pub(crate) close_inbox: Vec<u32>,
    /// Refusals (a take/return/delete failure) the feed shows, as message keys — resolved at the
    /// feed, which is where the VM's `GlobalStrings` can be read
    /// ([`crate::ui_action::keyed_line`]).
    pub(crate) errors: Vec<&'static str>,
}

impl crate::query_cache::AskOnce for MailOpen {
    fn clear_pending(&mut self) {
        self.bodies.clear_pending();
    }
}

impl MailOpen {
    /// A mailbox right-click (decision 0544): open (or re-show) the window's session and request its
    /// `MAIL_SHOW`. A *different* mailbox resets the session (rows + throttle); the same one keeps
    /// them and just re-shows (the reference re-fires `MAIL_SHOW` on every use — `CheckInbox`'s own
    /// throttle stops the refresh from spamming the wire).
    pub(crate) fn click(&mut self, mailbox: u64) {
        if self.mailbox != Some(mailbox) {
            self.mailbox = Some(mailbox);
            self.mails.clear();
            self.last_list_query = None;
        }
        self.show_requested = true;
    }

    /// The wire mail id at a 1-based display row — what the mail `CMSG`s address.
    fn mail_id_at(&self, index_1based: u32) -> Option<u32> {
        index_1based
            .checked_sub(1)
            .and_then(|i| self.mails.get(i as usize))
            .map(|e| e.message_id)
    }

    /// Flip a row's local `checked` READ bit — the no-reply `CMSG_MAIL_MARK_AS_READ` has no server
    /// answer (decision 0544 P1), so the read state is client-authoritative until the next list.
    fn mark_read(&mut self, index_1based: u32) {
        if let Some(e) = index_1based
            .checked_sub(1)
            .and_then(|i| self.mails.get_mut(i as usize))
        {
            e.checked |= CHECKED_READ;
        }
    }

    /// Close the open window (a client-side close, no packet — vanilla). Keeps nothing; a re-open
    /// re-lists.
    pub(crate) fn clear(&mut self) {
        self.mailbox = None;
        self.mails.clear();
        self.bodies.clear();
        self.show_requested = false;
        self.last_list_query = None;
        self.send_acks.clear();
        self.errors.clear();
    }

    /// Disconnect: drop the open window (mirrors the merchant/gossip session clears).
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

/// The mailbox window is an NPC-less interaction session: the standardized range guard
/// ([`crate::ui_session`]) client-side-closes it — the exact `CloseMail` clear — when the player
/// leaves the mailbox's interaction range (the client's generic interaction-target range gate
/// `0x493230`, the same one the mail `CMSG`s' server-side `CheckMailBox` (`MailHandler.cpp:65`)
/// enforces at 5 yd).
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
                    // Range-close before the feed so the clear turns into MAIL_CLOSED the same
                    // frame; feed before the input pass so an open/close is on screen the same
                    // frame; drain after it so a click's intent (CheckInbox, a take, a send) goes
                    // out the same frame (the ui_merchant ordering exactly). After the UnitFeed set
                    // so the SetInboxItem tooltip reads a landed item-template store.
                    close_npc_session_out_of_range::<MailOpen>.before(feed_mail),
                    feed_mail.after(crate::ui_unit::UnitFeed).in_set(UiFeed),
                    drain_mail.after(UiInput),
                    // The world-enter one-shot (decision 0548 §7: "once at UI/login load") — its
                    // own system, ordering-independent of the feed/drain pair above.
                    send_query_next_mail_time_on_enter,
                ),
            );
    }
}

/// `MSG_QUERY_NEXT_MAIL_TIME`, sent once per **world-enter** — VERIFIED faithful (decision 0913):
/// the query's two callers are the mail module's init tail and the close-with-pending path, and
/// that init hangs off the world-enter cascade `0x4908c0` (`PLAYER_ENTERING_WORLD` + 37 subsystem
/// inits), whose guard is re-armed by the paired teardown — so it runs per world-enter (login,
/// worldport, instance transfer, `ReloadUI`), not once per process as an earlier note had it.
///
/// The init stamps the countdown to [`MAIL_TIME_NO_MAIL`] before asking, which is why the reference
/// icon blinks off across a loading screen and comes back with the reply.
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

/// The message a `SMSG_SEND_MAIL_RESULT` code shows — **always** a catalog key, which is the whole
/// correction decision 1821 brought here (superseding 0544's hand-written pair).
///
/// The reference's handler `0x4ad050` indexes `[0x4ad1a0 + error]` into a nine-entry jump table at
/// `0x4ad17c`, every arm a `push <id>; call CGGameUI::DisplayError`. This module used to claim
/// *"No 1.12 GlobalString for either"* about codes 5 and 14 and inline near-miss English for them;
/// both are real rows, and their real strings differ from what we were showing. And the arm this
/// module used to spell `"Mail failed (n)."` is the reference's own `ja 0x4ad147` — every code
/// from 6 to 13 and everything past 15 lands on `ERR_MAIL_DATABASE_ERROR` with them.
///
/// `OK` and `EQUIP_ERROR` never reach here: the first is not a refusal (it has
/// [`MAIL_SENT_KEY`] instead) and the second resolves through the equip-error table
/// (`0x4ad12e` → `0x622630`), which is [`crate::ui_items`]' job.
pub(crate) fn mail_refusal(error: u32) -> &'static str {
    match error {
        mail_error::CANNOT_SEND_TO_SELF => "ERR_MAIL_TO_SELF", // 0x164
        mail_error::NOT_ENOUGH_MONEY => "ERR_NOT_ENOUGH_MONEY", // 0x25 — speaks, line 0x28
        mail_error::RECIPIENT_NOT_FOUND => "ERR_MAIL_TARGET_NOT_FOUND", // 0x165
        mail_error::NOT_YOUR_TEAM => "ERR_PLAYER_WRONG_FACTION", // 0xff
        mail_error::TRIAL_ACCOUNT => "ERR_RESTRICTED_ACCOUNT", // 0x1be
        mail_error::TOO_MANY_ATTACHMENTS => "ERR_MAIL_REACHED_CAP", // 0x1c4
        // INTERNAL_ERROR (6) and every other code, by the same `0x4ad147`.
        _ => "ERR_MAIL_DATABASE_ERROR", // 0x166
    }
}

/// The line a **successful** `SEND` shows: `ERR_MAIL_SENT` — id `0x167`, and a `kind 1` row, so it
/// is the yellow info line and not the red one. `0x4ad0b1` reaches it before the form reset
/// (`0x4acdc0`), gated on the action being `SEND`; a successful take/return/delete says nothing.
pub(crate) const MAIL_SENT_KEY: &str = "ERR_MAIL_SENT";

mod invoice;

use invoice::auction_mail;

/// Resolve one wire [`MailListEntry`] into the Lua-facing [`MailInboxRow`]: sender through the ask-
/// once name cache (player guids only), the enclosed item's name/quality/icon through the ask-once
/// item-template cache + `ItemDisplayInfo.dbc`, the body from the cache, the stationery basename
/// from `Stationery.dbc`. `None`s stay `None` while a query is in flight — the row shows a
/// placeholder and fills in when the answer lands (the merchant/loot pattern).
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
    // The VM's own GlobalStrings, for the auction notice's subject template. A resolver rather than
    // a table because the strings are the player's install's, and Rust must never carry a copy of
    // them (decision 0669's shape).
    get_text: &dyn Fn(&str) -> Option<String>,
) -> MailInboxRow {
    let is_gm = entry.stationery == STATIONERY_GM;
    let returned = entry.checked & CHECKED_RETURNED != 0;
    // Reply is allowed only to a not-yet-returned mail from a player (MailFrame.lua l.281).
    let can_reply = entry.sender_guid.is_some() && !returned;
    // Delete-vs-return (INTERIM, decision 0548): a mail RETURNS on delete/expiry only when it still
    // carries attachments (item or money) from a not-yet-returned player sender — that is the
    // server's own expiry behavior (vmangos `ReturnOrDeleteOldMails`). Everything else — plain
    // letters, system mail, already-returned mail — deletes. The exact client predicate behind
    // `InboxItemCanDelete` is not read from the reference; this mirrors the observable law.
    let can_delete =
        returned || entry.sender_guid.is_none() || (entry.item.is_none() && entry.money == 0);

    // The enclosed item, resolved ask-once from its template.
    let (item_id, item_roll, item_count, item_name, item_texture, item_quality) = match &entry.item
    {
        Some(att) => {
            let template = items.template(att.entry, 0, commands);
            // The rolled name (1547) — the same join the loot window and the auction rows make.
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

    // The letter body, raw. Hoisted because the auction invoice parses THIS and not the
    // `$`-substituted text below: the invoice is machine-written colon fields, and running a macro
    // expander over them would be expanding the auction house's own bookkeeping.
    let raw_body = (entry.item_text_id != 0)
        .then(|| bodies.get(entry.item_text_id))
        .flatten();

    // ── The auction house's mail (`ui_mail::invoice`, `0x4ace70`/`0x4af360`) ─────────────────
    // Two independent resolves off one subject: the DISPLAYED subject, which every auction notice
    // gets, and the INVOICE, which only a won/sold notice can answer and only once its body has
    // been fetched.
    let auction = (entry.message_type == mail_message_type::AUCTION)
        .then(|| invoice::parse_subject(&entry.subject))
        .flatten();
    // The item the notice is ABOUT — resolved from the subject's own entry, not from the
    // attachment: a "sold" notice encloses the money, and names an item it does not carry.
    let notice_item_name = auction
        .and_then(|a| items.template(a.entry, 0, commands))
        .map(|t| t.name.clone());
    // `<key>: %s` + the name. Until the item template lands the raw triplet stands — a subject that
    // reads `4428:0:1` for one frame is better than one that reads `Auction won: ` forever if the
    // query never answers, and the row re-resolves (and fires MAIL_INBOX_UPDATE) when it does.
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
            // An unresolved counterparty takes the miss tail this pass and fills in when the name
            // query lands — the reference does exactly this, and fires MAIL_INBOX_UPDATE on the
            // late arrival, which our snapshot diff does for free.
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
        // The letter body is server-authored text, so it runs the `$`-macro expander — the
        // reference does it from two sites in `GetInboxText` (decision 0754). Subject is the
        // local player, as it is for every panel seam.
        body: raw_body.map(|b| crate::npc_text::substitute(b, macros)),
        stationery_texture,
        // NOT "is an auction mail": the reference's `isInvoice` reads the fields its subject
        // parser persisted, and it persists them only for won(1)/sold(2) — so an outbid or expiry
        // notice answers false here (decision 1527). This is also what nils the body: the two
        // answers come off the same record state, which is exactly why MailFrame can lay the
        // invoice pane over the letter page.
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

/// Build the Lua-facing snapshot from [`MailOpen`] — `None` when no mailbox is open.
/// The usable stationery list (`0x4ad970`, 1970): every
/// `Stationery.dbc` row that is always available (`Flags & 1`) or whose item the player carries
/// (bags, not the bank — the client's `0x622270`), and whose item template is cached — the name,
/// icon and price come off the template — sorted by BuyPrice ascending, the client's comparator
/// (`0x4ada90`). A carried paper costs nothing to use (`cost` nil). Templates are asked once here
/// so the list fills from the second frame after world enter, mailbox or no mailbox.
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

/// Push the current mail into the VM and fire the show/update/close + send-result events on a
/// transition or content change (an async sender/item/body landing, a mark-read flip, a fresh
/// list). Diffed against a `Local` memory, exactly like the merchant/gossip feeds.
/// The feed's tail, bundled — the signature sits at the 16-SystemParam ceiling: the random-suffix
/// roll's catalogs (1547), where a refusal lands (1815), and the usable stationery list's memo
/// (1970), pushed on change like the inbox.
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
    // The `$`-macro subject for the letter body: the local player, as at every panel seam.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
    // The random-suffix roll's catalogs (1547): an enclosed item's rolled name.
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

    // Take/return/delete failures go to the surface — and the voice — their message record names.
    let refusals: Vec<_> = std::mem::take(&mut mail.errors)
        .into_iter()
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_mail", refusals);
    // SEND acks: MAIL_FAILED on EVERY send result (unblocks the button — `0x4ad15f`), plus
    // MAIL_SEND_SUCCESS on OK (resets the form) or the refusal's own line on a failure. The
    // success also SAYS so — `ERR_MAIL_SENT`, the yellow info line `0x4ad0b1` shows before it
    // resets the form (decision 1821).
    // A take emptied and purged a row: `CLOSE_INBOX_ITEM(index)` — the stock handler hides the
    // open letter when it is that row — ahead of the `MAIL_INBOX_UPDATE` the changed list fires
    // below, the client's own order (`0x4ad772` before `0x4ad7a3`, 1970).
    for index in std::mem::take(&mut mail.close_inbox) {
        script.fire_event("CLOSE_INBOX_ITEM", vec![ScriptValue::Int(i64::from(index))]);
    }
    // `SMSG_SEND_MAIL_RESULT 0x4ad050`, in its own order: the GlobalString toast, then — on
    // `action 0 / reason 0` only — the compose-tab reset `0x4acdc0(1)` with its three events, and
    // **last, on every path, `MAIL_FAILED`** (`0x4ad15f`, unconditional: a successful send fires it
    // too, as the generic "the send resolved, unblock the form" signal).
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

    // The macro subject, resolved before the row walk borrows the name cache again. `None` until
    // the player is streamed and named — the feed diffs on the substituted text, so a letter opened
    // that early re-substitutes as soon as it lands.
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
    // The send tab's usable stationery (1970): `Stationery.dbc` rows the player may use — the
    // always-available one, or one whose item they carry — whose item template is cached, priced
    // by BuyPrice ascending. The templates are asked ahead of any mailbox (the catalog is five
    // rows) so the list is whole the frame the window opens: the stock `SendMailFrame_Reset`
    // selects row 1 on show, and an empty list then would leave every send silently unsent.
    //
    // **Only once there is a player.** `feed_mail` carries no run condition, so it runs from the
    // first frame of the process — the login screen, before the socket exists. The five asks went
    // out there, the io thread dropped them ("not connected"), and `Items::template` had already
    // latched all five in its ask-once `pending` set, which is cleared on DISCONNECT and never on
    // connect. The templates were therefore never re-asked, the list stayed empty for the whole
    // session, and the failure this very comment describes — every send silently unsent — is what
    // it caused. Gating on the self player is also what the list means: "what may I use", which is
    // not a question until there is a me.
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
        // The selection is cleared on open AND close (`0x4ace07`, 1970); the stock tab's reset
        // on MAIL_SHOW re-selects row 1.
        script.clear_stationery();
        // The open core's own order (`0x4acd10`): store the mailbox, register
        // the interaction target, **reset the compose tab** — a fresh window carries no stale
        // attachment or money — and only THEN `MAIL_SHOW`. The reset's `MAIL_SEND_SUCCESS` is the
        // byte-verified open side-effect, so the page-turn sound on open is faithful, not a bug.
        script.reset_compose_tab();
        script.fire_event("MAIL_SHOW", vec![]);
    } else if closed {
        script.clear_stationery();
        script.fire_event("MAIL_CLOSED", vec![]);
        // The close core's tail (`0x4acdad`/`0x4acdb1`, decision 0913): a session where a
        // mail was read — or one arrived while the window was open — re-asks the server, and the
        // sender stamps the countdown to "no mail" on the way out. This is the whole mechanism by
        // which checking your mail clears the minimap icon; nothing on the inbox path touches it.
        // Both close paths converge here (explicit `CloseMail` and the walk-away range close), as
        // they do in the reference through the same generic dispatcher.
        if pending.take_refresh() {
            pending.on_query_sent();
            let _ = commands.0.send(ClientCommand::QueryNextMailTime);
        }
    } else if mail.mailbox.is_some() {
        // A re-click re-shows the (already open) window; MAIL_SHOW's CheckInbox refreshes the list.
        if show_requested {
            script.fire_event("MAIL_SHOW", vec![]);
        }
        // A content change (async name/body landed, list replaced, mark-read flip) → repaint.
        if changed {
            script.fire_event("MAIL_INBOX_UPDATE", vec![]);
        }
    }
    *last = fresh;
    *last_mailbox = mail.mailbox;

    // The arrival layer (decisions 0544 P3 / 0904 / 0913): step the client-local countdown, keep
    // the VM's `HasNewMail()` answer current, and fire `UPDATE_PENDING_MAIL` only where the
    // reference fires it. The two are deliberately separate: the *push* keeps the Lua getter
    // truthful whenever it is asked, while the *event* is edge-triggered at three sites only (the
    // query reply, a near-zero arrival, the step crossing ε) — so, exactly as in the reference, the
    // query sender's "no mail" stamp does not move the icon until the reply lands.
    // `MiniMapMailFrame` is the sole listener in all of 1.12 FrameXML and its handler is an
    // idempotent Show/Hide (`Minimap.xml` l.278-289, quoted in the module doc).
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

/// Drain the Lua intents into the mail `CMSG`s (decision 0544): `CheckInbox` → a throttled
/// `CMSG_GET_MAIL_LIST`; an opened mail → `CMSG_MAIL_MARK_AS_READ` (once, unread) + the ask-once
/// `CMSG_ITEM_TEXT_QUERY` for its body; take/return/delete → their per-mail `CMSG`s; `SendMail` →
/// `CMSG_SEND_MAIL` (the attachment's `(bag, slot)` resolved to its item guid here); `CloseMail` →
/// a local clear (no packet, vanilla).
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

    // CheckInbox → a session-scoped, 60s-throttled inbox query (`0x4aeab0`).
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

    // Opened mails: mark read (once) then ask-once the body (`0x4af110`: read FIRST, body second).
    for index in script.take_mail_opens() {
        let Some((mail_id, text_id, read)) = index
            .checked_sub(1)
            .and_then(|i| mail.mails.get(i as usize))
            .map(|e| (e.message_id, e.item_text_id, e.checked & CHECKED_READ != 0))
        else {
            continue;
        };
        // Arm the deferred refresh on **every** open, read or not: the reference's `GetInboxText`
        // calls the mark-as-read sender unconditionally, and the `[0xb6efcc] = 1` inside it is
        // unconditional too (`0x4adda6`) — so opening any letter is what makes the close re-ask the
        // server, and that is what clears the minimap icon (decision 0913). Arming it on an
        // already-read mail costs at most one redundant query on close; failing to arm it would
        // strand the icon lit, so the eager side is the safe side.
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

    // The per-mail row picks addressing the wire mail id.
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

    // SendMail: resolve the attachment's (bag, slot) to the wire item guid at send time (the
    // reference re-reads the slot when the send fires — a lazy resolve, decision 0216/0544).
    if let Some(req) = script.take_mail_send() {
        let item_guid = req.item.and_then(|(bag, slot)| {
            self_q.iter().next().and_then(|s| {
                crate::ui_items::slot_guid(&s.0, bag, (slot.max(1) - 1) as u8, &objects)
            })
        });
        if req.item.is_some() && item_guid.is_none() {
            // The attached item is gone from its slot: `ERR_ITEM_NOT_FOUND`, no packet, the
            // attachment dropped and `MAIL_SEND_INFO_UPDATE` fired (`0x4ae98d`, 1970).
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

    /// **The mail-result table, welded to the message ids it was read from** (decision 1821) —
    /// the reference's jump table at `0x4ad17c`, reached through the byte index at `0x4ad1a0`.
    /// The id, not the key, is what was disassembled, so the id is what this asserts.
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
        // `ja 0x4ad147` and the index table's `08` run: 7..13 and everything past 15 share the
        // database-error line, and there is no "(n)" arm for them to take.
        for code in [7, 8, 13, 16, 99, u32::MAX] {
            assert_eq!(id(mail_refusal(code)), 0x166, "mail error {code}");
        }
        // A success SAYS so — and on the YELLOW line, which is the half a red-only path would
        // have got wrong.
        assert_eq!(id(MAIL_SENT_KEY), 0x167);
        assert_eq!(
            benilla_ui::messages::by_key(MAIL_SENT_KEY)
                .expect("row")
                .kind,
            benilla_ui::messages::MsgKind::Info
        );
    }

    /// The no-subject macro context these row tests run under: none of them exercises a `$` token,
    /// and a subject-less context leaves plain text untouched (the expander only rewrites at a `$`).
    /// The macro path's own behaviour is covered in [`crate::npc_text`]'s tests.
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
        // Re-click the SAME mailbox keeps the rows + throttle, just re-requests the show.
        m.click(0x100);
        assert_eq!(m.mails.len(), 1);
        assert_eq!(m.last_list_query, Some(5.0));
        assert!(m.show_requested);
        // A DIFFERENT mailbox resets the session.
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
        // Mark row 1 read: its READ bit flips locally.
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

    /// No GlobalStrings in a unit test — the auction-subject rewrite is exercised where the
    /// strings are (`tests/mail_frame.rs`), and here every key simply misses.
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

        // A plain letter from a player (no attachments) → replyable AND deletable.
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

        // A player mail still carrying money → returnable, NOT deletable (the 0548 INTERIM law:
        // attachments return; the same holds for an enclosed item).
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

        // A returned mail → not replyable, deletable.
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

        // A system mail (no sender guid) → deletable.
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
        // The item name/texture are in flight (no template answer yet).
        assert!(row.item_name.is_none());
    }
}
