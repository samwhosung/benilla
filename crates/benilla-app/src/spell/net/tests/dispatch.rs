//! The spell's handlers as the whole client registers them, each packet run through the net
//! drain's dispatch: the cooldown lists a guid addresses, the inventory refusal's cast cancel and
//! the world entry's reset of the cast slots.

use super::*;
use crate::net::handlers::dispatch;
use crate::net::{ClientCommand, Guid};
use benilla_formats::SpellDisplay;

const ME: u64 = 10;

/// The whole client, headless, with us streamed in.
fn client() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = world
        .spawn((Guid(ME), SelfPlayer, ObjectStore::default()))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    (app, rx)
}

/// `SMSG_COOLDOWN_EVENT` and `SMSG_CLEAR_COOLDOWN` (`0x6e9670`) take the pet's list for the pet
/// bar's guid `[0xb714a0]` and the player's for any other (`0x6e9696`): a guid that is neither
/// ours nor the pet's reaches our list, and the pet's does not.
#[test]
fn a_cooldown_event_and_clear_on_any_guid_but_the_pets_reach_our_list() {
    const STEALTH: u32 = 1784;
    const STRANGER: u64 = 0x77;
    const PET: u64 = 0xf140_0000_0000_0042;
    // Bit 25 holds the record until its event (`0x6e3050`).
    let held = || SpellDisplay {
        recovery_ms: 10_000,
        attributes: 0x0200_0000,
        ..Default::default()
    };
    let (mut app, _rx) = client();
    let info = |app: &App| {
        app.world()
            .resource::<Cooldowns>()
            .info(STEALTH, 0, Some(&held()), Instant::now())
    };
    let seed = |app: &mut App| {
        app.world_mut().resource_mut::<Cooldowns>().start_spell(
            STEALTH,
            &held(),
            0,
            Instant::now(),
            None,
        );
    };

    seed(&mut app);
    app.world_mut()
        .resource_mut::<crate::ui_pet::PetBar>()
        .spells
        .pet_guid = PET;
    dispatch(
        app.world_mut(),
        vec![SessionEvent::CooldownEvent {
            spell_id: STEALTH,
            caster: PET,
        }],
    );
    assert!(!info(&app).enabled, "the pet's guid is the pet's list");

    dispatch(
        app.world_mut(),
        vec![SessionEvent::CooldownEvent {
            spell_id: STEALTH,
            caster: STRANGER,
        }],
    );
    let started = info(&app);
    assert!(
        started.enabled && started.remaining_ms > 0,
        "another guid's event starts our held record"
    );
    dispatch(
        app.world_mut(),
        vec![SessionEvent::ClearCooldown {
            spell_id: STEALTH,
            caster: STRANGER,
        }],
    );
    assert_eq!(info(&app).remaining_ms, 0, "and its clear removes it");
}

/// `SMSG_INVENTORY_CHANGE_FAILURE` naming the in-flight cast's item (`0x5e3a99`-`0x5e3aab`, the
/// guid `[0xcecab0]`) cancels it as `CancelCast(1, 0, 0x17)` (`0x5e3ab3`): the slot pops with no
/// packet and the failure's event fires. A refusal naming another item leaves the cast in flight.
#[test]
fn an_inventory_refusal_pops_the_cast_of_its_item_and_no_other() {
    const HEARTH: u32 = 8690;
    const STONE: u64 = 0x4000_0000_0000_0101;
    const BAG: u64 = 0x4000_0000_0000_0102;
    let (mut app, rx) = client();
    app.world_mut().insert_resource(Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [(HEARTH, SpellDisplay::default())].into_iter().collect(),
        ),
        ..Spells::empty_for_tests()
    });
    {
        let mut pending = app.world_mut().resource_mut::<PendingCast>();
        pending.fill_record();
        pending.fill_record_spell(HEARTH);
        pending.push_cast(
            HEARTH,
            Some(benilla_protocol::messages::ITEM_FLAG_PLAYERCAST),
            Some(STONE),
        );
    }
    let refusal = |item_guid| SessionEvent::InventoryFailure {
        reason: 0x16,
        required_level: None,
        item_guid,
        item2_guid: 0,
        bag_slot: 0xff,
    };

    dispatch(app.world_mut(), vec![refusal(BAG)]);
    assert_eq!(
        app.world().resource::<PendingCast>().current(),
        Some(HEARTH),
        "another item's refusal"
    );
    assert!(app.world().resource::<CastReports>().edges().is_empty());

    dispatch(app.world_mut(), vec![refusal(STONE)]);
    assert_eq!(app.world().resource::<PendingCast>().current(), None);
    assert!(matches!(
        app.world().resource::<CastReports>().edges()[..],
        [CastBarEdge::Failed]
    ));
    assert!(
        !rx.try_iter()
            .any(|c| matches!(c, ClientCommand::CancelCast { .. })),
        "`send` 0: no CMSG_CANCEL_CAST"
    );
}

