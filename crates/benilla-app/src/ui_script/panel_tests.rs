use benilla_ui::script::{
    ExtractedQuad, GossipMenu, GossipOptionView, GossipQuestRow, MerchantItem, MerchantState,
    QuadContent, SoundRequest, UiScript,
};

/// Load one manifest entry into `s`, panicking on any loader error and returning the frame count
/// it materialized — the panel tests all load `Interface\FrameXML\UIParent.xml` (decision 0084 §2's
/// slot manager) before the panel frame(s) under test, exactly as `benilla.toc`'s own order does,
/// so `ShowUIPanel`/`HideUIPanel` already exist when a frame's OnLoad/OnEvent references them.
///
/// This was a private disk-only reader until 1751's second window: `BankFrame.xml` is the
/// reference's own file off the player's chain now, and a reader that only knows `assets/ui`
/// cannot name it. [`super::test_ui::load_ui`] knows both stores by the manifest's own rule, so a
/// caller that names a chain entry has to open with `wow_data_or_skip!` (only the bank test below
/// does).
use super::test_ui::load_ui as load_xml;

/// Find a bare frame's own rect via its `QuadContent::Frame` entry (every frame emits one, at its
/// resolved rect, whether or not it paints anything itself — `UiScript::extract`'s doc). Used for
/// the synthetic pushable=7 loot marker, which has no visual layers of its own.
fn frame_rect(quads: &[ExtractedQuad], w: f32, h: f32) -> benilla_ui::layout::Rect {
    quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Frame => q
                .rect
                .filter(|r| (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bare-frame quad sized {w}x{h}"))
}

/// Load the stock `Interface\FrameXML\GossipFrame.xml` behind `UIParent.xml`
/// into a bare engine and drive it with a synthetic gossip menu — the whole phase-3 chain minus
/// Bevy (decision 0081), now over the UIPanel slot manager (decision 0084): the hidden→shown
/// lifecycle on GOSSIP_SHOW goes through ShowUIPanel (landing the window at the left slot,
/// TOPLEFT UIParent 0,-104 — pin §4's rect assertion), the greeting + option rows rendering, a
/// coded row disabled, a row click queuing the right select intent, and a close through
/// HideUIPanel vacating the left slot. Quest-row rendering (decision 0088 §3, the shared row pool)
/// gets its own dedicated test below (`shipped_gossip_frame_renders_quest_rows_above_options`).
#[test]
fn shipped_gossip_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // A measurer: `GossipResize` reads `GetTextHeight()` on the line after `SetText`, so a bare
    // VM sizes every row from the previous frame's box. See
    // `shipped_gossip_rows_grow_to_their_wrapped_labels` for why this is a harness gap and not an
    // engine one.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // The window + its scroll frame (bar + child) + the 32-row shared pool (quest rows and option
    // rows both draw from it, decision 0088 §3 — the reference's own NUMGOSSIPBUTTONS) + the close
    // button + the GOODBYE button. The greeting and the NPC-name banner are FontString layers (the
    // real GossipGreetingText ref l.241 / GossipFrameNpcNameText ref l.170) — not their own frames.
    assert_eq!(
        load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml"),
        43,
        "the stock file's own shape (1751) — ours materialized 41"
    );

    // Hidden by default: no gossip icon on screen.
    s.resolve();
    let vendor_icon = |quads: &[ExtractedQuad]| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("VendorGossipIcon"))
        })
    };
    assert!(!vendor_icon(&s.extract()), "gossip window starts hidden");
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "left slot empty before any panel opens"
    );

    // The app's feed: a two-option menu (a vendor option + a coded petition option).
    s.set_gossip(Some(GossipMenu {
        greeting: "Greetings, traveler. How may I help you?".into(),
        quests: Vec::new(),
        options: vec![
            GossipOptionView {
                label: "Let me browse your goods.".into(),
                icon_type: "vendor".into(),
                coded: false,
            },
            GossipOptionView {
                label: "I wish to sign the petition.".into(),
                icon_type: "gossip".into(),
                coded: true,
            },
        ],
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The window is shown (ShowUIPanel put it on the left slot), the greeting painted, the vendor
    // row's icon rendered.
    assert!(s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return GossipGreetingText:GetText()")
            .unwrap(),
        "Greetings, traveler. How may I help you?"
    );
    // Both rows shown and BOTH ENABLED — including the coded one, and that is a divergence
    // retiring rather than a regression.
    //
    // Our `GossipFrame.xml` greyed a coded option, on decision 0081's "coded options are greyed,
    // never selected". **1.12 does no such thing**: `GossipFrameOptionsUpdate` (ref l.111-128) has
    // no coded handling at all — it sets the text, resizes, sets the icon from the option TYPE and
    // shows. There is no `Disable()` anywhere in the file.
    //
    // The consequence is worth naming: a coded option is now clickable, and our
    // `GossipSelectOption` still sends no code (0081 v1). The server sees a select without the
    // code it asked for. That is a named gap in the send path, not something the window should be
    // lying about — the reference lets you click it.
    let states: (bool, bool, bool, bool) = s
        .eval(
            "return GossipTitleButton1:IsVisible(), GossipTitleButton1:IsEnabled() ~= 0,\n\
                        GossipTitleButton2:IsEnabled() ~= 0, GossipTitleButton3:IsVisible()",
        )
        .unwrap();
    assert_eq!(
        states,
        (true, true, true, false),
        "the reference greys nothing; extras hidden"
    );

    s.resolve();
    let quads = s.extract();

    // The slot anchor actually applied: the window's rect top-left sits at (0, 664) — screen
    // height 768 minus the left slot's 104px drop (pin §4's extract-rect assertion). The re-skinned
    // window has no solid-colour fill (the parchment quadrants are opaque), so it's found by its own
    // 384×512 frame quad rather than a background texture.
    let win = frame_rect(&quads, 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "gossip window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // The four QuestGreeting quadrant slabs ARE the parchment art (ref-GossipFrame.xml l.13-44):
    // 256-wide left halves, 128-wide right halves, each 256 tall, pinned to their corner, each
    // sampling its whole texture (no TexCoords).
    let quad_rect = |needle: &str, w: f32, h: f32| {
        let q = quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .unwrap_or_else(|| panic!("no texture quad for {needle}"));
        let r = q.rect.unwrap_or_else(|| panic!("no rect for {needle}"));
        assert!(
            (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5,
            "{needle} is {w}×{h}, got {}×{}",
            r.width(),
            r.height()
        );
        r
    };
    let tl = quad_rect("UI-QuestGreeting-TopLeft", 256.0, 256.0);
    assert_eq!(
        (tl.left, tl.top),
        (win.left, win.top),
        "TopLeft quadrant pinned to the window's TOPLEFT"
    );
    let tr = quad_rect("UI-QuestGreeting-TopRight", 128.0, 256.0);
    assert_eq!(
        (tr.right, tr.top),
        (win.right, win.top),
        "TopRight quadrant pinned to the window's TOPRIGHT"
    );
    let bl = quad_rect("UI-QuestGreeting-BotLeft", 256.0, 256.0);
    assert_eq!(
        (bl.left, bl.bottom),
        (win.left, win.bottom),
        "BotLeft quadrant pinned to the window's BOTTOMLEFT"
    );
    let br = quad_rect("UI-QuestGreeting-BotRight", 128.0, 256.0);
    assert_eq!(
        (br.right, br.bottom),
        (win.right, win.bottom),
        "BotRight quadrant pinned to the window's BOTTOMRIGHT"
    );

    let icon_rect = quads
        .iter()
        .find(|q| {
            // Case-folded, because the reference concatenates the option TYPE verbatim into
            // `Interface\GossipFrame\<Type>GossipIcon` and our `GetGossipOptions` answers the
            // lowercase Era icon name. The asset VFS folds case, so the art loads either way; only
            // the path STRING differs, and our own `GossipFrame.xml` used to capitalise it before
            // building the path. Whether 1.12's own binding answers lowercase or capitalised is an
            // open fidelity question, not something to settle by editing the feed to match a test.
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.to_ascii_lowercase().contains("vendorgossipicon"))
        })
        .and_then(|q| q.rect)
        .expect("vendor option icon visible after GOSSIP_SHOW");
    // The label text renders too.
    assert!(
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), .. }
                    if t == "Let me browse your goods.")
        }),
        "option label shows"
    );

    // Click option 1 (the icon center lies inside its row button) → SelectGossipOption(1) queues.
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert_eq!(s.take_gossip_selects(), vec![1]);
    assert!(!s.take_gossip_close());

    // The close button queues a close intent (the app clears state; here we drive the hide too)
    // through HideUIPanel, which vacates the left slot.
    s.run("GossipFrameCloseButton:Click()").unwrap();
    assert!(s.take_gossip_close());
    assert!(!s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "HideUIPanel vacated the left slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Decision 0088 §3's deferred item, now drawn: `GossipMenu.quests` render as rows on the SAME
/// shared pool the option rows already used, quest rows filling first (ref-GossipFrame.lua
/// l.24-29/63-128 — `GossipFrameAvailableQuestsUpdate`/`ActiveQuestsUpdate` before
/// `OptionsUpdate`), option rows landing directly below them via the one static anchor chain
/// (no runtime repositioning). One active quest + one available quest + one option: rows 1/2 carry
/// the quest titles, row 3 the option, row 4+ stay hidden, and clicking a quest row queues the
/// right 1-based position on `take_gossip_quest_selects` (`benilla-ui` `script/gossip.rs`).
#[test]
fn shipped_gossip_frame_renders_quest_rows_above_options() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

    s.set_gossip(Some(GossipMenu {
        greeting: "A word, traveler.".into(),
        quests: vec![
            GossipQuestRow {
                title: "Report to Goldshire".into(),
                level: 5,
                active: true,
            },
            GossipQuestRow {
                title: "A Threat Within".into(),
                level: 7,
                active: false,
            },
        ],
        options: vec![GossipOptionView {
            label: "Let me browse your goods.".into(),
            icon_type: "vendor".into(),
            coded: false,
        }],
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Rows 1-2 carry the quest titles (active first, matching the order the menu supplied them —
    // benilla's seam already flattens available/active into one ordered list, `ui_gossip`), row 3
    // the option, row 4+ hidden.
    let (r1_text, r1_vis, r2_vis, r3_text, r3_vis, r4_vis, r5_text, r5_vis, r6_vis): (
        String,
        bool,
        bool,
        String,
        bool,
        bool,
        String,
        bool,
        bool,
    ) = s
        .eval(
            "return GossipTitleButton1:GetText(), GossipTitleButton1:IsVisible(),\n\
                        GossipTitleButton2:IsVisible(),\n\
                        GossipTitleButton3:GetText(), GossipTitleButton3:IsVisible(),\n\
                        GossipTitleButton4:IsVisible(),\n\
                        GossipTitleButton5:GetText(), GossipTitleButton5:IsVisible(),\n\
                        GossipTitleButton6:IsVisible()",
        )
        .unwrap();
    // **AVAILABLE quests lead, then ACTIVE** — the reference's own order (`GossipFrameUpdate` calls
    // `GossipFrameAvailableQuestsUpdate` before `…ActiveQuestsUpdate`, ref l.27-28), and ours since
    // this window started reading the reference's two list verbs instead of the single ordered
    // `GetGossipQuestInfo` benilla invented. The packet listed the active quest first; the window
    // does not.
    // **Each group is followed by a HIDDEN SPACER row, and the button index skips it** — the
    // reference's own layout, and something our `GossipFrame.xml` did not do. Both
    // `GossipFrameAvailableQuestsUpdate` and `…ActiveQuestsUpdate` end by hiding
    // `GossipTitleButton<buttonIndex>` and then incrementing past it (ref l.80-84, l.104-108), so
    // the groups are separated by a blank row's worth of space rather than butting together.
    //
    // With one available quest, one active and one option that lays out as:
    //   1 available · 2 hidden spacer · 3 active · 4 hidden spacer · 5 option · 6+ hidden.
    assert_eq!((r1_text.as_str(), r1_vis), ("A Threat Within", true));
    assert!(!r2_vis, "the available group's trailing spacer");
    assert_eq!((r3_text.as_str(), r3_vis), ("Report to Goldshire", true));
    assert!(!r4_vis, "the active group's trailing spacer");
    assert_eq!(
        (r5_text.as_str(), r5_vis),
        ("Let me browse your goods.", true),
        "the option row sits below both quest groups"
    );
    assert!(!r6_vis, "nothing past the option row");

    // The per-row icon matches active vs available (ref l.75/99), verified via the resolved quads
    // rather than the Lua-side texture string, so the assertion also proves the rows actually paint.
    s.resolve();
    let quads = s.extract();
    let has_icon = |needle: &str| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
    };
    assert!(
        has_icon("AvailableQuestIcon"),
        "row 1 (available) icon renders"
    );
    assert!(has_icon("ActiveQuestIcon"), "row 2 (active) icon renders");

    // The window's rect places row 3 (the option) strictly below row 2 (the last quest row) — the
    // static anchor chain, not a runtime SetPoint, closes the gap between the two sections. Found
    // by each row's own label text (rects are y-up, per `ExtractedQuad::rect`'s doc — "below" means
    // a smaller `top`), the same technique the existing test already uses for the vendor icon/label.
    let label_top = |needle: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text { text: Some(t), .. } if t == needle => q.rect,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text quad for {needle:?}"))
            .top
    };
    let row2_top = label_top("Report to Goldshire");
    let row3_top = label_top("Let me browse your goods.");
    assert!(
        row3_top < row2_top,
        "row 3 (option) sits below row 2 (last quest row): row2 top {row2_top}, row3 top {row3_top}"
    );

    // Click quest row 2 — the ACTIVE quest "Report to Goldshire", which the packet listed FIRST.
    // The row calls `SelectGossipActiveQuest(1)`: 1 because it is the first row of the active list,
    // which is the index the reference's own `GossipTitleButton_OnClick` passes. The binding maps
    // that back to the whole-menu position the app's queue speaks — 1, the packet's own order. The
    // two numbers being different for the same click is exactly what the single-list
    // `SelectGossipQuest` we retired could not express.
    // Button 3, not 2: the reference's hidden spacer sits at 2 (see the layout note above).
    s.run("GossipTitleButton3:Click()").unwrap();
    assert_eq!(s.take_gossip_quest_selects(), vec![1]);
    assert!(
        s.take_gossip_selects().is_empty(),
        "a quest-row click never queues SelectGossipOption"
    );
}

/// A gossip option whose label WRAPS gets a row as tall as its wrapped text (the reference's
/// `GossipResize` — `SetHeight(GetTextHeight() + 2)`, ref-GossipFrame.lua l.130-132), so the static
/// row chain (each row on the previous row's BOTTOMLEFT) still stacks them clear of one another.
/// Without the resize every row stayed the template's 16 px while its label drew 2-3 wrapped lines,
/// and the labels printed on top of each other — the director's screenshot of a four-option
/// judgement menu, every option overlapping the next.
///
/// The engine-only harness has no font atlas, so the measure round-trip is answered with a
/// deterministic 6 px/char × 14 px/line fake (the app answers from the real atlas in-game). The
/// assertions pin the LAW — row height = its label's measured height + 2, and consecutive rows share
/// an edge — not any glyph metric.
#[test]
fn shipped_gossip_rows_grow_to_their_wrapped_labels() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // **A measurer, because the reference measures inside the tick it sets the text.**
    // `GossipResize` reads `titleButton:GetTextHeight()` on the line after `SetText`
    // (`GossipFrame.lua:130-137`), and without a `TextMeasure` installed a bare VM answers the
    // PREVIOUS frame's box — the row comes out 2px tall.
    //
    // This is what deferred the gossip window through 1751: it was read as "our measurement is a
    // frame late", an engine gap. It is not. `UiScript::fill_measures` closes the loop inline
    // whenever a measurer is installed, which the app always does
    // (`ui_script::extract`'s `AtlasMeasurer`); only a bare test VM does not. The gap was in the
    // harness, and it hid behind a window we had written to measure a frame later.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

    // Three long options — the shape of a real judgement/roleplay menu, every one of them wrapping
    // at the row label's 275 px width.
    let long = |t: &str| GossipOptionView {
        label: t.into(),
        icon_type: "gossip".into(),
        coded: false,
    };
    s.set_gossip(Some(GossipMenu {
        greeting: "Make your choice!".into(),
        quests: Vec::new(),
        options: vec![
            long(
                "I slay the man on the spot as my liege would expect me to, as he has broken the \
                  law of the land and it is my sworn duty to enforce it.",
            ),
            long(
                "I turn over the man to my liege for punishment, as the man has stolen, and I am \
                  not the arbiter of his fate.",
            ),
            long(
                "I allow the man to take enough corn to feed his family for a couple of days, \
                  encouraging him to leave the land.",
            ),
        ],
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The host's job: measure every FontString the frame asks about. 6 px/char, wrapped at the
    // request's own wrap width, 14 px per resulting line.
    let answer_measures = |s: &mut UiScript| {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => {
                        let lines = (ink / w).ceil().max(1.0);
                        (r.id, ink.min(w), lines * 14.0, r.key)
                    }
                    None => (r.id, ink, 14.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    };
    // Frame 1: the labels are measured (their heights land for the NEXT tick — this harness drives
    // the host's BATCH round-trip by hand, which is a frame late by construction; the app's own
    // synchronous measurer closes the same loop inside `resolve`).
    answer_measures(&mut s);
    s.resolve();
    // Frame 2: the frame's own settle pass reads those measures and sizes each row.
    s.tick(0.016);
    answer_measures(&mut s);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let row = |i: u32| -> (f32, f32, f32) {
        s.eval::<(f32, f32, f32)>(&format!(
            "return GossipTitleButton{i}:GetTop(), GossipTitleButton{i}:GetBottom(), \
             GossipTitleButton{i}:GetTextHeight()"
        ))
        .unwrap()
    };
    let (t1, b1, h1) = row(1);
    let (t2, b2, h2) = row(2);
    let (t3, b3, h3) = row(3);
    // Every label wrapped (the fake measure gives ≥ 2 lines of 14) …
    for (i, h) in [(1, h1), (2, h2), (3, h3)] {
        assert!(h >= 28.0, "row {i}'s label wraps: measured height {h}");
    }
    // … and each row is exactly its label + the reference's 2 px.
    for (i, (top, bottom, h)) in [(1, (t1, b1, h1)), (2, (t2, b2, h2)), (3, (t3, b3, h3))] {
        assert!(
            (top - bottom - (h + 2.0)).abs() < 0.5,
            "row {i} height is its wrapped label + 2: got {}, label {h}",
            top - bottom
        );
    }
    // The chain stacks them edge to edge — no row overprints the next (rects are y-up).
    assert!(
        (t2 - b1).abs() < 0.5,
        "row 2 starts where row 1 ends: row1 bottom {b1}, row2 top {t2}"
    );
    assert!(
        (t3 - b2).abs() < 0.5,
        "row 3 starts where row 2 ends: row2 bottom {b2}, row3 top {t3}"
    );
}

/// Pin §4's headline scenario, and §2's whole justification: with both windows loaded, opening
/// gossip then merchant closes gossip purely through panel replacement (both register
/// pushable=0, so the second `ShowUIPanel` replaces the left occupant — UIParent.lua l.729-732) —
/// never a server-side CloseGossip. Merchant's own close then vacates the slot.
/// The gossip window's open/close kits — the window-sound convention (decision 0090). The real
/// GossipFrame.xml frame Scripts play igQuestListOpen on OnShow (l.445) and igQuestListClose on OnHide
/// (l.454); GOSSIP_SHOW → ShowUIPanel → Show() fires OnShow, GOSSIP_CLOSED → HideUIPanel → Hide() fires
/// OnHide. Nothing queues at load (the frame is authored hidden="true").
#[test]
fn gossip_show_hide_plays_open_and_close_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

    // Hidden at load: no open sound (never transitions on startup).
    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );

    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListOpen".into())],
        "opening the gossip window plays igQuestListOpen"
    );

    s.fire_event("GOSSIP_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListClose".into())],
        "closing the gossip window plays igQuestListClose"
    );
}

