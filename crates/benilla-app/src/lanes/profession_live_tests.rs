//! The profession windows open inside the cast that opens them. `TryCastSpell` (`0x6e4b60`) sends
//! nothing for a spell whose first effect is a profession's (`0x6e4bce`): it toggles the window
//! in place, `ToggleTradeSkill` (`0x4fc910` at `0x6e4c22`) or `ToggleCraftWindow` (`0x4f6000` at
//! `0x6e4c34`), whose build and `TRADE_SKILL_SHOW` or `CRAFT_SHOW` signal before the cast returns.
//! So a listener's cast shows the window between its packet's events and the next packet's, and a
//! chunk that opens and closes leaves the window shut. The book's builds and getters read the
//! player, the bags and the item cache as they stand at their call (`0x4fca20`, `0x4fdd40`,
//! `0x4ff7a2`), and the item answers run the lists' callbacks inside their handler
//! (`ItemCacheReceive` `0x55bdb0`: `0x4fdc30`, `0x4fc8e0`, `0x4f7710`). The craft window's do the
//! same (`0x4f60c0`, `0x4f6c10`, `0x4f6cb0`, `0x4f6fe6`, `0x4f74d0`, `0x4f78b0`, `0x4f7a90`).

use benilla_formats::{
    SlaInfo, SpellDisplay, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_TRADE_SKILL,
};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_INV_SLOT_HEAD};
use benilla_protocol::messages::{ObjectType, FIELD_PLAYER_SKILL_INFO_1_1};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};

use super::unit_view_tests::{client, created, fall, frame, values, wire_packets};
use crate::ui_craft::CraftOpen;
use crate::ui_tradeskill::{TradeSkillOpen, TradeSkillOpens};

/// [`client`]'s fall, as its spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";
const TAILORING: u32 = 197;
const ENCHANTING: u32 = 333;
/// The Tailoring opener, Bolt of Linen Cloth, the Enchanting opener, and a Minor Health enchant.
const TAILOR: u32 = 3908;
const BOLT: u32 = 2963;
/// Linen Bandage, a Tailoring-line recipe [`crafter`] knows of but has not learned.
const BANDAGE: u32 = 3275;
/// Linen Cloth, Bolt of Linen Cloth's reagent, not cached.
const LINEN: u32 = 2589;
/// [`client`]'s player.
const ME: u64 = 0x7;
const ENCHANT: u32 = 7411;
const MINOR_HEALTH: u32 = 7420;
/// Strange Dust, the enchant's reagent, not cached, and a Runed Copper Rod, its totem, cached.
const DUST: u32 = 10_940;
const ROD: u32 = 6_218;
/// Made-up labels: a Beast Training opener, its one row, the pet spell that row teaches, the pet
/// line that holds it, and a wolf for the pet bar.
const BEASTS: u32 = 90_001;
const BEAST_ROW: u32 = 90_002;
const PET_SPELL: u32 = 90_003;
const BEAST_TRAINING: u32 = 261;
const PET_LINE: u32 = 208;
const WOLF_FAMILY: u32 = 1;
const WOLF_ENTRY: u32 = 299;
const PET: u64 = 0xF140_0000_0000_0042;

