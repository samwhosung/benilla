//! `ActionHasRange` (`0x4e5810`) and `IsActionInRange` (`0x4e56f0`) through
//! [`super::feed_action_state`] and a real Lua VM: both resolve the slot with `0x4e5a50` and test
//! the spell it stands for.

use super::*;
use crate::spell::AutoRepeatActive;
use crate::target::Selection;
use benilla_formats::{SpellDisplay, SpellRange};
use benilla_protocol::messages::{ActionButton, ItemUseSpell};
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;

/// The player's guid.
const ME: u64 = 0x10;
/// The selected unit's guid.
const SELECTED: u64 = 0xB2;
/// The spell every slot here stands for, aimed at an enemy (implicit target 6), so the pre-check
/// compares (`0x6e45f1`) and a selection answers 1 (`0x4e579c`).
const BOLT: u32 = 133;
/// Its `SpellRange.dbc` row.
const BOLT_RANGE: u32 = 4;
/// An item whose first trigger-0 block casts [`BOLT`].
const GRENADE: u32 = 4390;

/// What one pass of the feed answers for slot 1: `ActionHasRange(1)` and `IsActionInRange(1)`.
fn range_answers(
    button: ActionButton,
    items: Items,
    row: SpellRange,
    distance: f32,
) -> (bool, Option<i64>) {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    let mut slots = crate::ui_action::ActionSlots::default();
    slots.buttons.insert(button.slot, button);
    let bolt = SpellDisplay {
        range_index: BOLT_RANGE,
        effect_implicit_target_a: [6, 0, 0],
        ..Default::default()
    };
    app.insert_resource(PlayerActions::default())
        .insert_resource(slots)
        .insert_resource(crate::ui_macro::MacroBoundSpells::default())
        .insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(BOLT, bolt)].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: benilla_formats::SpellRangeCatalog::from_rows(
                [(BOLT_RANGE, row)].into_iter().collect(),
            ),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        })
        .init_resource::<Cooldowns>()
        .init_resource::<crate::spell::SpellModifiers>()
        .init_resource::<crate::ui_script::UiClock>()
        .init_resource::<AutoRepeatActive>()
        .init_resource::<crate::creature_anim::UnitEdits>()
        .add_message::<crate::player::StandStateRequest>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::net::WorldEnterCascadeMessage>()
        .init_resource::<crate::item_lane::ItemLane>()
        .insert_resource(crate::net::SelfGuid(Some(ME)))
        .init_resource::<crate::spell::AutoSelfCast>()
        .init_resource::<crate::spell::PendingCast>()
        .init_resource::<crate::spell::QueuedMeleeSpell>()
        .init_resource::<crate::spell::ActiveChannel>()
        .init_resource::<crate::spell::SpellTargeting>()
        .init_resource::<crate::spell::HeldForPick>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::net::Reputations>()
        .insert_resource(items)
        .insert_resource(NetCommands(tx));
    // A player under its own control, alive, with mana; an attackable creature at `distance`.
    app.world_mut().spawn((
        SelfPlayer,
        Transform::default(),
        ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x19),
            (22, 100),
            (23, 500),
            (46, 0x8),
        ])),
    ));
    let selected = app
        .world_mut()
        .spawn((
            Transform::from_xyz(distance, 0.0, 0.0),
            ObjectStore(ObjectFields::from_pairs(&[(2, 0x9), (22, 100)])),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(SELECTED, selected);
    app.insert_resource(Selection {
        target: Some(selected),
        guid: Some(SELECTED),
        last: None,
        lit: None,
    });
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.init_resource::<ActionEvents>()
        .init_resource::<EnterUsable>()
        .init_resource::<ActionStateMemory>()
        .add_systems(Update, feed_action_state);
    app.update();
    app.world()
        .non_send_resource::<UiScript>()
        .eval::<(bool, Option<i64>)>("return ActionHasRange(1) == 1, IsActionInRange(1)")
        .unwrap()
}

fn spell_slot() -> ActionButton {
    ActionButton {
        slot: 0,
        action: BOLT,
        kind: ACTION_KIND_SPELL,
    }
}

fn item_slot() -> ActionButton {
    ActionButton {
        slot: 0,
        action: GRENADE,
        kind: ACTION_KIND_ITEM,
    }
}

/// A grenade's template, whose on-use spell is its first trigger-0 block (`0x4e5ae0`).
fn grenade() -> Items {
    let mut items = Items::default();
    items.insert_template(
        GRENADE,
        Some(benilla_protocol::messages::ItemInfo {
            use_spell: Some(ItemUseSpell {
                spell_id: BOLT,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }),
            ..crate::items::test_template("Iron Grenade")
        }),
    );
    items
}

fn row(max: f32) -> SpellRange {
    SpellRange {
        min: 0.0,
        max,
        flags: 0,
    }
}

/// `ActionHasRange` (`0x4e5810`) tests `|min|` and `|max|` with `fcomp [0x8029d4]` and
/// `test ah,5` (`0x4e5885`, `0x4e5898`): a range not below 0x34800000, 2^-22, is a range, so 2^-22
/// itself counts and 1.5 * 2^-23, above `f32::EPSILON`, does not.
#[test]
fn action_has_range_tests_against_two_to_the_minus_22() {
    let at_bound = f32::from_bits(0x3480_0000);
    assert_eq!(at_bound, 2f32.powi(-22));
    assert!(
        range_answers(spell_slot(), Items::default(), row(at_bound), 1.0).0,
        "equal to the constant is a range"
    );
    let below = 1.5 * f32::EPSILON;
    assert!(below > f32::EPSILON && below < at_bound);
    assert!(
        !range_answers(spell_slot(), Items::default(), row(below), 1.0).0,
        "between 2^-23 and 2^-22 is no range"
    );
}

/// An item slot resolves through `GetActionSlotSpell` (`0x4e5a50`) to its template's first
/// trigger-0 spell (`0x4e5ae0`) with book 0, which both queries require (`0x4e584b`, `0x4e5757`):
/// a grenade casting a 0-35 spell has a range and reads in range at 37.5 yards (padded by both
/// reaches) and out at 38.5, as the same spell on a spell slot does.
#[test]
fn an_item_slot_answers_range_by_its_on_use_spell() {
    for button in [spell_slot(), item_slot()] {
        assert_eq!(
            range_answers(button, grenade(), row(35.0), 37.5),
            (true, Some(1)),
            "in range: {button:?}"
        );
        assert_eq!(
            range_answers(button, grenade(), row(35.0), 38.5),
            (true, Some(0)),
            "out of range: {button:?}"
        );
    }
}

/// An item whose template is not cached resolves to spell 0 (`0x4e5ae0` fails the template
/// fetch), whose missing record answers no range and nil (`0x4e5846`, `0x4e5752`).
#[test]
fn an_uncached_item_answers_no_range() {
    assert_eq!(
        range_answers(item_slot(), Items::default(), row(35.0), 10.0),
        (false, None)
    );
}
