//! The capture scenarios: named deterministic viewpoints (camera eye and look in raw WoW coords,
//! pinned game minute, optional UI fixture) and the golden table. Data only.

/// A named deterministic capture viewpoint: where the camera stands and looks, and the game minute.
#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    /// The `Map.dbc` id the coords belong to, since raw WoW coords repeat on every continent;
    /// `None` leaves the `$WOW_MAP` knob to decide, for the instruments that go anywhere.
    pub(super) map: Option<u32>,
    /// Camera eye, raw WoW coords `(x, y, z)`.
    pub(super) eye: [f32; 3],
    /// Camera look-at target, raw WoW coords.
    pub(super) look: [f32; 3],
    /// Game minute of day (`0..1440`), pinning the time-of-day lighting.
    pub(super) minute: u32,
    /// A UI window opened with canned state before the shot.
    pub(super) ui: Option<UiFixture>,
}

/// A UI window opened with synthetic state that mirrors what the server sends, so the capture
/// runs the real feed, VM, extract and render chain.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum UiFixture {
    /// The player UI with no window opened: the chrome that is there once the UI loads and the
    /// synthetic unit snapshot lands (`ui_script::demo_unit_feed`). A UI capture all the same, so
    /// [`ui_opted_in`] reads `ui.is_some()`.
    Bare,
    Merchant,
    Gossip,
    Quest,
    /// The bank window fed through a synthetic self-player descriptor: `PLAYER_FIELD_BANK_SLOT`
    /// guids, a bank bag, the purchased count in `PLAYER_BYTES_2` byte 2, and coinage.
    Bank,
    /// The multi-quest greeting panel (`QUEST_GREETING`, `QuestGreetingPanel`): a greeting line
    /// over `UI-Quest-BulletPoint` title rows.
    QuestGreeting,
    /// The quest log fed through a synthetic self-player's `PLAYER_QUEST_LOG` slots.
    QuestLog,
    Loot,
    Bag,
    /// The cooldown sweep at sixteen phases in one still: each backpack slot's
    /// `GetContainerItemCooldown` sits at its own fraction of one long cooldown, in reading order
    /// (slot 1 renders top-left, `ContainerFrame_GenerateFrame` numbering backwards). The VM clock
    /// `__benilla_now` is parked at a large value, so the settle's seconds are ~5e-4 of a phase.
    Cooldown,
    /// The cooldown filmstrip with the pet bar's autocast shine beside it: `UI-AutoCastButton.m2`
    /// is four additive emitters and no batch, sharing the tile atlas, so this asks whether its
    /// particles stay inside their own cell.
    CooldownShine,
    /// The bag window with the GameTooltip forced open over a known slot.
    Tooltip,
    /// The world-mouseover tooltip over a seeded unit: the default anchor puts it at the screen's
    /// bottom-right (`GameTooltip.lua:73-77`), never on the hovered model.
    TooltipWorld,
    /// The character window fed through a synthetic self player's stat block and equipped item
    /// guids, with the items in [`crate::items::Items`].
    Character,
    /// The world-entry loading screen held up with its `GameTips.dbc` tip: it is otherwise up for a
    /// second on a path nothing can pause.
    LoadingTip,
    /// The shared StaticPopup plate as the group-invite dialog (`PARTY_INVITE`), the one that
    /// floats over the world rather than another window's art.
    PartyInvite,
    /// A V-key nameplate over a synthetic Timber Wolf (entry 69, level 2, faction 32, display 604).
    /// At the forced 1024×768 window one gx unit is 1280 px, so the 0.1 × 0.025 plate lands at
    /// 128×32 logical px, the border texture's native size.
    VPlates,
    /// The stock world map opened at the Elwynn zone map with alternating explore bits, so the
    /// exploration overlays and the parchment both show.
    WorldMap,
    /// The spellbook over a seeded known-spell set resolved through the real chain (`Spell.dbc`,
    /// `SkillLineAbility.dbc`, the book feed).
    SpellBook,
    /// The macro window over a macro set made through the live `CreateMacro` path, second slot
    /// selected.
    Macro,
    /// The macro window's name and icon popup: the icon grid off `SpellIcon.dbc`, the name box.
    MacroPopup,
    /// The chat edit box opened with a typed draft through the live path (`focus_editbox`,
    /// `chat_edit_live`) over say and yell lines.
    ChatEdit,
    /// The chat dock revealed, Combat Log selected, the cursor on the General tab; `$WOW_TABHOVER`
    /// picks an alternate dock state (`fixtures.rs`).
    ChatTabHover,
    /// The social pane (`FriendsFrame`) opened through the live toggle. Its `FriendsDropDown` has
    /// no anchors, as in the reference, and must draw nothing at the screen origin.
    Social,
    /// Our Options window opened through the live panel path, Controls selected by default.
    Options,
    /// The Options window on the Audio page. The options fixtures read the CVar registration
    /// defaults, since a capture loads no config file.
    OptionsAudio,
    /// The Options window on the Graphics page.
    OptionsGraphics,
    /// The Options window on the Chat page (1.12's `CHAT_LABEL`), whose rows mix a saved variable
    /// and CVars in one column.
    OptionsChat,
    /// The colour picker seeded at a known colour, so the wheel and brightness markers, on art this
    /// client generates, sit somewhere checkable.
    ColorPicker,
    /// The Controls page with the Camera Following Style dropdown open: `DropDownList1` at the
    /// window's effective scale, the stored entry checked.
    OptionsDropdownList,
    /// The Options window mid-search: "volume" lists the four volume sliders under the Audio head.
    OptionsSearch,
    /// The Options window's Keybindings page, Movement expanded, over the 1.12 default bindings and
    /// GlobalStrings; the command registry registers in-fixture.
    KeyBindings,
    /// An overhead name with the river behind it: a named unit 25 yd out in the Elwynn river (the
    /// `water-noon` camera). Deep water is opaque (`WATER_DEEP_ALPHA` 1.0), so a name sorted before
    /// the liquid is painted out; it must read at full strength.
    NameWater,
    /// One cell of the lighting matrix: a creature or GameObject spawned through the live path at
    /// `at` ([`SubjectKind`], the note above [`SUBJECT_SUN`]).
    Subject {
        kind: SubjectKind,
        /// Where the subject stands, raw WoW coords: its feet, not its body centre.
        at: [f32; 3],
    },
}

