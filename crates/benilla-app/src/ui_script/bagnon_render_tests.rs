//! **"Only the frame header and the gold text. No bag slots at all."** — the director's Bagnon
//! report, reproduced end to end and then closed.
//!
//! ## Why this file exists
//!
//! The addon survey scored Bagnon `loaded, missing=0, session=ok, probe=ok` — every column green —
//! while the window on the director's screen was empty. Every one of those columns asks whether
//! something *raised*; none of them asks whether anything was **drawn**. Two independent faults
//! were hiding in that gap, and only one of them raises at all:
//!
//! | | what it does | does it raise? |
//! |---|---|---|
//! | `UnitName("player")` is nil at addon file scope | Bagnon reads the live bags as another character's offline cache → **0 slots**, window shown | **no** |
//! | the `SetItemButton*` family is missing | the per-slot update dies on the first slot → loop aborts, window never shown | yes, once |
//!
//! The director saw the first (their `Bagnon_Core` loaded at startup, so `currentPlayer` was
//! captured nil). Fix only that and you get the second. So the tests below pin **both**, and the
//! headline is a quad count, not an error count: [`bagnon_draws_a_slot_for_every_bag_slot`] is
//! green only when every live slot's quad actually reaches the render list — the backpack's 16
//! and, since the reference's own `ContainerFrame.lua` runs off the chain (1751), the keyring's 12
//! beside them (Bagnon's default bag set is `{-2, 0, 1, 2, 3, 4}`, `Bagnon.lua:25`).
//!
//! Nothing from the corpus is committed, and every test here skips cleanly on a machine without
//! it — the `ui_chat::ace_gate_tests` rule.

use std::path::Path;

use benilla_formats::addon_corpus_or_skip as corpus_or_skip;
use benilla_ui::script::{AddOnInfo, ContainerSlot, ContainerState, QuadContent, UiScript};
use benilla_ui::toc::Toc;

/// The corpus **and** a client install — for the tests whose subject is a global that comes from
/// the reference file this client SOURCES off the player's own patch chain rather than shipping
/// ([`super::reference_ui`], decision 1234).
///
/// `corpus_or_skip!` alone is the wrong precondition for them. With the corpus present but no
/// install, `ContainerFrameItemButton_OnEnter`/`_OnClick` are *legitimately* nil — that is the
/// documented "no install, no file" behaviour, not a defect — and the test then fails for a reason
/// that is not a bug. `reference_ui`'s own header already says tests that need the file gate on the
/// install the way every other client-data test does; these four never got that gate when 1234
/// added the sourcing seam and the tests in one landing.
///
/// It stayed invisible because it only bites where the install is *not* found, which on a dev
/// machine is only `scripts/gates.sh`'s `player-tests` rung: `--no-default-features` compiles out
/// the `dev` project-folder candidate, so the `WoW` symlink beside the worktree stops being visible.
macro_rules! corpus_and_install_or_skip {
    () => {{
        let _data = benilla_formats::wow_data_or_skip!();
        corpus_or_skip!()
    }};
}

fn read_toc(root: &Path, name: &str) -> Toc {
    let path = root.join(name).join(format!("{name}.toc"));
    Toc::parse(&benilla_ui::source::decode(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    ))
}

/// One addon's `.toc` files through the same two arms the real loader uses — `.lua` as a chunk,
/// anything else as FrameXML — with the AddOns root as the provider's path space (decision 1186).
fn load_addon_files(script: &UiScript, root: &Path, name: &str) -> Vec<String> {
    let toc = read_toc(root, name);
    let provider = |req: &str| -> Option<Vec<u8>> { std::fs::read(root.join(req)).ok() };
    let mut errors = Vec::new();
    for file in &toc.files {
        let path = benilla_ui::loader::join_ref(name, file);
        let Ok(bytes) = std::fs::read(root.join(&path)) else {
            errors.push(format!("{file}: not found"));
            continue;
        };
        if file.to_ascii_lowercase().ends_with(".lua") {
            // Named as the client names it — the corpus parses its own tracebacks.
            if let Err(e) =
                script.run_chunk_named(&bytes, &benilla_ui::script::addon_chunk_name(name, file))
            {
                errors.push(format!("{file}: {e}"));
            }
            continue;
        }
        match benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes)) {
            Ok(doc) => {
                let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
                errors.extend(report.errors.into_iter().map(|e| format!("{file}: {e}")));
            }
            Err(e) => errors.push(format!("{file}: {e}")),
        }
    }
    errors
}

/// The director's exact installed set. Bagnon and Bagnon_Options are `## LoadOnDemand: 1`, so the
/// startup walk skips them and `LoadAddOn` brings them in — which is why the registry below
/// carries a real root.
const INSTALLED: &[&str] = &[
    "!OmniCC",
    "Bagnon",
    "Bagnon_Core",
    "Bagnon_Forever",
    "Bagnon_Options",
    "MapCoords",
];

/// Whether the VM gets its `"player"` before the addons load — i.e. whether
/// [`super::seat_from_roster`] ran.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seat {
    /// What the app does now: the roster row goes in ahead of `load_ingame_ui`.
    BeforeAddons,
    /// What it did before the fix: nothing writes `"player"` until `ui_unit::feed_units` sees the
    /// self descriptor, seconds later.
    AfterAddons,
}

/// The roster row the director's session would carry, and the same struct the app reads —
/// so the seat under test is built by [`super::seat_from_roster`] itself, not by a copy of it.
fn roster() -> crate::char_select::Roster {
    let row = benilla_protocol::Character {
        guid: 7,
        name: "Harness".into(),
        race: 1,  // Human → Alliance
        class: 1, // Warrior
        gender: 0,
        level: 60,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        zone: 0,
        map: 0,
        position: benilla_protocol::wire::Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        flags: 0,
        equipment: [benilla_protocol::CharEnumItem::default(); 19],
        pet_display_id: 0,
        pet_level: 0,
        pet_family: 0,
    };
    crate::char_select::Roster::with_pending_pick(vec![row], 7)
}

/// The `"player"` snapshot the self descriptor builds when it lands — what `ui_unit::feed_units`
/// pushes (its `"player"` leg) and fires `PLAYER_ENTERING_WORLD` off. It is the roster seat plus
/// the fields only the descriptor can say: the unit EXISTS, at the roster's level, with health
/// and a live object. [`super::seat_from_roster`] deliberately leaves those at nil/0 (decision
/// 2263: the reference's `UnitExists("player")` is nil and `UnitLevel("player")` is 0 before the
/// object exists), so the seat can no longer stand in for the descriptor the way it did from 1230
/// until then — a fixture that never pushes this stays in the pre-object window for good, which
/// is a state the real client is never in when a player opens their bags.
fn descriptor_snapshot(seat: &benilla_ui::script::UnitState) -> benilla_ui::script::UnitState {
    benilla_ui::script::UnitState {
        exists: true,
        has_object: true,
        is_connected: true,
        guid: 7,
        level: 60,
        health: 4_000,
        max_health: 4_000,
        power_type: 1, // a Warrior's rage
        max_power: 1_000,
        ..seat.clone()
    }
}

