//! The bottom-band clearance guards. The main bar, the extra action bars, the stance and pet bars
//! and the reputation bar stack in the bottom band; everything else there (the bags, the cast bar,
//! the chat panes, the default tooltip anchor) clears them through `UIParent_ManageFramePositions`
//! (`UIParent.lua`), which seats the managed frames and writes the managed offsets.

use benilla_ui::script::UiScript;

/// Offsets the stock FrameXML writes and the manage pass uses: our files may read them, never
/// assign them.
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

/// Blanks out `<!-- … -->` regions, newlines kept so line numbers hold: an XML comment is prose,
/// not Lua, even when it spells a managed name.
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
    // A floor for the walk, not a census: `assets/ui` shrinks as windows move to the stock files.
    assert!(out.len() >= 6, "only {} xml files swept", out.len());
    out
}

/// No file of ours assigns a managed offset, as a local, a global or an `or`-guarded default: the
/// pass recomputes them on every bar change, so a stored copy goes stale. Reading one with an
/// inline fallback (`CONTAINER_OFFSET_Y or 70`) is fine; a left-hand side that only contains the
/// name counts as an assignment.
#[test]
fn no_shipped_file_declares_its_own_copy_of_a_managed_offset() {
    let mut offences = Vec::new();
    for (name, text) in shipped_xml() {
        if name == r"Interface\FrameXML\UIParent.xml" {
            continue; // the owner: its var rows define these
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

/// Frames the manage pass seats: the `UIPARENT_MANAGED_FRAME_POSITIONS` frame rows and four that
/// its tail seats by hand. The next guard checks each still appears in the stock `UIParent.lua`.
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

/// Top-level frames anchored to the screen's bottom edge that the pass does not manage, each with
/// its reason.
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

/// A top-level frame anchored to the screen's bottom edge sits in the contested band, so it has a
/// `UIPARENT_MANAGED_FRAME_POSITIONS` row, seats itself from a managed offset with a listener, or
/// is listed in `BOTTOM_EXEMPT` with its reason.
#[test]
fn every_bottom_anchored_top_level_frame_is_accounted_for() {
    // The pass is the stock `UIParent.lua`'s, read off the player's chain.
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

/// Top-level instances (not `virtual`, no `parent`) whose own anchors put a BOTTOM point on the
/// screen root: no `relativeTo`, or `UIParent` or `WorldFrame`, all the full screen. A child's
/// anchors place it inside its window, so they do not count.
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

/// The bars a player can raise into the band. The two vertical bars move the tenants sideways,
/// through the `rightLeft` and `rightRight` terms of `CONTAINER_OFFSET_X`.
const RAISABLE_BARS: &[&str] = &[
    "MultiBarBottomLeft",
    "MultiBarBottomRight",
    "ShapeshiftBarFrame",
    "MultiBarRight",
    "MultiBarLeft",
];

/// With any combination of bars raised, nothing in the bottom band overlaps a raised bar. Runs the
/// full UI and reruns the pass after each combination, as a live bar change does.
#[test]
fn no_bottom_band_frame_overlaps_a_raised_bar() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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

    s.run(
        "for _, f in ipairs({BenillaBagFrame, BenillaBagFrame1, BenillaBagFrame2, \
         BenillaBagFrame3, BenillaBagFrame4}) do if f then f:Show() end end",
    )
    .unwrap();

    // The frames in the band that must clear the bars. Each is checked only while shown, hence
    // the pair floor below.
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
            // The stock pass takes its bottom-bar flags from `SHOW_MULTI_ACTIONBAR_1`/`_2`, not
            // from the frames' shown state (`UIParent.lua:1599-1607`).
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

    // A floor on the pairs compared, so the sweep cannot pass by comparing nothing.
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

/// Touching edges do not overlap: the stack seats windows flush against each other.
fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// The item-push card overlaps a raised bottom bar, as in the reference: the card is MEDIUM under
/// the bag button, the multibars are `frameStrata="HIGH"` (`MultiActionBars.xml:36`), and
/// `MultiBarBottomRight`'s last buttons sit over the bag bar, so a push draws behind a filled
/// twelfth button. At 1600x900 the card peaks at x 1253.7..1298.0, y 48.9..93.1, and the bar
/// spans x 806..1306, y 57..95.
#[test]
fn the_item_push_card_shares_the_band_with_a_raised_bar_exactly_as_the_reference_does() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI loads on world entry, so a player always exists by then.
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
    // The stock `<Model>` card sizes from its file's box: `ForcedBackpackItem.m2` has one 1000 ms
    // non-looping sequence and a 0.02707 x 0.07962 box, in model units.
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
    s.tick(0.133); // the card's opaque peak
    s.resolve();
    assert!(
        shown(&s, "MainMenuBarBackpackButtonItemAnim"),
        "the card plays"
    );

    // The pane is the file's box in layout units at 16:9: 42.41 x 124.72, off the button's
    // BOTTOMRIGHT (-10, 0). The reference's card peaks at 48.9..93.1 above the floor, inside it.
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