/// What the lighting matrix puts in frame: both spawn with a streamed entity's component set,
/// differing only in `EntityKind`.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SubjectKind {
    /// The Timber Wolf (entry 69, display 604), the `vplates` and `name-water` subject.
    Creature,
    /// `World\SkillActivated\Containers\TreasureChest01.mdx` (`GameObjectDisplayInfo` 259) at its
    /// closed rest pose.
    Chest,
}

/// The on-demand Northshire framings at the Human start (`SPAWN_XY` (-8949.95, -132.49), ground
/// ≈ 83.5): a ground overlook down at terrain and the Abbey, and a sky view up at the dome.
pub(super) const GROUND_EYE: [f32; 3] = [-8980.0, -160.0, 110.0];
pub(super) const GROUND_LOOK: [f32; 3] = [-8949.95, -132.49, 84.0];
pub(super) const SKY_EYE: [f32; 3] = [-8980.0, -160.0, 112.0];
pub(super) const SKY_LOOK: [f32; 3] = [-8740.0, 80.0, 168.0]; // horizon in the lower third

// Farmhouse compass looks: an ordinary building shows defects the Abbey is immune to.
pub(super) const HOUSE_EYE: [f32; 3] = [-9439.1, 71.2, 68.0];

/// `Map.dbc` ids the golden spots stand on.
pub(super) const MAP_AZEROTH: u32 = 0;
pub(super) const MAP_KALIMDOR: u32 = 1;
pub(super) const MAP_DEEPRUN_TRAM: u32 = 369;