/// A VM shaped like the director's session: our whole interface, their installed addons walked in
/// dependency order the way `addons::load_third_party` walks them, and a real backpack.
fn seat(root: &Path, seat: Seat) -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);

    let mut registry: Vec<AddOnInfo> = INSTALLED
        .iter()
        .map(|n| super::addons::info_from_toc(n, &read_toc(root, n)))
        .collect();
    s.register_addons(registry.clone(), Some(root.to_path_buf()), None, None);
    s.set_realm_name("Harness");
    let player = super::seat_from_roster(&roster()).expect("a pending pick seats a player");
    if seat == Seat::BeforeAddons {
        // The real entry load seeds the player RECORD beside the seat (`lifecycle.rs`, decisions
        // 2261/2263); this one push does both, by `set_unit`'s keep-in-step rule — a `"player"`
        // push that carries a name seeds the record — so `UnitName("player")` is "Harness" at
        // addon file scope, which is what sends Bagnon down its LIVE path.
        s.set_unit("player", Some(player.clone()));
    }
    let failures = super::load_default_ui(&s);
    assert!(
        failures.is_empty(),
        "our own FrameXML failed to load: {failures:#?}"
    );

    // A real backpack — 16 slots, one occupied. `GetContainerNumSlots(0)` answering 16 is what
    // Bagnon's LIVE path reads; the whole bug is that it took a different path and never asked.
    //
    // The slot carries a real `item_id` and `link`, not just an icon: the render half only ever
    // needed the texture, but the INTERACTION half rides `GameTooltip:SetBagItem` (which requires
    // a resolved instance) and `PickupContainerItem` (which needs something to pick up), so a
    // texture-only slot would make every hover and every click a silent no-op — a fixture that
    // hides the very thing the test is for.
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 1,
            item_id: 4496,
            quality: Some(1),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );

    // The startup walk: dependencies first, LoadOnDemand skipped, one `ADDON_LOADED` each.
    let mut done: Vec<String> = Vec::new();
    for name in INSTALLED {
        walk(&mut s, root, name, &mut done, &mut registry);
    }
    // Re-registered with the startup set marked loaded, so `LoadAddOn` below meets satisfied
    // dependencies exactly as it does in a real session.
    s.register_addons(registry, Some(root.to_path_buf()), None, None);
    for event in ["VARIABLES_LOADED", "PLAYER_LOGIN"] {
        s.fire_event(event, Vec::new());
    }
    // The self descriptor's arrival: `ui_unit::feed_units` pushes the snapshot the descriptor
    // builds and fires PLAYER_ENTERING_WORLD off it — in BOTH arms, because that is the world
    // every real Bagnon open happens in. It matters: the reference's `GetKeyRingSize`
    // (`ContainerFrame.lua:773-786`) reads `UnitLevel("player")`, and a fixture left in the
    // pre-object window (level 0, the roster seat's answer since 2263) sizes the keyring at 4
    // where the level-60 session sizes it at 12. In the `AfterAddons` arm this is the FIRST time
    // the VM has ever had a `"player"` — which is the state this file exists to reproduce.
    s.set_unit("player", Some(descriptor_snapshot(&player)));
    s.fire_event("PLAYER_ENTERING_WORLD", Vec::new());
    for _ in 0..10 {
        s.tick(0.1);
    }
    s
}

fn walk(s: &mut UiScript, root: &Path, name: &str, done: &mut Vec<String>, reg: &mut [AddOnInfo]) {
    if done.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return;
    }
    let toc = read_toc(root, name);
    if toc.load_on_demand() {
        return; // the reference's `0x51f600` loads only records whose LoadOnDemand byte is 0
    }
    done.push(name.to_string());
    for dep in toc.dependencies() {
        if INSTALLED.iter().any(|n| n.eq_ignore_ascii_case(dep)) {
            walk(s, root, dep, done, reg);
        }
    }
    load_addon_files(s, root, name);
    if let Some(i) = reg.iter().position(|a| a.name.eq_ignore_ascii_case(name)) {
        reg[i].loaded = true;
    }
    s.fire_event(
        "ADDON_LOADED",
        vec![benilla_ui::script::ScriptValue::Str(name.to_string())],
    );
}

/// Every item-slot quad **Bagnon's own buttons** drew — the `UI-Quickslot2` ring, which is the one
/// texture an empty slot still paints (an occupied slot adds its icon on top).
///
/// **Attributed, not counted.** Our own `BenillaBagFrame` paints sixteen of exactly this texture,
/// so a bare total reads 16 whether Bagnon drew nothing or our window did — the "an addon replaces,
/// it does not add" trap. Every quad is charged to the nearest named frame above it
/// ([`UiScript::target_owner_name`]) and only `BagnonItem*` counts.
fn bagnon_slot_quads(s: &mut UiScript) -> usize {
    s.resolve();
    s.extract()
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                     if p.contains("UI-Quickslot2"))
        })
        .filter(|q| {
            s.target_owner_name(q.target)
                .is_some_and(|n| n.starts_with("BagnonItem"))
        })
        .count()
}

/// Bagnon's window, open, with the fixture's backpack in it — the state every interaction test
/// below starts from. `ToggleBackpack` is Bagnon's own replacement, and its first call
/// demand-loads the `Bagnon` addon (the frame does not exist before that).
fn open_bagnon(root: &Path) -> UiScript {
    let mut s = seat(root, Seat::BeforeAddons);
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    assert!(
        s.eval::<bool>("return Bagnon and Bagnon:IsVisible() or false")
            .unwrap(),
        "the window must be on screen before anything is clicked"
    );
    s.resolve();
    s
}

/// The centre of a named frame, in the y-up UI space [`UiScript::mouse_move`] /
/// [`UiScript::mouse_button`] take — the same space `resolve`/`extract` use, so a frame's own
/// `GetLeft`/`GetBottom` read straight back as a cursor position.
fn centre_of(s: &mut UiScript, name: &str) -> (f32, f32) {
    s.resolve();
    let r: Vec<f32> = s
        .eval(&format!(
            "local f = getglobal(\"{name}\") \
             return {{ f:GetLeft(), f:GetBottom(), f:GetWidth(), f:GetHeight() }}"
        ))
        .unwrap_or_else(|e| panic!("{name}: no resolved rect: {e}"));
    assert_eq!(r.len(), 4, "{name}: unresolved rect {r:?}");
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

/// The name of the Bagnon item button showing backpack slot `slot`. Bagnon lays its buttons out in
/// its own order and `SetID`s each to the game slot, so the button holding the fixture's one item
/// is found by asking, never by assuming an index.
fn bagnon_button_for_slot(s: &UiScript, bag: i64, slot: u32) -> String {
    s.eval::<String>(&format!(
        "for i = 1, 36 do \
           local b = getglobal(\"BagnonItem\"..i) \
           if b and b:GetID() == {slot} and b:GetParent():GetID() == {bag} then \
             return \"BagnonItem\"..i \
           end \
         end \
         return \"\""
    ))
    .inspect(|n| assert!(!n.is_empty(), "no BagnonItem* is bag {bag} slot {slot}"))
    .expect("the item-button scan")
}

/// Press and release the named button over `name`, the way a player's mouse does — through
/// `mouse_button`, so the whole engine path runs (hit test, `RegisterForClicks` gate, the frame's
/// own `OnClick`), never `s.run("Handler(...)")`.
fn click(s: &mut UiScript, name: &str, button: &str) {
    let (x, y) = centre_of(s, name);
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// **The headline, and the director's own symptom.** Press B with Bagnon installed and every one
/// of the backpack's 16 slots must reach the render list.
///
/// This is the test that was red before the two fixes and is green after — and the *number* is the
/// point, not the absence of an error: before, this VM raised nothing at all on the path the
/// director was on.
#[test]
fn bagnon_draws_a_slot_for_every_bag_slot() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // Bagnon's own entry point: it replaces `ToggleBackpack`, and its replacement demand-loads the
    // `Bagnon` addon on the first call (the frame does not exist before that).
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }

    assert!(
        s.eval::<bool>("return Bagnon and Bagnon:IsVisible() or false")
            .unwrap(),
        "the window must be on screen"
    );
    // 28 = the backpack's 16 + the keyring's 12. Bagnon's default bag set carries the keyring
    // (`Bagnon.lua:25`, `{-2, 0, 1, 2, 3, 4}`), and its size is the reference's own
    // `GetKeyRingSize` ladder off `UnitLevel("player")` (`ContainerFrame.lua:773-786`: 4, 8 at
    // 40, 12 at 50, 16 only above 60) — 12 for the fixture's level-60 Warrior. The four bag slots
    // hold nothing, so they add nothing.
    assert_eq!(
        s.eval::<i64>("return Bagnon.size").unwrap(),
        28,
        "Bagnon must size itself off the LIVE bags (GetContainerNumSlots(0) == 16 plus \
         GetKeyRingSize() == 12 at level 60), not off Bagnon_Forever's empty offline cache — nil \
         UnitName('player') is what sent it there"
    );
    assert_eq!(
        bagnon_slot_quads(&mut s),
        28,
        "every live slot must actually be DRAWN (16 backpack + 12 keyring) — the director saw a \
         title and a gold line and nothing else, while every column of the addon survey called \
         this addon fine"
    );
    assert!(
        s.errors().is_empty(),
        "and nothing may raise on the way: {:#?}",
        s.errors()
    );
}

