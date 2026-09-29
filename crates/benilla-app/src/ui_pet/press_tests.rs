//! A pet bar spell press and the pet's global cooldown: the spell arm of `0x4bd1d0` starts it
//! (`0x4bd367`-`0x4bd36e`), whole client headless, with the press applied as the script call it is.

use std::collections::HashMap;
use std::time::Instant;

use bevy::ecs::schedule::Schedule;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{PetActionEntry, PET_ACT_ENABLED};

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::spell::Cooldowns;
use crate::ui_action::Spells;

use super::bar::feed_pet_bar;
use super::drain::PetPress;
use super::PetBar;

const ME: u64 = 0x10;
const PET: u64 = 0x2A;
/// `UNIT_FIELD_CHARMEDBY` and `UNIT_FIELD_CREATEDBY`, low words; the high words stay 0.
const CHARMEDBY: u16 = 10;
const CREATEDBY: u16 = 14;
const FLAGS: u16 = 46;
/// `UNIT_FIELD_AURA` slot 0 and `UNIT_FIELD_AURAFLAGS`, a nibble per slot.
const AURA: u16 = 47;
const AURAFLAGS: u16 = 95;

const CLAW: u32 = 16829;
const BITE: u32 = 17258;
const GROWL: u32 = 14918;
const COWER: u32 = 1742;

/// Claw's shape: no timer of its own, and the ordinary pet GCD, category 133 for 1500 ms.
fn claw() -> SpellDisplay {
    SpellDisplay {
        name: "Claw".into(),
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    }
}

/// Bite's shape: the same GCD pair, and a 10 s category timer of its own.
fn bite() -> SpellDisplay {
    SpellDisplay {
        name: "Bite".into(),
        category: 19,
        category_recovery_ms: 10_000,
        ..claw()
    }
}

/// Growl's shape: a 5 s category timer and no GCD pair.
fn growl() -> SpellDisplay {
    SpellDisplay {
        name: "Growl".into(),
        category: 82,
        category_recovery_ms: 5_000,
        ..Default::default()
    }
}

/// Cower's shape here: a toggle with an active icon, so a press with its aura up cancels it.
fn cower() -> SpellDisplay {
    SpellDisplay {
        name: "Cower".into(),
        active_icon_id: 122,
        ..claw()
    }
}

/// Our own pet: `CREATEDBY` us, no flags.
fn owned_pet() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (CREATEDBY, ME as u32),
        (CREATEDBY + 1, 0),
    ]))
}

struct Rig {
    app: App,
    commands: Receiver<ClientCommand>,
    /// The bar's feed as a schedule of its own, so its `Local` memory lives across runs.
    feed: Schedule,
}

/// The whole client, headless, with `PET` on the bar and `catalog` as the spell table. Slot `n`
/// (1-based) holds `slots[n - 1]` as an enabled spell word. `pet` is its object, or none
/// streamed.
fn rig(catalog: Vec<(u32, SpellDisplay)>, slots: &[u32], pet: Option<ObjectStore>) -> Rig {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, commands) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    if let Some(store) = pet {
        let entity = world.spawn((Guid(PET), store)).id();
        world.resource_mut::<GuidIndex>().0.insert(PET, entity);
    }
    world.insert_resource(Spells {
        catalog: SpellCatalog::from_displays(catalog.into_iter().collect::<HashMap<_, _>>()),
        ..Spells::empty_for_tests()
    });
    let mut bar = world.resource_mut::<PetBar>();
    bar.spells.pet_guid = PET;
    for (i, &spell_id) in slots.iter().enumerate() {
        bar.spells.bar[i] = PetActionEntry::from(spell_id | (u32::from(PET_ACT_ENABLED) << 24));
    }
    let script = benilla_ui::script::UiScript::new().expect("a VM");
    script
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PET_BAR_UPDATE")
            f:RegisterEvent("PET_BAR_UPDATE_COOLDOWN")
            f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
        "#,
        )
        .expect("the listener registers");
    world.insert_non_send_resource(script);
    let mut feed = Schedule::default();
    feed.add_systems(feed_pet_bar);
    let mut rig = Rig {
        app,
        commands,
        feed,
    };
    // The bar arriving is `PET_BAR_UPDATE`'s; every test reads what a press adds to it.
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the bar arrives");
    rig
}