#[test]
fn shipped_panel_slot_replaces_gossip_with_merchant() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");
    // The vendor window is the reference's own since 1751, and its `MerchantFrame_UpdateMerchantInfo`
    // calls `TEXT()` while building every row — see `test_ui::MERCHANT_UI` for the rest.
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: vec![GossipOptionView {
            label: "Let me browse your goods.".into(),
            icon_type: "vendor".into(),
            coded: false,
        }],
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
        "gossip opened onto the (empty) left slot"
    );
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!((win.left, win.top), (0.0, 664.0));

    // The vendor gossip option is picked; the app's SMSG_LIST_INVENTORY handler opens the
    // merchant window with no server-side gossip close (pin §2) — purely a second ShowUIPanel.
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            // Not this test's subject (the slot manager is) — a row with no template answer yet
            // carries no link (decision 1059).
            link: None,
            max_stack: Some(1),
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Gossip is hidden (both pushable=0 ⇒ SetLeftFrame replaced it); merchant now holds the left
    // slot at the same anchor.
    assert!(
        !s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
        "opening merchant replaced gossip at the left slot"
    );
    assert!(s.eval::<bool>("return MerchantFrame:IsVisible()").unwrap());
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "merchant took the left slot gossip vacated"
    );
    assert!(s
        .eval::<bool>("return GetLeftFrame():GetName() == \"MerchantFrame\"")
        .unwrap());

    // Merchant's own close vacates the slot entirely.
    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "the slot is empty once the replacing window also closes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The cross-window session-clear (decision 0095): when one NPC window displaces another at the left
