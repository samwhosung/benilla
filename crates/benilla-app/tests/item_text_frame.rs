//! Drives the REAL reader window through the engine — bag letters (mail-made permanent copies)
//! and, later, books. Loads the same file chain the app does (cut to the reader's dependency
//! prefix), pushes an `ItemTextState`, fires the reference event flow `ITEM_TEXT_BEGIN` →
//! `ITEM_TEXT_READY` → `ITEM_TEXT_CLOSED`, and asserts the Lua actually paints (title, the "From,"
//! creator tail, page-button visibility, the close intent).
//!
//! The window itself is `Interface\FrameXML\ItemTextFrame.xml`, off the player's own patch chain
//! since 1751's ninth window — so every test here gates on the install, as every chain-backed
//! test does.

use benilla_ui::script::{ItemTextState, UiScript};

#[path = "common/mod.rs"]
mod common;

const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ui");

/// The reader's dependency prefix, in the manifest's own order. `ScrollTemplates.xml` and
/// `UIPanelTemplates.xml` joined it with decisions 1337/1338: the page sits in a real ScrollFrame
/// now, whose template is in the second and whose `ScrollFrame_OnLoad` is in the first.
const FILES: &[&str] = &[
    // `ITEM_TEXT_FROM`, which the reference's READY arm concatenates into the creator tail — and
    // `attempt to concatenate a nil value` kills the handler before it reaches its `ShowUIPanel`,
    // so the window simply never opens. Our deleted copy carried the string as a local fallback.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    // `GetMaterialTextColors`, which the reference's own `ItemTextFrame_OnEvent` calls to pick the
    // page and title ink. 1.12 keeps it in UIParent.lua and ours does the same (1751 window 9).
    "ScrollTemplates.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml", // the dialog engine (1960)
    "Interface\\FrameXML\\ItemTextFrame.xml",
];

/// **Both stores, told apart the manifest's own way** — a bare filename is a file we ship, a path
/// is the reference's own off the player's install (`ui_script::reference_ui::is_chain_entry`,
/// re-expressed here because that module is crate-private and this is an integration test).
///
/// The provider half matters as much as the loop: the reference's `ItemTextFrame.xml` pulls its
/// Lua through `<Script file="ItemTextFrame.lua"/>`, which the loader resolves against the
/// document's own directory — `Interface\FrameXML\ItemTextFrame.lua`, a chain path — so a
/// disk-only provider would leave every one of its globals nil and every test below red for a
/// reason that looks nothing like the cause.
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
        // A `.lua` entry is a chunk, not a document — `GlobalStrings.lua` is one, exactly as it is
        // in the reference's own TOC. Bytes, not text: a chunk goes to Lua as it sits in the
        // archive and only an XML parse decodes (1193).
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

/// The page body as the reader actually DRAWS it, one string per block.
///
/// `ItemTextPageText` is a `SimpleHTML` since decisions 1337/1338, and 5875's SimpleHTML has no
/// `GetText` — its Lua table is 19 entries and none of them is a text getter (`0x87ba80`; later
/// clients grew one, this one has not). So the page is read the way it is seen: off the render
/// list. A plain body is one block through the engine's raw-text fallback, which is what every
/// letter here is.
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

/// The letter flow: BEGIN paints the title, READY paints the body with the reference "From," tail
/// and shows the window; a single page shows no page number and no page-turn buttons.
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

/// The scrollbar track (the ref's black `$parentMiddle` strip) belongs in the scrollbar column,
/// right of the page — regression for the black bar over the parchment: the ref declares the
/// ARTWORK layer before BACKGROUND because `Middle` anchors to `Top` by name and anchors resolve
/// at SetPoint time; a reordered transcription silently fell back to the parent.
///
/// **The tripwire beside it is the whole stock chain's, and it moved with decision 2176.** An
/// unresolvable `relativeTo` used to anchor to the parent and warn; a *Lua* one now raises
/// (`0x87ccd4`) and an *XML* one is reported and skipped the way `0x767800` reports and skips it
/// (`"Couldn't find relative frame: %s"`, `0x878440`). So the assertion is on the new spelling —
/// zero of those over every file the player's own chain loads — and the raise's own tripwire is
/// `take_errors()` at the end, which a raise inside any `OnLoad` would land in.
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

/// An authorless multi-page text (a book): no "From," tail, the page number and the page-turn
/// buttons show per the reference visibility rules (page 1 with a next → Next only).
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

/// Closing: the X hides the panel, the OnHide queues the `CloseItemText` intent (the app then
/// clears the session and fires `ITEM_TEXT_CLOSED` — which hides again, harmlessly).
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

    // The app's answer: session cleared + ITEM_TEXT_CLOSED (stays hidden, no errors).
    s.set_item_text(None);
    s.fire_event("ITEM_TEXT_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return ItemTextFrame:IsShown()").unwrap());
    assert!(s.take_errors().is_empty());
}

