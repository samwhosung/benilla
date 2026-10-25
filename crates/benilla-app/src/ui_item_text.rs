//! The item-text reader (`ItemTextFrame.xml`) a bag letter, a book or a readable world object
//! opens in. Every route ends at `0x4e32e0(guid, flag)`, which looks the guid up as any object and
//! asks it for its text (`vtbl+0x74`), a mail-made letter's `ITEM_FIELD_ITEM_TEXT_ID` or else a
//! page chain from an item template's `PageText` or a GameObject's pageID column (TEXT `data[0]`,
//! GOOBER `data[7]`). A bag item or a TEXT object opens locally, with no packet: vmangos has no use
//! arm for a type-9 object and refuses `CMSG_READ_ITEM` without a template `PageText`, which the
//! Plain Letter lacks (`ItemHandler.cpp:421`). A GOOBER's opens on `SMSG_GAMEOBJECT_PAGETEXT`
//! (`0x5f88c0`), the server's answer to its use.
//!
//! The events follow `ItemTextFrame.lua:10-96`; `ITEM_TEXT_TRANSLATION`, the foreign-language
//! bar, never fires, as neither source carries the language column.
//!
//! The reader is a window on the interaction latch. Its cell is written at the read
//! (`0x4e3364`), but it registers only once the text resolves (`0x4e349a`), right before
//! `ITEM_TEXT_BEGIN`, so the window it opens over closes first; its release (`0x4e3323`,
//! `0x4e3357`) re-enters the core while latched, firing `ITEM_TEXT_CLOSED` twice.
//!
//! A packet's read, `SMSG_GAMEOBJECT_PAGETEXT`'s, runs on the item-text lane ([`ItemTextLane`]),
//! so its latch and its `ITEM_TEXT_BEGIN` fire at the packet's place among the other lanes.

use bevy::prelude::*;

use benilla_ui::script::{ItemTextState, UiScript};

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::query_cache::{Ask, QueryCache};
use crate::ui_mail::MailOpen;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

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

impl crate::query_cache::AskOnce for PageTexts {
    fn clear_pending(&mut self) {
        self.pages.clear_pending();
    }
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
    /// A mail-made letter's `ITEM_FIELD_ITEM_TEXT_ID`: one body and a creator line.
    Letter { text_id: u32 },
    /// A page chain. `visited` runs from the first page to the one shown, as pages link only
    /// forwards (the reference's array at `0xbc3fd0`); it is empty until the object's template
    /// answers, since the reference asks for the page id at paint time (`0x5f59d0`).
    Pages { visited: Vec<u32> },
}

pub(crate) struct ReadSession {
    /// An item's or a GameObject's guid; reading the same object again closes the reader.
    pub(crate) object_guid: u64,
    pub(crate) source: ReadSource,
    /// The open's tail ran (`0x4e349a`): the latch holds this reader. The cell alone cannot say
    /// so, as another window may hold the latch on the same guid (a trade partner's).
    registered: bool,
    /// Keyed on the VM; the leave-world sweep closes the reader, so no later VM inherits one.
    told: crate::ui_script::VmMemo<ItemTextTold>,
    /// The open has looked the object's record up: the open (`0x4e32e0`) runs once, and its
    /// callback re-enters it when the record lands.
    opened: bool,
    /// The text record the open asked for and waits on (`0x4e3465`, `0x4e347c`), whose answer's
    /// callback `0x4e3280` opens again (`0x4e32a5`), on the item-text lane at the answer's place.
    awaiting: Option<TextRecord>,
}

/// The record a reader's text comes from: a page of the page cache (`0xc0e174`), or a letter's
/// body of the item-text cache (`0xc0e0c0`), which mail shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextRecord {
    Page(u32),
    Body(u32),
}

#[derive(Default)]
struct ItemTextTold {
    /// `ITEM_TEXT_BEGIN` fired: once per open, before the text lands.
    begun: bool,
    ready: bool,
    /// The feed has run the open's or the page turn's lookups, which pass a callback (the page
    /// or the text at `0x4e3465`, `0x4e347c`, `0x4e363c`; the creator at `0x4e39b9`); the frames
    /// after it until `ITEM_TEXT_READY` read.
    asked: bool,
}