/// **The control that makes the headline mean something: reproduce the director's screen.**
///
/// With the player seated only *after* the addons load — benilla's own order until
/// [`super::seat_from_roster`] landed — Bagnon shows a window whose title and money frame are
/// right and which contains **not one slot**, and raises nothing doing it. If this ever starts
/// drawing slots, the headline above has stopped testing the thing it was written for.
#[test]
fn without_a_player_at_addon_load_bagnon_draws_an_empty_window() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::AfterAddons);
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }

    // The two things the director COULD see, exactly as reported.
    assert!(
        s.eval::<bool>("return Bagnon:IsVisible()").unwrap(),
        "the window is up"
    );
    assert_eq!(
        s.eval::<String>("return BagnonTitle:GetText()").unwrap(),
        "Harness's Inventory",
        "…with its header"
    );
    // And the thing they could not.
    assert_eq!(
        bagnon_slot_quads(&mut s),
        0,
        "no slot is drawn — this is the report, reproduced"
    );
    assert!(
        s.eval::<String>("return tostring(BagnonItem1)").unwrap() == "nil",
        "not one item button was even created"
    );
}

/// The **other** fault, pinned on its own so a later regression names itself: the reference's
/// `SetItemButton*` family (`Interface\FrameXML\ItemButtonTemplate.xml`).
///
/// Bagnon ends every slot update with `SetItemButtonDesaturated` / `SetItemButtonTexture` /
/// `SetItemButtonCount`. While those were nil the raise landed inside `BagnonFrame_AddBag`'s
/// per-slot loop, so the FIRST slot aborted it, `frame.size` never advanced past 0 and the window
/// never reached its own `frame:Show()`. That is a different picture from the one above — nothing
/// on screen at all — and it is what the identity fix alone would have produced.
#[test]
fn the_item_button_helpers_paint_a_slots_icon_and_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().expect("VM");
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches (decision 1848).
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
    assert!(failures.is_empty(), "our own FrameXML: {failures:#?}");

    // A button shaped the way the family's naming contract requires, built the way an addon does.
    s.run(
        r#"
        local b = CreateFrame("Button", "ProbeItemButton", UIParent)
        b:SetWidth(37) b:SetHeight(37)
        b:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        local icon = b:CreateTexture("ProbeItemButtonIconTexture", "BORDER")
        icon:SetAllPoints(b)
        local count = b:CreateFontString("ProbeItemButtonCount", "OVERLAY", "NumberFontNormal")
        count:SetPoint("BOTTOMRIGHT", b, "BOTTOMRIGHT", -5, 2)
        count:Hide()
    "#,
    )
    .expect("probe button");

    s.run(r#"SetItemButtonTexture(ProbeItemButton, "Interface\\Icons\\INV_Misc_Bag_08")"#)
        .expect("SetItemButtonTexture");
    s.run("SetItemButtonCount(ProbeItemButton, 5)")
        .expect("SetItemButtonCount");
    s.run("SetItemButtonDesaturated(ProbeItemButton, 1, 0.5, 0.5, 0.5)")
        .expect("SetItemButtonDesaturated");

    // Attributed to the probe button, never to the whole screen: the default UI paints its own
    // "1".."0" action-bar hotkeys, so a bare "is there a quad reading 1 anywhere" would answer yes
    // whatever these functions did. The `!` case below is the one that would have been fooled.
    let count_text = |s: &UiScript, want: &str| {
        s.extract().iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text, .. } if text.as_deref() == Some(want))
                && s.target_owner_name(q.target).as_deref() == Some("ProbeItemButton")
        })
    };

    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(&q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("INV_Misc_Bag_08"))),
        "the icon the addon asked for must be on the region"
    );
    assert!(
        count_text(&s, "5"),
        "a stack of 5 shows its count (>1 is the reference's own gate)"
    );
    // The no-shader branch: benilla has no `Texture:SetDesaturated`, so a locked slot takes the
    // reference's own fixed-function fallback — a flat 0.5 grey tint on the icon.
    let tint = s
        .eval::<Vec<f32>>("return {ProbeItemButtonIconTexture:GetVertexColor()}")
        .expect("GetVertexColor");
    assert_eq!(
        &tint[..3],
        &[0.5, 0.5, 0.5],
        "a locked item greys out (ref: `elseif not r or not shaderSupported`)"
    );

    // A count of 1 hides the label again — the reference's gate, not a one-way switch.
    s.run("SetItemButtonCount(ProbeItemButton, 1)").unwrap();
    s.resolve();
    assert!(
        !count_text(&s, "1") && !count_text(&s, "5"),
        "a single item shows no count"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The seat itself, in isolation: what the roster row actually puts in the VM, and the `None` that
/// keeps a capture (no pick, no roster) unchanged.
#[test]
fn the_roster_seat_names_the_character_the_addons_will_meet() {
    let seat = super::seat_from_roster(&roster()).expect("a pending pick seats a player");
    assert_eq!(seat.name.as_deref(), Some("Harness"));
    // **0, not 60 — the reference's answer, byte-verified** (decision 2263). `UnitLevel 0x517fc0`
    // carries no `"player"` fast path at all: it resolves the token, misses (no object, no roster
    // record for a zero GUID) and reaches `0x51813e push 0; push 0` — the NUMBER 0, one return.
    // The roster's real level arrives with the descriptor, within the second.
    assert_eq!(seat.level, 0);
    assert_eq!(seat.race_file.as_deref(), Some("Human"));
    assert_eq!(seat.class_file.as_deref(), Some("WARRIOR"));
    assert_eq!(seat.sex, 2, "the wire's 0 is UnitSex's 2");
    assert_eq!(
        seat.faction_group.as_deref(),
        Some("Alliance"),
        "nil here is 24 corpus addons stopping on AceDB-2.0's file-scope concatenation"
    );
    // **`exists` is false, and that is the correction 2263 made** (it was `true` from 1230 until
    // then). `UnitExists 0x515fb0` has no fast path either; its resolver reads the GUID out of the
    // OBJECT (`0x515994`), so with none it holds `0:0`, and the roster fallback `0x491900` bails
    // on a zero GUID at `0x4e80aa je` BEFORE fetching the active player — so the `0 == 0` that
    // would answer "that's me" never runs, and `0x516001` pushes nil.
    //
    // Safe to correct only *because* of the record above: 1230 seated a whole unit in order to
    // deliver a name at addon file scope, and the name no longer needs a unit to exist. The rest
    // of the seat is untouched and still load-bearing — the faction side below above all.
    assert!(!seat.exists);
    assert!(seat.is_player);
    // Deliberately NOT invented — the descriptor says these, within the second.
    assert_eq!((seat.health, seat.max_health), (0, 0));

    assert!(
        super::seat_from_roster(&crate::char_select::Roster::default()).is_none(),
        "no pick, no seat — a capture must load exactly as it did before"
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The INTERACTION half — the layer the render half could not see
// ─────────────────────────────────────────────────────────────────────────────────────────────
//
// The tests above prove Bagnon's window is *drawn*. The director's next report was that it is
// **dead to the touch**: hover raises, click raises, and the console fills up. Every column of the
// addon survey — load, session, probe, and 1230's render column — is blind to that, for one
// structural reason: **none of them clicks**. So these do, through
// [`UiScript::mouse_move`]/[`UiScript::mouse_button`], which is the whole engine path a player's
// mouse takes (hit test → `RegisterForClicks` gate → the frame's own `OnClick`), never a direct
// `s.run("Handler(...)")` that would prove only that a function exists.
//
// `the_whole_bagnon_window_survives_being_used` below is the *enumerator*: it drives every
// gesture in one VM and asserts the error list is empty. A scan can only find what it knows to
// look for; this finds whatever is actually missing.

/// **Bug 1, hover.** `Bagnon_Core/core/Item.lua:136` ends its normal-bag branch in
/// `ContainerFrameItemButton_OnEnter(item)` — the reference's own global. It was nil, so every
/// hover over every slot raised and no tooltip ever appeared.
#[test]
fn hovering_a_bagnon_slot_shows_the_items_tooltip() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    let (x, y) = centre_of(&mut s, &occupied);
    s.mouse_move(x, y);

    assert!(
        s.errors().is_empty(),
        "a hover must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>(&format!("return GameTooltip:IsOwned({occupied})"))
            .unwrap(),
        "the tooltip must belong to the hovered slot ({occupied})"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "…and be on screen"
    );

    // …and it drops again on the way out, the template's own OnLeave.
    let (ex, ey) = (x, y + 400.0);
    s.mouse_move(ex, ey);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsShown()").unwrap(),
        "moving off the slot hides the tooltip"
    );
    assert!(s.errors().is_empty(), "…without raising: {:#?}", s.errors());
}

/// **Bug 1, left click.** `Item.lua:105` routes a plain click to
/// `ContainerFrameItemButton_OnClick(mouseButton, ignoreModifiers)`, whose unmodified left arm is
/// `PickupContainerItem(this:GetParent():GetID(), this:GetID())`. The observable is the cursor.
#[test]
fn left_clicking_a_bagnon_slot_picks_the_item_up() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    assert!(
        !s.eval::<bool>("return CursorHasItem()").unwrap(),
        "nothing is held before the click"
    );
    click(&mut s, &occupied, "LeftButton");

    assert!(
        s.errors().is_empty(),
        "a left click must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return CursorHasItem()").unwrap(),
        "the item is now on the cursor"
    );

    // …and clicking an empty slot puts it back down, through the same one entry point.
    let empty = bagnon_button_for_slot(&s, 0, 2);
    click(&mut s, &empty, "LeftButton");
    assert_eq!(
        s.take_container_moves(),
        vec![benilla_ui::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 2,
            count: None,
        }],
        "the place half of the same gesture"
    );
    assert!(s.errors().is_empty(), "…nor the place: {:#?}", s.errors());
}

