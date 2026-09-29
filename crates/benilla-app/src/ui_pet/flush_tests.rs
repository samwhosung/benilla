//! The flush behind every mutation of the pet's cooldown list, whole client headless: each packet
//! that reaches the list arrives as the session event it is, and the pet's feeds run after it.
//! `0x4b31b0` fires `ACTIONBAR_UPDATE_COOLDOWN` then `SPELL_UPDATE_COOLDOWN` and `0x4bce90` fires
//! `PET_BAR_UPDATE_COOLDOWN`; none of the callers fires `PET_BAR_UPDATE`.

use std::time::Duration;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{PetSpellCooldown, PetSpells};
use benilla_protocol::SessionEvent;

use super::press_tests::{claw, growl, listen, listening_vm, rig, Rig, CLAW, GROWL, ME, PET};
use super::PetBar;
use crate::net::ObjectStore;

/// The three events, in the reference's order.
const FLUSH: [&str; 3] = [
    "ACTIONBAR_UPDATE_COOLDOWN",
    "SPELL_UPDATE_COOLDOWN",
    "PET_BAR_UPDATE_COOLDOWN",
];

/// `UNIT_FIELD_SUMMONEDBY`, low word, the owner test the GO's pet leg reads (`0x6e859a`).
const SUMMONEDBY: u16 = 12;

/// Our own pet by the field the GO's pet leg reads: `SUMMONEDBY` us.
fn summoned_pet() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (SUMMONEDBY, ME as u32),
        (SUMMONEDBY + 1, 0),
    ]))
}

/// The rig's VM.
fn ui(rig: &mut Rig) -> bevy::ecs::change_detection::Mut<'_, benilla_ui::script::UiScript> {
    rig.app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
}

/// Claw as the pet book lists it: named, iconed, and with the ordinary pet GCD.
fn claw_book_spell() -> SpellDisplay {
    SpellDisplay {
        icon: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
        ..claw()
    }
}

/// A rig with Growl (a 5 s recovery of its own) on slot 1 of `PET`'s bar.
fn growl_rig() -> Rig {
    rig(vec![(GROWL, growl())], &[GROWL], Some(summoned_pet()))
}

/// `SMSG_SPELL_GO` of `spell_id` by `caster`, as the session decodes a plain cast.
fn go(caster: u64, spell_id: u32) -> SessionEvent {
    SessionEvent::SpellGo {
        caster,
        spell_id,
        cast_flags: 0,
        hits: vec![],
        misses: vec![],
        target: None,
        go_target: None,
        dest: None,
        ammo_display_id: None,
        item_caster: None,
    }
}

impl Rig {
    /// The packets, handled as the session hands them to the client.
    fn receive(&mut self, events: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), events);
    }

    fn generation(&self) -> u64 {
        self.app.world().resource::<PetBar>().cooldowns.generation
    }
}

/// `0x6e85f7` inserts into the pet's list, `0x6e85fc` and `0x6e8601` flush. The bar takes no
/// `PET_BAR_UPDATE` from it, which the spellbook's pet tab would take as a repaint of its own.
#[test]
fn a_pets_spell_go_fires_the_flush_and_no_bar_update() {
    let mut rig = growl_rig();

    rig.receive(vec![go(PET, GROWL)]);

    assert_eq!(rig.frame(), FLUSH);
    assert!(rig.frame().is_empty(), "once, not every frame");
}

/// The GO's owner test (`0x6e859a`) admits our pet and nobody else's: no insert, no flush.
#[test]
fn a_strangers_pet_go_flushes_nothing() {
    let strangers = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (SUMMONEDBY, 0x99),
        (SUMMONEDBY + 1, 0),
    ]));
    let mut rig = rig(vec![(GROWL, growl())], &[GROWL], Some(strangers));

    rig.receive(vec![go(PET, GROWL)]);

    assert_eq!(rig.generation(), 0);
    assert!(rig.frame().is_empty());
}

/// `0x6e95b0`, `0x6e95b9`: the flush follows `SMSG_SPELL_COOLDOWN` for the pet's guid, and for it
/// alone; the player's own list flushes through the action state's feed.
#[test]
fn a_pet_addressed_spell_cooldown_fires_the_flush() {
    let mut rig = growl_rig();

    rig.receive(vec![SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(GROWL, 3000)],
    }]);
    assert!(
        rig.frame().is_empty(),
        "the player's list is not the pet's flush"
    );

    rig.receive(vec![SessionEvent::SpellCooldowns {
        caster: PET,
        cooldowns: vec![(GROWL, 3000)],
    }]);
    assert_eq!(rig.frame(), FLUSH);
}

