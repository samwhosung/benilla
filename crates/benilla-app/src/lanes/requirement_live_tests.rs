//! What an item is judged against reads the active player as the call runs, through the lent host:
//! the requirement state the red lines and the usable gate compare (`0x5eaae0`, `0x5ea930`), the
//! known-spell list (`0x60c740`) and the lock walk's openers (`0x5da5e0`), where the VM's copy holds
//! what the frame's push left. The item build's lookups ask their caches inside the build, as each
//! passes its callback there (`0x52e204`, `0x52b93b`).

use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    ItemTemplateView, LockOpeners, LockSlotView, PlayerReqState, TaughtSpell, UiScript,
};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// A key we carry, and the lockbox in our backpack its lock names.
const KEY: u64 = 0x4000_0000_0000_0101;
const KEY_ENTRY: u32 = 5060;
const LOCKBOX: u64 = 0x4000_0000_0000_0102;
const LOCKBOX_ENTRY: u32 = 4632;
const LOCK_ID: u32 = 7;
/// A recipe, the craft it teaches, and an item with a required level.
const RECIPE_ENTRY: u32 = 2406;
/// The recipe's use spell, a learn (`Effect[0]` 36) whose trigger spell is the craft.
const LEARN: u32 = 2540;
const TAUGHT: u32 = 2541;
const AXE_ENTRY: u32 = 1311;
/// A crafted item whose maker the name cache lacks, and a charter whose record it lacks.
const CRAFTED: u64 = 0x4000_0000_0000_0103;
const CRAFTED_ENTRY: u32 = 2575;
const SMITH: u64 = 0x55;
const CHARTER: u64 = 0x4000_0000_0000_0104;
const CHARTER_ENTRY: u32 = 5863;
const PETITION_ID: u32 = 77;

/// `OBJECT_FIELD_TYPE`, `OBJECT_FIELD_ENTRY`, `ITEM_FIELD_OWNER`, `ITEM_FIELD_CREATOR`,
/// `ITEM_FIELD_STACK_COUNT`, `ITEM_FIELD_ENCHANTMENT` slot 0, `UNIT_FIELD_LEVEL`,
/// `UNIT_FIELD_BYTES_0` and `PLAYER_FIELD_PACK_SLOT_1`.
const OBJECT_TYPE: u16 = 2;
const OBJECT_ENTRY: u16 = 3;
const ITEM_OWNER: u16 = 6;
const ITEM_CREATOR: u16 = 10;
const ITEM_STACK_COUNT: u16 = 14;
const ITEM_ENCHANT_0: u16 = 22;
const UNIT_LEVEL: u16 = 34;
const UNIT_BYTES_0: u16 = 36;
const PACK_SLOT_1: u16 = 532;

/// An item object of ours.
fn item(entry: u32, extra: &[(u16, u32)]) -> ObjectFields {
    let mut pairs = vec![
        (OBJECT_TYPE, 0x3),
        (OBJECT_ENTRY, entry),
        (ITEM_OWNER, ME as u32),
        (ITEM_STACK_COUNT, 1),
    ];
    pairs.extend_from_slice(extra);
    ObjectFields::from_pairs(&pairs)
}

