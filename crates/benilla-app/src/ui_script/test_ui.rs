//! The tests' interface loader: a bare filename is a file we ship under `assets/ui`, a path one off
//! the player's patch chain ([`super::reference_ui::is_chain_entry`], the manifest's rule). It
//! reads the source tree, not the compiled-in copy.

use benilla_ui::script::{QuadContent, UiScript};

/// Load one interface file into `s`, panicking on any loader error; returns the frames it built
/// (0 for a `.lua`). A chain entry needs client data, so a test naming one opens with
/// `benilla_formats::wow_data_or_skip!()`.
pub(crate) fn load_ui(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, false, false)
}

/// The cooldown indicator's file, as the stock `CooldownFrameTemplate` names it.
pub(super) const COOLDOWN_MODEL: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";

/// The cooldown indicator's file facts (`benilla-extract m2seq`), handed to the engine as the app
/// does: sequence 0 (id 0, 1000 ms, clamp) is the sweep `CooldownFrame_OnUpdateModel` scrubs,
/// sequence 1 (id 1, 1000 ms, clamp) the finish flash whose end hides the frame.
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

/// The play head `(anim_id, cursor_ms)` of the shown cooldown pane `owner` names, as the tile
/// renderer samples it.
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

/// [`cooldown_play`] for whichever cooldown pane is shown: `ContainerFrame_GenerateFrame` numbers
/// a bag's slots from the far end, so a one-item test asks for the sweep, not a name.
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

/// [`load_ui`], and a missing template fails too: the loader only warns, and the art is lost.
pub(super) fn load_ui_strict(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, true, false)
}

/// [`load_ui`], and any loader warning fails.
pub(super) fn load_ui_no_warnings(s: &UiScript, entry: &str) -> usize {
    load_entry(s, entry, false, true)
}

fn load_entry(s: &UiScript, entry: &str, strict_templates: bool, no_warnings: bool) -> usize {
    // The app registers its CVars before any file loads: stock `UIOptionsFrame.xml` reads two
    // camera CVars at `OnLoad` and raises on a nil. A value a test already set stands.
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
    // Seated before the load: `MultiActionBarFrame_OnLoad` indexes `UIOptionsFrameCheckButtons`
    // during this load walk.
    if path
        .rsplit('/')
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case("MultiActionBars.xml"))
    {
        s.run(MULTI_ACTION_BAR_STAND_INS)
            .expect("the multibar stand-ins");
    }
    // Our options window's Graphics rows read stock `OptionsFrameSliders` (`OptionsFrame.lua`) at
    // OnLoad; the manifest loads it one seat above ours, so a kit seats it before our file.
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

/// What a kit owes stock `UIParent.xml`: no-op stand-ins, seated when it loads, for what its
/// `<OnUpdate>`, `UIParent_OnEvent` and `ShowUIPanel` call from files a kit may stop short of. A
/// `function X()` is a plain global write, so the real definition replaces a stand-in in any order.
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
    -- l.21; our own OptionsFrame.xml re-declares it below that. Both are plain
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
            -- one-window kit never loads. An unresolvable name is a RAISE, so a target the kit is
            -- missing aborts `UIParent_ManageFramePositions` mid-pass rather than anchoring to
            -- the parent and carrying on.
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
    -- names are real, and all three options windows are the REFERENCE's own files,
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

/// What a kit owes stock `UIOptionsFrame.xml`: its `VARIABLES_LOADED` arm
/// (`UIOptionsFrame.lua:193-227`) calls six functions from files a kit may not load, and a kit
/// fires the event itself. Functions, so seating them at load is safe in any order.
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