/// **Bug 1, right click.** The reference's right arm with no merchant open is
/// `UseContainerItem(bag, slot)` — use/equip the item.
#[test]
fn right_clicking_a_bagnon_slot_uses_the_item() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    click(&mut s, &occupied, "RightButton");

    assert!(
        s.errors().is_empty(),
        "a right click must not raise: {:#?}",
        s.errors()
    );
    assert_eq!(
        s.take_container_uses(),
        vec![(0, 1)],
        "right-click uses the slot"
    );
}

/// **Bug 1, drag.** Bagnon registers `OnDragStart`/`OnReceiveDrag` on every item button and routes
/// both to `BagnonItem_OnClick("LeftButton", 1)` → `ContainerFrameItemButton_OnClick`'s
/// `ignoreModifiers` arm. Driven as a real gesture: press, move past the threshold, release.
#[test]
fn dragging_a_bagnon_slot_picks_the_item_up() {
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);
    let empty = bagnon_button_for_slot(&s, 0, 2);

    let (sx, sy) = centre_of(&mut s, &occupied);
    let (dx, dy) = centre_of(&mut s, &empty);
    s.mouse_move(sx, sy);
    s.mouse_button(sx, sy, "LeftButton", true);
    s.mouse_move(sx + 12.0, sy + 12.0); // past DRAG_START_THRESHOLD ⇒ OnDragStart
    s.mouse_move(dx, dy);
    s.mouse_button(dx, dy, "LeftButton", false);

    assert!(
        s.errors().is_empty(),
        "a drag must not raise: {:#?}",
        s.errors()
    );
    assert_eq!(
        s.take_container_moves(),
        vec![benilla_ui::script::ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 2,
            count: None,
        }],
        "drag-and-drop moved the item"
    );
}

/// **Bug 2 — our own handler, called the reference's way.** Bagnon takes
/// `MainMenuBarBackpackButton:GetScript("OnEnter")` and calls it back with **no arguments**
/// (`Bagnon.lua:87`), because that is the reference's contract: an XML script body takes nothing
/// and reads the frame off the `this` global. Ours compiled to `function(self, ...)` and its body
/// passed `self` straight into `GameTooltip:SetOwner`, so the hook handed it nil:
///
/// ```text
/// bad argument #2: error converting Lua nil to table
///   BagFrame.xml:789: in function 'BenillaBagToggle_OnEnter'
///   [string "MainMenuBarBackpackButton:OnEnter"]:2: in upvalue 'bMainBag_OnEnter'
/// ```
#[test]
fn the_backpack_button_still_hovers_with_bagnon_holding_its_script() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // Bagnon really did take the script over — otherwise this test proves nothing.
    assert!(
        s.eval::<bool>(
            "return MainMenuBarBackpackButton:GetScript(\"OnEnter\") == BagnonBlizMainBag_OnEnter"
        )
        .unwrap(),
        "Bagnon_AddBagHooks must have replaced the backpack button's OnEnter"
    );

    let (x, y) = centre_of(&mut s, "MainMenuBarBackpackButton");
    s.mouse_move(x, y);

    assert!(
        s.errors().is_empty(),
        "the hooked hover must not raise: {:#?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(MainMenuBarBackpackButton)")
            .unwrap(),
        "our own tooltip still opens, owned by the button"
    );
}

/// **Bug 3.** `Bagnon_Forever/database/ui.lua:61` sizes its character dropdown from
/// `button:GetTextWidth()` on a **CheckButton** — a real 1.12 Button method (`0x782290`, and
/// `GetStringWidth` is absent on Button).
#[test]
fn a_button_reports_its_own_label_width() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // Bagnon_Forever's exact shape: a CheckButton built from its own template, with a label.
    s.run(
        r#"
        local b = CreateFrame("CheckButton", "ProbeWidthButton", UIParent, "BagnonDBUINameBox")
        b:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
        b:SetText("ProbeLabelText")
    "#,
    )
    .expect("the probe button");
    s.resolve();

    // Answer the host measure for the button's OWN label, and nothing else — the assertion below
    // is then "the button forwarded to the right region and read the right metric", not merely
    // "some number came back". A headless VM has no font atlas, so an unanswered measure is 0 and
    // a `> 0` assertion would be untestable here (and unfalsifiable there).
    let req = s
        .fontstrings_needing_measure()
        .into_iter()
        .find(|r| r.text == "ProbeLabelText")
        .expect("the label asks to be measured");
    s.set_measured_text_unwrapped(&[(req.id, 61.0, 12.0, req.key)]);

    assert_eq!(
        s.eval::<f64>("return ProbeWidthButton:GetTextWidth()")
            .expect("Button:GetTextWidth"),
        61.0,
        "the Button reports its LABEL's natural extent"
    );
    assert_eq!(
        s.eval::<f64>("return ProbeWidthButton:GetTextHeight()")
            .expect("Button:GetTextHeight"),
        12.0
    );
    // A Button with no label at all answers 0 rather than raising — the reference dereferences a
    // FontString pointer here that a bare `CreateFrame("Button")` leaves null, and what it does
    // then is not byte-read, so this takes the harmless number.
    s.run(r#"CreateFrame("Button", "ProbeLabellessButton", UIParent)"#)
        .unwrap();
    assert_eq!(
        s.eval::<f64>("return ProbeLabellessButton:GetTextWidth()")
            .unwrap(),
        0.0
    );

    // The real consumer, end to end: Bagnon_Forever's own character list.
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    assert!(
        s.errors().is_empty(),
        "the character list must not raise: {:#?}",
        s.errors()
    );
}

