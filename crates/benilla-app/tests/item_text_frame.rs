//! Drives the stock reader window (`ItemTextFrame.xml`, off the player's chain) through the
//! engine: pushes an `ItemTextState`, fires `ITEM_TEXT_BEGIN`, `ITEM_TEXT_READY` and
//! `ITEM_TEXT_CLOSED`, and asserts what the Lua paints.

use benilla_ui::script::{ItemTextState, UiScript};

#[path = "common/mod.rs"]
mod common;

const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ui");

/// The reader's dependency prefix, in the manifest's order; the page's scroll frame inherits
/// `UIPanelScrollFrameTemplate` (`UIPanelTemplates.xml`).
const FILES: &[&str] = &[
    // `ITEM_TEXT_FROM`, concatenated into the creator tail (`ItemTextFrame.lua:41`); nil, it
    // kills the handler before `ShowUIPanel`.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    // `GetMaterialTextColors` (`UIParent.lua:1880`), which picks the page and title ink.
    r"Interface\FrameXML\UIParent.xml",
    "ScrollTemplates.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine
    "Interface\\FrameXML\\ItemTextFrame.xml",
];

/// Loads `FILES`: a bare filename from `assets/ui`, a path off the player's chain. The provider
/// also serves chain paths, since `ItemTextFrame.xml` pulls its Lua through a relative
/// `<Script file=…>` resolved against its own directory.
fn load_ui(script: &UiScript) {
    let dir = std::path::Path::new(UI_DIR);
    let chain = benilla_formats::wow_data()
        .and_then(|data| benilla_formats::open_chain(&data).ok())
        .expect("client data — every test here gates on it");
    let read = |req: &str| -> Option<Vec<u8>> {
        if req.contains('\\') || req.contains('/') {
            return chain.read(req).ok();
        }
        std::fs::read(dir.join(req)).ok()
    };
    for file in FILES {
        let bytes = read(file).unwrap_or_else(|| panic!("reading {file}"));
        // A `.lua` entry is a chunk, passed as raw bytes; only an XML parse decodes.
        if file.to_ascii_lowercase().ends_with(".lua") {
            script
                .run_chunk_named(&bytes, &format!("@{file}"))
                .unwrap_or_else(|e| panic!("running {file}: {e}"));
            continue;
        }
        let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
            .unwrap_or_else(|e| panic!("parsing {file}: {e}"));
        let report = benilla_ui::loader::load_in(script, &doc, &file.replace('\\', "/"), &read);
        assert!(
            report.errors.is_empty(),
            "{file} loaded with errors: {:#?}",
            report.errors
        );
    }
}

/// The page body as drawn, one string per block, read off the render list: `ItemTextPageText` is
/// a `SimpleHTML`, which has no text getter in 1.12 (`0x87ba80`). A plain body is one block.
fn page_blocks(s: &UiScript) -> Vec<String> {
    use benilla_ui::script::QuadContent;
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Text { text, .. } => text,
            _ => None,
        })
        .collect()
}

fn letter() -> ItemTextState {
    ItemTextState {
        item: "Plain Letter".into(),
        creator: Some("One".into()),
        text: "asd".into(),
        page: 1,
        has_next: false,
        material: None,
    }
}

/// BEGIN paints the title, READY paints the body with the "From," tail and shows the window; a
/// single page shows no page number and no page-turn buttons.
#[test]
fn a_letter_reads_with_the_creator_tail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    assert!(!s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());

    s.set_item_text(Some(letter()));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    assert_eq!(
        s.eval::<String>("return ItemTextTitleText:GetText()")
            .unwrap(),
        "Plain Letter"
    );
    s.fire_event("ITEM_TEXT_READY", vec![]);
    assert!(s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());
    assert!(
        page_blocks(&s).contains(&"\nasd\n\nFrom,\nOne\n\n".to_string()),
        "the reference creator tail (ITEM_TEXT_FROM really is comma'd), drawn as one raw block: {:?}",
        page_blocks(&s)
    );
    for hidden in [
        "ItemTextCurrentPage",
        "ItemTextPrevPageButton",
        "ItemTextNextPageButton",
        "ItemTextStatusBar",
    ] {
        assert!(
            !s.eval::<bool>(&format!("return {hidden}:IsShown()"))
                .unwrap(),
            "{hidden} must stay hidden on a single-page letter"
        );
    }
    assert!(s.take_errors().is_empty());
}