/// [`client`] knowing the Tailoring and Enchanting openers and one recipe of each, Bolt of Linen
/// Cloth's product cached, and the windows' events added to its spy.
fn crafter() -> App {
    let mut app = client();
    let opener = |name: &str, misc: i32| SpellDisplay {
        name: name.into(),
        effects: [SPELL_EFFECT_TRADE_SKILL, 0, 0],
        effect_misc_value: [misc, 0, 0],
        ..Default::default()
    };
    let mut reagents = [(0, 0); 8];
    reagents[0] = (LINEN, 2);
    let bolt = SpellDisplay {
        name: "Bolt of Linen Cloth".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [2996, 0, 0],
        reagents,
        ..Default::default()
    };
    let bandage = SpellDisplay {
        name: "Linen Bandage".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        ..Default::default()
    };
    let mut dust = [(0, 0); 8];
    dust[0] = (DUST, 1);
    let health = SpellDisplay {
        name: "Enchant Chest - Minor Health".into(),
        cast_ui: 3,
        reagents: dust,
        totems: [ROD, 0],
        description: Some("Returns you to $z.".into()),
        ..Default::default()
    };
    // A Beast Training row: `LEARN_SPELL` aimed at the pet (`+0x148` 5).
    let beast_row = SpellDisplay {
        name: "Growl".into(),
        cast_ui: 1,
        effects: [benilla_formats::SPELL_EFFECT_LEARN_SPELL, 0, 0],
        effect_implicit_target_a: [5, 0, 0],
        effect_trigger_spell: [PET_SPELL, 0, 0],
        ..Default::default()
    };
    let pet_spell = SpellDisplay {
        name: "Growl".into(),
        spell_level: 4,
        ..Default::default()
    };
    let sla = |line| SlaInfo {
        skill_id: line,
        req_skill_value: 1,
        forward_spell_id: 0,
        trivial_low: 1,
        trivial_high: 2,
    };
    let world = app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [
                (TAILOR, opener("Tailoring", 0)),
                (BOLT, bolt),
                (BANDAGE, bandage),
                (ENCHANT, opener("Enchanting", 3)),
                (MINOR_HEALTH, health),
                (BEASTS, opener("Beast Training", 1)),
                (BEAST_ROW, beast_row),
                (PET_SPELL, pet_spell),
            ]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    world.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: benilla_formats::SkillLineCatalog::from_abilities([
            (TAILOR, sla(TAILORING)),
            (BOLT, sla(TAILORING)),
            (BANDAGE, sla(TAILORING)),
            (ENCHANT, sla(ENCHANTING)),
            (MINOR_HEALTH, sla(ENCHANTING)),
            (BEASTS, sla(BEAST_TRAINING)),
            (BEAST_ROW, sla(BEAST_TRAINING)),
        ])
        .with_line_names([(TAILORING, "Tailoring"), (ENCHANTING, "Enchanting")])
        .with_line_abilities([(
            PET_LINE,
            PET_SPELL,
            benilla_formats::LineAbility {
                superseded_by: 0,
                train_points: 5,
            },
        )]),
    });
    {
        let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
        actions
            .spells
            .extend([TAILOR, BOLT, ENCHANT, MINOR_HEALTH, BEASTS, BEAST_ROW]);
        actions.profession_order.push(BOLT);
        actions.craft_order.extend([MINOR_HEALTH, BEAST_ROW]);
    }
    let mut items = world.resource_mut::<crate::items::Items>();
    items.insert_template(
        2996,
        Some(crate::items::test_template("Bolt of Linen Cloth")),
    );
    items.insert_template(ROD, Some(crate::items::test_template("Runed Copper Rod")));
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TRADE_SKILL_SHOW", "TRADE_SKILL_UPDATE", "TRADE_SKILL_CLOSE",
                    "CRAFT_SHOW", "CRAFT_UPDATE", "CRAFT_CLOSE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    app
}

/// `chunk` with the host lent, and what the spy saw.
fn hosted(app: &mut App, chunk: &str) -> Vec<String> {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").run(chunk).expect("the chunk runs")
    });
    let s = app.world_mut().non_send_resource_mut::<UiScript>();
    let seen = s.eval::<Vec<String>>("return Seen").unwrap();
    s.run("Seen = {}").unwrap();
    seen
}

/// A fall's listener casts the Tailoring opener: the book builds and shows inside that cast, so
/// its `TRADE_SKILL_UPDATE` (`0x4fd164`) and `TRADE_SKILL_SHOW` (`0x4fc99f`) fire between the
/// first fall's event and the second's, and the line is open there.
#[test]
fn an_openers_show_fires_between_its_packets_neighbours() {
    let mut app = crafter();
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                if not Cast then Cast = true CastSpellByName("Tailoring") end
            end)
            "#,
        )
        .unwrap();
    assert_eq!(
        frame(&mut app, vec![fall(5), fall(6)]),
        [FELL, "TRADE_SKILL_UPDATE", "TRADE_SKILL_SHOW", FELL]
    );
    assert_eq!(
        app.world().resource::<TradeSkillOpen>().line,
        Some(TAILORING)
    );
}