#[derive(Resource, Default)]
pub(crate) struct ItemTextOpen {
    pub(crate) pending: Option<ReadSession>,
    /// Reads a click or a bag use asked for, in order; [`apply_reads`] runs each as `0x4e32e0`
    /// with flag 0, with the world in hand for the latch.
    reads: Vec<(u64, ReadSource)>,
    /// While the item-text lane fires a read, the records whose answers its later packets carry:
    /// in the cache already, but not yet when the read's packet ran, so the begin waits on them.
    landing: Vec<TextRecord>,
}

impl ItemTextOpen {
    /// Read a bag letter; reading the open one again closes it. `true` when it is the open one.
    pub(crate) fn read_letter(&mut self, item_guid: u64, text_id: u32) -> bool {
        self.read(item_guid, ReadSource::Letter { text_id })
    }

    /// Read a page chain; the begin asks the object's template for the first page. `true` when
    /// it is the open one, which the read closes.
    pub(crate) fn read_pages(&mut self, object_guid: u64) -> bool {
        self.read(
            object_guid,
            ReadSource::Pages {
                visited: Vec::new(),
            },
        )
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
        self.pending = Some(ReadSession {
            object_guid,
            source,
            registered: false,
            told: Default::default(),
            opened: false,
            awaiting: None,
        });
    }

    /// A bag letter's session written directly, for a test that seats it.
    #[cfg(test)]
    pub(crate) fn open_letter(&mut self, item_guid: u64, text_id: u32) {
        self.open(item_guid, ReadSource::Letter { text_id });
    }

