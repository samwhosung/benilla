//! The capture *scenarios* — the named deterministic viewpoints (camera eye/look in raw WoW
//! coords, pinned game-minute, optional UI fixture) and the golden-scenario table itself. Data
//! only; the capture lifecycle (settle, screenshot, probe) stays in `super`.

/// A named, fully-deterministic capture viewpoint: where the camera sits + looks (raw WoW coords) and
/// the game-minute to render. One scenario → one golden PNG.
#[derive(Clone, Copy)]
pub(super) struct Scenario {
    pub(super) name: &'static str,
    /// `Map.dbc` id the eye/look coords belong to. Raw WoW coords repeat on every continent — the
    /// Felwood spot's tile (`33_24`) exists in Azeroth too, empty — so a scenario that could not
    /// name its map silently photographed the wrong world. The harness seeds
    /// [`benilla_world::world_map::CurrentMap`] from this before streaming starts.
    /// Which `Map.dbc` map this scenario photographs. `None` = **the `$WOW_MAP` knob decides** —
    /// the arbitrary-viewpoint instruments (`vista`, `waterfx`, `fxview`) go anywhere, so they have
    /// no map of their own and must not write one back over the knob. A named golden scenario
    /// always says, because raw WoW coords repeat on every continent.
    pub(super) map: Option<u32>,
    /// Camera eye, raw WoW coords `(x, y, z)`.
    pub(super) eye: [f32; 3],
    /// Camera look-at target, raw WoW coords.
    pub(super) look: [f32; 3],
    /// Game minute of day (`0..1440`) — pins the time-of-day lighting.
    pub(super) minute: u32,
    /// Open this UI window with canned state before the shot (the UI half of the harness — the
    /// look-pass instrument the 2026-07-03 director round demanded: window fidelity gets checked
    /// by MY eyes on a capture before it ever reaches the director's).
    pub(super) ui: Option<UiFixture>,
}