/// The client with us in the world, a level-10 human warrior carrying the lockbox, the key, the
/// crafted bar and the charter in backpack slots 1 to 4, their records, the recipe's and the axe's
/// cached, the VM's copy pushed as a level-60 player who knows nothing and opens nothing, and the
/// VM's item records and the lock row seated.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let world = app.world_mut();
    crate::script_calls::insert_test_vm(world);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let me = crate::items::test_spawn_object(
        world,
        ME,
        ObjectFields::from_pairs(&[
            (OBJECT_TYPE, 0x19),
            (UNIT_LEVEL, 10),
            (UNIT_BYTES_0, 1 | (1 << 8)),
            (PACK_SLOT_1, LOCKBOX as u32),
            (PACK_SLOT_1 + 1, (LOCKBOX >> 32) as u32),
            (PACK_SLOT_1 + 2, KEY as u32),
            (PACK_SLOT_1 + 3, (KEY >> 32) as u32),
            (PACK_SLOT_1 + 4, CRAFTED as u32),
            (PACK_SLOT_1 + 5, (CRAFTED >> 32) as u32),
            (PACK_SLOT_1 + 6, CHARTER as u32),
            (PACK_SLOT_1 + 7, (CHARTER >> 32) as u32),
        ]),
    );
    world.entity_mut(me).insert(crate::net::SelfPlayer);
    crate::items::test_spawn_item(world, LOCKBOX, item(LOCKBOX_ENTRY, &[]), false);
    crate::items::test_spawn_item(world, KEY, item(KEY_ENTRY, &[]), false);
    crate::items::test_spawn_item(
        world,
        CRAFTED,
        item(CRAFTED_ENTRY, &[(ITEM_CREATOR, SMITH as u32)]),
        false,
    );
    crate::items::test_spawn_item(
        world,
        CHARTER,
        item(CHARTER_ENTRY, &[(ITEM_ENCHANT_0, PETITION_ID)]),
        false,
    );
    // The recipe's learn spell, which its record's view names the craft through.
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog =
        benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            LEARN,
            benilla_formats::SpellDisplay {
                id: LEARN,
                effects: [36, 0, 0],
                effect_trigger_spell: [TAUGHT, 0, 0],
                ..Default::default()
            },
        )]));
    world.insert_resource(spells);
    // The records the item cache holds, which the bags and a build with the host lent read.
    let mut items = world.resource_mut::<crate::items::Items>();
    let record = crate::items::test_template;
    for (entry, t) in [
        (
            LOCKBOX_ENTRY,
            benilla_protocol::messages::ItemInfo {
                lock_id: LOCK_ID,
                ..record("Lockbox")
            },
        ),
        (KEY_ENTRY, record("Key")),
        (CRAFTED_ENTRY, record("Bar")),
        (
            CHARTER_ENTRY,
            benilla_protocol::messages::ItemInfo {
                flags: benilla_protocol::messages::ITEM_FLAG_CHARTER,
                ..record("Charter")
            },
        ),
        (
            RECIPE_ENTRY,
            benilla_protocol::messages::ItemInfo {
                spells: vec![benilla_protocol::messages::ItemSpellEntry {
                    index: 0,
                    spell_id: LEARN,
                    trigger: 0,
                    charges: 0,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }],
                ..record("Recipe")
            },
        ),
        (
            AXE_ENTRY,
            benilla_protocol::messages::ItemInfo {
                required_level: 20,
                ..record("Axe")
            },
        ),
    ] {
        items.insert_template(entry, Some(t));
    }
    let mut script = world.non_send_resource_mut::<UiScript>();
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script
        .run(
            r#"LOCKED = "Locked"; ITEM_MIN_LEVEL = "Requires Level %d"
            ITEM_SPELL_KNOWN = "Already known"; ITEM_CREATED_BY = "<Made by %s>"
            local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
            a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            -- The colour of the line that reads `text`, nil with none.
            function LineColor(text)
                for i = 1, TT:NumLines() do
                    local line = getglobal("TTTextLeft" .. i)
                    if line:GetText() == text then return line:GetTextColor() end
                end
            end
            function HasLine(text) return LineColor(text) ~= nil end"#,
        )
        .unwrap();
    script.set_screen_size(800.0, 600.0);
    script.set_player_req_state(PlayerReqState {
        level: 60,
        class_id: 1,
        race_id: 1,
        ..Default::default()
    });
    script.set_lock_openers(LockOpeners::default());
    script.set_lock_rows(std::collections::HashMap::from([(
        LOCK_ID,
        vec![LockSlotView {
            key_type: 1,
            index: KEY_ENTRY,
            skill: 0,
        }],
    )]));
    let view = |name: &str| ItemTemplateView {
        name: name.into(),
        quality: 1,
        ..Default::default()
    };
    script.set_item_template(
        LOCKBOX_ENTRY,
        ItemTemplateView {
            lock_id: LOCK_ID,
            ..view("Lockbox")
        },
    );
    script.set_item_template(KEY_ENTRY, view("Key"));
    script.set_item_template(
        RECIPE_ENTRY,
        ItemTemplateView {
            teaches: Some(TaughtSpell {
                ranks: vec![TAUGHT],
                pet: false,
            }),
            ..view("Recipe")
        },
    );
    script.set_item_template(
        AXE_ENTRY,
        ItemTemplateView {
            required_level: 20,
            ..view("Axe")
        },
    );
    script.set_item_template(CRAFTED_ENTRY, view("Bar"));
    script.set_item_template(
        CHARTER_ENTRY,
        ItemTemplateView {
            flags: 0x2000,
            ..view("Charter")
        },
    );
    (app, rx)
}