/// The black `$parentMiddle` track sits right of the page: the stock XML declares ARTWORK before
/// BACKGROUND because `Middle` anchors to `Top` by name, resolved at SetPoint time. Also, no
/// anchor in the chain is unresolved: an XML one is reported and skipped (`0x767800`,
/// `"Couldn't find relative frame: %s"` at `0x878440`) and a Lua one raises (`0x87ccd4`).
#[test]
fn the_scrollbar_track_sits_right_of_the_page() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    let unresolved: Vec<String> = s
        .warnings()
        .into_iter()
        .filter(|w| w.contains("Couldn't find relative frame"))
        .collect();
    assert!(unresolved.is_empty(), "unresolved anchors: {unresolved:#?}");

    s.set_screen_size(1024.0, 768.0);
    s.set_item_text(Some(letter()));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    s.resolve();
    let (track_left, page_right): (f32, f32) = s
        .eval(
            "return ItemTextScrollFrameMiddle:GetLeft(), \
             ItemTextScrollFrame:GetRight()",
        )
        .unwrap();
    assert!(
        track_left >= page_right - 0.5,
        "the track ({track_left}) must not overlap the page (right edge {page_right})"
    );
    assert!(s.take_errors().is_empty());
}

/// A book: no "From," tail; page 1 with a next shows the page number and Next only.
#[test]
fn a_book_page_shows_the_paging_chrome() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_item_text(Some(ItemTextState {
        item: "Lament of the Highborne".into(),
        creator: None,
        text: "page one".into(),
        page: 1,
        has_next: true,
        material: None,
    }));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    assert!(
        page_blocks(&s).contains(&"\npage one\n".to_string()),
        "the page body drawn: {:?}",
        page_blocks(&s)
    );
    assert_eq!(
        s.eval::<String>("return ItemTextCurrentPage:GetText()")
            .unwrap(),
        "1"
    );
    assert!(!s
        .eval::<bool>("return ItemTextPrevPageButton:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return ItemTextNextPageButton:IsShown()")
        .unwrap());
    s.run("ItemTextNextPageButton:Click()").unwrap();
    assert_eq!(s.take_item_text_page_turns(), vec![1]);
    assert!(s.take_errors().is_empty());
}

/// The X hides the panel and its OnHide queues `CloseItemText`; the app then clears the session
/// and fires `ITEM_TEXT_CLOSED`, which hides again harmlessly.
#[test]
fn closing_queues_the_close_intent() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_item_text(Some(letter()));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    let _ = s.take_item_text_close();

    s.run("ItemTextCloseButton:Click()").unwrap();
    assert!(!s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());
    assert!(s.take_item_text_close(), "OnHide queued the close intent");

    s.set_item_text(None);
    s.fire_event("ITEM_TEXT_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());
    assert!(s.take_errors().is_empty());
}