/// A UI window opened with synthetic-but-realistic state for a deterministic capture. The seed data
/// mirrors what the live server sends (names/icons resolve through the same offline caches the app
/// uses), so the capture exercises the real feed → VM → extract → render chain end to end.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum UiFixture {
    /// **The player UI, with no window opened.** `ui-unitframes`, `ui-combopoints` and
    /// `ui-actionbar` photograph chrome that is simply *there* once the UI loads and the synthetic
    /// unit snapshot lands (`ui_script::demo_unit_feed`) — there is nothing to seed and nothing to
    /// click. They used to say `ui: None`, which made the field mean two things at once ("seed
    /// this window" *and* "does this capture include the UI at all") and left the second
    /// unstateable: they were UI captures that the table said were world captures, kept working
    /// only by `WOW_CAPTURE_UI=1` on the command line. One field, one meaning — so
    /// [`ui_opted_in`] can just ask `ui.is_some()`.
    Bare,
    Merchant,
    Gossip,
    Quest,
    /// The bank window (decision 0604 phase 4) fed the REAL way (the QuestLog/Character pattern):
    /// a synthetic self-player whose descriptor carries occupied `PLAYER_FIELD_BANK_SLOT` guids, a
    /// held bank bag, a purchased count in `PLAYER_BYTES_2` byte 2, and a coinage purse — so the
    /// capture exercises the whole live chain (descriptor → `feed_bank`/`ui_items` → Lua →
    /// render): vault slots with icons, bag buttons (owned icon / bought-empty / red unpurchased),
    /// and the purchase row's DBC cost.
    Bank,
    /// The multi-quest greeting panel (`QUEST_GREETING`, `QuestGreetingPanel`): a greeting
    /// line plus an "Available Quests" list of `UI-Quest-BulletPoint` title rows — the frame the
    /// gossip-vs-greeting confusion turned on, and previously uncaptured (bullet/title seating had
    /// no regression baseline).
    QuestGreeting,
    /// The quest-log book window fed the REAL way: a synthetic self-player entity
    /// whose descriptor carries occupied `PLAYER_QUEST_LOG` slots, so the capture exercises the
    /// whole live chain (descriptor → `feed_quest_log` → template cache → seam → Lua → render) —
    /// nothing pushed to the VM by hand.
    QuestLog,
    Loot,
    Bag,
    /// **The cooldown sweep, at sixteen phases in one still**.
    ///
    /// The backpack filled slot for slot, each slot's `GetContainerItemCooldown` triple parked at
    /// its own fraction of one very long cooldown — so the sixteen `…Cooldown` model panes
    /// (`CooldownFrameTemplate`, the reference's own `UI-Cooldown-Indicator.m2` through the tile
    /// lane) each hold a different point of the 1000 ms sweep and the window reads
    /// left-to-right, top-to-bottom as a filmstrip of it. Game slot 1 renders TOP-LEFT
    /// (`ContainerFrame_GenerateFrame` numbers backwards — see `seed_bag_window`), so ascending
    /// slot is reading order.
    ///
    /// **The clock is pinned so the phases are, too.** `GetTime()` is the VM's own accumulating
    /// session clock (`__benilla_now`), and the frame count to the shutter varies run to run — so
    /// an absolute start would put the sweep somewhere different every capture and the baseline
    /// could never diff. The fixture parks `__benilla_now` at a large value and gives every
    /// slot a very long cooldown, which makes the settle window's few seconds worth
    /// `~5e-4` of one phase step.
    Cooldown,
    /// **The cooldown filmstrip with the pet bar's autocast SHINE up beside it**.
    ///
    /// `UI-AutoCastButton.m2` is four additive spline emitters and no render batch at all — 300
    /// live golden particles (`GlowStar.blp`, born `(0.98, 0.87, 0.19)`) circling the button. It
    /// shares the tile atlas with every other `<Model>` pane, and a particle is a world quad in
    /// ATLAS space: this scenario is the one that asks whether it stays inside its own cell.
    CooldownShine,
    /// The bag window with the GameTooltip forced open over a known slot — the tooltip look-pass
    /// instrument (crisp border, tiled edges, tinted plate, quality-coloured item name, snug size).
    Tooltip,
    /// The WORLD-mouseover tooltip forced open over a seeded unit — the default-anchor
    /// instrument: the plate must sit at the screen's bottom-right corner
    /// (−CONTAINER_OFFSET_X−13, +CONTAINER_OFFSET_Y — ref GameTooltip.lua l.73-77), never on
    /// the hovered model. Captures the wiring whose absence parked it at screen center.
    TooltipWorld,
    /// The character window's paper doll (decision 0208 phase 1a) fed the REAL way (the QuestLog
    /// pattern): a synthetic self player whose descriptor carries the full stat block + equipped
    /// item guids, item objects/templates in the [`crate::items::Items`] stores — so the capture
    /// exercises the whole live chain (descriptor → `ui_char` feed → snapshots/events → Lua →
    /// render): slot icons, the attribute/resistance panes with buff coloring, melee + ranged
    /// blocks, the ammo count, the level line.
    Character,
    /// **The loading screen, held up, with the tip of the day on it**.
    ///
    /// Not a window: it raises the world-entry loading screen — per-map backdrop, progress bar,
    /// and the `TipEdge::Pick` that chooses a `GameTips.dbc` row — and pins it so the harness has
    /// a still frame to settle on. Every other screen in the client can be photographed by
    /// standing in front of it; this one is up for a second on a path nobody can pause, which is
    /// exactly why its tip could ship dead, log `tip 0 of 74` twice in a live smoke run, and be
    /// caught by nothing at all.
    LoadingTip,
    /// The shared **StaticPopup** plate, shown as the group-invite dialog (`PARTY_INVITE`).
    ///
    /// Every other StaticPopup customer in the tree raises its dialog over another window's opaque
    /// art, where a missing backdrop is invisible; the invite is the one that floats over the
    /// WORLD, which is exactly why its interior is where the plate's presence can be read at all
    /// (report B319). Nothing captured the StaticPopup plate before this.
    PartyInvite,
    /// A V-key nameplate over a synthetic Timber Wolf (the reference client's own screenshot
    /// subject: entry 69, level 2, faction 32, display 604) — the plate look-pass instrument.
    /// At the 1024×768 window this scenario forces, one gx unit = 1280 px, so the 0.1 × 0.025
    /// plate must land at exactly 128×32 logical px: the border texture's native size, directly
    /// diffable against the decoded BLP.
    VPlates,
    /// The world map in its default windowed mode (the 1.14-style small window), opened at the
    /// Elwynn zone map with alternating explore bits — one frame exercising the window chrome,
    /// the scaled map block, the exploration fog (revealed overlays over the parchment base),
    /// and the enlarged player arrow.
    WorldMap,
    /// The spellbook opened over a seeded known-spell set that resolves
    /// through the REAL chain (`PlayerActions.spells` → `Spell.dbc` × `SkillLineAbility.dbc` →
    /// the book feed → Lua → render): the panel plates, the 12-slot page with name/rank text,
    /// passive graying, the skill-line tab strip, and the page footer.
    SpellBook,
    /// The macro window over a seeded macro set, second slot selected — the look
    /// instrument the window shipped WITHOUT, which is why "CREATE_MACROS" across its title bar
    /// reached the director's screen instead of a capture. Pins the two-tab row inside the
    /// 384-wide plate, the 18-slot grid, the selected-macro detail pane, the body box, and the
    /// bottom button row. Macros are made through the live `CreateMacro` path, so the fixture
    /// exercises the real engine table → `UPDATE_MACROS` → window chain.
    Macro,
    /// The macro window's NAME/ICON POPUP open over the same set — the other half of the
    /// window, and the denser one: the 5×4 icon grid off the real `SpellIcon.dbc` catalog, its
    /// faux scroll bar, the name box, and the Okay/Cancel row.
    MacroPopup,
    /// The chat window with the edit box OPEN over a say/yell line mix (decision 0288 P5's look
    /// instrument, added for the centered-"Say:"/invisible-typing regression): the box focused
    /// with a typed draft through the live open path (`focus_editbox` + `chat_edit_live`), so the
    /// capture checks the header (left-flush, Say-white), the typed text past the live insets,
    /// and the three-piece input border.
    ChatEdit,
    /// The chat dock **hovered**: revealed, the Combat Log selected, and the cursor parked on the
    /// unselected GENERAL tab — the first capture scenario that exercises a hover state at all,
    /// which decision 0254 recorded as the gap that left its own lane uncovered. `$WOW_TABHOVER`
    /// swings it through the four alternate dock states (see `fixtures.rs`).
    ChatTabHover,
    /// The social pane (`FriendsFrame`), opened through the live toggle — the instrument for B264.
    /// Its `FriendsDropDown` host is declared with no anchors, exactly as
    /// the reference's own `FriendsDropDown` is; while an unpositioned owner stood in a zero rect
    /// for its regions, the dropdown template's texture chain drew a stray capsule at the screen
    /// origin every time the pane opened. Needs no server state to show it.
    Social,
    /// The era-styled Options window, opened through the live panel path —
    /// the look-pass instrument for the whole options arc: chrome nine-slice seams, tab plates,
    /// search-box seat, category list art, the window scale. Static (no server state touched),
    /// so its pixels move only when the window or the atlas seam does.
    Options,
    /// The Options window ON THE AUDIO PAGE — the setting-row look instrument:
    /// checkbox art at its era seats, the 1.12 slider groove + thumb on the bare full-width bar
    /// (steppers cut 0989), child-row indent/small-font, the percent readouts, Defaults armed.
    /// Rows read the CVar registration defaults (hermetic capture = no config file), so the
    /// pixels move only with the window, the atlas seam, or a registered default.
    OptionsAudio,
    /// The Options window ON THE GRAPHICS PAGE (farclip row retired 0961 and back
    /// 1513; Environment Detail joined 0992, a slider since 1649) — uiScale at its 0.64..1.0 panel
    /// range with the percent readout, and three slider grooves under it. Rows
    /// read the CVar registration defaults (hermetic capture = no config file), so the pixels
    /// move only with the window, the atlas seam, or a registered default.
    OptionsGraphics,
    /// The Options window ON THE CHAT PAGE (B246's "no chat section in options").
    /// The page 1.12 calls `CHAT_LABEL`: four checkbox rows over three stores at once (a saved
    /// variable, three CVars), which is what makes it worth a baseline of its own — the row art is
    /// the Audio page's, but the page is the first to mix stores in one column.
    OptionsChat,
    /// The **colour picker with its wheel**: the hue disc and the brightness strip,
    /// both generated art with no BLP behind them, plus the two markers riding on them. Seeded at a
    /// known colour so the markers sit somewhere checkable rather than at the default white's
    /// centre. The one fixture whose subject is pixels this client computes.
    ColorPicker,
    /// The Controls page with the Camera Following Style MENU OPEN — the dropdown-list look
    /// instrument: the shared DropDownList1 at the window's effective scale (the kit's uiScale
    /// correction), three entries with the stored one checked, the kit's dialog backdrop. It was
    /// Environment Detail's list until 1649 put that row back on the reference's slider;
    /// the instrument follows the shape, not the row, so it moved to a row that still has a list.
    OptionsDropdownList,
    /// The Options window MID-SEARCH — the results-view look instrument: the
    /// "volume" query reflows the four live volume sliders under the clickable Audio head
    /// (GameFontNormalLarge), title "Search Results", Defaults hidden, the clear-X shown in
    /// the box. Hermetic like the other options fixtures; pixels move only with the window
    /// or a registered default.
    OptionsSearch,
    /// The KEY BINDINGS window — the era-shaped standalone KeyBindingFrame on
    /// its Movement page: the category sidebar (gold-locked Movement Keys), Command/Key 1/
    /// Key 2 columns over the honest tree's rows with the byte-real 1.12 defaults ("Move and
    /// Steer — Middle Mouse" leading, straight off the real GlobalStrings the capture VM loads
    /// like any run), the character-specific checkbox, the four bottom buttons. The registry
    /// registers in-fixture (hermetic capture = the plugin's PostStartup seed is not raced),
    /// so the pixels move only with the window, a registered default, or a 1.12 string.
    KeyBindings,
    /// A **floating overhead name with the river surface behind it** — the world-text-vs-liquid
    /// draw-order instrument. A named unit stands 25 yd out in the Elwynn river (the `water-noon`
    /// camera), so its name projects onto the water *beyond* it: the exact geometry of the
    /// director's Stormwind-canal report, where the plate sorted BEFORE the liquid and deep
    /// water (`WATER_DEEP_ALPHA` = 1.0, fully opaque) painted the glyphs out. Nothing in the
    /// sweep covered this — decision 0519 wrote the law and shipped it with the sort bias
    /// pointing the wrong way, invisible to every gate. The name must read at full strength.
    NameWater,
    /// One cell of the **lighting matrix**: a creature or a GameObject spawned
    /// through the live path at `at`, so each lane an object's light can take is photographed from
    /// two sides. See [`SubjectKind`] and the matrix note above [`SUBJECT_SUN`].
    Subject {
        kind: SubjectKind,
        /// Where the subject stands, raw WoW coords — its FEET, not its body centre.
        at: [f32; 3],
    },
}

