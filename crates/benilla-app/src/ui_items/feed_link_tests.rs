//! A bag item's link as [`super::feed_containers`] pushes it, read back through
//! `GetContainerItemLink`: `0x52ae00` (called at `0x4f997f`) passes the permanent enchant, the
//! random property id and the property seed to `0x52adb0`, all three 0 for a wrapped item and the
//! enchant 0 for a charter.

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::items::Items;
use crate::spell::Cooldowns;

const SWORD: u32 = 15268;
/// `ITEM_FIELD_FLAGS`, `ITEM_FIELD_ENCHANTMENT` slot 0's id, `ITEM_FIELD_PROPERTY_SEED` and
/// `ITEM_FIELD_RANDOM_PROPERTIES_ID`: the block's `+0x3c`, `+0x40`, `+0x94` and `+0x98`.
const FLAGS: u16 = 21;
const ENCHANT_0: u16 = 22;
const SEED: u16 = 43;
const ROLL: u16 = 44;

/// The link of the item in backpack slot 1, whose descriptor is `pairs` and whose template's flags
/// are `template_flags`.
fn bag_link(pairs: &[(u16, u32)], template_flags: u32) -> String {
    let now = std::time::Instant::now();
    let mut app = App::new();
    app.init_resource::<Items>()
        .init_resource::<Cooldowns>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::ui_items::EquipErrors>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::ui_items::net::BagOpens>()
        .init_resource::<super::BagUpdates>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<crate::ui_petition::PetitionState>()
        .insert_resource(crate::ui_script::UiClock {
            anchor: now,
            ui_now: 1000.0,
        })
        .add_systems(Update, super::feed_containers);
    let mut template = crate::items::test_template("Bloodrazor");
    template.quality = 3;
    template.flags = template_flags;
    app.world_mut()
        .resource_mut::<Items>()
        .insert_template(SWORD, Some(template));
    let guid = u64::from(benilla_protocol::guid::HIGH_ITEM) << 48 | 0xA1;
    let mut fields = vec![(2, 0x3), (3, SWORD), (14, 1)];
    fields.extend_from_slice(pairs);
    let sword = app
        .world_mut()
        .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&fields)))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(guid, sword);
    app.world_mut().spawn((
        crate::net::SelfPlayer,
        crate::net::ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x19),
            (532, guid as u32),
            (533, (guid >> 32) as u32),
        ])),
    ));
    let mut script = UiScript::new().unwrap();
    script.set_now(1000.0);
    app.insert_non_send_resource(script);
    app.update();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<String>("return GetContainerItemLink(0, 1)")
        .unwrap()
}

/// `0x52ae00` reads the enchant at `+0x40` (`0x52aeb2`), the id at `+0x98` and the seed at
/// `+0x94` (`0x52ae88`, `0x52ae8e`), and `0x52adb0` prints them in that order.
#[test]
fn a_bag_link_carries_the_enchant_the_roll_and_the_seed() {
    let link = bag_link(&[(ENCHANT_0, 1900), (ROLL, 862), (SEED, 1234)], 0);
    assert_eq!(
        link,
        "|cff0070dd|Hitem:15268:1900:862:1234|h[Bloodrazor]|h|r"
    );
}

/// A wrapped item (`ITEM_FIELD_FLAGS & 0x8`, `0x52ae66`-`0x52ae80`) passes 0 for all three.
#[test]
fn a_wrapped_bag_link_carries_no_numbers() {
    let link = bag_link(
        &[(FLAGS, 0x8), (ENCHANT_0, 1900), (ROLL, 862), (SEED, 1234)],
        0,
    );
    assert_eq!(link, "|cff0070dd|Hitem:15268:0:0:0|h[Bloodrazor]|h|r");
}

/// A charter's template flag `0x2000` (`test ah,0x20` at `0x52aeaa`) zeroes the enchant: its
/// slot 0 holds the petition id. The roll and the seed are read as for any item.
#[test]
fn a_charter_link_carries_no_enchant() {
    let link = bag_link(&[(ENCHANT_0, 77), (ROLL, 5), (SEED, 9)], 0x2000);
    assert_eq!(link, "|cff0070dd|Hitem:15268:0:5:9|h[Bloodrazor]|h|r");
}
