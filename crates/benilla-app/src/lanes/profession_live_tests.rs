//! The profession windows open inside the cast that opens them. `TryCastSpell` (`0x6e4b60`) sends
//! nothing for a spell whose first effect is a profession's (`0x6e4bce`): it toggles the window
//! in place, `ToggleTradeSkill` (`0x4fc910` at `0x6e4c22`) or `ToggleCraftWindow` (`0x4f6000` at
//! `0x6e4c34`), whose build and `TRADE_SKILL_SHOW` or `CRAFT_SHOW` signal before the cast returns.
//! So a listener's cast shows the window between its packet's events and the next packet's, and a
//! chunk that opens and closes leaves the window shut. The book's builds and getters read the
//! player, the bags and the item cache as they stand at their call (`0x4fca20`, `0x4fdd40`,
//! `0x4ff7a2`), and the item answers run the lists' callbacks inside their handler
//! (`ItemCacheReceive` `0x55bdb0`: `0x4fdc30`, `0x4fc8e0`).

use benilla_formats::{
    SlaInfo, SpellDisplay, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_TRADE_SKILL,
};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use benilla_protocol::messages::FIELD_PLAYER_SKILL_INFO_1_1;
use benilla_protocol::SessionEvent;

use super::unit_view_tests::{client, fall, frame, values};
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
    let health = SpellDisplay {
        name: "Enchant Chest - Minor Health".into(),
        cast_ui: 3,
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
        ])
        .with_line_names([(TAILORING, "Tailoring"), (ENCHANTING, "Enchanting")]),
    });
    {
        let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
        actions.spells.extend([TAILOR, BOLT, ENCHANT, MINOR_HEALTH]);
        actions.profession_order.push(BOLT);
        actions.craft_order.push(MINOR_HEALTH);
    }
    world.resource_mut::<crate::items::Items>().insert_template(
        2996,
        Some(crate::items::test_template("Bolt of Linen Cloth")),
    );
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
