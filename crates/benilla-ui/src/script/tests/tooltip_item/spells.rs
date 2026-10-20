//! The builder's spell loop (`0x52d975`..`0x52dd4d`): the trigger prefixes, each block's charges,
//! and a recipe's product and reagents.

use super::{lines_of, script};
use crate::script::*;

/// One template whose five slots carry `triggers` in order, each slot's text its trigger's name.
fn triggered(triggers: &[u32]) -> ItemTemplateView {
    ItemTemplateView {
        name: "Charm".into(),
        quality: 1,
        spell_lines: triggers
            .iter()
            .enumerate()
            .map(|(i, &t)| ItemSpellLine {
                trigger: t,
                spell_id: 100 + i as u32,
                text: format!("text{t}"),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// Render `id` into a fresh `TT` and answer its lines' texts.
fn texts_of(s: &mut UiScript, id: u32) -> Vec<String> {
    s.run(&format!(
        r#"
        local a = CreateFrame("Button", "SpellSlot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(tt, {id})
    "#
    ))
    .unwrap();
    lines_of(s).into_iter().map(|(t, _)| t).collect()
}

/// Only triggers 0, 1 and 2 take a prefix, through `"%s %s"` (`0x52da37`..`0x52dae2`); every other
/// trigger copies the text alone (`0x52da49`..`0x52da61`) and the line is still added
/// (`0x52db49`). Trigger 5 is no "Use:", and trigger 6 has a line.
#[test]
fn only_triggers_zero_to_two_take_a_prefix_and_every_other_adds_the_bare_text() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(9100, triggered(&[0, 1, 2, 5, 6]));
    assert_eq!(
        texts_of(&mut s, 9100),
        [
            "Charm",
            "[ONUSE] text0",
            "[ONEQUIP] text1",
            "[ONPROC] text2",
            "text5",
            "text6",
        ]
    );
    s.set_item_template(9101, triggered(&[3, 4]));
    assert_eq!(texts_of(&mut s, 9101), ["Charm", "text3", "text4"]);
    assert!(s.take_errors().is_empty());
}

/// A prefix key the install lacks reads as the empty string (`0x703bf0`), which `"%s %s"` still
/// joins: the line keeps its text behind the space, it is not dropped.
#[test]
fn a_missing_prefix_key_keeps_the_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run("ITEM_SPELL_TRIGGER_ONEQUIP = nil").unwrap();
    s.set_item_template(9102, triggered(&[1]));
    assert_eq!(texts_of(&mut s, 9102), ["Charm", " text1"]);
    assert!(s.take_errors().is_empty());
}

const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Render `id` into `TT` and answer its lines with their colours.
fn lines_after(s: &mut UiScript, id: u32) -> Vec<(String, [f32; 4])> {
    texts_of(s, id);
    lines_of(s)
}

fn spell_line(trigger: u32, text: &str, charges: i32) -> ItemSpellLine {
    ItemSpellLine {
        trigger,
        spell_id: 100,
        text: text.into(),
        charges,
        block: 0,
        recipe: None,
    }
}

/// Each block's charges follow its own line, white (`0x52db51`..`0x52db8f`): the absolute count,
/// none for -1 or for a template 0, which reads as -1 (`0x52da06`).
#[test]
fn each_blocks_charges_follow_its_own_line() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        9103,
        ItemTemplateView {
            name: "Charm".into(),
            quality: 1,
            spell_lines: vec![
                spell_line(0, "a", -5),
                spell_line(1, "b", 0),
                spell_line(0, "c", -1),
                spell_line(2, "d", 1),
            ],
            ..Default::default()
        },
    );
    let lines = lines_after(&mut s, 9103);
    assert_eq!(
        lines,
        [
            ("Charm".to_string(), WHITE),
            ("[ONUSE] a".to_string(), GREEN),
            ("[CHARGES 5 x]".to_string(), WHITE),
            ("[ONEQUIP] b".to_string(), GREEN),
            ("[ONUSE] c".to_string(), GREEN),
            ("[ONPROC] d".to_string(), GREEN),
            ("[CHARGES 1]".to_string(), WHITE),
        ]
    );
    assert!(s.take_errors().is_empty());
}

const RECIPE: u32 = 9200;
const POTION: u32 = 9201;
const THISTLE: u32 = 2452;
const BRIAR: u32 = 2450;
const VIAL: u32 = 3371;

/// A recipe for the potion, whose reagents are the thistle, two briarthorns and a vial.
fn recipe_fixture() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(
        RECIPE,
        ItemTemplateView {
            name: "Recipe: Swiftness Potion".into(),
            quality: 1,
            spell_lines: vec![ItemSpellLine {
                recipe: Some(ItemRecipe {
                    product: POTION,
                    reagents: vec![(THISTLE, 1), (BRIAR, 2), (VIAL, 1)],
                }),
                ..spell_line(0, "Teaches you how to make a Swiftness Potion.", -1)
            }],
            ..Default::default()
        },
    );
    s.set_item_template(
        POTION,
        ItemTemplateView {
            name: "Swiftness Potion".into(),
            quality: 1,
            spell_lines: vec![spell_line(0, "Increases run speed.", -1)],
            ..Default::default()
        },
    );
    for (id, name) in [(THISTLE, "Swiftthistle"), (BRIAR, "Briarthorn")] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                ..Default::default()
            },
        );
    }
    s
}

