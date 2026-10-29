//! The item-text reader (`ItemTextFrame.xml`) a bag letter, a book or a readable world object
//! opens in. Every route ends at `0x4e32e0(guid, flag)`, which looks the guid up as any object and
//! asks it for its text (`vtbl+0x74`), a mail-made letter's `ITEM_FIELD_ITEM_TEXT_ID` or else a
//! page chain from an item template's `PageText` or a GameObject's pageID column (TEXT `data[0]`,
//! GOOBER `data[7]`). A letter or a TEXT object opens locally, with no packet: vmangos has no use
//! arm for a type-9 object and refuses `CMSG_READ_ITEM` without a template `PageText`, which the
//! Plain Letter lacks (`ItemHandler.cpp:421`). A GOOBER's opens on `SMSG_GAMEOBJECT_PAGETEXT`
//! (`0x5f88c0`), the server's answer to its use. A bag book, an item with a page chain and no
//! item-text id whose `ITEM_FIELD_FLAGS` lack bit 0x2, sends `CMSG_READ_ITEM` after its begin
//! (`0x4e3586` → `0x5ec580`) and shows only on the reply ([`request_read`], `0x5ec600`):
//! `SMSG_READ_ITEM_OK` marks the item read and shows, `SMSG_READ_ITEM_FAILED` shows (reason 0),
//! shows untranslated with `ITEM_TEXT_TRANSLATION` (reason 1) or signals the close (any other).
//!
//! The events follow `ItemTextFrame.lua:10-96`.
//!
//! The reader is a window on the interaction latch. Its cell is written at the read
//! (`0x4e3364`), but it registers only once the text resolves (`0x4e349a`), right before
//! `ITEM_TEXT_BEGIN`, so the window it opens over closes first; its release (`0x4e3323`,
//! `0x4e3357`) re-enters the core while latched, firing `ITEM_TEXT_CLOSED` twice.
//!
//! Each step runs whole where its caller runs it: a packet's in its handler (the object's page
//! text, a record's answer, a read's reply, a name's answer), a page turn and `CloseItemText`
//! inside their calls, a click's or a bag use's read at its apply. What a handler signals goes on
//! the item-text lane at its place ([`ItemTextLane`]); the getters read the session through the
//! host at the call ([`ItemTextLive`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::query_cache::{Ask, QueryCache};
use crate::ui_mail::MailOpen;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

mod live;
pub(crate) use live::ItemTextLive;

/// One page of a book as the wire gave it (`SMSG_PAGE_TEXT_QUERY_RESPONSE`).
pub(crate) struct PageText {
    pub(crate) text: String,
    /// The next page's id; 0 on the last page.
    pub(crate) next: u32,
}

/// The ask-once page cache, the client's `PageText` dbcache (`0xc0e174`), keyed by page id.
/// vmangos answers one query with the whole chain; a missing page is still asked for alone.
#[derive(Resource, Default)]
pub(crate) struct PageTexts {
    pages: QueryCache<u32, PageText>,
}

impl PageTexts {
    pub(crate) fn insert(&mut self, page_id: u32, text: String, next: u32) {
        self.pages.insert(page_id, PageText { text, next });
    }

    /// The page store, the one [`crate::wdb`] saves.
    pub(crate) fn page_store(&self) -> &QueryCache<u32, PageText> {
        &self.pages
    }

    pub(crate) fn page_store_mut(&mut self) -> &mut QueryCache<u32, PageText> {
        &mut self.pages
    }

    /// The cached page, read as `ask` says; `guid` is the reading object, which vmangos discards.
    pub(crate) fn page(&self, page_id: u32, guid: u64, ask: Ask<'_>) -> Option<&PageText> {
        self.pages.read(page_id, ask, |commands| {
            debug!("page text: asking page {page_id} (object {guid:#x})");
            let _ = commands
                .0
                .send(ClientCommand::PageTextQuery { page_id, guid });
        })
    }
}

/// Which of the reference's two text sources a session shows; the object decides, not the click.
pub(crate) enum ReadSource {
    /// A mail-made letter's `ITEM_FIELD_ITEM_TEXT_ID` (`0xbc3fbc`): one body and a creator line.
    Letter { text_id: u32 },
    /// A page chain: the page list (`0xbc3fd0`), its first entry the object's first page
    /// (`0x4e342f`) and each next one as a show stores it after the page it shows (`0x4e3723`),
    /// and the shown page's index in it (`0xbc3fc4`). An entry not yet stored reads 0.
    Pages { list: Vec<u32>, page: usize },
}

impl ReadSource {
    /// A page chain read from its start: the list emptied and the page 0 (`0x4e3372`-`0x4e3386`).
    fn pages() -> Self {
        Self::Pages {
            list: Vec::new(),
            page: 0,
        }
    }
}

/// The page list's entry `i`, 0 where none is stored.
fn entry(list: &[u32], i: usize) -> u32 {
    list.get(i).copied().unwrap_or(0)
}

/// Store `id` as the page list's entry `i`.
fn store_entry(list: &mut Vec<u32>, i: usize, id: u32) {
    if list.len() <= i {
        list.resize(i + 1, 0);
    }
    list[i] = id;
}

pub(crate) struct ReadSession {
    /// An item's or a GameObject's guid; reading the same object again closes the reader.
    pub(crate) object_guid: u64,
    /// Which read this is: what its handlers signalled waits on the lane under it, and a verb its
    /// listeners call acts only on it ([`ItemTextLane::firing`]).
    serial: u64,
    pub(crate) source: ReadSource,
    /// The open's tail ran (`0x4e349a`): the latch holds this reader. The cell alone cannot say
    /// so, as another window may hold the latch on the same guid (a trade partner's).
    registered: bool,
    /// What the open or the show asked for and waits on: its answer runs the callback `0x4e3280`,
    /// which opens again (`0x4e32a5`) at the answer's packet.
    awaiting: Option<Wait>,
    /// The text cell (`0xbc3fe8`): empty at the open (`0x4e338f`), written by each show.
    text: String,
    /// The creators `ItemTextGetCreator` looked up and the name cache missed, each a registration
    /// of the callback `0x4e32b0` (`0x4e39b9`), spent by the name's answer.
    creator_waits: Vec<u64>,
}

impl ReadSession {
    /// The page cell plus one (`ItemTextGetPage`, `0x4e3adb`); a letter's is 1.
    fn page_number(&self) -> u32 {
        match &self.source {
            ReadSource::Pages { page, .. } => *page as u32 + 1,
            ReadSource::Letter { .. } => 1,
        }
    }

    /// No single text and a page id after the shown one (`ItemTextHasNextPage`, `0x4e3b25`).
    fn has_next(&self) -> bool {
        match &self.source {
            ReadSource::Pages { list, page } => entry(list, page + 1) != 0,
            ReadSource::Letter { .. } => false,
        }
    }
}

/// The record a reader's text comes from: a page of the page cache (`0xc0e174`), or a letter's
/// body of the item-text cache (`0xc0e0c0`), which mail shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextRecord {
    Page(u32),
    Body(u32),
}

/// A lookup the reader waits on, whose answer runs `0x4e3280`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// A text record (`0x4e3465`, `0x4e347c`, the show's `0x4e363c`, `0x4e36ab`).
    Record(TextRecord),
    /// An item's template, which its page id is read off with the callback (`0x4e33e5` →
    /// `CGItem_C::GetPageTextId 0x5d9e10`).
    Template(u32),
    /// A TEXT object's template, still in flight from its create: its pageID column reads 0
    /// (`0x5f5900`), and its handler's record callback opens the reader again when the record
    /// lands for the object in the cell (`TextHandlerOnRecordArrived 0x5f59d0` → `0x5f5a17`).
    ObjectTemplate(u32),
}

#[derive(Resource, Default)]
pub(crate) struct ItemTextOpen {
    pub(crate) pending: Option<ReadSession>,
    /// Reads a click or a bag use asked for, in order; [`apply_reads`] runs each as `0x4e32e0`
    /// with flag 0, with the world in hand for the latch.
    reads: Vec<(u64, ReadSource)>,
    /// The last read's serial.
    serials: u64,
}

impl ItemTextOpen {
    /// Read a bag letter; reading the open one again closes it. `true` when it is the open one.
    pub(crate) fn read_letter(&mut self, item_guid: u64, text_id: u32) -> bool {
        self.read(item_guid, ReadSource::Letter { text_id })
    }

    /// Read a page chain; the begin asks the object's template for the first page. `true` when
    /// it is the open one, which the read closes.
    pub(crate) fn read_pages(&mut self, object_guid: u64) -> bool {
        self.read(object_guid, ReadSource::pages())
    }

    fn read(&mut self, object_guid: u64, source: ReadSource) -> bool {
        let reread = self.object() == Some(object_guid);
        self.reads.push((object_guid, source));
        reread
    }

