//! A pet bar spell press and the pet's global cooldown: the spell arm of `0x4bd1d0` starts it
//! (`0x4bd367`-`0x4bd36e`), whole client headless, with the press applied as the script call it is.
//! The rig is shared with `flush_tests`, whose packets reach the same list.

use std::collections::HashMap;
use std::time::Instant;

use bevy::ecs::schedule::Schedule;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{
    PetActionEntry, PET_ACT_COMMAND, PET_ACT_ENABLED, PET_ACT_REACTION, PET_COMMAND_ATTACK,
    PET_COMMAND_FOLLOW, PET_COMMAND_STAY, PET_REACT_AGGRESSIVE,
};

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::spell::Cooldowns;
use crate::ui_action::Spells;

use super::bar::{feed_pet_bar, fire_pet_cooldown_events};
use super::drain::PetPress;
use super::PetBar;

pub(super) const ME: u64 = 0x10;
pub(super) const PET: u64 = 0x2A;
/// `UNIT_FIELD_CHARMEDBY`, `UNIT_FIELD_SUMMONEDBY` and `UNIT_FIELD_CREATEDBY`, low words; the high
/// words stay 0.
pub(super) const CHARMEDBY: u16 = 10;
pub(super) const SUMMONEDBY: u16 = 12;
pub(super) const CREATEDBY: u16 = 14;
pub(super) const FLAGS: u16 = 46;
/// `UNIT_FIELD_AURA` slot 0 and `UNIT_FIELD_AURAFLAGS`, a nibble per slot.
pub(super) const AURA: u16 = 47;
pub(super) const AURAFLAGS: u16 = 95;

pub(super) const CLAW: u32 = 16829;
pub(super) const BITE: u32 = 17258;
pub(super) const GROWL: u32 = 14918;
pub(super) const COWER: u32 = 1742;

/// Claw's shape: no timer of its own, and the ordinary pet GCD, category 133 for 1500 ms.
pub(super) fn claw() -> SpellDisplay {
    SpellDisplay {
        name: "Claw".into(),
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    }
}

/// Bite's shape: the same GCD pair, and a 10 s category timer of its own.
pub(super) fn bite() -> SpellDisplay {
    SpellDisplay {
        name: "Bite".into(),
        category: 19,
        category_recovery_ms: 10_000,
        ..claw()
    }
}

/// Growl's shape: a 5 s category timer and no GCD pair.
pub(super) fn growl() -> SpellDisplay {
    SpellDisplay {
        name: "Growl".into(),
        category: 82,
        category_recovery_ms: 5_000,
        ..Default::default()
    }
}

/// Cower's shape here: a toggle with an active icon, so a press with its aura up cancels it.
pub(super) fn cower() -> SpellDisplay {
    SpellDisplay {
        name: "Cower".into(),
        active_icon_id: 122,
        ..claw()
    }
}

/// Our own pet as vmangos fills it, `SUMMONEDBY` and `CREATEDBY` us (`Pet.cpp:264`, `:287`), no
/// flags: the predicate's owner test reads `SUMMONEDBY` (`0x4bd054`).
pub(super) fn owned_pet() -> ObjectStore {
    owned_pet_with(&[])
}

/// [`owned_pet`] with more fields set, which win over its own.
pub(super) fn owned_pet_with(extra: &[(u16, u32)]) -> ObjectStore {
    let mut pairs = vec![
        (SUMMONEDBY, ME as u32),
        (SUMMONEDBY + 1, 0),
        (CREATEDBY, ME as u32),
        (CREATEDBY + 1, 0),
    ];
    pairs.retain(|(index, _)| !extra.iter().any(|(set, _)| set == index));
    pairs.extend_from_slice(extra);
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs))
}

/// Our own player: `OBJECT_FIELD_TYPE` 0x19, alive.
pub(super) fn player() -> ObjectStore {
    player_with(&[])
}

/// [`player`] with more fields set.
pub(super) fn player_with(extra: &[(u16, u32)]) -> ObjectStore {
    let mut pairs = vec![(2, 0x19), (22, 100), (28, 100)];
    pairs.extend_from_slice(extra);
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs))
}

pub(super) struct Rig {
    pub(super) app: App,
    commands: Receiver<ClientCommand>,
    /// The pet's feeds as a schedule of their own, so their `Local` memory lives across runs.
    pub(super) feed: Schedule,
}

/// A VM whose listener records every event the pet's lists fire into `SEEN`, in order.
pub(super) fn listening_vm() -> benilla_ui::script::UiScript {
    let script = benilla_ui::script::UiScript::new().expect("a VM");
    listen(&script);
    script
}