/// **Bug 4.** `Bagnon_Core/core/Item.xml:34` hangs `<Model name="$parentCooldown"
/// inherits="CooldownFrameTemplate"/>` off every item button — the reference's own template, which
/// we had never declared, so every one of the 36 buttons warned and the child came out bare.
#[test]
fn the_reference_cooldown_template_resolves() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let s = open_bagnon(&root);
    let occupied = bagnon_button_for_slot(&s, 0, 1);

    // The child exists and answers as a frame, whatever Bagnon does to it.
    assert!(
        s.eval::<bool>(&format!("return getglobal(\"{occupied}Cooldown\") ~= nil"))
            .unwrap(),
        "the template's $parentCooldown child must exist"
    );
    // …and the one entry point every consumer drives it through does not raise on it.
    s.run(&format!(
        "CooldownFrame_SetTimer(getglobal(\"{occupied}Cooldown\"), GetTime(), 30, 1)"
    ))
    .expect("CooldownFrame_SetTimer on an addon's cooldown child");
    assert!(s.errors().is_empty(), "{:#?}", s.errors());
}

/// **The enumerator.** Every gesture a player makes in Bagnon's window, in one VM, with the error
/// list asserted empty at the end.
///
/// This is the test the arc was missing. Four landings in a row fixed the errors that happened to
/// fire and left the next layer to be found by the director playing, because every instrument we
/// had asks whether something *raised at load* and none of them touches the window. A scan finds
/// what it knows to look for; this finds what is actually missing, because it does what a player
/// does.
#[test]
fn the_whole_bagnon_window_survives_being_used() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // 1 · Every slot, hovered and left again — the OnEnter/OnUpdate/OnLeave loop on both an
    //     occupied and an empty slot, across every bag the window lays out (the keyring's slots
    //     come first in Bagnon's default order, the backpack's after them).
    let size = s.eval::<i64>("return Bagnon.size").expect("Bagnon.size");
    for i in 1..=size {
        let name = format!("BagnonItem{i}");
        if s.eval::<bool>(&format!("return getglobal(\"{name}\") ~= nil"))
            .unwrap_or(false)
        {
            let (x, y) = centre_of(&mut s, &name);
            s.mouse_move(x, y);
            s.tick(0.4); // Bagnon's own 0.3s OnUpdate tooltip refresh
            s.mouse_move(x, y + 400.0);
        }
    }

    // 2 · The window's own furniture: the title (a tooltip), the bag row toggle, the close button.
    for name in ["BagnonTitle", "BagnonShowBags", "BagnonCloseButton"] {
        if s.eval::<bool>(&format!("return getglobal(\"{name}\") ~= nil"))
            .unwrap_or(false)
        {
            let (x, y) = centre_of(&mut s, name);
            s.mouse_move(x, y);
        }
    }
    click(&mut s, "BagnonShowBags", "LeftButton"); // show the bag row…

    // 3 · Bagnon's OWN bag buttons (the row it just revealed — `BagnonBags0..4` plus the
    //     keyring's `BagnonBags-2`, from `Bagnon.xml`'s `$parent<id>` naming): hover, click and
    //     drag each. `BagnonBag_OnClick` is `PutItemInBackpack`/`PutItemInBag`, `BagnonBag_OnDrag`
    //     is `PickupBagFromSlot` — three engine verbs that did not exist here when this test was
    //     written and are `script/cursor/bag_verbs.rs` now.
    for id in ["0", "1", "2", "3", "4", "-2"] {
        let name = format!("BagnonBags{id}");
        if !s
            .eval::<bool>(&format!(
                "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
            ))
            .unwrap_or(false)
        {
            continue;
        }
        let (x, y) = centre_of(&mut s, &name);
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_move(x + 12.0, y + 12.0);
        s.mouse_button(x + 12.0, y + 12.0, "LeftButton", false);
    }

    // 4 · The money frame's three coin buttons — `OpenCoinPickupFrame`, which Bagnon's own
    //     `BagnonFrameMoney_OnClick` (`Frame.lua:537-543`) calls unguarded, exactly as the
    //     reference's `MoneyFrame.xml` coin buttons do.
    for coin in ["Gold", "Silver", "Copper"] {
        let name = format!("BagnonMoneyFrame{coin}Button");
        if s.eval::<bool>(&format!(
            "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
        ))
        .unwrap_or(false)
        {
            click(&mut s, &name, "LeftButton");
        }
    }

    // 5 · The bag-bar buttons Bagnon hooked on OUR bar — hover, click, and the keyring.
    for name in [
        "MainMenuBarBackpackButton",
        "CharacterBag0Slot",
        "CharacterBag1Slot",
        "KeyRingButton",
    ] {
        if !s
            .eval::<bool>(&format!(
                "local b = getglobal(\"{name}\") return b ~= nil and b:IsVisible()"
            ))
            .unwrap_or(false)
        {
            continue;
        }
        let (x, y) = centre_of(&mut s, name);
        s.mouse_move(x, y);
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_button(x, y, "LeftButton", false);
        s.mouse_move(x, y + 400.0);
    }

    // **A CLOSED LIST, not "is it empty".** Everything Bagnon reaches that benilla does not have
    // is named here, so this test does two jobs at once: it holds the residual honest (each entry
    // is a gap somebody decided, with a reason, not one nobody noticed), and it fails LOUDLY on
    // any error class that is not on it. "Assert the list is empty" would have had to be deleted
    // or `#[ignore]`d while a known gap stands, and then it would be catching nothing at all.
    //
    // To retire an entry: implement it and delete the line. To add one: you had better be able to
    // say why, in the same sentence.
    //
    // **Empty, and that is the list's best state, not a reason to delete it.** Four rows sat here
    // when it was written, and each retired the way the rule above says: the bag-BUTTON cursor
    // trio (`PutItemInBackpack`, `PutItemInBag`, `PickupBagFromSlot` — `Bag.lua:199-215`) once
    // `script/cursor/bag_verbs.rs` bound all three; and money on
    // the cursor (`OpenCoinPickupFrame`, `Frame.lua:537-543`) when the money kit went stock and
    // the reference's own `CoinPickupFrame.xml` joined the manifest with the money cursor under
    // it (decision 1962). With nothing on the list, "unexplained" is every error, which is the
    // assertion this test was always reaching for.
    const KNOWN_GAPS: &[(&str, &str)] = &[];

    let raised = s.errors();
    let unexplained: Vec<&String> = raised
        .iter()
        .filter(|e| !KNOWN_GAPS.iter().any(|(name, _)| e.contains(name)))
        .collect();
    assert!(
        unexplained.is_empty(),
        "using Bagnon raised {} error(s) that are NOT on the known-gap list:\n{:#?}",
        unexplained.len(),
        unexplained
    );
    // …and the other direction, so a gap that quietly closes does not leave a stale entry behind.
    for (name, site) in KNOWN_GAPS {
        assert!(
            raised.iter().any(|e| e.contains(name)),
            "`{name}` is listed as a known gap ({site}) but nothing raised on it — if it is \
             built now, delete the entry"
        );
    }
}

/// Bagnon's window with a **stacked** item in the backpack — the count FontString only paints when
/// the stack is >1 (`SetItemButtonCount`), so the plain [`open_bagnon`] fixture's single Small
/// Brown Pouch can never exercise it.
fn open_bagnon_stacked(root: &Path) -> UiScript {
    let mut s = seat(root, Seat::BeforeAddons);
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
            count: 200,
            item_id: 4496,
            quality: Some(1),
            link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();
    s
}

/// The resolved `QuadContent::Text` for the one named FontString, or a panic naming what was found.
fn text_quad(s: &mut UiScript, owner: &str) -> QuadContent {
    s.resolve();
    s.extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(_), .. })
                && s.target_owner_name(q.target).as_deref() == Some(owner)
        })
        .map(|q| q.content.clone())
        .unwrap_or_else(|| panic!("no text quad owned by {owner}"))
}