/// panel slot, the displaced window's OnHide fires its `CloseX()`, queuing the client-side clear
/// intent the app drains to end that session's resource. Without it the displaced session stayed
/// "open" in its resource, and the window would not reopen until the range-guard reset it — the
/// director's gossip↔vendor lockup. This drives the exact displacement and asserts the intents.
#[test]
fn displacing_an_npc_window_ends_the_displaced_session() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                         // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                         // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                         // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                         // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                         // and then loses the scrollbar silently.
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");
    // The vendor window is the reference's own since 1751, and its `MerchantFrame_UpdateMerchantInfo`
    // calls `TEXT()` while building every row — see `test_ui::MERCHANT_UI` for the rest.
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // Vendor open at the left slot; clear any startup/open intents.
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    let _ = s.take_merchant_close();
    let _ = s.take_gossip_close();

    // Gossip opens over it → SetLeftFrame hides the merchant → merchant OnHide → CloseMerchant().
    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_merchant_close(),
        "gossip displacing the vendor ends the vendor session (OnHide → CloseMerchant)"
    );

    // The reverse: the vendor back over gossip → gossip OnHide → CloseGossip().
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_gossip_close(),
        "the vendor displacing gossip ends the gossip session (OnHide → CloseGossip)"
    );
}

/// Pin §4's pushable path: a higher-`pushable` occupant (loot's future pushable=7 row, already
/// registered in `UIParent.xml`) gets pushed to the center slot rather than replaced when a
/// pushable=0 frame (merchant) wants the left spot (UIParent.lua l.734-741) — the synthetic
/// registrant the pin calls for, since no loot window ships yet. A bare `CreateFrame` with a
/// distinctive 50×50 size stands in for it: it has no visual layers, but `extract` still emits
/// its own `QuadContent::Frame` quad at its resolved rect (`UiScript::extract`'s doc), which is
/// enough to prove the slot math without a real loot window.
#[test]
fn shipped_panel_slot_pushable_promotes_to_center() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    // The vendor window is the reference's own since 1751, and its `MerchantFrame_UpdateMerchantInfo`
    // calls `TEXT()` while building every row — see `test_ui::MERCHANT_UI` for the rest.
    for f in super::test_ui::MERCHANT_UI {
        load_xml(&s, f);
    }
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    // The synthetic pushable=7 loot stand-in opens first onto the empty left slot. `CreateFrame`
    // frames start shown by default (matching the real client) — every shipped panel frame is
    // authored `hidden="true"` for exactly this reason (ShowUIPanel no-ops on an already-visible
    // frame), so the stand-in hides itself first too.
    s.run(
        r#"
            local loot = CreateFrame("Frame", "LootFrame")
            loot:Hide()
            loot:SetWidth(50); loot:SetHeight(50)
            ShowUIPanel(loot)
        "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot took the empty left slot"
    );
    s.resolve();
    let loot_left = frame_rect(&s.extract(), 50.0, 50.0);
    assert_eq!((loot_left.left, loot_left.top), (0.0, 664.0));

    // Merchant (pushable=0) then opens: loot's pushable=7 outranks it, so loot is pushed to the
    // center slot (384, -104) and merchant takes the left spot loot vacated.
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            // Not this test's subject (the slot manager is) — a row with no template answer yet
            // carries no link (decision 1059).
            link: None,
            max_stack: Some(1),
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot was pushed to center, not replaced"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"MerchantFrame\"")
            .unwrap(),
        "merchant took the left slot loot vacated"
    );
    s.resolve();
    let quads = s.extract();
    let loot_center = frame_rect(&quads, 50.0, 50.0);
    assert_eq!(
        (loot_center.left, loot_center.top),
        (384.0, 664.0),
        "loot moved to the center slot (TOPLEFT UIParent, 384, -104)"
    );
    let merchant_left = frame_rect(&quads, 384.0, 512.0);
    assert_eq!(
        (merchant_left.left, merchant_left.top),
        (0.0, 664.0),
        "merchant landed on the left slot loot vacated"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The gossip → bank handoff (decision 0604 follow-up, director-observed): vmangos sends no
/// `SMSG_GOSSIP_COMPLETE` for the gossip menu's bank option, and the slot logic alone can't close
/// the menu — the bank's `pushable = 6` promotes it to the *center* slot beside a pushable-0
/// gossip instead of replacing it. The app's `show_bank` therefore ends the gossip session itself,
/// so BANKFRAME_OPENED and GOSSIP_CLOSED fire in the *same frame* — in either order, since the
/// two feeds aren't ordered against each other. Both orders must converge on the same end state:
/// gossip hidden, the bank holding the LEFT slot (HideUIPanel's left-vacate branch slides a
/// center-parked "left-area" occupant back — UIParent.lua l.777-782).
#[test]
fn gossip_bank_option_hands_the_left_slot_to_the_bank() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::BankState;

    // The bank window is the reference's own file off the player's chain (1751).
    let _data = benilla_formats::wow_data_or_skip!();

    // The bank window's own dependency chain (bank_tests::setup), plus the gossip window.
    let order_first = |bank_first: bool| {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
        load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml"); // the reference bank's slot buttons inherit it
        load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
        load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
        load_xml(&s, r"Interface\FrameXML\UIParent.xml");
        load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
        load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
        load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
        load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
        load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
        load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
        load_xml(&s, "ScrollTemplates.xml"); // our file: the two scroll kits + the placeholder icon
                                             // (BenillaScroll_Step is only this suite's pan driver). The window's own
                                             // scroll frame inherits the reference's UIPanelScrollFrameTemplate, which
                                             // lives in UIPanelTemplates.xml (next) — NOT optional: a missing template is
                                             // a loader *warning*, not an error, so an under-loaded list passes load_xml
                                             // and then loses the scrollbar silently.
                                             // Before BankFrame, not after: its close and purchase buttons inherit UIPanelCloseButton
                                             // and UIPanelButtonTemplate, and an `inherits=` is resolved at LOAD.
        load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
        load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
        load_xml(&s, "Interface\\FrameXML\\BankFrame.xml");
        load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

        // The gossip menu is open on the banker (its bank option showing).
        s.set_gossip(Some(GossipMenu {
            greeting: "Welcome to the bank of Ironforge!".into(),
            quests: Vec::new(),
            options: vec![GossipOptionView {
                label: "I would like to check my deposit box.".into(),
                icon_type: "money".into(),
                coded: false,
            }],
        }));
        s.fire_event("GOSSIP_SHOW", vec![]);
        assert!(s
            .eval::<bool>("return GetLeftFrame():GetName() == \"GossipFrame\"")
            .unwrap());

        // The option is picked; SMSG_SHOW_BANK lands and the app both opens the bank AND clears
        // the gossip session in the same apply pass — the two events fire the same frame, in
        // whichever order the feeds run.
        s.set_money(0);
        s.set_bank(Some(BankState::default()));
        s.set_gossip(None);
        if bank_first {
            s.fire_event("BANKFRAME_OPENED", vec![]);
            s.fire_event("GOSSIP_CLOSED", vec![]);
        } else {
            s.fire_event("GOSSIP_CLOSED", vec![]);
            s.fire_event("BANKFRAME_OPENED", vec![]);
        }
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

        assert!(
            !s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
            "the gossip menu is gone once the vault is up (bank_first={bank_first})"
        );
        assert!(s.eval::<bool>("return BankFrame:IsVisible()").unwrap());
        assert!(
            s.eval::<bool>("return GetLeftFrame():GetName() == \"BankFrame\"")
                .unwrap(),
            "the bank ends at the LEFT slot, not parked at center (bank_first={bank_first})"
        );
        assert!(
            s.eval::<bool>("return GetCenterFrame() == nil").unwrap(),
            "the center slot is empty again (bank_first={bank_first})"
        );
    };
    order_first(true);
    order_first(false);
}

