//! The reader's getters as a listener reads them, and its events at their packets. The getters read
//! the reader's cells and its object at the call (`0x4e38f0`-`0x4e3b10`); the open signals
//! `ITEM_TEXT_BEGIN` and the show `ITEM_TEXT_READY` inside the packet that runs them (the object's
//! page text `0x5f88c0`, a record's or a name's answer through `0x4e3280` or `0x4e32b0`), a page
//! turn inside its call, and a close a later packet makes waits behind what the lane still holds.

use benilla_protocol::SessionEvent;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, fall, frame};

/// A fall's `UNIT_COMBAT`, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";
/// "The Lay of Ameth'Aran", entry 17188, a GOOBER whose first page is 358.
const TABLET: u64 = 0xF110_0043_2400_0001;
/// A second tablet of the same template.
const OTHER_TABLET: u64 = 0xF110_0043_2400_0002;
const BANKER: u64 = 0xF130_0000_0B1E_0001;
const ME: u64 = 0x7;

/// The tablets streamed, their template's first page 358 cached with 359 after it.
fn tablets(app: &mut App) {
    let mut data = [0i32; 24];
    data[7] = 358;
    let world = app.world_mut();
    world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(17188, 10, "The Lay of Ameth'Aran".into(), &data);
    world
        .resource_mut::<crate::ui_item_text::PageTexts>()
        .insert(358, "Page one.".into(), 359);
    for guid in [TABLET, OTHER_TABLET] {
        crate::items::test_spawn_object(
            world,
            guid,
            crate::go_templates::test_typed_fields(17188, 10),
        );
    }
}

/// A spy on the reader's events and `extra`, each recorded by name; `ITEM_TEXT_READY` with the
/// text, the creator and the page its getters read as it fires, and with `close_on_hide` the
/// stock frame's `ITEM_TEXT_CLOSED` → `HideUIPanel` → OnHide → `CloseItemText()`
/// (`ItemTextFrame.lua:92`, `ItemTextFrame.xml:349`).
fn spy(app: &mut App, extra: &[&str], close_on_hide: bool) {
    let list: Vec<String> = ["ITEM_TEXT_BEGIN", "ITEM_TEXT_READY", "ITEM_TEXT_CLOSED"]
        .iter()
        .chain(extra)
        .map(|e| format!("{e:?}"))
        .collect();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(&format!(
            r#"
            local close = {close_on_hide}
            local f = CreateFrame("Frame")
            for _, e in ipairs({{{}}}) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function()
                if event == "ITEM_TEXT_READY" then
                    table.insert(Seen, event .. ":" .. ItemTextGetText() .. "/"
                        .. tostring(ItemTextGetCreator()) .. "/" .. ItemTextGetPage())
                else
                    table.insert(Seen, event)
                end
                if close and event == "ITEM_TEXT_CLOSED" then CloseItemText() end
            end)
            "#,
            list.join(", ")
        ))
        .unwrap();
}

/// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
fn eval<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, live: bool, chunk: &str) -> T {
    if live {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
            s.expect("a VM").eval::<T>(chunk).expect("runs")
        })
    } else {
        crate::script_calls::eval_in_vm(app.world(), chunk)
    }
}

/// The reader as its getters read it.
const READ: &str = "return tostring(ItemTextGetItem()) .. '/' .. ItemTextGetText() .. '/' \
    .. ItemTextGetPage() .. '/' .. tostring(ItemTextHasNextPage())";

fn page_text(guid: u64) -> SessionEvent {
    SessionEvent::GameObjectPageText { guid }
}

fn reader(app: &App) -> Option<u64> {
    app.world()
        .resource::<crate::ui_item_text::ItemTextOpen>()
        .object()
}

/// The object's page text runs its open and its show inside its handler (`0x5f8915` →
/// `0x4e32e0`, `0x4e359b` → `0x4e35b0`): with no frame after it, the getters read the object's
/// name, the page the show wrote (`0xbc3fe8`), its number and the next page it stored
/// (`0x4e3723`), through the host; the VM's copy, which nothing pushes, holds none.
#[test]
fn the_reader_getters_read_the_session_at_the_call() {
    let mut app = client();
    tablets(&mut app);
    crate::net::handlers::dispatch(app.world_mut(), vec![page_text(TABLET)]);
    assert_eq!(
        eval::<String>(&mut app, true, READ),
        "The Lay of Ameth'Aran/Page one./1/1"
    );
    assert_eq!(eval::<String>(&mut app, false, READ), "nil//1/nil");
}

