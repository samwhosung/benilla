//! The spell tooltip as a listener builds it. The builder reads the player and the talent
//! modifier tables at the setter's call (`0x52e610`) and keeps nothing; the modifier handler
//! stores its cell and then runs the trainer's state pass (`0x6e99b4` → `0x6e99bb` for the flat
//! table, `0x6e9998` → `0x6e999f` for the percent one), whose `TRAINER_UPDATE` a listener hears
//! with the cell already stored.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{trainer_spell_state, TrainerSpell};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::{KeyedArg, SpellTooltipView, TooltipText, UiScript};
use bevy::prelude::*;

use super::unit_view_tests::created;

const ME: u64 = 0x10;
const TRAINER: u64 = 0xF130_0000_4500_0002;
/// A mage spell (family 3) on family-flag bit 5, costing 100 mana.
const FROSTBOLT: u32 = 116;
const FROST_BIT: u8 = 5;
/// `UNIT_FIELD_BYTES_0` of a human mage.
const BYTES_0: u16 = 36;
const HUMAN_MAGE: u32 = 1 | (8 << 8);

/// The listener: at each `TRAINER_UPDATE`, the book's first spell's cost line as `SetSpell` builds
/// it.
const LISTENER: &str = r#"
    MANA_COST = "%d Mana"
    SPELL_REAGENTS = "Reagents: "
    Costs = {}
    local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
    CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
    local f = CreateFrame("Frame")
    f:RegisterEvent("TRAINER_UPDATE")
    f:SetScript("OnEvent", function()
        TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetSpell(1, "spell")
        table.insert(Costs, tostring(TTTextLeft2:GetText()))
    end)
"#;

/// The client in the world as a mage knowing Frostbolt, at a trainer, and a VM whose listener
/// notes each `TRAINER_UPDATE`'s cost line.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let frostbolt = SpellDisplay {
        id: FROSTBOLT,
        name: "Frostbolt".into(),
        spell_family: 3,
        spell_family_flags: 1 << FROST_BIT,
        mana_cost: 100,
        ..Default::default()
    };
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(FROSTBOLT, frostbolt)])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
    });
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world.resource_mut::<crate::ui_trainer::TrainerOpen>().asked = TRAINER;
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(FROSTBOLT);
    let script = UiScript::new().expect("a VM");
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script.run(LISTENER).unwrap();
    world.insert_non_send_resource(script);
    crate::net::handlers::dispatch(
        world,
        vec![
            created(ME, EntityKind::Player, &[(BYTES_0, HUMAN_MAGE)]),
            SessionEvent::ObjectUpdateEnd,
        ],
    );
    world.flush();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::SelfPlayer);
    // The class's family, which the modifier gates read (`0x6e6b30`).
    world
        .resource_mut::<crate::spell::SpellModifiers>()
        .set_class_family(3);
    let list = SessionEvent::TrainerList {
        trainer: TRAINER,
        trainer_type: 0,
        services: vec![TrainerSpell {
            spell: FROSTBOLT,
            state: trainer_spell_state::GREEN,
            cost: 10,
            can_learn_primary_prof: false,
            is_primary_prof_first_rank: false,
            req_level: 1,
            req_skill: 0,
            req_skill_value: 0,
            req_spells: [0; 3],
        }],
        greeting: String::new(),
    };
    handled(&mut app, vec![list]);
    app
}

/// `packets` through their handlers, then the drain; the cost lines the listener read.
fn handled(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    crate::net::handlers::dispatch(app.world_mut(), packets);
    app.world_mut().flush();
    crate::lanes::fire_in_order(app.world_mut());
    crate::script_calls::eval_in_vm(app.world(), "local c = Costs Costs = {} return c")
}

