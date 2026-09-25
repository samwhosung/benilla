//! **The integration tests' interface loader — one copy, both stores**.
//!
//! The in-crate sibling of `ui_script::test_ui::load_ui`, which integration tests cannot reach:
//! they link this crate as a library, so its `#[cfg(test)]` items are not compiled for them. Each
//! `tests/*.rs` therefore grew its own reader off `assets/ui`, and every one of them broke the
//! first time a file it names became the reference's own.
//!
//! The rule is the manifest's, verbatim: **a bare filename is a file we ship, a path is the
//! reference's own off the player's installed chain.** Our shipped tree is flat, so a separator
//! decides, and nothing here needs to know which windows have migrated.
//!
//! The provider half matters as much as the loop. A sourced document pulls its Lua through its own
//! `<Script file="X.lua"/>`, which the loader resolves against the *including document's*
//! directory — `Interface\FrameXML\X.lua`, a chain path. A disk-only provider leaves every one of
//! those globals nil and the failures land nowhere near the cause.
//!
//! **A chain entry needs client data**, so a test that names one opens with
//! `benilla_formats::wow_data_or_skip!()`, like every other archive-backed test.

use benilla_ui::script::UiScript;

/// Load one manifest entry into `script`, panicking on any loader error.
///
/// A `.lua` entry is run as a chunk rather than parsed as a document — `GlobalStrings.lua` and
/// `LocaleProperties.lua` are entries of that shape in the real manifest too. Bytes, not text: a
/// chunk goes to Lua as it sits in the archive and only an XML parse decodes (1193).
pub fn load_ui(script: &UiScript, entry: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let chain = |req: &str| -> Option<Vec<u8>> {
        let data = benilla_formats::wow_data()?;
        benilla_formats::open_chain(&data).ok()?.read(req).ok()
    };
    let read = |req: &str| -> Option<Vec<u8>> {
        if req.contains('\\') || req.contains('/') {
            return chain(req);
        }
        std::fs::read(dir.join(req)).ok()
    };

    let bytes = read(entry).unwrap_or_else(|| panic!("{entry}: not found"));
    if entry.to_ascii_lowercase().ends_with(".lua") {
        script
            .run_chunk_named(&bytes, &format!("@{entry}"))
            .unwrap_or_else(|e| panic!("{entry}: {e}"));
        return;
    }
    let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
        .unwrap_or_else(|e| panic!("{entry}: {e}"));
    let report = benilla_ui::loader::load_in(script, &doc, &entry.replace('\\', "/"), &read);
    assert!(
        report.errors.is_empty(),
        "{entry}: loader errors: {:#?}",
        report.errors
    );
    let leaf = entry.rsplit(['\\', '/']).next().unwrap_or("");
    if leaf.eq_ignore_ascii_case("MainMenuBarMicroButtons.xml") {
        script
            .run(MICRO_BUTTON_STAND_INS)
            .expect("the micro-button stand-ins");
    }
    if leaf.eq_ignore_ascii_case("UIParent.xml") && entry.contains('\\') {
        script
            .run(UIPARENT_STAND_INS)
            .expect("the UIParent stand-ins");
    }
}

/// The stock micro-button row's unguarded reads, stood in for on the row's first call — one copy
/// per store, like the loader itself. `ui_script::test_ui::MICRO_BUTTON_STAND_INS` is the
/// original and carries the why.
/// The stock `UIParent.xml`'s unguarded callees, stood in for at load — see
/// `ui_script::test_ui::UIPARENT_STAND_INS`.
const UIPARENT_STAND_INS: &str = r#"
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

const MICRO_BUTTON_STAND_INS: &str = r#"
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
