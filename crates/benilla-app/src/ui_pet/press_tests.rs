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

/// A VM whose listener records every event the pet's lists fire into `SEEN`, in order. No engine
/// is behind it, so it keeps the calls its verbs make; the rig applies a press itself.
pub(super) fn listening_vm() -> benilla_ui::script::UiScript {
    let mut script = benilla_ui::script::UiScript::new().expect("a VM");
    script.keep_calls();
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
            super::drain::arm_owed_pet_gcds,
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

    /// `CastPetAction(slot)` as the stock bar runs it, with the call host lent, so a press the bar
    /// hands to the attempt casts through the ladder.
    pub(super) fn cast_pet_action(&mut self, slot: u32) {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM")
                .run(&format!("CastPetAction({slot})"))
                .expect("CastPetAction runs");
        });
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

/// `PLAYER_FARSIGHT`, low word: the view `0x5ee5a0` reads (`0x5ee5e9`).
const FARSIGHT: u16 = 712;

/// The rig's player with its view on `guid`, as vmangos puts it on a unit it possesses.
fn view_on(rig: &mut Rig, guid: u64) {
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert(player_with(&[
        (FARSIGHT, guid as u32),
        (FARSIGHT + 1, (guid >> 32) as u32),
    ]));
}

/// A unit the player mind-controls: charmed by us and possessed (`SpellAuras.cpp:2968-2969`).
fn possessed() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (CHARMEDBY, ME as u32),
        (CHARMEDBY + 1, 0),
        (FLAGS, crate::target::UNIT_FLAG_POSSESSED),
    ]))
}

/// The pet casts the commands sent carry, as `(caster, spell, target block)`.
fn pet_casts(sent: &[ClientCommand]) -> Vec<(u64, u32, benilla_protocol::messages::UseItemTarget)> {
    sent.iter()
        .filter_map(|c| match c {
            ClientCommand::PetCastSpell {
                caster_guid,
                spell_id,
                target,
            } => Some((*caster_guid, *spell_id, *target)),
            _ => None,
        })
        .collect()
}

/// `0x4bd35e`-`0x4bd365`: the unit the player possesses, the one `0x5ee5a0` answers, has its spell
/// press returned into the attempt with it as the caster (`0x4bd385`), whose send is
/// `CMSG_PET_CAST_SPELL` with the unit's guid (`0x6e5808`-`0x6e582d`), not `CMSG_PET_ACTION`. The
/// GCD is the send's (`0x6e58fb`), into the pet's list since the unit's charmer is the player
/// (`0x6e58c2`-`0x6e58f3`), and the arm's own (`0x4bd36e`) never runs.
#[test]
fn a_possessed_units_press_casts_through_the_attempt_as_the_units_cast() {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], Some(possessed()));
    view_on(&mut rig, PET);

    rig.cast_pet_action(1);

    let sent = rig.sent();
    assert_eq!(pet_actions(&sent), 0, "no CMSG_PET_ACTION");
    assert_eq!(
        pet_casts(&sent),
        [(
            PET,
            CLAW,
            benilla_protocol::messages::UseItemTarget::SelfImplicit
        )]
    );
    assert!(rig.player_list_untouched(), "the GCD is not the player's");
    // The send's GCD reaches the pet's list before the bar's feed reads it, and sweeps the slot.
    assert_eq!(
        rig.frame(),
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "PET_BAR_UPDATE_COOLDOWN"
        ]
    );
    assert_eq!(rig.pet_reads(CLAW, &claw()).1, 1500);
    let (_, duration, enable) = rig.cooldown(1);
    assert!((duration - 1.5).abs() < 1e-9 && enable == 1, "{duration}");
}

/// `0x5ee5a0` answers the far-sight unit alone (`0x5ee5e9`): with no view held the same unit's
/// press is the bar's order, which starts the GCD itself (`0x4bd36e`) and sends. A view held on
/// another unit leaves no bar to press (`0x4bcfbf`-`0x4bd012`).
#[test]
fn a_possessed_unit_the_view_is_not_on_presses_as_a_pet_order() {
    let mut rig = rig(vec![(CLAW, claw())], &[CLAW], Some(possessed()));
    view_on(&mut rig, 0);

    rig.cast_pet_action(1);

    let sent = rig.sent();
    assert_eq!(pet_actions(&sent), 1);
    assert!(pet_casts(&sent).is_empty());
    assert_eq!(rig.pet_reads(CLAW, &claw()).1, 1500, "the arm's own GCD");
}