fn hosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval(chunk).expect("runs")
    })
}

fn unhosted<T: benilla_ui::mlua::FromLuaMulti>(app: &App, chunk: &str) -> T {
    crate::script_calls::eval_in_vm(app.world(), chunk)
}

/// Our level as the call runs: the green range and the `ITEM_MIN_LEVEL` line's colour read the
/// level-10 player the world holds, where the VM's copy says 60.
#[test]
fn the_requirement_getters_read_our_level_as_the_call_runs() {
    let (mut app, _rx) = client();
    let range: i64 = hosted(&mut app, "return GetQuestGreenRange()");
    assert_eq!(range, 5, "level 10's band (`0x8076c0`)");
    let pushed: i64 = unhosted(&app, "return GetQuestGreenRange()");
    assert_eq!(pushed, 12, "the copy's level 60");
    const AXE: &str = "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetHyperlink('item:1311:0:0:0') \
                       return LineColor('Requires Level 20')";
    let (r, g, _): (f32, f32, f32) = hosted(&mut app, AXE);
    assert!(r > g, "red under our level 10: {r} {g}");
    let (r, g, _): (f32, f32, f32) = unhosted(&app, AXE);
    assert!(r > 0.99 && g > 0.99, "white under the copy's 60: {r} {g}");
}

/// The known-spell list as the call runs: a recipe whose craft we learnt reads known, its
/// `ITEM_SPELL_KNOWN` line printed (`0x52d54f`), where the copy knows nothing.
#[test]
fn a_recipe_reads_the_known_spell_list_as_the_call_runs() {
    let (mut app, _rx) = client();
    const RECIPE: &str = "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetHyperlink('item:2406:0:0:0') \
                          return HasLine('Already known')";
    assert!(!hosted::<bool>(&mut app, RECIPE), "not learnt yet");
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(TAUGHT);
    assert!(hosted::<bool>(&mut app, RECIPE), "learnt: known at once");
    assert!(!unhosted::<bool>(&app, RECIPE), "the copy knows nothing");
}

/// The lock walk's openers as the call runs: the key we carry opens the lockbox's lock, `LOCKED`
/// in green (`0x52be1c`); with the key gone the lock reads closed.
#[test]
fn a_locks_key_is_found_in_our_bags_as_the_call_runs() {
    let (mut app, _rx) = client();
    const LOCKED: &str = "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1) \
                          return LineColor('Locked')";
    let (r, g, _): (f32, f32, f32) = hosted(&mut app, LOCKED);
    assert!(g > r, "the key we carry opens it: {r} {g}");
    let world = app.world_mut();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world
        .get_mut::<crate::net::ObjectStore>(me)
        .expect("our store")
        .0
        .merge(ObjectFields::from_pairs(&[
            (PACK_SLOT_1 + 2, 0),
            (PACK_SLOT_1 + 3, 0),
        ]));
    let (r, g, _): (f32, f32, f32) = hosted(&mut app, LOCKED);
    assert!(r > g, "the key gone, the lock is closed: {r} {g}");
}

/// The build's lookups go out inside the build that misses them: the maker's name
/// (`CMSG_NAME_QUERY`, `0x52e204`) and the charter's record by the petition id the object carries
/// (`CMSG_PETITION_QUERY`, `0x52b93b`, `0x5ef337`), with no frame between.
#[test]
fn the_item_builds_lookups_are_asked_inside_the_build() {
    let (mut app, rx) = client();
    let _: () = hosted(
        &mut app,
        "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 3)",
    );
    let asked = crate::query_cache::queries(&rx);
    assert!(
        matches!(asked[..], [ClientCommand::NameQuery { guid: SMITH }]),
        "the maker's name is asked in the call: {asked:?}"
    );
    let _: () = hosted(
        &mut app,
        "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 4)",
    );
    let sent: Vec<ClientCommand> = rx.try_iter().collect();
    assert!(
        sent.iter().any(|c| matches!(
            c,
            ClientCommand::PetitionQuery {
                petition_id: PETITION_ID,
                item: CHARTER
            }
        )),
        "the charter's record is asked in the call: {sent:?}"
    );
}