    /// The reader's cell `[0xbc3fa0]`: the object read, from the read on.
    pub(crate) fn object(&self) -> Option<u64> {
        self.pending.as_ref().map(|s| s.object_guid)
    }

    /// A session on `object_guid` with `source`, written as the cell is (`0x4e3364`).
    pub(crate) fn open(&mut self, object_guid: u64, source: ReadSource) {
        self.serials += 1;
        self.pending = Some(ReadSession {
            object_guid,
            serial: self.serials,
            source,
            registered: false,
            awaiting: None,
            text: String::new(),
            creator_waits: Vec::new(),
        });
    }

    /// A bag letter's session written directly, for a test that seats it.
    #[cfg(test)]
    pub(crate) fn open_letter(&mut self, item_guid: u64, text_id: u32) {
        self.open(item_guid, ReadSource::Letter { text_id });
    }
}

/// `CloseItemText` (`0x4e3b70` → `0x4e32e0(0, 0)`) is the core itself.
impl LatchedWindow for ItemTextOpen {
    const WINDOW: NpcWindow = NpcWindow::ItemText;

    fn cell(&self) -> Option<u64> {
        self.object()
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// The close, `0x4e32e0` with a zero guid: with an object read, the latch release on the cell
/// (`0x4e3357`), the cell cleared (`0x4e3364`) and `ITEM_TEXT_CLOSED` (`0x4e33a2`), behind what
/// the lane still holds ([`emit`]). While the reader holds the latch, the release's `0x493310`
/// runs this once more inside it (`0x49338e`).
pub(crate) fn close_core(world: &mut World) {
    let Some((object, serial)) = world
        .resource::<ItemTextOpen>()
        .pending
        .as_ref()
        .map(|s| (s.object_guid, s.serial))
    else {
        return;
    };
    crate::ui_session::release(world, object);
    world.resource_mut::<ItemTextOpen>().pending = None;
    emit(world, Place::Here, serial, TextSignal::Closed);
}

/// What the reader signals (`0x703e50`): `ITEM_TEXT_BEGIN` (293), `ITEM_TEXT_TRANSLATION` (294)
/// with its seconds, `ITEM_TEXT_READY` (295), `ITEM_TEXT_CLOSED` (296).
#[derive(Clone, Copy, Debug, PartialEq)]
enum TextSignal {
    Begin,
    Translation(f64),
    Ready,
    Closed,
}

impl TextSignal {
    fn fire(self, script: &mut UiScript) {
        match self {
            Self::Begin => script.fire_event("ITEM_TEXT_BEGIN", vec![]),
            Self::Translation(seconds) => {
                script.fire_event("ITEM_TEXT_TRANSLATION", vec![ScriptValue::Number(seconds)])
            }
            Self::Ready => script.fire_event("ITEM_TEXT_READY", vec![]),
            Self::Closed => script.fire_event("ITEM_TEXT_CLOSED", vec![]),
        }
    }
}

/// The item-text lane: what the handlers signalled, each stamped at its place
/// ([`crate::lanes::stamp`]) under the read it is of.
#[derive(Resource, Default)]
pub(crate) struct ItemTextLane {
    steps: std::collections::VecDeque<(crate::lanes::Stamp, u64, TextSignal)>,
    /// The read whose signal fires now, while its listeners run: a close or a turn they call acts
    /// on that read alone, never on a newer one a later packet opened.
    firing: Option<u64>,
}

impl ItemTextLane {
    /// Queue `signal` of read `serial` behind everything stamped before it.
    fn push(&mut self, serial: u64, signal: TextSignal) {
        let stamp = crate::lanes::stamp();
        let at = self.steps.partition_point(|(s, _, _)| *s < stamp);
        self.steps.insert(at, (stamp, serial, signal));
    }

    fn head(&self) -> Option<crate::lanes::Stamp> {
        self.steps.front().map(|(s, _, _)| *s)
    }

    /// A close run in place drops what its read still owes the listeners of its open and its
    /// show: a show after the close finds the cell cleared (`0x4e35de`).
    fn closed_in_place(&mut self, serial: u64) {
        self.steps.retain(|(_, s, signal)| {
            *s != serial || !matches!(signal, TextSignal::Begin | TextSignal::Ready)
        });
    }

    /// Whether a verb a listener calls may act on read `serial`, the live one.
    fn admits(&self, serial: u64) -> bool {
        self.firing.is_none_or(|firing| firing == serial)
    }
}

/// Where a step runs: inside a packet's handler, whose signals go on the lane at its place, or
/// outside the dispatch, where they fire at once unless the lane still holds an earlier signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Packet,
    Here,
}

/// Signal `signal` of read `serial` at `place`: on the lane at a packet, or behind what it holds
/// ([`crate::lanes::waits_behind`]); else at once, with the host lent. A close fired at once drops
/// what its read still owes ([`ItemTextLane::closed_in_place`]).
fn emit(world: &mut World, place: Place, serial: u64, signal: TextSignal) {
    if let Some(mut lane) = world.get_resource_mut::<ItemTextLane>() {
        if place == Place::Packet || crate::lanes::waits_behind(lane.head()) {
            lane.push(serial, signal);
            return;
        }
        if signal == TextSignal::Closed {
            lane.closed_in_place(serial);
        }
    }
    crate::ui_session::fire(world, |script| signal.fire(script));
}

/// The session's end and the leave-world sweep drop what the lane holds: no VM is left to hear
/// it, or no world for it to read.
fn drop_item_text_lane(world: &mut World) {
    if let Some(mut lane) = world.get_resource_mut::<ItemTextLane>() {
        lane.steps.clear();
    }
}

/// The lane's signals while no in-game interface is up, which no drain fires and no listener hears.
fn drop_unheard(mut lane: ResMut<ItemTextLane>) {
    lane.steps.clear();
}

/// The leave-world sweep's reader leg (`0x490abc` → `0x4e3240` → `0x4e32e0(0, 0)`): the lane's
/// signals dropped, then the core.
fn sweep_item_text(world: &mut World) {
    drop_item_text_lane(world);
    close_core(world);
}

/// The item-text lane's head.
fn peek_item_text(world: &World) -> Option<crate::lanes::Stamp> {
    world.get_resource::<ItemTextLane>()?.head()
}

/// Each signal `until` admits, fired as its stamp's firing with the host lent, under its read
/// ([`ItemTextLane::firing`]). The lane stops after a signal whose listeners stamped anything, so
/// what they pushed fires next.
fn fire_item_text(In(until): In<crate::lanes::Until>, world: &mut World) {
    loop {
        let Some((at, serial, signal)) = world
            .resource::<ItemTextLane>()
            .steps
            .front()
            .copied()
            .filter(|(at, _, _)| until.admits(*at))
        else {
            return;
        };
        world.resource_mut::<ItemTextLane>().steps.pop_front();
        let ((), moved) = crate::lanes::firing(at, || {
            let outer = world.resource_mut::<ItemTextLane>().firing.replace(serial);
            crate::ui_session::fire(world, |script| signal.fire(script));
            world.resource_mut::<ItemTextLane>().firing = outer;
        });
        if moved {
            return;
        }
    }
}

/// The lane's firing touches nothing beside the call host and its own lane.
fn item_text_reads(_: &mut World) -> bevy::ecs::query::FilteredAccessSet {
    bevy::ecs::query::FilteredAccessSet::default()
}

/// The queued reads, each `0x4e32e0(guid, 0)`, run with the world in hand ([`read_core`]).
fn apply_reads(mut open: ResMut<ItemTextOpen>, mut commands: Commands) {
    for (guid, source) in std::mem::take(&mut open.reads) {
        commands.queue(move |world: &mut World| read_core(world, guid, source, Place::Here));
    }
}

/// One read, `0x4e32e0(guid, 0)`. The open object again (`0x4e330c`) releases the latch on the
/// cell (`0x4e3323`), which closes the reader if it holds it, then fires `ITEM_TEXT_CLOSED`
/// (`0x4e332d`); one never begun keeps its cell. Another object releases the latch on the old cell
/// (`0x4e3357`), closing a begun reader through `0x493310` once, then takes the cell (`0x4e3364`)
/// and opens ([`open_tail`]).
fn read_core(world: &mut World, guid: u64, source: ReadSource, place: Place) {
    let cell = world
        .resource::<ItemTextOpen>()
        .pending
        .as_ref()
        .map(|s| (s.object_guid, s.serial));
    if let Some((_, serial)) = cell.filter(|(object, _)| *object == guid) {
        crate::ui_session::release(world, guid);
        emit(world, place, serial, TextSignal::Closed);
        return;
    }
    if let Some((old, _)) = cell {
        crate::ui_session::release(world, old);
    }
    world.resource_mut::<ItemTextOpen>().open(guid, source);
    open_tail(world, place);
}

/// The open's tail (`0x4e33b0`-`0x4e359b`), at a read or at the callback `0x4e3280` that opens
/// again (`0x4e32e0(guid, 1)`) when what the open waited on lands: the object's text looked up
/// ([`look_up_text`]), and only with it at hand the latch, whose holder closes first (`0x4e349a`),
/// then `ITEM_TEXT_BEGIN` (`0x4e34a4`); then a bag book's read request ([`request_read`]), or the
/// show (`0x4e359b`).
fn open_tail(world: &mut World, place: Place) {
    let Ok(Some((object, serial))) = world.run_system_cached(look_up_text) else {
        return;
    };
    crate::ui_session::register(world, object);
    emit(world, place, serial, TextSignal::Begin);
    match world.run_system_cached_with(request_read, object) {
        Ok(true) => {}
        Ok(false) => show_at(world, place, object, true),
        Err(e) => debug!("item text: the read request did not run: {e}"),
    }
}

/// What the open or the show waited on landed (an answer's packet, `place`): its callback
/// (`0x4e3280`, `0x4e32b0`, `0x5f59d0`) opens the reader again (`0x4e32e0(guid, 1)`), as the
/// reference does; an open still waiting finishes there ([`open_tail`]).
///
/// Deviation: for a reader the latch already holds, the reference's open again re-registers it
/// (`0x4e349a` → `0x4930d0` → `0x49310a` → `0x493310` → `0x49338e` `0x4e32e0(0, 0)`), which resets
/// the reader and signals `ITEM_TEXT_CLOSED` (`0x4e339d`); the `ITEM_TEXT_BEGIN` after it never
/// shows the frame again, and the show finds the cell cleared, so `ITEM_TEXT_READY` never fires
/// (`0x4e35de`). In 1.12.1 a turn to a page not yet cached closes the book, which stays closed, and
/// a letter whose author's name is not cached shows without its From line, then closes when the
/// name lands. `0x4930d0`'s same-target test (`0x4930eb`-`0x4930f3`) only skips a unit's greeting,
/// so this is a bug. benilla keeps the book open and shows it again ([`show_at`]): the page or the
/// From line appears when it lands.
fn answered(world: &mut World, wait: Wait, place: Place) {
    let Some(sess) = world
        .resource_mut::<ItemTextOpen>()
        .pending
        .as_mut()
        .filter(|s| s.awaiting == Some(wait))
        .map(|s| {
            s.awaiting = None;
            (s.object_guid, s.registered)
        })
    else {
        return;
    };
    reopen(world, sess, place);
}

/// `0x4e32e0(guid, 1)` from a callback on the open reader `(object, registered)`.
fn reopen(world: &mut World, (object, registered): (u64, bool), place: Place) {
    if registered {
        show_at(world, place, object, true);
    } else {
        open_tail(world, place);
    }
}

/// The show `0x4e35b0(guid, translate)` at `place`: `ITEM_TEXT_READY` when it wrote the page.
fn show_at(world: &mut World, place: Place, guid: u64, translate: bool) {
    match world.run_system_cached_with(show, (guid, translate)) {
        Ok(Some(serial)) => emit(world, place, serial, TextSignal::Ready),
        Ok(None) => {}
        Err(e) => debug!("item text: the show did not run: {e}"),
    }
}

/// [`ItemTextShow::show`] on the open reader when `guid` is its object; its serial when it showed.
fn show(
    In((guid, translate)): In<(u64, bool)>,
    mut open: ResMut<ItemTextOpen>,
    reads: ItemTextShow,
) -> Option<u64> {
    let sess = open.pending.as_mut().filter(|s| s.object_guid == guid)?;
    reads.show(sess, translate).then_some(sess.serial)
}

/// The open's lookups (`0x4e33b0`-`0x4e348b`), each with the callback `0x4e3280`: the object
/// (any type, `0x4e33c3`), which gone clears the cell and ends the open (`0x4e33d0`); its first
/// page (`vtbl+0x74`), an item's off its template, which in flight is waited on; then the record
/// the session starts at, a page chain's shown page (`0x4e345c`) or a letter's body, which not at
/// hand is waited on. A page chain with no page keeps its cell and stops (`0x4e341d`). With the record at hand,
/// the session is marked latched for the register that follows; its object and serial.
fn look_up_text(mut open: ResMut<ItemTextOpen>, reads: ItemTextShow) -> Option<(u64, u64)> {
    let sess = open.pending.as_mut()?;
    if sess.registered || sess.awaiting.is_some() {
        return None;
    }
    let object = sess.object_guid;
    let ask = Ask::Event(&reads.net);
    let head = match reads.page_head(object, ask) {
        Head::Gone => {
            open.pending = None;
            return None;
        }
        Head::Waits(wait) => {
            sess.awaiting = Some(wait);
            return None;
        }
        Head::Page(head) => head,
    };
    let record = match &mut sess.source {
        ReadSource::Pages { list, page } => {
            if head == 0 {
                debug!("item text: {object:#x} has no page to read");
                return None;
            }
            store_entry(list, 0, head);
            TextRecord::Page(entry(list, *page))
        }
        ReadSource::Letter { text_id } => TextRecord::Body(*text_id),
    };
    if !reads.record_at_hand(record, object, ask) {
        sess.awaiting = Some(Wait::Record(record));
        return None;
    }
    sess.registered = true;
    Some((object, sess.serial))
}

/// `0x5ec600` on a read's reply, inside its packet. OK finds the item (mask 2, `0x5ec635`), marks
/// it read (`0x5d9a80`, `ITEM_FIELD_FLAGS` bit 0x2) and shows its page translated (`0x4e35b0`); a
/// missing item does neither. FAILED reads a reason: 0 shows, 1 shows untranslated and signals
/// `ITEM_TEXT_TRANSLATION` with the time in seconds, whatever item it names, any other signals
/// `ITEM_TEXT_CLOSED` when the guid is the open item's, closing nothing else.
fn apply_reply(world: &mut World, reply: ReadReply) {
    match reply {
        ReadReply::Ok(guid) => {
            let Some(entity) = world
                .resource::<crate::net::GuidIndex>()
                .0
                .get(&guid)
                .copied()
            else {
                return;
            };
            let Some(mut store) = world.get_mut::<crate::net::ObjectStore>(entity) else {
                return;
            };
            if !store.0.is_item() {
                return;
            }
            let flags = store.0.item_flags().unwrap_or(0) | ITEM_FLAG_READ;
            store.0.merge(benilla_protocol::ObjectFields::from_pairs(&[(
                ITEM_FIELD_FLAGS,
                flags,
            )]));
            show_at(world, Place::Packet, guid, true);
        }
        ReadReply::Failed {
            guid,
            reason,
            milliseconds,
        } => match reason {
            0 => show_at(world, Place::Packet, guid, true),
            1 => {
                // The signal follows the show but is not gated by it (`0x5ec6a1`-`0x5ec6c5`): an
                // open item's comes after its `ITEM_TEXT_READY`, any other's alone.
                show_at(world, Place::Packet, guid, false);
                let seconds = f64::from(milliseconds) * 0.001;
                let serial = world
                    .resource::<ItemTextOpen>()
                    .pending
                    .as_ref()
                    .map_or(0, |s| s.serial);
                emit(
                    world,
                    Place::Packet,
                    serial,
                    TextSignal::Translation(seconds),
                );
            }
            _ => {
                let open = world
                    .resource::<ItemTextOpen>()
                    .pending
                    .as_ref()
                    .filter(|s| s.object_guid == guid)
                    .map(|s| s.serial);
                if let Some(serial) = open {
                    emit(world, Place::Packet, serial, TextSignal::Closed);
                }
            }
        },
    }
}

/// What a read item's reply says (`0x5ec600`).
#[derive(Clone, Copy, Debug)]
enum ReadReply {
    Ok(u64),
    Failed {
        guid: u64,
        reason: u8,
        milliseconds: i32,
    },
}

/// The open's last step for an item (`0x4e34a9`-`0x4e3586`), after `ITEM_TEXT_BEGIN`: one with a
/// page chain and no item-text id (`+0x9c`) whose `ITEM_FIELD_FLAGS` lack bit 0x2, which a read
/// reply sets (`0x5d9a80`), sends `CMSG_READ_ITEM` from its place in its container (`0x5ec580`)
/// and shows on the reply; with no active player, no container or no place in it the open ends
/// there, and nothing shows. `true` for such an item; any other object goes straight to the show
/// (`0x4e359b`).
fn request_read(
    In(object): In<u64>,
    open: Res<ItemTextOpen>,
    objects: Objects,
    net: Res<NetCommands>,
) -> bool {
    let Some(sess) = open.pending.as_ref().filter(|s| s.object_guid == object) else {
        return true;
    };
    let Some(item) = objects.object(object).filter(|f| f.is_item()) else {
        return false;
    };
    let reads_by_server = matches!(sess.source, ReadSource::Pages { .. })
        && item.item_text_id().unwrap_or(0) == 0
        && item.item_flags().unwrap_or(0) & ITEM_FLAG_READ == 0;
    if !reads_by_server {
        return false;
    }
    match read_position(object, &objects) {
        Some((bag, slot)) => {
            let _ = net.0.send(ClientCommand::ReadItem { bag, slot });
        }
        None => debug!("item text: {object:#x} has no place in a container to read from"),
    }
    true
}

/// `ITEM_FIELD_FLAGS` (field 21) and its bit 0x2, which `SMSG_READ_ITEM_OK` sets on the item
/// (`0x5d9a80`).
const ITEM_FIELD_FLAGS: u16 = 21;
const ITEM_FLAG_READ: u32 = 0x2;

/// The container byte and slot `0x5ec580` sends for the item: the player's own slots as 255 and the
/// index in the player's 113 guids, an equipped bag (inventory slots 19 to 22) as its slot and the
/// index in the bag (`0x5ec598`, `0x5ec5b3`-`0x5ec5e5`); a bank bag or an unequipped container
/// sends nothing.
fn read_position(item: u64, objects: &Objects) -> Option<(u8, u8)> {
    let contained = objects.object(item)?.item_contained()?;
    let player = objects.player_fields()?;
    if objects.entity(contained) == objects.player() {
        let index = (0..=112u8).find(|&i| player_slot(player, i) == Some(item))?;
        return Some((0xFF, index));
    }
    let bag = (19..23u8).find(|&i| player.player_inv_slot(i) == Some(contained))?;
    let container = objects.object(contained)?;
    let index = (0..36u8).find(|&i| container.container_slot(i) == Some(item))?;
    Some((bag, index))
}

/// Slot `i` of the player's guid array: equipment and equipped bags (0 to 22), the backpack
/// (23 to 38), the bank (39 to 62), the bank bags (63 to 68), the buyback (69 to 80) and the
/// keyring (81 to 112).
fn player_slot(player: &benilla_protocol::ObjectFields, i: u8) -> Option<u64> {
    match i {
        0..=22 => player.player_inv_slot(i),
        23..=38 => player.player_pack_slot(i - 23),
        39..=62 => player.player_bank_slot(i - 39),
        63..=68 => player.player_bank_bag_slot(i - 63),
        69..=80 => player.player_buyback_slot(i - 69),
        81..=112 => player.player_keyring_slot(i - 81),
        _ => None,
    }
}

/// A read of `object_guid`'s pages run whole, as a click's apply runs it, for a test.
#[cfg(test)]
pub(crate) fn read_now(world: &mut World, object_guid: u64) {
    read_core(world, object_guid, ReadSource::pages(), Place::Here);
}

/// A read of the bag letter `item_guid` run whole, as a bag use's apply runs it, for a test.
#[cfg(test)]
pub(crate) fn read_letter_now(world: &mut World, item_guid: u64, text_id: u32) {
    read_core(
        world,
        item_guid,
        ReadSource::Letter { text_id },
        Place::Here,
    );
}

/// The reader's packet handlers, each running its step whole at its packet.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{Place, TextRecord, Wait};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text)
            .net_handler(
                SessionEventKind::GameObjectPageText,
                on_gameobject_page_text,
            )
            .net_handler(SessionEventKind::MailItemText, on_item_text)
            .net_handler(SessionEventKind::ItemTemplate, on_item_template)
            .net_handler(SessionEventKind::GameObjectInfo, on_object_template)
            .net_handler(SessionEventKind::PlayerName, on_player_name)
            .net_handler(SessionEventKind::ReadItemOk, on_read_item)
            .net_handler(SessionEventKind::ReadItemFailed, on_read_item);
    }

    /// `SMSG_READ_ITEM_OK` and `SMSG_READ_ITEM_FAILED` (`0x5e7d90` → `0x5ec600`), applied inside
    /// their packet ([`super::apply_reply`]).
    fn on_read_item(In(ev): In<SessionEvent>, world: &mut World) {
        let reply = match ev {
            SessionEvent::ReadItemOk { guid } => super::ReadReply::Ok(guid),
            SessionEvent::ReadItemFailed {
                guid,
                reason,
                milliseconds,
            } => super::ReadReply::Failed {
                guid,
                reason,
                milliseconds,
            },
            _ => return,
        };
        super::apply_reply(world, reply);
    }

    /// `SMSG_ITEM_TEXT_QUERY_RESPONSE`: a letter's body landing, whose callback `0x4e3280` opens
    /// the reader again when it waits on it; the mail handler caches the body ahead of this.
    fn on_item_text(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::MailItemText { text_id, .. } = ev {
            super::answered(
                world,
                Wait::Record(TextRecord::Body(text_id)),
                Place::Packet,
            );
        }
    }

    /// `SMSG_ITEM_QUERY_SINGLE_RESPONSE`: the template the open's page id waited on (`0x5d9e10`'s
    /// callback, `0x4e3280`), filed by the item handler ahead of this; a refusal runs the callback
    /// with status 0, which opens nothing (`0x4e3286`).
    fn on_item_template(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::ItemTemplate {
            entry,
            info: Some(_),
        } = ev
        {
            super::answered(world, Wait::Template(entry), Place::Packet);
        }
    }

    /// `SMSG_GAMEOBJECT_QUERY_RESPONSE`, filed by the object handler ahead of this: a TEXT
    /// object's handler opens the reader again when the record lands for the object in the cell
    /// (`0x5f59d0` → `0x5f5a17`).
    fn on_object_template(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::GameObjectInfo { entry, .. } = ev {
            super::answered(world, Wait::ObjectTemplate(entry), Place::Packet);
        }
    }

    /// `SMSG_NAME_QUERY_RESPONSE`, filed by the name cache ahead of this: each creator lookup that
    /// waited on the guid runs `0x4e32b0`, which opens the open reader again (`0x4e32d5`) when the
    /// name landed found (`0x4e32b6`); an empty name is a refusal, status 0.
    fn on_player_name(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::PlayerName { guid, name, .. } = ev else {
            return;
        };
        let Some((sess, waits)) = world
            .resource_mut::<super::ItemTextOpen>()
            .pending
            .as_mut()
            .map(|s| {
                let before = s.creator_waits.len();
                s.creator_waits.retain(|g| *g != guid);
                (
                    (s.object_guid, s.registered),
                    before - s.creator_waits.len(),
                )
            })
        else {
            return;
        };
        if name.is_empty() {
            return;
        }
        for _ in 0..waits {
            super::reopen(world, sess, Place::Packet);
        }
    }

    /// `SMSG_GAMEOBJECT_PAGETEXT` (`0x5f88c0`): the guid looked up as a GameObject (mask `0x20`,
    /// `0x5f88f9`), then the reader's open on it with flag 0 (`0x5f8915` → `0x4e32e0`), the same
    /// read a TEXT object's right-click makes, inside the packet; nothing for an object not
    /// streamed.
    fn on_gameobject_page_text(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::GameObjectPageText { guid } = ev else {
            return;
        };
        let is_gameobject = world
            .resource::<crate::net::GuidIndex>()
            .0
            .get(&guid)
            .and_then(|&e| world.get::<crate::net::ObjectStore>(e))
            .is_some_and(|f| {
                f.0.object_type() == Some(benilla_protocol::messages::ObjectType::GameObject)
            });
        if is_gameobject {
            super::read_core(world, guid, super::ReadSource::pages(), Place::Packet);
        }
    }

    /// One page per packet, cached; a reader that waits on the page opens again at its place
    /// (`0x4e3280`).
    fn on_page_text(In(ev): In<SessionEvent>, world: &mut World) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = ev
        {
            world
                .resource_mut::<super::PageTexts>()
                .insert(page_id, text, next_page_id);
            super::answered(
                world,
                Wait::Record(TextRecord::Page(page_id)),
                Place::Packet,
            );
        }
    }
}