/// A glue-screen capture: a login-side screen with no world, camera or map, sharing only the
/// shutter with [`Scenario`]. Not in the golden sweep; capturable by name.
#[derive(Clone, Copy)]
pub(super) struct GlueScenario {
    pub(super) name: &'static str,
    pub(super) screen: GlueScreen,
    /// The preview's race, sex and class ids, applied through `WOW_CHARCREATE_PICK` unless that is
    /// already set (`WOW_CHARCREATE_PICK=6,0,11 WOW_CAPTURE=glue-charcreate` is the tauren stage).
    pub(super) pick: Option<(u8, u8, u8)>,
}

/// Which glue screen a [`GlueScenario`] photographs. The select screen needs a roster and is not
/// here.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GlueScreen {
    /// Character creation: the `UI_*` backdrop scene's rig lighting and per-race fog, the preview
    /// body in its starting outfit, the GlueXML panel.
    CharCreate,
    /// The login screen, `UI_MainMenu` behind the account form. Its backdrop is the narrowest of
    /// the seven scenes, so a wide window (`WOW_WIN=2560x1440`) reaches its edges first.
    Login,
}

/// The glue scenarios: character creation as a human male warrior, the race the reference's own
/// screenshots use, and the login screen.
pub(super) const GLUE_SCENARIOS: &[GlueScenario] = &[
    GlueScenario {
        name: "glue-charcreate",
        screen: GlueScreen::CharCreate,
        pick: Some((1, 0, 1)),
    },
    GlueScenario {
        name: "glue-login",
        screen: GlueScreen::Login,
        pick: None,
    },
];

/// The golden baseline: two spots at noon and at night, Elwynn water and a Felwood hollow on
/// Kalimdor, their coordinates recorded with `/shot` (`benilla-config/shots.txt`). Held small on
/// purpose: a viewpoint being worked on belongs in [`ON_DEMAND`], capturable by name.
pub(super) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "water-noon",
        map: Some(MAP_AZEROTH),
        eye: WATER_EYE,
        look: WATER_LOOK,
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "water-night",
        map: Some(MAP_AZEROTH),
        eye: WATER_EYE,
        look: WATER_LOOK,
        minute: 0,
        ui: None,
    },
    Scenario {
        name: "felwood-noon",
        map: Some(MAP_KALIMDOR),
        eye: FELWOOD_EYE,
        look: FELWOOD_LOOK,
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "felwood-night",
        map: Some(MAP_KALIMDOR),
        eye: FELWOOD_EYE,
        look: FELWOOD_LOOK,
        minute: 0,
        ui: None,
    },
];

/// The Northshire overlook, north-east of the Abbey looking at it: terrain, trees, the Abbey WMO,
/// stained glass, props.
pub(super) const OVERLOOK_EYE: [f32; 3] = [-8955.0, -98.5, 91.1];
pub(super) const OVERLOOK_LOOK: [f32; 3] = [-8912.9, -125.4, 87.7];

/// The Elwynn river south-east of Northshire: open water, the shoreline blend, a murloc camp, fog.
pub(super) const WATER_EYE: [f32; 3] = [-9527.0, -310.6, 70.8];
pub(super) const WATER_LOOK: [f32; 3] = [-9499.4, -351.3, 61.4];

/// The Lion's Pride Inn common room at Goldshire: the hearth's MOCV-alpha self-illumination, a
/// daylight window, the chandelier, props; it exercises portal culling, the interior bake, MOLT
/// point lights and WMO props.
///
/// The camera must stand over a floor face: over a floorless pocket the portal cull's down-ray
/// reads outside and culls the containing group.
pub(super) const INN_EYE: [f32; 3] = [-9471.4, 39.4, 59.9];
pub(super) const INN_LOOK: [f32; 3] = [-9458.8, -7.5, 48.2];

/// An Elwynn rail fence across the sun's shadow boundary at 10:24: one span in sun, one in shade,
/// so both states of the MCSH sun term share one frame and only their difference carries the shot.
/// The fence is a doodad, whose shade is baked per vertex rather than ramped by `entity_shade`.
pub(super) const FENCE_EYE: [f32; 3] = [-9511.9, -4.0, 61.9];
pub(super) const FENCE_LOOK: [f32; 3] = [-9552.0, 18.6, 42.4];