    /// The open page chain shown at `page_id`, its first page, for a test that turns it.
    #[cfg(test)]
    pub(crate) fn show_page_for_test(&mut self, page_id: u32) {
        if let Some(sess) = self.pending.as_mut() {
            sess.source = ReadSource::Pages {
                visited: vec![page_id],
            };
        }
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
/// (`0x4e3357`), the cell cleared (`0x4e3364`) and `ITEM_TEXT_CLOSED` (`0x4e33a2`). While the
/// reader holds the latch, the release's `0x493310` runs this once more inside it (`0x49338e`).
pub(crate) fn close_core(world: &mut World) {
    let Some(object) = world.resource::<ItemTextOpen>().object() else {
        return;
    };
    crate::ui_session::release(world, object);
    world.resource_mut::<ItemTextOpen>().pending = None;
    crate::ui_session::note_closed(world, NpcWindow::ItemText);
    crate::ui_session::fire(world, |script| {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
    });
}

/// One step of the item-text lane.
#[derive(Clone, Copy, Debug)]
enum TextStep {
    /// `SMSG_GAMEOBJECT_PAGETEXT`'s read of a GameObject (`0x5f8915` → `0x4e32e0`).
    Read(u64),
    /// A text record's answer (`SMSG_PAGE_TEXT_QUERY_RESPONSE`, `SMSG_ITEM_TEXT_QUERY_RESPONSE`),
    /// whose callback `0x4e3280` opens the reader again when it waits on that record.
    Landed(TextRecord),
}

/// The item-text lane: each step stamped at its packet ([`crate::lanes::stamp`]).
#[derive(Resource, Default)]
pub(crate) struct ItemTextLane(std::collections::VecDeque<(crate::lanes::Stamp, TextStep)>);

impl ItemTextLane {
    /// Queue `step` behind everything stamped before it.
    fn push(&mut self, step: TextStep) {
        let stamp = crate::lanes::stamp();
        let at = self.0.partition_point(|(s, _)| *s < stamp);
        self.0.insert(at, (stamp, step));
    }
}

/// The session's end and the leave-world sweep drop what the lane holds: no later world reads
/// those objects, and a worldport's sweep closes the reader ahead of the lanes' drain.
fn drop_item_text_lane(world: &mut World) {
    if let Some(mut lane) = world.get_resource_mut::<ItemTextLane>() {
        lane.0.clear();
    }
}

/// The leave-world sweep's reader leg (`0x490abc` → `0x4e3240` → `0x4e32e0(0, 0)`): the lane's
/// reads dropped, then the core.
fn sweep_item_text(world: &mut World) {
    drop_item_text_lane(world);
    close_core(world);
}

/// What the item-text lane's firing touches beside the call host and its own lane, for the guard
/// on the lanes' frame-final reads: the begin's access as Bevy measures it, and the latch and its
/// windows, which the open's release and the begin's register read.
fn item_text_reads(world: &mut World) -> bevy::ecs::query::FilteredAccessSet {
    let mut access = crate::lanes::system_access(world, begin_item_text);
    for t in crate::ui_session::latch_reads(world) {
        if let Some(id) = world.components().get_resource_id(t) {
            access.add_unfiltered_resource_read(id);
        }
    }
    access
}

/// The item-text lane's head.
fn peek_item_text(world: &World) -> Option<crate::lanes::Stamp> {
    world
        .get_resource::<ItemTextLane>()?
        .0
        .front()
        .map(|(s, _)| *s)
}

/// Each step `until` admits, run as the firing of its stamp. A read is `0x4e32e0(guid, 0)`
/// ([`read_core`]), then its begin ([`begin_item_text`]), which waits on a record whose answer is
/// still on the lane; a record's answer that the open waits on is the callback's open again
/// (`0x4e3280` → `0x4e32e0(guid, 1)`), the begin alone. The begin's latch closes the window it
/// opens over and its `ITEM_TEXT_BEGIN` fires inside it; a template still in flight leaves the
/// begin to the feed, as the template's callback re-enters the open. The lane stops after a step
/// whose firing stamped anything, so what it pushed fires next.
fn fire_item_text(In(until): In<crate::lanes::Until>, world: &mut World) {
    loop {
        let Some((at, step)) = world
            .resource::<ItemTextLane>()
            .0
            .front()
            .copied()
            .filter(|(at, _)| until.admits(*at))
        else {
            return;
        };
        world.resource_mut::<ItemTextLane>().0.pop_front();
        let ((), moved) = crate::lanes::firing(at, || {
            match step {
                TextStep::Read(guid) => {
                    read_core(
                        world,
                        guid,
                        ReadSource::Pages {
                            visited: Vec::new(),
                        },
                    );
                    let landing = world
                        .resource::<ItemTextLane>()
                        .0
                        .iter()
                        .filter_map(|(_, s)| match s {
                            TextStep::Landed(record) => Some(*record),
                            TextStep::Read(_) => None,
                        })
                        .collect();
                    world.resource_mut::<ItemTextOpen>().landing = landing;
                }
                TextStep::Landed(record) => {
                    let mut open = world.resource_mut::<ItemTextOpen>();
                    match open.pending.as_mut() {
                        Some(sess) if sess.awaiting == Some(record) => sess.awaiting = None,
                        _ => return,
                    }
                }
            }
            if let Err(e) = world.run_system_cached(begin_item_text) {
                debug!("item text: the begin did not run: {e}");
            }
            world.resource_mut::<ItemTextOpen>().landing.clear();
        });
        if moved {
            return;
        }
    }
}

/// The lane's reads while no in-game interface is up, which no drain fires, handed to the frame's
/// reads unfired: nobody hears the begin, and the reader still takes the object.
fn spill_item_text_lane(mut lane: ResMut<ItemTextLane>, mut open: ResMut<ItemTextOpen>) {
    for (_, step) in lane.0.drain(..) {
        match step {
            TextStep::Read(guid) => {
                open.read_pages(guid);
            }
            // The feed's begin finds the record cached.
            TextStep::Landed(record) => {
                if let Some(sess) = open.pending.as_mut().filter(|s| s.awaiting == Some(record)) {
                    sess.awaiting = None;
                }
            }
        }
    }
}

/// The queued reads, each `0x4e32e0(guid, 0)`, run with the world in hand ([`read_core`]).
fn apply_reads(mut open: ResMut<ItemTextOpen>, mut commands: Commands) {
    for (guid, source) in std::mem::take(&mut open.reads) {
        commands.queue(move |world: &mut World| read_core(world, guid, source));
    }
}

/// One read, `0x4e32e0(guid, 0)`. The open object again (`0x4e330c`) releases the latch on the
/// cell (`0x4e3323`), which closes the reader if it holds it, then fires `ITEM_TEXT_CLOSED`
/// (`0x4e332d`); one never begun keeps its cell. Another object releases the latch on the old cell
/// (`0x4e3357`), closing a begun reader through `0x493310` once, then takes the cell (`0x4e3364`)
/// and starts over.
fn read_core(world: &mut World, guid: u64, source: ReadSource) {
    let cell = world.resource::<ItemTextOpen>().object();
    if cell == Some(guid) {
        crate::ui_session::release(world, guid);
        let closed = world.resource::<ItemTextOpen>().object().is_none();
        crate::ui_session::fire(world, |script| {
            if closed {
                script.set_item_text(None);
            }
            script.fire_event("ITEM_TEXT_CLOSED", vec![]);
        });
        return;
    }
    if let Some(old) = cell {
        crate::ui_session::release(world, old);
    }
    world.resource_mut::<ItemTextOpen>().open(guid, source);
}

/// The open's tail once the object's text resolves (`0x4e33b0`-`0x4e349f`): the first page's
/// record, or a letter's body, looked up with the callback `0x4e3280` (`0x4e3465`, `0x4e347c`),
/// and only with it at hand (`0x4e3489`, `0x4e348b`) the latch, whose holder closes first
/// (`0x4e349a`), then `ITEM_TEXT_BEGIN` (`0x4e34a4`) with the title and the material, as the stock
/// handler takes the text colour from the material (`ItemTextFrame.lua:18-23`). A record not at
/// hand is waited on: its answer opens again on the item-text lane, and no frame begins it. A page
/// chain with no page bails before both (`0x4e341d`) and is dropped.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn begin_item_text(
    mut open: ResMut<ItemTextOpen>,
    objects: Objects,
    items: Res<Items>,
    go_templates: Res<GameObjectTemplates>,
    materials: Option<Res<PageMaterials>>,
    pages: Res<PageTexts>,
    mail: Res<MailOpen>,
    net: Res<NetCommands>,
    mut commands: Commands,
) {
    let landing = open.landing.clone();
    let Some(sess) = open.pending.as_mut() else {
        return;
    };
    if sess.registered || sess.awaiting.is_some() {
        return;
    }
    let object = sess.object_guid;
    let is_pages = matches!(sess.source, ReadSource::Pages { .. });
    let ask = Ask::at(!std::mem::replace(&mut sess.opened, true), &net);
    let Some(readable) = readable(
        object,
        &objects,
        &items,
        &go_templates,
        materials.as_deref(),
        ask,
    ) else {
        return; // template in flight; the reference re-enters the open when it lands
    };
    if is_pages && readable.page_head == 0 {
        debug!("item text: {object:#x} has no page to read — closing");
        open.pending = None;
        return;
    }
    let record = match sess.source {
        ReadSource::Pages { .. } => TextRecord::Page(readable.page_head),
        ReadSource::Letter { text_id } => TextRecord::Body(text_id),
    };
    let at_hand = !landing.contains(&record)
        && match record {
            TextRecord::Page(page_id) => pages.page(page_id, object, ask).is_some(),
            TextRecord::Body(text_id) => mail
                .bodies
                .read(text_id, ask, |commands| {
                    let _ = commands.0.send(ClientCommand::ItemTextQuery {
                        text_id,
                        mail_id: 0,
                    });
                })
                .is_some(),
        };
    if !at_hand {
        sess.awaiting = Some(record);
        return;
    }
    let begun = ItemTextState {
        item: readable.title,
        creator: None,
        text: String::new(),
        page: 1,
        has_next: false,
        material: readable.material,
    };
    commands.queue(move |world: &mut World| begin_core(world, object, begun));
}

/// [`begin_item_text`]'s world half: the latch, then `ITEM_TEXT_BEGIN`, told once per VM.
fn begin_core(world: &mut World, object: u64, begun: ItemTextState) {
    match world.resource_mut::<ItemTextOpen>().pending.as_mut() {
        Some(sess) if sess.object_guid == object && !sess.registered => sess.registered = true,
        _ => return,
    }
    crate::ui_session::register(world, object);
    crate::ui_session::fire(world, |script| {
        script.set_item_text(Some(begun));
        script.fire_event("ITEM_TEXT_BEGIN", vec![]);
    });
    world.resource_scope(|world, mut open: Mut<ItemTextOpen>| {
        if let (Some(sess), Some(script)) = (
            open.pending.as_mut(),
            world.get_non_send_resource::<UiScript>(),
        ) {
            sess.told.get(script).begun = true;
        }
    });
}

/// A read of `object_guid`'s pages run to its begin, as a click and the next feed's reads and
/// begin would, for a test.
#[cfg(test)]
pub(crate) fn read_now(world: &mut World, object_guid: u64) {
    use bevy::ecs::system::RunSystemOnce;
    world.resource_mut::<ItemTextOpen>().read_pages(object_guid);
    world.run_system_once(apply_reads).expect("reads");
    world.flush();
    world.run_system_once(begin_item_text).expect("begin");
    world.flush();
}

/// The page-text reply's handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::PageTexts;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text)
            .net_handler(
                SessionEventKind::GameObjectPageText,
                on_gameobject_page_text,
            )
            .net_handler(SessionEventKind::MailItemText, on_item_text);
    }

    /// `SMSG_ITEM_TEXT_QUERY_RESPONSE`: a letter's body landing, whose callback `0x4e3280` opens
    /// the reader again when it waits on it; the mail handler caches the body.
    fn on_item_text(In(ev): In<SessionEvent>, mut lane: ResMut<super::ItemTextLane>) {
        if let SessionEvent::MailItemText { text_id, .. } = ev {
            lane.push(super::TextStep::Landed(super::TextRecord::Body(text_id)));
        }
    }

    /// `SMSG_GAMEOBJECT_PAGETEXT` (`0x5f88c0`): the guid looked up as a GameObject (mask `0x20`,
    /// `0x5f88f9`), then the reader's open on it with flag 0 (`0x5f8915` → `0x4e32e0`), the same
    /// read a TEXT object's right-click makes, on the item-text lane at the packet's place;
    /// nothing for an object not streamed.
    fn on_gameobject_page_text(
        In(ev): In<SessionEvent>,
        objects: crate::net::Objects,
        mut lane: ResMut<super::ItemTextLane>,
    ) {
        let SessionEvent::GameObjectPageText { guid } = ev else {
            return;
        };
        let is_gameobject = objects.object(guid).is_some_and(|f| {
            f.object_type() == Some(benilla_protocol::messages::ObjectType::GameObject)
        });
        if is_gameobject {
            lane.push(super::TextStep::Read(guid));
        }
    }

    /// One page per packet; the reader repaints off the cache on the next feed, and a reader that
    /// waits on the page opens again at its place (`0x4e3280`).
    fn on_page_text(
        In(ev): In<SessionEvent>,
        mut pages: ResMut<PageTexts>,
        mut lane: ResMut<super::ItemTextLane>,
    ) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = ev
        {
            pages.insert(page_id, text, next_page_id);
            lane.push(super::TextStep::Landed(super::TextRecord::Page(page_id)));
        }
    }
}