pub(crate) struct UiItemTextPlugin;

impl Plugin for UiItemTextPlugin {
    fn build(&self, app: &mut App) {
        use crate::lanes::LaneApp;
        net::register(app);
        app.init_resource::<ItemTextOpen>()
            .init_resource::<ItemTextLane>()
            .init_resource::<PageTexts>()
            .add_exclusive_lane("item text", peek_item_text, fire_item_text, item_text_reads)
            .add_latched_window::<ItemTextOpen>()
            .add_sweep_leg(SweepLeg::ItemText, sweep_item_text)
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                drop_item_text_lane.in_set(crate::ui_script::SessionEnd::Clears),
            )
            .add_systems(
                Update,
                (
                    // With no in-game interface up no drain fires the lane, and no listener
                    // hears it.
                    drop_unheard.run_if(not(crate::ui_script::ingame_ui_up)),
                    apply_reads,
                )
                    .chain()
                    .in_set(UiFeed),
            );
    }
}

/// What the read object answers: its name for the title (`ItemTextGetItem` `0x4e38f0`) and the
/// language its pages are rewritten in.
struct Readable {
    title: String,
    /// The language the page text is rewritten in at the show (`0x4e3856`): an item template's
    /// (`+0x1a0`, `0x4e37e9`) unless the read marked the item (`ITEM_FIELD_FLAGS` bit 0x2,
    /// `0x4e3762`), a GameObject's language column (`0x4e37fc`) always; `None` shows it as it is.
    language: Option<u32>,
}

