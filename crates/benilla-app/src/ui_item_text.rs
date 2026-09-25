//! The item-text reader session (`ItemTextFrame.xml`) — the one window every *readable* opens in:
//! a bag letter, a book in your bags, and a book/plaque lying in the world.
//!
//! **One reader, keyed on an object guid, opened locally with no permission packet.** That is the
//! reference's own shape, byte-verified: every route ends at `0x4e32e0(guid, flag)`,
//! which looks the guid up with typemask `1` (*any* object — item or GameObject), asks the object
//! for its page id through the shared `vtbl+0x74` getter, and pulls the text out of a cache. Which
//! of the two text sources applies is the object's own answer, not the caller's:
//!
//! - **a letter** — an item *instance* carrying `ITEM_FIELD_ITEM_TEXT_ID` (a mail-made permanent
//!   copy). One body, no pages; it rides the ask-once `CMSG_ITEM_TEXT_QUERY` cache mail letters use
//!   ([`crate::ui_mail::MailOpen::bodies`] — that map is the client's item-text cache, mail is just
//!   its first tenant), and its creator line resolves `ITEM_FIELD_CREATOR` through the name cache.
//! - **a page chain** — a readable item *template*'s `PageText`, or a `GAMEOBJECT_TYPE_TEXT` world
//!   object's template `data[0]`. Pages chain by `nextPageId` over the ask-once
//!   [`PageTexts`] cache (`CMSG_PAGE_TEXT_QUERY`), and the reader's prev/next buttons walk them.
//!   Authorless — a book has no `From,` tail.
//!
//! The routes in: `ui_items::drain::drain_container_uses` (the bag click's dispatcher arms #5 and
//! #6) and `target::click::act_on_right_click` (the world right-click's GO type-9 arm). None of
//! them sends a use/read packet — vmangos has no `GameObject::Use` case for type 9 at all, and its
//! `CMSG_READ_ITEM` handler gates on a template `PageText` the Plain Letter doesn't have, so both
//! would be answered with silence even if we asked.
//!
//! The feed mirrors the reference event flow (ItemTextFrame.lua l.10-96): open →
//! `ITEM_TEXT_BEGIN` (title known), everything fetched → `ITEM_TEXT_READY`, `CloseItemText()` →
//! session cleared + `ITEM_TEXT_CLOSED`. `ITEM_TEXT_TRANSLATION` (the foreign-language progress
//! bar) never fires — no translation mechanic on a private server's readables, and the `language`
//! column (`data[1]`/`LanguageID`) is carried by neither source here.

use bevy::prelude::*;

use benilla_ui::script::{ItemTextState, UiScript};

use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, Objects};
use crate::query_cache::QueryCache;
use crate::ui_mail::MailOpen;
use crate::ui_script::{UiFeed, UiInput};

/// One page of a book as the wire gave it (`SMSG_PAGE_TEXT_QUERY_RESPONSE`).
pub(crate) struct PageText {
    pub(crate) text: String,
    /// The next page's id; `0` = this is the last page.
    pub(crate) next: u32,
}

/// The ask-once page cache — the client's `PageText` dbcache (`0xc0e174`). Keyed by
/// page id, filled by [`crate::net::apply`]'s `SessionEvent::PageText` arm.
///
/// vmangos answers a single `CMSG_PAGE_TEXT_QUERY` with the **whole chain** (one response per page),
/// so asking for page 1 normally lands the entire book at once and a page turn is a cache hit. The
/// per-page fetch is still here for the server that answers one at a time.
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
    /// Record a page (and clear its in-flight flag).
    pub(crate) fn insert(&mut self, page_id: u32, text: String, next: u32) {
        self.pages.insert(page_id, Some(PageText { text, next }));
    }

    /// The cached page, asking for it once if this is the first asker. `guid` names the object
    /// doing the reading (the reference writes it after the page id; vmangos discards it).
    fn get_or_ask(&self, page_id: u32, guid: u64, commands: &NetCommands) -> Option<&PageText> {
        self.pages.get_or_ask(page_id, || {
            debug!("page text: asking page {page_id} (object {guid:#x})");
            let _ = commands
                .0
                .send(ClientCommand::PageTextQuery { page_id, guid });
        })
    }
}