impl Rig {
    /// The bar's feed for one frame, and the events it fired.
    fn frame(&mut self) -> Vec<String> {
        self.feed.run(self.app.world_mut());
        let mut script = self
            .app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        script.resolve();
        let seen = script.eval::<Vec<String>>("return SEEN").unwrap();
        script.run("SEEN = {}").unwrap();
        seen
    }

    /// `CastPetAction(slot)`, applied.
    fn press(&mut self, slot: u32) {
        self.app
            .world_mut()
            .run_system_once(move |mut press: PetPress| press.press_slot(slot))
            .expect("the press applies as a one-shot system");
    }

    /// The commands sent so far.
    fn sent(&self) -> Vec<ClientCommand> {
        self.commands.try_iter().collect()
    }

    /// `GetPetActionCooldown(slot)`, off what the last feed pushed: `(start, duration, enable)`.
    fn cooldown(&mut self, slot: u32) -> (f64, f64, i32) {
        self.app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval(&format!("return GetPetActionCooldown({slot})"))
            .unwrap()
    }

    /// What the pet's own list reads for `spell_id`, as the bar's reader asks it.
    fn pet_reads(&self, spell_id: u32, spell: &SpellDisplay) -> (u32, u32) {
        let info = self.app.world().resource::<PetBar>().cooldowns.info(
            spell_id,
            0,
            Some(spell),
            Instant::now(),
        );
        (info.remaining_ms, info.duration_ms)
    }

    /// Whether the pet's own list took no record at all, whichever category a reader asks in.
    fn pet_list_untouched(&self) -> bool {
        self.app.world().resource::<PetBar>().cooldowns.generation == 0
    }

    /// Whether the player's own list was touched at all.
    fn player_list_untouched(&self) -> bool {
        self.app.world().resource::<Cooldowns>().generation == 0
    }
}

fn pet_actions(sent: &[ClientCommand]) -> usize {
    sent.iter()
        .filter(|c| matches!(c, ClientCommand::PetAction { .. }))
        .count()
}

/// `0x4bd367`: the press starts the GCD on the pet's list before it sends, and every slot on the
/// bar sharing the category reads it through `GetPetActionCooldown`.
#[test]
fn a_spell_press_starts_the_pets_gcd_for_every_slot_sharing_its_category() {
    let mut rig = rig(
        vec![(CLAW, claw()), (BITE, bite()), (GROWL, growl())],
        &[CLAW, BITE, GROWL],
        Some(owned_pet()),
    );

    rig.press(1);

    assert_eq!(pet_actions(&rig.sent()), 1, "and the press still sends");
    let (remaining, duration) = rig.pet_reads(CLAW, &claw());
    assert_eq!(duration, 1500, "the spell's StartRecoveryTime");
    assert!(remaining > 0 && remaining <= 1500, "running: {remaining}");
    assert_eq!(
        rig.pet_reads(BITE, &bite()).1,
        1500,
        "a spell sharing category 133 reads the same GCD, its own timers unstarted"
    );
    assert_eq!(
        rig.pet_reads(GROWL, &growl()),
        (0, 0),
        "a spell outside the category reads nothing"
    );
    assert!(rig.player_list_untouched(), "list 1 is the pet's, not ours");

    // `0x6e2e8e`: one `PET_BAR_UPDATE_COOLDOWN`, no bar update with it.
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE_COOLDOWN"]);
    let (start, duration, enable) = rig.cooldown(1);
    assert!(start > 0.0 && (duration - 1.5).abs() < 1e-9 && enable == 1);
    let (_, duration, enable) = rig.cooldown(2);
    assert!(
        (duration - 1.5).abs() < 1e-9 && enable == 1,
        "the second slot sweeps the GCD: {duration}"
    );
    assert_eq!(rig.cooldown(3), (0.0, 0.0, 1), "Growl shows no sweep");
}

