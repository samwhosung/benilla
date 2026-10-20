//! The combat lines through the drain: what holds a line, what names its endpoints, and the XP
//! line's group and raid forms.

use benilla_protocol::field::FIELD_OBJECT_ENTRY;

use super::combat::{self, Fills, Named, UnitClass};
use super::feed::{xp_gain_line, RestState};
use super::notice_tests::Feed;
use super::ChatEventKind as K;

const ME: u64 = 7;

/// A frame that records the first argument of each `event` it sees, into `Seen`.
fn spy(feed: &Feed, event: &str) {
    feed.script()
        .run(&format!(
            r#"
            Seen = {{}}
            Drain = CreateFrame("Frame", "BenillaDrainSpy")
            Drain:SetScript("OnEvent", function() table.insert(Seen, arg1) end)
            Drain:RegisterEvent("{event}")
        "#
        ))
        .unwrap();
}

fn seen(feed: &Feed) -> Vec<String> {
    feed.script().eval::<Vec<String>>("return Seen").unwrap()
}

/// The reference parks a line that asked the item cache for a name in a list with no timer or
/// count (`0x6293a0`), and only the answer (`0x6294cc`-`0x62959c`) or shutdown (`0x629a90`)
/// removes it, so a record that lands late still writes its line.
#[test]
fn a_line_waiting_on_an_item_name_waits_until_the_cache_answers() {
    let _data = benilla_formats::wow_data_or_skip!();
    const LINEN: u32 = 2589;
    let mut feed = Feed::new();
    spy(&feed, super::event::event_name(K::SpellTradeskills));
    feed.world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let line = combat::queue(
        K::SpellTradeskills,
        combat::TRADESKILL_LOG,
        (ME, UnitClass::Me),
        (ME, UnitClass::Me),
        Fills::default(),
        Named::Item(LINEN),
    )
    .unwrap();
    feed.world
        .resource_mut::<super::ChatLog>()
        .push_combat(line);
    for _ in 0..300 {
        feed.drain();
    }
    assert!(seen(&feed).is_empty(), "nothing to name it with yet");
    feed.world
        .resource_mut::<crate::items::Items>()
        .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
    feed.drain();
    assert_eq!(seen(&feed), ["You create Linen Cloth."]);
    feed.drain();
    assert_eq!(seen(&feed).len(), 1, "written once");
}

/// `GetCombatLogName` (`0x6264e0`) names a held object by its type mask: a game object by its
/// record (`0x626547` → `0x5f8100`), an item by the item cache without asking (`0x62656e` →
/// `0x55ba30`), and a name the caches lack is `UNKNOWNOBJECT`.
#[test]
fn a_game_object_or_item_endpoint_is_named_from_its_record() {
    let _data = benilla_formats::wow_data_or_skip!();
    const TRAP: u64 = 0xf110_0000_0000_0007;
    const BARE_TRAP: u64 = 0xf110_0000_0000_0008;
    const VIAL: u64 = 0x4000_0000_0000_0042;
    let mut feed = Feed::new();
    let kind = K::SpellCreatureVsSelfDamage;
    spy(&feed, super::event::event_name(kind));
    for (guid, mask, entry) in [
        (TRAP, 0x21, 1731),
        (BARE_TRAP, 0x21, 1732),
        (VIAL, 0x3, 2589),
    ] {
        let e = feed
            .world
            .spawn(crate::net::ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (2, mask),
                    (FIELD_OBJECT_ENTRY, entry),
                ]),
            ))
            .id();
        feed.world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
    }
    feed.world
        .init_resource::<crate::go_templates::GameObjectTemplates>();
    feed.world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(1731, 6, "Venom Trap".into(), &[0; 24]);
    feed.world
        .resource_mut::<crate::items::Items>()
        .insert_template(2589, Some(crate::items::test_template("Poison Vial")));
    for caster in [TRAP, VIAL, BARE_TRAP] {
        let line = combat::queue(
            kind,
            combat::SIMPLECAST,
            (caster, UnitClass::Creature),
            (ME, UnitClass::Me),
            Fills {
                spell: "Poison".into(),
                ..Default::default()
            },
            Named::Ready,
        )
        .unwrap();
        feed.world
            .resource_mut::<super::ChatLog>()
            .push_combat(line);
    }
    feed.drain();
    assert_eq!(
        seen(&feed),
        [
            "Venom Trap casts Poison on you.",
            "Poison Vial casts Poison on you.",
            "Unknown casts Poison on you.",
        ]
    );
}