pub(crate) struct UiItemTextPlugin;

impl Plugin for UiItemTextPlugin {
    fn build(&self, app: &mut App) {
        use crate::lanes::LaneApp;
        net::register(app);
        crate::query_cache::register::<PageTexts>(app);
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
                // The reads and the begin ahead of the feed, so a read that resolves paints this
                // frame.
                (
                    spill_item_text_lane.run_if(not(crate::ui_script::ingame_ui_up)),
                    apply_reads,
                    begin_item_text,
                    feed_item_text,
                )
                    .chain()
                    .in_set(UiFeed),
            );
    }
}

/// What the read object answers: its name for the title (`ItemTextGetItem` `0x4e38f0`), its
/// material (`ItemTextGetMaterial` `0x4e39f0`: an item's `PageMaterial`, a GameObject's
/// pageMaterial column) and its first page.
struct Readable {
    title: String,
    /// The `PageTextMaterial.dbc` basename; `None` is the Lua's Parchment default.
    material: Option<String>,
    /// The first `PageText` id; 0 for a letter or a template with no page.
    page_head: u32,
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
    if benilla_protocol::guid::is_gameobject(guid) {
        let fields = objects.object(guid)?;
        return name(go_templates.of(fields)?.page_material(fields));
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, Ask::Read)?;
    name(t.page_material)
}