/// What the lighting matrix puts in frame. Both spawn with the same live component set a streamed
/// entity gets (`NetEntity` + descriptor), differing only in `EntityKind` — so the matrix exercises
/// the real unit and GameObject paths rather than a bespoke preview.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SubjectKind {
    /// The Timber Wolf (entry 69, display 604) — the same subject `vplates`/`name-water` use, so a
    /// defect seen here can be A/B'd against them directly.
    Creature,
    /// `World\SkillActivated\Containers\TreasureChest01.mdx` (`GameObjectDisplayInfo` 259) — the
    /// canonical world chest, at its closed rest pose.
    Chest,
}

/// The ON-DEMAND Northshire framings, anchored at the Human start (around `SPAWN_XY`
/// `(-8949.95, -132.49)`, ground ≈ 83.5). Two framings from one spot exercise the whole stack the
/// linear-HDR rework rebuilds:
/// - a **ground** overlook (camera pitched down at textured terrain + the Abbey) across the day arc —
///   terrain lighting, ambient, shadows, distance fog, model lighting;
/// - a **sky** view (camera pitched up at the dome + horizon) at day and dusk — the sky-dome gradient,
///   the fog horizon, and (at dusk) the warp + low sun + emerging stars.
pub(super) const GROUND_EYE: [f32; 3] = [-8980.0, -160.0, 110.0];
pub(super) const GROUND_LOOK: [f32; 3] = [-8949.95, -132.49, 84.0];
pub(super) const SKY_EYE: [f32; 3] = [-8980.0, -160.0, 112.0];
pub(super) const SKY_LOOK: [f32; 3] = [-8740.0, 80.0, 168.0]; // up + out: horizon in the lower third, dome above

// Farmhouse viewpoints: compass looks from the human-start login spot. Kept
// permanently — the pale-film regression was invisible for hours because every baseline framed the
// Abbey, one of the few buildings immune to it. Baselines must cover ordinary buildings too.
pub(super) const HOUSE_EYE: [f32; 3] = [-9439.1, 71.2, 68.0];

/// `Map.dbc` ids the golden spots stand on.
pub(super) const MAP_AZEROTH: u32 = 0;
pub(super) const MAP_KALIMDOR: u32 = 1;
pub(super) const MAP_DEEPRUN_TRAM: u32 = 369;

/// A **glue-screen** capture — the login-side screens, which have no world, no camera and no map.
///
/// A separate table rather than a variant of [`Scenario`], because the two share nothing but a
/// name and an output path: every field of a `Scenario` says where a camera stands in a streamed
/// world. What they do share is the shutter, and that is the part worth sharing — it waits for the
/// *image* to stop changing, which is as true of a glue screen as of a landscape.
///
/// Not in [`SCENARIOS`]: the golden sweep is the director's blessed set (0632's law, 0817's cut,
/// 1183's eviction) and growing it is their call, not the harness's. Capturable by name like any
/// on-demand viewpoint.
#[derive(Clone, Copy)]
pub(super) struct GlueScenario {
    pub(super) name: &'static str,
    pub(super) screen: GlueScreen,
    /// The preview's race, sex and class ids — the reference comparisons are per-race, so a
    /// scenario says which one it photographs. Applied through the existing
    /// `WOW_CHARCREATE_PICK` instrument rather than a second path into `CreateSelection` — and
    /// **that instrument wins when it is already set**, so one scenario photographs any race
    /// (`WOW_CHARCREATE_PICK=6,0,11 WOW_CAPTURE=glue-charcreate` is the tauren stage). `None` for
    /// a screen with no preview to pick.
    pub(super) pick: Option<(u8, u8, u8)>,
}

/// Which glue screen a [`GlueScenario`] photographs. The select screen wants a roster (so it
/// needs a server, or a seeded one) and is not here.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GlueScreen {
    /// The character-creation screen. Photographs, in one frame: the authored-rig
    /// lighting of the `UI_*` backdrop scene and its per-race fog (the lane 1171 left uncovered),
    /// the preview body in its starting outfit, and the GlueXML panel around them.
    CharCreate,
    /// The login screen — `UI_MainMenu` behind the account form. Nearly static,
    /// which is why it was not here until its framing was the subject: the
    /// gate's backdrop is the narrowest art of the seven scenes, so it is the one whose edges a
    /// wide window reaches first. `WOW_WIN=2560x1440` is the reporter's shape.
    Login,
}