/// A kill's XP line follows the packet's group float (`0x637cd0`, `0x637ac0`): `ftol(xp / float)`
/// against `xp` is equal for the plain line, below for `_GROUP` ("+N group bonus") and above for
/// `_RAID`, N the difference; a rested kill does the same on its base XP, appending `_GROUP` or
/// `_RAID` to the state's key (`0x637b70`-`0x637bd5`). `ftol` truncates and wraps as `__ftol`
/// does (`0x40a2b0`).
#[test]
fn a_kills_group_float_words_the_group_or_raid_form() {
    let get = |key: &str| {
        Some(
            match key {
                "COMBATLOG_XPGAIN_FIRSTPERSON" => "%s dies, you gain %d experience.",
                "COMBATLOG_XPGAIN_FIRSTPERSON_GROUP" => {
                    "%s dies, you gain %d experience. (+%d group bonus)"
                }
                "COMBATLOG_XPGAIN_FIRSTPERSON_RAID" => {
                    "%s dies, you gain %d experience. (-%d raid penalty)"
                }
                "COMBATLOG_XPGAIN_EXHAUSTION1" => {
                    "%s dies, you gain %d experience. (%s exp %s bonus)"
                }
                "COMBATLOG_XPGAIN_EXHAUSTION1_GROUP" => {
                    "%s dies, you gain %d experience. (%s exp %s bonus, +%d group bonus)"
                }
                "COMBATLOG_XPGAIN_EXHAUSTION1_RAID" => {
                    "%s dies, you gain %d experience. (%s exp %s bonus, -%d raid penalty)"
                }
                _ => return None,
            }
            .to_string(),
        )
    };
    let plain = |total, group| {
        xp_gain_line(
            Some("Defias Thug"),
            total,
            total,
            group,
            Some(RestState {
                state: 2,
                row: Some(("Normal", 1.0)),
            }),
            &get,
        )
    };
    let rested = |total, base, group| {
        xp_gain_line(
            Some("Defias Thug"),
            total,
            base,
            group,
            Some(RestState {
                state: 1,
                row: Some(("Rested", 2.0)),
            }),
            &get,
        )
    };
    let dies = "Defias Thug dies, you gain";
    assert_eq!(
        plain(60, 1.0).as_deref(),
        Some(format!("{dies} 60 experience.").as_str())
    );
    // 60 / 1.5 = 40, below 60: the group bonus is the 20 between.
    assert_eq!(
        plain(60, 1.5).as_deref(),
        Some(format!("{dies} 60 experience. (+20 group bonus)").as_str())
    );
    // 30 / 0.5 = 60, above 30: a raid penalty of 30.
    assert_eq!(
        plain(30, 0.5).as_deref(),
        Some(format!("{dies} 30 experience. (-30 raid penalty)").as_str())
    );
    // 60 / 1.2f = 49.999998…, which `__ftol` truncates to 49.
    assert_eq!(
        plain(60, 1.2).as_deref(),
        Some(format!("{dies} 60 experience. (+11 group bonus)").as_str())
    );
    // A float of 0 divides to infinity, which `__ftol` stores as 0: all 60 is bonus.
    assert_eq!(
        plain(60, 0.0).as_deref(),
        Some(format!("{dies} 60 experience. (+60 group bonus)").as_str())
    );
    // The rested forms compare on the base XP (40 → 52 with the 12 of bonus).
    assert_eq!(
        rested(52, 40, 1.0).as_deref(),
        Some(format!("{dies} 52 experience. (+12 exp Rested bonus)").as_str())
    );
    assert_eq!(
        rested(52, 40, 2.0).as_deref(),
        Some(format!("{dies} 52 experience. (+12 exp Rested bonus, +20 group bonus)").as_str())
    );
    assert_eq!(
        rested(52, 40, 0.5).as_deref(),
        Some(format!("{dies} 52 experience. (+12 exp Rested bonus, -40 raid penalty)").as_str())
    );
}