/// **The director's report: "item stack size text is missing the black bg."**
///
/// Bagnon writes `<FontString name="$parentCount" font="NumberFontNormal">`
/// (`Bagnon_Core/core/Item.xml:14`) — the `font=` attribute, which the reference resolves against
/// the font-object registry BEFORE the filesystem (`0x783d15 call 0x783870(value, create = 0)`;
/// see [`super::super::loader`]'s `apply_fontstring_font`). We read it as a path only, so the
/// literal string `"NumberFontNormal"` became the face: no height, no colour, and no
/// `outline="NORMAL"` — and the atlas fell back silently to Friz 12.
///
/// **The black is the OUTLINE, not a shadow.** `NumberFontNormal` carries no `<Shadow>` in 1.12
/// (`Fonts.xml:226`, matching the install byte for byte), so `shadow: None` here is correct and is
/// asserted as such — a future "fix" that adds one would be a divergence, not an improvement.
#[test]
fn a_stack_count_wears_the_font_object_its_font_attr_names() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);

    let QuadContent::Text {
        text,
        font,
        font_height,
        outline,
        shadow,
        color,
        ..
    } = text_quad(&mut s, "BagnonItem1")
    else {
        unreachable!("filtered to Text above")
    };

    assert_eq!(text.as_deref(), Some("200"), "the stack count is on screen");
    // Every one of these was lost while `font=` was read as a path. The face is the assertion that
    // the registry was consulted at all; the outline is the one the director can SEE.
    assert_eq!(
        font.as_deref(),
        Some("Fonts\\ARIALN.TTF"),
        "`font=\"NumberFontNormal\"` must resolve the font OBJECT, not become a font path"
    );
    assert_eq!(font_height, Some(14.0), "NumberFontNormal's height");
    assert_eq!(
        outline,
        benilla_ui::script::Outline::Normal,
        "the black ring around a stack count IS `outline=\"NORMAL\"` — this is the reported symptom"
    );
    assert_eq!(
        color,
        Some([1.0, 1.0, 1.0, 1.0]),
        "NumberFontNormal's white"
    );
    assert_eq!(
        shadow, None,
        "and NO drop shadow: NumberFontNormal has no <Shadow> in 1.12 either (Fonts.xml:226). \
         The readability is the outline; adding a shadow here would be a divergence"
    );
}

/// **The director's report: the character dropdown draws as a solid WHITE box.**
///
/// `BagnonPopupFrame` (`Bagnon_Core/core/Frame.xml:23-36`) backs itself with
/// `Interface\ChatFrame\ChatFrameBackground` — art that is white by design — and tints it
/// black→dark-grey with a `<Gradient>`. The loader parsed `<Color>` and `<TexCoords>` but never
/// `<Gradient>`, and dropped it in silence, so the white art rendered untinted.
///
/// `<Gradient>` survives beside a `file=` where `<Color>` does not, and that asymmetry is the
/// point: they land in different fields (vertex colours `+0xb8`, written by `0x77f910`, vs the
/// texture `+0xcc`, written by `0x770360`).
#[test]
fn a_texture_gradient_tints_the_art_it_sits_on() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let bg = s
        .extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                     if p.contains("ChatFrameBackground"))
                && s.target_owner_name(q.target).as_deref() == Some("BagnonDBUICharacterList")
        })
        .map(|q| q.content.clone())
        .expect("the popup's background art reaches the render list");

    let QuadContent::Texture { color, .. } = bg else {
        unreachable!("filtered to Texture above")
    };
    // The two stops are (0,0,0,0.9) and (0.2,0.2,0.2,0.9); `RegionData::gradient` is folded to its
    // midpoint by the paint, so this is 0.1 grey at the stops' shared alpha. `None` — or white —
    // is the bug: untinted ChatFrameBackground is the white slab on the director's screen.
    assert_eq!(
        color,
        Some([0.1, 0.1, 0.1, 0.9]),
        "the <Gradient> must tint the art; untinted this is the reported white box"
    );
}

/// **The director's report: "when I first open the bags with bagnon the gold numbers are all
/// cramped up; if I close and open again it looks good."**
///
/// Bagnon's money display is `SmallMoneyFrameTemplate`, off the chain's own `MoneyFrame.xml`. Its
/// `ShowCoin` used to size each coin with `label:GetStringWidth()`, which is served from the
/// measure round-trip and therefore reads **0 in the tick that set the text**: every coin came out
/// at exactly one icon width and the digits overlapped. The second open looked right because the
/// first open's measure had landed in the cache by then — the reopen was reading the previous open's numbers.
///
/// The fix is the engine's font measurer answering inside the Lua call that asked
/// (`benilla_ui`'s `script::measure`): `GetStringWidth` returns a real number in the tick that set
/// the text. A digits-only stand-in — a per-digit advance feed pushed ahead from the app — shipped
/// first and was retired by the general answer (decision 1285).
///
/// The assertion is the FIRST open, which is the half that was broken.
#[test]
fn a_money_frame_is_the_right_width_on_the_first_open() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // 12345g 67s 89c — three denominations with different digit counts, so a width that ignored
    // the digits (the bug) cannot coincide with one that counts them.
    s.set_money(123_456_789);
    // The app feeds this once per atlas scale, before any window opens. A flat 8px advance makes
    // the expected widths exact arithmetic rather than a font-dependent number.
    s.set_text_measurer(Box::new(super::FixedWidthFont(8.0)));

    s.run("ToggleBackpack()").expect("ToggleBackpack");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let widths: Vec<f32> = s
        .eval(
            "local o = {} \
             for _, n in ipairs({\"Gold\", \"Silver\", \"Copper\"}) do \
               local b = getglobal(\"BagnonMoneyFrame\" .. n .. \"Button\") \
               table.insert(o, b and b:GetWidth() or -1) \
             end \
             return o",
        )
        .expect("the three coin buttons");

    // MONEY_ICON_WIDTH_SMALL is 13; the digits are 8px each. 12345 → 5, 67 → 2, 89 → 2.
    assert_eq!(
        widths,
        vec![5.0 * 8.0 + 13.0, 2.0 * 8.0 + 13.0, 2.0 * 8.0 + 13.0],
        "each coin must be its digits PLUS its icon on the first open. All three coming back at \
         13.0 — the bare icon width — is the reported cramping: it means the width was taken from \
         a text measure that had not landed yet"
    );
}

/// The font a FontString/Button label actually resolved: `(path, height)`.
fn resolved_font(s: &UiScript, lua_expr: &str) -> (String, String) {
    let r: Vec<String> = s
        .eval(&format!(
            "local fs = {lua_expr} \
             if not fs then return {{ \"MISSING\", \"MISSING\" }} end \
             local p, h = fs:GetFont() \
             return {{ tostring(p), tostring(h) }}"
        ))
        .expect("GetFont");
    (r[0].clone(), r[1].clone())
}

/// **A Button's `<NormalFont>`/`<HighlightFont>`/`<DisabledFont>` take `font=` as well as
/// `inherits=`** — they are `<Font>`-TYPED elements, routed by `CSimpleButton::LoadXML 0x7788c0`
/// at `0x778bf4` into the same `0x783c30` a top-level `<Font>` uses, so `font=` gets the identical
/// registry-first treatment (`0x783d15` → `0x783d22 call 0x770c60`).
///
/// We read only `inherits=`, so every corpus button declaring `font=` silently kept **no font
/// object at all**. Bagnon does it at seven sites; `BagnonDBUINameBox` asks for
/// `GameFontNormalLarge` (16px) and got nothing. Real FrameXML always writes `inherits=`, which is
/// why nothing we ship ever noticed.
///
/// `style=` is deliberately not read — it does not exist in 1.12.1 (an isolated-token scan returns
/// zero against nine controls each returning one); it is a later-client idiom.
#[test]
fn a_button_state_font_takes_the_font_attribute_not_just_inherits() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon_stacked(&root);
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }

    assert_eq!(
        resolved_font(
            &s,
            "getglobal(\"BagnonDBUICharacterList1\"):GetFontString()"
        ),
        ("Fonts\\FRIZQT__.TTF".into(), "16".into()),
        "`<NormalFont font=\"GameFontNormalLarge\"/>` must link the font object — 16px, not the \
         12px default, and emphatically not the nil this returned while only `inherits=` was read"
    );
}