/// The glue sweep. Human, 1 (Human male warrior) — the race whose scene the reference's own
/// screenshots use, and the outfit path most exercised elsewhere — and the login gate.
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

/// THE golden baseline: **two spots FRAMED BY THE DIRECTOR, each at noon and at night — four
/// captures, and that is the whole sweep.** Elwynn water and a Felwood hollow on Kalimdor. The
/// numbers below are verbatim from their `benilla-config/shots.txt` ([`/shot`] — it was
/// `~/.benilla/shots.txt` when they were taken; decision 1181 moved it).
///
/// **The Stormwind canal was the third spot and is deleted, not evicted** (at the
/// director's instruction — the one place the "nothing is deleted" rule below does not apply). It
/// stopped holding the bar of 0 by one pixel per cell, and the cause is a renderer defect that is
/// still open: two WMO surfaces intersect there, one MSAA sample sits in an exact depth tie, and
/// reverse-Z `GreaterEqual` gives that sample to whichever batch drew last — which varies because
/// draw order follows spawn order (1182 measured it, 16/16). **Deleting the shot is not a fix**; it
/// removes the only cell that was reporting the bug. 1182 holds the repro recipe.
///
/// **Held small on purpose.** 0632 cut a thirty-shot set down to six for exactly
/// this reason and stated the law — *a few spots chosen with the director, times a couple of day
/// times* — and then the set grew back to **twenty-one** one well-argued addition at a time: an
/// interior, a second continent, a shadow-line fence, four creature cells, four chest cells, two
/// indoor creature cells. Every one had a real case. The aggregate was 42 windows popping open on the
/// director's screen per `selfcheck`, several of them not reproducible, and a standing invitation to
/// spend sessions chasing harness ghosts — which is what happened. The director's call:
/// *"get rid of that shit, making more problems than it's helping… just keep the simple stuff we had
/// before, 3 frames 2 times of day."*
///
/// So growth is now the thing to resist, not to justify. **Nothing is deleted** — every evicted
/// viewpoint is still capturable by name from [`ON_DEMAND`], which is where a subject being worked on
/// belongs. A shot earns a place here only by the director putting it here.
///
/// The Northshire overlook toward the **Abbey** was 0632's shot 1 and is the one this set drops, at
/// the director's instruction, in favour of the Felwood hollow — which covers strictly more: a second
/// continent, its own tileset, fog palette, liquid type and WDL horizon, none of which the Elwynn
/// spots can photograph at all.
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

/// Director shot 1 — the Northshire overlook, north-east of the Abbey looking at it: terrain,
/// trees, the Abbey WMO, stained glass, props.
pub(super) const OVERLOOK_EYE: [f32; 3] = [-8955.0, -98.5, 91.1];
pub(super) const OVERLOOK_LOOK: [f32; 3] = [-8912.9, -125.4, 87.7];

/// Director shot 3 — Elwynn river, south-east of Northshire: open water dominant, shoreline blend,
/// murloc camp, fog.
pub(super) const WATER_EYE: [f32; 3] = [-9527.0, -310.6, 70.8];
pub(super) const WATER_LOOK: [f32; 3] = [-9499.4, -351.3, 61.4];

/// Director shot 4 — INSIDE the Lion's Pride Inn at Goldshire, standing in the
/// common room: the hearth (the MOCV-alpha self-illum bake), a daylight window, the lit chandelier,
/// the stair run, floor boards and ceiling beams, and a room full of props. The sweep's only
/// interior, and the only shot that exercises portal culling, the INT bake, the MOLT point pools
/// and WMO props at all — the surface the ledger's dungeon reports live on, and the one B101's
/// prop-cull regression (which hid the Blackrock lava falls) crossed unseen.
///
/// The agent's own interior framing (`inn-interior`, a tight crop of the kitchen hearth) was judged
/// badly framed and stays in [`ON_DEMAND`]; this is the director's, and it frames the ROOM.
///
/// NB the camera must stand over a FLOOR FACE: over a floorless pocket the portal cull's down-ray
/// reads "outside" and faithfully culls the containing group, which vanishes the room.
pub(super) const INN_EYE: [f32; 3] = [-9471.4, 39.4, 59.9];
pub(super) const INN_LOOK: [f32; 3] = [-9458.8, -7.5, 48.2];

/// Director shot 6 — an Elwynn rail fence lying ACROSS the sun's shadow boundary,
/// at 10:24: the right-hand span is in full sun, the left-hand span in shade, and the same edge
/// runs on across the road behind it. One frame holding **both** states of the MCSH sun term, on
/// terrain and on a doodad at once, with the ramp visible as the hard line between them.
///
/// The lighting matrix samples the lit and shadowed lanes as SEPARATE cells at separate
/// positions, so a regression that scaled both equally could pass both. Here the two states share
/// one frame, one model and one texture, so only their DIFFERENCE can carry the shot — the control
/// is inside the picture. It also lights the third lane neither matrix subject touches: a
/// world-placed **doodad**, which takes its shade per-vertex at bake rather than through
/// `entity_shade`'s per-object ramp.
///
/// Found by the director, who framed it because they had already spotted it was "both lit and unlit
/// from sun, half half".
pub(super) const FENCE_EYE: [f32; 3] = [-9511.9, -4.0, 61.9];
pub(super) const FENCE_LOOK: [f32; 3] = [-9552.0, 18.6, 42.4];

/// Director shot 5 — a Felwood hollow on **Kalimdor** (`MAP_KALIMDOR`): the
/// corrupted forest floor's root mat, a stand of emissive `felwoodmushroom` doodads, a pool of
/// green sludge (a liquid type no other golden shot contains), the vast trunks behind, and the
/// zone's sick-green fog and light palette.
///
/// The sweep's only shot outside Azeroth and outside the Elwynn/Stormwind palette — until this one
/// landed, a fog, tileset, WDL-horizon or per-map lighting regression anywhere else in the world
/// had nowhere to show up. It is also what forced [`Scenario::map`]: this spot's ADT tile
/// (`33_24`) exists in Azeroth too, empty, so on the old table it would have photographed a void.
pub(super) const FELWOOD_EYE: [f32; 3] = [4060.9, -944.3, 256.8];
pub(super) const FELWOOD_LOOK: [f32; 3] = [4014.0, -954.4, 242.9];