fn vial_arrives(s: &mut UiScript) {
    s.set_item_template(
        VIAL,
        ItemTemplateView {
            name: "Empty Vial".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.tick(0.1);
}

/// A block whose spell teaches a spell that creates an item is a recipe: its line is white
/// (`0x52db25`), the product's tooltip follows from an incremental build, its name behind a
/// newline (`0x52dbce`, `0x52b874`), then the reagents as one white `"\n%s"` of `ITEM_REQ_SKILL`,
/// a count above 1 in `"%s (%d)"`, joined by `", "` (`0x52dbee`..`0x52dd38`).
#[test]
fn a_recipe_line_is_white_and_shows_its_product_and_reagents() {
    let mut s = recipe_fixture();
    vial_arrives(&mut s);
    let lines = lines_after(&mut s, RECIPE);
    assert_eq!(
        lines,
        [
            ("Recipe: Swiftness Potion".to_string(), WHITE),
            (
                "[ONUSE] Teaches you how to make a Swiftness Potion.".to_string(),
                WHITE
            ),
            ("\nSwiftness Potion".to_string(), WHITE),
            ("[ONUSE] Increases run speed.".to_string(), GREEN),
            (
                "\n[REQ_SKILL Swiftthistle, Briarthorn (2), Empty Vial]".to_string(),
                WHITE
            ),
        ]
    );
    assert!(s.take_errors().is_empty());
}

/// A reagent whose record is not cached asks for it and leaves the reagent line out
/// (`0x52dc27`..`0x52dc32`, `0x52dcd8`); its answer rebuilds the tooltip with the line. A product
/// in flight asks too and adds nothing, the rest of the build going on (`0x52b72e`).
#[test]
fn a_reagent_or_product_in_flight_waits_for_its_answer() {
    let mut s = recipe_fixture();
    s.run(
        r#"
        local a = CreateFrame("Button", "RecipeSlot"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        TT:SetOwner(a, "ANCHOR_RIGHT")
        SetItemByIdForTest(TT, 9200)
    "#,
    )
    .unwrap();
    let texts =
        |s: &mut UiScript| -> Vec<String> { lines_of(s).into_iter().map(|(t, _)| t).collect() };
    assert_eq!(
        texts(&mut s).len(),
        4,
        "no reagent line: {:?}",
        texts(&mut s)
    );
    assert_eq!(s.take_item_stat_asks(), [VIAL]);
    vial_arrives(&mut s);
    assert_eq!(
        texts(&mut s).last().map(String::as_str),
        Some("\n[REQ_SKILL Swiftthistle, Briarthorn (2), Empty Vial]"),
        "the answer rebuilds the reagent line"
    );

    let mut s = recipe_fixture();
    vial_arrives(&mut s);
    s.set_item_template(
        RECIPE + 2,
        ItemTemplateView {
            name: "Recipe: Lost".into(),
            quality: 1,
            spell_lines: vec![ItemSpellLine {
                recipe: Some(ItemRecipe {
                    product: 9999,
                    reagents: vec![(VIAL, 1)],
                }),
                ..spell_line(0, "Teaches.", -1)
            }],
            ..Default::default()
        },
    );
    let lines = lines_after(&mut s, RECIPE + 2);
    let texts: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Recipe: Lost",
            "[ONUSE] Teaches.",
            "\n[REQ_SKILL Empty Vial]"
        ]
    );
    assert_eq!(s.take_item_stat_asks(), [9999]);
    assert!(s.take_errors().is_empty());
}

/// Render through `call` on `TT` (owned first) and answer the lines' texts.
fn texts_after(s: &mut UiScript, call: &str) -> Vec<String> {
    s.run(&format!(
        r#"
        local a = SpellSlot or CreateFrame("Button", "SpellSlot")
        a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local tt = TT or CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        {call}
    "#
    ))
    .unwrap();
    lines_of(s).into_iter().map(|(t, _)| t).collect()
}

/// The charges lines of what `call` renders.
fn charges_after(s: &mut UiScript, call: &str) -> Vec<String> {
    texts_after(s, call)
        .into_iter()
        .filter(|t| t.starts_with("[CHARGES"))
        .collect()
}

/// A wand whose three blocks carry template charges 5, 0 and -3.
fn wand() -> ItemTemplateView {
    ItemTemplateView {
        name: "Wand".into(),
        quality: 1,
        spell_lines: vec![
            spell_line(0, "a", 5),
            ItemSpellLine {
                block: 1,
                ..spell_line(0, "b", 0)
            },
            ItemSpellLine {
                block: 2,
                ..spell_line(0, "c", -3)
            },
        ],
        ..Default::default()
    }
}

/// The count a block's line shows comes from the source the builder picks (`0x52d9bd`..`0x52da06`):
/// with an instance block and no item object, the block's `+0x428`, the -1 that prints nothing
/// unless the setter wrote its own (a link and a loot row clear it, `0x5326e0`; the trade
/// partner's slot carries the wire's, `0x534595`, one value for every block); with an item object
/// and no block value, the object's `ITEM_FIELD_SPELL_CHARGES` for that block (`0x52d9e9`), where
/// 0 prints; with neither, the template's. A template 0 is -1 from any source (`0x52da06`).
#[test]
fn a_blocks_charges_come_from_the_source_the_builder_picks() {
    use std::collections::HashMap;
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_item_template(9104, wand());
    assert_eq!(
        charges_after(&mut s, "SetItemByIdForTest(TT, 9104)"),
        ["[CHARGES 5 x]", "[CHARGES 3 x]"],
        "a template source counts the template's"
    );
    assert!(
        charges_after(&mut s, r#"TT:SetHyperlink("item:9104:0:0:0")"#).is_empty(),
        "a link's block holds -1"
    );
    s.set_loot(Some(LootState {
        rows: vec![Some(LootRow {
            name: Some("Wand".into()),
            quantity: 1,
            quality: Some(1),
            item_id: 9104,
            ..Default::default()
        })],
        ..Default::default()
    }));
    assert!(
        charges_after(&mut s, "TT:SetLootItem(1)").is_empty(),
        "so does a loot row's"
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 9104,
                    count: 1,
                    quality: Some(1),
                    guid: 0x4000_0000_0000_0104,
                    spell_charges: [2, 7, 0, 0, 0],
                    ..Default::default()
                },
            )]),
        }),
    );
    assert_eq!(
        charges_after(&mut s, "TT:SetBagItem(0, 1)"),
        ["[CHARGES 2 x]", "[CHARGES 0 x]"],
        "an item object counts its own, block by block, 0 included"
    );
    let mut trade = TradeState::default();
    trade.target.slots[0] = Some(TradeSlotItem {
        item_id: 9104,
        name: Some("Wand".into()),
        count: 1,
        quality: Some(1),
        charges: 4,
        ..Default::default()
    });
    s.set_trade(Some(trade));
    assert_eq!(
        charges_after(&mut s, "TT:SetTradeTargetItem(1)"),
        ["[CHARGES 4 x]", "[CHARGES 4 x]"],
        "the partner's block counts the wire's for every block"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// `SetAction`'s item leg writes into the block the summed absolute charges of every copy the
/// player holds (`0x622180` at `0x532553`, stored at `+0x428`), but only when the cached
/// template's block-0 charges (`+0x144`) are neither -1 nor 0 (`0x532520`, `0x532525`). With an
/// item object and a block value other than -1 the builder counts the block's (`0x52d9d1`), else
/// the object's own: two wands of 3 and 2 charges read 5, and two potion stacks (template -1)
/// read their object's -1, no line.
#[test]
fn an_action_items_charges_sum_every_copy_of_a_charged_item() {
    use std::collections::HashMap;
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    for (id, name, charges) in [(9105, "Potion", -1), (9106, "Wand", 5)] {
        s.set_item_template(
            id,
            ItemTemplateView {
                name: name.into(),
                quality: 1,
                spell_lines: vec![spell_line(0, "Use.", charges)],
                spell_charges_0: charges,
                ..Default::default()
            },
        );
    }
    let held = |item_id, guid, charges_0| ContainerSlot {
        item_id,
        count: 5,
        quality: Some(1),
        guid,
        spell_charges: [charges_0, 0, 0, 0, 0],
        ..Default::default()
    };
    s.set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: HashMap::from([
                (1, held(9105, 0x105, -1)),
                (2, held(9105, 0x106, -1)),
                (3, held(9106, 0x107, 3)),
                (4, held(9106, 0x108, 2)),
            ]),
        }),
    );
    for (id, item) in [(1, 9105), (2, 9106)] {
        s.set_action(
            id,
            Some(ActionSlot {
                kind: 0x80,
                action: item,
                ..Default::default()
            }),
        );
    }
    assert!(
        charges_after(&mut s, "TT:SetAction(1)").is_empty(),
        "a potion stack reads its object's -1"
    );
    assert_eq!(charges_after(&mut s, "TT:SetAction(2)"), ["[CHARGES 5 x]"]);
    assert_eq!(
        charges_after(&mut s, "TT:SetBagItem(0, 3)"),
        ["[CHARGES 3 x]"],
        "the bag slot reads its object's own"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// A recipe's product is built into the same tooltip with no item object (`0x52dbce`), and an
/// incremental build keeps the instance fields its outer build set (`0x52b6ca` skips the stores
/// at `0x52b6cc`..`0x52b70a`): through a link, the product reads the link's block, so it shows no
/// `ITEM_RANDOM_ENCHANT` placeholder (`0x52c9a3`), no durability (`+0x434` is 0) and no charges
/// (`+0x428` is -1); through a template source it has no block either, so it shows the
/// placeholder and its template's charges, and still no durability (`0x52cca8`).
#[test]
fn a_recipes_product_reads_the_outer_block() {
    let mut s = recipe_fixture();
    vial_arrives(&mut s);
    s.set_item_template(
        RECIPE + 3,
        ItemTemplateView {
            name: "Pattern: Robe".into(),
            quality: 1,
            spell_lines: vec![ItemSpellLine {
                recipe: Some(ItemRecipe {
                    product: RECIPE + 4,
                    reagents: vec![(VIAL, 1)],
                }),
                ..spell_line(0, "Teaches.", -1)
            }],
            ..Default::default()
        },
    );
    s.set_item_template(
        RECIPE + 4,
        ItemTemplateView {
            name: "Robe".into(),
            quality: 2,
            max_durability: 60,
            random_property: 5,
            spell_lines: vec![spell_line(0, "Glows.", 3)],
            ..Default::default()
        },
    );
    let template = texts_after(&mut s, &format!("SetItemByIdForTest(TT, {})", RECIPE + 3));
    assert_eq!(
        template,
        [
            "Pattern: Robe",
            "[ONUSE] Teaches.",
            "\nRobe",
            "[ITEM_RANDOM_ENCHANT]",
            "[ONUSE] Glows.",
            "[CHARGES 3 x]",
            "\n[REQ_SKILL Empty Vial]",
        ]
    );
    let link = texts_after(
        &mut s,
        &format!(r#"TT:SetHyperlink("item:{}:0:0:0")"#, RECIPE + 3),
    );
    assert_eq!(
        link,
        [
            "Pattern: Robe",
            "[ONUSE] Teaches.",
            "\nRobe",
            "[ONUSE] Glows.",
            "\n[REQ_SKILL Empty Vial]",
        ]
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}
