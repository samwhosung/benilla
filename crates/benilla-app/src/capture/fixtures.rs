//! The capture UI fixtures (split from `mod.rs`, file-size budget): each arm seeds one
//! window's synthetic-but-realistic state — see [`seed_ui_fixture`]'s doc.

use super::*;

/// The `vplates` fixture's wolf — the reference client's own screenshot subject (vmangos
/// `creature_template` 69 "Timber Wolf": level 2, faction template 32, display 604), standing at
/// the scenario's look point.
const WOLF_ENTRY: u32 = 69;
const WOLF_GUID: u64 = (0xF130u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 0x69;
const WOLF_POS: [f32; 3] = [-8949.95, -132.49, 83.9];
const WOLF_DISPLAY: u32 = 604;
const WOLF_FACTION: u32 = 32;

/// The `name-water` fixture's unit: the same wolf, re-seated 25 yd along the water scenario's own
/// look bearing (`WATER_EYE` → `WATER_LOOK`) at the river surface, so its overhead name projects
/// onto the water *beyond* it.
pub(super) const NAME_WATER_POS: [f32; 3] = [-9512.97, -331.29, 61.4];

/// The lighting matrix's chest: `GameObjectDisplayInfo` 259,
/// `World\SkillActivated\Containers\TreasureChest01.mdx`. GameObject guids carry the `0xF110` high
/// word, and the descriptor is left at its defaults — an unstated `GAMEOBJECT_STATE` holds the
/// closed rest pose, which is the frame we want.
const CHEST_DISPLAY: u32 = 259;
const CHEST_GUID: u64 = (0xF110u64 << 48) | 0x744;

/// Which way a lighting-matrix subject faces (Bevy yaw). The matrix's `front` camera sits on the
/// lighting sun's bearing (azimuth 45°), so the subject is turned to meet it: `front` reads the
/// face, `rear` the tail — the same body, its lit and unlit sides.
const SUBJECT_YAW: f32 = 2.36;

/// Seed the fixture window's state resources once, right after the scene goes resident — the real
/// feeds then push it into the VM during the settle window exactly as live wire data would. Item
/// icons resolve through the offline `ItemDisplayCatalog` (display ids chosen from entries that
/// catalog is known to carry); names land directly in the caches (no server to ask).
pub(super) fn seed_ui_fixture(
    mut ctx: ResMut<CaptureCtx>,
    mut commands: Commands,
    progress: Res<WorldLoadProgress>,
    mut merchant: ResMut<crate::ui_merchant::MerchantOpen>,
    mut gossip: ResMut<crate::ui_gossip::GossipState>,
    mut quest: ResMut<crate::ui_quest::QuestGiver>,
    mut quest_log: ResMut<crate::ui_quest_log::QuestLog>,
    mut loot: ResMut<crate::ui_loot::LootState>,
    // The item store and the object index as one param (the 16-SystemParam ceiling): the item
    // objects are entities in the index (2334), and the fixture spawns them like the wire does.
    store: (ResMut<crate::items::Items>, ResMut<crate::net::GuidIndex>),
    mut names: ResMut<crate::names::NameCache>,
    icons: Option<Res<crate::entities::ItemDisplays>>,
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut vplates: ResMut<crate::vplates::VPlateMode>,
    mut selection: ResMut<crate::target::Selection>,
    mut player: ResMut<crate::player::Player>,
    // Bundled: Bevy systems cap at 16 top-level params — a nested tuple is one param.
    (mut actions, mut bank, mut exit, mut loading): (
        ResMut<crate::ui_action::PlayerActions>,
        ResMut<crate::ui_bank::BankOpen>,
        MessageWriter<AppExit>,
        ResMut<crate::loading_screen::LoadingScreen>,
    ),
) {
    let (mut items, mut index) = store;
    // A glue-screen capture has no world scenario, and no glue screen opens a UI fixture.
    let Some(scenario) = ctx.scenario else {
        return;
    };
    let Some(fixture) = scenario.ui else {
        return;
    };
    if ctx.ui_seeded || !(progress.total > 0 && progress.ready == progress.total) {
        return;
    }
    ctx.ui_seeded = true;

    // **A UI capture with no script VM is not a capture — refuse it.** Every seed below opens its
    // window by calling into the in-game UI, so with no VM they all fail the same way: a nil
    // global, one `warn!` in a log full of pipeline chatter, a valid-looking PNG of a UI-less
    // world, and exit 0. That is the false-negative shape `docs/METHOD.md` §6 exists to prevent, and it
    // burned a session. `scenario_wants_ui` removed the cause (a `ui:` scenario no longer needs
    // `WOW_CAPTURE_UI=1`); this is the tripwire for whatever else could leave the VM absent, and
    // it exits non-zero the way the window-size refusal does (`video::warn_if_window_mismatch`).
    if script.is_none() {
        error!(
            "capture: REFUSING this capture — scenario {:?} declares a UI fixture but no script              VM exists, so its window cannot be opened and the shot would be a UI-less world              wearing the scenario's name.",
            scenario.name
        );
        exit.write(AppExit::error());
        return;
    }

    // A creature guid whose entry bits (24–47) carry 90001 — the NameCache resolves vendor/NPC
    // names by that entry, so inserting the name by entry makes the title path run for real.
    const NPC_ENTRY: u32 = 90_001;
    const NPC_GUID: u64 = (0xF130u64 << 48) | ((NPC_ENTRY as u64) << 24) | 0x42;
    // Display ids the offline icon catalog is known to resolve (crates/benilla-formats items.rs
    // anchors): sword, food, shield, hearthstone.
    const DISP_SWORD: u32 = 1542;
    const DISP_FOOD: u32 = 2473;
    const DISP_SHIELD: u32 = 18730;
    const DISP_STONE: u32 = 6418;

    let template = |name: &str, quality: u32| benilla_protocol::messages::ItemInfo {
        class: 4,
        subclass: 0,
        name: name.into(),
        display_info_id: 0, // per-row display comes from the vendor/loot row itself
        quality,
        flags: 0,
        buy_price: 0,
        sell_price: 0,
        inventory_type: 0,
        allowable_class: -1,
        allowable_race: -1,
        item_level: 0,
        required_level: 0,
        required_skill: 0,
        required_skill_rank: 0,
        required_spell: 0,
        required_honor_rank: 0,
        required_city_rank: 0,
        required_rep_faction: 0,
        required_rep_rank: 0,
        max_count: 0,
        stackable: 1,
        container_slots: 0,
        stats: Vec::new(),
        damages: Vec::new(),
        dmg_min: 0.0,
        dmg_max: 0.0,
        dmg_type: 0,
        armor: 0,
        resistances: [0; 6],
        delay_ms: 0,
        ammo_type: 0,
        ranged_mod_range: 0.0,
        spells: Vec::new(),
        spell_charges_0: 0,
        use_spell: None,
        bonding: 0,
        description: String::new(),
        page_text: 0,
        language_id: 0,
        page_material: 0,
        start_quest: 0,
        lock_id: 0,
        material: 0,
        sheath: 0,
        random_property: 0,
        block: 0,
        item_set: 0,
        max_durability: 0,
        area: 0,
        map: 0,
        bag_family: 0,
    };

    match fixture {
        // Nothing to open — the UI being loaded (and `demo_unit_feed`'s synthetic player/target)
        // IS the fixture. The `script.is_none()` refusal above still guards it: these scenarios
        // photograph the player UI, so a run without a VM is as wrong for them as for any other.
        UiFixture::Bare => {}
        // The one fixture that opens no window: it raises the world-entry loading screen over the
        // settled scene and pins it there, tip and all. `hold_for_capture` sets the `Pick` edge;
        // `crate::game_tip::drive_game_tip` paints it on the next frame, and the stability watch
        // settles on the held image.
        UiFixture::LoadingTip => loading.hold_for_capture(scenario.map),
        UiFixture::Merchant => {
            names.insert_creature(
                NPC_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Godric Rothgar".into(),
                    subname: None,
                    creature_type: 0,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            // Eight rows across both columns: long names to exercise the wrap, mixed prices to
            // exercise the coin denominations (75c, 1s 23c, 998g-class purse comes from set_money).
            // The last two columns per row are requiredLevel and remaining stock: boots + gloves
            // are level-gated past the fixture player (level 12, matching the standing player
            // frame in the scene) so their rows render the ref's UNUSABLE red (plate/socket
            // 1,0,0 · icon 0.9,0,0), and the Small Shield is sold out so its row renders the
            // 0.5 gray.
            let stock: [(&str, u32, u32, u32, u32); 8] = [
                ("Tarnished Chain Vest", DISP_SWORD, 75, 0, u32::MAX),
                ("Tarnished Chain Leggings", DISP_FOOD, 75, 0, u32::MAX),
                ("Tarnished Chain Belt", DISP_STONE, 37, 0, u32::MAX),
                ("Tarnished Chain Boots", DISP_SHIELD, 57, 20, u32::MAX),
                ("Tarnished Chain Bracers", DISP_SWORD, 37, 0, u32::MAX),
                ("Tarnished Chain Gloves", DISP_FOOD, 37, 20, u32::MAX),
                ("Small Shield", DISP_SHIELD, 34, 0, 0),
                ("Large Round Shield", DISP_SHIELD, 12_345, 0, u32::MAX), // 1g 23s 45c
            ];
            let rows = stock
                .iter()
                .enumerate()
                .map(|(i, (name, disp, price, req_level, count))| {
                    let entry = 91_000 + i as u32;
                    let mut t = template(name, 1);
                    t.required_level = *req_level;
                    items.insert_template(entry, Some(t));
                    benilla_protocol::messages::VendorItem {
                        slot: i as u32 + 1,
                        entry,
                        display_id: *disp,
                        current_count: *count,
                        price: *price,
                        max_durability: 40,
                        buy_count: 1,
                    }
                })
                .collect();
            merchant.open(NPC_GUID, rows);
            // The usable gate's player half, pushed by hand: there is no SelfPlayer in capture
            // (same reasoning as the bag fixture's manual snapshot below), so feed_player_req
            // never runs and this seed is not clobbered. Level 12 human warrior — the standing
            // player frame's level, under the boots/gloves gate above.
            if let Some(s) = script.as_mut() {
                s.set_player_req_state(benilla_ui::script::PlayerReqState {
                    level: 12,
                    class_id: 1,
                    race_id: 1,
                    ..Default::default()
                });
            }
        }
        UiFixture::Gossip => {
            names.insert_creature(
                NPC_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Marshal McBride".into(),
                    subname: None,
                    creature_type: 0,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            gossip.npc = Some(NPC_GUID);
            gossip.text_id = 1;
            // The greeting of the live menu the overflow was reported on: long enough that the
            // wrapped options below it run past the parchment, so this capture stands on the
            // scrolling path rather than beside it.
            gossip.greeting = Some(
                "A man has been caught stealing corn from the fields of a noble, a lord known \
                 for his harsh taxes throughout the land.$B$BMake your choice!"
                    .into(),
            );
            // Quest rows ride above the options — an available quest (its own
            // AvailableQuestIcon dot) so the capture covers the quest-row icon/text seating, not
            // just the option rows.
            gossip.quests = vec![(783, 0, 5, "Eagan Peltskinner".into())];
            // One short option AND four that WRAP — the live shape both gossip bugs came in as
            // (the director's screenshots), and a menu deliberately TALLER than the parchment so
            // the capture covers the whole chain: the stock `GossipResize`'s per-row auto-height,
            // the scroll frame that contains the overflow, and the
            // scrollbar that appears with it. A fixture of one-line labels showed none of this —
            // every row fit the template's 16 px and nothing ever overflowed.
            let judgement = [
                "I slay the man on the spot as my liege would expect me to, as he has broken the \
                 law of the land and it is my sworn duty to enforce it.",
                "I turn over the man to my liege for punishment, as he has stolen, and I am not \
                 the arbiter of his fate.",
                "I confiscate the corn he has stolen, warn him that stealing is a path towards \
                 doom and destruction, but I let him go to return to his family.",
                "I allow the man to take enough corn to feed his family for a couple of days, \
                 encouraging him to leave the land.",
            ];
            gossip.options = vec![benilla_protocol::messages::GossipOption {
                index: 0,
                icon: 1,
                coded: false,
                message: "Let me browse your goods.".into(),
            }];
            gossip
                .options
                .extend(judgement.iter().enumerate().map(|(i, message)| {
                    benilla_protocol::messages::GossipOption {
                        index: i as u32 + 1,
                        icon: 0,
                        coded: false,
                        message: (*message).into(),
                    }
                }));
        }
        UiFixture::Bank => {
            // The banker (a pure banker — the Ironforge vault's own name) + the vault fed the REAL
            // way: everything below lands in the descriptor/caches, and `feed_bank`/`ui_items` push
            // it into the VM over the settle window exactly as live wire data would.
            use benilla_protocol::messages::ObjectFields;
            names.insert_creature(
                NPC_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Soleil Stonemantle".into(),
                    subname: Some("Banker".into()),
                    creature_type: 7,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: true,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            // Vault items (bank wire slots 0, 1, 2, 7 → fields 564+2i) + the held bank bag in bag
            // slot 0 (field 612). Icons resolve through the offline catalog's known display ids;
            // the bag reuses the hearthstone display (no bag icon among the capture anchors —
            // structural stand-in, not a look claim).
            const G_SHIELD: u64 = 0x2001;
            const G_JERKY: u64 = 0x2002;
            const G_SWORD: u64 = 0x2003;
            const G_STONE: u64 = 0x2004;
            const G_BANKBAG: u64 = 0x2005;
            let obj = |entry: u32, stack: u32| {
                ObjectFields::from_pairs(&[(3, entry), (14, stack)]) // OBJECT_ENTRY, STACK_COUNT
            };
            for (guid, entry, stack, name, disp, quality) in [
                (G_SHIELD, 94_001, 1, "Small Shield", DISP_SHIELD, 2),
                (G_JERKY, 94_002, 5, "Tough Jerky", DISP_FOOD, 1),
                (G_SWORD, 94_003, 1, "Worn Shortsword", DISP_SWORD, 1),
                (G_STONE, 94_004, 1, "Hearthstone", DISP_STONE, 1),
                (G_BANKBAG, 94_005, 1, "Small Brown Pouch", DISP_STONE, 1),
            ] {
                crate::items::spawn_item(&mut commands, &mut index, guid, obj(entry, stack), false);
                let mut t = template(name, quality);
                t.display_info_id = disp;
                if guid == G_BANKBAG {
                    t.class = 1; // container
                    t.container_slots = 6;
                }
                items.insert_template(entry, Some(t));
            }
            // The held bank bag is a real CONTAINER object (its contents stream on the bag item —
            // decision 0604): 6 slots, one occupied, so the POPOUT window (container 5) is in the
            // shot too — its snug-fit stitch, lit bag button, and own-icon portrait.
            crate::items::spawn_item(
                &mut commands,
                &mut index,
                G_BANKBAG,
                ObjectFields::from_pairs(&[
                    (3, 94_005),
                    (14, 1),
                    (48, 6),              // CONTAINER_NUM_SLOTS
                    (50, G_JERKY as u32), // CONTAINER_SLOT_1
                ]),
                true,
            );
            // The self player: 4 occupied vault slots, the bank bag, TWO bought bag slots
            // (`PLAYER_BYTES_2` byte 2 — bag button 1 owned, 2 bought-but-empty, 3–6 the red
            // unpurchased tint), and a 12g 34s 56c purse — the 10g third-slot cost from the real
            // `BankBagSlotPrices.dbc` renders affordable-white under it.
            const PLAYER_GUID: u64 = 0x51;
            let fields = ObjectFields::from_pairs(&[
                (194, 2 << 16),          // PLAYER_BYTES_2 — bankBagSlots (byte 2) = 2
                (564, G_SHIELD as u32),  // BANK_SLOT_1 (vault slot 0)
                (566, G_JERKY as u32),   // vault slot 1
                (568, G_SWORD as u32),   // vault slot 2
                (578, G_STONE as u32),   // vault slot 7 — a gap, like a real vault
                (612, G_BANKBAG as u32), // BANK_BAG_SLOT_1 — bag button 1's icon
                (1176, 123_456),         // PLAYER_FIELD_COINAGE
            ]);
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));
            // Open the session — `feed_bank` fires BANKFRAME_OPENED (title + portrait) on the
            // next frames exactly as a live SMSG_SHOW_BANK would.
            bank.open(NPC_GUID);
            // One-shot popout: open bank bag 1 as soon as its container feed lands (the click
            // path needs `GetContainerNumSlots(5) > 0`, which arrives over the settle frames —
            // the window's OnUpdate polls, flag-guarded, exactly once). `BankFrameBag1`'s own
            // `GetID()` is 5, the container id, which is what its handler passes to `ToggleBag`.
            if let Some(s) = script.as_mut() {
                if let Err(e) = s.run(
                    "BankFrame:SetScript(\"OnUpdate\", function()\n\
                         if not benillaBankPopoutDone and GetContainerNumSlots(5) > 0 then\n\
                             benillaBankPopoutDone = 1\n\
                             this = getglobal(\"BankFrameBag1\")\n\
                             BankFrameItemButtonBag_OnClick(\"LeftButton\")\n\
                         end\n\
                     end)",
                ) {
                    warn!("capture: ui-bank popout hook failed: {e}");
                }
            }
        }
        UiFixture::Quest => {
            names.insert_creature(
                NPC_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Marshal McBride".into(),
                    subname: None,
                    creature_type: 0,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            // A QUEST_DETAILS-shaped view (the accept panel): a description + objectives, one choice
            // reward, one fixed reward, and a money reward — the same offline caches the live feed
            // reads resolve the row names/icons.
            items.insert_template(92_001, Some(template("Brackwater Cudgel", 2)));
            items.insert_template(92_002, Some(template("Militia Warhammer", 1)));
            items.insert_template(92_003, Some(template("Bandit Cloak", 1)));
            let choice = |entry: u32, disp: u32| benilla_protocol::messages::QuestRewardItem {
                item_id: entry,
                count: 1,
                display_id: disp,
            };
            quest.open(
                NPC_GUID,
                crate::ui_quest::QuestView::Detail(benilla_protocol::messages::QuestDetails {
                    npc: NPC_GUID,
                    quest_id: 783,
                    title: "A Threat Within".into(),
                    details:
                        "Kobolds have infested the Echo Ridge Mine to the northeast. Slay them \
                              and return to me."
                            .into(),
                    objectives: "Kill 8 Kobold Vermin.".into(),
                    auto_finish: 1,
                    choices: vec![choice(92_001, DISP_SWORD), choice(92_002, DISP_STONE)],
                    rewards: vec![choice(92_003, DISP_SHIELD)],
                    money: 1234, // 12s 34c
                    reward_spell: 0,
                }),
            );
        }
        UiFixture::QuestGreeting => {
            names.insert_creature(
                NPC_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Marshal McBride".into(),
                    subname: None,
                    creature_type: 0,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            // The multi-quest greeting: two AVAILABLE quests (not in the log → bullet rows under the
            // "Available Quests" header), matching the director's live screenshot subject.
            quest.open(
                NPC_GUID,
                crate::ui_quest::QuestView::Greeting(benilla_protocol::messages::QuestGiverList {
                    npc: NPC_GUID,
                    greeting: "Greetings, $N. The town of Goldshire needs able hands.".into(),
                    emote_delay: 0,
                    emote: 0,
                    quests: vec![
                        benilla_protocol::messages::QuestListEntry {
                            quest_id: 84,
                            icon: 0,
                            level: 4,
                            title: "Brotherhood of Thieves".into(),
                        },
                        benilla_protocol::messages::QuestListEntry {
                            quest_id: 40,
                            icon: 0,
                            level: 3,
                            title: "Eagan Peltskinner".into(),
                        },
                    ],
                }),
            );
        }
        UiFixture::QuestLog => {
            // The log is fed from the self player's PLAYER_QUEST_LOG descriptor slots (decision
            // 0109: descriptor-as-truth) — so the fixture spawns a synthetic self player carrying
            // two occupied slots and lets `feed_quest_log` run the real chain. Slot layout per the
            // 0109 wire pin: id / packed 6-bit counters + state byte / timer, base field 198.
            use benilla_protocol::messages::{ObjectFields, QuestObjective, QuestTemplate};
            const KOBOLD_ENTRY: u32 = 90_002;
            names.insert_creature(
                KOBOLD_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Kobold Vermin".into(),
                    subname: None,
                    creature_type: 0,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            // The item objective's target + the reward rows (log icons resolve through the item
            // template's display_info_id, not a wire display id).
            let mut with_icon = template("Chipped Boar Tusk", 0);
            with_icon.display_info_id = DISP_STONE;
            items.insert_template(93_001, Some(with_icon));
            let mut cudgel = template("Brackwater Cudgel", 2);
            cudgel.display_info_id = DISP_SWORD;
            items.insert_template(93_002, Some(cudgel));
            let mut cloak = template("Bandit Cloak", 1);
            cloak.display_info_id = DISP_SHIELD;
            items.insert_template(93_003, Some(cloak));

            let blank = QuestObjective {
                creature_or_go: 0,
                required_count: 0,
                item_id: 0,
                item_count: 0,
                text: String::new(),
            };
            // Entry 1 (selected by first-valid auto-selection): in progress, one creature
            // objective at 3/10 (the slot counter below) + one item objective (bags are empty in
            // capture → 0/5), choice + fixed rewards, money. `quest_type: 1` is QuestInfo.dbc's
            // "Elite" — the real 783 is a plain quest, but this fixture is the only thing that
            // puts a row TAG in front of the capture, and the tag and the "(Complete)" state word
            // are different branches of the same row string (entry 2 covers the other).
            quest_log.insert_template(QuestTemplate {
                quest_id: 783,
                method: 2,
                level: 2,
                zone_or_sort: 12,
                quest_type: 1,
                rep_objective_faction: 0,
                rep_objective_value: 0,
                next_quest_in_chain: 0,
                money: 150, // 1s 50c
                money_max_level: 0,
                reward_spell: 0,
                src_item_id: 0,
                flags: 0,
                rewards: [(93_003, 1), (0, 0), (0, 0), (0, 0)],
                choices: [(93_002, 1), (93_001, 3), (0, 0), (0, 0), (0, 0), (0, 0)],
                point_map_id: 0,
                point_x: 0.0,
                point_y: 0.0,
                point_opt: 0,
                title: "A Threat Within".into(),
                objectives_text: "Slay 10 Kobold Vermin and recover 5 Chipped Boar Tusks, then \
                                  return to Marshal McBride."
                    .into(),
                details: "Your first task is one of cleansing. A clan of kobolds have infested \
                          the woods to the north. Go there and fight the kobold vermin you find. \
                          Reduce their numbers so that we may one day drive them from Northshire."
                    .into(),
                end_text: String::new(),
                objectives: [
                    QuestObjective {
                        creature_or_go: KOBOLD_ENTRY,
                        required_count: 10,
                        item_id: 0,
                        item_count: 0,
                        text: String::new(),
                    },
                    QuestObjective {
                        creature_or_go: 0,
                        required_count: 0,
                        item_id: 93_001,
                        item_count: 5,
                        text: String::new(),
                    },
                    blank.clone(),
                    blank.clone(),
                ],
            });
            // Entry 2: whole-quest COMPLETE — exercises the row's "(Complete)" tag.
            quest_log.insert_template(QuestTemplate {
                quest_id: 7,
                method: 2,
                level: 3,
                zone_or_sort: 12,
                quest_type: 0,
                rep_objective_faction: 0,
                rep_objective_value: 0,
                next_quest_in_chain: 0,
                money: 25,
                money_max_level: 0,
                reward_spell: 0,
                src_item_id: 0,
                flags: 0,
                rewards: [(0, 0); 4],
                choices: [(0, 0); 6],
                point_map_id: 0,
                point_x: 0.0,
                point_y: 0.0,
                point_opt: 0,
                title: "Kobold Camp Cleanup".into(),
                objectives_text: "Kill 10 kobold workers.".into(),
                details: "The kobold infestation grows.".into(),
                end_text: String::new(),
                objectives: [
                    QuestObjective {
                        creature_or_go: KOBOLD_ENTRY,
                        required_count: 10,
                        item_id: 0,
                        item_count: 0,
                        text: String::new(),
                    },
                    blank.clone(),
                    blank.clone(),
                    blank,
                ],
            });

            // The synthetic self player: slot 0 = quest 783 (counter0 = 3), slot 1 = quest 7
            // (counter0 = 10, state byte COMPLETE) — the packing `SetQuestSlotCounter`/
            // `SetQuestSlotState` write (0109 pin).
            let fields = ObjectFields::from_pairs(&[
                (198, 783),
                (199, 3),
                (200, 0),
                (201, 7),
                (202, 10 | (0x01 << 24)),
                (203, 0),
            ]);
            // A player guid + cached name ride along — the feeds resolve the player identity
            // (chat-macro substitution) through the same (ObjectStore, Guid) self query as live.
            const PLAYER_GUID: u64 = 0x51;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));

            // Open the book (the L binding's path). The feed pushes on the following frames of the
            // settle window; OnShow + QUEST_LOG_UPDATE repaint exactly as live.
            if let Some(s) = script.as_mut() {
                if let Err(e) = s.run("ToggleQuestLog()") {
                    warn!("capture: ui-questlog seed failed to open the log: {e}");
                }
            }
        }
        UiFixture::Loot => {
            items.insert_template(90_117, Some(template("Chipped Boar Tusk", 0)));
            items.insert_template(90_118, Some(template("Ruined Pelt", 1)));
            loot.open(
                NPC_GUID,
                benilla_protocol::messages::loot_type::CORPSE,
                4, // 4 copper — the coin row
                vec![
                    benilla_protocol::messages::LootItem {
                        slot: 0,
                        item_id: 90_117,
                        count: 1,
                        display_info_id: DISP_STONE,
                        random_property_id: 0,
                        slot_type: 0,
                    },
                    benilla_protocol::messages::LootItem {
                        slot: 1,
                        item_id: 90_118,
                        count: 3,
                        display_info_id: DISP_FOOD,
                        random_property_id: 0,
                        slot_type: 0,
                    },
                ],
            );
        }
        UiFixture::Bag => {
            let Some(mut script) = script else {
                return;
            };
            seed_bag_window(&mut script, icons.as_deref());
            seed_equipped_bags(&mut script, icons.as_deref());
        }
        UiFixture::Cooldown => {
            let Some(mut script) = script else {
                return;
            };
            seed_cooldown_filmstrip(&mut script, icons.as_deref());
        }
        UiFixture::CooldownShine => {
            let Some(mut script) = script else {
                return;
            };
            seed_cooldown_filmstrip(&mut script, icons.as_deref());
            // The pet bar's autocast shine, raised by hand: no pet is fed here, so the bar's own
            // show path never runs. The shine is the subject, not the bar's chrome.
            if let Err(e) = script.run(
                "PetActionBarFrame:Show()\n\
                 for i = 1, 4 do\n\
                     local b = getglobal(\"PetActionButton\"..i)\n\
                     if b then b:Show(); getglobal(b:GetName()..\"AutoCast\"):Show() end\n\
                 end",
            ) {
                warn!("capture: ui-cooldown-shine failed to raise the autocast shine: {e}");
            }
        }
        UiFixture::WorldMap => {
            let Some(mut script) = script else {
                return;
            };
            // Server-less, Player defaults to the origin — off every zone rect, so the blip
            // projects to the (0,0) hide sentinel and the arrow never shows. Park the avatar at
            // the scenario's Northshire spot so the arrow lands on the Elwynn map for real.
            player.pos = benilla_assets::coords::wow_to_bevy(scenario.eye);
            // Alternating explore bits: roughly half of every zone's overlays reveal, so the
            // capture shows fog doing its job (some sub-areas drawn, some parchment).
            script.set_world_map_explored(vec![0x5555_5555; 64]);
            // Open the map (fullscreen since 0221) at the Elwynn zone map (continent 2, zone 10 —
            // "Elwynn Forest" in the alphabetical zone list).
            if let Err(e) = script.run("ToggleWorldMap(); SetMapZoom(2, 10)") {
                warn!("capture: ui-worldmap seed failed to open the map: {e}");
            }
        }
        UiFixture::PartyInvite => {
            let Some(script) = script else {
                return;
            };
            // Raised through the real registry entry, not a hand-built frame: the text comes from
            // the chain's own `INVITATION` GlobalString and the two buttons from ACCEPT/DECLINE,
            // so the capture exercises the same Show path a real invite takes.
            if let Err(e) = script.run(r#"StaticPopup_Show("PARTY_INVITE", "Brisca")"#) {
                warn!("capture: ui-partyinvite seed failed to raise the dialog: {e}");
            }
        }
        UiFixture::Tooltip => {
            let Some(mut script) = script else {
                return;
            };
            seed_bag_window(&mut script, icons.as_deref());
            // The hovered shield's full template view + a player state that FAILS its level
            // requirement — the capture demos the whole 0274 P1 line law: quality name, bind,
            // slot|type, armor/block, stats, durability, a RED requirement line, a LONG green
            // Use: line (WRAPS at the wrap column — the shape whose two-step re-measure never
            // converged under the hover re-enter loop, the live bread/hearthstone spill), a
            // charges line AFTER the wrap (the spill's canary), and the quoted flavor text.
            script.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 12,
                class_id: 1,
                race_id: 1,
                skills: Default::default(),
                ..Default::default()
            });
            script.set_item_template(
                2362,
                benilla_ui::script::ItemTemplateView {
                    name: "Small Shield".into(),
                    quality: 2,
                    class: 4,
                    subclass: 6,
                    inventory_type: 14,
                    bonding: 2,
                    stats: vec![(7, 3)],
                    armor: 85,
                    block: 4,
                    max_durability: 45,
                    required_level: 20,
                    spell_triggers: vec![(
                        0,
                        72,
                        "Restores 243 health over 21 sec.  Must remain seated while eating.".into(),
                    )],
                    charges: 1,
                    description: "A stout little shield of Northshire pine.".into(),
                    sell_price: 152,
                    item_set: 161,
                    ..Default::default()
                },
            );
            // The item-SET block (`0x52b650`; real Defias Leather shape, 5 members, one equipped):
            // gold "(1/5)" header + spacer, cream/gray member ladder, green (2)-bonus vs gray
            // (4)-bonus — the whole block's visual regression instrument.
            let mut inv: benilla_ui::script::InventorySlots = Default::default();
            inv[4] = Some(benilla_ui::script::InvSlotView {
                durability: None,
                item_id: 6303,
                ..Default::default()
            });
            script.set_inventory_slots(inv);
            script.set_item_set(
                161,
                benilla_ui::script::ItemSetView {
                    name: "Defias Leather".into(),
                    members: vec![
                        (6303, Some("Defias Mark".into())),
                        (6304, Some("Defias Belt".into())),
                        (6305, Some("Defias Gloves".into())),
                        (6306, Some("Defias Trousers".into())),
                        (6307, Some("Defias Boots".into())),
                    ],
                    bonuses: vec![
                        (2, "Increases movement speed slightly.".into()),
                        (
                            4,
                            "Immune to Defias Pillager and Defias Looter spells.".into(),
                        ),
                    ],
                    ..Default::default()
                },
            );
            // Force the tooltip open over the top-left bag button (`Item16` ⇒ game slot 1, the
            // green Small Shield — see `seed_bag_window` on the backwards numbering) via the same
            // OnEnter path a hover fires — deterministic, no synthetic mouse. A top-left seat keeps
            // the ANCHOR_RIGHT tooltip on-screen; the engine's auto-size pass settles it over the
            // capture window.
            //
            // **Which window the backpack landed in is ASKED, never assumed**: the reference
            // recycles twelve `ContainerFrame`s across every container, so the index depends on
            // what else is open. `IsBagOpen` is its own published scan.
            if let Err(e) = script.run(
                "local i = IsBagOpen(0)\n\
                 if i then\n\
                     ContainerFrameItemButton_OnEnter(getglobal(\"ContainerFrame\"..i..\"Item16\"))\n\
                 end",
            ) {
                warn!("capture: ui-tooltip seed failed to open the tooltip: {e}");
            }
        }
        UiFixture::TooltipWorld => {
            let Some(mut script) = script else {
                return;
            };
            // A PvP-flagged friendly guard (3 lines: name / level+type / PvP) under the cursor,
            // pushed the way the mouseover feed pushes it, then
            // the engine's world drive — the same call `drive_mouseover_tooltip` makes on a hover
            // change. The ONLY thing this instrument is for is WHERE the plate sits: the default
            // corner (bottom-right, −13/+70 with the load-time offsets), per the now-wired
            // OnTooltipSetDefaultAnchor. Content (name/level/health bar) rides along.
            script.set_unit(
                "mouseover",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    name: Some("Stormwind Guard".into()),
                    health: 38,
                    max_health: 50,
                    level: 25,
                    reaction: 5,
                    creature_type_name: Some("Humanoid".into()),
                    // The faction-name line ("Stormwind", white, between level and PvP) — the
                    // director's Marshal McBride reference shape.
                    faction_name: Some("Stormwind".into()),
                    pvp: true,
                    ..Default::default()
                }),
            );
            if !script.world_tooltip_unit("mouseover") {
                warn!("capture: ui-tooltip-world seed failed to open the tooltip");
            }
        }
        UiFixture::Character => {
            // The paper doll fed the REAL way (the QuestLog pattern): a synthetic self player whose
            // descriptor carries the full 0208 stat block — the `ui_char` feed then builds the
            // snapshots and fires the events exactly as live. Values are a plausible level-12
            // warrior; one positive (stamina, fire) and one negative (spirit) buff exercise the
            // green/red stat coloring. All indices are the 0208-verified constants.
            use benilla_protocol::messages::ObjectFields;
            const PLAYER_GUID: u64 = 0x51;
            // Equipped item guids (chest / main hand / ranged) + an arrow stack in the backpack.
            const G_CHEST: u64 = 0x1001;
            const G_SWORD: u64 = 0x1002;
            const G_BOW: u64 = 0x1003;
            const G_ARROWS: u64 = 0x1004;

            let fields = ObjectFields::from_pairs(&[
                (34, 12),                 // UNIT_FIELD_LEVEL
                (36, 4 | 1 << 8),         // UNIT_FIELD_BYTES_0 — night elf warrior, male, mana
                (126, 2400),              // BASEATTACKTIME[0] ms
                (128, 2000),              // RANGEDATTACKTIME ms
                (134, 13.0f32.to_bits()), // MINDAMAGE
                (135, 19.0f32.to_bits()), // MAXDAMAGE
                // Stats: str/agi/sta/int/spi.
                (150, 45),
                (151, 25),
                (152, 40),
                (153, 15),
                (154, 20),
                // Resistances: armor + fire/nature/frost/arcane.
                (155, 250),
                (157, 10),
                (158, 5),
                (159, 10),
                (161, 15),
                (165, 78),                // ATTACK_POWER
                (168, 30),                // RANGED_ATTACK_POWER
                (171, 9.0f32.to_bits()),  // MINRANGEDDAMAGE
                (172, 14.5f32.to_bits()), // MAXRANGEDDAMAGE
                // PLAYER_SKILL_INFO triplets: swords 58/60, unarmed 24/60, bows 55/60.
                (718, 43),
                (719, 58 | 60 << 16),
                (721, 162),
                (722, 24 | 60 << 16),
                (724, 45),
                (725, 55 | 60 << 16),
                // Stat buffs (INT on the wire): +10 stamina, −5 spirit; +10 fire
                // resistance. The −5 is the two's-complement word an x86-hosted server sends; an
                // arm64 host saturates that same debuff to a flat 0, so the RED leg of the sheet is
                // exercisable here and not on this deploy.
                (1179, 10),             // POSSTAT2
                (1186, (-5i32) as u32), // NEGSTAT4
                (1189, 10),             // RESISTANCEBUFFMODSPOSITIVE[2] (fire)
                // Equipment guids (INV_SLOT_HEAD base 486 + 2·slot): chest 4, main hand 15,
                // ranged 17; the first backpack slot (PACK_SLOT_1 base 532) holds the arrows.
                (494, G_CHEST as u32),
                (516, G_SWORD as u32),
                (520, G_BOW as u32),
                (532, G_ARROWS as u32),
                (1223, 93_012), // PLAYER_AMMO_ID — the arrows' entry
            ]);

            // The item objects (entry + stack) and their ask-once templates. Icons resolve through
            // the offline catalog's known display ids; the bow reuses the hearthstone display (no
            // bow icon among the capture anchors — structural stand-in, not a look claim).
            let obj = |entry: u32, stack: u32| {
                ObjectFields::from_pairs(&[(3, entry), (14, stack)]) // OBJECT_ENTRY, STACK_COUNT
            };
            crate::items::spawn_item(&mut commands, &mut index, G_CHEST, obj(93_010, 1), false);
            crate::items::spawn_item(&mut commands, &mut index, G_SWORD, obj(93_011, 1), false);
            crate::items::spawn_item(&mut commands, &mut index, G_BOW, obj(93_013, 1), false);
            crate::items::spawn_item(&mut commands, &mut index, G_ARROWS, obj(93_012, 200), false);
            let mut chest = template("Tarnished Chainmail", 1);
            chest.display_info_id = DISP_SHIELD;
            chest.inventory_type = 5;
            items.insert_template(93_010, Some(chest));
            let mut sword = template("Militia Shortsword", 2);
            sword.class = 2; // weapon: sword 1h → skill 43 (the Attack row's skill line)
            sword.subclass = 7;
            sword.display_info_id = DISP_SWORD;
            sword.inventory_type = 21;
            sword.dmg_min = 13.0;
            sword.dmg_max = 19.0;
            sword.delay_ms = 2400;
            items.insert_template(93_011, Some(sword));
            let mut bow = template("Cracked Shortbow", 1);
            bow.class = 2; // weapon: bow → skill 45 (the ranged block's skill line)
            bow.subclass = 2;
            bow.display_info_id = DISP_STONE;
            bow.inventory_type = 15;
            items.insert_template(93_013, Some(bow));
            let mut arrows = template("Rough Arrow", 0);
            arrows.class = 6; // projectile — the ammo slot's icon + bag-summed count (200)
            arrows.inventory_type = 24; // INVTYPE_AMMO — the equip drains' SET_AMMO fork (0526)
            arrows.display_info_id = DISP_STONE;
            items.insert_template(93_012, Some(arrows));

            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));

            // Open the window (the C binding's path). The feed pushes on the following frames of
            // the settle window; OnShow + the UNIT_* events repaint exactly as live.
            if let Some(s) = script.as_mut() {
                if let Err(e) = s.run("ToggleCharacter(\"PaperDollFrame\")") {
                    warn!("capture: ui-char seed failed to open the window: {e}");
                }
            }
        }
        UiFixture::VPlates => {
            use benilla_protocol::messages::ObjectFields;
            // The synthetic self player, at the camera eye (the 20 yd plate gate measures from
            // here): level 2 human — the wolf cons YELLOW, the reference screenshot's digit.
            const PLAYER_GUID: u64 = 0x51;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 2),      // UNIT_FIELD_LEVEL
                    (35, 1),      // UNIT_FIELD_FACTIONTEMPLATE — human
                    (36, 0x0101), // UNIT_FIELD_BYTES_0 — race human, class warrior
                    // UNIT_FIELD_FLAGS bit 3 (`PLAYER_CONTROLLED`) — which every real player unit
                    // carries and this hand-built snapshot did not. `CanAttack` picks its terminal
                    // arm on this bit (1530): without it the observer reads as a creature, the pair
                    // takes the both-uncontrolled arm — which needs a HOSTILE reaction in one
                    // direction, and a wolf's is neutral — and the scenario's plate stopped being
                    // drawn at all. A fixture is a snapshot of the live component set; this field
                    // was simply missing from it.
                    (46, 0x8),
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
                Transform::from_translation(wow_to_bevy(scenario.eye)),
            ));
            // The wolf: the live spawn's component set (net/apply.rs) with the descriptor seeded
            // directly, standing at the look point facing the camera, at full health.
            names.insert_creature(
                WOLF_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Timber Wolf".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            let wolf = commands
                .spawn((
                    crate::net::Guid(WOLF_GUID),
                    crate::net::NetEntity {
                        kind: benilla_protocol::EntityKind::Unit,
                        display_id: Some(WOLF_DISPLAY),
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(&[
                        (22, 100),          // UNIT_FIELD_HEALTH
                        (28, 100),          // UNIT_FIELD_MAXHEALTH
                        (34, 2),            // UNIT_FIELD_LEVEL
                        (35, WOLF_FACTION), // UNIT_FIELD_FACTIONTEMPLATE
                    ])),
                    Transform {
                        translation: wow_to_bevy(WOLF_POS),
                        rotation: Quat::from_rotation_y(3.8),
                        ..default()
                    },
                    Visibility::default(),
                ))
                .id();
            vplates.enemies = true;
            // The wolf is the TARGET: the plate draws lit — the bar's uniform brighten (the
            // watcher's target leg) — plus the target's ring/emissive, the real targeted look
            // this fixture regression-pins.
            selection.target = Some(wolf);
            selection.guid = Some(WOLF_GUID);
        }
        UiFixture::Options => {
            let Some(script) = script else {
                return;
            };
            // Static window, opened through the live panel path — nothing else to seed. What the
            // capture pins: the era chrome (nine-slice seams, right-edge straddle), the tab
            // plates, the search-box seat, the category list art with Controls selected (the
            // OnShow default), and the window's fit scale.
            if let Err(e) = script.run("ShowUIPanel(BenillaOptionsFrame)") {
                warn!("capture: ui-options seed failed to open the window: {e}");
            }
        }
        UiFixture::OptionsAudio => {
            let Some(script) = script else {
                return;
            };
            // The Audio page: register the real CVar set first — the hermetic capture has
            // no CvarPlugin file load to race, and the rows must read real values, not the
            // nil-tolerant zeros — then open and select through the live paths.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameCategoryListRowAudio:Click()",
            ) {
                warn!("capture: ui-options-audio seed failed: {e}");
            }
        }
        UiFixture::OptionsGraphics => {
            let Some(script) = script else {
                return;
            };
            // The Graphics page, same posture as the Audio fixture: real CVar set, live
            // open-and-select paths.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) =
                script.run("ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameCategoryListRowGraphics:Click()")
            {
                warn!("capture: ui-options-graphics seed failed: {e}");
            }
        }
        UiFixture::OptionsChat => {
            let Some(script) = script else {
                return;
            };
            // The Chat page (1589), the page fixtures' posture: the real CVar set, then the live
            // open path. Its Remove Chat Hover Delay row reads a saved-variable global that
            // `ChatFrame.xml` declares at file scope, so a hermetic capture sees the shipped "0"
            // and the row paints unchecked — which is the shipped default, not a missing load.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameCategoryListRowChat:Click()",
            ) {
                warn!("capture: ui-options-chat seed failed: {e}");
            }
        }
        UiFixture::ColorPicker => {
            let Some(script) = script else {
                return;
            };
            // The corpus's own opening move (Dewdrop-2.0's colour row, `ColorPickerFrame.xml`'s
            // header): set the colour, ask for opacity, show the window. A saturated mid-hue at
            // three-quarter brightness puts BOTH markers off their defaults — the wheel's off
            // centre and off the axes, the strip's a quarter down — so a mirrored axis or a
            // swapped marker is visible in the still.
            if let Err(e) = script.run(
                "ColorPickerFrame.hasOpacity = 1; ColorPickerFrame.opacity = 0.3; \
                 ColorPickerFrame:SetColorRGB(0.15, 0.55, 0.75); \
                 ShowUIPanel(ColorPickerFrame)",
            ) {
                warn!("capture: ui-color-picker seed failed: {e}");
            }
        }
        UiFixture::OptionsDropdownList => {
            let Some(script) = script else {
                return;
            };
            // The dropdown list open (re-seated onto Camera Following Style by 1649), same
            // posture as the page fixtures: real CVar set, the live open-select-toggle path. The
            // list's width lands inside the click that opens it — the stock kit's
            // `UIDropDownMenu_Refresh` sizes every button from `normalText:GetWidth() + 60`
            // (`UIDropDownMenu.lua` l.395-422) and the engine's measurer answers that getter in
            // the call that asked, so there is no settle to wait out.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameCategoryListRowControls:Click(); \
                 BenillaOptionsFrameContainerBodyControlsRowCameraFollowStyleDropdownButton:Click()",
            ) {
                warn!("capture: ui-options-dropdown seed failed: {e}");
            }
        }
        UiFixture::KeyBindings => {
            let Some(mut script) = script else {
                return;
            };
            // The Keybindings page (1008 — the Options window's category), the page fixtures'
            // posture: the real command registry first (hermetic capture — the plugin's
            // PostStartup seed isn't raced, the register_cvars precedent), then the live open
            // path; CVars registered too so the sibling category rows behave. Movement is
            // expanded so the lens sees both a header row and the byte-real default capsules.
            script.register_cvars(crate::cvars::registered_pairs());
            script.register_bindings(&crate::bindings::registry_commands());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); \
                 BenillaOptionsFrameCategoryListRowKeybindings:Click(); \
                 KeyBindings_ExpandSection(1, true); KeyBindingsPage_Update()",
            ) {
                warn!("capture: ui-keybindings seed failed: {e}");
            }
        }
        UiFixture::OptionsSearch => {
            let Some(script) = script else {
                return;
            };
            // Mid-search, same posture as the page fixtures: real CVar set, then the
            // live open path and a typed query — "volume" reflows the four volume sliders
            // under the Audio head (Master matched by the token too, so no pull-in here; the
            // pull-in has its unit test). Focused: captures pin the caret visible, so
            // this baseline also pins the caret hugging the text's end — the drawn-space
            // advance law's visual regression guard.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameSearchBox:SetText(\"volume\"); BenillaOptionsFrameSearchBox:SetFocus()",
            )
            {
                warn!("capture: ui-options-search seed failed: {e}");
            }
        }
        UiFixture::SpellBook => {
            // The director's own report reproduced: a HUMAN WARRIOR (race 1,
            // class 1) who learned Fireball + Mind Flay via a GM command during testing. The book
            // resolves through the REAL chain (live 1.12 spell ids; names/icons/ranks from the
            // local Spell.dbc, tab lines from SkillLineAbility.dbc, the General collapse from
            // SkillRaceClassInfo.dbc keyed on the self player's race/class — nothing pushed by
            // hand). The self player's descriptor carries the race/class the tab classifier reads.
            use benilla_protocol::messages::ObjectFields;
            const PLAYER_GUID: u64 = 0x51;
            const G_SWORD: u64 = 0x1002;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 12),              // UNIT_FIELD_LEVEL
                    (36, 1 | 1 << 8),      // UNIT_FIELD_BYTES_0 — human (race 1), warrior (class 1)
                    (516, G_SWORD as u32), // main-hand item guid (INV_SLOT_HEAD 486 + 15·2)
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));
            // The equipped main-hand weapon — the auto-attack borrows its icon.
            crate::items::spawn_item(
                &mut commands,
                &mut index,
                G_SWORD,
                ObjectFields::from_pairs(&[(3, 93_011), (14, 1)]),
                false,
            );
            let mut sword = template("Militia Shortsword", 2);
            sword.display_info_id = DISP_SWORD;
            items.insert_template(93_011, Some(sword));
            let Some(script) = script.as_mut() else {
                return;
            };
            actions.spells.extend([
                // The auto-attack — its icon must be the equipped sword, NOT spell 6603's `Temp`
                // placeholder face. Lands in General (no skill line).
                6603, // Attack
                // Warrior class abilities → their own class-line tabs (flag clear for a warrior):
                // Charge/Heroic Strike/Rend on Arms, Battle Shout on Fury.
                100,  // Charge (Arms)
                78,   // Heroic Strike (Arms)
                772,  // Rend (Arms)
                6673, // Battle Shout (Fury)
                // A human racial (Racial - Human line) → collapses to the General tab.
                20600, // Perception
                // The cheated cross-class test spells: no Fire/Shadow SkillRaceClassInfo row admits
                // a warrior, so BOTH collapse into General — the capture must show NO Fire/Shadow
                // tab (the director's spurious tabs), those spells sitting in General instead.
                133, // Fireball (Fire)
                589, // Shadow Word: Pain (Shadow)
                // Add-gate check: a language + an armor proficiency a live warrior carries, both
                // DO_NOT_DISPLAY — must NOT appear at all.
                668,  // Language: Common
                9078, // Cloth proficiency
            ]);
            // Open the book (the P binding's own path). The feed pushes the model on the
            // following frames of the settle window; SPELLS_CHANGED repaints exactly as live.
            if let Err(e) = script.run("ToggleSpellBook(BOOKTYPE_SPELL)") {
                warn!("capture: ui-spellbook seed failed to open the book: {e}");
            }
        }
        UiFixture::Macro | UiFixture::MacroPopup => {
            let Some(script) = script.as_mut() else {
                return;
            };
            // Made through the LIVE `CreateMacro` path, not by pushing a table in — so the capture
            // exercises engine table → `UPDATE_MACROS` → window exactly as a player's own edit
            // does. Icon indices are into the real `SpellIcon.dbc` catalog `ui_macro` loads at
            // PostStartup, so they resolve to real art. The bodies are the shapes that matter: a
            // plain `/cast`, a MULTI-LINE macro (what the body box has to lay out), and a
            // `/script` line (the non-`/cast` case the bar's bound-spell resolve must decline).
            const SEED: [(&str, u32, &str); 4] = [
                ("Charge", 1, "/cast Charge"),
                (
                    "Pull",
                    9,
                    "/cast Charge\n/say Incoming!\n/script CastSpellByName(\"Battle Shout\")",
                ),
                ("Shout", 17, "/cast Battle Shout"),
                ("Sit", 25, "/sit"),
            ];
            let mut seed = String::new();
            for (name, icon, body) in SEED {
                seed.push_str(&format!(
                    "CreateMacro(\"{name}\", {icon}, \"{}\", 1, nil)\n",
                    // Lua-escape the body: a `/script` line carries its own quotes, and a macro
                    // body's newlines have to survive as newlines.
                    body.replace('"', "\\\"").replace('\n', "\\n")
                ));
            }
            // Slot 2 selected: the multi-line body, so the detail pane and the body box both have
            // something in them. The popup fixture then opens the chooser over that selection via
            // the ref's `MacroEditButton` path — an EDIT, so the name box arrives pre-filled.
            seed.push_str("ShowMacroFrame()\nMacroButton2:Click()\n");
            if fixture == UiFixture::MacroPopup {
                seed.push_str("MacroEditButton:Click()\n");
            }
            if let Err(e) = script.run(&seed) {
                warn!("capture: ui-macro seed failed: {e}");
            }
        }
        UiFixture::Social => {
            // Nothing to seed: the stray capsule rode the pane's *declaration*, not its
            // contents — an empty friends list opens the same frames a full one does.
            let Some(script) = script.as_mut() else {
                return;
            };
            if let Err(e) = script.run("ToggleFriendsFrame(1)") {
                warn!("capture: ui-social seed failed to open the pane: {e}");
            }
        }
        UiFixture::ChatEdit => {
            let Some(script) = script.as_mut() else {
                return;
            };
            // The director's repro shape: a say/yell mix behind the OPEN edit box. The box goes
            // through the live open path (focus + a typed draft); `chat_edit_live` then drives the
            // header text/color and the `15 + headerWidth` insets over the settle frames exactly
            // as in-game, so the capture checks header seating AND typed-text visibility.
            for (text, r, g, b) in [
                ("[One] says: testing the box", 1.0, 1.0, 1.0),
                ("[One] says: a second line to stack", 1.0, 1.0, 1.0),
                ("[One] yells: FUU", 1.0, 64.0 / 255.0, 64.0 / 255.0),
            ] {
                script.add_chat_message("ChatFrame1", text, r, g, b);
            }
            script.focus_editbox("ChatFrameEditBox");
            // A draft with "northshire" selected (`HighlightText(6,16)`): the capture shows the
            // opaque-gray selection highlight (ctor 0xFF606060) under the glyphs AND the white
            // caret at the selection's end — the whole text-UI overlay stack in one golden.
            // …then pin the hover-revealed chrome: in-game the tab + the black window textures
            // follow the OS cursor (FCF_OnUpdate), which a headless capture can't hover —
            // replace the OnUpdate with a fixed-reveal one so the golden also locks the window
            // tint (chat-cache COLOR 0 0 0) and the text-sized tab (BenillaFCF_TabResize, which
            // must keep retrying until the label's measure lands).
            if let Err(e) = script.run(
                "ChatFrameEditBox:SetText(\"hello northshire\")\n\
                 ChatFrameEditBox:HighlightText(6, 16)\n\
                 ChatFrame1:SetScript('OnUpdate', function()\n\
                     PanelTemplates_TabResize(10, ChatFrame1Tab)\n\
                     ChatFrame1Tab:SetAlpha(1.0)\n\
                     FCF_SetWindowAlpha(ChatFrame1, 0.25, 1)\n\
                 end)",
            ) {
                warn!("capture: ui-chatedit seed failed: {e}");
            }
        }
        UiFixture::ChatTabHover => {
            let Some(script) = script.as_mut() else {
                return;
            };
            // Lines so the window has content, the Combat Log selected, then the cursor parked
            // on the GENERAL tab — the unselected-tab hover the director reported.
            //
            // A capture CAN hover: the app only pumps the OS cursor into the VM on a non-synthetic
            // run (`ui_script::input`'s `.filter(|_| !ui_hidden && !synthetic)`), so a fed
            // `mouse_move` is what the whole run then sees. That is the hover instrument decision
            // 0254 wrote down as missing ("No capture scenario exercises a hover state, so nothing
            // in the harness covers the very texture this decision is about") — and 0254's own
            // named residual is exactly what this scenario turned out to show.
            for (text, r, g, b) in [
                ("[One] says: hello northshire", 1.0, 1.0, 1.0),
                ("[One] says: a second line to stack", 1.0, 1.0, 1.0),
            ] {
                script.add_chat_message("ChatFrame1", text, r, g, b);
            }
            // `$WOW_TABHOVER` picks which of the five dock states to shoot. It is an A/B rig,
            // not a setting: the four alternates are what let a change to the composite be read
            // as a difference rather than judged from one frame. Default "1" is the reported
            // case, so the golden is stable without the variable.
            //   1 (default)  Combat Log selected, hovering GENERAL      — the report
            //   3            General selected, hovering COMBAT LOG      — its mirror
            //   2            Combat Log selected, hovering COMBAT LOG   — hover on the SELECTED tab
            //   0            revealed, hovering neither tab             — the no-glow control
            //   9            cursor off the dock                        — bare scene, the baseline
            //                                                             a tab quad is measured against
            let mode = std::env::var("WOW_TABHOVER").unwrap_or_else(|_| "1".into());
            let select = if mode == "3" { 1 } else { 2 };
            if let Err(e) = script.run(&format!("FCF_SelectDockFrame(ChatFrame{select})")) {
                warn!("capture: ui-chat-tabhover select failed: {e}");
            }
            script.resolve();
            let expr = match mode.as_str() {
                "2" | "3" => "return ChatFrame2Tab:GetCenter()",
                "0" => {
                    "return (ChatFrame2:GetLeft() + ChatFrame2:GetRight()) / 2, \
                        (ChatFrame2:GetBottom() + ChatFrame2:GetTop()) / 2"
                }
                // 9: park far away — the dock conceals itself, so the tab band is bare scene.
                "9" => "return 2000, 2000",
                _ => "return ChatFrame1Tab:GetCenter()",
            };
            let centre: Result<(f32, f32), _> = script.eval(expr);
            match centre {
                Ok((x, y)) => {
                    script.mouse_move(x, y);
                    // Settle the reveal ramp without waiting on the app's own frames.
                    for _ in 0..40 {
                        if let Err(e) = script.run("FCF_OnUpdate(0.05)") {
                            warn!("capture: ui-chat-tabhover pump failed: {e}");
                            break;
                        }
                        script.resolve();
                    }
                }
                Err(e) => warn!("capture: ui-chat-tabhover centre failed: {e}"),
            }
        }
        UiFixture::NameWater => {
            use benilla_protocol::messages::ObjectFields;
            // The subject is an NPC's overhead name, and `UnitNameNPC` registers "0" (1804's
            // byte-read default), so without this the shot contains no name at all — which is
            // exactly what it had contained since 1804 landed. Set through Lua, the way a player
            // turns it on, so the sync drains it into `NameConfig` like any other CVar write.
            if let Some(script) = script.as_deref_mut() {
                if let Err(e) = script.run("SetCVar(\"UnitNameNPC\", \"1\")") {
                    warn!("capture: name fixture could not enable UnitNameNPC: {e}");
                }
            }
            // The synthetic self player at the eye (the reaction lookup reads its store, and the
            // name colour is that verdict).
            const SELF_GUID: u64 = 0x51;
            names.insert_player(SELF_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 2),      // UNIT_FIELD_LEVEL
                    (35, 1),      // UNIT_FIELD_FACTIONTEMPLATE — human
                    (36, 0x0101), // UNIT_FIELD_BYTES_0 — race human, class warrior
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(SELF_GUID),
                Transform::from_translation(wow_to_bevy(scenario.eye)),
            ));
            // The named unit out in the river (the `vplates` wolf, re-seated): 25 yd along the
            // scenario's own look bearing, so its overhead name lands on the water surface
            // BEYOND it — the geometry that catches a plate sorting before the liquid.
            names.insert_creature(
                WOLF_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Timber Wolf".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
            commands.spawn((
                crate::net::Guid(WOLF_GUID),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: Some(WOLF_DISPLAY),
                    scale: 1.0,
                },
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),          // UNIT_FIELD_HEALTH
                    (28, 100),          // UNIT_FIELD_MAXHEALTH
                    (34, 2),            // UNIT_FIELD_LEVEL
                    (35, WOLF_FACTION), // UNIT_FIELD_FACTIONTEMPLATE
                ])),
                Transform {
                    translation: wow_to_bevy(NAME_WATER_POS),
                    rotation: Quat::from_rotation_y(2.2),
                    ..default()
                },
                Visibility::default(),
            ));
            // The floating NAME is the subject, so no V-plate may exist (a plated unit never
            // draws one — the ShouldShowName exclusivity) even though enemy plates boot ON.
            vplates.enemies = false;
        }
        // The lighting matrix. One spawn, through the same component set a streamed
        // entity gets, at a position whose light lane was read out of the data (see the matrix note
        // in `scenarios`). Deliberately ANONYMOUS: no name is registered and plates are forced off,
        // so no glyph rides over the body — the diff of these cells must be about light and nothing
        // else.
        UiFixture::Subject { kind, at } => {
            use benilla_protocol::messages::ObjectFields;
            let transform = Transform {
                translation: wow_to_bevy(at),
                rotation: Quat::from_rotation_y(SUBJECT_YAW),
                ..default()
            };
            match kind {
                SubjectKind::Creature => {
                    commands.spawn((
                        crate::net::Guid(WOLF_GUID),
                        crate::net::NetEntity {
                            kind: benilla_protocol::EntityKind::Unit,
                            display_id: Some(WOLF_DISPLAY),
                            scale: 1.0,
                        },
                        crate::net::ObjectStore(ObjectFields::from_pairs(&[
                            (22, 100),          // UNIT_FIELD_HEALTH
                            (28, 100),          // UNIT_FIELD_MAXHEALTH
                            (34, 2),            // UNIT_FIELD_LEVEL
                            (35, WOLF_FACTION), // UNIT_FIELD_FACTIONTEMPLATE
                        ])),
                        transform,
                        Visibility::default(),
                    ));
                }
                SubjectKind::Chest => {
                    commands.spawn((
                        crate::net::Guid(CHEST_GUID),
                        crate::net::NetEntity {
                            kind: benilla_protocol::EntityKind::GameObject,
                            display_id: Some(CHEST_DISPLAY),
                            scale: 1.0,
                        },
                        crate::net::ObjectStore(ObjectFields::default()),
                        transform,
                        Visibility::default(),
                    ));
                }
            }
            vplates.enemies = false;
        }
    }
}