// ---------------------------------------------------------------------------------------------
// The LIGHTING MATRIX — one subject, three lanes, two sides.
//
// The golden spots are landscape: they photograph terrain, buildings and water, and (captures being
// server-less) contain no creature or GameObject at all. So the *object* light path — the one every
// creature, player, pet and chest in the game is lit by — had no regression coverage whatsoever,
// while the bug ledger's unit reports (blacked-out textures, mis-lit NPCs across city WMOs) sit
// squarely on it.
//
// The three positions are the three lanes an object's light can take, and they were CHOSEN FROM THE
// DATA, not by eye — `WOW_LIGHT_AT`/`WOW_LIGHT_GRID` (`wmo_portal::audit::light_probe`) reports the
// lane and the terrain MCSH bit at any world point:
//
//   SUN    (-9500, 56)   terrain z 56.48   MCSH false  lane exterior-on-terrain  — sun term at full
//   SHADE  (-9500, 44)   terrain z 55.95   MCSH true   lane exterior-on-terrain  — sun term dimmed
//   INDOOR (-9469.4, 31.9)  terrain z none  zone-text indoor 5, lane BAKE g05    — no sun at all
//
// SUN and SHADE are twelve yards apart on the same open ground, so between that pair the ONLY
// changed input is the baked shadow bit — exactly the discriminator `entity_shade` ramps on
// (2.5 lit → 0.5 shadowed). INDOOR stands in the Goldshire inn's common room, where the interior
// classifier owns the tag instead and the sun never reaches.
//
// Two sides per lane, straddling the light: the game's LIGHTING sun is near-fixed at azimuth 45°
// (`sun::follow`), so `front` puts the camera on that bearing (sun behind us — the subject's lit
// face) and `rear` puts it opposite (the shadowed face). Indoors there is no sun, so the pair simply
// samples the bake from two sides. Cameras sit at a fixed offset off the subject's feet, eye a
// little above and aimed at the body, so every cell frames the subject identically and a diff is
// about light, never framing.

/// Lighting-matrix subject positions (feet, raw WoW coords) — see the note above.
pub(super) const SUBJECT_SUN: [f32; 3] = [-9500.0, 56.0, 56.48];
pub(super) const SUBJECT_SHADE: [f32; 3] = [-9500.0, 44.0, 55.95];
pub(super) const SUBJECT_INDOOR: [f32; 3] = [-9469.4, 31.9, 57.9];

