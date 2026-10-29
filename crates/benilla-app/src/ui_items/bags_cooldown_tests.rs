//! The bags' live read ([`crate::ui_items::BagLive`]) against the cooldown list's edges, through
//! a real Lua VM.

use benilla_protocol::messages::{ItemUseSpell, SpellCooldown};
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::items::Items;
use crate::spell::{CategoryPad, Cooldowns, SpellModifiers};

/// The Hearthstone's entry and its on-use spell.
const HEARTHSTONE: u32 = 6948;
const RECALL: u32 = 8690;

/// `StartCooldown 0x6e2c60` inserts with no flush (`0x6e2dcc`), so a login's
/// `SMSG_INITIAL_SPELLS` seed of an item's cooldown moves no [`Cooldowns::generation`]; the bags
/// still read it, as `GetContainerItemCooldown` queries the list itself (`0x4f9a7f` →
/// `0x6e2ed0`).
#[test]
fn an_initial_item_cooldown_alone_reaches_the_bags() {
    let now = std::time::Instant::now();
    let mut app = App::new();
    app.init_resource::<Items>()
        .init_resource::<Cooldowns>()
        .init_resource::<crate::net::GuidIndex>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::sound::MessageSounds>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<crate::ui_petition::PetitionRecords>()
        .insert_resource(crate::ui_script::HostClock(crate::ui_script::UiClock {
            anchor: now,
            ui_now: 1000.0,
        }));
    app.world_mut().resource_mut::<Items>().insert_template(
        HEARTHSTONE,
        Some(benilla_protocol::messages::ItemInfo {
            use_spell: Some(ItemUseSpell {
                spell_id: RECALL,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }),
            ..crate::items::test_template("Hearthstone")
        }),
    );
    // The Hearthstone in backpack slot 1 (`PLAYER_FIELD_PACK_SLOT_1`, 532).
    let guid = u64::from(benilla_protocol::guid::HIGH_ITEM) << 48 | 0xA1;
    let stone = app
        .world_mut()
        .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, HEARTHSTONE),
            (14, 1),
        ])))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(guid, stone);
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
    let duration = |app: &mut App| {
        crate::ui_items::bags::eval_with_bags::<f64>(
            app.world_mut(),
            "local _, d = GetContainerItemCooldown(0, 1) return d",
        )
    };
    assert_eq!(duration(&mut app), 0.0, "bagged, cold");

    // Half of a one-minute recall still to run at login.
    let wire = SpellCooldown {
        spell_id: RECALL as u16,
        item_id: HEARTHSTONE as u16,
        category: 0,
        spell_cd_ms: 30_000,
        category_cd_ms: 0,
    };
    let recall = benilla_formats::SpellDisplay {
        recovery_ms: 60_000,
        ..Default::default()
    };
    app.world_mut().resource_mut::<Cooldowns>().seed_initial(
        &wire,
        Some(&recall),
        None,
        &SpellModifiers::default(),
        CategoryPad::NoPlayer,
        now,
    );
    assert_eq!(duration(&mut app), 60.0, "the seeded record, read");
}