/// A chunk that casts an opener and closes its window leaves both shut: each opener toggles and
/// signals in its cast (`0x4fc910`, `0x4f6000`), each close in its call (`0x4fc870`, `0x4f5fe0`),
/// so the engine, the VM and the opener queue agree once the chunk returns.
#[test]
fn a_chunk_that_opens_and_closes_leaves_no_window_open() {
    let mut app = crafter();
    assert_eq!(
        hosted(
            &mut app,
            r#"CastSpellByName("Tailoring") CloseTradeSkill()
            CastSpellByName("Enchanting") CloseCraft()"#
        ),
        [
            "TRADE_SKILL_UPDATE",
            "TRADE_SKILL_SHOW",
            "TRADE_SKILL_CLOSE",
            "CRAFT_UPDATE",
            "CRAFT_SHOW",
            "CRAFT_CLOSE",
        ]
    );
    let world = app.world();
    assert!(world.resource::<TradeSkillOpens>().0.is_empty());
    assert_eq!(world.resource::<TradeSkillOpen>().line, None);
    assert_eq!(world.resource::<CraftOpen>().line, None);
    let script = world.non_send_resource::<UiScript>();
    assert!(!script.trade_skill_open() && !script.craft_open());
}

/// The player's first skill slot holding Tailoring at `value` of 75 (`PLAYER_SKILL_INFO_1_1`).
fn tailoring(value: u32) -> SessionEvent {
    values(
        ME,
        &[
            (FIELD_PLAYER_SKILL_INFO_1_1, TAILORING),
            (FIELD_PLAYER_SKILL_INFO_1_1 + 1, (75 << 16) | value),
        ],
    )
}

/// `chunk` run with the host lent, its spy's record dropped.
fn quietly(app: &mut App, chunk: &str) {
    hosted(app, chunk);
}

/// A fall's listener reads `GetTradeSkillLine` at its call (`0x4fdd40` finds the slot at
/// `0x4fddb6` and reads its words): between a packet that raises the rank to 5 and one that raises
/// it to 6, it reads 5, its own place's rank, where the book's last build held another.
#[test]
fn the_line_getter_reads_the_rank_its_packet_left() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Tailoring")"#);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                local _, rank, max = GetTradeSkillLine()
                table.insert(Seen, "rank:" .. rank .. "/" .. max)
            end)
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![tailoring(5), fall(5), tailoring(6)]);
    let ranks: Vec<&String> = seen.iter().filter(|e| e.starts_with("rank:")).collect();
    assert_eq!(ranks, ["rank:5/75"], "{seen:?}");
}

/// A reagent's record answer runs the callback `GetTradeSkillReagentInfo`'s miss kept
/// (`0x4ff6cd` with `0x4fdc30`) inside its handler, whose rebuild signals `TRADE_SKILL_UPDATE`
/// (`0x4fdc3c` → `0x4fd164`) between the packets around it, and the getter then reads the record.
#[test]
fn a_reagents_record_answer_signals_the_update_at_its_packet() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Tailoring")"#);
    let bolt_row = r#"
        local row
        for i = 1, GetNumTradeSkills() do
            if GetTradeSkillInfo(i) == "Bolt of Linen Cloth" then row = i end
        end
    "#;
    quietly(
        &mut app,
        &format!("{bolt_row} Missed = (GetTradeSkillReagentInfo(row, 1)) == nil"),
    );
    assert!(app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<bool>("return Missed")
        .unwrap());
    let answer = SessionEvent::ItemTemplate {
        entry: LINEN,
        info: Some(Box::new(crate::items::test_template("Linen Cloth"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "TRADE_SKILL_UPDATE", FELL]
    );
    let name = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval::<String>(&format!(
            "{bolt_row} return (GetTradeSkillReagentInfo(row, 1))"
        ))
    })
    .unwrap();
    assert_eq!(name, "Linen Cloth");
}