/// What a kit owes stock `MultiActionBars.xml`: its OnLoad (`MultiActionBars.lua:10`) writes five
/// rows into `UIOptionsFrameCheckButtons`, which `UIOptionsFrame.xml` declares eighteen toc rows
/// earlier; `or {}` keeps the real table when that file loaded first.
pub(super) const MULTI_ACTION_BAR_STAND_INS: &str = r#"
    UIOptionsFrameCheckButtons = UIOptionsFrameCheckButtons or {}
    -- The five ROWS it writes into, not just the table: the reference's hack assigns
    -- `UIOptionsFrameCheckButtons["SHOW_MULTIBAR1_TEXT"].setFunc`, which needs the row to exist.
    -- Empty rows on purpose — the real ones carry `index = 33..36, 40`, and a stand-in that
    -- restated those numbers would be a transcription of the reference's table in a test helper,
    -- which `OptionsFrame.xml` does not carry either. Any kit that reads an index has the
    -- real window loaded and therefore the real table.
    for _, key in ipairs({ "SHOW_MULTIBAR1_TEXT", "SHOW_MULTIBAR2_TEXT", "SHOW_MULTIBAR3_TEXT",
                           "SHOW_MULTIBAR4_TEXT", "ALWAYS_SHOW_MULTIBARS_TEXT" }) do
        UIOptionsFrameCheckButtons[key] = UIOptionsFrameCheckButtons[key] or {}
    end
"#;

/// What a kit owes the stock micro-button row: `UpdateMicroButtons`
/// (`MainMenuBarMicroButtons.lua:20-84`) reads ten panels, `KeyRingButton` and `IsBagOpen`
/// unguarded. Hidden, unnamed stand-ins are seated on its first call, not at load: a named frame's
/// publish never overwrites (`0x701bd0`), so an early one would shadow the real frame.
/// `tests/common/mod.rs` carries the same chunk.
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

/// One file's bytes, off the chain for a path or from this crate's `assets/ui` for a bare name;
/// also the loader's `<Include>`/`<Script file=>` provider.
pub(super) fn read(req: &str) -> Option<Vec<u8>> {
    if super::reference_ui::is_chain_entry(req) {
        return super::reference_ui::read(req);
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    std::fs::read(dir.join(req)).ok()
}

/// What stock `MerchantFrame.xml` needs to load and behave, including `BasicControls.xml` for the
/// `TEXT()` that `MerchantFrame.lua:70` calls on first show and `ItemButtonTemplate.xml`, whose
/// absence only warns. Needs client data: open with `benilla_formats::wow_data_or_skip!()`.
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
    // The stock window tab, whose `<OnShow>` needs the `UIPanelTemplates` pair above it.
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    "ScrollTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\StaticPopup.xml",
    "Interface\\FrameXML\\GameTooltip.xml", // app load order: tooltip before merchant
];

/// Stock `GossipFrame.xml`'s dependencies in `benilla.toc` order, shared with `ui_gossip`'s feed
/// tests. The greeting pane inherits `UIPanelScrollFrameTemplate`, and a missing template only
/// warns, losing the scrollbar silently. Needs client data.
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

/// What stock `LootFrame.xml` needs to load and behave. `GlobalStrings.lua` because a bare VM
/// lacks what the app loads at setup (the master-loot menu reads `GROUP` and `GIVE_LOOT`),
/// `ItemButtonTemplate.xml` because a missing template only warns, and `PartyFrame.xml` because
/// `LootFrame.lua:217` reads `MAX_PARTY_MEMBERS` at load. Needs client data.
pub(super) const LOOT_UI: &[&str] = &[
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml", // UIParent.lua: the panel slot manager and the fades
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\BasicControls.xml",
    "Interface\\FrameXML\\StaticPopup.xml",
    "Interface\\FrameXML\\GameTooltip.xml", // TOOLTIP_DEFAULT_COLOR, read by the dropdown backdrop
    "Interface\\FrameXML\\UIDropDownMenu.xml", // GroupLootDropDown's OnLoad calls UIDropDownMenu_Initialize
    // `UnitPopup.lua` reads `ITEM_QUALITY_COLORS` at file scope (`UnitPopup.lua:47-49`), so its
    // declarer, `UIParent.lua:65`, precedes it.
    "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
    "Interface\\FrameXML\\UnitPopup.xml",
    // Each `PartyMemberFrame<N>` and its pet frame run `UnitFrame_Initialize` (UnitFrame.lua) at
    // OnLoad, which calls `SetTextStatusBarText` (TextStatusBar.lua).
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    "Interface\\FrameXML\\UnitFrame.xml",
    // `RefreshBuffs`, which each party row's OnLoad reaches (`PartyMemberFrame.lua:60`), is in
    // BuffFrame.lua: the reference's toc has BuffFrame at 40 and PartyFrame at 45.
    "Interface\\FrameXML\\BuffFrame.xml",
    "Interface\\FrameXML\\PartyFrame.xml",
];