/// A set of two members, the first named in the cache, with a bonus at two.
const SET: u32 = 161;
const MARK: u32 = 6303;
const BELT: u32 = 6304;
const SPEED: u32 = 9000;
const SET_HOVER: &str = "TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetHyperlink('item:6303:0:0:0')";

/// The client with the set table, the first member's name and the bonus's spell seated.
fn set_client() -> (App, Receiver<ClientCommand>) {
    let (mut app, rx) = client();
    let world = app.world_mut();
    world.insert_resource(crate::ui_items::ItemSets(
        benilla_formats::ItemSetCatalog::from_rows([(
            SET,
            benilla_formats::ItemSetInfo {
                name: "Defias Leather".into(),
                items: vec![MARK, BELT],
                bonuses: vec![(2, SPEED)],
                required_skill: 0,
                required_skill_rank: 0,
            },
        )]),
    ));
    world
        .resource_mut::<crate::item_names::ItemNames>()
        .file(MARK, Some("Defias Mark".into()));
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog =
        benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            SPEED,
            benilla_formats::SpellDisplay {
                description: Some("Increases speed by $s1%.".into()),
                effect_base_points: [9, 0, 0],
                effect_base_dice: [1, 0, 0],
                effect_die_sides: [1, 0, 0],
                ..Default::default()
            },
        )]));
    world.insert_resource(spells);
    world.resource_mut::<crate::items::Items>().insert_template(
        MARK,
        Some(benilla_protocol::messages::ItemInfo {
            item_set: SET,
            ..crate::items::test_template("Defias Mark")
        }),
    );
    let mut script = world.non_send_resource_mut::<UiScript>();
    script.set_item_template(
        MARK,
        ItemTemplateView {
            name: "Defias Mark".into(),
            item_set: SET,
            ..Default::default()
        },
    );
    script
        .run(r#"ITEM_SET_NAME = "%s (%d/%d)"; ITEM_SET_BONUS_GRAY = "(%d) Set: %s""#)
        .unwrap();
    (app, rx)
}

/// The set block reads the set table, the item-name cache and the bonus's description as the call
/// runs (`0x52d8a0`), where the VM's copy holds no set: a named member prints its line, an unnamed
/// one is skipped and asked inside the build (`CMSG_ITEM_NAME_QUERY`, `0x52df4b`), and the bonus's
/// `$s1` template expanded against its spell (`0x5075f0`).
#[test]
fn an_item_sets_block_reads_the_set_table_and_the_names_as_the_call_runs() {
    let (mut app, rx) = set_client();
    let hover = format!(
        "{SET_HOVER} return HasLine('Defias Leather (0/2)'), HasLine('  Defias Mark'), \
         HasLine('(2) Set: Increases speed by 10%.')"
    );
    let live: (bool, bool, bool) = hosted(&mut app, &hover);
    assert_eq!(
        live,
        (true, true, true),
        "the block off the table and the cache"
    );
    assert!(
        crate::query_cache::queries(&rx)
            .iter()
            .any(|c| matches!(c, ClientCommand::ItemNameQuery { entry: BELT, .. })),
        "the unnamed member is asked inside the build"
    );
    let pushed: (bool, bool, bool) = unhosted(&app, &hover);
    assert_eq!(pushed, (false, false, false), "the VM's copy holds no set");
}

/// A member's name answer rebuilds the block with the host lent: the tick's cache answer
/// (`0x52e530`) runs the build again, which reads the set through the host and prints the line the
/// first build skipped.
#[test]
fn a_members_name_answer_rebuilds_the_set_block_with_the_host_lent() {
    let (mut app, _rx) = set_client();
    let _: () = hosted(&mut app, SET_HOVER);
    let belt = "return HasLine('  Defias Belt')";
    assert!(!unhosted::<bool>(&app, belt), "unnamed: skipped");
    app.world_mut()
        .resource_mut::<crate::item_names::ItemNames>()
        .file(BELT, Some("Defias Belt".into()));
    // The client's tick, which runs with the host lent.
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").tick(0.0));
    assert!(
        unhosted::<bool>(&app, belt),
        "the answer's rebuild prints it"
    );
    let block: bool = unhosted(&app, "return HasLine('Defias Leather (0/2)')");
    assert!(block, "the rebuild read the set through the host");
}