/// A Felwood hollow on Kalimdor: the root mat, emissive `felwoodmushroom` doodads, a sludge pool
/// (a liquid type no other golden shot has), the zone's green fog and light. Its ADT tile
/// (`33_24`) exists in Azeroth too, empty.
pub(super) const FELWOOD_EYE: [f32; 3] = [4060.9, -944.3, 256.8];
pub(super) const FELWOOD_LOOK: [f32; 3] = [4014.0, -954.4, 242.9];

// ---------------------------------------------------------------------------------------------
// The lighting matrix: one subject, three lanes, two sides.
//
// The golden spots hold no creature or GameObject, so these cells cover the object light path.
// The three positions are the three lanes an object's light can take, found with
// `WOW_LIGHT_AT`/`WOW_LIGHT_GRID` (`wmo_portal::audit::light_probe`):
//
//   SUN    (-9500, 56)      terrain z 56.48  MCSH false  exterior-on-terrain, sun term at full
//   SHADE  (-9500, 44)      terrain z 55.95  MCSH true   exterior-on-terrain, sun term dimmed
//   INDOOR (-9469.4, 31.9)  no terrain       zone-text indoor 5, interior bake group 05, no sun
//
// SUN and SHADE differ only in the baked shadow bit `entity_shade` ramps on (2.5 lit, 0.5
// shadowed). The lighting sun sits near azimuth 45° (`sun::follow`): `front` puts the camera on
// that bearing, facing the lit side, `rear` opposite. Every cell frames its subject identically,
// so a diff is about light.

/// Lighting-matrix subject positions, the feet, raw WoW coords.
pub(super) const SUBJECT_SUN: [f32; 3] = [-9500.0, 56.0, 56.48];
pub(super) const SUBJECT_SHADE: [f32; 3] = [-9500.0, 44.0, 55.95];
pub(super) const SUBJECT_INDOOR: [f32; 3] = [-9469.4, 31.9, 57.9];