/// Which text a read session is showing — the reference's two sources, which it picks between by
/// asking the *object* (an instance text id, else a page id), not by how the click arrived.
pub(crate) enum ReadSource {
    /// An item instance's `ITEM_FIELD_ITEM_TEXT_ID` — a mail-made letter. Single body, has a
    /// creator line.
    Letter { text_id: u32 },
    /// A page chain — a readable item template's `PageText`, or a TEXT GameObject's `data[0]`.
    /// `visited` is the trail of page ids from the first to the one on screen, so **Prev** can walk
    /// back a chain that only links forwards (the reference keeps the same array, `[0xbc3fd0]`).
    /// **Empty until the object's template answers**: the reference asks the *object* for its page
    /// id at paint time (`vtbl+0x74`, with an ask-once template callback that re-enters the whole
    /// open — `0x5f59d0`), so a click that lands before the template does still opens a reader.
    Pages { visited: Vec<u32> },
}

/// The open read session; `None` in [`ItemTextOpen::pending`] = no reader open.
pub(crate) struct ReadSession {
    /// The read object's guid — an item's or a GameObject's. Title/creator resolve off it each
    /// frame until ready, and re-using the *same* object closes the reader (the reference's toggle).
    pub(crate) object_guid: u64,
    pub(crate) source: ReadSource,
    /// What the live VM has been told about this session — keyed on the VM (decisions
    /// 1290/1291), because the session is world state (the letter is still open) while the
    /// fires are VM state: a `/reload` with a book up replaces the frame tree, and the fresh
    /// one needs the `ITEM_TEXT_BEGIN` and repaint the old one already consumed. Before this,
    /// the reader never came back after a reload while the host still believed it was open.
    told: crate::ui_script::VmMemo<ItemTextTold>,
}

/// [`ReadSession::told`]'s payload — the per-VM fire latches.
#[derive(Default)]
struct ItemTextTold {
    /// `ITEM_TEXT_BEGIN` fired (the reference fires it once per open, before the text lands).
    begun: bool,
    /// `ITEM_TEXT_READY` fired — the session is fully painted.
    ready: bool,
}

/// The reader-session resource ([`ReadSession`]).
#[derive(Resource, Default)]
pub(crate) struct ItemTextOpen {
    pub(crate) pending: Option<ReadSession>,
    /// A [`ItemTextOpen::toggle_closed`] fired and the frame has not been told yet. The click
    /// routes have no `UiScript` in reach, so the close event is handed to the feed — which owns
    /// the script — rather than each caller re-implementing it.
    closing: bool,
}

impl ItemTextOpen {
    /// Open a read for a bag letter (dispatcher arm #6). Re-opening restarts the reference event
    /// flow from `ITEM_TEXT_BEGIN`.
    pub(crate) fn open_letter(&mut self, item_guid: u64, text_id: u32) {
        self.open(item_guid, ReadSource::Letter { text_id });
    }

    /// Open a read for a page chain — a readable item template (arm #5) or a TEXT GameObject. The
    /// page head is not passed in: the feed asks the object's template for it, like the reference.
    pub(crate) fn open_pages(&mut self, object_guid: u64) {
        self.open(
            object_guid,
            ReadSource::Pages {
                visited: Vec::new(),
            },
        );
    }

    /// The reference's **toggle** (`0x4e32e0`'s `arg2 == 0` head, which *every* click route passes —
    /// the bag readable at `0x5d8e5e` and the TEXT GameObject at `0x5f58e7` both `xor edx,edx`):
    /// re-clicking the readable whose reader is already open *closes* it. Returns whether it fired,
    /// so the caller stops there.
    pub(crate) fn toggle_closed(&mut self, object_guid: u64) -> bool {
        let open = self
            .pending
            .as_ref()
            .is_some_and(|s| s.object_guid == object_guid);
        if open {
            self.pending = None;
            self.closing = true;
        }
        open
    }

    /// Take the pending toggle-close, for the feed to turn into `ITEM_TEXT_CLOSED`.
    fn take_closing(&mut self) -> bool {
        std::mem::take(&mut self.closing)
    }

    fn open(&mut self, object_guid: u64, source: ReadSource) {
        self.pending = Some(ReadSession {
            object_guid,
            source,
            told: Default::default(),
        });
    }
}