/// **B240's render half, on the reported page.** The plaque body (`page_text` 2676, the
/// *Alliance Military Ranks* wall plaque in Stormwind's Old Town) went through the reader and came
/// out as its own source — `<HTML><BODY><H1 align="center">…` drawn literally, and cut off with
/// "..." partway down. Both were the page being a plain FontString where the reference has a
/// `SimpleHTML` (decisions 1337/1338).
///
/// What this pins is the whole path the report exercises: the app's page feed → the reader's
/// `"\n" .. body .. "\n"` padding → the markup parse → the drawn blocks. The falsification is
/// stated at the bottom: if the parse ever regresses to the raw-text fallback, the page draws as
/// one block that still has angle brackets in it, which is exactly what was reported.
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
    // The reported symptom, stated as the thing that must not come back.
    assert!(
        !drawn
            .iter()
            .any(|b| b.contains("<HTML>") || b.contains("<P align")),
        "the markup itself must never reach the page — that is what was photographed: {drawn:?}"
    );
    // And the second half of the same symptom: the body is no longer a height-pinned FontString,
    // so decision 1332's ellipsis seam cannot cut a long page short. No block ends in the
    // truncation marker.
    assert!(
        !drawn.iter().any(|b| b.ends_with("...")),
        "a block was truncated — the page is height-pinned again: {drawn:?}"
    );
}

/// **B342, on the reported page, through the real archives.** The symptom: the Alliance crest
/// on *A Treatise on Military Ranks* draws several times the reference's size, with the page's own
/// text over it.
///
/// The body is `page_text` 2654, quoted verbatim below, and its one `<IMG>` carries **no `width=`
/// and no `height=`**. In the reference that is the CONTENT-derived span: the resolver's size call
/// is virtual, and `CSimpleTexture`'s override answers an authored `0.0` with the loaded texture's
/// texel extent, one texel to one FrameXML unit (`0x770720`, decision 1349).
/// `Interface\PvPRankBadges\PvPRankAlliance` is a 128×128 BLP, so the crest is a 128-unit
/// square inside a 270-wide page.
///
/// The engine half is pinned in `benilla-ui`'s own `simplehtml` tests against a stub oracle; what
/// this adds is the two ends only the app has — the **real** reader XML the report was
/// photographed against, and the **real** file, measured by the same decoder the renderer draws
/// with. It skips on a machine with no client data, like every other archive-backed sweep.
#[test]
fn the_reported_book_crest_draws_at_the_blps_own_size() {
    let _data = benilla_formats::wow_data_or_skip!();
    let data = benilla_formats::wow_data_or_skip!();
    let chain = std::sync::Mutex::new(benilla_formats::open_chain(&data).expect("open chain"));

    let mut s = UiScript::new().unwrap();
    // The host oracle, wired exactly as `ui_script::lifecycle::install_addon_asset_resolvers` wires the
    // live one: the same decoder, so the size the layout resolves with is the size the screen shows.
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
    // The reported symptom, stated as the thing that must not come back: the page is 270 units
    // wide, and the crest used to be stretched across all of it with the text over the top.
    assert!(
        r.right - r.left < 270.0,
        "the crest spans the whole page again — that is the photograph"
    );

    // **The adjacent state**: page 2 of the same book (`page_text` 2655) is five unsized `<IMG>`s,
    // one rank badge per officer rank, and every one of them is a 32x32 BLP. Under the reported
    // defect all five were page-wide slabs stacked over each other's text; each is now its own
    // 32-unit square. (What separates them vertically is the `<BR/>`/`<P>` blocks between them —
    // a floated image reserves nothing — so their spacing is the font engine's business and this VM
    // has none: every text block measures zero here and the five land on one line. The SIZES are
    // the claim.)
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

/// **B288, closed at the reported symptom** (decision 1507): the Verdant Note open from
/// the bag, then a quest giver's gossip — both frames drew at the same TOPLEFT 0,-104 anchor,
/// page text and greeting interleaved. The cause was the reader's missing `UIPanelWindows` row:
/// registered (the ref's own `{ area = "left", pushable = 0 }`, UIParent.lua l.20), the two are
/// left-slot rivals and showing one replaces the other — in BOTH orders, each displaced window's
/// OnHide ending its own session exactly as the merchant/gossip pair already does.
#[test]
fn a_quest_givers_gossip_displaces_the_open_reader() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::GossipMenu;

    let mut s = UiScript::new().unwrap();
    load_ui(&s);
    // The other rival — the reference's own window since 1751, so it goes through the shared
    // both-stores loader rather than a disk read off `assets/ui`. A hand-rolled reader here is
    // exactly what `tests/common` exists to replace: it broke the moment this file became the
    // player's own, and a disk-only provider would leave `GossipFrame.lua`'s globals nil besides.
    common::load_ui(&s, "Interface\\FrameXML\\GossipFrame.xml");

    // The note is open and holding the left slot.
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

    // The quest giver's gossip opens: one window, not two — the reported stack cannot form.
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
    // The app's answer to that close must not disturb the gossip that displaced it.
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