/// Every other named viewpoint, capturable by name (`WOW_CAPTURE=<name>`) but not in the sweep.
pub(super) const ON_DEMAND: &[Scenario] = &[
    // ---- The Deeprun Tram's undersea tube (map 369) ----
    // The one shipped map with no `Light.dbc` row, not even the falloff-0 global maps 0 and 1
    // carry, so its atmosphere is the WMO's own MFOG (record 2: RGB(30,53,100), end 236.1 yd,
    // start scalar 0.05); and a global (WDT `MODF`) WMO.
    //
    // Do not baseline it: server-less, no group of the WMO reaches the frame, so the shot is the
    // bare sky dome from any eye, though the live client draws the map. Kept as the reproducer.
    Scenario {
        name: "tram-undersea",
        map: Some(MAP_DEEPRUN_TRAM),
        eye: TRAM_EYE,
        look: TRAM_LOOK,
        minute: 720,
        ui: None,
    },
    // ---- The Tainted Scar, Blasted Lands ----
    // Looking up across the crater, mostly sky. The zone's `LightParams` 36 has cloud density
    // 0.85 (Elwynn 0.50), so the cloud layer's colour is the sky: the cloud-palette reproducer.
    Scenario {
        name: "tainted-scar-noon",
        map: Some(MAP_AZEROTH),
        eye: SCAR_EYE,
        look: SCAR_LOOK,
        minute: 720,
        ui: None,
    },
    // ---- Out of the baseline sweep ----
    // `chest-shade-{front,rear}` are not reproducible (two runs of one build: MAE 2.721 / 2.649,
    // ~7.8 % of pixels).
    Scenario {
        name: "overlook-noon",
        map: Some(MAP_AZEROTH),
        eye: OVERLOOK_EYE,
        look: OVERLOOK_LOOK,
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "overlook-night",
        map: Some(MAP_AZEROTH),
        eye: OVERLOOK_EYE,
        look: OVERLOOK_LOOK,
        minute: 0,
        ui: None,
    },
    // No `inn-night`: hearth and candles light the room, so the clock barely reaches it (MAE
    // 0.198 against `inn-noon`).
    Scenario {
        name: "inn-noon",
        map: Some(MAP_AZEROTH),
        eye: INN_EYE,
        look: INN_LOOK,
        minute: 720,
        ui: None,
    },
    // At 10:24, not noon: the shadow boundary is the subject, and at noon it slides off the rails.
    Scenario {
        name: "fence-shadowline-day",
        map: Some(MAP_AZEROTH),
        eye: FENCE_EYE,
        look: FENCE_LOOK,
        minute: 624,
        ui: None,
    },
    // The shadow bit is baked and present at every hour; the night frame pins how much of the shade
    // term survives once the ambient and moon palette carry the frame.
    Scenario {
        name: "fence-shadowline-night",
        map: Some(MAP_AZEROTH),
        eye: FENCE_EYE,
        look: FENCE_LOOK,
        minute: 0,
        ui: None,
    },
    Scenario {
        name: "creature-sun-front",
        map: Some(MAP_AZEROTH),
        eye: [-9496.46, 59.54, 58.28],
        look: [-9500.00, 56.00, 57.28],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_SUN,
        }),
    },
    Scenario {
        name: "creature-sun-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9503.54, 52.46, 58.28],
        look: [-9500.00, 56.00, 57.28],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_SUN,
        }),
    },
    Scenario {
        name: "creature-shade-front",
        map: Some(MAP_AZEROTH),
        eye: [-9496.46, 47.54, 57.75],
        look: [-9500.00, 44.00, 56.75],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_SHADE,
        }),
    },
    Scenario {
        name: "creature-shade-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9503.54, 40.46, 57.75],
        look: [-9500.00, 44.00, 56.75],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_SHADE,
        }),
    },
    Scenario {
        name: "creature-indoor-front",
        map: Some(MAP_AZEROTH),
        eye: [-9466.57, 34.73, 59.70],
        look: [-9469.40, 31.90, 58.70],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_INDOOR,
        }),
    },
    Scenario {
        name: "creature-indoor-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9472.23, 29.07, 59.70],
        look: [-9469.40, 31.90, 58.70],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Creature,
            at: SUBJECT_INDOOR,
        }),
    },
    Scenario {
        name: "chest-sun-front",
        map: Some(MAP_AZEROTH),
        eye: [-9496.82, 59.18, 57.98],
        look: [-9500.00, 56.00, 56.93],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_SUN,
        }),
    },
    Scenario {
        name: "chest-sun-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9503.18, 52.82, 57.98],
        look: [-9500.00, 56.00, 56.93],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_SUN,
        }),
    },
    Scenario {
        name: "chest-shade-front",
        map: Some(MAP_AZEROTH),
        eye: [-9496.82, 47.18, 57.45],
        look: [-9500.00, 44.00, 56.40],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_SHADE,
        }),
    },
    Scenario {
        name: "chest-shade-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9503.18, 40.82, 57.45],
        look: [-9500.00, 44.00, 56.40],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_SHADE,
        }),
    },
    // Not reproducible (two runs of one build: front MAE 1.551 / 9.2 % of pixels, rear 0.529 /
    // 7.6 %): the whole body shifts brightness in registration, so the GameObject's interior light
    // lane does not converge by the shutter, while the creature at the same spot is bit-identical.
    Scenario {
        name: "chest-indoor-front",
        map: Some(MAP_AZEROTH),
        eye: [-9466.85, 34.45, 59.40],
        look: [-9469.40, 31.90, 58.35],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_INDOOR,
        }),
    },
    Scenario {
        name: "chest-indoor-rear",
        map: Some(MAP_AZEROTH),
        eye: [-9471.95, 29.35, 59.40],
        look: [-9469.40, 31.90, 58.35],
        minute: 720,
        ui: Some(UiFixture::Subject {
            kind: SubjectKind::Chest,
            at: SUBJECT_INDOOR,
        }),
    },
    Scenario {
        name: "house-north",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9389.1, 71.2, 58.0],
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "house-south",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9489.1, 71.2, 58.0],
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "house-west",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9439.1, 121.2, 58.0],
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "house-east",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9439.1, 21.2, 58.0],
        minute: 720,
        ui: None,
    },
    // At midnight the SIDN night fraction (`grade.x`) is 1.0, so the MOMT 0x10 window glow is live;
    // every other WMO scenario sits before the 20:30 ramp.
    Scenario {
        name: "house-north-midnight",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9389.1, 71.2, 58.0],
        minute: 0,
        ui: None,
    },
    // The inn kitchen: its hearth carries the building's strongest MOCV-alpha bake, α≈100 at the
    // firebox (group-local (-32.9, 1.5, 2), world ≈ (-9461.7, -8.4, 58) per MODF uid 71414 on tile
    // 31,49, origin (-9464.25, 24.39, 56.53), rot -97°). Over a floor face, as for `INN_EYE`;
    // the reference culls a floorless pocket's group the same way (outside leg `0x6811ca`).
    Scenario {
        name: "inn-interior",
        map: Some(MAP_AZEROTH),
        eye: [-9463.3, 4.4, 58.8],
        look: [-9462.1, -5.6, 58.5],
        minute: 720,
        ui: None,
    },
    Scenario {
        name: "northshire-dusk",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 1170, // 19:30, warm dusk light and fog
        ui: None,
    },
    Scenario {
        name: "northshire-sky-noon",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: SKY_LOOK,
        minute: 720, // day sky-dome gradient + fog horizon
        ui: None,
    },
    Scenario {
        name: "northshire-sky-dusk",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: SKY_LOOK,
        minute: 1170, // dusk dome warp + low sun + stars emerging
        ui: None,
    },
    // Straight into the sun at 17:30 (elevation ≈30°, azimuth 45°, clear of the Northshire ridge)
    // with the view lerp at max: the 20-unit sunGlare quad must fade off with no hard edge.
    Scenario {
        name: "northshire-sun-flare",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8797.0, 23.0, 264.0], // eye + 300·(elev 30°, az 45°), the sun at 17:30
        minute: 1050,
        ui: None,
    },
    // The moon rising at 22:44 (azimuth 45°, elevation ≈15°): the disc rises edge-first behind the
    // ridge (per-pixel terrain occlusion), and no glare ring shows, the moon dnCurve being zero
    // until 22:45.
    Scenario {
        name: "northshire-moonrise",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8775.0, 45.0, 190.0], // eye + 300·(elev 15°, az 45°), the moon at 22:44
        minute: 1364,
        ui: None,
    },
    // Midnight, moon overhead (azimuth 45°, elevation 55°), dnCurve 1.0, star curve 1.0: the disc
    // and its glare ring at full strength over the star field.
    Scenario {
        name: "northshire-moon-halo",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8858.0, -38.0, 358.0], // eye + 300·(elev 55°, az 45°), the moon at 00:00
        minute: 0,
        ui: None,
    },
    // The `.tele Stormwind` spot (vmangos `game_tele`: -8833.38, 628.63, 94.01, o=1.065) at head
    // height into the Trade District: the city-scale perf scene, the whole city WMO resident.
    Scenario {
        name: "stormwind",
        map: Some(MAP_AZEROTH),
        eye: [-8833.38, 628.63, 96.0],
        look: [-8809.1, 672.3, 94.0],
        minute: 720,
        ui: None,
    },
    // The UI window fixtures: each a shipped window with canned state over the noon ground view.
    Scenario {
        name: "ui-merchant",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Merchant),
    },
    Scenario {
        name: "ui-gossip",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Gossip),
    },
    Scenario {
        name: "ui-bank",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bank),
    },
    Scenario {
        name: "ui-quest",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Quest),
    },
    Scenario {
        name: "ui-questgreeting",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::QuestGreeting),
    },
    Scenario {
        name: "ui-questlog",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::QuestLog),
    },
    Scenario {
        name: "ui-loot",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Loot),
    },
    Scenario {
        name: "ui-bag",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bag),
    },
    // Sixteen bag slots, sixteen cooldown phases, one still.
    Scenario {
        name: "ui-cooldown",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Cooldown),
    },
    // The filmstrip with the autocast shine sharing the atlas: the cell-bleed instrument.
    Scenario {
        name: "ui-cooldown-shine",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::CooldownShine),
    },
    Scenario {
        name: "ui-tooltip",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Tooltip),
    },
    // The default anchor, screen bottom-right (−13/+70), over a seeded hostile wolf.
    Scenario {
        name: "ui-tooltip-world",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::TooltipWorld),
    },
    Scenario {
        name: "ui-char",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Character),
    },
    // Player and target frames from `demo_unit_feed`'s synthetic snapshots.
    Scenario {
        name: "ui-unitframes",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    // `demo_unit_feed` seeds a rogue with four points on the wolf for this scenario only; the demo
    // player is otherwise a warrior, which lights no dot.
    Scenario {
        name: "ui-combopoints",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    Scenario {
        name: "ui-partyinvite",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::PartyInvite),
    },
    // The main action bar, its slots and XP seeded by `demo_unit_feed`. The bar is 1024 wide plus
    // 128 px end caps, so `lib.rs` gives this scenario a wider, shorter window.
    Scenario {
        name: "ui-actionbar",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    // Framed like the reference screenshot: an eye-height look at a wolf ~8 yd off. Plates are
    // widgets hung off `WorldFrame`, so they need this scenario's `ui:` fixture; `lib.rs` sizes the
    // window 1024×768, the 1:1 gx window.
    Scenario {
        name: "vplates",
        map: Some(MAP_AZEROTH),
        eye: [-8956.5, -137.5, 85.6],
        look: [-8949.95, -132.49, 84.8],
        minute: 720,
        ui: Some(UiFixture::VPlates),
    },
    // `lib.rs` gives the map's centred 1024×768 chrome a 1100×800 window.
    Scenario {
        name: "ui-worldmap",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::WorldMap),
    },
    // A human warrior's book seeded with two off-class spells.
    Scenario {
        name: "ui-spellbook",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::SpellBook),
    },
    Scenario {
        name: "ui-macro",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Macro),
    },
    Scenario {
        name: "ui-macro-popup",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::MacroPopup),
    },
    Scenario {
        name: "ui-chatedit",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ChatEdit),
    },
    // The tab plate and its additive highlight over the world, which the UI-over-world composite
    // decides.
    Scenario {
        name: "ui-chat-tabhover",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ChatTabHover),
    },
    Scenario {
        name: "ui-social",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Social),
    },
    // The backdrop is whatever `Map.dbc` and `LoadingScreens.dbc` give this map, so the shot covers
    // the art chain and the bar as well as the tip.
    Scenario {
        name: "loading-tip",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::LoadingTip),
    },
    Scenario {
        name: "ui-options",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Options),
    },
    Scenario {
        name: "ui-options-audio",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsAudio),
    },
    Scenario {
        name: "ui-options-graphics",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsGraphics),
    },
    Scenario {
        name: "ui-options-chat",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsChat),
    },
    Scenario {
        name: "ui-color-picker",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ColorPicker),
    },
    Scenario {
        name: "ui-options-dropdown",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsDropdownList),
    },
    Scenario {
        name: "ui-keybindings",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::KeyBindings),
    },
    Scenario {
        name: "ui-options-search",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsSearch),
    },
    // Same camera as `water-noon`, plus a named unit out in the water.
    Scenario {
        name: "name-water",
        map: Some(MAP_AZEROTH),
        eye: WATER_EYE,
        look: WATER_LOOK,
        minute: 720,
        ui: Some(UiFixture::NameWater),
    },
];