/// The open's `ITEM_TEXT_BEGIN` and the show's `ITEM_TEXT_READY` fire at the page text's place
/// between a fall's `UNIT_COMBAT` on each side, the show's listener reading its page.
#[test]
fn the_readers_events_fire_between_neighbouring_packets_events() {
    let mut app = client();
    tablets(&mut app);
    spy(&mut app, &[], false);
    let seen = frame(&mut app, vec![fall(5), page_text(TABLET), fall(5)]);
    assert_eq!(
        seen,
        [
            FELL,
            "ITEM_TEXT_BEGIN",
            "ITEM_TEXT_READY:Page one./nil/1",
            FELL
        ]
    );
}

/// A tablet's page text, then a bank's open in the same frame: the bank's register closes the
/// reader first (`0x4930d0` → `0x493310` → `0x4e32e0(0, 0)`), so its `ITEM_TEXT_CLOSED` waits
/// behind the open's signals and fires ahead of `BANKFRAME_OPENED`, and the frame leaves the bank
/// latched, the reader shut.
#[test]
fn a_one_frame_open_then_close_leaves_no_stale_state() {
    let mut app = client();
    tablets(&mut app);
    crate::items::test_spawn_object(
        app.world_mut(),
        BANKER,
        benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]),
    );
    spy(&mut app, &["BANKFRAME_OPENED"], true);
    let seen = frame(
        &mut app,
        vec![page_text(TABLET), SessionEvent::ShowBank { banker: BANKER }],
    );
    // Until the lanes fire inside the dispatch, the show's listener reads the reader as the frame
    // left it, shut.
    let events: Vec<&str> = seen.iter().map(|e| e.split(':').next().unwrap()).collect();
    assert_eq!(
        events,
        [
            "ITEM_TEXT_BEGIN",
            "ITEM_TEXT_READY",
            "ITEM_TEXT_CLOSED",
            "BANKFRAME_OPENED"
        ]
    );
    assert_eq!(reader(&app), None);
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        Some(BANKER)
    );
    assert_eq!(
        app.world().resource::<crate::ui_bank::BankOpen>().banker,
        Some(BANKER)
    );
}

/// The close a later packet made fires behind the lane, while a third packet has opened another
/// tablet: the stock OnHide's `CloseItemText()` inside it closes the read it was signalled for,
/// already shut, and leaves the newer one open.
#[test]
fn a_parked_closes_nested_close_spares_the_newer_read() {
    let mut app = client();
    tablets(&mut app);
    crate::items::test_spawn_object(
        app.world_mut(),
        BANKER,
        benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]),
    );
    spy(&mut app, &["BANKFRAME_OPENED", "BANKFRAME_CLOSED"], true);
    let seen = frame(
        &mut app,
        vec![
            page_text(TABLET),
            SessionEvent::ShowBank { banker: BANKER },
            page_text(OTHER_TABLET),
        ],
    );
    assert_eq!(
        seen,
        [
            "ITEM_TEXT_BEGIN",
            "ITEM_TEXT_READY:Page one./nil/1",
            "ITEM_TEXT_CLOSED",
            "BANKFRAME_OPENED",
            "BANKFRAME_CLOSED",
            "ITEM_TEXT_BEGIN",
            "ITEM_TEXT_READY:Page one./nil/1",
        ]
    );
    assert_eq!(reader(&app), Some(OTHER_TABLET));
    assert_eq!(
        app.world()
            .resource::<crate::ui_session::InteractLatch>()
            .guid(),
        Some(OTHER_TABLET)
    );
}

/// `ItemTextNextPage()` moves to page 359 and shows it inside its call (`0x4e38b0` →
/// `0x4e35b0`), which finds it uncached and asks with the callback `0x4e3280` (`0x4e36ab`); the
/// page's answer shows it at its place, its listener reading page 2.
#[test]
fn a_turn_to_a_page_answered_later_shows_at_the_answer() {
    let mut app = client();
    tablets(&mut app);
    spy(&mut app, &[], false);
    frame(&mut app, vec![page_text(TABLET)]);
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    eval::<()>(&mut app, true, "ItemTextNextPage()");
    let asked: Vec<u32> = rx
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::PageTextQuery { page_id, .. } => Some(page_id),
            _ => None,
        })
        .collect();
    assert_eq!(asked, [359], "the show asks the page inside the call");
    assert!(eval::<Vec<String>>(&mut app, false, "local s = Seen Seen = {} return s").is_empty());
    let page = SessionEvent::PageText {
        page_id: 359,
        text: "Page two.".into(),
        next_page_id: 0,
    };
    let seen = frame(&mut app, vec![fall(5), page, fall(5)]);
    assert_eq!(seen, [FELL, "ITEM_TEXT_READY:Page two./nil/2", FELL]);
}