/// What stock `CharacterFrame.xml`, `PaperDollFrame.xml` and `PetPaperDollFrame.xml` need to load
/// and behave. `CharacterFrame_OnLoad` reaches `TextStatusBar.lua`, `PlayerFrame.xml`,
/// `MainMenuBar.xml` (`MainMenuExpBar`) and the `UIPanelTemplates` pair; two bite only on show:
/// `HonorFrame.xml`, whose labels `PaperDollFrame.lua:103` and `:122` write, and
/// `MainMenuBarMicroButtons.xml` (`MicroButtonTooltipText` on a tab hover). Needs client data.
pub(super) const CHARACTER_UI: &[&str] = &[
    // The stock strings, which `PaperDollFrame_OnLoad` reads at load; there are no fallbacks.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `GetText`, the gendered-string helper `ReputationFrame.lua:65` calls for each standing.
    r"Interface\FrameXML\LocaleProperties.lua",
    "Interface\\FrameXML\\BasicControls.xml", // TEXT(), which those labels go through
    "Interface\\FrameXML\\ItemButtonTemplate.xml", // PaperDollItemSlotButtonTemplate's base
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    r"Interface\FrameXML\UIParent.xml", // Model_OnLoad/_Rotate*/_OnUpdate: the model turntable
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\Cooldown.xml", // CooldownFrameTemplate + CooldownFrame_SetTimer, per equipment slot
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    "Interface\\FrameXML\\StaticPopup.xml",
    // The unit frames' dropdowns call `UIDropDownMenu_Initialize` at load, so the kit and the menu
    // table precede them.
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\UIMenu.xml",
    "Interface\\FrameXML\\UnitPopup.xml",
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    // `UnitFrame_Initialize` and `CombatFeedback_Initialize`, called by the frames below at load.
    "Interface\\FrameXML\\UnitFrame.xml",
    "Interface\\FrameXML\\CombatFeedback.xml",
    "Interface\\FrameXML\\PlayerFrame.xml",
    // `PetFrame.xml`'s debuff buttons inherit `PartyBuffButtonTemplate`, declared here; the
    // manifest reaches this file only through `PartyFrame.xml`'s `<Include>`.
    "Interface\\FrameXML\\PartyFrameTemplates.xml",
    "Interface\\FrameXML\\PetFrame.xml",
    "Interface\\FrameXML\\ActionButtonTemplate.xml",
    "Interface\\FrameXML\\MainMenuBar.xml",
    "Interface\\FrameXML\\ActionBarFrame.xml",
    "Interface\\FrameXML\\BonusActionBarFrame.xml",
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    // Every page must exist before the window opens: `CharacterFrame_ShowSubFrame` hides each
    // `CHARACTERFRAME_SUBFRAMES` page it is not showing, unguarded (`CharacterFrame.lua:25-32`).
    "ScrollTemplates.xml", // our scroll kit
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // The stock window tab, whose `<OnShow>` needs the `UIPanelTemplates` pair above it.
    r"Interface\FrameXML\CharacterFrameTemplates.xml",
    // ReputationFrame's detail check boxes inherit its `OptionsCheckButtonTemplate`.
    "Interface\\FrameXML\\OptionsFrameTemplates.xml",
    "Interface\\FrameXML\\CharacterFrame.xml",
    "Interface\\FrameXML\\PaperDollFrame.xml",
    "Interface\\FrameXML\\PetPaperDollFrame.xml",
    // `updateContainerFrameAnchors`, which `ReputationWatchBar_Update` calls when the bar moves
    // (`ReputationFrame.lua:248`): in the reference the bar's presence reflows the bag row.
    "Interface\\FrameXML\\ContainerFrame.xml",
    r"Interface\FrameXML\ReputationFrame.xml",
    "Interface\\FrameXML\\SkillFrame.xml",
    "Interface\\FrameXML\\HonorFrame.xml",
];