/// The object's [`Readable`], item or GameObject alike; `None` while its template is in flight.
/// An item's is read as `ask` says, the open reading its page id off the record (`0x4e33ec`); a
/// GameObject's is the one its create asked for.
fn readable(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
    ask: Ask<'_>,
) -> Option<Readable> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    if let Some((fields, go)) = objects
        .object(guid)
        .and_then(|f| Some((f, go_templates.of_guid(objects, guid)?)))
    {
        return Some(Readable {
            title: go.name.clone(),
            material: name(go.page_material(fields)),
            page_head: go.page_id(fields),
        });
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, ask)?;
    Some(Readable {
        title: t.name.clone(),
        material: name(t.page_material),
        page_head: t.page_text,
    })
}

/// Drive the begun session to `ITEM_TEXT_READY`, which shows the window, once the body and any
/// creator line are in.
fn feed_item_text(
    // The mail session's body cache is the call host's too ([`crate::ui_mail::MailVerbs`]).
    mut vm: crate::script_calls::VmWith<(ResMut<'static, ItemTextOpen>, Res<'static, MailOpen>)>,
    pages: Res<PageTexts>,
    objects: Objects,
    items: Res<Items>,
    names: Res<NameCache>,
    go_templates: Res<GameObjectTemplates>,
    materials: Option<Res<PageMaterials>>,
    commands: Res<NetCommands>,
    // The `$`-macro subject for the page body: the local player.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
    unit_names: crate::ui_unit::NameTables,
    filter: Option<Res<crate::text_filter::TextFilter>>,
    switches: Option<Res<crate::text_filter::TextFilterSwitches>>,
) {
    vm.feed(|script, (mut open, mail), fires| {
        let Some(script) = script else {
            return;
        };
        let Some(sess) = open.pending.as_mut() else {
            return;
        };
        // The begin ([`begin_item_text`]) fires first, latching the reader.
        if sess.told.get(script).ready || !sess.told.get(script).begun {
            return;
        }
        let ask = Ask::at(
            !std::mem::replace(&mut sess.told.get(script).asked, true),
            &commands,
        );
        let Some(readable) = readable(
            sess.object_guid,
            &objects,
            &items,
            &go_templates,
            materials.as_deref(),
            ask,
        ) else {
            return; // template in flight; the reference re-enters the open when it lands
        };

        let is_letter = matches!(sess.source, ReadSource::Letter { .. });
        let (creator, text, page, has_next) = match &mut sess.source {
            ReadSource::Letter { text_id } => {
                let creator_guid = objects
                    .object(sess.object_guid)
                    .and_then(|o| o.item_creator());
                let creator = match creator_guid {
                    None => None, // authorless
                    Some(guid) => match names.resolve_player(guid, ask) {
                        Some(name) => Some(name.to_string()),
                        None => return, // name query in flight
                    },
                };
                // The body, from the client's one item-text cache, which mail shares.
                let body = mail
                    .bodies
                    .read(*text_id, ask, |commands| {
                        let _ = commands.0.send(ClientCommand::ItemTextQuery {
                            text_id: *text_id,
                            mail_id: 0,
                        });
                    })
                    .cloned();
                let Some(text) = body else {
                    return; // body query in flight
                };
                (creator, text, 1, false)
            }
            ReadSource::Pages { visited } => {
                if visited.is_empty() {
                    visited.push(readable.page_head);
                }
                let page_id = *visited.last().expect("seeded just above");
                let Some(page) = pages.page(page_id, sess.object_guid, ask) else {
                    return; // page query in flight
                };
                // A book has no creator line.
                (
                    None,
                    page.text.clone(),
                    visited.len() as u32,
                    page.next != 0,
                )
            }
        };

        // Server-authored text runs the `$`-macro expander, as in the reference's
        // `ItemTextFrame.cpp`.
        let subject = crate::npc_text::player_identity(&self_q, &names, unit_names.get());
        let mut text = crate::npc_text::substitute(
            &text,
            &crate::npc_text::MacroContext {
                subject: subject.as_ref(),
                states: &states,
            },
        );
        // A single item text is masked after the expansion (`0x4e3672`, `dl = 0`); a page chain's
        // pages are not (`0x4e3744`-`0x4e3860` reach no masker).
        if let (true, Some(filter)) = (is_letter, filter.as_deref()) {
            filter.mask(switches.as_deref().is_some_and(|s| s.profanity), &mut text);
        }
        script.set_item_text(Some(ItemTextState {
            item: readable.title,
            creator,
            text,
            page,
            has_next,
            material: readable.material,
        }));
        fires.post("ITEM_TEXT_READY", vec![]);
        sess.told.get(script).ready = true;
    })
}