/// A learn inside an open book rebuilds it with flag 1 at its packet (`0x5e9d79`), reading the
/// line's list as the learn left it (`0x5e9d72`) and banding the new recipe on the rank as it
/// stands there (`0x4fcc28`-`0x4fcc41`): with no Tailoring slot yet it reads `optimal`, though a
/// later packet of the frame raises the rank past its trivial mark.
#[test]
fn a_learn_inside_an_open_book_bands_its_recipe_at_its_packet() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Tailoring")"#);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("TRADE_SKILL_UPDATE")
            f:SetScript("OnEvent", function()
                for i = 1, GetNumTradeSkills() do
                    local name, kind = GetTradeSkillInfo(i)
                    if name == "Linen Bandage" then table.insert(Seen, "band:" .. kind) end
                end
            end)
            "#,
        )
        .unwrap();
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            SessionEvent::SpellLearned { spell_id: BANDAGE },
            tailoring(5),
            fall(6),
        ],
    );
    let bands: Vec<&String> = seen.iter().filter(|e| e.starts_with("band:")).collect();
    assert_eq!(
        bands.first().map(|b| b.as_str()),
        Some("band:optimal"),
        "{seen:?}"
    );
}

/// A build that finds no player (the lookup `0x468460` at `0x4fcb11`) has zeroed the full count
/// first (`0x4fca52`) and leaves past its signal (`0x4fcb1b` → `0x4fd170`): a reagent's answer
/// while our player is gone, as a world transfer leaves it, empties the book and signals nothing.
#[test]
fn a_build_with_no_player_empties_the_book_silently() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Tailoring")"#);
    quietly(
        &mut app,
        r#"for i = 1, GetNumTradeSkills() do GetTradeSkillReagentInfo(i, 1) end"#,
    );
    let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    app.world_mut()
        .entity_mut(me)
        .remove::<crate::net::SelfPlayer>();
    let answer = SessionEvent::ItemTemplate {
        entry: LINEN,
        info: Some(Box::new(crate::items::test_template("Linen Cloth"))),
    };
    let seen = frame(&mut app, vec![fall(5), answer, fall(6)]);
    assert!(!seen.iter().any(|e| e == "TRADE_SKILL_UPDATE"), "{seen:?}");
    let first = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<Option<String>>("return (GetTradeSkillInfo(1))")
        .unwrap();
    assert_eq!(first, None, "the rows are gone");
}

/// A `/reload` with a product's record in flight: the open's build kept the callback `0x4fc8e0`
/// in the item cache, which outlives the VM, so the record's answer rebuilds the book with flag 1
/// into the new interface at its packet (`0x4fc8fb`, `0x4fc900`), which then lists it.
#[test]
fn a_products_record_answer_rebuilds_the_book_after_a_reload() {
    let mut app = crafter();
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .forget_templates();
    assert_eq!(
        hosted(&mut app, r#"CastSpellByName("Tailoring")"#),
        ["TRADE_SKILL_SHOW"],
        "the pending build signals no update"
    );
    // The reload: a new VM, seeded and entered as the load and the cascade do.
    let mut script = UiScript::new().unwrap();
    crate::ui_tradeskill::seed_repeat(app.world(), &mut script);
    script.reset_crafts_on_world_enter();
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"UNIT_COMBAT", "TRADE_SKILL_UPDATE"}) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    app.world_mut().insert_non_send_resource(script);
    let answer = SessionEvent::ItemTemplate {
        entry: 2996,
        info: Some(Box::new(crate::items::test_template("Bolt of Linen Cloth"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        ["UNIT_COMBAT", "TRADE_SKILL_UPDATE", "UNIT_COMBAT"]
    );
    let rows = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<i64>("return GetNumTradeSkills()")
        .unwrap();
    assert!(rows > 0, "{rows}");
}

/// A product's record the cache refused is looked up again by `GetTradeSkillItemStats`, which
/// asks with the callback `0x4fdc30` inside its call (`0x4fdf64`-`0x4fdf95`): a fall's listener's
/// query goes out ahead of the next packet's handler send, and its callback waits for the answer.
#[test]
fn a_products_stats_miss_asks_inside_the_getters_call() {
    let mut app = crafter();
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    quietly(&mut app, r#"CastSpellByName("Tailoring")"#);
    let refusal = SessionEvent::ItemTemplate {
        entry: 2996,
        info: None,
    };
    frame(&mut app, vec![refusal]);
    sent.try_iter().for_each(drop);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                if Asked then return end
                Asked = true
                for i = 1, GetNumTradeSkills() do
                    if GetTradeSkillInfo(i) == "Bolt of Linen Cloth" then
                        GetTradeSkillItemStats(i)
                    end
                end
            end)
            "#,
        )
        .unwrap();
    let duel = SessionEvent::DuelRequested {
        arbiter: 0xF110_0000_0000_0055,
        challenger: ME,
    };
    frame(&mut app, vec![fall(5), duel]);
    let order: Vec<&str> = sent
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::ItemQuery { entry: 2996, .. } => Some("ItemQuery"),
            crate::net::ClientCommand::DuelAccepted { .. } => Some("DuelAccepted"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["ItemQuery", "DuelAccepted"]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .take_waiters(2996, crate::items::RecordCallback::TradeSkill),
        1,
        "the getter's callback waits on the answer"
    );
}

/// A fall's listener that records `expr`'s answer at its call as `tag:answer`.
fn listen(app: &mut App, tag: &str, expr: &str) {
    app.world()
        .non_send_resource::<UiScript>()
        .run(&format!(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                table.insert(Seen, "{tag}:" .. tostring({expr}))
            end)
            "#
        ))
        .unwrap();
}