/// `0x4bd355`: a spell with `AttributesEx4 & 0x20` takes the same route on an ordinary pet's bar.
/// With nothing selected its unit word (`Targets 0x2`) raises the targeting cursor (`0x6e50c8`),
/// which keeps the pet as the record's caster, so the bind `SpellTargetUnit` makes (`0x6e5b10`)
/// sends `CMSG_PET_CAST_SPELL` (`0x6e54f0`) at that unit, with the send's GCD the pet's.
#[test]
fn a_client_targeted_spell_press_casts_through_the_cursor_as_the_pets_cast() {
    let aimed = || SpellDisplay {
        attributes_ex4: 0x20,
        targets: 0x2,
        ..claw()
    };
    let mut rig = rig(vec![(CLAW, aimed())], &[CLAW], Some(owned_pet()));

    crate::script_calls::with_hosted_vm(rig.app.world_mut(), |s| {
        let s = s.expect("a VM");
        let up: bool = s
            .eval("CastPetAction(1) return SpellIsTargeting() == 1")
            .unwrap();
        assert!(up, "the cursor is up for the pet's spell");
        s.run(r#"SpellTargetUnit("player")"#).unwrap();
    });

    let sent = rig.sent();
    assert_eq!(pet_actions(&sent), 0);
    assert_eq!(
        pet_casts(&sent),
        [(
            PET,
            CLAW,
            benilla_protocol::messages::UseItemTarget::Unit(ME)
        )]
    );
    assert!(rig.player_list_untouched());
    rig.frame();
    assert_eq!(rig.pet_reads(CLAW, &claw()).1, 1500, "the send's GCD");
}

/// A spell with the attack-start bits (`0x6e5200`, here `AttributesEx 0x200`) that the possessed
/// unit casts goes through TryCast's attack pick with that unit as the actor, while it is not
/// attacking (`0x6e4ee8`-`0x6e4f13`: `0x60ecb0`, then `0x612df0(unit, &guid)`). With nothing
/// selected and no enemy near, the pick refuses with `ERR_NO_ATTACK_TARGET` and nothing is cast.
/// At a foe it keeps, the cast goes out, and TryCast's tail (`0x6e51c9`-`0x6e51de`) calls
/// `StartAttack 0x6131a0` on the unit, which for the pet bar's unit (`0x613200`) presses the
/// bar's Attack at that foe (`0x4bd630`, word `0x07000002`): its `CMSG_PET_ACTION`, and the attack
/// latch a possessed unit raises (`0x4bd42e`).
#[test]
fn a_possessed_units_combat_spell_picks_its_target_and_orders_the_attack() {
    const FOE: u64 = 0x77;
    const STRIKE: u32 = 3010;
    let strike = || SpellDisplay {
        attributes_ex: 0x200,
        targets: 0x2,
        ..claw()
    };
    let unit = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (CHARMEDBY, ME as u32),
        (CHARMEDBY + 1, 0),
        (FLAGS, crate::target::UNIT_FLAG_POSSESSED),
        (22, 100),
        (28, 100),
    ]));
    let mut rig = rig(vec![(STRIKE, strike())], &[STRIKE], Some(unit));
    // The view on the unit, and a player-controlled actor, whose `CanAttack` arm needs no faction
    // table.
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert(player_with(&[
        (FARSIGHT, PET as u32),
        (FARSIGHT + 1, 0),
        (FLAGS, 0x8),
    ]));
    // The call fires its refusals as it returns (`UI_ERROR_MESSAGE`).
    let script = world.non_send_resource::<benilla_ui::script::UiScript>();
    script
        .run(
            r#"
            ERR_NO_ATTACK_TARGET = "There is nothing to attack."
            ERRORS = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:SetScript("OnEvent", function() table.insert(ERRORS, arg1) end)
        "#,
        )
        .expect("the error listener registers");
    let errors = |rig: &mut Rig| {
        let script = rig
            .app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        let seen = script.eval::<Vec<String>>("return ERRORS").unwrap();
        script.run("ERRORS = {}").unwrap();
        seen
    };

    rig.cast_pet_action(1);
    assert!(pet_casts(&rig.sent()).is_empty(), "the pick refused");
    assert_eq!(errors(&mut rig), ["There is nothing to attack."]);
    assert!(
        !rig.app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .active(),
        "the pick stops the attempt before its cursor"
    );

    let world = rig.app.world_mut();
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

    rig.cast_pet_action(1);
    let sent = rig.sent();
    let attack = PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24);
    assert_eq!(
        pet_casts(&sent),
        [(
            PET,
            STRIKE,
            benilla_protocol::messages::UseItemTarget::Unit(FOE)
        )]
    );
    assert!(
        matches!(
            sent.last(),
            Some(ClientCommand::PetAction { pet_guid: PET, packed, target_guid: FOE }) if *packed == attack
        ),
        "the Attack order follows the cast: {sent:?}"
    );
    assert!(
        latched(&rig).2,
        "a possessed unit's Attack raises the latch"
    );
    assert!(errors(&mut rig).is_empty());
}

