//! **The tests' interface loader — one copy, both stores**.
//!
//! Half a dozen test files carried an identical private `load_xml` that read
//! `assets/ui/<name>` off disk, parsed it, and asserted the loader reported no errors — each with
//! the comment "duplicated so this file is self-contained". That was fine while `assets/ui` was
//! the only place an interface file could come from. It is not any more: a migrated window's file
//! lives in the player's own patch chain, so a test that wants bag windows has to name
//! `Interface\FrameXML\ContainerFrame.xml`, and six copies of a disk-only reader cannot say that.
//!
//! [`load_ui`] is that reader, generalised the same way [`super::manifest::load_manifest`] is:
//! **a bare filename is a file we ship, a path is one off the player's install.** The two are told
//! apart by [`super::reference_ui::is_chain_entry`], the manifest's own rule, so a test loads the
//! entries in the order `benilla.toc` names them and gets what the client gets.
//!
//! It reads the SOURCE TREE rather than the compiled-in copy, deliberately: these tests exist to
//! catch a mistake in a file somebody just edited, and `content::read`'s dev-build probe already
//! prefers disk for the same reason.

use benilla_ui::script::{QuadContent, UiScript};

/// Load one interface file into `s`, panicking on any loader error, and return how many frames it
/// materialized (`0` for a `.lua` entry, which materializes none).
///
/// `entry` is a manifest entry: `"BagFrame.xml"` for one of ours, or
/// `"Interface\\FrameXML\\ContainerFrame.xml"` for the reference's own — which also brings its
/// `<Script file="ContainerFrame.lua"/>` off the chain, exactly as it does in a real load.
///
/// **A chain entry needs client data**, so a test that names one has to open with
/// `benilla_formats::wow_data_or_skip!()`; [`BAG_UI`] is a list that always does.
pub(crate) fn load_ui(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, false, false)
}

/// The cooldown indicator's file: `Interface\Cooldown\UI-Cooldown-Indicator.mdx`, as the
/// stock `CooldownFrameTemplate` names it.
pub(super) const COOLDOWN_MODEL: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";

/// The cooldown indicator's file facts (`benilla-extract m2seq`): sequence 0 = id 0, 1000 ms,
/// clamp — the sweep `CooldownFrame_OnUpdateModel` scrubs; sequence 1 = id 1, 1000 ms, clamp —
/// the finish flash, whose completion hides the frame. Handed to the engine the way the app does
/// once the asset lands; every pane holding the file arms its Stand.
pub(super) fn cooldown_facts(s: &mut UiScript) {
    use benilla_ui::widget::{ModelFileFacts, SequenceFacts};
    let seq = |anim_id, duration_ms| SequenceFacts {
        anim_id,
        duration_ms,
        looping: false,
    };
    s.set_model_facts(
        COOLDOWN_MODEL,
        ModelFileFacts {
            sequences: vec![seq(0, 1000), seq(1, 1000)],
            bbox: ([0.0; 3], [0.0; 3]),
            cameras: 0,
        },
    );
}

/// The play head `(anim_id, cursor_ms)` of the SHOWN cooldown pane `owner` names (`None` while
/// it is hidden, or has nothing armed) — what the tile renderer samples the file at. Read the way
/// the renderer reads it: the pane's quad in the extract joined to the engine's paint list.
pub(super) fn cooldown_play(s: &UiScript, owner: &str) -> Option<(u16, u32)> {
    let heads = s.visible_model_panes();
    s.extract().into_iter().find_map(|q| match &q.content {
        QuadContent::ModelPane {
            handle,
            model: Some(m),
            ..
        } if m.eq_ignore_ascii_case(COOLDOWN_MODEL)
            && s.quad_owner_name(q.target).as_deref() == Some(owner) =>
        {
            heads
                .iter()
                .find(|p| p.handle == *handle)
                .and_then(|p| p.play.map(|ph| (ph.anim_id, ph.cursor_ms)))
        }
        _ => None,
    })
}

/// [`cooldown_play`] for whichever cooldown pane is shown — the bag windows number their slots
/// from the far end (`ContainerFrame_GenerateFrame`: `Item{j}` carries id `size − j + 1`), so a
/// test that seats one item asks for "the" sweep rather than a name.
pub(super) fn cooldown_play_any(s: &UiScript) -> Option<(u16, u32)> {
    let heads = s.visible_model_panes();
    s.extract().into_iter().find_map(|q| match &q.content {
        QuadContent::ModelPane {
            handle,
            model: Some(m),
            ..
        } if m.eq_ignore_ascii_case(COOLDOWN_MODEL) => heads
            .iter()
            .find(|p| p.handle == *handle)
            .and_then(|p| p.play.map(|ph| (ph.anim_id, ph.cursor_ms))),
        _ => None,
    })
}

/// [`load_ui`], and **a missing template is a failure too**.
///
/// A frame that inherits a template nothing declares is a loader *warning*, not an error: the frame
/// is built and simply has none of the template's art. So an under-loaded dependency list passes
/// [`load_ui`] and then loses a window's whole skin silently — which is why four of the social
/// windows' test modules grew this check by hand. It is one function now rather than four copies,
/// and any test may ask for it.
pub(super) fn load_ui_strict(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, true, false)
}

/// [`load_ui`], and **no loader warning of any kind is tolerated**.
///
/// Stricter than [`load_ui_strict`], which only fails a missing template. A file whose own
/// assignment is "this loads perfectly clean" wants the whole warnings channel asserted empty — a
/// stale unknown-attribute or dropped-script warning is exactly the drift that check exists to
/// catch. It was a fourth private disk-only reader in `group_loot_tests.rs` until 1838; a
/// disk-only reader cannot name a chain file, which is what that test now loads.
pub(super) fn load_ui_no_warnings(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, false, true)
}