/// What `seen` recorded under `tag`.
fn tagged<'a>(seen: &'a [String], tag: &str) -> Vec<&'a str> {
    seen.iter()
        .map(String::as_str)
        .filter(|e| e.strip_prefix(tag).is_some_and(|r| r.starts_with(':')))
        .collect()
}

/// The player's first skill slot holding Enchanting at `value` of 75.
fn enchanting(value: u32) -> SessionEvent {
    values(
        ME,
        &[
            (FIELD_PLAYER_SKILL_INFO_1_1, ENCHANTING),
            (FIELD_PLAYER_SKILL_INFO_1_1 + 1, (75 << 16) | value),
        ],
    )
}

/// `expr`'s answer as a string, read with the host lent.
fn read(app: &mut App, expr: &str) -> String {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<String>(&format!("return tostring({expr})"))
    })
    .unwrap()
}

/// `packets` through their handlers, each one packet, with no frame: the world as they leave it.
fn handle(app: &mut App, packets: Vec<Vec<SessionEvent>>) {
    crate::net::handlers::dispatch(app.world_mut(), wire_packets(packets));
}

/// `GetCraftName` reads the player's effect slot of the open type at its call (`0x4f6c3b` →
/// `0x5ea360`): a learn of the Enchanting opener writes slot 3 inside its handler (`0x5e9e1c`-
/// `0x5e9e36`), and the next call reads its name, with no frame between to push it.
#[test]
fn the_craft_name_reads_the_effect_slot_at_its_call() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    assert_eq!(read(&mut app, "GetCraftName()"), "nil");
    handle(
        &mut app,
        vec![vec![SessionEvent::SpellLearned { spell_id: ENCHANT }]],
    );
    assert_eq!(read(&mut app, "GetCraftName()"), "Enchanting");
}

/// `GetCraftDisplaySkillLine` finds its slot and reads its words at the call (`0x4f6d52`,
/// `0x4f6d99`, `0x4f6e08`): between a packet that raises Enchanting to 5 and one that raises it to
/// 6, it reads 5, where the open's build held none.
#[test]
fn the_display_line_reads_the_rank_its_packet_left() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    listen(
        &mut app,
        "rank",
        "(function() local n, r, m = GetCraftDisplaySkillLine() return n .. r .. '/' .. m end)()",
    );
    let seen = frame(&mut app, vec![enchanting(5), fall(5), enchanting(6)]);
    assert_eq!(tagged(&seen, "rank"), ["rank:Enchanting5/75"], "{seen:?}");
}

/// An item of `entry` at `guid`, `stack` of it, ours.
fn item(guid: u64, entry: u32, stack: u32) -> SessionEvent {
    SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, entry),
            (6, ME as u32),
            (8, ME as u32),
            (FIELD_ITEM_STACK_COUNT, stack),
        ])
        .into_created(ObjectType::Item),
    }
}