/// The material a quest from `guid` paints on, `GetQuestBackgroundMaterial` (`0x502230`), the
/// body of `ItemTextGetMaterial`; `None` is FrameXML's Parchment. Both only read the records
/// (`0x50229a`, `0x4e3a5a`).
pub(crate) fn object_material(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
) -> Option<String> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    let fields = objects.object(guid)?;
    // The descriptor's type mask decides (`0x50227b`-`0x5022b5`, `0x4e3a38`-`0x4e3a73`): an item
    // first, then a GameObject, anything else none.
    if fields.is_item() {
        let t = items.template(fields.object_entry()?, guid, Ask::Read)?;
        return name(t.page_material);
    }
    if is_gameobject(fields) {
        return name(go_templates.of(fields)?.page_material(fields));
    }
    None
}

/// The object's [`Readable`], item or GameObject alike, its records only read; `None` with no
/// object or no record.
fn readable(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
) -> Option<Readable> {
    let fields = objects.object(guid)?;
    if is_gameobject(fields) {
        let go = go_templates.of(fields)?;
        return Some(Readable {
            title: go.name.clone(),
            language: Some(go.language(fields)),
        });
    }
    let t = items.template(fields.object_entry()?, guid, Ask::Read)?;
    Some(Readable {
        title: t.name.clone(),
        language: (fields.item_flags().unwrap_or(0) & ITEM_FLAG_READ == 0).then_some(t.language_id),
    })
}