/// A letter whose author the name cache lacks: its show signals `ITEM_TEXT_READY` with no creator,
/// whose `ItemTextGetCreator()` asks the name with the callback `0x4e32b0` (`0x4e39b9`); the
/// name's answer shows the letter again at its place, now with its author. A refusal shows
/// nothing.
#[test]
fn a_letters_missing_author_shows_at_the_names_answer() {
    const LETTER: u64 = 0x4000_0000_0000_0009;
    const AUTHOR: u64 = 0x42;
    let mut app = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(5_000, Some(crate::items::test_template("Letter")));
    world
        .resource_mut::<crate::ui_mail::MailOpen>()
        .bodies
        .insert(77, "Dear you.".into());
    crate::items::test_spawn_item(
        world,
        LETTER,
        benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, 5_000),
            (8, ME as u32),
            (10, AUTHOR as u32),
            (11, 0),
        ]),
        false,
    );
    spy(&mut app, &[], false);
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let names = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::NameQuery { guid: AUTHOR }))
            .count()
    };
    crate::ui_item_text::read_letter_now(app.world_mut(), LETTER, 77);
    assert_eq!(
        eval::<Vec<String>>(&mut app, false, "local s = Seen Seen = {} return s"),
        ["ITEM_TEXT_BEGIN", "ITEM_TEXT_READY:Dear you./nil/1"]
    );
    assert_eq!(names(&rx), 1, "the getter asks the author");
    let name = |name: &str| SessionEvent::PlayerName {
        guid: AUTHOR,
        name: name.into(),
        race: 1,
        gender: 0,
        class: 1,
    };
    let seen = frame(&mut app, vec![fall(5), name("Brisca"), fall(5)]);
    assert_eq!(seen, [FELL, "ITEM_TEXT_READY:Dear you./Brisca/1", FELL]);
    assert_eq!(names(&rx), 0);
    // A second lookup that misses, then a refusal: nothing shows.
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .invalidate_player(AUTHOR);
    assert!(eval::<bool>(
        &mut app,
        true,
        "return ItemTextGetCreator() == nil"
    ));
    assert_eq!(names(&rx), 1);
    assert!(frame(&mut app, vec![name("")]).is_empty());
}

/// A TEXT object read before its template lands keeps the reader's cell with no page to read
/// (`0x5f5900` reads 0, `0x4e341d`); the template's answer runs its handler's record callback,
/// which opens the reader again (`0x5f59d0` → `0x5f5a17`), so the book begins and shows at the
/// answer's place.
#[test]
fn a_text_objects_book_read_before_its_template_opens_when_it_lands() {
    const PLAQUE: u64 = 0xF110_0000_4321_0001;
    let mut app = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_item_text::PageTexts>()
        .insert(358, "Page one.".into(), 0);
    crate::items::test_spawn_object(
        world,
        PLAQUE,
        crate::go_templates::test_typed_fields(4321, 9),
    );
    spy(&mut app, &[], false);
    crate::ui_item_text::read_now(app.world_mut(), PLAQUE);
    assert_eq!(reader(&app), Some(PLAQUE), "the cell stays with no page");
    assert!(frame(&mut app, vec![]).is_empty());
    let mut data = [0i32; 24];
    data[0] = 358;
    let template = SessionEvent::GameObjectInfo {
        entry: 4321,
        type_id: 9,
        display_id: 0,
        name: "Plaque".into(),
        data,
    };
    let seen = frame(&mut app, vec![fall(5), template, fall(5)]);
    assert_eq!(
        seen,
        [
            FELL,
            "ITEM_TEXT_BEGIN",
            "ITEM_TEXT_READY:Page one./nil/1",
            FELL
        ]
    );
}

/// The reader's answers read the record their packet filed: each kind's cache handler runs ahead
/// of the reader's, whatever the plugin order, as the reference's receive files the record before
/// its callbacks run (`0x55bdb0`, `0x55f426`).
#[test]
fn the_caches_file_each_answer_ahead_of_the_readers_handler() {
    use benilla_protocol::SessionEventKind as K;
    let app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let table = app
        .world()
        .resource::<crate::net::handlers::NetHandlers>()
        .census();
    for (kind, filer, reader) in [
        (
            K::MailItemText,
            "ui_mail::net::on_mail_item_text",
            "ui_item_text::net::on_item_text",
        ),
        (
            K::ItemTemplate,
            "net::objects::on_item_answer",
            "ui_item_text::net::on_item_template",
        ),
        (
            K::GameObjectInfo,
            "net::objects::on_gameobject_info",
            "ui_item_text::net::on_object_template",
        ),
        (
            K::PlayerName,
            "names::net::on_player_name",
            "ui_item_text::net::on_player_name",
        ),
    ] {
        let order: Vec<&str> = table[&kind]
            .iter()
            .map(|name| name.trim_start_matches("benilla_app::"))
            .collect();
        let at = |name: &str| order.iter().position(|n| *n == name);
        assert!(
            matches!((at(filer), at(reader)), (Some(f), Some(r)) if f < r),
            "{kind:?}: {order:?}"
        );
    }
}
