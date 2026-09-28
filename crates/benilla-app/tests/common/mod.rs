//! The integration tests' interface loader: integration tests cannot reach
//! `ui_script::test_ui::load_ui`, which is `#[cfg(test)]`. A bare filename is a file we ship
//! under `assets/ui`, a path is the reference's own off the installed chain, and `<Script file>`
//! includes resolve through the same provider. A test that names a chain entry opens with
//! `benilla_formats::wow_data_or_skip!()`. A kit loads in the production order, `FrameXML.toc`'s
//! then `layer.toc`'s, which every load checks ([`follow_production_order`]).

use benilla_ui::script::UiScript;

/// Load one manifest entry into `script`, panicking on any loader error. A `.lua` entry runs as a
/// chunk of raw bytes, as `GlobalStrings.lua` does in the real manifest; only XML is decoded.
pub fn load_ui(script: &UiScript, entry: &str) {
    follow_production_order(script, entry);
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
    // The stock GameMenuFrame.xml sources UIParent.lua again, which drops the stand-ins' wrappers.
    if (leaf.eq_ignore_ascii_case("UIParent.xml") || leaf.eq_ignore_ascii_case("GameMenuFrame.xml"))
        && entry.contains('\\')
    {
        script
            .run(UIPARENT_STAND_INS)
            .expect("the UIParent stand-ins");
    }
}

/// Where `entry` falls in the production load: a `FrameXML.toc` row at its row, a file a row pulls
/// in (`<Include>` or `<Script file=>`, at any depth) just above the first row that does, a layer
/// file after every row, and `None` for anything else; the copy of
/// `ui_script::test_ui::production_rank`.
fn production_rank(entry: &str) -> Option<usize> {
    static RANKS: std::sync::OnceLock<std::collections::HashMap<String, usize>> =
        std::sync::OnceLock::new();
    let ranks = RANKS.get_or_init(|| {
        let key = |path: &str| path.replace('/', "\\").to_ascii_lowercase();
        let mut ranks = std::collections::HashMap::new();
        let Some(chain) =
            benilla_formats::wow_data().and_then(|data| benilla_formats::open_chain(&data).ok())
        else {
            return ranks;
        };
        let read = |path: &str| chain.read(path).ok();
        let toc = read(r"Interface\FrameXML\FrameXML.toc").unwrap_or_default();
        let rows: Vec<String> = benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&toc))
            .files
            .iter()
            .map(|f| format!(r"Interface\FrameXML\{f}"))
            .collect();
        for (i, row) in rows.iter().enumerate() {
            ranks.insert(key(row), 2 * i + 1);
        }
        for (i, row) in rows.iter().enumerate() {
            let mut queue = vec![row.clone()];
            while let Some(file) = queue.pop() {
                let Some(bytes) = read(&file) else { continue };
                let text = String::from_utf8_lossy(&bytes);
                for chunk in text.split('<').skip(1) {
                    let tag = chunk.split('>').next().unwrap_or("").trim_start();
                    if !tag.starts_with("Include") && !tag.starts_with("Script") {
                        continue;
                    }
                    let Some(name) = tag
                        .split("file=\"")
                        .nth(1)
                        .and_then(|r| r.split('"').next())
                    else {
                        continue;
                    };
                    let path = format!(r"Interface\FrameXML\{name}");
                    if let std::collections::hash_map::Entry::Vacant(slot) = ranks.entry(key(&path))
                    {
                        slot.insert(2 * i);
                        queue.push(path);
                    }
                }
            }
        }
        let layer = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui/layer.toc");
        let layer = std::fs::read_to_string(layer).unwrap_or_default();
        let after = 2 * rows.len() + 2;
        for (j, file) in benilla_ui::toc::Toc::parse(&layer).files.iter().enumerate() {
            ranks.insert(key(file), after + j);
        }
        ranks
    });
    ranks
        .get(&entry.replace('/', "\\").to_ascii_lowercase())
        .copied()
}

thread_local! {
    /// Per VM session, the furthest [`production_rank`] a kit has loaded and the file at it.
    static KIT_AT: std::cell::RefCell<std::collections::HashMap<u64, (usize, String)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Panics when `entry` loads after a file the production load runs after it.
fn follow_production_order(script: &UiScript, entry: &str) {
    let Some(rank) = production_rank(entry) else {
        return;
    };
    KIT_AT.with(|at| {
        let mut at = at.borrow_mut();
        if let Some((furthest, file)) = at.get(&script.session()) {
            assert!(
                rank >= *furthest,
                "{entry} loads after {file}, but the production load runs it first: a kit \
                 follows FrameXML.toc's order, then layer.toc's"
            );
            if rank == *furthest {
                return;
            }
        }
        at.insert(script.session(), (rank, entry.to_string()));
    });
}

/// The stock `UIParent.xml`'s unguarded callees, stood in for at load; the copy of
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
    -- the reference indexes, and the wrappers in `GameMenuAdapters.xml` are what tell these two
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

/// The stock micro-button row's unguarded reads, stood in for on the row's first call; the copy of
/// `ui_script::test_ui::MICRO_BUTTON_STAND_INS`.
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