/// **A `<FontHeight>` with no `font=` beside it is dead XML**, and so is a bare `outline=`.
///
/// `CSimpleFontString::LoadXML`'s height/outline/monochrome block sits at `[0x77111e, 0x771254)`
/// and its ONLY predecessor is the `font=`-names-a-file miss leg. With no `font=` at all,
/// `0x7710f1`/`0x7710fa je 0x771254` jump straight past it: the attributes are never parsed. They
/// have no independent existence — they are companions of a font FILE path, nothing more.
///
/// The reference's own FrameXML contains exactly one such site, and it is the proof: `ZoneText.xml`
/// gives `AutoFollowStatusText` `inherits="GameFontNormal"` (12px) **and** a
/// `<FontHeight val="20">`. The real client draws that line at 12 and Blizzard's 20 never does
/// anything. We honoured it — one of the few places benilla was drawing text at a size the
/// reference does not.
#[test]
fn a_fontheight_with_no_font_attr_beside_it_is_never_read() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let s = open_bagnon_stacked(&root);
    assert_eq!(
        resolved_font(&s, "AutoFollowStatusText"),
        ("Fonts\\FRIZQT__.TTF".into(), "12".into()),
        "AutoFollowStatusText inherits GameFontNormal (12) and declares <FontHeight val=20> with \
         no font= — the 20 is dead XML in the reference, so 20 here means we are reading an \
         attribute the client never reaches"
    );
}

/// A stand-in font engine for the fixture: every character `PER_CHAR` wide, one line tall. The
/// *numbers* the real engine produces are pinned by `ui_text::atlas::metrics_tests` against the
/// client's own fonts; what these tests are about is that an answer arrives **in the tick that
/// asked**, which is exactly what a fixture with head-arithmetic widths shows clearly.
struct BlockFont;

const PER_CHAR: f32 = 7.0;

impl benilla_ui::script::TextMeasure for BlockFont {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.chars().count() as f32 * PER_CHAR;
        (w, 12.0, w)
    }
}

/// **Bug 4 — the director's narrow dropdown.** `Bagnon_Forever/database/ui.lua:56-61` sizes its
/// character list by setting each row's text and reading the row back in the same tick:
///
/// ```lua
/// button:SetText(player)
/// if button:GetTextWidth() + 40 > width then width = button:GetTextWidth() + 40 end
/// ```
///
/// With `GetTextWidth` served only by the extract-time round-trip that read **0** for every row, so
/// the frame took the `0 + 40` floor and drew 40px wide while seven full-width character names hung
/// out of its right edge — the director's screenshot. The list must now be as wide as its widest
/// name plus the addon's own 40px of checkbox and padding.
#[test]
fn the_character_dropdown_is_as_wide_as_the_names_in_it() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);
    s.set_text_measurer(Box::new(BlockFont));
    // Three saved characters on this realm, the longest 10 letters — Bagnon_Forever's own store.
    s.run(
        "local realm = GetRealmName() \
         BagnonForeverData[realm][\"Onewarrior\"] = { g = 100 } \
         BagnonForeverData[realm][\"Onerogue\"] = { g = 200 }",
    )
    .expect("seed the character store");
    s.run("BagnonDBUI_ShowCharacterList(Bagnon)")
        .expect("BagnonDBUI_ShowCharacterList");
    for _ in 0..3 {
        s.tick(0.1);
    }
    s.resolve();

    let widest = "Onewarrior".len() as f32 * PER_CHAR + 40.0;
    assert_eq!(
        s.eval::<f32>("return BagnonDBUICharacterList:GetWidth()")
            .unwrap(),
        widest,
        "the list sizes itself from its rows' own text width, read in the tick that set it"
    );
    // …and the rows are really inside it: the addon's 6px left inset plus a name, not a 140px
    // template button overhanging a 40px box.
    let (list_r, row_r): (f32, f32) = s
        .eval::<Vec<f32>>(
            "local l = BagnonDBUICharacterList \
             return { l:GetLeft() + l:GetWidth(), BagnonDBUICharacterList1:GetLeft() + \
             BagnonDBUICharacterList1:GetTextWidth() + 24 }",
        )
        .map(|v| (v[0], v[1]))
        .unwrap();
    assert!(
        row_r <= list_r,
        "a row's text must end inside the list: text right edge {row_r} > list right edge {list_r}"
    );
}

/// **Bug 5 — the director's stale offline bags.** Bagnon_Forever mirrors the live bags into
/// `BagnonForeverData` (its saved variable) so the dropdown can show other characters' inventories
/// offline. Its recorder does a full scan only on a character's FIRST login; afterwards it trusts
/// `BAG_UPDATE(bagID)`, and `SaveBagData` **deletes** a bag's record whenever
/// `GetContainerNumSlots(bag)` reads 0. The feed used to hand it exactly that: on the logout
/// despawn frame the self store is gone, and diffing the absence as an all-empty snapshot fired a
/// size-0 `BAG_UPDATE` for every bag — every record erased seconds before the saved-variables
/// write, leaving each recently-logged-out character money-only ("g" has no delete path) and the
/// offline view stale. Here: the login scan records the live backpack, and the record must still
/// be intact after the absent-source frame and the logout events — what the shutdown write
/// actually persists. The mechanism's own law is pinned beside the feed
/// (`ui_items::feed::tests::an_absent_self_player_is_no_source_never_an_empty_bag_burst`).
#[test]
fn bagnon_forevers_records_survive_the_logout_boundary() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = seat(&root, Seat::BeforeAddons);

    // The first-login scan (PLAYER_LOGIN, inside `seat`) recorded the live backpack: 16 slots,
    // the pouch in slot 1, in Bagnon_Forever's own short-link shape.
    //
    // **`16,1,` — the 1 is the reference's, not a stray.** The recorder's count is
    // `GetInventoryItemCount("player", ContainerIDToInventoryID(0))`, and `ContainerIDToInventoryID`
    // has no backpack arm (`0x4f94e0`: `id - 1 < 4` → `+20`), so bag 0 asks inventory slot 19 —
    // the tabard slot. `GetInventoryItemCount 0x4c8680` pushes **1** for an empty slot
    // (`0x4c8797`), and 1 for a
    // worn tabard's stack count too, so the real client writes this row as `16,1,` either way.
    let record = "local r = BagnonForeverData[GetRealmName()][UnitName('player')] \
                  return tostring(r[0] and r[0].s), tostring(r[0] and r[0][1])";
    let (size, item) = s
        .eval::<(String, String)>(record)
        .expect("the record reads");
    assert_eq!(
        size, "16,1,",
        "the login scan records the backpack's size row"
    );
    assert_eq!(
        item, "4496",
        "the login scan records the occupied slot's short link"
    );

    // An equipped bag arriving on the feed — inventory surface FIRST (the bag item in INV slot
    // 20), then the container push and its `BAG_UPDATE`: the order the schedule guarantees
    // (`feed_char.before(feed_containers)`, ui_char.rs). The recorder's size row asks the bag
    // ITEM's own count and link (`GetInventoryItemLink("player", ContainerIDToInventoryID(1))`);
    // raced the other way it recorded every equipped bag linkless — the `s = "8,0,"` rows in the
    // director's data.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[20] = Some(benilla_ui::script::InvSlotView {
        item_id: 4497,
        count: 1,
        quality: 1,
        name: Some("Small Green Pouch".into()),
        link: Some("|cffffffff|Hitem:4497:0:0:0|h[Small Green Pouch]|h|r".into()),
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_container(
        1,
        Some(ContainerState {
            name: Some("Small Green Pouch".into()),
            num_slots: 6,
            slots: Default::default(),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(1)]);
    let bag_row = s
        .eval::<String>(
            "return tostring(BagnonForeverData[GetRealmName()][UnitName('player')][1].s)",
        )
        .expect("the bag row reads");
    assert_eq!(
        bag_row, "6,1,4497",
        "an equipped bag's size row carries its own count and short link"
    );

    // The logout boundary: the despawn frame (no self store) and then the shutdown's own events.
    // The record the write persists must be the bags the player actually had.
    let mut memory = crate::ui_items::feed::FeedMemory::default();
    crate::ui_items::feed::apply_container_source(
        &mut s,
        &mut memory,
        None,
        Default::default(),
        Vec::new(),
        Vec::new(),
    );
    s.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
    s.fire_event("PLAYER_LOGOUT", Vec::new());
    let (size, item) = s
        .eval::<(String, String)>(record)
        .expect("the record reads");
    assert_eq!(
        size, "16,1,",
        "the logout boundary must not erase the size row"
    );
    assert_eq!(item, "4496", "the logout boundary must not erase the slot");
}

