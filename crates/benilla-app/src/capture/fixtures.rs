//! The capture UI fixtures: each arm seeds one window's synthetic but realistic state.

use super::*;

/// The `vplates` fixture's wolf, the reference screenshot's subject (vmangos `creature_template`
/// 69 "Timber Wolf": level 2, faction template 32, display 604), at the scenario's look point.
const WOLF_ENTRY: u32 = 69;
const WOLF_GUID: u64 = (0xF130u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 0x69;
const WOLF_POS: [f32; 3] = [-8949.95, -132.49, 83.9];
const WOLF_DISPLAY: u32 = 604;
const WOLF_FACTION: u32 = 32;

/// The `name-water` fixture's wolf, 25 yd along the water scenario's look bearing at the river
/// surface, so its overhead name projects onto the water beyond it.
pub(super) const NAME_WATER_POS: [f32; 3] = [-9512.97, -331.29, 61.4];

/// The lighting matrix's chest: `GameObjectDisplayInfo` 259, `TreasureChest01.mdx`. GameObject
/// guids carry the `0xF110` high word; the default descriptor holds the closed rest pose.
const CHEST_DISPLAY: u32 = 259;
const CHEST_GUID: u64 = (0xF110u64 << 48) | 0x744;

/// A lighting-matrix subject's Bevy yaw, facing the `front` camera on the sun's bearing (azimuth
/// 45°), so `front` shows the lit side and `rear` the unlit one.
const SUBJECT_YAW: f32 = 2.36;

/// Seeds the fixture window's state once the scene is resident; the real feeds push it into the VM
/// during the settle window as live wire data would. Icons resolve through the offline
/// `ItemDisplayCatalog`, and names go straight into the caches.
pub(super) fn seed_ui_fixture(
    mut ctx: ResMut<CaptureCtx>,
    mut commands: Commands,
    progress: Res<WorldLoadProgress>,
    mut merchant: ResMut<crate::ui_merchant::MerchantOpen>,
    mut gossip: ResMut<crate::ui_gossip::GossipState>,
    mut quest: ResMut<crate::ui_quest::QuestGiver>,
    mut quest_log: ResMut<crate::ui_quest_log::QuestLog>,
    mut loot: ResMut<crate::ui_loot::LootState>,
    // One param under Bevy's 16-param cap: item objects are entities in the index, spawned as
    // the wire does.
    store: (ResMut<crate::items::Items>, ResMut<crate::net::GuidIndex>),
    mut names: ResMut<crate::names::NameCache>,
    icons: Option<Res<crate::entities::ItemDisplays>>,
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut vplates: ResMut<crate::vplates::VPlateMode>,
    mut selection: ResMut<crate::target::Selection>,
    mut player: ResMut<crate::player::Player>,
    // A nested tuple, one param under Bevy's 16-param cap.
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

    // A UI capture with no script VM would be a UI-less world under the scenario's name, so it
    // exits non-zero, as the window-size refusal does.
    if script.is_none() {
        error!(
            "capture: REFUSING this capture — scenario {:?} declares a UI fixture but no script \
             VM exists, so its window cannot be opened and the shot would be a UI-less world \
             wearing the scenario's name.",
            scenario.name
        );
        exit.write(AppExit::error());
        return;
    }

    // A creature guid whose entry bits (24-47) carry 90001; the NameCache resolves NPC names by
    // entry.
    const NPC_ENTRY: u32 = 90_001;
    const NPC_GUID: u64 = (0xF130u64 << 48) | ((NPC_ENTRY as u64) << 24) | 0x42;
    // Display ids the offline icon catalog resolves: sword, food, shield, hearthstone.
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
        // Nothing to open: the loaded UI and `demo_unit_feed`'s player and target are the fixture.
        UiFixture::Bare => {}
        // Holds the world-entry loading screen, tip and all, over the settled scene.
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
            // Eight rows: long names for the wrap, mixed prices for the coins. The last two
            // columns are required level and stock: boots and gloves are gated past the level-12
            // player so they render the unusable red, and the sold-out Small Shield the 0.5 gray.
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
            // The usable gate's player half, by hand: with no SelfPlayer in capture,
            // `feed_player_req` never runs to overwrite it. A level-12 human warrior.
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
            // A greeting long enough that the wrapped options below run past the parchment.
            gossip.greeting = Some(
                "A man has been caught stealing corn from the fields of a noble, a lord known \
                 for his harsh taxes throughout the land.$B$BMake your choice!"
                    .into(),
            );
            // An available quest row above the options, for the quest-row icon and text seating.
            gossip.quests = vec![(783, 0, 5, "Eagan Peltskinner".into())];
            // One short option and four that wrap, taller than the parchment, so the capture covers
            // the stock `GossipResize` per-row height, the scroll frame and its scrollbar.
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
            // A pure banker and the vault, fed through the descriptor and caches that `feed_bank`
            // and `ui_items` push into the VM as live wire data would.
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
            // Vault items (bank slots 0, 1, 2, 7 at fields 564+2i) and the bank bag in bag slot 0
            // (field 612); the bag borrows the hearthstone display as a stand-in.
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
            // The bank bag is a container object (its contents stream on the bag item): 6 slots,
            // one occupied, so its popout window (container 5) is in the shot too.
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
            // The self player: 4 vault slots, the bank bag, two bought bag slots (`PLAYER_BYTES_2`
            // byte 2: button 1 owned, 2 bought and empty, 3-6 unpurchased red) and a 12g 34s 56c
            // purse, under which the 10g third-slot cost (`BankBagSlotPrices.dbc`) is affordable.
            const PLAYER_GUID: u64 = 0x51;
            let fields = ObjectFields::from_pairs(&[
                (194, 2 << 16),          // PLAYER_BYTES_2: bankBagSlots (byte 2) = 2
                (564, G_SHIELD as u32),  // BANK_SLOT_1 (vault slot 0)
                (566, G_JERKY as u32),   // vault slot 1
                (568, G_SWORD as u32),   // vault slot 2
                (578, G_STONE as u32),   // vault slot 7, after a gap
                (612, G_BANKBAG as u32), // BANK_BAG_SLOT_1: bag button 1's icon
                (1176, 123_456),         // PLAYER_FIELD_COINAGE
            ]);
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));
            // `feed_bank` then fires BANKFRAME_OPENED as a live SMSG_SHOW_BANK would.
            bank.open(NPC_GUID);
            // Opens bank bag 1 once its container feed lands (`GetContainerNumSlots(5) > 0`);
            // `BankFrameBag1`'s `GetID()` is 5, the container id its handler passes to `ToggleBag`.
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
            // A QUEST_DETAILS view (the accept panel): description, objectives, two choice rewards,
            // one fixed reward and money.
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
            // The multi-quest greeting: two available quests, bullet rows under "Available Quests".
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
            // The log reads the self player's PLAYER_QUEST_LOG slots, so a synthetic self player
            // carries two and `feed_quest_log` runs the real chain. A slot is id, packed 6-bit
            // counters plus a state byte, and timer, from field 198.
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
            // The item objective and reward rows; log icons come from the template's
            // `display_info_id`.
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
            // Entry 1, auto-selected: a creature objective at 3/10, an item objective at 0/5,
            // rewards and money. `quest_type: 1` is `QuestInfo.dbc`'s "Elite", though the real 783
            // is plain, so a row tag is in the shot.
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
            // Entry 2: complete, for the row's "(Complete)" tag.
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

            // Slot 0 is quest 783 (counter 0 at 3), slot 1 quest 7 (counter 0 at 10, state
            // COMPLETE), packed as vmangos's `SetQuestSlotCounter` and `SetQuestSlotState` write.
            let fields = ObjectFields::from_pairs(&[
                (198, 783),
                (199, 3),
                (200, 0),
                (201, 7),
                (202, 10 | (0x01 << 24)),
                (203, 0),
            ]);
            // A guid and cached name, as the feeds resolve the player through the self query.
            const PLAYER_GUID: u64 = 0x51;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));

            // The L binding's path; the feed pushes over the settle window.
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
                4, // 4 copper, the coin row
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
            // The pet bar's autocast shine, raised by hand, as no pet is fed.
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
            // At the default origin the player is off every zone rect and the arrow hides, so park
            // it at the scenario's Northshire spot.
            player.pos = benilla_assets::coords::wow_to_bevy(scenario.eye);
            // Alternating explore bits, so about half of each zone's overlays reveal.
            script.set_world_map_explored(vec![0x5555_5555; 64]);
            // The Elwynn zone map: continent 2, zone 10 in the alphabetical zone list.
            if let Err(e) = script.run("ToggleWorldMap(); SetMapZoom(2, 10)") {
                warn!("capture: ui-worldmap seed failed to open the map: {e}");
            }
        }
        UiFixture::PartyInvite => {
            let Some(script) = script else {
                return;
            };
            // Raised through the real `StaticPopupDialogs` entry, as a real invite is.
            if let Err(e) = script.run(r#"StaticPopup_Show("PARTY_INVITE", "Brisca")"#) {
                warn!("capture: ui-partyinvite seed failed to raise the dialog: {e}");
            }
        }
        UiFixture::Tooltip => {
            let Some(mut script) = script else {
                return;
            };
            seed_bag_window(&mut script, icons.as_deref());
            // The hovered shield's full template and a player under its level requirement: quality
            // name, bind, slot and type, armor and block, stats, durability, a red requirement, a
            // long green Use: line that wraps, a charges line after it, and the flavor text.
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
            // The item-set block (`0x52b650`), Defias Leather with one of 5 equipped: the gold
            // "(1/5)" header, the member ladder, a green (2) bonus and a gray (4) one.
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
            // Opens the tooltip over the top-left button (`Item16`, game slot 1, the Small Shield)
            // through the hover's OnEnter path. `IsBagOpen` names the backpack's window, as the
            // twelve `ContainerFrame`s are recycled across containers.
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
            // A PvP-flagged friendly guard under the cursor, pushed as the mouseover feed does,
            // then the call `drive_mouseover_tooltip` makes. The subject is the placement, the
            // `GameTooltip_SetDefaultAnchor` corner (`GameTooltip.lua:73-77`).
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
                    // The faction-name line, white, between level and PvP.
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
            // A synthetic self player carrying the full stat block, which the `ui_char` feed turns
            // into snapshots and events as live. A level-12 warrior; positive (stamina, fire) and
            // negative (spirit) buffs exercise the green and red stat colours.
            use benilla_protocol::messages::ObjectFields;
            const PLAYER_GUID: u64 = 0x51;
            // Equipped item guids (chest, main hand, ranged) and an arrow stack in the backpack.
            const G_CHEST: u64 = 0x1001;
            const G_SWORD: u64 = 0x1002;
            const G_BOW: u64 = 0x1003;
            const G_ARROWS: u64 = 0x1004;

            let fields = ObjectFields::from_pairs(&[
                (34, 12),                 // UNIT_FIELD_LEVEL
                (36, 4 | 1 << 8),         // UNIT_FIELD_BYTES_0: night elf warrior, male, mana
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
                // Stat buffs (INT on the wire): +10 stamina, -5 spirit, +10 fire resistance. The
                // -5 is the two's-complement word an x86 server sends; an arm64 one sends 0.
                (1179, 10),             // POSSTAT2
                (1186, (-5i32) as u32), // NEGSTAT4
                (1189, 10),             // RESISTANCEBUFFMODSPOSITIVE[2] (fire)
                // Equipment guids (INV_SLOT_HEAD base 486 + 2·slot): chest 4, main hand 15,
                // ranged 17; the first backpack slot (PACK_SLOT_1, 532) holds the arrows.
                (494, G_CHEST as u32),
                (516, G_SWORD as u32),
                (520, G_BOW as u32),
                (532, G_ARROWS as u32),
                (1223, 93_012), // PLAYER_AMMO_ID: the arrows' entry
            ]);

            // The item objects and their templates; the bow borrows the hearthstone display as a
            // stand-in.
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
            sword.class = 2; // weapon: 1h sword, skill 43 (the Attack row's skill line)
            sword.subclass = 7;
            sword.display_info_id = DISP_SWORD;
            sword.inventory_type = 21;
            sword.dmg_min = 13.0;
            sword.dmg_max = 19.0;
            sword.delay_ms = 2400;
            items.insert_template(93_011, Some(sword));
            let mut bow = template("Cracked Shortbow", 1);
            bow.class = 2; // weapon: bow, skill 45 (the ranged block's skill line)
            bow.subclass = 2;
            bow.display_info_id = DISP_STONE;
            bow.inventory_type = 15;
            items.insert_template(93_013, Some(bow));
            let mut arrows = template("Rough Arrow", 0);
            arrows.class = 6; // projectile: the ammo slot's icon and bag-summed count (200)
            arrows.inventory_type = 24; // INVTYPE_AMMO: the equip drains' SET_AMMO fork
            arrows.display_info_id = DISP_STONE;
            items.insert_template(93_012, Some(arrows));

            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(fields),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));

            // The C binding's path; the feed pushes over the settle window.
            if let Some(s) = script.as_mut() {
                if let Err(e) = s.run("ToggleCharacter(\"PaperDollFrame\")") {
                    warn!("capture: ui-char seed failed to open the window: {e}");
                }
            }
        }
        UiFixture::VPlates => {
            use benilla_protocol::messages::ObjectFields;
            // The self player at the camera eye (the 20 yd plate gate measures from here): a
            // level-2 human, so the wolf cons yellow as in the reference screenshot.
            const PLAYER_GUID: u64 = 0x51;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 2),      // UNIT_FIELD_LEVEL
                    (35, 1),      // UNIT_FIELD_FACTIONTEMPLATE: human
                    (36, 0x0101), // UNIT_FIELD_BYTES_0: race human, class warrior
                    // UNIT_FIELD_FLAGS bit 3 (`PLAYER_CONTROLLED`), which every player carries;
                    // `CanAttack` picks its arm on it, and without it no plate draws on the wolf.
                    (46, 0x8),
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
                Transform::from_translation(wow_to_bevy(scenario.eye)),
            ));
            // The wolf: the live spawn's component set, at the look point facing the camera.
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
            // The wolf is the target, so its plate draws lit with the target ring.
            selection.target = Some(wolf);
            selection.guid = Some(WOLF_GUID);
        }
        UiFixture::Options => {
            let Some(script) = script else {
                return;
            };
            // Opened through the live panel path, with Controls selected by its OnShow.
            if let Err(e) = script.run("ShowUIPanel(BenillaOptionsFrame)") {
                warn!("capture: ui-options seed failed to open the window: {e}");
            }
        }
        UiFixture::OptionsAudio => {
            let Some(script) = script else {
                return;
            };
            // The Audio page: the real CVar set first, so the rows read real values, then the
            // live open and select paths.
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
            // The Graphics page, as the Audio fixture.
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
            // The Chat page, as the Audio fixture. Its Remove Chat Hover Delay row reads a saved
            // variable `ChatFrame.xml` declares, so it paints unchecked, the shipped "0".
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
            // An addon's usual opening: set the colour, ask for opacity, show the window. The
            // colour puts both markers off their defaults, so a mirrored axis shows.
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
            // The Camera Following Style list open. Its width lands inside the opening click:
            // `UIDropDownMenu_Refresh` sizes buttons from `normalText:GetWidth() + 60`
            // (`UIDropDownMenu.lua:405`), which the engine answers in the same call.
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
            // The Keybindings page: the real command registry and CVar set first, then the live
            // open path, with Movement expanded to show a header row and the default bindings.
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
            // Mid-search: "volume" gathers the four volume sliders under the Audio head. The box
            // is focused, so the caret shows at the text's end.
            script.register_cvars(crate::cvars::registered_pairs());
            if let Err(e) = script.run(
                "ShowUIPanel(BenillaOptionsFrame); BenillaOptionsFrameSearchBox:SetText(\"volume\"); BenillaOptionsFrameSearchBox:SetFocus()",
            )
            {
                warn!("capture: ui-options-search seed failed: {e}");
            }
        }
        UiFixture::SpellBook => {
            // A human warrior who also knows two cross-class spells. The book resolves through
            // the real chain: names, icons and ranks from `Spell.dbc`, tabs from
            // `SkillLineAbility.dbc`, the General collapse from `SkillRaceClassInfo.dbc` keyed on
            // the descriptor's race and class.
            use benilla_protocol::messages::ObjectFields;
            const PLAYER_GUID: u64 = 0x51;
            const G_SWORD: u64 = 0x1002;
            names.insert_player(PLAYER_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 12),              // UNIT_FIELD_LEVEL
                    (36, 1 | 1 << 8),      // UNIT_FIELD_BYTES_0: human (race 1), warrior (class 1)
                    (516, G_SWORD as u32), // main-hand item guid (INV_SLOT_HEAD 486 + 15·2)
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(PLAYER_GUID),
            ));
            // The equipped main-hand weapon, whose icon the auto-attack borrows.
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
                // The auto-attack shows the equipped sword's icon, not the placeholder of spell 6603,
                // in General (no skill line).
                6603, // Attack
                // Warrior abilities go to their class-line tabs: Arms and Fury.
                100,  // Charge (Arms)
                78,   // Heroic Strike (Arms)
                772,  // Rend (Arms)
                6673, // Battle Shout (Fury)
                // A human racial collapses to General.
                20600, // Perception
                // Cross-class spells: no Fire or Shadow `SkillRaceClassInfo` row admits a warrior,
                // so both collapse into General and no Fire or Shadow tab appears.
                133, // Fireball (Fire)
                589, // Shadow Word: Pain (Shadow)
                // A language and an armor proficiency, both DO_NOT_DISPLAY, so neither appears.
                668,  // Language: Common
                9078, // Cloth proficiency
            ]);
            // The P binding's path; the feed pushes over the settle window.
            if let Err(e) = script.run("ToggleSpellBook(BOOKTYPE_SPELL)") {
                warn!("capture: ui-spellbook seed failed to open the book: {e}");
            }
        }
        UiFixture::Macro | UiFixture::MacroPopup => {
            let Some(script) = script.as_mut() else {
                return;
            };
            // Made through the live `CreateMacro` path, so `UPDATE_MACROS` reaches the window as a
            // player's edit does; icons index `SpellIcon.dbc`. The bodies: a plain `/cast`, a
            // multi-line macro, and a `/script` line the bar's bound-spell resolve must decline.
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
                    // Lua-escape the body's quotes and newlines.
                    body.replace('"', "\\\"").replace('\n', "\\n")
                ));
            }
            // Slot 2, the multi-line body, selected. The popup fixture opens the chooser through
            // `MacroEditButton`, an edit, so the name box arrives filled.
            seed.push_str("ShowMacroFrame()\nMacroButton2:Click()\n");
            if fixture == UiFixture::MacroPopup {
                seed.push_str("MacroEditButton:Click()\n");
            }
            if let Err(e) = script.run(&seed) {
                warn!("capture: ui-macro seed failed: {e}");
            }
        }
        UiFixture::Social => {
            // Nothing to seed: an empty friends list opens the same frames a full one does.
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
            // Say and yell lines behind the open edit box, opened through the live path;
            // `chat_edit_live` then drives the header and the `15 + headerWidth` insets as in-game.
            for (text, r, g, b) in [
                ("[One] says: testing the box", 1.0, 1.0, 1.0),
                ("[One] says: a second line to stack", 1.0, 1.0, 1.0),
                ("[One] yells: FUU", 1.0, 64.0 / 255.0, 64.0 / 255.0),
            ] {
                script.add_chat_message("ChatFrame1", text, r, g, b);
            }
            script.focus_editbox("ChatFrameEditBox");
            // A draft with "northshire" selected: the gray selection highlight (`0xFF606060`)
            // under the glyphs and the caret at its end. The tab and window tint follow the
            // cursor in `FCF_OnUpdate`, so a fixed-reveal OnUpdate pins them; it calls
            // `PanelTemplates_TabResize` every frame until the label's measure lands.
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
            // Lines for content, the Combat Log selected, and the cursor on the General tab: an
            // unselected tab's hover. A capture can hover, as the OS cursor only reaches the VM
            // on a non-synthetic run, so a fed `mouse_move` stands.
            for (text, r, g, b) in [
                ("[One] says: hello northshire", 1.0, 1.0, 1.0),
                ("[One] says: a second line to stack", 1.0, 1.0, 1.0),
            ] {
                script.add_chat_message("ChatFrame1", text, r, g, b);
            }
            // `$WOW_TABHOVER` picks one of five dock states for an A/B:
            //   1 (default)  Combat Log selected, hovering General
            //   3            General selected, hovering Combat Log
            //   2            Combat Log selected, hovering Combat Log (the selected tab)
            //   0            revealed, hovering neither tab (no glow)
            //   9            cursor off the dock (bare scene, the baseline)
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
                // 9: park far away, so the dock conceals itself.
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
            // The subject is an NPC's overhead name, and `UnitNameNPC` defaults to "0"; set
            // through Lua as a player would, so the sync drains it into `NameConfig`.
            if let Some(script) = script.as_deref_mut() {
                if let Err(e) = script.run("SetCVar(\"UnitNameNPC\", \"1\")") {
                    warn!("capture: name fixture could not enable UnitNameNPC: {e}");
                }
            }
            // The self player at the eye; the name colour is its reaction to the wolf.
            const SELF_GUID: u64 = 0x51;
            names.insert_player(SELF_GUID, "Benilla".into(), None);
            commands.spawn((
                crate::net::ObjectStore(ObjectFields::from_pairs(&[
                    (34, 2),      // UNIT_FIELD_LEVEL
                    (35, 1),      // UNIT_FIELD_FACTIONTEMPLATE: human
                    (36, 0x0101), // UNIT_FIELD_BYTES_0: race human, class warrior
                ])),
                crate::net::SelfPlayer,
                crate::net::Guid(SELF_GUID),
                Transform::from_translation(wow_to_bevy(scenario.eye)),
            ));
            // The wolf out in the river, its name over the water beyond it, which catches a name
            // sorting before the liquid.
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
            // A plated unit draws no floating name, so enemy plates go off.
            vplates.enemies = false;
        }
        // The lighting matrix: one spawn with a streamed entity's component set, anonymous and
        // with plates off, so no glyph rides over the body.
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