/// The manifest slice stock `FriendsFrame.xml` and `RaidFrame.xml` reach at load or on show, in its
/// order; [`load_social_ui`] adds the raid tab's LoadOnDemand addon.
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
    // The stock window tab, whose `<OnShow>` needs the `UIPanelTemplates` pair above it.
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

/// Load [`SOCIAL_UI`], then the raid tab's LoadOnDemand addon as the app reaches it: seated off the
/// chain and loaded by stock `RaidFrame_LoadUI`. Needs client data.
pub(super) fn load_social_ui(s: &mut UiScript) {
    for f in SOCIAL_UI {
        load_ui_strict(s, f);
    }
    seat_chain_addon(s, "Blizzard_RaidUI");
    s.run("RaidFrame_LoadUI()").unwrap();
}

/// The files a test needs before it can open a bag window, in `benilla.toc` order, trimmed to what
/// the bags reach for. Needs client data: open with `benilla_formats::wow_data_or_skip!()`.
pub(super) const BAG_UI: &[&str] = &[
    // The stock strings: the bar's hovers pass them to `GameTooltip:SetText`, which raises on nil.
    "Interface\\FrameXML\\GlobalStrings.lua",
    "Interface\\FrameXML\\Fonts.xml",
    // `TEXT()`, which the backpack button's OnEnter and `BagSlotButton_OnEnter` call.
    "Interface\\FrameXML\\BasicControls.xml",
    // `UIParent`: the twelve `ContainerFrame`s are its children, `updateContainerFrameAnchors`
    // anchors each open bag to its parent, and `OpenAllBags` returns unless `UIParent` is visible.
    r"Interface\FrameXML\UIParent.xml",
    "ScrollTemplates.xml", // our scroll kit
    "Interface\\FrameXML\\ItemButtonTemplate.xml",
    r"Interface\FrameXML\MoneyFrame.lua",
    r"Interface\FrameXML\MoneyFrame.xml",
    "Interface\\FrameXML\\LocaleProperties.lua",
    "Interface\\FrameXML\\GameTooltip.xml",
    "Interface\\FrameXML\\Cooldown.xml",
    // `MainMenuBar.xml` and its templates: the bag bar's `parent="MainMenuBarArtFrame"` resolves
    // at load, and `MainMenuBar_UpdateKeyRing` puts the keyring on the bar.
    "Interface\\FrameXML\\ActionButtonTemplate.xml",
    "Interface\\FrameXML\\TextStatusBar.lua",
    "Interface\\FrameXML\\TextStatusBar.xml",
    "Interface\\FrameXML\\MainMenuBar.xml",
    "Interface\\FrameXML\\ActionBarFrame.xml",
    "Interface\\FrameXML\\BonusActionBarFrame.xml",
    // `UpdateMicroButtons`, which the keyring's OnHide and OnShow call
    // (`ContainerFrame.lua:117`, `:137`).
    r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
    r"Interface\FrameXML\UIPanelTemplates.lua",
    r"Interface\FrameXML\UIPanelTemplates.xml",
    // The dialog engine, after the `UIPanelCloseButton` it inherits.
    r"Interface\FrameXML\StaticPopup.xml",
    "Interface\\FrameXML\\ContainerFrame.xml",
    // `BagSlotButtonTemplate` inherits `PaperDollItemSlotButtonTemplate`, and its OnLoad
    // (`PaperDollItemSlotButton_OnLoad`) gives each bag button its inventory-slot id, 20..23.
    // `CharacterFrame.xml` stays out: a missing `parent=` only warns.
    "Interface\\FrameXML\\PaperDollFrame.xml",
    // The stock bag bar, with `BagSlotButtonTemplate` and `KEYRING_CONTAINER`.
    "Interface\\FrameXML\\MainMenuBarBagButtons.xml",
    // `ContainerFrameItemButton_OnClick` hides `StackSplitFrame` on every plain click
    // (`ContainerFrame.lua:581`) and opens it on the shift fork.
    "Interface\\FrameXML\\StackSplitFrame.xml",
    // The chat edit box: the shift arm tests `ChatFrameEditBox:IsShown()`
    // (`ContainerFrame.lua:568`) to choose between linking the item and splitting the stack.
    "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
    "Interface\\FrameXML\\ChatFrame.xml",
    "Interface\\FrameXML\\UIDropDownMenu.xml",
    "Interface\\FrameXML\\UIPanelTemplates.lua",
    "Interface\\FrameXML\\UIPanelTemplates.xml",
    "Interface\\FrameXML\\FloatingChatFrame.xml",
    // Our adapters over the stock container files, loaded after them and the bar so they win.
    "ContainerFrameAdapters.xml",
    // `updateContainerFrameAnchors` measures every open bag against `BankFrame:GetRight()`
    // (`ContainerFrame.lua:505`) on every open and close, so the bank window is a hard dependency.
    "Interface\\FrameXML\\BankFrame.xml",
];