/// The Stormwind Old Town plaque (`page_text` 2676) draws as HTML blocks, not as its own markup,
/// and is not truncated: the page is a `SimpleHTML`, not a FontString.
#[test]
fn the_reported_html_page_draws_as_blocks_not_as_its_own_markup() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    s.set_item_text(Some(ItemTextState {
        item: "Alliance Military Ranks".into(),
        creator: None,
        text: "<HTML>\n<BODY>\n\
               <H1 align=\"center\">ALLIANCE MILITARY RANKS</H1><BR/>\n\
               <P align=\"center\">OFFICERS</P><BR/>\n\
               <P align=\"center\">Grand Marshal</P>\n\
               <P align=\"center\">Knight</P><BR/>\n\
               <P align=\"center\">ENLISTED</P><BR/>\n\
               <P align=\"center\">Private</P>\n\
               </BODY>\n</HTML>"
            .into(),
        page: 1,
        has_next: false,
        material: None,
    }));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    assert!(s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());

    let drawn = page_blocks(&s);
    for line in [
        "ALLIANCE MILITARY RANKS",
        "OFFICERS",
        "Grand Marshal",
        "Knight",
        "ENLISTED",
        "Private",
    ] {
        assert!(
            drawn.contains(&line.to_string()),
            "{line:?} should draw as its own block; drawn: {drawn:?}"
        );
    }
    assert!(
        !drawn
            .iter()
            .any(|b| b.contains("<HTML>") || b.contains("<P align")),
        "the markup itself must never reach the page — that is what was photographed: {drawn:?}"
    );
    // A height-pinned FontString would end a long page in an ellipsis.
    assert!(
        !drawn.iter().any(|b| b.ends_with("...")),
        "a block was truncated — the page is height-pinned again: {drawn:?}"
    );
}

/// An unsized `<IMG>` draws at its texture's texel extent, one texel to one FrameXML unit
/// (`CSimpleTexture`, `0x770720`): the crest on `page_text` 2654, quoted below, is a 128×128 BLP
/// inside a 270-wide page.
#[test]
fn the_reported_book_crest_draws_at_the_blps_own_size() {
    let _data = benilla_formats::wow_data_or_skip!();
    let data = benilla_formats::wow_data_or_skip!();
    let chain = std::sync::Mutex::new(benilla_formats::open_chain(&data).expect("open chain"));

    let mut s = UiScript::new().unwrap();
    // Wired as `ui_script::lifecycle::install_addon_asset_resolvers` wires the live one.
    s.set_texture_size_probe(Box::new(move |path| {
        benilla_assets::sprite_dimensions(&chain, None, path)
    }));
    load_ui(&s);
    s.set_item_text(Some(ItemTextState {
        item: "A Treatise on Military Ranks".into(),
        creator: None,
        text: "<HTML>\n<BODY>\n\
               <H1 align=\"center\">A TREATISE ON MILITARY RANKS</H1>\n\
               <BR/>\n<BR/>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRankAlliance\" align=\"left\"/>\n\
               <BR/>\n\
               <P align=\"right\">What follows are</P>\n\
               <P align=\"right\">the military ranks</P>\n\
               </BODY>\n</HTML>"
            .into(),
        page: 1,
        has_next: true,
        material: None,
    }));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    s.resolve();

    let crest: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| match &q.content {
            benilla_ui::script::QuadContent::Texture { path, .. } => {
                path.as_deref().is_some_and(|t| {
                    t.eq_ignore_ascii_case("Interface\\PvPRankBadges\\PvPRankAlliance")
                })
            }
            _ => false,
        })
        .filter_map(|q| q.rect)
        .collect();
    assert_eq!(crest.len(), 1, "one crest quad on the page");
    let r = crest[0];
    assert_eq!(
        (r.right - r.left, r.top - r.bottom),
        (128.0, 128.0),
        "the crest draws at the BLP's own 128x128 — a texel is a FrameXML unit"
    );
    // The page is 270 units wide.
    assert!(
        r.right - r.left < 270.0,
        "the crest spans the whole page again — that is the photograph"
    );

    // Page 2 (`page_text` 2655): five unsized `<IMG>`s, each a 32x32 BLP. This VM has no font
    // engine, so text measures zero and only the sizes are checked, not the spacing.
    s.set_item_text(Some(ItemTextState {
        item: "A Treatise on Military Ranks".into(),
        creator: None,
        text: "<HTML>\n<BODY>\n\
               <H1 align=\"center\">OFFICER RANKS OF THE ALLIANCE</H1><BR/>\n\
               <P align=\"center\">Part 1</P>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRank14\" align=\"left\"/><BR/>\n\
               <P align=\"right\">Grand Marshal</P><BR/><BR/>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRank13\" align=\"left\"/><BR/>\n\
               <P align=\"right\">Field Marshal</P><BR/><BR/>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRank12\" align=\"left\"/><BR/>\n\
               <P align=\"right\">Marshal</P><BR/><BR/>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRank11\" align=\"left\"/><BR/>\n\
               <P align=\"right\">Commander</P><BR/><BR/>\n\
               <IMG src=\"Interface\\PvPRankBadges\\PvPRank10\" align=\"left\"/><BR/>\n\
               <P align=\"right\">Lieutenant Commander</P><BR/><BR/>\n\
               </BODY>\n</HTML>"
            .into(),
        page: 2,
        has_next: true,
        material: None,
    }));
    s.fire_event("ITEM_TEXT_READY", vec![]);
    s.resolve();

    let badges: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| match &q.content {
            benilla_ui::script::QuadContent::Texture { path, .. } => path
                .as_deref()
                .is_some_and(|t| t.starts_with("Interface\\PvPRankBadges\\PvPRank")),
            _ => false,
        })
        .filter_map(|q| q.rect)
        .collect();
    assert_eq!(badges.len(), 5, "one quad per officer rank");
    for b in &badges {
        assert_eq!(
            (b.right - b.left, b.top - b.bottom),
            (32.0, 32.0),
            "each rank badge is its own 32x32 BLP, not a page-wide slab"
        );
    }
}