/// `GAMEOBJECT_TYPE_TEXT`, whose handler reopens the reader when its record lands (`0x5f59d0`).
const GO_TYPE_TEXT: i32 = 9;

/// Whether the object is a GameObject (type mask `0x20`), whose virtuals the reader calls; the
/// object's own descriptor says, not its guid.
fn is_gameobject(fields: &benilla_protocol::ObjectFields) -> bool {
    fields.object_type_mask().is_some_and(|m| m & 0x20 != 0)
}

/// An object's first page as the open reads it (`vtbl+0x74`).
enum Head {
    /// No object (`0x4e33ce`).
    Gone,
    /// An item whose template is in flight, asked with the callback (`0x5d9e10`), or a TEXT
    /// object's, whose handler reopens at its arrival (`0x5f59d0`): no page yet.
    Waits(Wait),
    /// The page id, 0 for none (a letter, a template with no page).
    Page(u32),
}

/// What the open and the show read: the object, its records, the text caches, and the page's
/// expansion, masking and rewrite.
#[derive(SystemParam)]
pub(crate) struct ItemTextShow<'w, 's> {
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    go_templates: Res<'w, GameObjectTemplates>,
    pages: Res<'w, PageTexts>,
    /// The mail session's body cache is the reader's too ([`crate::ui_mail::MailVerbs`]).
    mail: Res<'w, MailOpen>,
    /// The name cache and the world states the `$` macros read; absent in a harness without them.
    names: Option<Res<'w, NameCache>>,
    net: Res<'w, NetCommands>,
    // The `$`-macro subject for the page body: the local player.
    self_q: Query<
        'w,
        's,
        (&'static crate::net::ObjectStore, &'static crate::net::Guid),
        With<crate::net::SelfPlayer>,
    >,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    unit_names: crate::ui_unit::NameTables<'w>,
    filter: Option<Res<'w, crate::text_filter::TextFilter>>,
    switches: Option<Res<'w, crate::text_filter::TextFilterSwitches>>,
    // The word pools and the character's skill per language as the show runs (`0x4e3837`).
    languages: crate::ui_chat::LanguageReads<'w, 's>,
}

impl ItemTextShow<'_, '_> {
    /// The object's first page (`vtbl+0x74`): a GameObject's pageID column (`0x5f5900`), an
    /// item's template `PageText` (`0x5d9e10`), asked as `ask` says.
    fn page_head(&self, guid: u64, ask: Ask<'_>) -> Head {
        let Some(fields) = self.objects.object(guid) else {
            return Head::Gone;
        };
        let Some(entry) = fields.object_entry() else {
            return Head::Page(0);
        };
        if is_gameobject(fields) {
            return match self.go_templates.of(fields) {
                Some(go) => Head::Page(go.page_id(fields)),
                None if fields.gameobject_type_id() == GO_TYPE_TEXT => {
                    Head::Waits(Wait::ObjectTemplate(entry))
                }
                None => Head::Page(0),
            };
        }
        match self.items.template(entry, guid, ask) {
            Some(t) => Head::Page(t.page_text),
            None => Head::Waits(Wait::Template(entry)),
        }
    }

    /// Whether `record` is cached, asked as `ask` says on a miss.
    fn record_at_hand(&self, record: TextRecord, object: u64, ask: Ask<'_>) -> bool {
        match record {
            TextRecord::Page(page_id) => self.pages.page(page_id, object, ask).is_some(),
            TextRecord::Body(text_id) => self.body(text_id, ask).is_some(),
        }
    }

    fn body(&self, text_id: u32, ask: Ask<'_>) -> Option<&String> {
        self.mail.bodies.read(text_id, ask, |commands| {
            let _ = commands.0.send(ClientCommand::ItemTextQuery {
                text_id,
                mail_id: 0,
            });
        })
    }

    /// `0x4e35b0`'s body on the open `sess`, whose object it found: the record of a letter's body
    /// or of the shown page looked up with the callback (`0x4e363c`, `0x4e36ab`), which not at hand
    /// is waited on and shows nothing; else a page's next id stored after it (`0x4e3723`), the
    /// text expanded for the player (`0x506f70`), a letter's masked (`0x4e3672`), a page rewritten
    /// in its object's language (`0x4e3856`), into the text cell. Whether it showed.
    fn show(&self, sess: &mut ReadSession, translate: bool) -> bool {
        let object = sess.object_guid;
        if self.objects.object(object).is_none() {
            return false;
        }
        let ask = Ask::Event(&self.net);
        let is_letter = matches!(sess.source, ReadSource::Letter { .. });
        let raw = match &mut sess.source {
            ReadSource::Letter { text_id } => match self.body(*text_id, ask) {
                Some(body) => body.clone(),
                None => {
                    sess.awaiting = Some(Wait::Record(TextRecord::Body(*text_id)));
                    return false;
                }
            },
            ReadSource::Pages { list, page } => {
                let page_id = entry(list, *page);
                let Some(record) = self.pages.page(page_id, object, ask) else {
                    sess.awaiting = Some(Wait::Record(TextRecord::Page(page_id)));
                    return false;
                };
                store_entry(list, *page + 1, record.next);
                record.text.clone()
            }
        };
        // Server-authored text runs the `$`-macro expander, as in the reference's
        // `ItemTextFrame.cpp`.
        let subject = self.names.as_deref().and_then(|names| {
            crate::npc_text::player_identity(&self.self_q, names, self.unit_names.get())
        });
        let no_states = crate::world_state::WorldStates::default();
        let mut text = crate::npc_text::substitute(
            &raw,
            &crate::npc_text::MacroContext {
                subject: subject.as_ref(),
                states: self.states.as_deref().unwrap_or(&no_states),
            },
        );
        // A single item text is masked after the expansion (`0x4e3672`, `dl = 0`); a page chain's
        // pages are not (`0x4e3744`-`0x4e3860` reach no masker).
        if let (true, Some(filter)) = (is_letter, self.filter.as_deref()) {
            filter.mask(
                self.switches.as_deref().is_some_and(|s| s.profanity),
                &mut text,
            );
        }
        // A page is rewritten in its object's language to the player's skill (`0x4e3856`,
        // `0x49b560`), or to skill 0 when the show is a reason-1 read reply's (`0x4e37b3`); a
        // single item text is not (`0x4e3666`-`0x4e3677`).
        let language = readable(object, &self.objects, &self.items, &self.go_templates)
            .and_then(|r| r.language);
        if let (false, Some(language)) = (is_letter, language) {
            text = self
                .languages
                .gate()
                .garble_item_text(language, translate, &text);
        }
        sess.text = text;
        true
    }
}