fn load_entry(s: &UiScript, entry: &str, strict_templates: bool, no_warnings: bool) -> usize {
    // **A kit's VM is the client's VM, and that includes the CVar table**. The app
    // registers `crate::cvars::REGISTERED` at startup, before any interface file loads;
    // `UiScript::new()` carries only `benilla-ui`'s own. The stock `UIOptionsFrame.xml` reads two
    // camera CVars inside its dropdowns' `OnLoad` and raises on a nil, so a kit that skips this is
    // measuring a client that does not exist. Idempotent, and it never clobbers a value a test set
    // first — `register_cvars` only refreshes an existing slot's default.
    s.register_cvars(crate::cvars::registered_pairs());
    let path = entry.replace('\\', "/");
    let bytes = read(&path).unwrap_or_else(|| panic!("{entry}: not found"));
    if path.to_ascii_lowercase().ends_with(".lua") {
        s.run_chunk_named(&bytes, &format!("@{entry}"))
            .unwrap_or_else(|e| panic!("{entry}: {e}"));
        return 0;
    }
    let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
        .unwrap_or_else(|e| panic!("{entry}: {e}"));
    let provider = |req: &str| -> Option<Vec<u8>> { read(req) };
    // Seated BEFORE the load, not after it like the two below: `MultiActionBarFrame_OnLoad`
    // indexes `UIOptionsFrameCheckButtons` from inside this very load walk, where the micro row's
    // and UIParent's callees only run later.
    if path
        .rsplit('/')
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case("MultiActionBars.xml"))
    {
        s.run(MULTI_ACTION_BAR_STAND_INS)
            .expect("the multibar stand-ins");
    }
    // **Our options window's Graphics rows read the REFERENCE's slider table, at OnLoad**:
    // `OptionsFrameSliders[1..3]` are the bounds those three rows are built with,
    // and they come from `Interface\FrameXML\OptionsFrame.lua` — which the shipped manifest loads
    // as part of the stock VIDEO window, one seat above ours. A kit that seats our file alone has
    // to bring it too, and brings the reference's own file rather than a transcription of the
    // three rows: a stand-in here would be a second copy of numbers whose whole point is that
    // they are no longer ours. Seated BEFORE the load, for the same reason MultiActionBars is:
    // the rows index it from inside this very load walk.
    if path
        .rsplit('/')
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case("OptionsFrame.xml"))
        && !super::reference_ui::is_chain_entry(&path)
    {
        let bytes = read("Interface/FrameXML/OptionsFrame.lua")
            .expect("the reference's own OptionsFrame.lua");
        s.run_chunk_named(&bytes, "@Interface\\FrameXML\\OptionsFrame.lua")
            .expect("the video window's slider table");
    }
    let report = benilla_ui::loader::load_in(s, &doc, &path, &provider);
    assert!(
        report.errors.is_empty(),
        "{entry}: loader errors: {:?}",
        report.errors
    );
    let leaf = path.rsplit('/').next().unwrap_or("");
    if leaf.eq_ignore_ascii_case("MainMenuBarMicroButtons.xml") {
        s.run(MICRO_BUTTON_STAND_INS)
            .expect("the micro-button stand-ins");
    }
    if leaf.eq_ignore_ascii_case("UIParent.xml") && super::reference_ui::is_chain_entry(&path) {
        s.run(UIPARENT_STAND_INS).expect("the UIParent stand-ins");
    }
    if leaf.eq_ignore_ascii_case("UIOptionsFrame.xml") {
        s.run(UIOPTIONS_STAND_INS)
            .expect("the stock options window's stand-ins");
    }
    if no_warnings {
        assert!(
            report.warnings.is_empty(),
            "{entry}: loader warnings: {:?}",
            report.warnings
        );
    }
    if strict_templates {
        let missing: Vec<&String> = report
            .warnings
            .iter()
            .filter(|w| w.contains("unknown template"))
            .collect();
        assert!(
            missing.is_empty(),
            "{entry}: inherits a template this house does not ship (the frame loads, its ART does \
             not): {missing:?}"
        );
    }
    report.frames
}