/// The listener [`listening_vm`] carries, on a VM that has one already.
pub(super) fn listen(script: &benilla_ui::script::UiScript) {
    script
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PET_BAR_UPDATE")
            f:RegisterEvent("PET_BAR_UPDATE_COOLDOWN")
            f:RegisterEvent("ACTIONBAR_UPDATE_COOLDOWN")
            f:RegisterEvent("SPELL_UPDATE_COOLDOWN")
            f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
        "#,
        )
        .expect("the listener registers");
}

/// The whole client, headless, with `PET` on the bar and `catalog` as the spell table. Slot `n`
/// (1-based) holds `slots[n - 1]` as an enabled spell word. `pet` is its object, or none
/// streamed.
pub(super) fn rig(
    catalog: Vec<(u32, SpellDisplay)>,
    slots: &[u32],
    pet: Option<ObjectStore>,
) -> Rig {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, commands) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    // Our own object, which a spell press needs to resolve (`0x4bd31a`).
    let me = world
        .spawn((Guid(ME), SelfPlayer, Transform::default(), player()))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
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
    world.insert_non_send_resource(listening_vm());
    // The order the plugin gives them: the feeds that push a pet cooldown, then the flush.
    let mut feed = Schedule::default();
    feed.add_systems(
        (
            crate::ui_pet_book::feed_pet_book,
            feed_pet_bar,
            fire_pet_cooldown_events,
        )
            .chain(),
    );
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
    /// The bar's feeds for one frame, and the events they fired.
    pub(super) fn frame(&mut self) -> Vec<String> {
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
    pub(super) fn press(&mut self, slot: u32) {
        self.app
            .world_mut()
            .run_system_once(move |mut press: PetPress| press.press_slot(slot))
            .expect("the press applies as a one-shot system");
    }

    /// `PetAttack` and the other one-shot orders, as their slot's packed word, applied.
    pub(super) fn order(&mut self, packed: u32) {
        self.app
            .world_mut()
            .run_system_once(move |mut press: PetPress| press.order(packed))
            .expect("the order applies as a one-shot system");
    }

    /// The commands sent so far.
    pub(super) fn sent(&self) -> Vec<ClientCommand> {
        self.commands.try_iter().collect()
    }

    /// `GetPetActionCooldown(slot)`, off what the last feed pushed: `(start, duration, enable)`.
    pub(super) fn cooldown(&mut self, slot: u32) -> (f64, f64, i32) {
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
    pub(super) fn pet_list_untouched(&self) -> bool {
        self.app.world().resource::<PetBar>().cooldowns.generation == 0
    }

    /// Whether the player's own list was touched at all.
    fn player_list_untouched(&self) -> bool {
        self.app.world().resource::<Cooldowns>().generation == 0
    }
}

pub(super) fn pet_actions(sent: &[ClientCommand]) -> usize {
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

    // `0x6e2e77`, `0x6e2e8e`: the flush, in the reference's order, and no bar update with it.
    assert_eq!(
        rig.frame(),
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "PET_BAR_UPDATE_COOLDOWN"
        ]
    );
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
        (FLAGS, crate::target::UNIT_FLAG_POSSESSED),
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
    let running = owned_pet_with(&[(AURA, COWER), (AURAFLAGS, 0x3)]);
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

/// What a press latched: the mode word, its signal count and the attack latch.
pub(super) fn latched(rig: &Rig) -> (u32, u32, bool) {
    let bar = rig.app.world().resource::<PetBar>();
    (bar.spells.state, bar.bar_signals, bar.attacking)
}

/// Everything a refused spell press leaves untouched: no command on the wire, nothing latched, no
/// GCD on the pet's list, no event.
pub(super) fn assert_press_refused(rig: &mut Rig, before: (u32, u32, bool)) {
    assert!(rig.sent().is_empty(), "no packet of any kind");
    assert_eq!(latched(rig), before, "nothing latched");
    assert!(rig.pet_list_untouched(), "no GCD armed");
    assert!(rig.frame().is_empty(), "no event");
}

/// `0x4bd1f2`: the dispatcher leaves for the epilogue, before the arm, its GCD and its send, when
/// the pet's object does not resolve, as it is while the pet is out of view and the bar stands on
/// its guid.
#[test]
fn a_spell_press_with_no_pet_object_sends_nothing_and_arms_nothing() {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], None);
    let before = latched(&rig);

    rig.press(1);

    assert_press_refused(&mut rig, before);
}