/// `PageTextMaterial.dbc`; absent without client data, leaving every reader on Parchment.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

/// The reader's verbs inside their calls ([`benilla_ui::script::ItemTextCall`]): the page turns
/// and the creator's lookup here, the close core in the host
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct ItemTextSteps<'w, 's> {
    /// Absent in a harness without the reader.
    open: Option<ResMut<'w, ItemTextOpen>>,
    lane: Option<ResMut<'w, ItemTextLane>>,
    /// Absent in a harness without the objects or the caches.
    show: Option<ItemTextShow<'w, 's>>,
}

impl ItemTextSteps<'_, '_> {
    /// The open read a verb acts on: the live one, unless a signal of another read fires now
    /// ([`ItemTextLane::admits`]).
    fn session(&mut self) -> Option<&mut ReadSession> {
        let lane = self.lane.as_deref();
        self.open
            .as_deref_mut()?
            .pending
            .as_mut()
            .filter(|s| lane.is_none_or(|l| l.admits(s.serial)))
    }

    /// `ItemTextPrevPage()` (`0x4e3870`) or `ItemTextNextPage()` (`0x4e38b0`) as `delta`: on an
    /// open page chain, back while the page is above the first (`0x4e388d`), on while the list
    /// holds a page after the shown one (`0x4e38d5`), then the show with `translate` set
    /// (`0x4e35b0`), which asks for a page not cached. Whether it showed, for the call to signal
    /// `ITEM_TEXT_READY` (`0x4e3860`) inside it.
    pub(crate) fn turn(&mut self, delta: i32) -> bool {
        let Some(sess) = self.session() else {
            return false;
        };
        let ReadSource::Pages { list, page } = &mut sess.source else {
            return false; // a letter has no pages; its buttons never show
        };
        if !turn_page(list, page, delta) {
            return false;
        }
        let Some(show) = self.show.as_ref() else {
            return false;
        };
        let Some(sess) = self.open.as_deref_mut().and_then(|o| o.pending.as_mut()) else {
            return false;
        };
        show.show(sess, true)
    }

    /// The reader's cell `[0xbc3fa0]`, as a close its listeners call finds it.
    pub(crate) fn object(&mut self) -> Option<u64> {
        self.session().map(|s| s.object_guid)
    }

    /// The close core's cell clear (`0x4e3364`), in place: what the read still owes its listeners
    /// is dropped ([`ItemTextLane::closed_in_place`]).
    pub(crate) fn clear(&mut self) {
        let Some(open) = self.open.as_deref_mut() else {
            return;
        };
        if let Some(sess) = open.pending.take() {
            if let Some(lane) = self.lane.as_deref_mut() {
                lane.closed_in_place(sess.serial);
            }
        }
    }

    /// `ItemTextGetCreator` on a creator the name cache missed: the lookup with the callback
    /// `0x4e32b0` (`0x4e39b9`), which asks, kept once per call on the open read.
    pub(crate) fn wait_for_creator(&mut self, guid: u64) {
        let Some(show) = self.show.as_ref() else {
            return;
        };
        let asked = show
            .names
            .as_deref()
            .map(|names| names.resolve_player(guid, Ask::Event(&show.net)).is_some());
        if asked != Some(false) {
            return;
        }
        if let Some(sess) = self.session() {
            sess.creator_waits.push(guid);
        }
    }
}

