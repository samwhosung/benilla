//! The item-text reader (`ItemTextFrame.xml`) a bag letter, a book or a readable world object
//! opens in, locally with no packet as in the reference: every route ends at
//! `0x4e32e0(guid, flag)`, which looks the guid up as any object and asks it for its text
//! (`vtbl+0x74`), a mail-made letter's `ITEM_FIELD_ITEM_TEXT_ID` or else a page chain from an
//! item template's `PageText` or a `GAMEOBJECT_TYPE_TEXT` object's `data[0]`. vmangos has no use
//! arm for a type-9 object and refuses `CMSG_READ_ITEM` without a template `PageText`, which the
//! Plain Letter lacks (`ItemHandler.cpp:421`).
//!
//! The events follow `ItemTextFrame.lua:10-96`; `ITEM_TEXT_TRANSLATION`, the foreign-language
//! bar, never fires, as neither source carries the language column.

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
        self.pages.insert(page_id, Some(PageText { text, next }));
    }

    /// The cached page, asked for once; `guid` is the reading object, which vmangos discards.
    fn get_or_ask(&self, page_id: u32, guid: u64, commands: &NetCommands) -> Option<&PageText> {
        self.pages.get_or_ask(page_id, || {
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
    /// Keyed on the VM, so a `/reload` re-fires `ITEM_TEXT_BEGIN` for a reader still open.
    told: crate::ui_script::VmMemo<ItemTextTold>,
}

#[derive(Default)]
struct ItemTextTold {
    /// `ITEM_TEXT_BEGIN` fired: once per open, before the text lands.
    begun: bool,
    ready: bool,
}

#[derive(Resource, Default)]
pub(crate) struct ItemTextOpen {
    pub(crate) pending: Option<ReadSession>,
    /// A toggle closed the reader and the frame is not told yet; the feed holds the script.
    closing: bool,
}

impl ItemTextOpen {
    /// Open a bag letter; re-opening restarts from `ITEM_TEXT_BEGIN`.
    pub(crate) fn open_letter(&mut self, item_guid: u64, text_id: u32) {
        self.open(item_guid, ReadSource::Letter { text_id });
    }

    /// Open a page chain; the feed asks the object's template for the first page.
    pub(crate) fn open_pages(&mut self, object_guid: u64) {
        self.open(
            object_guid,
            ReadSource::Pages {
                visited: Vec::new(),
            },
        );
    }

    /// The reference's toggle, `0x4e32e0` with flag 0 from both click routes (`0x5d8e5e`,
    /// `0x5f58e7`): reading the open object again closes it. Returns whether it closed.
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

/// The page-text reply's handler.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::PageTexts;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::PageText, on_page_text);
    }

    /// One page per packet; the reader repaints off the cache on the next feed.
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
                    // Feed before the input pass so an open paints this frame; drain after it
                    // so a close clears this frame.
                    feed_item_text.in_set(UiFeed),
                    drain_item_text.after(UiInput),
                ),
            );
    }
}

/// What the read object answers: its name for the title (`ItemTextGetItem` `0x4e38f0`), its
/// material (`ItemTextGetMaterial` `0x4e39f0`: an item's `PageMaterial`, a GameObject's
/// `data[2]`) and its first page.
struct Readable {
    title: String,
    /// The `PageTextMaterial.dbc` basename; `None` is the Lua's Parchment default.
    material: Option<String>,
    /// The first `PageText` id; 0 for a letter or a template with no page.
    page_head: u32,
}

/// The material a quest from `guid` paints on, `GetQuestBackgroundMaterial` (`0x502230`), the
/// body of `ItemTextGetMaterial`; `None` is FrameXML's Parchment. Asks for an uncached GameObject.
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

/// The object's [`Readable`], item or GameObject alike; `None` while its template is in flight.
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

/// Drive the open session to `ITEM_TEXT_BEGIN` once the template resolves, as the stock handler
/// takes the text colour from the material (`ItemTextFrame.lua:18-23`), then to
/// `ITEM_TEXT_READY`, which shows the window, once the body and any creator line are in.
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
    // The `$`-macro subject for the page body: the local player.
    self_q: Query<(&crate::net::ObjectStore, &crate::net::Guid), With<crate::net::SelfPlayer>>,
    states: Res<crate::world_state::WorldStates>,
) {
    let Some(mut script) = script else {
        return;
    };
    // A toggle closed the reader; the frame is told here, where the script is.
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
        return; // template in flight; the reference re-enters the open when it lands
    };

    // No page to show: the reference bails before firing anything (`0x4e341d`).
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
            // The body, from the client's one item-text cache, which mail shares.
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
            // A book has no creator line.
            (
                None,
                page.text.clone(),
                visited.len() as u32,
                page.next != 0,
            )
        }
    };

    // Server-authored text runs the `$`-macro expander, as in the reference's `ItemTextFrame.cpp`.
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

/// `PageTextMaterial.dbc`; absent without client data, leaving every reader on Parchment.
#[derive(Resource)]
pub(crate) struct PageMaterials(pub(crate) benilla_formats::PageTextMaterialCatalog);

/// Drain the reader's intents: `CloseItemText()` clears the session and fires `ITEM_TEXT_CLOSED`;
/// a page turn re-runs the feed from `ITEM_TEXT_READY`, as the reference does not re-fire BEGIN.
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
            // Repaint on the next feed without re-firing BEGIN.
            sess.told.get(&script).ready = false;
        }
    }
    if script.take_item_text_close() && open.pending.take().is_some() {
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
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

    /// A GameObject's material is its template data by type; a creature's is Parchment.
    #[test]
    fn a_quest_sourced_from_an_object_paints_the_objects_material() {
        let items = Items::default();
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