/// `PageTextMaterial.dbc`; absent without client data, leaving every reader on Parchment.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

/// The reader's verbs inside their calls ([`benilla_ui::script::ItemTextCall`]): the page turns
/// here, the close core in the host ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ItemTextSteps<'w> {
    /// Absent in a harness without the reader.
    open: Option<ResMut<'w, ItemTextOpen>>,
    pages: Option<Res<'w, PageTexts>>,
    commands: Res<'w, NetCommands>,
}

impl ItemTextSteps<'_> {
    /// `ItemTextPrevPage()` (`0x4e3870`) or `ItemTextNextPage()` (`0x4e38b0`) as `delta`: the page
    /// moved, then the show (`0x4e35b0`) looks its record up with a callback (`0x4e363c`), which
    /// asks for a page not cached. The repaint and `ITEM_TEXT_READY` follow on the feed, which
    /// does not fire BEGIN again.
    pub(crate) fn turn(&mut self, script: &mut UiScript, delta: i32) {
        let (Some(open), Some(pages)) = (self.open.as_deref_mut(), self.pages.as_deref()) else {
            return;
        };
        let Some(sess) = open.pending.as_mut() else {
            return;
        };
        let ReadSource::Pages { visited } = &mut sess.source else {
            return; // a letter has no pages; its buttons never show
        };
        if !turn_page(visited, delta, |id| pages.pages.get(id).map(|p| p.next)) {
            return;
        }
        let page_id = *visited.last().expect("a turn lands on a page");
        let _ = pages.page(page_id, sess.object_guid, Ask::Event(&self.commands));
        let told = sess.told.get(script);
        told.ready = false;
        told.asked = true;
    }

    /// The reader's cell `[0xbc3fa0]`.
    pub(crate) fn object(&self) -> Option<u64> {
        self.open.as_deref().and_then(ItemTextOpen::object)
    }

    /// The close core's cell clear (`0x4e3364`).
    pub(crate) fn clear(&mut self) {
        if let Some(open) = self.open.as_deref_mut() {
            open.pending = None;
        }
    }
}