/// Two packets in one frame, the first at level 20 with the key in our pack, the second at level
/// 40 with it gone: each `UNIT_LEVEL` listener reads its own packet's level and bags, the green
/// range (`0x4e17d0`) and the lock walk's held items (`0x5da5e0`, `0x622270`), not the frame's
/// last.
#[test]
fn each_level_listener_reads_its_own_packets_level_and_bags() {
    use super::unit_view_tests::{created, values, wire_packets};
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::{EntityKind, SessionEvent};

    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, _sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let mut script = UiScript::new().expect("a VM");
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script
        .run(
            r#"LOCKED = "Locked"
            local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
            a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_LEVEL")
            f:SetScript("OnEvent", function()
                if arg1 ~= "player" then return end
                TT:SetOwner(Slot, "ANCHOR_RIGHT")
                TT:SetBagItem(0, 1)
                local lock = "none"
                for i = 1, TT:NumLines() do
                    local line = getglobal("TTTextLeft" .. i)
                    if line:GetText() == "Locked" then
                        local r, g = line:GetTextColor()
                        lock = g > r and "open" or "shut"
                    end
                end
                table.insert(Seen, UnitLevel("player") .. ":" .. GetQuestGreenRange() .. ":" .. lock)
            end)"#,
        )
        .unwrap();
    script.set_screen_size(800.0, 600.0);
    script.set_lock_rows(std::collections::HashMap::from([(
        LOCK_ID,
        vec![LockSlotView {
            key_type: 1,
            index: KEY_ENTRY,
            skill: 0,
        }],
    )]));
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        let mut lockbox = crate::items::test_template("Lockbox");
        lockbox.lock_id = LOCK_ID;
        items.insert_template(LOCKBOX_ENTRY, Some(lockbox));
        items.insert_template(KEY_ENTRY, Some(crate::items::test_template("Key")));
    }
    let item_create = |guid: u64, entry: u32| SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[(2, 0x3), (3, entry), (6, ME as u32), (8, ME as u32)])
            .into_created(ObjectType::Item),
    };
    let frame = |app: &mut App, packets: Vec<Vec<SessionEvent>>| {
        for ev in wire_packets(packets) {
            inbound.send(ev).expect("the inbound channel");
        }
        app.update();
    };
    // Us at level 10, the lockbox in backpack slot 1 and its key in slot 2.
    frame(
        &mut app,
        vec![
            vec![item_create(LOCKBOX, LOCKBOX_ENTRY)],
            vec![item_create(KEY, KEY_ENTRY)],
            vec![created(
                ME,
                EntityKind::Player,
                &[
                    (UNIT_LEVEL, 10),
                    (UNIT_BYTES_0, 1 | (1 << 8)),
                    (PACK_SLOT_1, LOCKBOX as u32),
                    (PACK_SLOT_1 + 1, (LOCKBOX >> 32) as u32),
                    (PACK_SLOT_1 + 2, KEY as u32),
                    (PACK_SLOT_1 + 3, (KEY >> 32) as u32),
                ],
            )],
        ],
    );
    frame(&mut app, vec![]);
    frame(&mut app, vec![]);
    crate::script_calls::eval_in_vm::<()>(app.world(), "Seen = {}");
    frame(
        &mut app,
        vec![
            vec![values(ME, &[(UNIT_LEVEL, 20)])],
            vec![values(
                ME,
                &[(UNIT_LEVEL, 40), (PACK_SLOT_1 + 2, 0), (PACK_SLOT_1 + 3, 0)],
            )],
        ],
    );
    let seen: Vec<String> = crate::script_calls::eval_in_vm(app.world(), "return Seen");
    assert_eq!(seen, ["20:6:open", "40:8:shut"]);
}