/// The modifier packet stores its cell and signals `TRAINER_UPDATE` behind it: the listener's
/// `SetSpell` builds the cost with the new cell, where the VM's copy holds the view pushed before
/// the packet.
#[test]
fn a_listeners_set_spell_reads_the_modifier_its_packet_stored() {
    let mut app = client();
    // What the view held before this packet, as a push ahead of it left it.
    let before = SpellTooltipView {
        name: "Frostbolt".into(),
        cost: Some(TooltipText::keyed("MANA_COST", vec![KeyedArg::D(100)])),
        ..Default::default()
    };
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spell_tooltip(FROSTBOLT, before);
    let modifier = SessionEvent::SpellModifier {
        flat: true,
        mask_bit: FROST_BIT,
        op: crate::spell::OP_COST,
        value: -40,
    };
    assert_eq!(handled(&mut app, vec![modifier]), ["60 Mana"]);
}

/// `"mouseover"` resolves to the published guid (`0x515970`) and its getters read that unit at the
/// call, as `"target"`'s do: a raid mark a packet sets on the hovered unit reads with the host lent
/// (`GetRaidTargetIndex`, by guid, `0x4bb4f1`), where the hover's push keeps what its build read.
#[test]
fn the_mouseover_getters_read_the_hovered_unit_at_the_call() {
    use super::unit_view_tests::{client, frame};
    use bevy::ecs::system::RunSystemOnce;
    /// The harness's target.
    const WOLF: u64 = 0xF130_0000_0044_0001;
    let mut app = client();
    let world = app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Hovered>() = crate::target::Hovered {
        target: Some(wolf),
        guid: Some(WOLF),
        distance: 2.0,
        ..Default::default()
    };
    world
        .run_system_once(crate::ui_tooltip::drive_mouseover_tooltip)
        .unwrap();
    frame(
        &mut app,
        vec![SessionEvent::RaidTargetSet {
            icon: 2,
            guid: WOLF,
        }],
    );
    let read = r#"return GetRaidTargetIndex("mouseover")"#;
    let live: Option<i64> =
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").eval(read))
            .expect("the chunk runs");
    let pushed: Option<i64> = crate::script_calls::eval_in_vm(app.world(), read);
    assert_eq!((live, pushed), (Some(3), None));
}

/// A reagent the item cache misses at the build is asked with the tooltip as its cookie and
/// counted (`0x52f3f4`, `0x52f3ff`); its answer builds the spell again from the kept id
/// (`0x52f820` → `0x52e610`), so the shown tooltip gains the line with no new hover.
#[test]
fn a_reagents_answer_rebuilds_the_shown_spell_tooltip() {
    /// Arcane Powder.
    const POWDER: u32 = 17020;
    let mut app = client();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (POWDER, 1);
    let frostbolt = SpellDisplay {
        id: FROSTBOLT,
        name: "Frostbolt".into(),
        reagents,
        ..Default::default()
    };
    app.world_mut()
        .resource_mut::<crate::ui_action::Spells>()
        .catalog = SpellCatalog::from_displays(HashMap::from([(FROSTBOLT, frostbolt)]));
    let lines = r#"
        local t = {}
        for i = 1, TT:NumLines() do t[i] = getglobal("TTTextLeft" .. i):GetText() end
        return table.concat(t, " | ")
    "#;
    let hosted = |app: &mut App, chunk: &str| -> String {
        crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").eval(chunk))
            .expect("the chunk runs")
    };
    hosted(
        &mut app,
        r#"TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetSpell(1, "spell"); return """#,
    );
    assert_eq!(hosted(&mut app, lines), "Frostbolt", "the name unknown");
    // The record lands, in the app's cache and the VM's copy.
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(POWDER, Some(crate::items::test_template("Arcane Powder")));
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_item_template(
            POWDER,
            benilla_ui::script::ItemTemplateView {
                name: "Arcane Powder".into(),
                ..Default::default()
            },
        );
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").tick(0.01));
    assert_eq!(
        hosted(&mut app, lines),
        "Frostbolt | Reagents: |cffff2020Arcane Powder|r",
        "none carried, so red"
    );
}