/// `0x6e3071`-`0x6e3080` (`SMSG_COOLDOWN_EVENT`, `SMSG_CLEAR_COOLDOWN`) and `0x6e9712`-`0x6e971c`
/// (`SMSG_COOLDOWN_CHEAT`): each flushes for the pet's list as for a spell cast.
#[test]
fn the_pets_event_clear_and_cheat_packets_fire_the_flush() {
    // Attributes bit 25: the record parks until the event.
    let parked = || SpellDisplay {
        attributes: 1 << 25,
        ..growl()
    };
    let mut rig = rig(vec![(GROWL, parked())], &[GROWL], Some(summoned_pet()));
    let park = |rig: &mut Rig| {
        rig.receive(vec![SessionEvent::SpellCooldowns {
            caster: PET,
            cooldowns: vec![(GROWL, 3000)],
        }]);
        assert_eq!(rig.frame(), FLUSH, "parked");
    };

    park(&mut rig);
    rig.receive(vec![SessionEvent::CooldownEvent {
        spell_id: GROWL,
        caster: PET,
    }]);
    assert_eq!(rig.frame(), FLUSH, "the parked timers start");

    rig.receive(vec![SessionEvent::ClearCooldown {
        spell_id: GROWL,
        caster: PET,
    }]);
    assert_eq!(rig.frame(), FLUSH, "the record goes");

    park(&mut rig);
    rig.receive(vec![SessionEvent::CooldownCheat { caster: PET }]);
    assert_eq!(rig.frame(), FLUSH, "the GM wipe");
}

/// The frame's mutations are one flush for the frame, as the player's list has them.
#[test]
fn several_mutations_in_a_frame_fire_the_flush_once() {
    let mut rig = growl_rig();

    rig.receive(vec![
        go(PET, GROWL),
        SessionEvent::SpellCooldowns {
            caster: PET,
            cooldowns: vec![(GROWL, 3000)],
        },
    ]);

    assert_eq!(rig.frame(), FLUSH);
}

/// `SMSG_PET_SPELLS` seeds the list through `0x6e2c60`, which flushes nothing, and ends in
/// `SetPet`'s `SignalEvent(0x161)` (`0x4bc90c`) whatever the packet changed: a packet that moves
/// only the cooldowns is one `PET_BAR_UPDATE`, not the flush.
#[test]
fn the_pet_spells_packet_signals_the_bar_and_never_flushes() {
    let mut rig = growl_rig();
    let same_bar = PetSpells {
        cooldowns: vec![PetSpellCooldown {
            spell_id: GROWL,
            category: 82,
            spell_cd_ms: 4000,
            category_cd_ms: 0,
        }],
        ..rig.app.world().resource::<PetBar>().spells.clone()
    };

    rig.receive(vec![SessionEvent::PetSpells(Box::new(same_bar))]);
    assert_eq!(
        rig.frame(),
        ["PET_BAR_UPDATE"],
        "the bar is what it was and its cooldown is new: the packet's own signal repaints it"
    );
    let (_, duration, enable) = rig.cooldown(1);
    assert!(
        (duration - 4.0).abs() < 1e-9 && enable == 1,
        "and the seeded cooldown reached the bar: {duration}"
    );
    assert_eq!(rig.generation(), 0, "a seed is no mutation of the flush");

    // The teardown clears the list and flushes nothing either.
    rig.receive(vec![SessionEvent::PetSpells(Box::default())]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"]);
}

/// A reset moves no counter back: a mutation, the teardown, a new bar, and the next mutation
/// flushes once. A counter that fell back to 0 would read as a mutation at the teardown.
#[test]
fn a_teardown_and_a_new_bar_do_not_hide_the_next_flush() {
    let mut rig = growl_rig();
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
    let held = rig.generation();

    rig.receive(vec![SessionEvent::PetSpells(Box::default())]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the teardown");
    assert_eq!(rig.generation(), held, "the counter is where it was");

    let mut bar = rig.app.world().resource::<PetBar>().spells.clone();
    bar.pet_guid = PET;
    bar.bar[0] = benilla_protocol::messages::PetActionEntry::from(
        GROWL | (u32::from(benilla_protocol::messages::PET_ACT_ENABLED) << 24),
    );
    rig.receive(vec![SessionEvent::PetSpells(Box::new(bar))]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the new bar");

    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
}

/// The stock `Cooldown` frame ends its own sweep, and the reference flushes no event for the end
/// of a cooldown: an elapsed record moves neither the list nor the events.
#[test]
fn a_natural_expiry_fires_nothing() {
    let brief = || SpellDisplay {
        name: "Brief".into(),
        recovery_ms: 30,
        ..Default::default()
    };
    let mut rig = rig(vec![(GROWL, brief())], &[GROWL], Some(summoned_pet()));
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
    assert!(rig.cooldown(1).1 > 0.0, "running");

    std::thread::sleep(Duration::from_millis(80));

    assert!(
        rig.frame().is_empty(),
        "its end is the frame's, not an event"
    );
    assert_eq!(
        rig.cooldown(1).1,
        0.0,
        "and the sweep is gone from the push"
    );
}

/// A new VM meets the list as it stands: the flush is for a mutation, not for a load.
#[test]
fn a_new_vm_is_not_flushed_for_a_list_that_already_moved() {
    let mut rig = growl_rig();
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);

    // The interface reloads: a new VM, the bar pushed into it afresh.
    rig.app.world_mut().insert_non_send_resource(listening_vm());
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the bar, and no flush");

    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH, "the next mutation is a flush");
}

/// The flush reads what the feeds pushed: it runs after the bar's feed and the book's, which
/// push the pet's cooldown triples the handlers ask for, and (declared through the cooldown
/// events) after the player's own flush. Skips where bevy carries no system names.
#[test]
fn the_flush_runs_after_every_feed_that_pushes_a_pet_cooldown() {
    use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};
    use bevy::prelude::Update;
    use std::collections::{HashMap, HashSet};

    let mut app = headless_client();
    let c = census(&mut app, Update, SyncPoints::Declared);
    let named = |needle: &str| {
        let hits: Vec<_> = c
            .systems
            .iter()
            .filter(|(_, s)| s.name.ends_with(needle))
            .map(|(k, _)| *k)
            .collect();
        (!hits.is_empty()).then(|| {
            assert_eq!(hits.len(), 1, "one system named {needle}");
            hits[0]
        })
    };
    let Some(fire) = named("::fire_pet_cooldown_events") else {
        eprintln!("skipped: this build carries no system names");
        return;
    };
    let mut after: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        after.entry(*a).or_default().push(*b);
    }
    let runs_before = |first| {
        let (mut seen, mut stack) = (HashSet::new(), vec![first]);
        while let Some(n) = stack.pop() {
            if n == fire {
                return true;
            }
            if seen.insert(n) {
                stack.extend(after.get(&n).into_iter().flatten().copied());
            }
        }
        false
    };
    for feed in ["::feed_pet_bar", "::feed_pet_book", "::feed_action_state"] {
        let key = named(feed).unwrap_or_else(|| panic!("{feed} is scheduled"));
        assert!(runs_before(key), "{feed} runs before the pet's flush");
    }
}