/// A menu too tall for the parchment SCROLLS instead of spilling out of the window — the reference's
/// own answer (`GossipGreetingScrollFrame`, ref-GossipFrame.xml l.223-436), which benilla had omitted
/// on the grounds that "our short greeting needs no scrolling". Once the rows grew to their wrapped
/// text, a menu of long options ran straight off the bottom of the window and over the world (the
/// director's second screenshot).
///
/// Pins the whole mechanism: the content is measured into the scroll child, the range is the overflow,
/// the bar appears only when there IS overflow, every row is CLIPPED to the frame's rect, and
/// scrolling moves the content up under that clip. Same deterministic measure fake as the row test.
#[test]
fn an_overflowing_gossip_menu_scrolls_instead_of_spilling() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // A measurer: `GossipResize` reads `GetTextHeight()` on the line after `SetText`, so a bare
    // VM sizes every row from the previous frame's box. See
    // `shipped_gossip_rows_grow_to_their_wrapped_labels` for why this is a harness gap and not an
    // engine one.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml"); // UIPanelScrollFrameTemplate — see the note above
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

    // Eight wrapping options: ~4 lines each, far past the 334 px scroll frame.
    let long = |n: usize| GossipOptionView {
        label: format!(
            "Option {n}: I slay the man on the spot as my liege would expect me to, as he has \
             broken the law of the land and it is my sworn duty to enforce it, whatever the cost."
        ),
        icon_type: "gossip".into(),
        coded: false,
    };
    s.set_gossip(Some(GossipMenu {
        greeting: "Make your choice!".into(),
        quests: Vec::new(),
        options: (1..=8).map(long).collect(),
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);

    let answer_measures = |s: &mut UiScript| {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => (r.id, ink.min(w), (ink / w).ceil().max(1.0) * 14.0, r.key),
                    None => (r.id, ink, 14.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    };
    // Settle: measures land, rows fit, the child is sized to them, the bar re-ranges.
    for _ in 0..3 {
        answer_measures(&mut s);
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // There is a range to scroll — which is the property that matters and the one the window
    // exists to have.
    //
    // The child's own HEIGHT is no longer part of the assertion. Ours grew it past the 334px frame
    // in `BenillaGossipFrame_SizeScrollChild`, a helper the reference does not have: stock leaves
    // the scroll child at the frame's size and lets the rows' own extents drive the range. So the
    // child measures 334 and the range is non-zero, where ours had both. Asserting the range alone
    // is asserting the behaviour rather than one implementation's way of reaching it.
    let (child_h, range): (f32, f32) = s
        .eval(
            "return GossipGreetingScrollChildFrame:GetHeight(), \
             GossipGreetingScrollFrame:GetVerticalScrollRange()",
        )
        .unwrap();
    assert!(
        range > 0.0,
        "the overflowing menu has somewhere to scroll: child {child_h}, range {range}"
    );
    // … and the bar is up for it (it stays hidden when everything fits — the fit case is covered by
    // `shipped_gossip_frame_drives_end_to_end`'s two-option menu).
    assert!(
        s.eval::<bool>("return GossipGreetingScrollFrameScrollBar:IsVisible()")
            .unwrap(),
        "the scrollbar shows once the menu overflows"
    );

    // Every row is CLIPPED to the scroll frame — the mechanism that replaces "spilling out of the
    // window". The engine clips by carrying a clip rect on the quad (decision 0112 §4/§5, applied in
    // `ui_pass`), not by shrinking its rect, so the check is on the clip each row's text quad rides
    // out with: it must be the scroll frame's own rect, and rows past the bottom must be entirely
    // outside it (nothing of them survives the clip).
    let (frame_top, frame_bottom): (f32, f32) = s
        .eval("return GossipGreetingScrollFrame:GetTop(), GossipGreetingScrollFrame:GetBottom()")
        .unwrap();
    let painted_rows =
        |s: &mut UiScript| -> Vec<(benilla_ui::layout::Rect, benilla_ui::layout::Rect)> {
            s.extract()
                .into_iter()
                .filter_map(|q| match &q.content {
                    QuadContent::Text { text: Some(t), .. } if t.starts_with("Option ") => {
                        Some((q.rect?, q.clip?))
                    }
                    _ => None,
                })
                .collect()
        };
    let rows = painted_rows(&mut s);
    assert_eq!(rows.len(), 8, "all eight option labels extract");
    for (rect, clip) in &rows {
        assert!(
            (clip.top - frame_top).abs() < 0.5 && (clip.bottom - frame_bottom).abs() < 0.5,
            "row clipped to the scroll frame [{frame_bottom}, {frame_top}], got {clip:?}"
        );
        let _ = rect;
    }
    // And the menu really is longer than the window: at least one row sits entirely below the
    // frame's bottom edge (drawn nowhere, because the clip discards it) rather than over the world.
    assert!(
        rows.iter().any(|(rect, _)| rect.top < frame_bottom),
        "the menu overflows: some row is entirely below the frame"
    );

    // Scrolling pans the content up under that clip by exactly the scroll amount. Rects are y-up
    // (`ExtractedQuad::rect`), so "up" means row 1's top EDGE VALUE grows as it slides off the top.
    let first_top =
        |s: &mut UiScript| -> f32 { s.eval::<f32>("return GossipTitleButton1:GetTop()").unwrap() };
    let before = first_top(&mut s);
    s.run("BenillaScroll_Step(GossipGreetingScrollFrame, 100)")
        .unwrap();
    s.resolve();
    let after = first_top(&mut s);
    assert!(
        (after - before - 100.0).abs() < 0.5,
        "scrolling 100px lifts the content 100px: {before} → {after}"
    );
    // The bar followed the scroll (SyncBar off OnVerticalScroll), and the rows are still clipped.
    assert_eq!(
        s.eval::<f32>("return GossipGreetingScrollFrameScrollBar:GetValue()")
            .unwrap(),
        100.0,
        "the bar seats at the scroll offset"
    );
    for (_, clip) in painted_rows(&mut s) {
        assert!(
            (clip.top - frame_top).abs() < 0.5 && (clip.bottom - frame_bottom).abs() < 0.5,
            "a scrolled row still clips to the frame, got {clip:?}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── the panel manager as an ADDON reaches it (1206's audit, finished) ─────────────────────────

/// **An addon's own window, registered in `UIPanelWindows`, gets the left slot** — the same slot,
/// at the same anchor, as one of ours.
///
/// `UIPanelWindows["MyFrame"] = { area = "left", pushable = 1 }` is the 1.12 way an addon asks to
/// be a panel rather than a floating box, and **20 corpus addons write that line**. The table has
/// been present since the manager landed, so nothing would have raised — but a table an addon can
/// write to and a manager that honours what it wrote are different claims, and only the second one
/// is worth anything (1203's lesson: a capability absent without a failure is the quiet kind).
///
/// The assertion is deliberately the *same* rect the shipped panels are asserted at
/// (`(0.0, 664.0)` for a 384x512 at 1024x768), so an addon's window is not merely "somewhere" —
/// it is where a client window would be.
#[test]
fn an_addons_own_frame_registered_in_uipanelwindows_takes_the_left_slot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml"); // UIPanelScrollFrameTemplate — see the note above
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GossipFrame.xml");

    // The addon's three lines, in the order an addon writes them.
    s.run(
        r#"AddonPanel = CreateFrame("Frame", "AddonPanel", UIParent)
           AddonPanel:SetWidth(384) AddonPanel:SetHeight(512)
           AddonPanel:Hide()
           UIPanelWindows["AddonPanel"] = { area = "left", pushable = 1 }
           ShowUIPanel(AddonPanel)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return AddonPanel:IsVisible()").unwrap(),
        "ShowUIPanel showed the addon's frame"
    );
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "the addon's panel is at the left slot, where a client panel would be"
    );

    // ...and it takes part in the pushable arbitration like one of ours. The reference's own
    // branch (UIParent.lua l.725-741): left is the addon at pushable=1, nothing in center, and a
    // pushable=0 window arrives — `leftInfo.pushable > info.pushable`, so the addon's frame is
    // MOVED TO CENTER and the client window takes left. Both stay visible.
    //
    // Worth spelling out because the obvious expectation is the opposite one, and it is what this
    // test asserted first: "a pushable=0 window replaces what is holding the slot" is only true
    // when BOTH are pushable=0 (l.728-731). Getting this wrong for an addon means either losing
    // its window or leaving two panels on top of each other.
    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return AddonPanel:IsVisible()").unwrap(),
        "the addon's pushable=1 panel was PUSHED, not closed"
    );
    s.resolve();
    assert!(
        s.extract().iter().any(|q| q
            .rect
            .is_some_and(|r| (r.left - 384.0).abs() < 0.5 && (r.top - 664.0).abs() < 0.5)),
        "and it is at the center slot (UIParent TOPLEFT +384, -104)"
    );

    // HideUIPanel on an addon frame that is not currently in a slot must be a no-op, not an error.
    s.run("HideUIPanel(AddonPanel)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── the registry pinned to the bytes (decision 1507) ──────────────────────────────────────────

/// **The 1507 rows read exactly as the reference wrote them.** The registry is DATA — a different
/// number is a different window order — and 1507 exists because one row drifted unnoticed for
/// months (CharacterFrame carried pushable=0 labelled as "the ref's own row"; the bytes say
/// `pushable = 2, whileDead = 1`, UIParent.lua l.19) while another was simply missing
/// (ItemTextFrame, l.20 — bug B288: the reader opened through ShowUIPanel's unregistered bare-Show
/// branch, so gossip seated itself straight over the open note). A drive-by edit re-introducing
/// either now fails a named test instead of waiting for a player with a Verdant Note.
#[test]
fn the_1507_registry_rows_match_the_reference_bytes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    for probe in [
        // ItemTextFrame — UIParent.lua l.20 (the B288 row).
        "UIPanelWindows['ItemTextFrame'].area == 'left'",
        "UIPanelWindows['ItemTextFrame'].pushable == 0",
        "UIPanelWindows['ItemTextFrame'].whileDead == nil",
        // CharacterFrame — UIParent.lua l.19: pushable 2 (slides beside an NPC session), not 0.
        "UIPanelWindows['CharacterFrame'].pushable == 2",
        "UIPanelWindows['CharacterFrame'].whileDead == 1",
        // The whileDead flags the ref authors and 1507 carried (l.21, l.25, Blizzard_TalentUI:71).
        "UIPanelWindows['SpellBookFrame'].whileDead == 1",
        "UIPanelWindows['QuestLogFrame'].whileDead == 1",
        // UIChildWindows — UIParent.lua l.44-50 verbatim, all four shipped.
        // `TalentFrame`'s row is `Blizzard_TalentUI.lua:71`'s, registered when that addon loads
        // (1988 retired our copy of it, which the reference does not keep in UIParent.lua).
        "table.getn(UIChildWindows) == 4",
        "UIChildWindows[1] == 'OpenMailFrame'",
        "UIChildWindows[2] == 'GuildControlPopupFrame'",
        "UIChildWindows[3] == 'GuildMemberDetailFrame'",
        "UIChildWindows[4] == 'GuildInfoFrame'",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {probe}")).unwrap(),
            "registry drifted from the bytes: {probe}"
        );
    }
}

/// **A dead player opens only the windows whose row asked for it** — ShowUIPanel's own guard
/// (UIParent.lua l.663-666: `UnitIsDead("player") and not info.whileDead` refuses the open),
/// live now that `UnitIsDead` is a real unit binding. The refused window must not show AND must
/// not take a slot; a `whileDead = 1` row (the quest log, l.25) opens exactly as alive.
#[test]
fn a_dead_player_opens_only_whiledead_windows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            max_health: 100,
            health: 0,
            dead: true,
            ..Default::default()
        }),
    );

    // Stand-ins for two shipped rows: GossipFrame (no whileDead) and QuestLogFrame (whileDead=1).
    // Bare frames are enough — the guard runs before any seat is chosen.
    s.run(
        r#"local g = CreateFrame("Frame", "GossipFrame") g:SetWidth(50); g:SetHeight(50) g:Hide()
           local q = CreateFrame("Frame", "QuestLogFrame") q:SetWidth(50); q:SetHeight(50) q:Hide()
           ShowUIPanel(GossipFrame)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return GossipFrame:IsShown()").unwrap(),
        "a row without whileDead is refused while dead"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "the refused window took no slot"
    );
    // ...and the refusal is heard: NotWhileDeadError (the binary's 0x48d340 — push 0x7e) queued
    // the catalog row's key for the app to resolve and toast.
    assert_eq!(
        s.take_ui_errors(),
        vec!["ERR_PLAYER_DEAD"],
        "the dead refusal queued its error key"
    );

    s.run("ShowUIPanel(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestLogFrame:IsShown()").unwrap(),
        "a whileDead = 1 row opens exactly as alive"
    );
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == 'QuestLogFrame'")
            .unwrap(),
        "and it seats normally at the left slot"
    );
    assert!(
        s.take_ui_errors().is_empty(),
        "an admitted open raises no error"
    );
}

