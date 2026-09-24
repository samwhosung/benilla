//! **The bottom-of-screen clearance law, enforced**.
//!
//! The bottom band of the screen is shared: the main bar sits on it, the extra action bars, the
//! stance/pet bar and the reputation watch bar stack above it, and the bag windows, the cast bar,
//! the chat panes and the default tooltip anchor all have to keep clear of whatever is standing.
//! `UIParent_ManageFramePositions()` (UIParent.xml) is the ONE thing that decides that — it seats
//! the managed frames and writes the managed globals — and every one of the defects below was a
//! frame that quietly opted out of it.
//!
//! Three guards, each pinning a different way to opt out:
//!
//! 1. [`no_shipped_file_declares_its_own_copy_of_a_managed_offset`] — the *shadow copy*.
//!    `BagFrame.xml` kept `local BENILLA_CONTAINER_OFFSET_X/Y = 0/70`. Locals, so the pass's
//!    writes could never reach them; the open-bag stack sat at the no-bars corner forever and a
//!    raised bottom multibar drew straight through the lowest bag window (director-caught — the
//!    screenshot that opened 1499). `GameTooltip.xml` carried the softer `X = X or 0` form of the
//!    same thing. A value with two written statements has no owner.
//!
//! 2. [`every_bottom_anchored_top_level_frame_is_accounted_for`] — the *new frame nobody wired
//!    up*. This is the guard that has to hold for features that do not exist yet: a top-level
//!    frame anchored to the screen's bottom edge is, by construction, in the contested band, so it
//!    must be a managed row, a listener consumer, or an exemption someone wrote a reason for.
//!
//! 3. [`no_bottom_band_frame_overlaps_a_raised_bar`] — the *symptom itself*, over the real shipped
//!    XML: for every combination of raised bars, no bottom-band frame's rect may intersect a
//!    visible bar's. This is the one that would have failed on the director's screenshot.

use benilla_ui::script::UiScript;

/// The globals `UIPARENT_MANAGED_FRAME_POSITIONS`'s `isVar` rows own. Only `UIParent.xml` may
/// write these; everyone else reads them fresh at use (falling back inline, never by assigning).
const MANAGED_GLOBALS: &[&str] = &[
    "CONTAINER_OFFSET_X",
    "CONTAINER_OFFSET_Y",
    "PETACTIONBAR_XPOS",
    "PETACTIONBAR_YPOS",
    "BATTLEFIELD_TAB_OFFSET_Y",
];

fn ui_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui")
}

/// Blank out `<!-- … -->` regions, keeping every newline so line numbers still point at the file.
/// The scan below reads Lua, and an XML comment is prose: `BagFrame.xml`'s own explanation of the
/// backpack anchor says "CONTAINER_OFFSET_X=0 / CONTAINER_OFFSET_Y=70" in English, which is a
/// description of the law, not a violation of it.
fn without_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find("-->").map(|e| e + 3).unwrap_or(after.len());
        for ch in after[..end].chars() {
            if ch == '\n' {
                out.push('\n');
            }
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn shipped_xml() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(ui_dir())
        .expect("assets/ui")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "xml"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).expect("read"),
            )
        })
        .collect();
    out.sort();
    // A sanity floor for the walk, not a census: `assets/ui` retires file by file (1751), so
    // the floor sits well under the count rather than one step above it (1956).
    assert!(out.len() >= 6, "only {} xml files swept", out.len());
    out
}