/// Every remaining named viewpoint — the UI look-pass fixtures, the sun/moon/sky regression
/// fixtures, the house-compass and street scenes. Capturable by name (`WOW_CAPTURE=<name>`) for
/// debugging and look passes, but NOT part of the blessed baseline sweep.
pub(super) const ON_DEMAND: &[Scenario] = &[
    // ---- The Deeprun Tram's undersea tube (map 369) ----
    // The one shipped map with NO `Light.dbc` row at all — not even the falloff-0 global that maps
    // 0/1 carry — so its whole atmosphere has to come from the building's own MFOG (record 2:
    // RGB(30,53,100), end 236.1 yd, start scalar 0.05; the camera's group `Subway_002` names it).
    // A global (WDT `MODF`) WMO too, the placement shape no other scenario exercises.
    //
    // ⚠ **This shot currently photographs NOTHING — do not baseline it.** The building registers
    // ("terrain: map DeeprunTram has no tiles — its world is one WMO") but no group ever reaches
    // the frame in server-less capture, so the shot is the bare sky dome from any eye, inside the
    // tube or between the tubes, at `WOW_CAPTURE_STABLE` 30 or 1800 alike (the 1800 run reported
    // "never settled", i.e. it was still waiting, not still loading). The live client draws this
    // map fine — the director's screenshots are of it — so the gap is the harness's, and it is
    // kept here as the reproducer rather than deleted: a WMO-only map is exactly the residency
    // shape 0688 wired and nothing has ever photographed it.
    Scenario {
        name: "tram-undersea",
        map: Some(MAP_DEEPRUN_TRAM),
        eye: TRAM_EYE,
        look: TRAM_LOOK,
        minute: 720,
        ui: None,
    },
    // ---- The Tainted Scar, Blasted Lands (report B90: "the skybox is almost pure white") ----
    // A level look across the crater from the reported spot, so the top half of the frame is
    // nothing but sky. The zone's `LightParams` 36 is the shape that matters: cloud density
    // **0.85** (Elwynn's is 0.50) puts near-total overcast on the dome, so whatever colour the
    // cloud layer resolves IS the sky here — which is why this zone shows a cloud-palette defect
    // that a half-clouded zone hides. Kept as the standing reproducer for the cloud palette.
    Scenario {
        name: "tainted-scar-noon",
        map: Some(MAP_AZEROTH),
        eye: SCAR_EYE,
        look: SCAR_LOOK,
        minute: 720,
        ui: None,
    },
    // ---- Evicted from the blessed sweep by decision 0817, NOT deleted ----
    // The sweep had grown from 0632's six to twenty-one, i.e. 42 windows on the director's screen per
    // `selfcheck`, and the director cut it back to three spots x two day times. Everything below this
    // note used to be in SCENARIOS and is still capturable by name — which is the right home for a
    // subject somebody is actively working on. `chest-shade-{front,rear}` additionally measured NOT
    // reproducible on the sweep that prompted the cut (MAE 2.721 / 2.649, ~7.8 % of pixels, two runs
    // of one build) — same coplanar-batch draw-order defect as `chest-indoor-*` below (0815 Open).
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
    // The sweep's only interior until 0817 moved it here; no `inn-night` (measured MAE 0.198 against
    // `inn-noon` — the room is lit by hearth and candles, so the clock barely reaches it).
    Scenario {
        name: "inn-noon",
        map: Some(MAP_AZEROTH),
        eye: INN_EYE,
        look: INN_LOOK,
        minute: 720,
        ui: None,
    },
    // The day cell keeps the director's own minute (10:24) rather than the sweep's noon: the subject
    // IS the shadow boundary, and where it falls is a function of the sun's angle, so at noon it
    // slides off the rails and the shot degrades into an ordinary fence.
    Scenario {
        name: "fence-shadowline-day",
        map: Some(MAP_AZEROTH),
        eye: FENCE_EYE,
        look: FENCE_LOOK,
        minute: 624,
        ui: None,
    },
    // The night twin, at the director's call — this record's own first draft argued against it on
    // the grounds that there is no shadow line without a sun, which was reasoning from the mechanism
    // and never measured. The bit it photographs is BAKED, present at every hour; what the night
    // frame pins is the other half of the mechanism — how much of the shade term survives once the
    // sun's contribution is gone and the ambient/moon palette carries the frame. That is a question
    // about the light curves, and a baseline is the right place to hold the answer (0749 addendum).
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
    // The chest INDOORS is out of the blessed sweep and in here instead: measured over two runs of
    // one build it is not reproducible (front MAE 1.551 / 9.2 % of pixels, rear 0.529 / 7.6 %), and
    // the diff is the whole body shifting brightness in perfect registration — LIGHT, not pose. The
    // GameObject's interior light lane does not converge by the shutter, while the CREATURE at the
    // same spot is bit-identical. That is the ledger's "mis-lit objects inside city WMOs" class, and
    // it is open. Capture them by name to work on it; they rejoin the sweep the day
    // they are deterministic.
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
    // The same farmhouse at MIDNIGHT — the one WMO fixture where the SIDN night fraction
    // (`grade.x`) is 1.0, so the MOMT 0x10 window glow is live pixels instead of a ×0 term.
    // Added for the static-gx slice-2 parity sweep (1429): every other WMO scenario sits at
    // noon or 19:30, both before the 20:30 ramp, and a wrong SIDN mirror would have passed
    // every diff unseen.
    Scenario {
        name: "house-north-midnight",
        map: Some(MAP_AZEROTH),
        eye: HOUSE_EYE,
        look: [-9389.1, 71.2, 58.0],
        minute: 0,
        ui: None,
    },
    // Inside the Lion's Pride Inn KITCHEN (its hearth carries the building's strongest MOCV-alpha
    // bake, α≈100 at the firebox — group-local (-32.9, 1.5, 2), world ≈ (-9461.7, -8.4, 58) per
    // the real MODF: uid 71414 on tile 31,49, origin (-9464.25, 24.39, 56.53), rot -97°). The
    // WMO-interior baseline: the INT bake, the MOCV-alpha self-illum glow on the hearth bricks
    // (frame right), the MOLT point pools on props, the fire doodads. Interiors previously had no
    // capture at all. NB the camera must stand over a FLOOR FACE: over a floorless pocket the
    // portal cull's down-ray reads "outside" and faithfully culls the containing group (the real
    // client does the same — the audit's "faithful-cull residue"), which vanishes the room.
    Scenario {
        name: "inn-interior",
        map: Some(MAP_AZEROTH),
        eye: [-9463.3, 4.4, 58.8],
        look: [-9462.1, -5.6, 58.5],
        minute: 720,
        ui: None,
    },
    // `northshire-noon`/`northshire-night` are gone: the director's `overlook` frames the same
    // valley from a spot they chose, at the same two day times. Dusk has no golden twin, so it
    // stays as the warm-light/fog fixture.
    Scenario {
        name: "northshire-dusk",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 1170, // 19:30 — warm dusk light + fog
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
    // Straight INTO the visible sun with the view lerp at max (f = 1) — the lens-flare regression
    // fixture. At 17:30 the sun sits at elev ≈30°, azimuth 45°, clear of the
    // Northshire ridge (the sky-dusk scene's 19:30 sun hides BEHIND the mountains, which is how
    // the halo-edge artifact escaped every baseline): the full 20-unit sunGlare star-ray quad must
    // fade off smoothly with no hard edge (the old far-placed quad depth-fought the sky dome and
    // was cut along a giant faceted circle).
    Scenario {
        name: "northshire-sun-flare",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8797.0, 23.0, 264.0], // eye + 300·(elev 30°, az 45°) — the sun's spot at 17:30
        minute: 1050,
        ui: None,
    },
    // The rising MOON low over the same bearing (az 45°, elev ≈15° at 22:44). Originally the flare
    // occlusion-gate fixture; since the byte-pinned moon dnCurve landed the
    // halo is dark here BY LAW — the curve is flat zero until 22:45 — so this now regression-checks
    // two things: the disc rises edge-first behind the ridge (per-pixel terrain occlusion), and NO
    // glare ring exists anywhere this early (a halo at 22:44 = the dn gate broke). The live halo's
    // appearance is `northshire-moon-halo` below; the gate's die-on-the-rock behavior keeps its
    // unit tests (`flare_ray_*`) and the sun fixtures.
    Scenario {
        name: "northshire-moonrise",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8775.0, 45.0, 190.0], // eye + 300·(elev 15°, az 45°) — the moon's spot at 22:44
        minute: 1364,
        ui: None,
    },
    // The moon's halo at its byte-law PEAK — midnight, moon overhead (az 45°, elev 55°), dnCurve
    // 1.0, dense stars (star curve 1.0): the warm disc + the gamma-added soft glare ring at full
    // envelope over the star field. The regression baseline for the halo's correct look —
    // the moonrise fixture above proves its absence early, this one its presence at depth of night.
    Scenario {
        name: "northshire-moon-halo",
        map: Some(MAP_AZEROTH),
        eye: SKY_EYE,
        look: [-8858.0, -38.0, 358.0], // eye + 300·(elev 55°, az 45°) — the moon's spot at 00:00
        minute: 0,
        ui: None,
    },
    // The `.tele Stormwind` spot (vmangos game_tele: -8833.38, 628.63, 94.01, o=1.065), eye at
    // head height looking along the tele facing into the Trade District — the city-scale perf
    // scene (the 2026-07-13 "20–30 fps in Stormwind" report). The whole city WMO + its doodad
    // load is resident here; Northshire scenes never exercise that scale.
    Scenario {
        name: "stormwind",
        map: Some(MAP_AZEROTH),
        eye: [-8833.38, 628.63, 96.0],
        look: [-8809.1, 672.3, 94.0],
        minute: 720,
        ui: None,
    },
    // The UI window fixtures (2026-07-03): each opens a shipped window with canned state over the
    // noon ground view. These are the look-pass instrument — window fidelity gets checked on these
    // captures (by the coordinator's own reading + benilla-visual regression diffs) BEFORE any
    // director look pass, so "looks like shit" gets caught in-loop, not on the director's screen.
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
    // The GameTooltip forced open over a seeded bag slot (a green-quality item). Run with
    // `WOW_CAPTURE=ui-tooltip`.
    // The cooldown sweep as a filmstrip: sixteen bag slots, sixteen phases, one deterministic
    // still. Run with `WOW_CAPTURE=ui-cooldown`.
    Scenario {
        name: "ui-cooldown",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Cooldown),
    },
    // The same filmstrip with the autocast shine sharing the atlas — the cell-bleed instrument.
    // Run with `WOW_CAPTURE=ui-cooldown-shine`.
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
    // The world-mouseover tooltip at the DEFAULT corner (screen bottom-right, −13/+70) over a
    // seeded hostile wolf. Run with `WOW_CAPTURE=ui-tooltip-world`.
    Scenario {
        name: "ui-tooltip-world",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::TooltipWorld),
    },
    // The character window's paper doll over a fully-seeded synthetic self player. Run with
    // `WOW_CAPTURE=ui-char`.
    Scenario {
        name: "ui-char",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Character),
    },
    // The player + target unit frames (no window fixture — the frames come from `demo_unit_feed`,
    // which seeds synthetic "player"/"target" snapshots whenever WOW_CAPTURE_UI=1). Run with
    // `WOW_CAPTURE=ui-unitframes`.
    Scenario {
        name: "ui-unitframes",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    // The combo-point dots at the target frame's top-right. `demo_unit_feed`
    // seeds a ROGUE with four points banked on the selected wolf for this scenario only — the demo
    // player is a warrior everywhere else, and a warrior authentically lights no dot. Run with
    // `WOW_CAPTURE=ui-combopoints`.
    Scenario {
        name: "ui-combopoints",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    // The group-invite dialog over open world — the shared StaticPopup plate's only look-pass.
    // Run with `WOW_CAPTURE=ui-partyinvite`.
    Scenario {
        name: "ui-partyinvite",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::PartyInvite),
    },
    // The re-skinned main action bar (no window fixture — the bar loads by default under
    // WOW_CAPTURE_UI=1, and `demo_unit_feed` seeds the action slots + player XP). The bar is 1024
    // wide + 128px end caps, so this fixture takes a WIDER, shorter window (see main.rs's per-capture
    // sizing) — the default 640px UI window would crop it. Run with
    // `WOW_CAPTURE=ui-actionbar`.
    Scenario {
        name: "ui-actionbar",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Bare),
    },
    // The V-key nameplate over a synthetic Timber Wolf, framed like the reference screenshot
    // (an eye-height look at a wolf ~8 yd off, Northshire ground). **Plates are real widgets since
    // 2148**, so they need the UI VM and a `WorldFrame` to hang off — which this scenario's own
    // `ui:` fixture supplies (it is what makes `run_mode::capture_ui_opted_in` true). The older
    // note here said the opposite ("plates draw through the UiQuads overlay… no WOW_CAPTURE_UI
    // needed"), which was the painter's truth and stopped being true the day they became frames.
    // Run with `WOW_CAPTURE=vplates` (main.rs sizes this window 1024×768 — the 1:1 gx window).
    Scenario {
        name: "vplates",
        map: Some(MAP_AZEROTH),
        eye: [-8956.5, -137.5, 85.6],
        look: [-8949.95, -132.49, 84.8],
        minute: 720,
        ui: Some(UiFixture::VPlates),
    },
    // The fullscreen world map (decision 0203 phase 2), forced open at the world sheet. The frame's
    // 1024×768 chrome needs the taller window main.rs gives the 1:1 gx captures. Run with
    // `WOW_CAPTURE=ui-worldmap`.
    Scenario {
        name: "ui-worldmap",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::WorldMap),
    },
    // The spellbook over a seeded mage book. Run with `WOW_CAPTURE=ui-spellbook`.
    Scenario {
        name: "ui-spellbook",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::SpellBook),
    },
    // The macro window over a seeded macro set. Run with
    // `WOW_CAPTURE=ui-macro`.
    Scenario {
        name: "ui-macro",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Macro),
    },
    // The same window with the name/icon popup open. Run with
    // `WOW_CAPTURE=ui-macro-popup`.
    Scenario {
        name: "ui-macro-popup",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::MacroPopup),
    },
    // The chat edit box open with a typed draft over seeded lines. Run with
    // `WOW_CAPTURE=ui-chatedit`.
    Scenario {
        name: "ui-chatedit",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ChatEdit),
    },
    // The hovered chat dock — the tab plate + its ADD highlight over the world, which is the
    // pair the UI-over-world composite space decides. Run with
    // `WOW_CAPTURE=ui-chat-tabhover`.
    Scenario {
        name: "ui-chat-tabhover",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ChatTabHover),
    },
    // The social pane, for the stray-dropdown regression. Run with
    // `WOW_CAPTURE=ui-social`.
    Scenario {
        name: "ui-social",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Social),
    },
    // The world-entry loading screen, held up with its tip of the day. Run with
    // `WOW_CAPTURE=loading-tip`. The backdrop is whatever `Map.dbc` → `LoadingScreens.dbc` gives
    // this scenario's map, so it photographs the art chain and the bar as well as the tip.
    Scenario {
        name: "loading-tip",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::LoadingTip),
    },
    // The era Options window over the ground scene. Run with
    // `WOW_CAPTURE=ui-options`.
    Scenario {
        name: "ui-options",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::Options),
    },
    // The same window on the AUDIO page. Run with
    // `WOW_CAPTURE=ui-options-audio`.
    Scenario {
        name: "ui-options-audio",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsAudio),
    },
    // The same window on the GRAPHICS page. Run with
    // `WOW_CAPTURE=ui-options-graphics`.
    Scenario {
        name: "ui-options-graphics",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsGraphics),
    },
    // The same window on the CHAT page (1589). Run with
    // `WOW_CAPTURE=ui-options-chat`.
    Scenario {
        name: "ui-options-chat",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsChat),
    },
    // The colour picker, wheel and all (1592). Run with
    // `WOW_CAPTURE=ui-color-picker`.
    Scenario {
        name: "ui-color-picker",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::ColorPicker),
    },
    // The Controls page with the Camera Following Style dropdown OPEN (re-seated 1649). Run
    // with `WOW_CAPTURE=ui-options-dropdown`.
    Scenario {
        name: "ui-options-dropdown",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsDropdownList),
    },
    // The options window's Keybindings page, Movement expanded (1008). Run with
    // `WOW_CAPTURE=ui-keybindings`.
    Scenario {
        name: "ui-keybindings",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::KeyBindings),
    },
    // The same window MID-SEARCH: the "volume" results view. Run with
    // `WOW_CAPTURE=ui-options-search`.
    Scenario {
        name: "ui-options-search",
        map: Some(MAP_AZEROTH),
        eye: GROUND_EYE,
        look: GROUND_LOOK,
        minute: 720,
        ui: Some(UiFixture::OptionsSearch),
    },
    // An overhead NAME with the river surface behind it — the world-text-vs-liquid draw-order
    // instrument (see [`UiFixture::NameWater`]). Same camera as `water-noon`, plus a named unit
    // out in the water. Run with `WOW_CAPTURE=name-water`.
    Scenario {
        name: "name-water",
        map: Some(MAP_AZEROTH),
        eye: WATER_EYE,
        look: WATER_LOOK,
        minute: 720,
        ui: Some(UiFixture::NameWater),
    },
];