/// The `name-close` subject: the `name-water` wolf's name, orbited at any distance. World text is
/// the only consumer that draws the glyph sheet at other than 1:1, so a glyph-cell sampling defect
/// shows only in a magnified name.
///
/// Knobs, defaults in parentheses: `WOW_NAME_DIST` yd from the name (4), `WOW_NAME_AZ` the eye's
/// bearing in degrees, 0 = +X (124), `WOW_NAME_EL` its elevation in degrees (8), `WOW_NAME_H` the
/// name's height above the feet in yd (1.4). The camera looks straight at the name.
pub(super) const NAME_CLOSE_AT: [f32; 3] = super::fixtures::NAME_WATER_POS;

/// The Deeprun Tram tube (`tram-undersea`). The Subway WMO is the map's global `MODF` at the
/// origin with identity rotation, so these are also its model-space coords.
pub(super) const TRAM_EYE: [f32; 3] = [-2.44, -1250.0, -120.0];
pub(super) const TRAM_LOOK: [f32; 3] = [-2.44, -1400.0, -118.0];

/// The Tainted Scar (`tainted-scar-noon`): the eye is `.go xyz -11892.70 -2647.08 -4.68`
/// (`game_tele TheTaintedScar`) lifted clear of the crater floor, looking north 25° up, since the
/// rim hides the dome from a level look; the ridge sits low in the frame as the control.
pub(super) const SCAR_EYE: [f32; 3] = [-11892.7, -2647.1, 20.0];
pub(super) const SCAR_LOOK: [f32; 3] = [-11792.7, -2647.1, 66.6];

