//! The profession windows open inside the cast that opens them. `TryCastSpell` (`0x6e4b60`) sends
//! nothing for a spell whose first effect is a profession's (`0x6e4bce`): it toggles the window
//! in place, `ToggleTradeSkill` (`0x4fc910` at `0x6e4c22`) or `ToggleCraftWindow` (`0x4f6000` at
//! `0x6e4c34`), whose build and `TRADE_SKILL_SHOW` or `CRAFT_SHOW` signal before the cast returns.
//! So a listener's cast shows the window between its packet's events and the next packet's, and a
//! chunk that opens and closes leaves the window shut.

use benilla_formats::{
    SlaInfo, SpellDisplay, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_TRADE_SKILL,
};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, fall, frame};
use crate::ui_craft::CraftOpen;
use crate::ui_tradeskill::{TradeSkillOpen, TradeSkillOpens};

/// [`client`]'s fall, as its spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";
const TAILORING: u32 = 197;
const ENCHANTING: u32 = 333;
/// The Tailoring opener, Bolt of Linen Cloth, the Enchanting opener, and a Minor Health enchant.
const TAILOR: u32 = 3908;
const BOLT: u32 = 2963;
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
    let bolt = SpellDisplay {
        name: "Bolt of Linen Cloth".into(),
        attributes: SPELL_ATTR_IS_TRADESKILL,
        effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
        effect_item_type: [2996, 0, 0],
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