/// The `name-close` instrument's subject: the `name-water` wolf's overhead NAME, orbited at an
/// arbitrary distance. **World text is the only consumer that draws the glyph sheet at anything
/// but 1:1** — the 2-D path rasterizes at the size it draws, texel for pixel, by construction — so
/// a defect in how a glyph CELL is sampled can only appear in a magnified name, and nothing
/// photographed one: `name-water`'s 25 yd MINIFIES the same glyphs, which hides exactly this.
///
/// Not a golden scenario (its output is the knobs', not the name's), same as `fxview`/`vista`:
/// `WOW_NAME_DIST` (yd from the name, default 4), `WOW_NAME_AZ` (compass bearing the eye stands
/// on, degrees, 0 = +X), `WOW_NAME_EL` (eye elevation above the name, degrees), `WOW_NAME_H` (the
/// name's height above the unit's feet, yd — the anchor this aims at). The camera looks straight
/// at the name, so the shot is centred and the magnification is `WOW_NAME_DIST` alone.
pub(super) const NAME_CLOSE_AT: [f32; 3] = super::fixtures::NAME_WATER_POS;

/// The Deeprun Tram undersea tube — see the `tram-undersea` scenario. Raw WoW coords; the Subway
/// WMO is the map's global `MODF` at the origin with identity rotation, so these are also its
/// model-space coords.
pub(super) const TRAM_EYE: [f32; 3] = [-2.44, -1250.0, -120.0];
pub(super) const TRAM_LOOK: [f32; 3] = [-2.44, -1400.0, -118.0];