/// The name of the `ContainerFrame` showing bag `id`, asked of `IsBagOpen`: the reference recycles
/// twelve windows across every container (`ContainerFrame_GetOpenFrame`).
pub(super) fn bag_window(s: &UiScript, id: i64) -> Option<String> {
    s.eval::<Option<i64>>(&format!("return IsBagOpen({id})"))
        .unwrap()
        .map(|i| format!("ContainerFrame{i}"))
}

/// Is bag `id` open?
pub(super) fn bag_open(s: &UiScript, id: i64) -> bool {
    bag_window(s, id).is_some()
}

/// The item button in bag `id`'s open window that holds game slot `slot`, found by `GetID`:
/// `ContainerFrame_GenerateFrame` numbers the buttons backwards (`index = size - j + 1`).
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

/// Move the mouse onto the centre of `name` through the engine's hit test and `OnEnter`: stock
/// handlers read `this`, which only the engine sets.
pub(super) fn hover(s: &mut UiScript, name: &str) {
    let (x, y) = centre_of(s, name);
    s.mouse_move(x, y);
}

/// Move the mouse well clear of everything: the `OnLeave` half of [`hover`].
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

/// Seat stock `WorldFrame.xml` and `MirrorTimer.xml`: `WorldFrame_OnUpdate` loops to
/// `MIRRORTIMER_NUMTIMERS`, which `MirrorTimer.lua` declares, and raises every tick without it.
/// Its other loop reads `STATICPOPUP_NUMDIALOGS`, which every caller already loads.
pub(super) fn load_world_frame(s: &UiScript) {
    load_ui(s, r"Interface\FrameXML\WorldFrame.xml");
    load_ui(s, r"Interface\FrameXML\MirrorTimer.xml");
}

/// A completed left click on the game world, at a point the loaded `WorldFrame` owns: searched on
/// a grid, since a harness's windows may cover any fixed point.
pub(super) fn world_click(s: &mut UiScript) {
    let (x, y) = world_point(s);
    s.mouse_move(x, y);
    s.mouse_button(x, y, "LeftButton", true);
    assert!(
        s.mouse_button(x, y, "LeftButton", false),
        "a world drop consumes the completed click"
    );
}

/// [`world_click`]'s search.
fn world_point(s: &mut UiScript) -> (f32, f32) {
    s.resolve();
    let r: Vec<f32> = s
        .eval(
            "local f = WorldFrame \
             return { f:GetLeft(), f:GetBottom(), f:GetWidth(), f:GetHeight() }",
        )
        .expect(
            "the fixture must load Interface\\FrameXML\\WorldFrame.xml (load_world_frame): a \
             world drop is a click on THAT frame, not on nothing",
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
/// `UIParentLoadAddOn(name)` loads it as the app does. Needs client data.
pub(super) fn seat_chain_addon(s: &mut UiScript, name: &str) {
    let toc = super::reference_ui::read(&format!("Interface/AddOns/{name}/{name}.toc"))
        .map(|b| benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&b)))
        .unwrap_or_else(|| panic!("{name}: no toc off the chain"));
    let mut info = super::addons::info_from_toc(name, &toc);
    info.chain = true;
    s.set_addon_chain_reader(Box::new(super::reference_ui::read));
    s.register_addons(vec![info], None, None, None);
}