/// The stock spellbook (`SpellBookFrame.lua:209`-`231`): a `SpellButton` redraws on
/// `SPELL_UPDATE_COOLDOWN`, and on `PET_BAR_UPDATE` while the pet tab is open, and never hears
/// `PET_BAR_UPDATE_COOLDOWN`. So the GCD a pet-bar press starts sweeps the pet tab's button
/// through the flush's `SPELL_UPDATE_COOLDOWN` alone.
#[test]
fn a_pet_gcd_sweeps_the_spellbooks_pet_tab_button() {
    benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::{PetActionEntry, PET_ACT_ENABLED};

    let mut rig = rig(
        vec![(CLAW, claw_book_spell())],
        &[CLAW],
        Some(summoned_pet()),
    );
    {
        let world = rig.app.world_mut();
        // Our class, so the book reads a token (`0x4b4463`); no `ChrClasses` row, so "PET".
        world.spawn((
            crate::net::SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(36, 3 << 8)])),
        ));
        world.insert_resource(crate::chr_classes::ChrClassTable(Default::default()));
        world.resource_mut::<PetBar>().spells.spells = vec![PetActionEntry::from(
            CLAW | (u32::from(PET_ACT_ENABLED) << 24),
        )];
    }

    let mut script = benilla_ui::script::UiScript::new().unwrap();
    script.set_screen_size(1024.0, 768.0);
    let failures = crate::ui_script::load_default_ui(&script);
    assert!(failures.is_empty(), "load failures: {failures:?}");
    listen(&script);
    rig.app.world_mut().insert_non_send_resource(script);

    // The book arrives, and the window opens on its pet tab.
    rig.frame();
    let mut s = ui(&mut rig);
    s.run("ToggleSpellBook(BOOKTYPE_PET)").unwrap();
    s.resolve();
    assert!(
        s.eval::<bool>("return SpellBookFrame.bookType == BOOKTYPE_PET")
            .unwrap(),
        "the pet tab is open"
    );
    assert_eq!(
        s.eval::<String>("return SpellButton1SpellName:GetText()")
            .unwrap(),
        "Claw"
    );
    assert!(
        !s.eval::<bool>("return SpellButton1Cooldown:IsShown()")
            .unwrap(),
        "no cooldown yet"
    );

    // A press on the bar's Claw starts the pet's GCD.
    rig.press(1);
    rig.frame();

    let mut s = ui(&mut rig);
    s.resolve();
    assert!(
        s.eval::<bool>("return SpellButton1Cooldown:IsShown()")
            .unwrap(),
        "the GCD sweeps the spellbook's button"
    );
    assert!(
        (s.eval::<f64>("return SpellButton1Cooldown.duration")
            .unwrap()
            - 1.5)
            .abs()
            < 1e-9,
        "and it is the 1.5 s the press started"
    );
    let errors = s.errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
}
