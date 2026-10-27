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
//! shows untranslated with `ITEM_TEXT_TRANSLATION` (reason 1) or closes (any other).
//!
//! The events follow `ItemTextFrame.lua:10-96`.
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
    /// A bag book's show waits on the server's reply to its read (`0x4e3586`), or on a read the
    /// open could not send.
    awaiting_reply: bool,
    /// The reply showed the page again (`0x4e35b0`): the next feed repaints and signals
    /// `ITEM_TEXT_READY`.
    reshow: bool,
    /// The translation time of a reply of reason 1, in seconds (`0x5ec6ac`), signalled after the
    /// show.
    translation: Option<f64>,
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
            awaiting_reply: false,
            reshow: false,
            translation: None,
        });
    }

    /// `0x4e35b0` from a read reply: the page shows again when the guid is the open one, with the
    /// translation time of a reply that carries one.
    fn show(&mut self, guid: u64, translation: Option<f64>) {
        if let Some(sess) = self.pending.as_mut().filter(|s| s.object_guid == guid) {
            sess.awaiting_reply = false;
            sess.reshow = true;
            sess.translation = translation;
        }
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
    /// A read's reply (`SMSG_READ_ITEM_OK`, `SMSG_READ_ITEM_FAILED`), which `0x5ec600` applies.
    Reply(ReadReply),
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
                            TextStep::Read(_) | TextStep::Reply(_) => None,
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
                TextStep::Reply(reply) => {
                    apply_reply(world, reply);
                    return;
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
            // No reader is open with no interface up to hear it.
            TextStep::Reply(_) => {}
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
    if let Err(e) = world.run_system_cached_with(request_read, object) {
        debug!("item text: the read request did not run: {e}");
    }
}

/// `0x5ec600` on a read's reply, at its packet's place on the item-text lane. OK finds the item
/// (mask 2, `0x5ec635`), marks it read (`0x5d9a80`, `ITEM_FIELD_FLAGS` bit 0x2) and shows its page
/// translated (`0x4e35b0`); a missing item does neither. FAILED reads a reason: 0 shows, 1 shows
/// untranslated and signals `ITEM_TEXT_TRANSLATION` with the time in seconds, whatever item it
/// names, any other signals
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
            world.resource_mut::<ItemTextOpen>().show(guid, None);
        }
        ReadReply::Failed {
            guid,
            reason,
            milliseconds,
        } => match reason {
            0 => world.resource_mut::<ItemTextOpen>().show(guid, None),
            1 => {
                // The signal follows the show but is not gated by it (`0x5ec6a1`-`0x5ec6c5`): an
                // open item's comes after its `ITEM_TEXT_READY`, any other's at once.
                let seconds = f64::from(milliseconds) * 0.001;
                if world.resource::<ItemTextOpen>().object() == Some(guid) {
                    world
                        .resource_mut::<ItemTextOpen>()
                        .show(guid, Some(seconds));
                } else {
                    crate::ui_session::fire(world, |script| {
                        script.fire_event(
                            "ITEM_TEXT_TRANSLATION",
                            vec![benilla_ui::script::ScriptValue::Number(seconds)],
                        );
                    });
                }
            }
            _ if world.resource::<ItemTextOpen>().object() == Some(guid) => {
                crate::ui_session::fire(world, |script| {
                    script.fire_event("ITEM_TEXT_CLOSED", vec![]);
                });
            }
            _ => {}
        },
    }
}