/// The cursor binds a unit caster's cast against that caster: `BindTarget` resolves the record's
/// caster (`0x6e5b6d`-`0x6e5b89`, `0xceac50`) for its relations and its range test
/// (`0x6e6059`-`0x6e6063` → `0x6e47b0`). A spell that excludes its caster (`AttributesEx
/// 0x80000`) binds us for the pet, and a 5 yd spell is out of range at us from a pet 30 yd off.
#[test]
fn the_cursor_binds_a_unit_casters_spell_against_that_unit() {
    const MEND: u32 = 3111;
    const NEAR: u32 = 1;
    // Client-targeted (`AttributesEx4 0x20`), a unit word, a 5 yd row.
    let aimed = |attributes_ex| SpellDisplay {
        attributes_ex4: 0x20,
        attributes_ex,
        targets: 0x2,
        range_index: NEAR,
        ..claw()
    };
    let bound_at_us = |spell: SpellDisplay, pet_at: Vec3| {
        let mut rig = rig(
            vec![(MEND, spell)],
            &[MEND],
            Some(owned_pet_with(&[(22, 100), (28, 100)])),
        );
        let world = rig.app.world_mut();
        let pet = world.resource::<GuidIndex>().0[&PET];
        world
            .entity_mut(pet)
            .insert(GlobalTransform::from_translation(pet_at));
        world.resource_mut::<Spells>().ranges = benilla_formats::SpellRangeCatalog::from_rows(
            [(
                NEAR,
                benilla_formats::SpellRange {
                    min: 0.0,
                    max: 5.0,
                    flags: 0,
                },
            )]
            .into(),
        );
        crate::script_calls::with_hosted_vm(rig.app.world_mut(), |s| {
            let s = s.expect("a VM");
            let up: bool = s
                .eval("CastPetAction(1) return SpellIsTargeting() == 1")
                .unwrap();
            assert!(up, "the cursor is up for the pet's spell");
            s.run(r#"SpellTargetUnit("player")"#).unwrap();
        });
        pet_casts(&rig.sent())
    };
    let at_us = [(
        PET,
        MEND,
        benilla_protocol::messages::UseItemTarget::Unit(ME),
    )];
    assert_eq!(
        bound_at_us(aimed(0x80000), Vec3::ZERO),
        at_us,
        "we are not the pet's spell's caster"
    );
    assert_eq!(
        bound_at_us(aimed(0), Vec3::new(30.0, 0.0, 0.0)),
        [],
        "out of the pet's range"
    );
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

/// The pet bar's Attack runs the attack validator (`0x4bd40d` → `0x612df0`), whose `SelectTarget`
/// (`0x613100`) binds a kept target to a standing targeting cursor and casts inside the validator,
/// so the cast goes out ahead of the `CMSG_PET_ACTION` it validated.
#[test]
fn a_pet_attack_with_the_cursor_up_casts_before_the_pet_acts() {
    const FOE: u64 = 0x77;
    const HEAL: u32 = 2050;
    let mut rig = rig(
        vec![(CLAW, claw())],
        &[],
        Some(owned_pet_with(&[(22, 100), (28, 100)])),
    );
    let world = rig.app.world_mut();
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
    world.resource_mut::<crate::spell::SpellTargeting>().enter(
        HEAL,
        crate::spell::CastCommit::Spell,
        0x0002,
    );
    rig.frame();
    crate::script_calls::with_hosted_vm(rig.app.world_mut(), |s| {
        s.expect("a VM").run_binding("PetAttack()")
    })
    .expect("the key runs");

    let sent = rig.sent();
    assert!(
        matches!(
            sent.as_slice(),
            [
                ClientCommand::CastSpell {
                    spell_id: HEAL,
                    target: Some(FOE)
                },
                ClientCommand::PetAction {
                    pet_guid: PET,
                    target_guid: FOE,
                    ..
                },
            ]
        ),
        "{sent:?}"
    );
}

/// `PetWait` signals `PET_BAR_UPDATE` in the verb (`0x4bc979`); the app's mirror of the latch and
/// the bar catching up with it a frame later add no second signal.
#[test]
fn a_wait_press_is_signalled_once() {
    let mut rig = rig(vec![], &[], Some(owned_pet()));
    rig.app.world_mut().resource_mut::<PetBar>().spells.bar[0] = PetActionEntry::from(0x0700_0000);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the token arrives");

    rig.app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .run("PetWait()")
        .unwrap();
    rig.order(0x0700_0000);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the verb's own, in place");
    assert!(rig.frame().is_empty(), "nothing more");
    assert_eq!(pet_actions(&rig.sent()), 1, "and the order went out");
}

/// A press the app has not mirrored yet is not rolled back by a push of another slot: only a
/// slot whose source moved is pushed.
#[test]
fn a_verbs_latch_survives_a_push_of_another_slot() {
    let mut rig = rig(vec![], &[], Some(owned_pet()));
    {
        let mut bar = rig.app.world_mut().resource_mut::<PetBar>();
        bar.spells.bar[0] = PetActionEntry::from(0x0700_0000);
        // Following: Wait is unlit.
        bar.spells.state = 1 << 8;
    }
    rig.frame();

    let lit = |rig: &mut Rig| {
        rig.app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval::<bool>("local _, _, _, _, a = GetPetActionInfo(1) return a == 1")
            .unwrap()
    };
    assert!(!lit(&mut rig));
    rig.app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .run("PetWait()")
        .unwrap();
    assert!(lit(&mut rig), "the verb lit Wait");
    rig.app.world_mut().resource_mut::<PetBar>().spells.bar[1] = PetActionEntry::from(0x0700_0001);
    rig.frame();
    assert!(lit(&mut rig), "the stale store unlit it");
}

/// A latch the VM made but this side never sent (the pet not usable here that frame) does not
/// outlive the next `SMSG_PET_SPELLS`: its signal (`0x4bc90c`) re-pushes the whole bar and the
/// state word, equal values included.
#[test]
fn a_packet_overwrites_a_latch_this_side_never_sent() {
    let mut rig = rig(vec![], &[], Some(owned_pet()));
    {
        let mut bar = rig.app.world_mut().resource_mut::<PetBar>();
        bar.spells.bar[0] = PetActionEntry::from(0x0700_0000);
        bar.spells.state = 1 << 8;
    }
    rig.frame();
    let lit = |rig: &mut Rig| {
        rig.app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval::<bool>("local _, _, _, _, a = GetPetActionInfo(1) return a == 1")
            .unwrap()
    };
    rig.app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .run("PetWait()")
        .unwrap();
    assert!(lit(&mut rig), "the verb lit Wait");
    // No order mirrored; the packet repeats the old state.
    {
        let mut bar = rig.app.world_mut().resource_mut::<PetBar>();
        bar.bar_signals = bar.bar_signals.wrapping_add(1);
    }
    rig.frame();
    assert!(!lit(&mut rig), "the packet's state is the bar's");
}
