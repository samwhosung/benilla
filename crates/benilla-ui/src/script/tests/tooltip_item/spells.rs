//! The builder's spell loop (`0x52d975`..`0x52dd4d`): the trigger prefixes and the lines a slot's
//! spell adds.

use super::{lines_of, script};
use crate::script::*;

/// One template whose five slots carry `triggers` in order, each slot's text its trigger's name.
fn triggered(triggers: &[u32]) -> ItemTemplateView {
    ItemTemplateView {
        name: "Charm".into(),
        quality: 1,
        spell_triggers: triggers
            .iter()
            .enumerate()
            .map(|(i, &t)| (t, 100 + i as u32, format!("text{t}")))
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