/// The book reader's packet handler (in the net handler table since 2313).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::PageTexts;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::UiItemTextPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text);
    }

    /// The book-page cache — one page per packet, the whole chain in answer to the first ask;
    /// the reader repaints off it on the next feed.
    fn on_page_text(In(ev): In<SessionEvent>, mut pages: ResMut<PageTexts>) {
        if let SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        } = ev
        {
            pages.insert(page_id, text, next_page_id);
        }
    }
}

pub(crate) struct UiItemTextPlugin;

impl Plugin for UiItemTextPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PageTexts>(app);
        app.init_resource::<ItemTextOpen>()
            .init_resource::<PageTexts>()
            .add_systems(
                Update,
                (
                    // Feed before the input pass so an open paints the same frame; drain after it so
                    // a Close click clears the same frame (the ui_mail ordering).
                    feed_item_text.in_set(UiFeed),
                    drain_item_text.after(UiInput),
                ),
            );
    }
}

/// What the reader's *object* answers, whichever source its text comes from: the window title
/// (`ItemTextGetItem 0x4e38f0` → the looked-up object's `vtbl+0x70` name) and the frame material
/// (`ItemTextGetMaterial 0x4e39f0` → an item template's `PageMaterial`, a GameObject's `data[2]`),
/// plus the page head a page chain starts at. `None` = the template is still in flight.
struct Readable {
    title: String,
    /// `PageTextMaterial.dbc` id → basename; `None` = the Lua's Parchment default.
    material: Option<String>,
    /// The first `PageText` id — `0` for a letter (which has no page chain) and for a readable
    /// whose template carries no page.
    page_head: u32,
}

/// The material a quest sourced from `guid` paints on — `GetQuestBackgroundMaterial 0x502230`,
/// whose body is `ItemTextGetMaterial`'s: the source object by guid, an item's template
/// `PageMaterial` or a GameObject's template data by type, the id named through
/// `PageTextMaterial.dbc`; a creature, an uncached template or an id with no row is `None`, which
/// is FrameXML's "Parchment". A GameObject whose template is not cached yet is asked for.
pub(crate) fn object_material(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
    commands: &NetCommands,
) -> Option<String> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    if benilla_protocol::guid::is_gameobject(guid) {
        return match go_templates.get(guid) {
            Some(go) => go.quest_material.and_then(name),
            None => {
                go_templates.request(guid, commands);
                None
            }
        };
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, commands)?;
    name(t.page_material)
}

/// Resolve the object's title + material + page head, whether it is an item or a GameObject. The
/// reference does exactly this — one guid lookup with typemask 1, then virtual getters — so a
/// single resolve serves both sources.
fn readable(
    guid: u64,
    objects: &Objects,
    items: &Items,
    go_templates: &GameObjectTemplates,
    materials: Option<&PageMaterials>,
    commands: &NetCommands,
) -> Option<Readable> {
    let name = |id: u32| materials.and_then(|m| m.0.name(id)).map(str::to_string);
    if let Some(go) = go_templates.get(guid) {
        return Some(Readable {
            title: go.name.clone(),
            material: go.text_page.and_then(|p| name(p.material)),
            page_head: go.text_page.map_or(0, |p| p.page_id),
        });
    }
    let entry = objects.object(guid)?.object_entry()?;
    let t = items.template(entry, guid, commands)?;
    Some(Readable {
        title: t.name.clone(),
        material: name(t.page_material),
        page_head: t.page_text,
    })
}