/// The reader's `UIPanelWindows` row (`{ area = "left", pushable = 0 }`, `UIParent.lua:20`) makes
/// it and gossip left-slot rivals: showing one replaces the other in both orders, and each
/// displaced window's OnHide ends its own session.
#[test]
fn a_quest_givers_gossip_displaces_the_open_reader() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::GossipMenu;

    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    // The stock gossip window, off the chain.
    common::load_ui(&s, "Interface\\FrameXML\\GossipFrame.xml");

    s.set_item_text(Some(letter()));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    assert!(s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == 'ItemTextFrame'")
            .unwrap(),
        "the registered reader seats at the left slot, not a bare Show"
    );
    let _ = s.take_item_text_close();

    s.set_gossip(Some(GossipMenu {
        greeting: "What can I do for you?".into(),
        quests: Vec::new(),
        options: Vec::new(),
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.take_errors().is_empty());
    assert!(
        !s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap(),
        "gossip replaced the note — the B288 stack (both drawn at 0,-104) cannot form"
    );
    assert!(s
        .eval::<bool>("return GossipFrame:IsShown() and GetLeftFrame():GetName() == 'GossipFrame'")
        .unwrap());
    assert!(
        s.take_item_text_close(),
        "the displaced reader's OnHide ended the read session (CloseItemText)"
    );
    // The app's close must not disturb the gossip that displaced it.
    s.set_item_text(None);
    s.fire_event("ITEM_TEXT_CLOSED", vec![]);
    assert!(s.eval::<bool>("return GossipFrame:IsShown()").unwrap());

    // The reverse order: reading the note over an open gossip ends the gossip session.
    let _ = s.take_gossip_close();
    s.set_item_text(Some(letter()));
    s.fire_event("ITEM_TEXT_BEGIN", vec![]);
    s.fire_event("ITEM_TEXT_READY", vec![]);
    assert!(s.take_errors().is_empty());
    assert!(
        s.eval::<bool>("return ItemTextFrame:IsShown() and not GossipFrame:IsShown()")
            .unwrap(),
        "the reader replaces gossip the same way"
    );
    assert!(
        s.take_gossip_close(),
        "the displaced gossip's OnHide ended the gossip session (CloseGossip)"
    );
}