/// Find a scenario by name across both tables, as `WOW_CAPTURE=` resolution does; every `ui-*`
/// fixture lives in [`ON_DEMAND`].
pub(super) fn by_name(name: &str) -> Option<&'static Scenario> {
    SCENARIOS
        .iter()
        .chain(ON_DEMAND.iter())
        .find(|s| s.name == name)
}

fn scenario_declares_ui() -> bool {
    std::env::var("WOW_CAPTURE")
        .ok()
        .and_then(|name| by_name(&name))
        .is_some_and(|s| s.ui.is_some())
}

/// Whether the player UI is opted into this capture: the scenario declares a `ui:` fixture, or
/// `WOW_CAPTURE_UI=1` asks for it over a world scene, whose baselines are otherwise UI-free.
///
/// Its three consumers must agree, or the capture is plausible and wrong: the UI load
/// (`ui_script::lifecycle::ui_wanted`), the synthetic `"player"`/`"target"` snapshot
/// (`ui_script::capture_ui_active`, `demo_unit_feed`) and the window size (`lib.rs`).
pub(crate) fn ui_opted_in() -> bool {
    std::env::var("WOW_CAPTURE_UI").as_deref() == Ok("1") || scenario_declares_ui()
}

#[cfg(test)]
mod ui_opt_in_tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// A window scenario opts the UI in without `WOW_CAPTURE_UI=1`; a world scenario does not.
    #[test]
    fn a_ui_scenario_opts_the_ui_in_and_a_world_scenario_does_not() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ui = EnvGuard::unset("WOW_CAPTURE_UI");

        let _c = EnvGuard::set("WOW_CAPTURE", "ui-questlog");
        assert!(
            ui_opted_in(),
            "a `ui:` scenario is a UI capture by construction"
        );

        // A `Bare` fixture opens no window and still opts in.
        let _c = EnvGuard::set("WOW_CAPTURE", "ui-unitframes");
        assert!(ui_opted_in(), "…including the fixtureless `Bare` ones");

        // A world scenario's baseline tests the world render and stays UI-free.
        let world = SCENARIOS
            .iter()
            .chain(ON_DEMAND.iter())
            .find(|s| s.ui.is_none())
            .expect("the tables have world scenarios");
        let _c = EnvGuard::set("WOW_CAPTURE", world.name);
        assert!(
            !ui_opted_in(),
            "world scenario {} must stay pristine",
            world.name
        );

        // The env var opts a world scene in.
        let _on = EnvGuard::set("WOW_CAPTURE_UI", "1");
        assert!(ui_opted_in(), "the env var still opts a world scene in");
    }

    /// A `ui-*` name without a fixture would be a world capture wearing a UI name.
    #[test]
    fn every_ui_named_scenario_declares_a_fixture() {
        for s in SCENARIOS
            .iter()
            .chain(ON_DEMAND.iter())
            .filter(|s| s.name.starts_with("ui-"))
        {
            assert!(
                s.ui.is_some(),
                "scenario {} is named ui-* but declares no `ui:` fixture — it would be treated as \
                 a world capture and photographed with no player UI. If it opens no window, that \
                 is what `UiFixture::Bare` is for.",
                s.name
            );
        }
    }
}