/// The open's last step for an item (`0x4e34a9`-`0x4e3586`), after `ITEM_TEXT_BEGIN`: one with a
/// page chain and no item-text id (`+0x9c`) whose `ITEM_FIELD_FLAGS` lack bit 0x2, which a read
/// reply sets (`0x5d9a80`), sends `CMSG_READ_ITEM` from its place in its container (`0x5ec580`)
/// and shows on the reply; with no active player, no container or no place in it the open ends
/// there, and nothing shows. Any other object goes straight to the show (`0x4e359b`).
fn request_read(
    In(object): In<u64>,
    mut open: ResMut<ItemTextOpen>,
    objects: Objects,
    net: Res<NetCommands>,
) {
    let Some(sess) = open.pending.as_mut().filter(|s| s.object_guid == object) else {
        return;
    };
    let Some(item) = objects.object(object).filter(|f| f.is_item()) else {
        return;
    };
    let reads_by_server = matches!(sess.source, ReadSource::Pages { .. })
        && item.item_text_id().unwrap_or(0) == 0
        && item.item_flags().unwrap_or(0) & ITEM_FLAG_READ == 0;
    if !reads_by_server {
        return;
    }
    sess.awaiting_reply = true;
    match read_position(object, &objects) {
        Some((bag, slot)) => {
            let _ = net.0.send(ClientCommand::ReadItem { bag, slot });
        }
        None => debug!("item text: {object:#x} has no place in a container to read from"),
    }
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
            .net_handler(SessionEventKind::MailItemText, on_item_text)
            .net_handler(SessionEventKind::ReadItemOk, on_read_item)
            .net_handler(SessionEventKind::ReadItemFailed, on_read_item);
    }

    /// `SMSG_READ_ITEM_OK` and `SMSG_READ_ITEM_FAILED` (`0x5e7d90` → `0x5ec600`), applied at their
    /// place on the item-text lane ([`super::apply_reply`]).
    fn on_read_item(In(ev): In<SessionEvent>, mut lane: ResMut<super::ItemTextLane>) {
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
        lane.push(super::TextStep::Reply(reply));
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
            language: Some(go.language(fields)),
        });
    }
    let fields = objects.object(guid)?;
    let t = items.template(fields.object_entry()?, guid, ask)?;
    Some(Readable {
        title: t.name.clone(),
        material: name(t.page_material),
        page_head: t.page_text,
        language: (fields.item_flags().unwrap_or(0) & ITEM_FLAG_READ == 0).then_some(t.language_id),
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
    // The character's skill per language and the word pools, absent in a harness without chat.
    languages: Option<Res<crate::ui_chat::ChatLanguages>>,
) {
    vm.feed(|script, (mut open, mail), fires| {
        let Some(script) = script else {
            return;
        };
        let Some(sess) = open.pending.as_mut() else {
            return;
        };
        // The begin ([`begin_item_text`]) fires first, latching the reader.
        if std::mem::take(&mut sess.reshow) {
            sess.told.get(script).ready = false;
        }
        if sess.told.get(script).ready || !sess.told.get(script).begun || sess.awaiting_reply {
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
        // A page is rewritten in its object's language to the player's skill (`0x4e3856`,
        // `0x49b560`), or to skill 0 when the show is a reason-1 read reply's (`0x4e37b3`); a
        // single item text is not (`0x4e3666`-`0x4e3677`).
        if let (false, Some(language), Some(languages)) =
            (is_letter, readable.language, languages.as_deref())
        {
            text = languages.garble_item_text(language, sess.translation.is_none(), &text);
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
        if let Some(seconds) = sess.translation.take() {
            fires.post(
                "ITEM_TEXT_TRANSLATION",
                vec![benilla_ui::script::ScriptValue::Number(seconds)],
            );
        }
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
                .add_systems(
                    Update,
                    (apply_reads, begin_item_text, feed_item_text).chain(),
                );
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

    /// The word pool of language 1 (the tests' own): every five-letter word becomes `kazum`.
    fn orcish() -> benilla_formats::LanguageWords {
        benilla_formats::LanguageWords::from_rows([(1, "kazum".to_string())])
    }

    /// The reader's resources over a character who knows language 1 to `skill`.
    fn reading_app(skill: u32) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
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
            .insert_resource(crate::ui_chat::ChatLanguages::for_test(
                orcish(),
                &[(1, skill)],
            ))
            .add_systems(
                Update,
                (apply_reads, begin_item_text, feed_item_text).chain(),
            );
        net::register(&mut app);
        app.insert_non_send_resource(UiScript::new().unwrap());
        (app, rx)
    }

    fn page_text(app: &App) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval("return ItemTextGetText()")
            .expect("runs")
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
            let (mut app, _rx) = reading_app(skill);
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
            assert_eq!(page_text(&app), want, "skill {skill}, language {language}");
        }
    }

    /// A letter's single item text is not rewritten, whatever its template's language
    /// (`0x4e3666`-`0x4e3677` reach no `0x49b560`).
    #[test]
    fn a_letters_text_is_not_rewritten() {
        const LETTER: u64 = 0x4000_0000_0000_0009;
        let (mut app, _rx) = reading_app(0);
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
        assert_eq!(page_text(&app), PAGE);
    }

    /// A bag book's page is rewritten in its template's language (`+0x1a0`, `0x4e37e9`) to the
    /// player's skill, unless a read marked the item (`0x4e3762`: the OK reply, so it shows as
    /// it is) or the show is a reason-1 reply's, which reads skill 0 (`0x4e37b3`); a letter's
    /// single text is never rewritten (`0x4e3666`).
    #[test]
    fn a_bag_books_page_is_rewritten_unless_read_and_untranslated_on_reason_1() {
        use benilla_protocol::SessionEvent as E;
        const BOOK: u64 = 0x4000_0000_0000_0021;
        const ME: u64 = 0x10;
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
            let (mut app, _rx) = reading_app(skill);
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
            assert_eq!(page_text(&app), want, "skill {skill}, {reply}");
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