/// **A frame ARRIVING at the center seat puts the child windows away; a frame PUSHED there does
/// not** — SetCenterFrame's `UIChildWindows` hide loop (UIParent.lua l.839-846), carried since
/// 1507, and the ref's exact asymmetry: MovePanelToCenter re-seats by direct assignment after a
/// `SetCenterFrame(nil)`, so the slide never runs the loop. The open letter (OpenMailFrame) is
/// the shipped tenant; a bare stand-in proves the slot math without the mail window's whole
/// dependency chain.
#[test]
fn a_frame_arriving_at_center_puts_the_child_windows_away() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");

    // The letter is open; two shipped-row stand-ins take the seats: MerchantFrame (pushable 0)
    // holds left, TradeFrame (pushable 1) then ARRIVES at center (UIParent.lua l.734-741's
    // else-arm: the incumbent outranks nobody, the newcomer settles at center).
    s.run(
        r#"local m = CreateFrame("Frame", "OpenMailFrame") m:SetWidth(50); m:SetHeight(50)
           local a = CreateFrame("Frame", "MerchantFrame") a:SetWidth(50); a:SetHeight(50) a:Hide()
           local b = CreateFrame("Frame", "TradeFrame") b:SetWidth(50); b:SetHeight(50) b:Hide()
           ShowUIPanel(MerchantFrame)
           ShowUIPanel(TradeFrame)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == 'TradeFrame'")
            .unwrap(),
        "the pushable=1 newcomer arrived at the center seat"
    );
    assert!(
        !s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "the arriving center frame hid the open letter (the UIChildWindows loop)"
    );

    // The asymmetry: seats cleared, letter re-shown, LootFrame (pushable 7) holds left and a
    // pushable=0 window shoves it — MovePanelToCenter SLIDES loot across, and the letter stays.
    s.run(
        r#"HideUIPanel(TradeFrame) HideUIPanel(MerchantFrame)
           OpenMailFrame:Show()
           local l = CreateFrame("Frame", "LootFrame") l:SetWidth(50); l:SetHeight(50) l:Hide()
           ShowUIPanel(LootFrame)
           ShowUIPanel(MerchantFrame)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == 'LootFrame'")
            .unwrap(),
        "loot slid to center (MovePanelToCenter), merchant took left"
    );
    assert!(
        s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "a PUSHED frame does not run the child-window loop — the ref's exact trigger"
    );
}