/// `0x6e2e0f`-`0x6e2e21`: a spell with neither `StartRecovery*` field starts nothing.
#[test]
fn a_spell_without_a_gcd_pair_starts_none() {
    let free = || SpellDisplay {
        start_recovery_category: 0,
        start_recovery_ms: 0,
        ..claw()
    };
    let mut rig = rig(vec![(CLAW, free())], &[CLAW], Some(owned_pet()));

    rig.press(1);

    assert_eq!(pet_actions(&rig.sent()), 1);
    assert!(rig.pet_list_untouched());
    assert_eq!(rig.pet_reads(CLAW, &claw()), (0, 0));
    assert!(rig.frame().is_empty(), "no cooldown edge either");
}

/// `0x4bd35e`-`0x4bd365`: a possessed unit's press returns into the generic cast entry
/// (`0x6e4b60`), which starts its own GCD at its send (`0x6e58fb`), so this arm starts none.
#[test]
fn a_possessed_units_press_starts_no_pet_gcd() {
    let charmed = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (CHARMEDBY, ME as u32),
        (CHARMEDBY + 1, 0),
        (FLAGS, super::drain::UNIT_FLAG_POSSESSED),
    ]));
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], Some(charmed));

    rig.press(1);

    assert!(rig.pet_list_untouched());
    assert!(rig.frame().is_empty());
}

/// `0x4bd355`: `AttributesEx4 & 0x20` takes the same route out of the arm.
#[test]
fn a_client_targeted_spell_press_starts_no_pet_gcd() {
    let ground = || SpellDisplay {
        attributes_ex4: 0x20,
        ..claw()
    };
    let mut rig = rig(vec![(CLAW, ground())], &[CLAW], Some(owned_pet()));

    rig.press(1);

    assert!(rig.pet_list_untouched());
    assert!(rig.frame().is_empty());
}

/// `0x4bd24f`: a press on a running aura cancels it and returns before the arm.
#[test]
fn an_aura_cancel_press_starts_no_pet_gcd() {
    let running = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (CREATEDBY, ME as u32),
        (CREATEDBY + 1, 0),
        (AURA, COWER),
        (AURAFLAGS, 0x3),
    ]));
    let mut rig = rig(vec![(COWER, cower())], &[COWER], Some(running));

    rig.press(1);

    let sent = rig.sent();
    assert_eq!(pet_actions(&sent), 0, "the cancel sends no pet action");
    assert!(sent.iter().any(|c| matches!(
        c,
        ClientCommand::PetCancelAura {
            spell_id: COWER,
            ..
        }
    )));
    assert!(rig.pet_list_untouched());
    assert!(rig.frame().is_empty());

    // The same spell with its aura down is an ordinary press, and starts the GCD.
    let mut rig = self::rig(vec![(COWER, cower())], &[COWER], Some(owned_pet()));
    rig.press(1);
    assert_eq!(rig.pet_reads(COWER, &cower()).1, 1500);
}

/// `0x4bd34f`: the arm leaves before the GCD when the pet object does not resolve.
#[test]
fn a_press_with_no_pet_object_starts_no_pet_gcd() {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], None);

    rig.press(1);

    assert!(rig.pet_list_untouched());
    assert!(rig.frame().is_empty());
}

/// Op 21 applies to `StartRecoveryTime` first (`0x6e2e2c`), the player's table for a pet's
/// spell as for ours.
#[test]
fn the_pets_gcd_takes_the_players_op_21() {
    let shaved = || SpellDisplay {
        spell_family: 9,
        spell_family_flags: 1,
        ..claw()
    };
    let mut rig = rig(vec![(CLAW, shaved())], &[CLAW], Some(owned_pet()));
    {
        let mut mods = rig
            .app
            .world_mut()
            .resource_mut::<crate::spell::SpellModifiers>();
        mods.set_class_family(9);
        mods.set(true, 0, crate::spell::OP_GCD, -500);
    }

    rig.press(1);

    assert_eq!(rig.pet_reads(CLAW, &shaved()).1, 1000);
}