/// `0x4bd2e7`-`0x4bd2fe`: an id past the `Spell.dbc` table's maximum, or one inside it with no
/// record, leaves the arm the same way; so does the unused slot's spell 0, which has none.
#[test]
fn a_spell_press_with_no_catalog_row_sends_nothing() {
    // Slots 1-3: a row the catalog lacks, the unused slot's spell 0, and the top of the id range.
    // Slot 4: the control, whose row it holds.
    let mut rig = rig(
        vec![(BITE, bite())],
        &[CLAW, 0, u32::from(u16::MAX), BITE],
        Some(owned_pet()),
    );
    let before = latched(&rig);

    for slot in 1..=3 {
        rig.press(slot);
        assert_press_refused(&mut rig, before);
    }

    rig.press(4);
    assert_eq!(pet_actions(&rig.sent()), 1, "the control sends");
}

/// `0x4bd1f2`: the predicate also needs the active player's object (`0x4bcf92`).
#[test]
fn a_spell_press_with_no_player_object_sends_nothing() {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], Some(owned_pet()));
    rig.app
        .world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .remove(&ME);
    // The bar greys as the player leaves, which is its own repaint.
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"]);
    let before = latched(&rig);

    rig.press(1);

    assert_press_refused(&mut rig, before);
}

/// `0x4bd1f2`: the aura leg (`0x4bd240`) sits behind the predicate, so with the active player
/// unresolved a running aura is not cancelled either.
#[test]
fn an_aura_cancel_press_with_no_player_object_sends_nothing() {
    let running = owned_pet_with(&[(AURA, COWER), (AURAFLAGS, 0x3)]);
    let mut rig = rig(vec![(COWER, cower())], &[COWER], Some(running));
    rig.app
        .world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .remove(&ME);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the bar greys");
    let before = latched(&rig);

    rig.press(1);

    assert_press_refused(&mut rig, before);
}

/// `0x4bd1f2`: a reaction and the Follow and Stay commands have no exit of their own (`0x4bd391`,
/// `0x4bd3a3`), only the predicate's, so with the pet's object unresolved none latches or sends.
#[test]
fn a_command_or_reaction_press_with_no_pet_object_sends_nothing() {
    let word = |kind: u8, action: u32| action | (u32::from(kind) << 24);
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], None);
    for packed in [
        word(PET_ACT_COMMAND, PET_COMMAND_FOLLOW),
        word(PET_ACT_COMMAND, PET_COMMAND_STAY),
        word(PET_ACT_REACTION, PET_REACT_AGGRESSIVE),
    ] {
        rig.app.world_mut().resource_mut::<PetBar>().spells.bar[0] = PetActionEntry::from(packed);
        rig.frame();
        let before = latched(&rig);

        rig.press(1);

        assert_press_refused(&mut rig, before);
    }
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

/// `0x4bd1f2`: the Attack command's own null-pet branch (`0x4bd403`-`0x4bd405`) repeats the lookup
/// the predicate made (`0x4bd034`) on the same guid and typemask, so it never runs: with the pet's
/// object unresolved the press leaves at the gate and sends nothing, at the selection or any
/// other target.
#[test]
fn an_attack_press_with_no_pet_object_sends_nothing() {
    const FOE: u64 = 0x77;
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], None);
    let world = rig.app.world_mut();
    // A player-controlled attacker, whose `CanAttack` arm needs no faction table.
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (22, 100),
            (28, 100),
            (FLAGS, 0x8),
        ])));
    let foe = world
        .spawn((
            Guid(FOE),
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
            ])),
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(FOE, foe);
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(foe),
        guid: Some(FOE),
        ..Default::default()
    };
    let attack = PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24);
    world.resource_mut::<PetBar>().spells.bar[0] = PetActionEntry::from(attack);
    rig.frame();
    let before = latched(&rig);

    rig.press(1);

    assert_press_refused(&mut rig, before);

    // The same press with the pet streamed in goes out at the selection, and an ordinary pet's
    // Attack raises no latch.
    let world = rig.app.world_mut();
    let pet = world
        .spawn((Guid(PET), owned_pet_with(&[(22, 100), (28, 100)])))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(PET, pet);

    rig.press(1);

    let sent = rig.sent();
    assert!(
        matches!(
            sent.as_slice(),
            [ClientCommand::PetAction { pet_guid: PET, packed, target_guid: FOE }] if *packed == attack
        ),
        "{sent:?}"
    );
    assert!(!latched(&rig).2, "an ordinary pet's Attack raises no latch");
}