/// `GetCraftReagentInfo` counts the reagent in the bags at its call (`0x4f76b2` → `0x622130`) and
/// `GetCraftSpellFocus` asks whether they hold the totem at its call (`0x4f7a3c` → `0x622270`):
/// a fall ahead of the packets that bring both reads none, one behind them both, where the open's
/// build counted none.
#[test]
fn the_craft_counts_read_the_bags_their_packet_left() {
    const DUST_ITEM: u64 = 0x4000_0000_0000_0001;
    const ROD_ITEM: u64 = 0x4000_0000_0000_0002;
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    listen(
        &mut app,
        "count",
        "(function() local _, _, _, have = GetCraftReagentInfo(1, 1) \
         local _, tool = GetCraftSpellFocus(1) return have .. '/' .. tostring(tool) end)()",
    );
    // The first two backpack slots (`PLAYER_FIELD_PACK_SLOT_1`, inventory slots 23 and 24).
    let seat = |slot: u16, guid: u64| {
        let field = FIELD_PLAYER_INV_SLOT_HEAD + 2 * slot;
        [(field, guid as u32), (field + 1, (guid >> 32) as u32)]
    };
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            item(DUST_ITEM, DUST, 2),
            item(ROD_ITEM, ROD, 1),
            values(ME, &[seat(23, DUST_ITEM), seat(24, ROD_ITEM)].concat()),
            fall(6),
        ],
    );
    assert_eq!(
        tagged(&seen, "count"),
        ["count:0/nil", "count:2/1"],
        "{seen:?}"
    );
}

/// `GetCraftInfo`'s cost and level read the pet bar's unit at the call (`0xb714a0` at `0x4f6fe6`,
/// `0x468460` at `0x4f7010`): before the packet that brings the wolf, 0 and 0; after its handler,
/// with no frame or rebuild between, its line's training points (`+0x38`, `0x4f704b`) and the
/// taught spell's level (`0x60c840`).
#[test]
fn the_craft_cost_reads_the_pet_at_its_call() {
    let mut app = crafter();
    {
        let world = app.world_mut();
        world.insert_resource(crate::ui_pet_stats::PetFamilyTables {
            families: benilla_formats::CreatureFamilies::from_skill_lines([(
                WOLF_FAMILY,
                [PET_LINE, 0],
            )]),
            foods: Default::default(),
        });
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_creature(
                WOLF_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Wolf".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: WOLF_FAMILY,
                    rank: 0,
                    type_flags: 0,
                    pet_spell_list: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        world
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = PET;
    }
    quietly(&mut app, r#"CastSpellByName("Beast Training")"#);
    let cost =
        "(function() local _, _, _, _, _, tp, l = GetCraftInfo(1) return tp .. '/' .. l end)()";
    assert_eq!(read(&mut app, cost), "0/0");
    handle(
        &mut app,
        vec![vec![created(PET, EntityKind::Unit, &[(3, WOLF_ENTRY)])]],
    );
    assert_eq!(read(&mut app, cost), "5/4");
}

/// `GetCraftDescription` expands the row's text at its call (`0x4f7b53` → `0x5075f0`), whose `$z`
/// reads the bind point there (`0x5ed420` at `0x507f07`) and the `HOME_INN` template from the VM's
/// strings without one (`0x507f3a`): the inn before the bind point's packet, the area after its
/// handler, with no frame between; a rewrite of the template reaches the next call.
#[test]
fn the_craft_description_reads_the_bind_point_at_its_call() {
    let mut app = crafter();
    app.insert_resource(crate::ui_quest_log::QuestHeaderNamesRes(
        benilla_formats::QuestHeaderNames::from_zones([(12, "Elwynn Forest")]),
    ));
    app.world()
        .non_send_resource::<UiScript>()
        .run("HOME_INN = 'your inn'")
        .unwrap();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    let desc = "GetCraftDescription(1)";
    assert_eq!(read(&mut app, desc), "Returns you to your inn.");
    quietly(&mut app, "HOME_INN = 'the inn'");
    assert_eq!(read(&mut app, desc), "Returns you to the inn.");
    handle(&mut app, vec![vec![SessionEvent::BindPoint { area: 12 }]]);
    assert_eq!(read(&mut app, desc), "Returns you to Elwynn Forest.");
}

/// `SMSG_PET_SPELLS` rebuilds the open craft list inside its handler (`0x4bdafa` → `0x4f60c0`),
/// whose build bands each row on the skill as it stands there (`0x4f620e` → `0x5ea520`,
/// `0x4f6215`-`0x4f6267`) and signals `CRAFT_UPDATE` at its tail (`0x4f661a`): between the falls
/// around it, its listener reads the band the rank before it gives, not the open's.
#[test]
fn a_pet_book_rebuilds_the_craft_list_at_its_packet() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("CRAFT_UPDATE")
            f:SetScript("OnEvent", function()
                local _, _, kind = GetCraftInfo(1)
                table.insert(Seen, "kind:" .. kind)
            end)
            "#,
        )
        .unwrap();
    let seen = frame(
        &mut app,
        vec![
            fall(5),
            enchanting(5),
            SessionEvent::PetSpells(Box::default()),
            fall(6),
        ],
    );
    let craft: Vec<&String> = seen
        .iter()
        .filter(|e| *e == FELL || e.starts_with("CRAFT") || e.starts_with("kind:"))
        .collect();
    assert_eq!(
        craft,
        [FELL, "CRAFT_UPDATE", "kind:trivial", FELL],
        "{seen:?}"
    );
}