/// Move the shown page one step on `list` (the reference's array at `0xbc3fd0`): Next to the
/// entry after it when non-zero (`0x4e38d5`, `jle`), Prev while above the first (`0x4e388d`);
/// whether it moved.
fn turn_page(list: &[u32], page: &mut usize, delta: i32) -> bool {
    if delta > 0 {
        if entry(list, *page + 1) as i32 <= 0 {
            return false;
        }
        *page += 1;
    } else {
        if *page == 0 {
            return false; // already on page 1
        }
        *page -= 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The open reader's text cell (`0xbc3fe8`), which the getter reads through the host.
    fn shown(app: &App) -> String {
        app.world()
            .resource::<ItemTextOpen>()
            .pending
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    }

    /// A profanity list masking the one word `darn`.
    fn darn_filter() -> crate::text_filter::TextFilter {
        crate::text_filter::TextFilter::new(
            &[benilla_formats::FilterPattern {
                id: 1,
                pattern: r"\<darn\>".into(),
            }],
            &[],
        )
    }

    /// A single item text (a letter's body) is masked after the expansion (`0x4e3672`, `dl = 0`),
    /// gated by `profanityFilter`; `ItemTextGetText` returns the masked text.
    #[test]
    fn a_letters_text_is_masked_when_the_filter_is_on() {
        const FIELD_OBJECT_ENTRY: u16 = 3;
        const LETTER: u64 = 0x4000_0000_0000_0009;
        for (profanity, want) in [(true, "Oh !@#$ it."), (false, "Oh darn it.")] {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.insert_resource(NetCommands(tx))
                .init_resource::<ItemTextOpen>()
                .init_resource::<PageTexts>()
                .init_resource::<MailOpen>()
                .init_resource::<Items>()
                .init_resource::<NameCache>()
                .init_resource::<GameObjectTemplates>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::world_state::WorldStates>()
                .insert_resource(darn_filter())
                .insert_resource(crate::text_filter::TextFilterSwitches {
                    profanity,
                    ..Default::default()
                })
                .add_systems(Update, apply_reads);
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(5_000, Some(crate::items::test_template("Letter")));
            crate::items::test_spawn_item(
                app.world_mut(),
                LETTER,
                benilla_protocol::ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 5_000)]),
                false,
            );
            app.world_mut()
                .resource_mut::<MailOpen>()
                .bodies
                .insert(77, "Oh darn it.".into());
            app.world_mut()
                .resource_mut::<ItemTextOpen>()
                .read_letter(LETTER, 77);
            for _ in 0..4 {
                app.update();
            }
            assert_eq!(shown(&app), want, "profanityFilter {profanity}");
        }
    }

    /// A page chain's pages reach no masker: `0x4e35b0`'s page arm (`0x4e3744`-`0x4e3860`) never
    /// calls `0x4a1a60`, so a book shows its words as written whatever `profanityFilter` is.
    #[test]
    fn a_books_page_is_not_masked() {
        const TABLET: u64 = 0xF110_0043_2400_0001;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<ItemTextOpen>()
            .init_resource::<ItemTextLane>()
            .init_resource::<PageTexts>()
            .init_resource::<MailOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<GameObjectTemplates>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::world_state::WorldStates>()
            .insert_resource(darn_filter())
            .init_resource::<crate::text_filter::TextFilterSwitches>()
            .add_systems(Update, apply_reads);
        net::register(&mut app);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let mut data = [0i32; 24];
        data[7] = 358;
        app.world_mut()
            .resource_mut::<GameObjectTemplates>()
            .insert(17188, 10, "The Lay".into(), &data);
        let tablet = app
            .world_mut()
            .spawn((
                crate::net::Guid(TABLET),
                crate::net::ObjectStore(crate::go_templates::test_typed_fields(17188, 10)),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(TABLET, tablet);
        let fire = |app: &mut App, events| {
            crate::net::handlers::dispatch(app.world_mut(), events);
            app.world_mut()
                .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
                .expect("the item-text lane fires");
        };
        fire(
            &mut app,
            vec![benilla_protocol::SessionEvent::GameObjectPageText { guid: TABLET }],
        );
        app.update();
        app.update();
        fire(
            &mut app,
            vec![benilla_protocol::SessionEvent::PageText {
                page_id: 358,
                text: "Oh darn it.".into(),
                next_page_id: 0,
            }],
        );
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(shown(&app), "Oh darn it.");
    }

    /// A bag book (a page chain, no item-text id, `ITEM_FIELD_FLAGS` bit 0x2 clear) sends
    /// `CMSG_READ_ITEM` from its pack slot once its begin has run (`0x4e3586` → `0x5ec580`) and
    /// shows nothing until the reply: OK marks the item read and shows (`0x5ec640`), a later read
    /// of it sends nothing; FAILED shows (reason 0), shows untranslated and signals
    /// `ITEM_TEXT_TRANSLATION` in seconds (1) or only signals `ITEM_TEXT_CLOSED` (2) (`0x5ec600`).
    #[test]
    fn a_bag_book_asks_the_server_and_shows_on_the_reply() {
        use benilla_protocol::SessionEvent as E;
        const BOOK: u64 = 0x4000_0000_0000_0021;
        const ME: u64 = 0x10;
        const PACK_SLOT_1: u16 = 532;
        for reply in ["ok", "reason 0", "reason 1", "reason 2"] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.insert_resource(NetCommands(tx))
                .init_resource::<ItemTextOpen>()
                .init_resource::<ItemTextLane>()
                .init_resource::<PageTexts>()
                .init_resource::<MailOpen>()
                .init_resource::<Items>()
                .init_resource::<NameCache>()
                .init_resource::<GameObjectTemplates>()
                .init_resource::<crate::net::GuidIndex>()
                .init_resource::<crate::world_state::WorldStates>()
                .add_systems(Update, apply_reads);
            net::register(&mut app);
            let script = UiScript::new().unwrap();
            script
                .run(
                    r#"SEEN = {} local f = CreateFrame("Frame")
                    for _, e in ipairs({"ITEM_TEXT_BEGIN", "ITEM_TEXT_READY",
                            "ITEM_TEXT_TRANSLATION", "ITEM_TEXT_CLOSED"}) do f:RegisterEvent(e) end
                    f:SetScript("OnEvent", function()
                        table.insert(SEEN, event .. (arg1 and (":" .. arg1) or ""))
                    end)"#,
                )
                .unwrap();
            app.insert_non_send_resource(script);
            let world = app.world_mut();
            let mut template = crate::items::test_template("Gnarled Book");
            template.page_text = 358;
            world
                .resource_mut::<Items>()
                .insert_template(5_000, Some(template));
            world
                .resource_mut::<PageTexts>()
                .insert(358, "Once upon a time.".into(), 0);
            let player = world
                .spawn((
                    crate::net::Guid(ME),
                    crate::net::SelfPlayer,
                    crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                        (PACK_SLOT_1 + 6, BOOK as u32),
                        (PACK_SLOT_1 + 7, (BOOK >> 32) as u32),
                    ])),
                ))
                .id();
            world
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(ME, player);
            crate::items::test_spawn_item(
                world,
                BOOK,
                benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x3),
                    (3, 5_000),
                    (8, ME as u32),
                    (9, (ME >> 32) as u32),
                ]),
                false,
            );
            let seen = |app: &mut App| -> String {
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<String>("local s = table.concat(SEEN, ',') SEEN = {} return s")
                    .expect("runs")
            };
            let reads = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<(u8, u8)> {
                rx.try_iter()
                    .filter_map(|c| match c {
                        ClientCommand::ReadItem { bag, slot } => Some((bag, slot)),
                        _ => None,
                    })
                    .collect()
            };

            app.world_mut()
                .resource_mut::<ItemTextOpen>()
                .read_pages(BOOK);
            for _ in 0..4 {
                app.update();
            }
            assert_eq!(
                reads(&rx),
                [(0xFF, 23 + 3)],
                "{reply}: the read, from pack slot 4"
            );
            assert_eq!(
                seen(&mut app),
                "ITEM_TEXT_BEGIN",
                "{reply}: no page before the reply"
            );

            let reply_events = match reply {
                "ok" => vec![E::ReadItemOk { guid: BOOK }],
                other => vec![E::ReadItemFailed {
                    guid: BOOK,
                    reason: other.strip_prefix("reason ").unwrap().parse().unwrap(),
                    milliseconds: 2_500,
                }],
            };
            crate::net::handlers::dispatch(app.world_mut(), reply_events);
            app.world_mut()
                .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
                .expect("the item-text lane fires");
            for _ in 0..4 {
                app.update();
            }
            let want = match reply {
                "ok" | "reason 0" => "ITEM_TEXT_READY",
                "reason 1" => "ITEM_TEXT_READY,ITEM_TEXT_TRANSLATION:2.5",
                _ => "ITEM_TEXT_CLOSED",
            };
            assert_eq!(seen(&mut app), want, "{reply}");
            if reply == "reason 1" {
                // The signal does not wait on the item being the open one (`0x5ec6a1`).
                let stranger = vec![E::ReadItemFailed {
                    guid: BOOK + 1,
                    reason: 1,
                    milliseconds: 175,
                }];
                crate::net::handlers::dispatch(app.world_mut(), stranger);
                app.world_mut()
                    .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
                    .expect("the item-text lane fires");
                assert_eq!(seen(&mut app), "ITEM_TEXT_TRANSLATION:0.175");
            }
            if reply == "ok" {
                let flags = app
                    .world()
                    .resource::<crate::net::GuidIndex>()
                    .0
                    .get(&BOOK)
                    .and_then(|&e| app.world().get::<crate::net::ObjectStore>(e))
                    .and_then(|s| s.0.item_flags());
                assert_eq!(flags, Some(2), "the item is marked read (`0x5d9a80`)");
                // A read of it again: no request, the page at once (`0x4e359b`).
                close_core(app.world_mut());
                app.world_mut()
                    .resource_mut::<ItemTextOpen>()
                    .read_pages(BOOK);
                for _ in 0..4 {
                    app.update();
                }
                assert_eq!(reads(&rx), Vec::<(u8, u8)>::new());
                let events = seen(&mut app);
                assert!(
                    events.ends_with("ITEM_TEXT_BEGIN,ITEM_TEXT_READY"),
                    "{events}"
                );
            }
        }
    }

    /// `SMSG_GAMEOBJECT_PAGETEXT` (`0x5f88c0`) resolves the GameObject and opens the reader on it
    /// (`0x4e32e0(guid, 0)`), whose page is the object's pageID column (`0x5f5900`, a GOOBER's
    /// `data[7]`) and its material the pageMaterial column (`0x5f5950`, `data[9]`): "The Lay of
    /// Ameth'Aran", entry 17188, page 358.
    #[test]
    fn a_goobers_page_text_opens_the_reader_on_its_page() {
        const TABLET: u64 = 0xF110_0043_2400_0001;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<ItemTextOpen>()
            .init_resource::<ItemTextLane>()
            .init_resource::<PageTexts>()
            .init_resource::<MailOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<GameObjectTemplates>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::world_state::WorldStates>()
            .add_systems(Update, apply_reads);
        net::register(&mut app);
        app.insert_non_send_resource(UiScript::new().unwrap());
        let mut data = [0i32; 24];
        data[7] = 358;
        data[9] = 2;
        app.world_mut()
            .resource_mut::<GameObjectTemplates>()
            .insert(17188, 10, "The Lay of Ameth'Aran".into(), &data);
        let tablet = app
            .world_mut()
            .spawn((
                crate::net::Guid(TABLET),
                crate::net::ObjectStore(crate::go_templates::test_typed_fields(17188, 10)),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(TABLET, tablet);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::GameObjectPageText { guid: TABLET }],
        );
        app.world_mut()
            .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
            .expect("the item-text lane fires");
        app.update();
        app.update();
        let open = app.world().resource::<ItemTextOpen>();
        let sess = open
            .pending
            .as_ref()
            .expect("the reader opens on the tablet");
        assert_eq!(sess.object_guid, TABLET);
        assert!(!sess.registered, "the begin waits on its page");
        let asked: Vec<u32> = crate::query_cache::queries(&rx)
            .iter()
            .filter_map(|c| match c {
                ClientCommand::PageTextQuery { page_id, .. } => Some(*page_id),
                _ => None,
            })
            .collect();
        assert_eq!(asked, [358], "its page is data[7]");
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::PageText {
                page_id: 358,
                text: "Page one.".into(),
                next_page_id: 0,
            }],
        );
        app.world_mut()
            .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
            .expect("the item-text lane fires");
        let open = app.world().resource::<ItemTextOpen>();
        assert!(
            open.pending.as_ref().is_some_and(|s| s.registered),
            "the page's answer begins it"
        );
    }

    /// Next moves while the list holds a page after the shown one (`0x4e38d5`), Prev while above
    /// the first (`0x4e388d`); the list keeps the pages a show stored (`0x4e3723`).
    #[test]
    fn page_turns_walk_the_list_both_ways() {
        let list = [100, 101, 102, 0];
        let mut page = 0;
        assert!(!turn_page(&list, &mut page, -1), "page 1 has no Prev");
        assert!(turn_page(&list, &mut page, 1));
        assert!(turn_page(&list, &mut page, 1));
        assert_eq!(page, 2);
        assert!(!turn_page(&list, &mut page, 1), "the last page has no Next");
        assert!(turn_page(&list, &mut page, -1));
        assert_eq!(page, 1);
    }

    /// A page whose show has not stored its next has no Next.
    #[test]
    fn a_page_not_yet_shown_refuses_the_turn() {
        let mut page = 0;
        assert!(!turn_page(&[7], &mut page, 1));
        assert_eq!(page, 0);
    }

    #[test]
    fn a_page_read_opens_before_its_template_lands() {
        let mut world = World::new();
        world.init_resource::<ItemTextOpen>();
        assert!(!world
            .resource_mut::<ItemTextOpen>()
            .read_pages(0xF110_0000_0000_0001));
        use bevy::ecs::system::RunSystemOnce;
        world.run_system_once(apply_reads).unwrap();
        world.flush();
        let open = world.resource::<ItemTextOpen>();
        let sess = open.pending.as_ref().expect("open");
        assert!(matches!(&sess.source, ReadSource::Pages { list, page: 0 } if list.is_empty()));
    }

    /// The word pool of language 1 (the tests' own): every five-letter word becomes `kazum`.
    fn orcish() -> benilla_formats::LanguageWords {
        benilla_formats::LanguageWords::from_rows([(1, "kazum".to_string())])
    }

    /// Our player in [`reading_app`].
    const READER: u64 = 0x10;

    /// The reader's resources over our player, who knows language 1 to `skill` and holds `fields`
    /// besides.
    fn reading_app(
        skill: u16,
        fields: &[(u16, u32)],
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<ItemTextOpen>()
            .init_resource::<ItemTextLane>()
            .init_resource::<PageTexts>()
            .init_resource::<MailOpen>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<GameObjectTemplates>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::world_state::WorldStates>()
            .insert_resource(crate::ui_chat::ChatLanguages::for_test(orcish()))
            .add_systems(Update, apply_reads);
        net::register(&mut app);
        let world = app.world_mut();
        crate::ui_chat::test_speaker::know(world, 1);
        let mut pairs = crate::ui_chat::test_speaker::skill_fields(skill).to_vec();
        pairs.extend_from_slice(fields);
        let player = world
            .spawn((
                crate::net::Guid(READER),
                crate::net::SelfPlayer,
                crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs)),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(READER, player);
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    /// "hello" is understood at skill 150 and not at 0 (`hash % 300 < skill`, `0x49b79c`).
    const PAGE: &str = "hello <br> hello.";

    /// A GameObject's page text always goes through the rewrite in its language column
    /// (`0x4e37fc`, a GOOBER's `data[8]`): to the player's skill, keeping `<..>` spans and
    /// punctuation (`0x4e3856`, both flags set); language 0 shows it as it is.
    #[test]
    fn a_game_objects_page_is_rewritten_in_its_language() {
        const TABLET: u64 = 0xF110_0043_2400_0001;
        for (skill, language, want) in [(0, 1, "kazum <br> kazum."), (150, 1, PAGE), (0, 0, PAGE)] {
            let (mut app, _rx) = reading_app(skill, &[]);
            let mut data = [0i32; 24];
            data[7] = 358;
            data[8] = language;
            app.world_mut()
                .resource_mut::<GameObjectTemplates>()
                .insert(17188, 10, "The Lay".into(), &data);
            let tablet = app
                .world_mut()
                .spawn((
                    crate::net::Guid(TABLET),
                    crate::net::ObjectStore(crate::go_templates::test_typed_fields(17188, 10)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(TABLET, tablet);
            app.world_mut()
                .resource_mut::<PageTexts>()
                .insert(358, PAGE.into(), 0);
            crate::net::handlers::dispatch(
                app.world_mut(),
                vec![benilla_protocol::SessionEvent::GameObjectPageText { guid: TABLET }],
            );
            app.world_mut()
                .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
                .expect("the item-text lane fires");
            for _ in 0..4 {
                app.update();
            }
            assert_eq!(shown(&app), want, "skill {skill}, language {language}");
        }
    }

    /// A letter's single item text is not rewritten, whatever its template's language
    /// (`0x4e3666`-`0x4e3677` reach no `0x49b560`).
    #[test]
    fn a_letters_text_is_not_rewritten() {
        const LETTER: u64 = 0x4000_0000_0000_0009;
        let (mut app, _rx) = reading_app(0, &[]);
        let mut template = crate::items::test_template("Letter");
        template.language_id = 1;
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(5_000, Some(template));
        crate::items::test_spawn_item(
            app.world_mut(),
            LETTER,
            benilla_protocol::ObjectFields::from_pairs(&[(3, 5_000)]),
            false,
        );
        app.world_mut()
            .resource_mut::<MailOpen>()
            .bodies
            .insert(77, PAGE.into());
        app.world_mut()
            .resource_mut::<ItemTextOpen>()
            .read_letter(LETTER, 77);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(shown(&app), PAGE);
    }

    /// A bag book's page is rewritten in its template's language (`+0x1a0`, `0x4e37e9`) to the
    /// player's skill, unless a read marked the item (`0x4e3762`: the OK reply, so it shows as
    /// it is) or the show is a reason-1 reply's, which reads skill 0 (`0x4e37b3`); a letter's
    /// single text is never rewritten (`0x4e3666`).
    #[test]
    fn a_bag_books_page_is_rewritten_unless_read_and_untranslated_on_reason_1() {
        use benilla_protocol::SessionEvent as E;
        const BOOK: u64 = 0x4000_0000_0000_0021;
        const ME: u64 = READER;
        const PACK_SLOT_1: u16 = 532;
        const SCRAMBLED: &str = "kazum <br> kazum.";
        for (skill, reply, want) in [
            (0, "ok", PAGE),
            (150, "ok", PAGE),
            (0, "reason 0", SCRAMBLED),
            (150, "reason 0", PAGE),
            // Skill 150 passes "hello" whole: only the untranslated show scrambles it.
            (150, "reason 1", SCRAMBLED),
        ] {
            let (mut app, _rx) = reading_app(
                skill,
                &[
                    (PACK_SLOT_1 + 6, BOOK as u32),
                    (PACK_SLOT_1 + 7, (BOOK >> 32) as u32),
                ],
            );
            let world = app.world_mut();
            let mut template = crate::items::test_template("Gnarled Book");
            template.page_text = 358;
            template.language_id = 1;
            world
                .resource_mut::<Items>()
                .insert_template(5_000, Some(template));
            world
                .resource_mut::<PageTexts>()
                .insert(358, PAGE.into(), 0);
            crate::items::test_spawn_item(
                world,
                BOOK,
                benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x3),
                    (3, 5_000),
                    (8, ME as u32),
                    (9, (ME >> 32) as u32),
                ]),
                false,
            );
            app.world_mut()
                .resource_mut::<ItemTextOpen>()
                .read_pages(BOOK);
            for _ in 0..4 {
                app.update();
            }
            let reply_events = match reply {
                "ok" => vec![E::ReadItemOk { guid: BOOK }],
                other => vec![E::ReadItemFailed {
                    guid: BOOK,
                    reason: other.strip_prefix("reason ").unwrap().parse().unwrap(),
                    milliseconds: 2_500,
                }],
            };
            crate::net::handlers::dispatch(app.world_mut(), reply_events);
            app.world_mut()
                .run_system_cached_with(fire_item_text, crate::lanes::Until::all())
                .expect("the item-text lane fires");
            for _ in 0..4 {
                app.update();
            }
            assert_eq!(shown(&app), want, "skill {skill}, {reply}");
        }
    }
}