/// **What a kit owes the stock micro-button row.** `UpdateMicroButtons`
/// (`MainMenuBarMicroButtons.lua:20-84`) reads ten panels, `KeyRingButton` and `IsBagOpen`
/// UNGUARDED — the reference's own contract, because in the client every one of them is up
/// before anything can be shown. A kit is a prefix of the manifest and stops wherever its window
/// does, so [`load_entry`] wraps the function the moment the row loads: on its FIRST call — a
/// panel's OnShow, by which time the kit is complete — the wrapper seats a hidden, unnamed
/// stand-in under each name the kit never declared (a hidden frame is what a closed panel
/// answers), then runs the reference's own body. First use rather than load time, and unnamed,
/// because both registries keep the first writer: the arena's name map
/// (`named_registry_is_non_overwriting`) and `_G` itself — a named frame is published
/// non-overwriting (`0x701bd0`: a `lua_gettable` nil check on GLOBALSINDEX, which is why a
/// metatable fallback would block the real publish just the same). A stand-in seated before the
/// real declaration shadows the real frame for good; the first cut did exactly that, 126
/// failures, every tabbed panel's `numTabs` read off the stand-in. `fire_chat_login`'s
/// `UIOptionsFrame` stand-in is the same move for the chat files. Seated here rather than at the
/// kits' seventy-odd consumers because the dependency is the row's, not any one window's — and
/// the shipped manifest never needs it. `tests/common/mod.rs` carries the same chunk for the
/// integration tests, which cannot reach this module.
/// **What a kit owes the stock `UIParent.xml`.** Its `<OnUpdate>` calls `FCF_OnUpdate`,
/// `UnitPopup_OnUpdate` and `BattlefieldFrame_OnUpdate` unguarded (the chat, unit-menu and
/// battlefield files, far below it in the manifest), `UIParent_OnEvent`'s `PLAYER_ENTERING_WORLD`
/// arm calls `MultiActionBar_Update`, and `ShowUIPanel` calls `CloseAllBags` (the container file's).
/// A kit that stops short of those files would raise on its first tick or first shown panel, so
/// [`load_entry`] seats a no-op stand-in under each name the moment the stock file loads. Unlike
/// the micro row's frames these are FUNCTIONS: a later chunk's `function X()` overwrites a global
/// outright, so seating at load is safe and the kit's order does not matter.
pub(super) const UIPARENT_STAND_INS: &str = r#"
    -- Callees of the stock UIParent.xml's <OnUpdate> and of UIParent_OnEvent's arms that live in
    -- files a kit may stop short of, plus the bag verbs the stock ShowUIPanel calls and the two
    -- container constants its window walks read: no-op stand-ins, each overwritten by the real
    -- definition when its file loads (a chunk's `function X()` is a plain global write, unlike a
    -- frame's non-overwriting publish — which is why the FRAMES below are seated on first use).
    FCF_OnUpdate = FCF_OnUpdate or function() end
    FCF_DockUpdate = FCF_DockUpdate or function() end
    UnitPopup_OnUpdate = UnitPopup_OnUpdate or function() end
    BattlefieldFrame_OnUpdate = BattlefieldFrame_OnUpdate or function() end
    MultiActionBar_Update = MultiActionBar_Update or function() end
    RaidOptionsFrame_UpdatePartyFrames = RaidOptionsFrame_UpdatePartyFrames or function() end
    LocalizeFrames = LocalizeFrames or function() end
    updateContainerFrameAnchors = updateContainerFrameAnchors or function() end
    -- 1.12 keeps UpdateNameplates in UIOptionsFrame.lua, which a kit reaches only at manifest
    -- l.21; our own OptionsFrame.xml re-declares it below that (decision 2132). Both are plain
    -- `function X()` writes, so a full kit ends on ours and a short one keeps this no-op.
    UpdateNameplates = UpdateNameplates or function() end
    CloseAllBags = CloseAllBags or function() end
    OpenBackpack = OpenBackpack or function() end
    CloseBackpack = CloseBackpack or function() end
    NUM_CONTAINER_FRAMES = NUM_CONTAINER_FRAMES or 0
    -- The reference's own initial values (`ContainerFrame.lua:11-12`), not zeroes: the tooltip's
    -- default corner is `-CONTAINER_OFFSET_X - 13, CONTAINER_OFFSET_Y`, so a kit reading zero here
    -- would seat every default-anchored plate 70 units low.
    CONTAINER_OFFSET_X = CONTAINER_OFFSET_X or 0
    CONTAINER_OFFSET_Y = CONTAINER_OFFSET_Y or 70
    BATTLEFIELD_TAB_OFFSET_Y = BATTLEFIELD_TAB_OFFSET_Y or 210
    -- The pass writes its `isVar` rows with `setglobal`, but only for a name that already reads
    -- non-nil (`frame = getglobal(index); if frame then`), so an unseeded one is never written at
    -- all. These four are the reference's own seeds, from the files that declare them
    -- (ContainerFrame.lua, PetActionBarFrame.lua, WorldStateFrame.lua).
    PETACTIONBAR_YPOS = PETACTIONBAR_YPOS or 98
    PETACTIONBAR_XPOS = PETACTIONBAR_XPOS or 36

    -- The frames these three read UNGUARDED, seated on the call rather than at load: a frame's
    -- publish to _G is non-overwriting (0x701bd0), so a stand-in seated before the real file loads
    -- would shadow the real window for good.
    local function benilla_seat(names)
        for _, name in ipairs(names) do
            if not getglobal(name) then local f = CreateFrame("Frame") f:Hide() setglobal(name, f) end
        end
    end
    local real_manage = UIParent_ManageFramePositions
    function UIParent_ManageFramePositions()
        benilla_seat({ "MainMenuBar", "MultiBarLeft", "MultiBarRight", "MultiBarBottomLeft",
            "PetActionBarFrame", "ShapeshiftBarFrame", "ReputationWatchBar", "MainMenuExpBar",
            "MainMenuBarMaxLevelBar", "CastingBarFrame", "QuestTimerFrame", "QuestWatchFrame",
            "DurabilityFrame", "DurabilityShield", "DurabilityOffWeapon", "DurabilityRanged",
            "MinimapCluster", "ChatFrame1", "ChatFrame2", "ShapeshiftBarLeft",
            "ShapeshiftBarMiddle", "ShapeshiftBarRight", "BattlefieldMinimapTab" })
        -- …and every FRAME the managed table itself names (`frame:IsObjectType` on a nil is what
        -- a kit missing one raises) — read off the table, so a row added there needs nothing here.
        -- The `isVar` rows are skipped: those keys are global NUMBERS the pass writes
        -- (CONTAINER_OFFSET_X/Y, PETACTIONBAR_YPOS…), and a frame seated under one of those names
        -- would be arithmetic's problem two files later.
        local named = {}
        for name, row in pairs(UIPARENT_MANAGED_FRAME_POSITIONS) do
            if not row.isVar then table.insert(named, name) end
            -- …and every row's ANCHOR TARGET (`anchorTo`, l.1668), which is a different set: the
            -- keys are the frames being MOVED, the targets are what they move relative to, and
            -- several targets (`ActionButton1`, `MainMenuBarArtFrame`) are declared in files a
            -- one-window kit never loads. An unresolvable name is a RAISE now (decision 2176), so
            -- a target the kit is missing aborts `UIParent_ManageFramePositions` mid-pass where it
            -- used to anchor to the parent and carry on.
            if row.anchorTo then table.insert(named, row.anchorTo) end
        end
        benilla_seat(named)
        for _, name in ipairs({ "SlidingActionBarTexture0", "SlidingActionBarTexture1" }) do
            if not getglobal(name) then setglobal(name, UIParent:CreateTexture()) end
        end
        return real_manage()
    end
    -- The four options/menu windows `IsOptionFrameOpen` (l.997) and `ToggleGameMenu` (l.1467)
    -- index unguarded. `IsOptionFrameOpen` is on the path of every window close, so a kit that
    -- loads no options window raised on the first bag click. In the shipped manifest all four
    -- names are real, and since 2177 all three options windows are the REFERENCE's own files,
    -- loaded hidden — including `OptionsFrame`, the video window, which used to be our own
    -- window's name. Ours is `BenillaOptionsFrame` now and is not in this list: it is not a name
    -- the reference indexes, and the wrappers in `GameMenuFrame.xml` are what tell these two
    -- functions about it. A KIT is a prefix of the manifest and may load none of the four, which
    -- is what these stand-ins are for.
    local function benilla_seat_options()
        benilla_seat({ "GameMenuFrame", "OptionsFrame", "UIOptionsFrame", "SoundOptionsFrame" })
        if not OptionsFrameCancel then
            OptionsFrameCancel = CreateFrame("Button")
            OptionsFrameCancel:Hide()
        end
    end
    local real_option_open = IsOptionFrameOpen
    function IsOptionFrameOpen()
        benilla_seat_options()
        return real_option_open()
    end
    local real_toggle_menu = ToggleGameMenu
    function ToggleGameMenu(clicked)
        benilla_seat_options()
        return real_toggle_menu(clicked)
    end
"#;

/// **What a kit owes the stock `MultiActionBars.xml`.** Its `MultiActionBarFrame_OnLoad`
/// (`MultiActionBars.lua:10`, under the reference's own comment *"Hack to get around load order
/// dependencies"*) writes five rows into `UIOptionsFrameCheckButtons` — a table whose home is
/// `UIOptionsFrame.xml`, which the reference's toc seats at l.21, eighteen rows above this one.
/// The shipped manifest has that order and needs nothing here; a KIT is a prefix
/// of the manifest and dozens of them load the bars without any options window.
///
/// `or {}` rather than a fresh table, and seated at load rather than on first use, for the reason
/// 1988 gives: a table, like a function, is a plain global write, so a real declaration that lands
/// afterwards simply wins — and in manifest order it never lands afterwards, which is exactly what
/// the shipped `UIOptionsFrameCheckButtons` order proof asserts. A kit that loads both in manifest
/// order therefore gets the real table with the bars' five rows in it, which is the client's own
/// state.
/// **What a kit owes the stock `UIOptionsFrame.xml`.** Its `UIOptionsFrame_OnEvent`'s
/// VARIABLES_LOADED arm (`UIOptionsFrame.lua` l.193-227) is the reference's own load-time ladder —
/// the one decision 2115 retired our re-expression of — and it calls six functions unguarded:
/// `BuffButtons_UpdatePositions`, `FCF_Set_SimpleChat`, `FCF_Set_ChatLocked`,
/// `SetChatMouseOverDelay`, `MultiActionBar_ShowAllGrids` and
/// `RaidOptionsFrame_UpdatePartyFrames`. In the shipped manifest that is fine twice over: the
/// buff bar loads far above this row, the chat and raid files far below it, and the event does not
/// fire until every one of them has loaded. A KIT is a prefix of the manifest and fires the event
/// itself, so it needs the names to exist.
///
/// Functions, so load-time seating is safe and the kit's order does not matter: a later chunk's
/// `function X()` is a plain global write that overwrites the stand-in, unlike a frame's
/// non-overwriting publish (1988's reasoning, and its precedent).
pub(super) const UIOPTIONS_STAND_INS: &str = r#"
    BuffButtons_UpdatePositions = BuffButtons_UpdatePositions or function() end
    FCF_Set_SimpleChat = FCF_Set_SimpleChat or function() end
    FCF_Set_NormalChat = FCF_Set_NormalChat or function() end
    FCF_Set_ChatLocked = FCF_Set_ChatLocked or function() end
    SetChatMouseOverDelay = SetChatMouseOverDelay or function() end
    MultiActionBar_ShowAllGrids = MultiActionBar_ShowAllGrids or function() end
    RaidOptionsFrame_UpdatePartyFrames = RaidOptionsFrame_UpdatePartyFrames or function() end
    -- …and the one its own CHECK BUTTONS reach: three of them register VARIABLES_LOADED for
    -- themselves (xml l.373, 925, 993), and CheckButton43's arm calls `PartyFrame.lua`'s
    -- `UpdatePartyMemberBackground`. The other two call the file's own dropdown loaders.
    UpdatePartyMemberBackground = UpdatePartyMemberBackground or function() end
"#;

pub(super) const MULTI_ACTION_BAR_STAND_INS: &str = r#"
    UIOptionsFrameCheckButtons = UIOptionsFrameCheckButtons or {}
    -- The five ROWS it writes into, not just the table: the reference's hack assigns
    -- `UIOptionsFrameCheckButtons["SHOW_MULTIBAR1_TEXT"].setFunc`, which needs the row to exist.
    -- Empty rows on purpose — the real ones carry `index = 33..36, 40`, and a stand-in that
    -- restated those numbers would be a transcription of the reference's table in a test helper,
    -- which is the thing 2115 deleted from `OptionsFrame.xml`. Any kit that reads an index has the
    -- real window loaded and therefore the real table.
    for _, key in ipairs({ "SHOW_MULTIBAR1_TEXT", "SHOW_MULTIBAR2_TEXT", "SHOW_MULTIBAR3_TEXT",
                           "SHOW_MULTIBAR4_TEXT", "ALWAYS_SHOW_MULTIBARS_TEXT" }) do
        UIOptionsFrameCheckButtons[key] = UIOptionsFrameCheckButtons[key] or {}
    end
"#;

pub(super) const MICRO_BUTTON_STAND_INS: &str = r#"
    local real = UpdateMicroButtons
    function UpdateMicroButtons()
        for _, name in ipairs({ "CharacterFrame", "SpellBookFrame", "QuestLogFrame", "GameMenuFrame",
            "OptionsFrame", "SoundOptionsFrame", "UIOptionsFrame", "FriendsFrame", "WorldMapFrame",
            "HelpFrame" }) do
            if not getglobal(name) then local f = CreateFrame("Frame") f:Hide() setglobal(name, f) end
        end
        if not KeyRingButton then KeyRingButton = CreateFrame("Button") KeyRingButton:Hide() end
        if not KEYRING_CONTAINER then KEYRING_CONTAINER = -2 end
        if not IsBagOpen then function IsBagOpen() return nil end end
        return real()
    end
"#;

/// One file's bytes, from whichever store its path names — the chain for a path, this crate's
/// `assets/ui` for a bare name. Also the `<Include>` / `<Script file=>` provider, which is why it
/// takes an already-resolved path in either space.
pub(super) fn read(req: &str) -> Option<Vec<u8>> {
    if super::reference_ui::is_chain_entry(req) {
        return super::reference_ui::read(req);
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    std::fs::read(dir.join(req)).ok()
}

/// The files a test needs before it can open a bag window, in manifest order: the four templates
/// stock `ContainerFrame.xml` inherits, then the reference's own file, then our bag bar.
///
/// Named as a list rather than folded into a `setup()` because the callers all want to add their
/// own files around it (a merchant, the bank, the action bar art), and the ORDER is the thing
/// being reused — it is `benilla.toc`'s, trimmed to what the bags actually reach for.
/// What the **loot window** needs before `Interface\FrameXML\LootFrame.xml` will load and behave —
/// the same shape as [`BAG_UI`], for the same reason, and it grew for 1751 exactly as that one did.
///
/// Three of these are load-bearing in a way that is invisible if you leave them out:
///
/// * **`GlobalStrings.lua`** — the stock file concatenates `GROUP` and `GIVE_LOOT` while building
///   the master-loot menu. The app loads the player's own copy ahead of the whole manifest at VM
///   setup; a bare `UiScript::new()` does not. Our deleted `LootFrame.xml` carried
///   `X = X or "…"` fallbacks for precisely these; the stock file carries none.
/// * **`ItemButtonTemplate.xml`** — stock `LootButtonTemplate` inherits it, and a missing template
///   is a loader *warning*, not an error. Leave it out and every row loads clean with no art and
///   no `$parentIconTexture`, which reads as a pass until an assertion looks for an icon.
/// * **`PartyFrame.xml`** — for `MAX_PARTY_MEMBERS`, which stock `LootFrame.lua:217` does
///   arithmetic on at LOAD time, not at click time. Its home is the reference's own
///   (`PartyMemberFrame.lua:1`), and it wants the dropdown kit and `UnitPopup` ahead of it — the
///   manifest's order (167 → 185 → 262), reproduced rather than short-circuited. Setting the
///   constant by hand would pass and teach nothing about the real load.
///
/// Needs client data, like [`BAG_UI`]: open with `benilla_formats::wow_data_or_skip!()`.
/// What the **vendor window** needs before `Interface\FrameXML\MerchantFrame.xml` will load and
/// behave — the same shape as [`BAG_UI`] and [`LOOT_UI`], and it grew for 1751 the same way.
///
/// The two that fail in the ways worth naming:
///
/// * **`BasicControls.xml`** — for `TEXT()`, the reference's identity-function wrapper, which
///   stock `MerchantFrame.lua:70` calls while building every row. Absent, it raises on the first
///   `MerchantFrame_UpdateMerchantInfo`, which is the first thing the window does when it shows.
/// * **`Interface\FrameXML\ItemButtonTemplate.xml`** — the stock rows' `$parentItemButton`
///   inherits it, and a missing template is a loader *warning*: the rows load clean with no art.
///
/// Needs client data: open with `benilla_formats::wow_data_or_skip!()`.
pub(super) const MERCHANT_UI: &[&str] = &[
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\BasicControls.xml", // TEXT()
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // The reference's own window tab (`CharacterFrameTabButtonTemplate`), whose `<OnShow>`
    // fits each tab to its text — it needs the `UIPanelTemplates` pair above it (1993).
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "ScrollTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml",
    "Interface\\FrameXML\\GameTooltip.xml", // app load order: tooltip before merchant
];

/// The stock gossip window's dependencies in `benilla.toc` order — the list
/// `panel_tests::shipped_gossip_frame_drives_end_to_end` and its siblings load by hand, shared so
/// a feed-level test in `ui_gossip` can drive the reference's own `GossipFrame.xml` (1751).
/// `UIParent.xml` brings the slot manager (`ShowUIPanel`/`HideUIPanel`) the window's OnEvent
/// calls; `ScrollTemplates.xml` is our scroll kit the greeting pane inherits — a missing template
/// is a loader *warning*, so an under-loaded list passes and silently loses the scrollbar.
///
/// Needs client data: open with `benilla_formats::wow_data_or_skip!()`.
pub(crate) const GOSSIP_UI: &[&str] = &[
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml",
    "ScrollTemplates.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    r"Interface\FrameXML\GlobalStrings.lua",
    r"Interface\FrameXML\BasicControls.xml",
    r"Interface\FrameXML\LocaleProperties.lua",
    r"Interface\FrameXML\StaticPopup.xml",
];

pub(super) const LOOT_UI: &[&str] = &[
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml", // UIParent + UIParent.lua (the slot manager, the fades; 1988)
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\StaticPopup.xml",
    "Interface\\FrameXML\\GameTooltip.xml", // TOOLTIP_DEFAULT_COLOR, read by the dropdown backdrop
    "Interface\\FrameXML\\UIDropDownMenu.xml", // GroupLootDropDown's OnLoad calls UIDropDownMenu_Initialize
    // `UnitPopup.xml` reads ITEM_QUALITY_COLORS at FILE SCOPE for its three loot-threshold rows,
    // exactly as the reference's own UnitPopup.lua:47-49 does — so its declarer has to precede it.
    // That declarer is UIParent (ref UIParent.lua:65); 1888 moved the table there when Fonts.xml
    // went on the chain, because the reference's Fonts.xml does not declare it.
    "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
    "Interface\\FrameXML\\UnitPopup.xml",
    // …and what its rows' OnLoad calls: every `PartyMemberFrame<N>` and its pet frame runs
    // `UnitFrame_Initialize`, which lives in UnitFrame.lua and itself calls
    // `SetTextStatusBarText` out of TextStatusBar.lua. Naming PartyFrame without these loads
    // four rows that each raise on their own OnLoad — the loader reports it, but only because
    // `load_ui_strict` looks; a plain load would have gone quiet.
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    "Interface\\FrameXML\\UnitFrame.xml",
    // …and `RefreshBuffs`, which `PartyMemberFrame.lua:60` calls from each row's OnLoad. Its home
    // is BuffFrame.lua, which the chain's BuffFrame.xml sources — the reference's own toc has
    // BuffFrame at 40 and PartyFrame at 45 for exactly this reason.
    "Interface\\FrameXML\\BuffFrame.xml",
    "Interface\\FrameXML\\PartyFrame.xml",
];

/// What the **character window** needs before `Interface\FrameXML\CharacterFrame.xml`,
/// `PaperDollFrame.xml` and `PetPaperDollFrame.xml` will load and behave — the same shape as
/// [`BAG_UI`] / [`LOOT_UI`] / [`MERCHANT_UI`], and grown the same way.
///
/// This one is the longest of the four, and the reason is `CharacterFrame_OnLoad`: it is the only
/// migrated window whose LOAD-time body reaches outside its own file, and it reaches into four
/// others at once —
///
/// * **`Interface\FrameXML\TextStatusBar.lua`** for `SetTextStatusBarTextPrefix`, which it calls
///   three times before doing anything else. Its `.xml` twin comes too, because the pet page's XP
///   bar inherits the `TextStatusBar` template it declares and a missing template is a loader
///   *warning* — the bar would load with no art and no text region and read as a pass.
/// * **`Interface\FrameXML\PlayerFrame.xml`** for `PlayerFrameHealthBar` / `PlayerFrameManaBar`,
///   the frames those three calls name, and **`PetFrame.xml`** for `PetFrameHealthBar` /
///   `PetFrameManaBar`, which `CharacterFrame_OnShow` shows the text on. Both call
///   `UnitFrame_Initialize` in their OnLoad and `PlayerFrame` also calls `CombatFeedback_Initialize`,
///   so `UnitFrame.xml` and `CombatFeedback.xml` come first — the reference's own toc order.
///   (This entry used to be our one `UnitFrames.xml`, and this note used to say "when they are
///   migrated, this becomes the stock pair". They are; it did.)
/// * **`Interface\FrameXML\MainMenuBar.xml`** for `MainMenuExpBar` — the third bar of that prefix call — and for
///   `ShowWatchedReputationBarText` / `HideWatchedReputationBarText`, which the window's
///   show/hide pair calls.
/// * **`UIPanelTemplates.xml`** for `PanelTemplates_SetNumTabs`/`_SetTab`, the last two lines of
///   that same OnLoad.
///
/// And two more that only bite later, which is exactly why they are written down:
///
/// * **`Interface\FrameXML\HonorFrame.xml`** — `PaperDollFrame_SetLevel` and `_SetGuild` write
///   `HonorLevelText` and `HonorGuildText` "while we're at it" (`PaperDollFrame.lua:100`/`:120`).
///   Nothing touches them at load, so leaving this out loads clean and then raises the first time
///   the window is SHOWN. It sits below the character block in the manifest for the same reason.
/// * **`MainMenuBarMicroButtons.xml`** — `UpdateMicroButtons` (called by both
///   `CharacterFrame_OnShow` and `_OnHide`) and `MicroButtonTooltipText` (all five tab hovers).
///   A tab hover is the only thing that reaches the second one, so its absence is invisible
///   until a test hovers a tab. The reference's own file since 1987; the panels its
///   `UpdateMicroButtons` reads unguarded and this kit stops short of are
///   [`MICRO_BUTTON_STAND_INS`]'s.
///
/// Needs client data, like its three siblings: open with `benilla_formats::wow_data_or_skip!()`.
pub(super) const CHARACTER_UI: &[&str] = &[
    // The reference's own localized strings. The stock file has no `X = X or "…"` fallbacks —
    // `PaperDollFrame_OnLoad` sets seven labels from them at LOAD, and `PaperDollFrame_SetStats`
    // concatenates `SPELL_STAT0_NAME`..`4` on every repaint.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `GetText` — the reference's gendered-string helper, which stock
    // `ReputationFrame.lua:65` calls for every row's standing label. Manifest line 94
    // (1875).
    r"Interface\FrameXML\LocaleProperties.lua",
    "Interface\\FrameXML\\BasicControls.xml", // TEXT(), which every one of those label sets goes through
    "Interface\\FrameXML\\ItemButtonTemplate.xml", // PaperDollItemSlotButtonTemplate's base
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml", // Model_OnLoad/_Rotate*/_OnUpdate — the model panes' turntable
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\Cooldown.xml", // CooldownFrameTemplate + CooldownFrame_SetTimer, per equipment slot
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\StaticPopup.xml",
    // The unit frames' four right-click dropdowns call `UIDropDownMenu_Initialize` at LOAD, so the
    // kit and the menu table it initialises from both precede them — the manifest's own order
    // (175 → 189 → 193 → 263).
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\UIMenu.xml",
    "Interface\\FrameXML\\UnitPopup.xml",
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    // `UnitFrame_Initialize` and `CombatFeedback_Initialize`, which the two windows below call in
    // their OnLoad. Neither file declares a frame; both are pure script.
    "Interface\\FrameXML\\UnitFrame.xml",
    "Interface\\FrameXML\\CombatFeedback.xml",
    "Interface\\FrameXML\\PlayerFrame.xml",
    // `PetFrame.xml`'s four `PetFrameBuff*` inherit `PartyBuffButtonTemplate`, which lives here.
    // The manifest never names this file: it arrives through `PartyFrame.xml`'s
    // `<Include file="PartyFrameTemplates.xml"/>`, and the reference's own toc puts PartyFrame (45)
    // ahead of TargetFrame (46) and PetFrame (47) for exactly that reason. This kit wants the
    // template and not four party member frames, so it takes the included file directly.
    "Interface\\FrameXML\\PartyFrameTemplates.xml",
    "Interface\\FrameXML\\PetFrame.xml",
    "Interface\\FrameXML\\ActionButtonTemplate.xml",
    "Interface\\FrameXML\\MainMenuBar.xml",
    "Interface\\FrameXML\\ActionBarFrame.xml",
    "Interface\\FrameXML\\BonusActionBarFrame.xml",
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    // The two page files these three need before the window can be OPENED, which is not the same
    // as before it can load: `CHARACTERFRAME_SUBFRAMES` lists all five pages by name and
    // `CharacterFrame_ShowSubFrame` calls `getglobal(value):Hide()` on each one it is not showing
    // (`CharacterFrame.lua:25-32`), unguarded. A missing page is `attempt to index a nil value` on
    // the very first `ToggleCharacter` — load-clean, then dead on the first click. Their own
    // template dependencies come with them.
    "ScrollTemplates.xml", // SkillFrame's faux list + trough
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // The reference's own window tab (`CharacterFrameTabButtonTemplate`), whose `<OnShow>`
    // fits each tab to its text — it needs the `UIPanelTemplates` pair above it (1993).
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    // The four options templates off the chain — ReputationFrame's detail check boxes inherit
    // `OptionsCheckButtonTemplate`. Ours beside it is gone with 2115: its one template,
    // `UIOptionsCheckButtonTemplate`, comes off the chain out of `UIOptionsFrame.xml` now.
    "Interface\\FrameXML\\OptionsFrameTemplates.xml",
    "Interface\\FrameXML\\CharacterFrame.xml",
    "Interface\\FrameXML\\PaperDollFrame.xml",
    "Interface\\FrameXML\\PetPaperDollFrame.xml",
    // `updateContainerFrameAnchors` — `ReputationWatchBar_Update` hard-calls it when the bar
    // moves (ReputationFrame.lua:248), because in the reference the bar's presence reflows the
    // bag row. It comes with `ContainerFrame.xml`, manifest 585 against the pane's 693 (1875).
    "Interface\\FrameXML\\ContainerFrame.xml",
    r"Interface\FrameXML\ReputationFrame.xml",
    "Interface\\FrameXML\\SkillFrame.xml",
    "Interface\\FrameXML\\HonorFrame.xml",
];

/// The social window's slice of the manifest (1959): everything the reference's
/// `FriendsFrame.xml` + `RaidFrame.xml` reach at load or on show — `TEXT` and `GetText`, the
/// bar chain under `UpdateMicroButtons`, the chat window whose edit box `FriendsFrame_SendMessage`
/// opens, the unit menu, the party frames `RaidFrame_OnLoad` reconciles — in the manifest's own
/// order; the raid tab's LoadOnDemand addon follows through [`load_social_ui`]. Shared by the
/// friends, guild and raid harnesses.
pub(super) const SOCIAL_UI: &[&str] = &[
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    r"Interface\FrameXML\UIParent.xml",
    "Interface\\FrameXML\\Cooldown.xml",
    "Interface\\FrameXML\\ActionButtonTemplate.xml",
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    "Interface\\FrameXML\\MainMenuBar.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\ActionBarFrame.xml",
    "Interface\\FrameXML\\BonusActionBarFrame.xml",
    "ScrollTemplates.xml",
    "Interface\\FrameXML\\UIPanelTemplates.lua",
    "Interface\\FrameXML\\UIPanelTemplates.xml",
    // The reference's own window tab (`CharacterFrameTabButtonTemplate`), whose `<OnShow>`
    // fits each tab to its text — it needs the `UIPanelTemplates` pair above it (1993).
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\OptionsFrameTemplates.xml",
    "Interface\\FrameXML\\ReputationFrame.xml",
    "Interface\\FrameXML\\StaticPopup.xml",
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "KeyBindingsPage.xml",
    "OptionsFrame.xml",
    "Interface\\FrameXML\\MultiActionBars.xml",
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    "Interface\\FrameXML\\UnitPopup.xml",
    "Interface\\FrameXML\\UIMenu.xml",
    "Interface\\FrameXML\\ChatFrame.xml",
    "Interface\\FrameXML\\FloatingChatFrame.xml",
    "Interface\\FrameXML\\BuffFrame.xml",
    "Interface\\FrameXML\\UnitFrame.xml",
    "Interface\\FrameXML\\CombatFeedback.xml",
    "Interface\\FrameXML\\PartyFrame.xml",
    "Interface\\FrameXML\\FriendsFrame.xml",
    "Interface\\FrameXML\\RaidFrame.xml",
];

/// Load [`SOCIAL_UI`], then the raid tab's LoadOnDemand addon the way the app reaches it: seated
/// off the chain as a registry row (1957) and loaded by the reference's own `RaidFrame_LoadUI`
/// (UIParent.xml; 1967). Needs client data — the caller has checked with `wow_data_or_skip!`.
pub(super) fn load_social_ui(s: &mut UiScript) {
    for f in SOCIAL_UI {
        load_ui_strict(s, f);
    }
    seat_chain_addon(s, "Blizzard_RaidUI");
    s.run("RaidFrame_LoadUI()").unwrap();
}

pub(super) const BAG_UI: &[&str] = &[
    // The reference's own localized strings — `BACKPACK_TOOLTIP`, `EQUIP_CONTAINER`, `KEYRING`,
    // the `*_FONT_COLOR_CODE` pair. The app loads this at VM setup, ahead of the manifest
    // (`ui_script/mod.rs`, `setup_script`); a test VM has to say so itself. Not optional since
    // 1751's third window: stock `MainMenuBarBagButtons.lua`'s hovers pass these straight into
    // `GameTooltip:SetText`, and `SetText(nil)` raises rather than showing an empty plate. Our
    // deleted `BagFrame.xml` carried `X = X or "…"` fallbacks for exactly this gap; the real file
    // is the better answer, and these tests already gate on the install.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `TEXT()` — the reference's own identity-function wrapper, which stock
    // `MainMenuBarBackpackButton`'s OnEnter calls (`GameTooltip:SetText(TEXT(BACKPACK_TOOLTIP)…)`)
    // and `BagSlotButton_OnEnter` calls for `EQUIP_CONTAINER`. Manifest entry 3, and not optional
    // for the bag bar since 1751's third window made that bar the reference's own.
    "Interface\\FrameXML\\BasicControls.xml",
    // `UIParent` itself: the twelve `ContainerFrame`s declare `parent="UIParent"`, and
    // `updateContainerFrameAnchors` anchors each open bag to `frame:GetParent()` while
    // `OpenAllBags` opens with `if not UIParent:IsVisible() then return end`. Without it the
    // windows fall out of the cascade and the reference's own layout pass has nothing to measure.
    r"Interface\FrameXML\UIParent.xml",
    "ScrollTemplates.xml", // our scroll kit + the placeholder icon
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\Cooldown.xml",
    // The bag BAR declares `parent="MainMenuBarArtFrame"`, resolved at LOAD — so without this the
    // six buttons fall back to UIParent and sit at a level no production run ever puts them at.
    // It also carries `MainMenuBar_UpdateKeyRing`, which is what puts the keyring on the bar.
    "Interface\\FrameXML\\ActionButtonTemplate.xml",
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    "Interface\\FrameXML\\MainMenuBar.xml",
    "Interface\\FrameXML\\ActionBarFrame.xml",
    "Interface\\FrameXML\\BonusActionBarFrame.xml",
    // `UpdateMicroButtons` — the KEYRING's own OnShow/OnHide calls it (ContainerFrame.lua l.117,
    // l.137), because in the reference the keyring's existence moves the micro-button row.
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // The dialog engine, after the UIPanelCloseButton it inherits (1960).
    r"Interface\FrameXML\StaticPopup.xml",
    "Interface\\FrameXML\\ContainerFrame.xml",
    // `PaperDollItemSlotButtonTemplate` and the `PaperDollItemSlotButton_*` family behind it,
    // which every bag button inherits and runs — resolved at load, so this has to precede the bar
    // exactly as it does in the manifest. Stock `BagSlotButtonTemplate`'s OnLoad *is*
    // `PaperDollItemSlotButton_OnLoad()`, which gives each bag button its inventory-slot id
    // (20..23 via `GetInventorySlotInfo`), its six event registrations and its first paint.
    //
    // The whole paper-doll file, because that is where the reference declares both — our
    // `ItemSlotButtonTemplates.xml` held a transcribed copy of the template only because our own
    // character window loaded too late to declare it, and it is deleted. Its
    // companion `Interface\\FrameXML\\CharacterFrame.xml` is deliberately NOT here: this list is
    // the bags, `PaperDollFrame` only names `CharacterFrame` in `parent=` (a missing parent is a
    // loader warning, not an error), and `CharacterFrame_OnLoad` would drag in the unit frames,
    // the XP bar and the text-status-bar file for a window no bag test opens.
    "Interface\\FrameXML\\PaperDollFrame.xml",
    // The bag BAR itself, the reference's own since 1751's third window: MainMenuBarBackpackButton,
    // CharacterBag0..3Slot, KeyRingButton, `BagSlotButtonTemplate`, and `KEYRING_CONTAINER`.
    "Interface\\FrameXML\\MainMenuBarBagButtons.xml",
    // `StackSplitFrame` is not optional either: the reference's own
    // `ContainerFrameItemButton_OnClick` calls `StackSplitFrame:Hide()` on EVERY plain click
    // (ContainerFrame.lua l.581) before the pickup, and opens it on the shift fork.
    "Interface\\FrameXML\\StackSplitFrame.xml",
    // …nor is the chat edit box. The reference's SHIFT arm opens with
    // `if ( ChatFrameEditBox:IsShown() )` (ContainerFrame.lua l.569) to decide between posting the
    // item's link and splitting the stack, so a VM without it raises before either.
    "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
    "Interface\\FrameXML\\ChatFrame.xml",
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\UIPanelTemplates.lua",
    "Interface\\FrameXML\\UIPanelTemplates.xml",
    "Interface\\FrameXML\\FloatingChatFrame.xml",
    // Our adapters over the reference's container files — the keyring tooltip wrapper and the
    // three bag verbs 0561 shadows (`OpenBackpack`/`CloseBackpack`/`CloseAllBags`). It has to be
    // AFTER `ContainerFrame.xml` and after the bar, which is why it is here and not up with
    // `UIParent.xml`.
    "ContainerFrameAdapters.xml",
    // `updateContainerFrameAnchors` measures every open bag against `BankFrame:GetRight()`
    // (ContainerFrame.lua l.505) on EVERY open and close, so the bank window is not optional
    // scenery for a bag test — it is a hard dependency of the reference's own layout pass. It is
    // one in the real client too; the manifest just satisfies it far below the bags. The
    // reference's own file since 1751's second window.
    "Interface\\FrameXML\\BankFrame.xml",
];

/// The name of the `ContainerFrame` currently showing bag `id`, or `None` if it is not open.
///
/// **Ask, never assume.** The reference recycles twelve windows across every container
/// (`ContainerFrame_GetOpenFrame`), so which one a bag lands in depends on what else is open —
/// there is no `BenillaBagFrame2` to name any more, and pinning `ContainerFrame3` in a test would
/// pin a coincidence. `IsBagOpen` is the reference's own published scan.
pub(super) fn bag_window(s: &UiScript, id: i64) -> Option<String> {
    s.eval::<Option<i64>>(&format!("return IsBagOpen({id})"))
        .unwrap()
        .map(|i| format!("ContainerFrame{i}"))
}

/// Is bag `id` open? [`bag_window`]'s predicate half.
pub(super) fn bag_open(s: &UiScript, id: i64) -> bool {
    bag_window(s, id).is_some()
}

/// The item button in bag `id`'s open window that holds game slot `slot`.
///
/// Asked of the buttons' own `GetID`, never derived: `ContainerFrame_GenerateFrame` numbers them
/// backwards (`index = size - j + 1`, so `…Item1` is the bag's LAST slot, bottom-right), and a
/// window generated for a different bag size numbers them differently.
pub(super) fn bag_slot_button(s: &UiScript, id: i64, slot: u32) -> String {
    let w = bag_window(s, id).unwrap_or_else(|| panic!("bag {id} is not open"));
    s.eval::<String>(&format!(
        "for j = 1, MAX_CONTAINER_ITEMS do \
           local b = getglobal(\"{w}Item\"..j) \
           if b and b:IsShown() and b:GetID() == {slot} then return \"{w}Item\"..j end \
         end return \"\""
    ))
    .inspect(|n| assert!(!n.is_empty(), "no {w}Item* is bag {id} slot {slot}"))
    .expect("the item-button scan")
}

/// The centre of a named frame, in the y-up UI space `mouse_move`/`mouse_button` take.
pub(super) fn centre_of(s: &mut UiScript, name: &str) -> (f32, f32) {
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

/// Move the mouse onto the centre of `name` — the whole engine path (hit test → `OnEnter`), never
/// `s.run("Handler(button)")`.
///
/// **This is not a style preference any more.** The reference's own handlers read `this`
/// (`ContainerFrameItemButton_OnClick(button, ignoreModifiers)` takes the MOUSE button as its first
/// argument and gets the frame from `this`), and only the engine sets `this`. A migrated window's
/// tests therefore drive the mouse, which is also the stronger test — it puts the
/// `RegisterForClicks` gate and the template's own script wiring under test.
pub(super) fn hover(s: &mut UiScript, name: &str) {
    let (x, y) = centre_of(s, name);
    s.mouse_move(x, y);
}

/// Move the mouse well clear of everything — the `OnLeave` half of [`hover`].
pub(super) fn unhover(s: &mut UiScript) {
    s.mouse_move(-500.0, -500.0);
}

/// Press and release `button` over the centre of `name`, the way a player's mouse does.
pub(super) fn click(s: &mut UiScript, name: &str, button: &str) {
    let (x, y) = centre_of(s, name);
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// Seat the world frame in a fixture, the way the manifest does — the file the world drop's
/// click target comes from, plus the one its `OnUpdate` reads a constant out of.
///
/// `WorldFrame_OnUpdate` ticks the popups and breath bars that a hidden UI would otherwise
/// freeze, and it walks `1, MIRRORTIMER_NUMTIMERS` to do it — a global whose home is
/// `MirrorTimer.lua`, manifest entry 53. A harness that takes the world frame without it raises
/// `'for' limit must be a number` on every tick. Loading the real file rather than setting the
/// constant by hand is [`BAG_UI`]'s own rule (`PartyFrame.xml` is there for the same reason):
/// a stubbed constant passes and teaches nothing about the real load.
///
/// `StaticPopup.xml` — the other name that loop reads — is already in every fixture that calls
/// this, since the popup is what these tests are about.
pub(super) fn load_world_frame(s: &UiScript) {
    load_ui(s, r"Interface\FrameXML\WorldFrame.xml");
    load_ui(s, r"Interface\FrameXML\MirrorTimer.xml");
}

/// A completed left CLICK on the game world, at a point the loaded `WorldFrame` actually owns.
///
/// **The point is searched, not assumed, and that is the whole lesson of B380**.
/// Every world-drop fixture in the house used to click `(-50, -50)` — off-screen, where the hit
/// test answers nothing at all — which is a world click only in a house with no `WorldFrame`
/// loaded. The stock file's frame is full-screen and mouse-enabled, so from the day it joined the
/// manifest it took every real world click while those fixtures went on passing over the void.
///
/// The point cannot be a constant either, because a harness holds the windows its test needs and
/// not the ones that would hide them — `bag_setup`'s `PaperDollFrame` has no `CharacterFrame` to
/// sit inside, so it covers the middle of the screen. So: walk a coarse grid over the world
/// frame's own rect and take the first cell it owns. When it owns none, panic naming what is
/// there — a fixture with no world to click is one that would pass for the wrong reason.
pub(super) fn world_click(s: &mut UiScript) {
    let (x, y) = world_point(s);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    assert!(
        s.mouse_button(x, y, "LeftButton", false),
        "a world drop consumes the completed click"
    );
}

/// [`world_click`]'s search. Separate so the failure can say what it looked at.
fn world_point(s: &mut UiScript) -> (f32, f32) {
    s.resolve();
    let r: Vec<f32> = s
        .eval(
            "local f = WorldFrame \
             return { f:GetLeft(), f:GetBottom(), f:GetWidth(), f:GetHeight() }",
        )
        .expect(
            "the fixture must load Interface\\FrameXML\\WorldFrame.xml (load_world_frame): a \
             world drop is a click on THAT frame, not on nothing (decision 2089)",
        );
    assert_eq!(r.len(), 4, "WorldFrame: unresolved rect {r:?}");
    let (steps, mut blockers) = (7, Vec::new());
    for row in 0..steps {
        for col in 0..steps {
            let x = r[0] + r[2] * (col as f32 + 0.5) / steps as f32;
            let y = r[1] + r[3] * (row as f32 + 0.5) / steps as f32;
            match s.hit_test(x, y) {
                Some(id) if s.is_world_frame(id) => return (x, y),
                _ => blockers.push(
                    s.hit_test_name(x, y)
                        .unwrap_or_else(|| "<anonymous>".into()),
                ),
            }
        }
    }
    blockers.sort();
    blockers.dedup();
    panic!("no point on the world frame is clickable — the UI covers all {steps}x{steps} of them: {blockers:?}")
}

/// Seat one of the reference's LoadOnDemand Blizzard addons off the chain, so a harness's
/// `UIParentLoadAddOn(name)` can load it the way the app does (1957; the combat text since
/// 1964). Needs client data — the caller has already checked with `wow_data_or_skip!`.
pub(super) fn seat_chain_addon(s: &mut UiScript, name: &str) {
    let toc = super::reference_ui::read(&format!("Interface/AddOns/{name}/{name}.toc"))
        .map(|b| benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&b)))
        .unwrap_or_else(|| panic!("{name}: no toc off the chain"));
    let mut info = super::addons::info_from_toc(name, &toc);
    info.chain = true;
    s.set_addon_chain_reader(Box::new(super::reference_ui::read));
    s.register_addons(vec![info], None, None, None);
}