/// Drive the open session to `ITEM_TEXT_BEGIN`/`ITEM_TEXT_READY` as its pieces land. BEGIN holds
/// until the object's template resolves — it carries the title *and* the material, and the Lua's
/// BEGIN handler picks the page font and text colour off the material, so firing it early would
/// paint a Stone plaque in parchment ink. READY holds until the body and, for a letter, the creator
/// line are both in. (The window itself only shows on READY — `ItemTextFrame.lua` calls
/// `ShowUIPanel` there, not on BEGIN.)
fn feed_item_text(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<ItemTextOpen>,
    mail: Res<MailOpen>,
    pages: Res<PageTexts>,
    objects: Objects,
    items: Res<Items>,
    names: Res<NameCache>,
    go_templates: Res<GameObjectTemplates>,
    materials: Option<Res<PageMaterials>>,
    commands: Res<NetCommands>,
    // The `$`-macro subject for the page body: the local player, as at every panel seam.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
) {
    let Some(mut script) = script else {
        return;
    };
    // A re-click closed the reader (the reference's toggle → its `0x128`). The click route cleared
    // the session; the frame is told here, where the script is.
    if open.take_closing() {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
    }
    let Some(sess) = open.pending.as_mut() else {
        return;
    };
    if sess.told.get(&script).ready {
        return;
    }
    let Some(readable) = readable(
        sess.object_guid,
        &objects,
        &items,
        &go_templates,
        materials.as_deref(),
        &commands,
    ) else {
        return; // template in flight — the reference re-enters the whole open when it lands
    };

    // A page read whose template answers with no page has nothing to show — the reference bails
    // before firing anything (`0x4e341d`, both text sources zero). Drop the session silently.
    if matches!(sess.source, ReadSource::Pages { .. }) && readable.page_head == 0 {
        debug!(
            "item text: {:#x} has no page to read — closing",
            sess.object_guid
        );
        open.pending = None;
        return;
    }

    if !sess.told.get(&script).begun {
        script.set_item_text(Some(ItemTextState {
            item: readable.title.clone(),
            creator: None,
            text: String::new(),
            page: 1,
            has_next: false,
            material: readable.material.clone(),
        }));
        script.fire_event("ITEM_TEXT_BEGIN", vec![]);
        sess.told.get(&script).begun = true;
    }

    let (creator, text, page, has_next) = match &mut sess.source {
        ReadSource::Letter { text_id } => {
            // The creator line: `ITEM_FIELD_CREATOR` → name cache (ask-once). `None` guid =
            // authorless.
            let creator_guid = objects
                .object(sess.object_guid)
                .and_then(|o| o.item_creator());
            let creator = match creator_guid {
                None => None, // authorless
                Some(guid) => match names.resolve(guid, &commands) {
                    Some(name) => Some(name.to_string()),
                    None => return, // name query in flight
                },
            };
            // The body: the shared ask-once item-text cache (fetch if this is the first asker).
            let body = mail
                .bodies
                .get_or_ask(*text_id, || {
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
            let Some(page) = pages.get_or_ask(page_id, sess.object_guid, &commands) else {
                return; // page query in flight
            };
            // A book is authorless — the creator leg is the reference's item-instance one.
            (
                None,
                page.text.clone(),
                visited.len() as u32,
                page.next != 0,
            )
        }
    };

    // Page/book text is server-authored, so it runs the `$`-macro expander — the reference does it
    // from two sites in `ItemTextFrame.cpp`, subject = the local player.
    let subject = crate::npc_text::player_identity(&self_q, &names, &commands);
    let text = crate::npc_text::substitute(
        &text,
        &crate::npc_text::MacroContext {
            subject: subject.as_ref(),
            states: &states,
        },
    );
    script.set_item_text(Some(ItemTextState {
        item: readable.title,
        creator,
        text,
        page,
        has_next,
        material: readable.material,
    }));
    script.fire_event("ITEM_TEXT_READY", vec![]);
    sess.told.get(&script).ready = true;
}

/// `PageTextMaterial.dbc` as a resource — the reader frame's material basename,
/// loaded once at startup ([`crate::entities`]); absent when the client data is, in which case every
/// readable falls to the Lua's Parchment default.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

/// Drain the reader's intents: `CloseItemText()` clears the session (+ `ITEM_TEXT_CLOSED`, the
/// reference C-side answer the frame's OnHide relies on); a page turn walks the chain — Next
/// appends the current page's `nextPageId` to the trail, Prev pops back — and re-runs the feed from
/// `ITEM_TEXT_READY` (the reference repaints the same frame, it does not re-fire BEGIN).
fn drain_item_text(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<ItemTextOpen>,
    pages: Res<PageTexts>,
) {
    let Some(mut script) = script else {
        return;
    };
    for delta in script.take_item_text_page_turns() {
        let Some(sess) = open.pending.as_mut() else {
            continue;
        };
        let ReadSource::Pages { visited } = &mut sess.source else {
            continue; // a letter has no pages; its buttons never show
        };
        if turn_page(visited, delta, |id| pages.pages.get(id).map(|p| p.next)) {
            // Repaint on the next feed, without re-firing BEGIN (within this VM; a fresh VM
            // re-begins regardless, which is the reload repaint).
            sess.told.get(&script).ready = false;
        }
    }
    if script.take_item_text_close() && open.pending.take().is_some() {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
    }
}

/// Walk the visited-page trail one step. A `PageText` chain only links **forwards**, so Prev is a
/// pop off the trail rather than a lookup — the reference keeps the same array (`[0xbc3fd0]`).
/// Returns whether the page actually changed (a no-op turn must not repaint).
///
/// `next_of` is the page cache: `None` = the page hasn't landed yet, `Some(0)` = last page. Both
/// refuse the turn; neither can normally be clicked, since the Next button only shows while the
/// painted page reported a next.
fn turn_page(visited: &mut Vec<u32>, delta: i32, next_of: impl Fn(u32) -> Option<u32>) -> bool {
    let Some(&current) = visited.last() else {
        return false; // head not resolved yet — no page is painted to turn from
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

    /// A three-page book: 100 → 101 → 102. Next walks it, Prev walks back, and the ends refuse.
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

    /// A Next clicked before the page landed is refused rather than pushing a bogus id.
    #[test]
    fn a_page_still_in_flight_refuses_the_turn() {
        let mut visited = vec![7];
        assert!(!turn_page(&mut visited, 1, |_| None));
        assert_eq!(visited, [7]);
    }

    /// The reference's toggle: re-clicking the readable that is already open closes it; a
    /// *different* one does not.
    #[test]
    fn re_reading_the_same_object_toggles_closed() {
        let mut open = ItemTextOpen::default();
        open.open_pages(0xF110_0000_0000_0001);
        assert!(!open.toggle_closed(0xF110_0000_0000_0002), "another book");
        assert!(open.pending.is_some());
        assert!(open.toggle_closed(0xF110_0000_0000_0001));
        assert!(open.pending.is_none());
        assert!(
            open.take_closing(),
            "the frame must be told — a cleared session alone leaves the window on screen"
        );
        assert!(!open.take_closing(), "drained");
        assert!(!open.toggle_closed(0xF110_0000_0000_0001), "nothing open");
        assert!(!open.take_closing(), "a no-op toggle closes nothing");
    }

    /// A page chain opens with an EMPTY trail — the head comes from the object's template as the
    /// feed paints, so a click that beats the ask-once template query still opens a reader.
    #[test]
    fn a_page_read_opens_before_its_template_lands() {
        let mut open = ItemTextOpen::default();
        open.open_pages(0xF110_0000_0000_0001);
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

    /// `GetQuestBackgroundMaterial`'s resolve, the reader's twin: a GameObject source answers its
    /// template data by type through `PageTextMaterial.dbc`; a creature source answers nothing
    /// (FrameXML's Parchment); an uncached GameObject is asked for and answers nothing meanwhile.
    #[test]
    fn a_quest_sourced_from_an_object_paints_the_objects_material() {
        let items = Items::default();
        // No item source in this case — every one of these is a GameObject or a creature (2334).
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut gos = GameObjectTemplates::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
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
            object_material(
                go_guid(100),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            )
            .as_deref(),
            Some("Stone")
        );
        assert_eq!(
            object_material(
                go_guid(102),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            )
            .as_deref(),
            Some("Marble"),
            "a chest reads data[9]"
        );
        // A creature quest-giver: neither template, nothing asked.
        let creature = (u64::from(benilla_protocol::guid::HIGH_UNIT) << 48) | (6 << 24) | 1;
        assert_eq!(
            object_material(creature, &objects, &items, &gos, Some(&catalog), &commands),
            None
        );
        assert!(rx.try_recv().is_err(), "a creature source asks for nothing");
        // An uncached object: nothing yet, and the template is asked for.
        assert_eq!(
            object_material(
                go_guid(777),
                &objects,
                &items,
                &gos,
                Some(&catalog),
                &commands
            ),
            None
        );
        assert!(
            rx.try_recv().is_ok(),
            "an uncached object's template is asked for"
        );
    }
}