/// **Bug 6 — the director's "smaller bags open their own little bags".** Bagnon's whole
/// interception model is the global override (`Bagnon_Core/core/Overrides.lua`: `ToggleBag = …`)
/// plus a `SetScript("OnClick")` wrap on our bar buttons whose replacement calls the ORIGINAL
/// handler back for a plain click. Both layers end at whatever the original OnClick does — and
/// ours toggled `BenillaBagFrame<N>` directly instead of calling `ToggleBag`, so every slot-button
/// click walked straight past both of Bagnon's hooks and opened the native window (the backpack
/// button was fixed in an earlier pass; the four slots were not). The ref's own
/// `BagSlotButton_OnClick` (MainMenuBarBagButtons.lua l.3-21) calls the GLOBAL `ToggleBag`, and
/// its checked tail scans only the NATIVE ContainerFrames — so with Bagnon holding the bags a
/// slot click toggles Bagnon's one window and the slot button stays unlit. Both halves pinned
/// here with a real mouse click, an equipped bag in slot 2, and Bagnon fully hooked.
#[test]
fn a_bag_slot_click_toggles_bagnon_not_the_native_window() {
    // The install too: "is a native bag window open" is asked of the reference's own `IsBagOpen`
    // scan (`ContainerFrame.lua:177`, off the chain) — the windows are RECYCLED (1751), so there
    // is no frame to name, and `bag_tests.rs` asks the same way.
    let root = corpus_and_install_or_skip!();
    let mut s = open_bagnon(&root);

    // An equipped bag behind CharacterBag1Slot (bagId 2): the inventory surface (the bag ITEM in
    // inv slot 21) and its container — both fed before any click, the schedule's own order.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[21] = Some(benilla_ui::script::InvSlotView {
        item_id: 4497,
        count: 1,
        quality: 1,
        name: Some("Small Green Pouch".into()),
        link: Some("|cffffffff|Hitem:4497:0:0:0|h[Small Green Pouch]|h|r".into()),
        icon: Some("Interface\\Icons\\INV_Misc_Bag_10_Green".into()),
        ..Default::default()
    });
    s.set_inventory_slots(inv);
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Green Pouch".into()),
            num_slots: 6,
            slots: Default::default(),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(2)]);
    s.resolve();

    // Bagnon is open (the fixture); a REAL click on the slot button must CLOSE Bagnon — the
    // toggle routed through the override — and must never show the native window.
    // Both interception layers must be live, and the click exercises both: the SetScript wrap
    // (Bagnon.lua's Bagnon_AddBagHooks — its replacement calls our CAPTURED original back with
    // no args, so `self` inside the XML wrapper must still resolve on the re-entrant call) and
    // the global ToggleBag override the original then reaches.
    assert!(
        s.eval::<bool>("return CharacterBag1Slot:GetScript('OnClick') == BagnonBlizBag_OnClick")
            .unwrap_or(false),
        "Bagnon's SetScript OnClick wrap must be in place on the slot button"
    );
    let (x, y) = centre_of(&mut s, "CharacterBag1Slot");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        !s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the slot click must reach Bagnon's ToggleBag override and close its window"
    );
    assert!(
        !super::test_ui::bag_open(&s, 2),
        "the native bag window must NOT open — the click belongs to the addon's override"
    );

    // Click again: Bagnon back open, native still shut, and the slot button unlit — the ref's
    // checked tail reads only the native window (its ContainerFrame scan), which Bagnon never
    // shows.
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    for _ in 0..2 {
        s.tick(0.1);
    }
    assert!(
        s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the second click re-opens Bagnon through the same override"
    );
    assert!(
        !super::test_ui::bag_open(&s, 2),
        "the native window stays shut on the re-open too"
    );
    assert!(
        !s.eval::<bool>("return CharacterBag1Slot:GetChecked() and true or false")
            .unwrap(),
        "the slot button stays unlit with an addon holding the bags (the ref scan is native-only)"
    );
    // Nothing is filtered out any more: the bag-button cursor trio the round-trip reaches
    // (`PutItemInBag` on the click) is built (`script/cursor/bag_verbs.rs`), so a raise on it is
    // a real one.
    let raised = s.errors();
    assert!(
        raised.is_empty(),
        "the click round-trip raised: {raised:#?}"
    );
}

/// **Bug 7 — the director's unlit backpack button — and where it stands since the bag bar went
/// stock.** Bagnon lights `MainMenuBarBackpackButton` from its window's OnShow/OnHide
/// (`Bagnon.lua:33/39` — `SetChecked(1/0)`), and OnShow dispatch is synchronous
/// (`0x775750` → `0x76ae10`), so whoever writes checked AFTER the toggle wins. On the
/// reference's own click path the stock `BackpackButton_OnClick` tail
/// (`MainMenuBarBagButtons.lua:55-69`) runs last: it re-derives checked by scanning the NATIVE
/// container frames, finds none for bag 0 while an addon holds the bags, and writes 0 — so the
/// real client leaves the button unlit after a click with Bagnon, a wart of its own. Our own
/// `BagFrame.xml` once closed that wart on the director's call (Bug 7: an auto-toggle undo in
/// place of the scan, a stated divergence); decision 1783 sourced the stock file unedited and the
/// divergence went with it, unrecorded — and this suite, skipping at every land from 08-30
/// (2329), never said so.
///
/// What this pins is the client as it is, which is the reference: the bare `ToggleBackpack()`
/// path (the `B` binding's body) ends lit, because nothing runs after OnShow there; a real click
/// ends **unlit** on open as well as on close, because the stock tail is the last word. Decision
/// 2332 retired Bug 7's divergence on record (1751 §2 reserves shims for engine differences, and
/// this is a look); if the director ever wants it lit, the second assertion below flips back and
/// a shim in `ContainerFrameAdapters.xml` carries the why.
#[test]
fn the_backpack_button_lit_state_with_bagnon_holding_the_bags() {
    benilla_formats::wow_data_or_skip!();
    let root = corpus_or_skip!();
    let mut s = open_bagnon(&root);

    // The fixture opened Bagnon through the bare global (the binding's own body): OnShow's
    // SetChecked(1) must be standing — nothing runs after it on that path.
    assert!(
        s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "the bare ToggleBackpack() path (the B binding) must leave the button lit"
    );

    // A REAL click on the button: auto-toggle flips, the wrapper's undo repairs it, Bagnon's
    // OnHide writes the truth — closed and unlit.
    let (x, y) = centre_of(&mut s, "MainMenuBarBackpackButton");
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        !s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the click must toggle Bagnon closed through its override"
    );
    assert!(
        !s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "closed ⇒ unlit (Bagnon's OnHide write is the last word)"
    );

    // And the second click: open again — and UNLIT, the stock tail's native scan overwriting
    // Bagnon's OnShow write, as the reference does. Bug 7's "lit" is the assertion this replaces;
    // it comes back with the shim if the director asks for it (the doc above).
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    assert!(
        s.eval::<bool>("return Bagnon:IsVisible() or false")
            .unwrap(),
        "the second click re-opens Bagnon"
    );
    assert!(
        !s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap(),
        "open by click ⇒ unlit: the stock BackpackButton_OnClick tail's native scan is the last \
         word, as on the reference (Bug 7's divergence retired with the stock bag bar, 1783)"
    );
}