/// The Tainted Scar — see the `tainted-scar-noon` scenario. The eye is the ledger's own pin for
/// report B90 (`.go xyz -11892.70 -2647.08 -4.68 0`, `game_tele TheTaintedScar`) lifted clear of
/// the crater floor; the look is 25 deg above level, north. The pitch is the whole point: the crater
/// rim is tall enough that a dead-level look photographs nothing but rock, and the subject here is
/// the dome. At 25 deg the ridge line sits low in the frame as the control and the top two thirds
/// are sky.
pub(super) const SCAR_EYE: [f32; 3] = [-11892.7, -2647.1, 20.0];
pub(super) const SCAR_LOOK: [f32; 3] = [-11792.7, -2647.1, 66.6];

/// **Does the scenario named by `$WOW_CAPTURE` declare a UI fixture?** — i.e. is this a capture
/// whose *subject* is a window.
///
/// `WOW_CAPTURE_UI=1` exists so that world baselines stay UI-free: a capture regression-tests the
/// world render, and the player UI painted over it would make every diff a UI diff. That default
/// is right. What was wrong is that it also gated the scenarios that ARE the UI — a `ui-*`
/// scenario declares `ui: Some(..)` in the table above, which is the harness's own statement that
/// the window is the point, and then needed the env var repeated on the command line to mean it.
///
/// **Forgetting it did not fail.** The run brought up the world with no in-game UI, the fixture's
/// seed hit a nil global (`ToggleQuestLog`), logged ONE `warn!` into ~100 lines of pipeline
/// chatter, wrote a perfectly valid PNG of a UI-less world, and exited **0**. That is an
/// instrument that manufactures a confident false negative — the shape `docs/METHOD.md` §6 spends a
/// paragraph on — and it cost a session a wasted build and nearly a wrong conclusion about a fix
/// that was in fact working.
///
/// So a `ui:` fixture now opts the UI in on its own account. World scenarios (`ui: None`) are
/// untouched and stay pristine; `WOW_CAPTURE_UI=1` still works and still means what it meant, for
/// the case it was actually for — opting the UI into a WORLD scenario's shot.
/// Find a scenario by name across **both** tables — the blessed sweep [`SCENARIOS`] and
/// [`ON_DEMAND`], exactly as the harness's own `WOW_CAPTURE=` resolution does (only
/// the sweep is narrowed, every viewpoint stays capturable by name). Every `ui-*` fixture lives in
/// `ON_DEMAND`, so a lookup that searched only `SCENARIOS` would answer "not a UI scenario" for
/// precisely the scenarios this predicate exists to serve — which is what the test below caught
/// on the first run of it.
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

/// **The one predicate**: is the player UI opted into this capture? Either the scenario declares a
/// `ui:` fixture, or `WOW_CAPTURE_UI=1` asks for it over a world scene.
///
/// It is one function because it has three consumers that MUST agree, and the cost of them
/// disagreeing is a capture that looks plausible and isn't: the UI load itself
/// (`ui_script::lifecycle::ui_wanted`), the synthetic `"player"`/`"target"` snapshot that
/// populates the frames in a server-less run (`ui_script::capture_ui_active` →
/// `demo_unit_feed`), and the window SIZE the scenario is denominated in (`lib.rs`). Load the UI
/// without the demo feed and `ui-unitframes` photographs empty frames; load it without the size
/// and every UI baseline is denominated in the world window. Neither failure looks like a failure.
pub(crate) fn ui_opted_in() -> bool {
    std::env::var("WOW_CAPTURE_UI").as_deref() == Ok("1") || scenario_declares_ui()
}

#[cfg(test)]
mod ui_opt_in_tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// The footgun this predicate exists to remove: a scenario whose subject is a window opts the
    /// UI in **without** `WOW_CAPTURE_UI=1`, while a world scenario still does not. Before this,
    /// `WOW_CAPTURE=ui-questlog` alone brought up a UI-less world, failed its seed on a nil
    /// global, wrote a valid PNG and exited 0.
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

        // The one that made the concept honest: it opens no window, so it used to say
        // `ui: None` and was a UI capture the table called a world capture.
        let _c = EnvGuard::set("WOW_CAPTURE", "ui-unitframes");
        assert!(ui_opted_in(), "…including the fixtureless `Bare` ones");

        // A world scenario's baseline regression-tests the WORLD render and must stay UI-free.
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

        // …unless the env var asks for it over a world scene, which is what it was always for.
        let _on = EnvGuard::set("WOW_CAPTURE_UI", "1");
        assert!(ui_opted_in(), "the env var still opts a world scene in");
    }

    /// Every `ui-*`-named scenario really does declare a fixture — otherwise it would silently be
    /// a world capture wearing a UI name, and the predicate above would not save it.
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