/// Every world entry runs `Spell_C::SystemInitialize 0x6e7150` (`ClientInitializeGame 0x401570`
/// → `CGUnit_C::Initialize 0x6033c0`, `0x6038da`), which empties the in-flight slot and its save
/// (`0x6e72e7`-`0x6e7305`), the targeting word (`0x6e72e0`) and the auto-repeat spell
/// (`0x6e730b`): a cast, a queued strike or a cursor the last character left is gone.
#[test]
fn a_world_entry_empties_the_cast_slots() {
    const FIREBALL: u32 = 133;
    const HEROIC_STRIKE: u32 = 78;
    const BLIZZARD: u32 = 10;
    const AUTO_SHOT: u32 = 75;
    let (mut app, _rx) = client();
    {
        let world = app.world_mut();
        world.resource_mut::<QueuedMeleeSpell>().arm(HEROIC_STRIKE);
        // A cast pushed over the strike saves it (`0x6e4adf`).
        world
            .run_system_once(
                |mut slots: crate::spell::CastSlots, net: Res<NetCommands>| {
                    slots.core(&net).push(
                        FIREBALL,
                        crate::spell::Held::Cast {
                            item_flags: None,
                            item: None,
                        },
                    );
                },
            )
            .unwrap();
        world.resource_mut::<crate::spell::SpellTargeting>().enter(
            BLIZZARD,
            crate::spell::CastCommit::Spell,
            0x40,
        );
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
        assert_eq!(
            world.resource::<QueuedMeleeSpell>().saved(),
            Some(HEROIC_STRIKE)
        );
        assert_eq!(world.resource::<PendingCast>().current(), Some(FIREBALL));
    }

    dispatch(
        app.world_mut(),
        vec![SessionEvent::Connected {
            self_guid: ME,
            name: "Probethree".to_string(),
            billing: Default::default(),
            tutorial_flags: None,
            addon_info: None,
        }],
    );

    let world = app.world();
    assert_eq!(world.resource::<PendingCast>().current(), None);
    assert_eq!(world.resource::<QueuedMeleeSpell>().current(), None);
    assert_eq!(world.resource::<QueuedMeleeSpell>().saved(), None);
    assert!(!world.resource::<crate::spell::SpellTargeting>().active());
    assert_eq!(world.resource::<AutoRepeatActive>().0, None);
}

/// The entry itself, in packet order.
fn enter_world() -> SessionEvent {
    SessionEvent::Connected {
        self_guid: ME,
        name: "Probethree".to_string(),
        billing: Default::default(),
        tutorial_flags: None,
        addon_info: None,
    }
}

/// The same entry zeroes both modifier tables (`0x6e72c4`, `0x6e72d6`) and the class family
/// (`0x6e7317`) at the entry and in packet order: a modifier the same drain brings after it
/// stands.
#[test]
fn a_world_entry_clears_the_modifiers_in_packet_order() {
    use crate::spell::{SpellModifiers, OP_COST};
    /// A mage spell (family 3) with one family bit.
    fn mage(bit: u8) -> SpellDisplay {
        SpellDisplay {
            spell_family: 3,
            spell_family_flags: 1 << bit,
            ..Default::default()
        }
    }
    let (mut app, _rx) = client();
    {
        let mut mods = app.world_mut().resource_mut::<SpellModifiers>();
        mods.set_class_family(3);
        mods.set(true, 5, OP_COST, -30);
        assert_eq!(mods.apply(&mage(5), OP_COST, 100), 70);
    }

    dispatch(
        app.world_mut(),
        vec![
            enter_world(),
            SessionEvent::SpellModifier {
                flat: true,
                mask_bit: 6,
                op: OP_COST,
                value: -20,
            },
        ],
    );

    let mut mods = app.world_mut().resource_mut::<SpellModifiers>();
    assert_eq!(
        mods.apply(&mage(6), OP_COST, 100),
        100,
        "the class family is 0 until the avatar names it"
    );
    mods.set_class_family(3);
    assert_eq!(mods.apply(&mage(5), OP_COST, 100), 100, "cleared");
    assert_eq!(
        mods.apply(&mage(6), OP_COST, 100),
        80,
        "the packet after the entry"
    );
}

/// The entry zeroes the Feed Pet spell `[0xcecad8]` (`0x6e7311`); the next spell list latches it
/// again.
#[test]
fn a_world_entry_clears_the_feed_pet_spell() {
    let (mut app, _rx) = client();
    app.world_mut()
        .resource_mut::<crate::ui_action::LearnedAbilities>()
        .feed_pet = Some(6991);
    dispatch(app.world_mut(), vec![enter_world()]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_action::LearnedAbilities>()
            .feed_pet,
        None
    );
}