/// Seed + open the backpack window with a fixed item set — shared by the `Bag` and `Tooltip`
/// fixtures. The bag is a standalone addon (no `ShowUIPanel` path): drive the container snapshot
/// and purse directly, then open and paint. The feed (`crate::ui_items`) leaves bag 0 alone when
/// there is no `SelfPlayer` (net is disabled in capture), so this manual snapshot is not clobbered.
/// The pinned `GetTime()` value the cooldown filmstrip parks the VM's session clock at, seconds.
/// Large, because the reference's own `CooldownFrame_SetTimer` refuses a `start <= 0` and the
/// starts here are `now − fraction · span`.
const COOLDOWN_NOW_S: f64 = 100_000.0;

/// Each filmstrip slot's cooldown duration, seconds. Long enough that the settle window's few
/// seconds of VM clock move a phase by `~5e-4` of a step, so the shot is reproducible.
const COOLDOWN_SPAN_S: f64 = 10_000.0;

/// **The cooldown sweep as a filmstrip**: sixteen backpack slots, each parked at its own
/// fraction of one very long cooldown, so the window shows sixteen points of the 1000 ms sweep at
/// once — the instrument for "what does the indicator actually draw", which no test can reach
/// (the engine tests prove the scrub, not the picture).
///
/// Game slot 1 renders TOP-LEFT and slot 16 bottom-right (`ContainerFrame_GenerateFrame` numbers
/// backwards — see [`seed_bag_window`]), so ascending slot is reading order and ascending
/// fraction reads as a filmstrip.
fn seed_cooldown_filmstrip(
    script: &mut benilla_ui::script::UiScript,
    icons: Option<&crate::entities::ItemDisplays>,
) {
    // Park the session clock FIRST: `set_container` stores each triple against it, and
    // `GetContainerItemCooldown`'s cold-at-expiry guard reads it.
    if let Err(e) = script.run(&format!("__benilla_now = {COOLDOWN_NOW_S}")) {
        warn!("capture: ui-cooldown failed to pin the session clock: {e}");
    }
    const DISP_STONE: u32 = 6418;
    let texture = icons
        .and_then(|i| i.catalog.get(DISP_STONE))
        .and_then(|d| d.icon.clone());
    let span_ms = COOLDOWN_SPAN_S * 1000.0;
    let now_ms = COOLDOWN_NOW_S * 1000.0;
    let mut slots = std::collections::HashMap::new();
    for slot in 1u32..=16 {
        // Sixteen phases across the sweep, biased off both ends: 0 is the uniform disc and 1 is
        // the flash, and neither is what this instrument is looking at.
        let fraction = (f64::from(slot) - 0.5) / 16.0;
        slots.insert(
            slot,
            benilla_ui::script::ContainerSlot {
                petition: None,
                durability: None,
                duration_ms: None,
                bar_placeable: true,
                texture: texture.clone(),
                count: 1,
                quality: Some(1),
                item_id: 6948,
                link: Some("|cffffffff|Hitem:6948|h[Hearthstone]|h|r".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: Some(((now_ms - fraction * span_ms) as i64, span_ms as u32, true)),
                readable: false,
                creator: None,
                flags: 0,
                already_bound: false,
                enchants: Vec::new(),
            },
        );
    }
    script.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    if let Err(e) = script.run("OpenBag(0)") {
        warn!("capture: ui-cooldown failed to open the backpack: {e}");
    }
}

fn seed_bag_window(
    script: &mut benilla_ui::script::UiScript,
    icons: Option<&crate::entities::ItemDisplays>,
) {
    // Display ids the offline icon catalog is known to resolve (crates/benilla-formats items.rs).
    const DISP_SWORD: u32 = 1542;
    const DISP_FOOD: u32 = 2473;
    const DISP_SHIELD: u32 = 18730;
    const DISP_STONE: u32 = 6418;
    // Icon path from the same offline ItemDisplayInfo catalog the live feed reads.
    let icon = |disp: u32| -> Option<String> {
        icons
            .and_then(|i| i.catalog.get(disp))
            .and_then(|d| d.icon.clone())
    };
    let slot = |disp: u32, count: u32, id: u32, name: &str, quality: u32| {
        benilla_ui::script::ContainerSlot {
            petition: None,
            durability: None,
            duration_ms: None,
            bar_placeable: true,
            texture: icon(disp),
            count,
            quality: Some(quality),
            item_id: id,
            link: Some(format!("|cffffffff|Hitem:{id}|h[{name}]|h|r")),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            already_bound: false,
            enchants: Vec::new(),
        }
    };
    // ~5 items scattered across the 16 slots, a mix of stacked and unstacked, under their real
    // vanilla template ids (the Tooltip fixture seeds a full template view for the hovered one).
    // Game slot 1 carries the green-quality Small Shield the Tooltip fixture hovers, and it is
    // rendered TOP-LEFT: `ContainerFrame_GenerateFrame` numbers backwards (`index = size - j + 1`)
    // and anchors `Item1` at the window's BOTTOMRIGHT, filling leftwards then upwards — so for a
    // 16-slot bag the top-left button is `Item16` and it carries slot 1. A top-left seat keeps its
    // ANCHOR_RIGHT tooltip on-screen.
    let mut slots = std::collections::HashMap::new();
    slots.insert(1, slot(DISP_SHIELD, 1, 2362, "Small Shield", 2));
    slots.insert(3, slot(DISP_FOOD, 5, 117, "Tough Jerky", 1));
    slots.insert(6, slot(DISP_SWORD, 1, 25, "Worn Shortsword", 1));
    slots.insert(11, slot(DISP_STONE, 1, 6948, "Hearthstone", 1));
    slots.insert(16, slot(DISP_FOOD, 20, 159, "Refreshing Spring Water", 1));
    script.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    // Player money nonzero so the purse renders all three denominations (1g 23s 45c).
    script.set_money(12_345);
    // Open the window with the reference's own verb — it builds and paints one in the same call
    // (`ToggleBag` → `OpenBag` → `ContainerFrame_GenerateFrame`), which is why nothing has to
    // repaint it afterwards. `OpenBag(0)` rather than `OpenBackpack()`: only the backpack is fed
    // here, and 0561 makes the latter open every equipped bag.
    if let Err(e) = script.run("OpenBag(0)") {
        warn!("capture: bag window seed failed: {e}");
    }

    // Seed a few chat lines into ChatFrame1 (the bottom-left docked window) — the look-pass
    // instrument for the chat-line drop shadow + the ChatFontNormal wiring (Slice C). This rides the
    // Bag/Tooltip fixtures, NOT the merchant/gossip A/B pair, so the DPI before/after captures stay
    // identical. The pinned per-type colors: SYSTEM yellow, SAY white, LOOT green. The LOOT pair is
    // also the regression baseline for the item link inside a coloured line (`ui_loot::receive_line`
    // — the quality escape wins over the line colour for the bracketed name, and the `x2` after the
    // `|r` falls back to LOOT green): a common/white Tough Jerky and an uncommon/green stack.
    for (text, r, g, b) in [
        ("Welcome to Northshire Valley.", 1.0, 1.0, 0.0),
        ("[Marshal McBride] says: Well met, citizen.", 1.0, 1.0, 1.0),
        (
            "You receive loot: |cffffffff|Hitem:117:0:0:0|h[Tough Jerky]|h|r.",
            0.0,
            170.0 / 255.0,
            0.0,
        ),
        (
            "You receive loot: |cff1eff00|Hitem:4306:0:0:0|h[Silk Cloth]|h|rx2.",
            0.0,
            170.0 / 255.0,
            0.0,
        ),
    ] {
        script.add_chat_message("ChatFrame1", text, r, g, b);
    }
}

/// Seed + open three equipped-bag windows at DIFFERENT sizes so the capture shows the snug-fit
/// background stitch (BenillaBagWindow_FitBackground) across its cases: a 6-slot pouch (2 rows,
/// partial top row — the director's Small Brown Pouch), an 8-slot bag (2 rows, full top row), and a
/// 10-slot bag (3 rows, partial top row). They stack up-and-left from the backpack via the window
/// stack, exactly as in-game — so the capture is the look-check for both the per-bag height and the
/// stack layout. A couple of items each verify the slot rings still seat on the baked wells.
fn seed_equipped_bags(
    script: &mut benilla_ui::script::UiScript,
    icons: Option<&crate::entities::ItemDisplays>,
) {
    const DISP_SWORD: u32 = 1542;
    const DISP_FOOD: u32 = 2473;
    const DISP_STONE: u32 = 6418;
    let icon = |disp: u32| -> Option<String> {
        icons
            .and_then(|i| i.catalog.get(disp))
            .and_then(|d| d.icon.clone())
    };
    let slot =
        |disp: u32, count: u32, name: &str, quality: u32| benilla_ui::script::ContainerSlot {
            petition: None,
            durability: None,
            duration_ms: None,
            bar_placeable: true,
            texture: icon(disp),
            count,
            quality: Some(quality),
            item_id: 0,
            link: Some(format!("|cffffffff|Hitem:0|h[{name}]|h|r")),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            already_bound: false,
            enchants: Vec::new(),
        };

    // Seed the four equipped-bag EQUIPMENT slots (live inv ids 20..23 = Bag0Slot..Bag3Slot) with a
    // bag item icon each, so every bag window's portrait shows its OWN bag icon (the real client's
    // SetBagPortaitTexture, now wired) instead of the default backpack — the char feed does this
    // live; capture has no feed, so seed it here.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    for (id, icon_name) in [
        (20usize, "INV_Misc_Bag_08"),
        (21, "INV_Misc_Bag_10_Blue"),
        (22, "INV_Misc_Bag_09"),
    ] {
        inv[id] = Some(benilla_ui::script::InvSlotView {
            durability: None,
            item_id: 1,
            icon: Some(format!("Interface\\Icons\\{icon_name}")),
            count: 1,
            quality: 1,
            ..Default::default()
        });
    }
    script.set_inventory_slots(inv);

    for (bag_id, name, size, items) in [
        (
            // The director's case — every slot filled so the well/ring alignment is visible on all six.
            1,
            "Small Brown Pouch",
            6u32,
            vec![
                (1u32, DISP_SWORD, 1u32, "Worn Dagger", 1u32),
                (2, DISP_FOOD, 5, "Tough Jerky", 1),
                (3, DISP_STONE, 1, "Rough Stone", 1),
                (4, DISP_SWORD, 1, "Worn Shortsword", 1),
                (5, DISP_FOOD, 3, "Spring Water", 1),
                (6, DISP_STONE, 2, "Coarse Stone", 1),
            ],
        ),
        (
            2,
            "Light Leather Bag",
            8,
            vec![
                (1, DISP_FOOD, 5, "Tough Jerky", 1),
                (5, DISP_SWORD, 1, "Worn Dagger", 1),
            ],
        ),
        (
            3,
            "Journeyman's Backpack",
            10,
            vec![
                (2, DISP_STONE, 3, "Coarse Stone", 1),
                (7, DISP_SWORD, 1, "Worn Shortsword", 1),
            ],
        ),
    ] {
        let mut slots = std::collections::HashMap::new();
        for (s, disp, count, iname, q) in items {
            slots.insert(s, slot(disp, count, iname, q));
        }
        script.set_container(
            bag_id,
            Some(benilla_ui::script::ContainerState {
                name: Some(name.into()),
                num_slots: size,
                slots,
            }),
        );
        // The reference's own verb, which builds the window as it opens it — and it must run
        // AFTER `set_container`, since `OpenBag` refuses a container with no slots.
        if let Err(e) = script.run(&format!("OpenBag({bag_id})")) {
            warn!("capture: equipped bag {bag_id} seed failed: {e}");
        }
    }
}