/// A craft build that finds no player (the lookup `0x468460` at `0x4f60ff`) has zeroed the counts
/// first (`0x4f60d1`-`0x4f60dd`) and leaves past its signal (`0x4f6109` → `0x4f661f`): a reagent's
/// answer while our player is gone runs the callback `0x4f7710` (`0x4f771a`), which empties the
/// list and signals nothing.
#[test]
fn a_craft_build_with_no_player_empties_the_list_silently() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    quietly(&mut app, "GetCraftReagentInfo(1, 1)");
    let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    app.world_mut()
        .entity_mut(me)
        .remove::<crate::net::SelfPlayer>();
    let answer = SessionEvent::ItemTemplate {
        entry: DUST,
        info: Some(Box::new(crate::items::test_template("Strange Dust"))),
    };
    let seen = frame(&mut app, vec![fall(5), answer, fall(6)]);
    assert!(!seen.iter().any(|e| e == "CRAFT_UPDATE"), "{seen:?}");
    let script = app.world().non_send_resource::<UiScript>();
    assert_eq!(script.eval::<i64>("return GetNumCrafts()").unwrap(), 0);
    assert_eq!(
        script
            .eval::<Option<String>>("return (GetCraftInfo(1))")
            .unwrap(),
        None,
        "the rows are gone"
    );
}

/// With our player in place, the same answer's callback rebuilds the list at its packet and
/// signals `CRAFT_UPDATE` between the falls around it (`0x4f771a` → `0x4f661a`), and the getter
/// then reads the record.
#[test]
fn a_craft_reagents_record_answer_signals_the_update_at_its_packet() {
    let mut app = crafter();
    quietly(&mut app, r#"CastSpellByName("Enchanting")"#);
    quietly(&mut app, "GetCraftReagentInfo(1, 1)");
    let answer = SessionEvent::ItemTemplate {
        entry: DUST,
        info: Some(Box::new(crate::items::test_template("Strange Dust"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "CRAFT_UPDATE", FELL]
    );
    let name = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<String>("return (GetCraftReagentInfo(1, 1))")
    })
    .unwrap();
    assert_eq!(name, "Strange Dust");
}

/// Another player's visible item rebuilds both open lists (`0x5e2810` → `0x5dee30`: `0x4fca20(0)`
/// at `0x5dee70`, `0x4f60c0` at `0x5dee75`), ahead of its test for the active player
/// (`0x5dee82`), so each window's update fires at that player's packet.
#[test]
fn another_players_visible_item_rebuilds_the_open_lists() {
    const OTHER: u64 = 0x9;
    let mut app = crafter();
    quietly(
        &mut app,
        r#"CastSpellByName("Tailoring") CastSpellByName("Enchanting")"#,
    );
    let entry = benilla_protocol::field::FIELD_PLAYER_VISIBLE_ITEM_1_CREATOR + 2;
    frame(
        &mut app,
        vec![created(OTHER, EntityKind::Player, &[(entry, 2996)])],
    );
    assert_eq!(
        frame(&mut app, vec![values(OTHER, &[(entry, 2589)])]),
        ["TRADE_SKILL_UPDATE", "CRAFT_UPDATE"]
    );
}