/// Walk the trail one step: Next pushes the page's `nextPageId` from `next_of`, the cache, and
/// Prev pops (the reference's array at `0xbc3fd0`). A page not landed or a last page refuses;
/// returns whether the page changed.
fn turn_page(visited: &mut Vec<u32>, delta: i32, next_of: impl Fn(u32) -> Option<u32>) -> bool {
    let Some(&current) = visited.last() else {
        return false; // head not resolved yet
    };
    if delta > 0 {
        match next_of(current) {
            Some(next) if next != 0 => visited.push(next),
            _ => return false,
        }
    } else if visited.len() > 1 {
        visited.pop();
    } else {
        return false; // already on page 1
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The open looks the record up with a callback (`0x4e32e0`), and so does `ItemTextGetCreator`
    /// (`0x4e39b9`) and the show (`0x4e363c`); the frames that wait read. A refused creator is
    /// asked by no frame, and again by the next read of the letter.
    #[test]
    fn a_refused_letter_creator_is_asked_at_the_next_read_never_by_a_frame() {
        // `OBJECT_FIELD_ENTRY` and `ITEM_FIELD_CREATOR`'s two words.
        const FIELD_OBJECT_ENTRY: u16 = 3;
        const FIELD_ITEM_CREATOR: u16 = 10;
        const LETTER: u64 = 0x4000_0000_0000_0009;
        const AUTHOR: u64 = 0x42;
        let (tx, rx) = crossbeam_channel::unbounded();
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
            .add_systems(
                Update,
                (apply_reads, begin_item_text, feed_item_text).chain(),
            );
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(5_000, Some(crate::items::test_template("Letter")));
        crate::items::test_spawn_item(
            app.world_mut(),
            LETTER,
            benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_OBJECT_ENTRY, 5_000),
                (FIELD_ITEM_CREATOR, AUTHOR as u32),
                (FIELD_ITEM_CREATOR + 1, 0),
            ]),
            false,
        );
        let names_asked = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            crate::query_cache::queries(rx)
                .iter()
                .filter(|c| matches!(c, ClientCommand::NameQuery { guid: AUTHOR }))
                .count()
        };
        app.world_mut()
            .resource_mut::<MailOpen>()
            .bodies
            .insert(77, "Dear you.".into());
        app.world_mut()
            .resource_mut::<ItemTextOpen>()
            .read_letter(LETTER, 77);
        app.update();
        app.update();
        assert_eq!(names_asked(&rx), 1, "the read asks the creator");
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(AUTHOR, String::new(), None);
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(names_asked(&rx), 0, "no waiting frame asks the refusal");
        // A close, then a new read of the letter.
        close_core(app.world_mut());
        assert!(app.world().resource::<ItemTextOpen>().pending.is_none());
        app.world_mut()
            .resource_mut::<ItemTextOpen>()
            .read_letter(LETTER, 77);
        app.update();
        app.update();
        assert_eq!(names_asked(&rx), 1, "the next read asks it, once");
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
                .add_systems(
                    Update,
                    (apply_reads, begin_item_text, feed_item_text).chain(),
                );
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
            let text: String = app
                .world()
                .non_send_resource::<UiScript>()
                .eval("return ItemTextGetText()")
                .expect("runs");
            assert_eq!(text, want, "profanityFilter {profanity}");
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
            .add_systems(
                Update,
                (apply_reads, begin_item_text, feed_item_text).chain(),
            );
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
        let text: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return ItemTextGetText()")
            .expect("runs");
        assert_eq!(text, "Oh darn it.");
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
            .add_systems(
                Update,
                (apply_reads, begin_item_text, feed_item_text).chain(),
            );
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

    #[test]
    fn page_turns_walk_the_chain_both_ways() {
        let chain = |id: u32| match id {
            100 => Some(101),
            101 => Some(102),
            102 => Some(0), // last page
            _ => None,
        };
        let mut visited = vec![100];
        assert!(!turn_page(&mut visited, -1, chain), "page 1 has no Prev");
        assert_eq!(visited, [100]);

        assert!(turn_page(&mut visited, 1, chain));
        assert!(turn_page(&mut visited, 1, chain));
        assert_eq!(visited, [100, 101, 102]);
        assert!(
            !turn_page(&mut visited, 1, chain),
            "the last page has no Next"
        );

        assert!(turn_page(&mut visited, -1, chain));
        assert_eq!(visited, [100, 101], "Prev pops the forward-only trail");
    }

    #[test]
    fn a_page_still_in_flight_refuses_the_turn() {
        let mut visited = vec![7];
        assert!(!turn_page(&mut visited, 1, |_| None));
        assert_eq!(visited, [7]);
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
        assert!(matches!(&sess.source, ReadSource::Pages { visited } if visited.is_empty()));
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