/// The `GetTime()` the cooldown filmstrip pins the VM's session clock at, seconds. Large, as
/// `CooldownFrame_SetTimer` ignores a start not above 0 (`Cooldown.lua:2`) and the starts here
/// are `now - fraction * span`.
const COOLDOWN_NOW_S: f64 = 100_000.0;

/// Each filmstrip slot's cooldown, seconds; long enough that the settle window barely moves it.
const COOLDOWN_SPAN_S: f64 = 10_000.0;

/// The cooldown sweep as a filmstrip: sixteen backpack slots, each at its own fraction of one long
/// cooldown, in reading order since game slot 1 renders top-left.
fn seed_cooldown_filmstrip(
    script: &mut benilla_ui::script::UiScript,
    icons: Option<&crate::entities::ItemDisplays>,
) {
    // The clock first: `set_container` stores each triple against it, and
    // `GetContainerItemCooldown`'s expiry guard reads it.
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
        // Sixteen phases, off both ends (0 is the uniform disc, 1 the flash).
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

/// Seeds and opens the backpack with a fixed item set, for the `Bag` and `Tooltip` fixtures. With
/// no `SelfPlayer` in capture the `ui_items` feed leaves bag 0 alone.
fn seed_bag_window(
    script: &mut benilla_ui::script::UiScript,
    icons: Option<&crate::entities::ItemDisplays>,
) {
    // Display ids the offline icon catalog resolves.
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
    // Five items under their real template ids. Game slot 1, the Small Shield the Tooltip
    // fixture hovers, renders top-left: `ContainerFrame_GenerateFrame` numbers backwards
    // (`index = size - j + 1`) from `Item1` at the bottom right (`ContainerFrame.lua:426-442`),
    // so its button is `Item16`, where the ANCHOR_RIGHT tooltip stays on-screen.
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
    // `OpenBag` builds and paints the window in one call (`ContainerFrame_GenerateFrame`). Not
    // `OpenBackpack()`, which `ContainerFrameAdapters.xml` makes open every equipped bag, and only
    // the backpack is fed.
    if let Err(e) = script.run("OpenBag(0)") {
        warn!("capture: bag window seed failed: {e}");
    }

    // Chat lines in ChatFrame1 for the line shadow and `ChatFontNormal`: SYSTEM yellow, SAY white,
    // LOOT green. In the loot lines the item's quality escape wins for its bracketed name, and the
    // `x2` after `|r` falls back to the line's green.
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

/// Seeds and opens three equipped bags of different sizes, for the background fit
/// `ContainerFrame_GenerateFrame` does: 6 slots (a partial top row), 8 (full) and 10 (three rows,
/// partial top). They stack up and left from the backpack as in-game.
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

    // The equipped-bag slots (inv ids 20-23, `Bag0Slot`-`Bag3Slot`) get an icon each, so each
    // window's portrait shows its own bag through `SetBagPortaitTexture`; live, the char feed
    // does this.
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
            // Every slot filled, so the slot alignment shows on all six.
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
        // After `set_container`: `OpenBag` refuses a container with no slots.
        if let Err(e) = script.run(&format!("OpenBag({bag_id})")) {
            warn!("capture: equipped bag {bag_id} seed failed: {e}");
        }
    }
}