#[cfg(test)]
mod quest_material_tests {
    use super::*;

    fn go_guid(entry: u64) -> u64 {
        (u64::from(benilla_protocol::guid::HIGH_GAMEOBJECT) << 48) | (entry << 24) | 1
    }

    /// A GameObject's material is its template data by type; a creature's is Parchment.
    #[test]
    fn a_quest_sourced_from_an_object_paints_the_objects_material() {
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        // Each source object is streamed; its descriptor, not its guid, names the template.
        for (entry, type_id) in [(100, 2), (102, 10), (777, 3)] {
            objs.spawn_object(
                go_guid(entry),
                crate::go_templates::test_typed_fields(entry as u32, type_id),
            );
        }
        let objects = objs.get();
        let mut gos = GameObjectTemplates::default();
        let catalog = PageMaterials(benilla_formats::PageTextMaterialCatalog::from_rows(&[
            (1, "Parchment"),
            (2, "Stone"),
            (3, "Marble"),
        ]));
        let mut data = [0i32; 24];
        data[2] = 2;
        data[9] = 3;
        gos.insert(100, 2, "Wanted Poster".into(), &data);
        gos.insert(102, 10, "Chest".into(), &data);
        assert_eq!(
            object_material(go_guid(100), &objects, &items, &gos, Some(&catalog)).as_deref(),
            Some("Stone")
        );
        assert_eq!(
            object_material(go_guid(102), &objects, &items, &gos, Some(&catalog)).as_deref(),
            Some("Marble"),
            "a chest reads data[9]"
        );
        // A creature quest-giver: neither template, nothing asked.
        let creature = (u64::from(benilla_protocol::guid::HIGH_UNIT) << 48) | (6 << 24) | 1;
        assert_eq!(
            object_material(creature, &objects, &items, &gos, Some(&catalog)),
            None
        );
        // An uncached object: nothing yet, and nothing asked: the material getters only read
        // (`0x50229a`, `0x4e3a5a`).
        assert_eq!(
            object_material(go_guid(777), &objects, &items, &gos, Some(&catalog)),
            None
        );
    }
}