/// **Nothing outside `UIParent.xml` may ASSIGN a managed offset** — not as a local, not as a
/// global, not as an `or`-guarded default.
///
/// Reading one is fine and expected (`-CONTAINER_OFFSET_X` in the right stack, `CONTAINER_OFFSET_Y`
/// in the bag stack and the tooltip anchor). Falling back at the point of use is fine
/// (`local offsetY = CONTAINER_OFFSET_Y or 70`) — that is a read with a default, and it is what the
/// per-window harnesses that ship no `UIParent.xml` need. What is forbidden is *storing* the value
/// anywhere the pass cannot reach, because the pass recomputes these on every bar change and a
/// stored copy is stale from the moment the next bar goes up.
///
/// The rule is spelled as "an assignment statement whose left-hand side names a managed global,
/// or a name containing one" — the second half is what catches `BENILLA_CONTAINER_OFFSET_Y`, which
/// was not the global's name at all and is exactly why nothing noticed it for so long.
#[test]
fn no_shipped_file_declares_its_own_copy_of_a_managed_offset() {
    let mut offences = Vec::new();
    for (name, text) in shipped_xml() {
        if name == r"Interface\FrameXML\UIParent.xml" {
            continue; // the owner: its var rows are where these numbers are defined
        }
        for (n, line) in without_xml_comments(&text).lines().enumerate() {
            let code = line.split("--").next().unwrap_or("");
            let Some((lhs, _)) = code.split_once('=') else {
                continue;
            };
            // `==`, `<=`, `>=`, `~=` are comparisons, not assignments.
            if code[lhs.len() + 1..].starts_with('=')
                || lhs.ends_with(['<', '>', '~', '=', '!'])
                || lhs.contains("--")
            {
                continue;
            }
            let lhs = lhs.trim().trim_start_matches("local ").trim();
            if MANAGED_GLOBALS.iter().any(|g| lhs.contains(g)) {
                offences.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offences.is_empty(),
        "a managed offset is owned by UIParent.xml's manage pass and may only be READ elsewhere \
         (fall back inline at the point of use — `local y = CONTAINER_OFFSET_Y or 70` — never by \
         assigning a copy, which goes stale the moment a bar is raised; decision 1499):\n{}",
        offences.join("\n")
    );
}

/// Frames whose seat the manage pass owns — the `UIPARENT_MANAGED_FRAME_POSITIONS` keys that name
/// a frame, plus the two the pass's custom tail seats by hand. Kept here as literal text so this
/// test and `UIParent.xml` cannot drift apart silently: the next guard asserts every one of these
/// really appears in the shipped pass.
const MANAGED_FRAMES: &[&str] = &[
    "MultiBarBottomLeft",
    "GroupLootFrame1",
    "TutorialFrameParent",
    "FramerateLabel",
    "CastingBarFrame",
    "ChatFrame1",
    "ChatFrame2",
    "ShapeshiftBarFrame",
    "PetActionBarFrame",
    "QuestTimerFrame",
    "DurabilityFrame",
    "QuestWatchFrame",
];

/// Top-level frames that ARE anchored to the screen's bottom edge and are deliberately not managed.
/// Every entry carries the reason, because "it was already like that" is how the bag stack got its
/// shadow copy. Adding a frame here is a decision; adding one silently is what this test forbids.
const BOTTOM_EXEMPT: &[(&str, &str)] = &[
    (
        "MainMenuBar",
        "the base itself — the bar everything else measures its clearance FROM, so it has no \
         clearance of its own to compute",
    ),
    ("UIParent", "the full-screen root itself"),
    (
        "ZoneTextFrame",
        "a mid-screen splash at BOTTOM +512 — nowhere near the contested band",
    ),
    ("SubZoneTextFrame", "as ZoneTextFrame"),
    (
        "MultiBarRight",
        "a BAR, not a tenant of the band — it is one of the things the others clear. Its own seat \
         is the reference's (MultiActionBars.xml: BOTTOMRIGHT -7,+98, with MultiBarLeft riding its \
         TOPLEFT), and it sets the rightLeft/rightRight flags CONTAINER_OFFSET_X reads",
    ),
    (
        "ItemRefTooltip",
        "the reference's own answer, matched exactly (ItemRef.xml l.4): frameStrata HIGH + \
         toplevel + movable at BOTTOM +80. A linked-item window lands ABOVE the HIGH bars rather \
         than clearing them (decision 1318), and the player drags it where they want",
    ),
    (
        "ChatFrame3",
        "undocked chat panes are user-placed, and the reference manages only ChatFrame1/2 in \
         UIPARENT_MANAGED_FRAME_POSITIONS. All five ship hidden at a placeholder BOTTOMLEFT",
    ),
    ("ChatFrame4", "as ChatFrame3"),
    ("ChatFrame5", "as ChatFrame3"),
    ("ChatFrame6", "as ChatFrame3"),
    ("ChatFrame7", "as ChatFrame3"),
    (
        "BlackoutWorld",
        "the world map's full-screen cover (WorldMapFrame.xml) — TOPLEFT+BOTTOMRIGHT is how it \
         fills the screen, not a seat in the contested band, and clearance is the opposite of \
         what it wants: a blackout lifted over the action bars would leave a strip of world \
         showing under the map. It joins this population at all only because decision 1757 took \
         its parent=UIParent away, which is what lets it survive the UIParent:Hide() that \
         showing the map performs",
    ),
];

/// **A top-level frame anchored to the screen's bottom edge is either managed, or exempt with a
/// stated reason.** There is no third option, and this is the guard that covers features nobody
/// has written yet.
///
/// The band at the bottom of the screen is contested by construction: the main bar owns it, and
/// every optional bar stacks upward into it. So the moment a new window anchors `BOTTOM`,
/// `BOTTOMLEFT` or `BOTTOMRIGHT` to the screen root, it has taken on a clearance problem — and the
/// only correct answers are to join `UIPARENT_MANAGED_FRAME_POSITIONS`, to seat itself from a
/// managed global and register a listener (the bag stack's answer), or to say out loud why it does
/// not need to. Failing here is not a bug report; it is a prompt to pick one.
#[test]
fn every_bottom_anchored_top_level_frame_is_accounted_for() {
    // The pass is the stock `UIParent.lua`'s since 1988 — read off the player's chain.
    let _data = benilla_formats::wow_data_or_skip!();
    let pass = String::from_utf8_lossy(
        &super::reference_ui::read(r"Interface\FrameXML\UIParent.lua")
            .expect("the stock UIParent.lua off the chain"),
    )
    .into_owned();
    for f in MANAGED_FRAMES {
        assert!(
            pass.contains(f),
            "{f} is listed here as managed but does not appear in UIParent.lua's pass — this \
             list has drifted from the file it mirrors"
        );
    }

    let mut unaccounted = Vec::new();
    for (file, text) in shipped_xml() {
        let doc = benilla_ui::framexml::parse(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
        for (name, anchors) in bottom_anchored_top_level(&doc) {
            let known = MANAGED_FRAMES.contains(&name.as_str())
                || BOTTOM_EXEMPT.iter().any(|(n, _)| *n == name);
            if !known {
                unaccounted.push(format!("{file}: {name} ({anchors})"));
            }
        }
    }
    assert!(
        unaccounted.is_empty(),
        "these top-level frames anchor to the screen's bottom edge but nothing decides their \
         clearance over the action bars. Give each one a row in \
         UIPARENT_MANAGED_FRAME_POSITIONS (the stock UIParent.lua's table, since 1988), or add \
         it to BOTTOM_EXEMPT here with the reason it needs no row (decision 1499):\n{}",
        unaccounted.join("\n")
    );
}

/// Every top-level INSTANCE (a `<Frame>`/`<Button>`/… that is not `virtual` and carries no
/// `parent` attribute) whose own `<Anchors>` name a BOTTOM point against the screen root — an
/// anchor with no `relativeTo`, or one naming `UIParent`/`WorldFrame`, all three of which resolve
/// to the same full-screen rect.
///
/// Only the frame's OWN anchors, not its children's: a child that anchors BOTTOM to its parent is
/// stating a position inside a window, which is nobody's clearance problem.
fn bottom_anchored_top_level(doc: &benilla_ui::framexml::ParsedDocument) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for item in &doc.items {
        let benilla_ui::framexml::TopLevel::Instance(el) = item else {
            continue;
        };
        if el.attr("parent").is_some() {
            continue;
        }
        let Some(name) = el.name() else { continue };
        if name.contains("$parent") {
            continue;
        }
        let hits: Vec<String> = el
            .children
            .iter()
            .filter(|c| c.tag.eq_ignore_ascii_case("Anchors"))
            .flat_map(|anchors| anchors.children.iter())
            .filter(|a| a.tag.eq_ignore_ascii_case("Anchor"))
            .filter(|a| {
                a.attr("point")
                    .is_some_and(|p| p.to_ascii_uppercase().starts_with("BOTTOM"))
                    && a.attr("relativeTo")
                        .is_none_or(|r| r == "UIParent" || r == "WorldFrame")
            })
            .filter_map(|a| a.attr("point").map(|p| p.to_string()))
            .collect();
        if !hits.is_empty() {
            out.push((name.to_string(), hits.join("/")));
        }
    }
    out
}

/// Every bar a player can raise into the band the tenants below have to share.
///
/// The two VERTICAL bars are in here deliberately, even though they are the only pair that moves
/// the tenants sideways rather than upward: `CONTAINER_OFFSET_X`'s `rightLeft = 90` /
/// `rightRight = 45` arithmetic has never once run in a shipped session, because until the
/// visibility options (1500) both bars were unconditionally hidden and the flags could not be set.
/// Half of the managed table was dead arithmetic nothing exercised. It runs here.
const RAISABLE_BARS: &[&str] = &[
    "MultiBarBottomLeft",
    "MultiBarBottomRight",
    "ShapeshiftBarFrame",
    "MultiBarRight",
    "MultiBarLeft",
];

/// **The symptom, pinned: with any combination of bars raised, nothing in the bottom band overlaps
/// a raised bar.**
///
/// This is the assertion that would have failed on the screenshot that opened 1499 — an open bag
/// window drawn straight through a bottom multibar's buttons. It runs against the REAL shipped
/// XML, drives the bars' visibility directly (so it keeps holding however the toggles are
/// eventually wired), and re-runs the pass between combinations exactly as a live bar change does.
#[test]
fn no_bottom_band_frame_overlaps_a_raised_bar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");

    // Open every bag window. The stack only exists once something is in it, and an EMPTY stack is
    // exactly the state that hid this defect from every earlier test — `bag_tests` never loads
    // `MultiBars.xml` and the multibar tests never open a bag, so no suite had both on screen at
    // once. Shown through the real OnShow, so the stack layout runs the way a B keypress runs it.
    s.run(
        "for _, f in ipairs({BenillaBagFrame, BenillaBagFrame1, BenillaBagFrame2, \
         BenillaBagFrame3, BenillaBagFrame4}) do if f then f:Show() end end",
    )
    .unwrap();

    // Everything that sits in the band and must give way. Each is checked only while shown, so a
    // frame that never appears contributes nothing — which is why the pair counter below matters.
    const TENANTS: &[&str] = &[
        "BenillaBagFrame",
        "BenillaBagFrame1",
        "BenillaBagFrame2",
        "BenillaBagFrame3",
        "BenillaBagFrame4",
        "CastingBarFrame",
        "ChatFrame1",
    ];

    let mut pairs = 0usize;
    for mask in 0..(1u32 << RAISABLE_BARS.len()) {
        let mut raised = Vec::new();
        for (i, bar) in RAISABLE_BARS.iter().enumerate() {
            let on = mask & (1 << i) != 0;
            // **Through the saved globals, not a bare Show.** The stock pass reads
            // `SHOW_MULTI_ACTIONBAR_1`/`_2` for its bottom-bar flags (`UIParent.lua:1598-1606`),
            // never the frames' shown state, so raising a bar by hand and running the pass is a
            // state the client never reaches — and it clears nothing (1988; our retired copy of
            // the pass read `IsShown`, which is what let this drive work before).
            let global = match *bar {
                "MultiBarBottomLeft" => Some("SHOW_MULTI_ACTIONBAR_1"),
                "MultiBarBottomRight" => Some("SHOW_MULTI_ACTIONBAR_2"),
                _ => None,
            };
            if let Some(g) = global {
                s.run(&format!("{g} = {}", if on { "1" } else { "nil" }))
                    .unwrap();
            }
            s.run(&format!(
                "if {bar} then {bar}:{}() end",
                if on { "Show" } else { "Hide" }
            ))
            .unwrap();
            if on {
                raised.push(*bar);
            }
        }
        s.run("UIParent_ManageFramePositions()").unwrap();
        s.resolve();

        for bar in &raised {
            if !shown(&s, bar) {
                continue;
            }
            let bar_rect = rect(&s, bar);
            for victim in TENANTS {
                if !shown(&s, victim) {
                    continue;
                }
                let v = rect(&s, victim);
                pairs += 1;
                assert!(
                    !overlaps(bar_rect, v),
                    "with {raised:?} raised, {victim} {v:?} overlaps {bar} {bar_rect:?} — \
                     something in the bottom band is not clearing the bars. Its seat must come \
                     from UIParent_ManageFramePositions, read fresh (decision 1499)."
                );
            }
        }
    }

    // Never let this pass by testing nothing: a renamed bag frame or a bar that fails to show
    // would otherwise turn the whole guard green while checking zero pairs. With three raisable
    // bars over seven tenants the real number is in the dozens.
    assert!(
        pairs >= 24,
        "only {pairs} bar/tenant pairs were actually compared — the sweep is not exercising the \
         band it claims to guard"
    );
}

fn shown(s: &UiScript, name: &str) -> bool {
    s.eval::<bool>(&format!(
        "return {name} and {name}:IsShown() and true or false"
    ))
    .unwrap_or(false)
}

/// `(left, bottom, right, top)` in screen pixels.
fn rect(s: &UiScript, name: &str) -> (f32, f32, f32, f32) {
    s.eval::<(f32, f32, f32, f32)>(&format!(
        "return {name}:GetLeft(), {name}:GetBottom(), {name}:GetRight(), {name}:GetTop()"
    ))
    .unwrap_or_else(|e| panic!("rect of {name}: {e}"))
}

/// Touching edges do not overlap — the stack seats windows flush against each other by design.
fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// **The item-push card's travel band overlaps a raised bottom bar — and so does the reference's.**
///
/// The director reported the looted-item card as "hidden behind" the extra action bar in the same
/// breath as the bag overlap, so this measures it instead of reasoning about it. The answer is not
/// the one the bag half had:
///
/// ```text
/// card at its opaque peak   x 1253.7..1298.0   y 48.9..93.1
/// MultiBarBottomRight       x  806.0..1306.0   y 57.0..95.0
/// ```
///
/// The card is MEDIUM (a child of a MEDIUM bag button); the bars are `frameStrata="HIGH"`
/// (`MultiActionBars.xml` l.36/151/266/381). **The reference has exactly this arrangement** — its
/// bag buttons are `parent="MainMenuBarArtFrame"` under a `MainMenuBar` with no strata at all, and
/// its `MultiBarBottomRight` runs from `MultiBarBottomLeft`'s right edge +10 for 500 px, which puts
/// its last buttons directly over the bag bar at every resolution. So a push into a bag draws
/// behind that bar's twelfth button whenever that slot is filled, in the real 1.12.1 client too.
/// It goes unnoticed because a multibar's EMPTY wells are hidden, so most players have nothing
/// there; the director's screenshot has a chicken in exactly that slot.
///
/// This is therefore a **finding test, not a guard**: it records what the reference's own geometry
/// produces so that changing either side has to come and edit these numbers on purpose. If we ever
/// diverge deliberately — raising the card's strata, or lifting its band clear of the bars — this
/// is the test that says so out loud rather than the change slipping through green.
#[test]
fn the_item_push_card_shares_the_band_with_a_raised_bar_exactly_as_the_reference_does() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    // The card is the stock `<Model>` since 2015, sized by its file's box once the facts land
    // (`ForcedBackpackItem.m2`: one 1000 ms clamp; the box 0.02707 × 0.07962 model units).
    s.set_model_facts(
        r"Interface\ItemAnimations\ForcedBackpackItem.mdx",
        benilla_ui::widget::ModelFileFacts {
            sequences: vec![benilla_ui::widget::SequenceFacts {
                anim_id: 0,
                duration_ms: 1000,
                looping: false,
            }],
            bbox: ([0.0, 0.0, 0.0], [0.02707, 0.07962, 0.0]),
            cameras: 0,
        },
    );
    s.run("MultiBarBottomLeft:Show() MultiBarBottomRight:Show() UIParent_ManageFramePositions()")
        .unwrap();
    s.resolve();

    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(0),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Bag_08".into()),
        ],
    );
    s.tick(0.133); // the opaque peak — the instant the card is most visible
    s.resolve();
    assert!(
        shown(&s, "MainMenuBarBackpackButtonItemAnim"),
        "the card plays"
    );

    // The pane's rect is the file's box in layout units at 16:9 — the card's whole travel band,
    // 42.41 × 124.72 hung off the button's BOTTOMRIGHT (−10, 0) — and the card the file's keys
    // put inside it at its opaque peak is the reference's 48.9..93.1 above the screen floor
    // (0887 measured that quad by hand; it sits inside this band).
    let card = rect(&s, "MainMenuBarBackpackButtonItemAnim");
    let bar = rect(&s, "MultiBarBottomRight");
    assert!(
        (card.2 - card.0 - 42.41).abs() < 0.05 && (card.3 - card.1 - 124.72).abs() < 0.05,
        "the pane's rect is the file's box: {card:?}"
    );
    assert!(
        card.1 <= 48.9 && card.3 >= 93.1,
        "the reference's card peak 48.9..93.1 lies inside the pane's band: {card:?}"
    );
    assert!(
        overlaps(bar, card),
        "the reference's own geometry puts the card inside the bar's band ({bar:?} vs {card:?}) — \
         if this stops being true, benilla has diverged from it and the divergence needs a record"
    );
}
