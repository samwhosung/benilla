//! The pet system's tests, in one module because they share their fixtures.

use benilla_protocol::messages::{
    PetActionEntry, PetSpells, PET_ACT_COMMAND, PET_ACT_DISABLED, PET_ACT_ENABLED, PET_ACT_PASSIVE,
    PET_ACT_REACTION, PET_COMMAND_ATTACK, PET_COMMAND_DISMISS, PET_COMMAND_FOLLOW,
    PET_COMMAND_STAY, PET_REACT_AGGRESSIVE, PET_REACT_DEFENSIVE, PET_REACT_PASSIVE,
    PET_STATE_BAR_DISABLED,
};
use benilla_ui::script::PetActionView;

use crate::net::{ClientCommand, NetCommands, ObjectStore};

use super::bar::*;
use super::drain::*;
use super::menu::*;
use super::unit::*;
use super::*;

fn packed(action: u32, kind: u8) -> PetActionEntry {
    PetActionEntry::from(action | (u32::from(kind) << 24))
}

/// [`slot_view`] for a slot not showing active.
fn view(
    entry: PetActionEntry,
    bar: &PetSpells,
    spell: Option<&benilla_formats::SpellDisplay>,
    cooldown: Option<(i64, u32, bool)>,
    pet_attacking: bool,
) -> PetActionView {
    slot_view(entry, bar, spell, cooldown, pet_attacking, false)
}

fn spell(name: &str, rank: Option<&str>) -> benilla_formats::SpellDisplay {
    benilla_formats::SpellDisplay {
        name: name.to_string(),
        rank: rank.map(str::to_string),
        icon: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
        ..Default::default()
    }
}

/// The state dword as the server packs it: react in byte 0, command in byte 1.
fn state(command: u32, react: u32) -> PetSpells {
    PetSpells {
        pet_guid: 0x2A,
        state: react | (command << 8),
        ..Default::default()
    }
}

#[test]
fn command_tokens_light_on_the_current_command() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);

    let follow = view(
        packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND),
        &bar,
        None,
        None,
        false,
    );
    assert_eq!(follow.name.as_deref(), Some("PET_ACTION_FOLLOW"));
    assert_eq!(follow.texture.as_deref(), Some("PET_FOLLOW_TEXTURE"));
    assert!(follow.is_token && follow.active);
    assert!(!follow.attack_active, "Follow is not the attack fork");

    let stay = view(
        packed(PET_COMMAND_STAY, PET_ACT_COMMAND),
        &bar,
        None,
        None,
        false,
    );
    assert!(!stay.active, "only the CURRENT command is lit");
}

#[test]
fn the_attack_latch_lights_attack_whatever_the_command_state_says() {
    let following = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);

    let idle = view(attack, &following, None, None, false);
    assert!(!idle.active && !idle.attack_active);

    let on = view(attack, &following, None, None, true);
    assert!(on.active, "the latch lights it even on a FOLLOW command");
    assert!(on.attack_active, "and the next press calls the pet off");

    let follow = view(
        packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND),
        &following,
        None,
        None,
        true,
    );
    assert!(!follow.attack_active);
    assert!(follow.active, "…but Follow is still the standing command");
}

#[test]
fn reaction_tokens_use_the_mode_keys_and_light_on_the_current_react() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let def = view(
        packed(PET_REACT_DEFENSIVE, PET_ACT_REACTION),
        &bar,
        None,
        None,
        false,
    );
    assert_eq!(def.name.as_deref(), Some("PET_MODE_DEFENSIVE"));
    assert_eq!(def.texture.as_deref(), Some("PET_DEFENSIVE_TEXTURE"));
    assert!(def.is_token && def.active);
    assert!(
        !view(
            packed(PET_REACT_AGGRESSIVE, PET_ACT_REACTION),
            &bar,
            None,
            None,
            false
        )
        .active
    );
}

#[test]
fn a_disabled_bar_reads_passive_and_lights_no_command() {
    let mut bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    bar.state |= PET_STATE_BAR_DISABLED;

    let passive = view(
        packed(PET_REACT_PASSIVE, PET_ACT_REACTION),
        &bar,
        None,
        None,
        false,
    );
    assert!(passive.active, "a bar that cannot be ordered reads Passive");
    assert!(
        !view(
            packed(PET_REACT_DEFENSIVE, PET_ACT_REACTION),
            &bar,
            None,
            None,
            false
        )
        .active
    );

    assert!(
        !view(
            packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND),
            &bar,
            None,
            None,
            false
        )
        .active,
        "the command it IS on goes dark too"
    );
}

#[test]
fn usability_is_the_disabled_bit_and_the_pets_crowd_control() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    assert!(actions_usable(&bar, Some(0)));
    assert!(
        actions_usable(&bar, None),
        "a missing descriptor is not a no"
    );

    for flag in [0x0004_0000, 0x0040_0000, 0x0080_0000] {
        assert!(!actions_usable(&bar, Some(flag)), "flag {flag:#x} disables");
    }
    // Those three are stunned, confused, fleeing; POSSESSED is not among them.
    assert!(actions_usable(&bar, Some(0x0100_0000)));

    bar.spells.state |= PET_STATE_BAR_DISABLED;
    assert!(!actions_usable(&bar, Some(0)));
}

#[test]
fn spell_slots_read_their_autocast_off_bits_31_and_30() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let claw = spell("Claw", Some("Rank 3"));

    let on = view(
        packed(3010, PET_ACT_ENABLED),
        &bar,
        Some(&claw),
        None,
        false,
    );
    assert_eq!(on.name.as_deref(), Some("Claw"));
    assert_eq!(on.subtext.as_deref(), Some("Rank 3"));
    assert_eq!(on.spell_id, Some(3010));
    assert!(!on.is_token);
    assert!(on.autocast_allowed && on.autocast_enabled);
    assert!(!on.active, "a SPELL slot never reports isActive");

    let off = view(
        packed(3010, PET_ACT_DISABLED),
        &bar,
        Some(&claw),
        None,
        false,
    );
    assert!(off.autocast_allowed && !off.autocast_enabled);

    let passive = view(
        packed(3010, PET_ACT_PASSIVE),
        &bar,
        Some(&claw),
        None,
        false,
    );
    assert!(!passive.autocast_allowed && !passive.autocast_enabled);
}

/// The client's zero word, and vmangos's `(0, ACT_DISABLED)` filler, which misses the catalog.
#[test]
fn both_kinds_of_empty_slot_draw_nothing() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);

    let zero = PetActionEntry::default();
    assert!(zero.is_empty());
    assert_eq!(
        view(zero, &bar, None, None, false),
        PetActionView::default()
    );

    let filler = packed(0, PET_ACT_DISABLED);
    assert!(!filler.is_empty(), "the WORD is not zero");
    assert_eq!(
        view(filler, &bar, None, None, false),
        PetActionView {
            // The word stays: it is the drag's relocation candidate.
            packed: filler.packed,
            ..Default::default()
        },
        "…but spell id 0 resolves to nothing, so the button is still empty"
    );
}

#[test]
fn an_unresolvable_spell_draws_nothing() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let entry = packed(999, PET_ACT_ENABLED);
    let v = view(entry, &bar, None, None, false);
    assert_eq!(
        v,
        PetActionView {
            packed: entry.packed,
            ..Default::default()
        }
    );
    assert!(!v.autocast_allowed && !v.autocast_enabled);
}

/// A type outside 1-7 draws empty; the reference's default arm under-pushes there (`0x4bde6f`).
#[test]
fn an_unknown_type_byte_is_inert() {
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let claw = spell("Claw", None);
    let entry = packed(3010, 0x33);
    assert_eq!(
        view(entry, &bar, Some(&claw), None, false),
        PetActionView {
            packed: entry.packed,
            ..Default::default()
        }
    );
}

#[test]
fn slot_lookup_is_one_based_and_bounded() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    bar.spells.bar[0] = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    bar.spells.bar[9] = packed(PET_REACT_PASSIVE, PET_ACT_REACTION);

    assert_eq!(slot_entry(&bar, 1).unwrap().action(), PET_COMMAND_ATTACK);
    assert_eq!(slot_entry(&bar, 10).unwrap().action(), PET_REACT_PASSIVE);
    assert!(slot_entry(&bar, 0).is_none());
    assert!(slot_entry(&bar, 11).is_none());
    assert!(slot_entry_mut(&mut bar, 0).is_none());
    assert!(slot_entry_mut(&mut bar, 11).is_none());
}

#[test]
fn a_press_latches_the_state_the_server_never_echoes() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    bar.spells.state |= PET_STATE_BAR_DISABLED;

    latch_press(&mut bar, packed(PET_COMMAND_STAY, PET_ACT_COMMAND), false);
    assert_eq!(bar.spells.command_state() & 0xFF, PET_COMMAND_STAY);
    assert_eq!(bar.spells.react_state(), PET_REACT_DEFENSIVE, "untouched");
    assert!(
        bar.spells.bar_disabled(),
        "the disabled bit is the SERVER's — a command press must preserve it"
    );

    latch_press(
        &mut bar,
        packed(PET_REACT_AGGRESSIVE, PET_ACT_REACTION),
        false,
    );
    assert_eq!(bar.spells.react_state(), PET_REACT_AGGRESSIVE);
    assert_eq!(bar.spells.command_state() & 0xFF, PET_COMMAND_STAY);
    assert!(bar.spells.bar_disabled());

    latch_press(&mut bar, packed(3010, PET_ACT_DISABLED), false);
    assert_eq!(bar.spells.command_state() & 0xFF, PET_COMMAND_STAY);
    assert_eq!(bar.spells.react_state(), PET_REACT_AGGRESSIVE);

    latch_press(
        &mut bar,
        packed(PET_COMMAND_DISMISS, PET_ACT_COMMAND),
        false,
    );
    assert_eq!(
        bar.spells.command_state() & 0xFF,
        PET_COMMAND_STAY,
        "Dismiss ends the pet; it never becomes its standing command"
    );
}

/// `OnClick`'s `SetChecked(0)` unlights every press; `0x4bc940`/`0x4bc960` relight it by
/// signalling unconditionally, and a hunter's Attack skips the signal at `0x4bd429`.
#[test]
fn every_press_the_reference_signals_forces_a_repaint() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let signals = |b: &PetBar| b.bar_signals;

    // The mode the pet is already in: the state does not move, the signal still fires.
    let before = signals(&bar);
    latch_press(&mut bar, packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND), false);
    assert_eq!(bar.spells.command_state(), PET_COMMAND_FOLLOW, "unmoved");
    assert_ne!(signals(&bar), before, "and it repaints anyway");

    let before = signals(&bar);
    latch_press(
        &mut bar,
        packed(PET_REACT_DEFENSIVE, PET_ACT_REACTION),
        false,
    );
    assert_eq!(bar.spells.react_state(), PET_REACT_DEFENSIVE, "unmoved");
    assert_ne!(signals(&bar), before, "same for the reaction side");

    // The presses the reference leaves silent.
    let before = signals(&bar);
    latch_press(
        &mut bar,
        packed(PET_COMMAND_DISMISS, PET_ACT_COMMAND),
        false,
    );
    latch_press(&mut bar, packed(3010, PET_ACT_ENABLED), false);
    latch_press(&mut bar, packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND), false);
    assert_eq!(
        signals(&bar),
        before,
        "Dismiss, a spell and a pet's Attack all reach 0x4bd444 without signalling"
    );

    // A possessed unit's Attack signals (`0x4bd429`).
    latch_press(&mut bar, packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND), true);
    assert_ne!(signals(&bar), before);
}

/// The latch's possession gate is `0x4bd420`.
#[test]
fn only_a_possessed_units_attack_press_raises_the_latch() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    assert!(!bar.attacking);

    latch_press(&mut bar, attack, false);
    assert!(
        !bar.attacking,
        "an ordinary pet can never raise the latch, so its button can never light"
    );

    latch_press(&mut bar, attack, true);
    assert!(bar.attacking, "a possessed unit can");

    bar.attacking = false;
    latch_press(&mut bar, packed(PET_COMMAND_DISMISS, PET_ACT_COMMAND), true);
    latch_press(&mut bar, packed(PET_REACT_PASSIVE, PET_ACT_REACTION), true);
    assert!(!bar.attacking, "only ATTACK raises it");
}

/// `0x5ee5a0` asks for the unit we possess, not the one we own.
#[test]
fn the_latch_gate_is_possession_not_ownership() {
    const FLAGS: u16 = 46;
    const CHARMEDBY: u16 = 10;
    const CREATEDBY: u16 = 14;
    let unit =
        |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
    let me = Some(0x77u64);

    // An ordinary hunter pet: ours, in combat, not possessed.
    let hunter_pet = unit(&[
        (CREATEDBY, 0x77),
        (CREATEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert!(!possessing(Some(&hunter_pet), me));

    // The same pet under Eyes of the Beast.
    let driven = unit(&[
        (CREATEDBY, 0x77),
        (CREATEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_POSSESSED),
    ]);
    assert!(possessing(Some(&driven), me));

    // A mind-controlled mob: CHARMEDBY comes first, so it needs no CREATEDBY.
    let charmed = unit(&[(CHARMEDBY, 0x77), (CHARMEDBY + 1, 0), (FLAGS, 0x0100_0000)]);
    assert!(possessing(Some(&charmed), me));

    // Another player's possessed unit.
    let theirs = unit(&[
        (CHARMEDBY, 0x99),
        (CHARMEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_POSSESSED),
    ]);
    assert!(!possessing(Some(&theirs), me));

    assert!(!possessing(None, me));
}

#[test]
fn a_pets_attack_button_never_lights() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    let follow = packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND);

    for _ in 0..3 {
        assert!(
            commit_press(&mut bar, attack, false, false),
            "the order goes"
        );
        let lit = view(attack, &bar.spells, None, None, bar.attacking);
        assert!(!lit.active, "and the button does not light — ever");
        assert!(
            !lit.attack_active,
            "so the next press orders again, never calls off"
        );
    }
    assert_eq!(
        bar.spells.command_state(),
        PET_COMMAND_FOLLOW,
        "an attack order leaves the standing command alone"
    );
    assert!(
        view(follow, &bar.spells, None, None, bar.attacking).active,
        "and Follow keeps the light that is actually a mode's"
    );

    latch_press(&mut bar, packed(PET_COMMAND_STAY, PET_ACT_COMMAND), false);
    assert_eq!(bar.spells.command_state(), PET_COMMAND_STAY);
}

#[test]
fn a_possessed_units_attack_button_does_light() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    let follow = packed(PET_COMMAND_FOLLOW, PET_ACT_COMMAND);

    assert!(commit_press(&mut bar, attack, false, true));
    assert!(view(attack, &bar.spells, None, None, bar.attacking).active);
    assert!(view(attack, &bar.spells, None, None, bar.attacking).attack_active);
    assert!(
        view(follow, &bar.spells, None, None, bar.attacking).active,
        "the latch is not the command byte — Follow keeps its own light"
    );

    // `PetStopAttack` lowers the latch, the only thing lighting Attack.
    bar.attacking = false;
    let called_off = view(attack, &bar.spells, None, None, bar.attacking);
    assert!(!called_off.active);
    assert!(!called_off.attack_active);
}

/// The veto jumps past the send (`0x4bd414 je 0x4bd4c6`).
#[test]
fn a_refused_attack_neither_sends_nor_lights_the_button() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);

    assert!(
        !commit_press(&mut bar, attack, true, true),
        "a vetoed order never reaches the wire"
    );
    assert!(!bar.attacking, "and never raises the latch");
    assert!(
        !view(attack, &bar.spells, None, None, bar.attacking).active,
        "so the button stays dark, even on the one bar where it could light"
    );
    assert_eq!(
        bar.spells.command_state(),
        PET_COMMAND_FOLLOW,
        "a refusal cannot move the standing command either"
    );

    // With the gate clear, the same press sends and latches.
    assert!(commit_press(&mut bar, attack, false, true));
    assert!(bar.attacking);
    assert!(view(attack, &bar.spells, None, None, bar.attacking).active);
}

/// The old-target clear's `PetStopAttack` (`0x493910`, at `0x493a18`).
#[test]
fn touching_your_target_calls_the_pet_off() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let commands = NetCommands(tx);
    let mut bar = PetBar {
        spells: PetSpells {
            pet_guid: 0xF14,
            ..state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE)
        },
        attacking: true,
        ..Default::default()
    };

    // Selecting from nothing is not a clear (`0x493937`).
    assert!(!old_target_cleared(None, Some(7)));
    // Replacing a selection is, and so is dropping it.
    assert!(old_target_cleared(Some(7), Some(9)));
    assert!(old_target_cleared(Some(7), None));
    // A re-select never reaches the clear (`0x493540`).
    assert!(!old_target_cleared(Some(7), Some(7)));

    assert!(stop_pet_attack(&mut bar, &commands));
    assert!(!bar.attacking, "the latch is down");
    assert!(
        matches!(rx.try_recv(), Ok(ClientCommand::PetStopAttack { pet_guid }) if pet_guid == 0xF14),
        "and the server is told, with the BAR's guid"
    );

    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    assert!(!view(attack, &bar.spells, None, None, bar.attacking).active);

    // With the latch down it sends nothing (`0x4bd65e`).
    assert!(!stop_pet_attack(&mut bar, &commands));
    assert!(rx.try_recv().is_err());
}

/// The owner test falls back to SUMMONEDBY, not `0x5ee5a0`'s CREATEDBY.
#[test]
fn the_attack_events_read_the_pets_combat_flag_not_the_click_latch() {
    const FLAGS: u16 = 46;
    const CHARMEDBY: u16 = 10;
    const SUMMONEDBY: u16 = 12;
    let unit =
        |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
    let me = Some(0x77u64);

    // Ours by SUMMONEDBY, fighting or not.
    let mine = |flags: u32| unit(&[(SUMMONEDBY, 0x77), (SUMMONEDBY + 1, 0), (FLAGS, flags)]);
    assert_eq!(
        pet_combat_flag(&mine(UNIT_FLAG_PET_IN_COMBAT), me),
        Some(true)
    );
    assert_eq!(pet_combat_flag(&mine(0), me), Some(false));
    // The callback tests one bit.
    assert_eq!(pet_combat_flag(&mine(0x1000), me), Some(false));

    // Another player's minion never fires.
    let theirs = unit(&[
        (SUMMONEDBY, 0x99),
        (SUMMONEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert_eq!(pet_combat_flag(&theirs, me), None);

    // CHARMEDBY wins over SUMMONEDBY, both ways.
    let charmed_by_me = unit(&[
        (CHARMEDBY, 0x77),
        (CHARMEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert_eq!(pet_combat_flag(&charmed_by_me, me), Some(true));
    let stolen = unit(&[
        (CHARMEDBY, 0x99),
        (CHARMEDBY + 1, 0),
        (SUMMONEDBY, 0x77),
        (SUMMONEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert_eq!(pet_combat_flag(&stolen, me), None);
}

#[test]
fn only_the_attack_order_is_gated() {
    assert!(is_attack_order(packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND)));
    for action in [
        PET_COMMAND_STAY,
        PET_COMMAND_FOLLOW,
        PET_COMMAND_DISMISS,
        4,
        9,
    ] {
        assert!(
            !is_attack_order(packed(action, PET_ACT_COMMAND)),
            "command {action} sends unconditionally"
        );
    }
    // Aggressive shares Attack's action 2; the type decides.
    assert!(!is_attack_order(packed(
        PET_REACT_AGGRESSIVE,
        PET_ACT_REACTION
    )));
}

/// A pet with one aura in slot 0. `AURAFLAGS` packs a nibble per slot: `0x2` is an effect-index
/// bit (the slot is live), `0x1` is `AFLAG_CANCELABLE`, the bit `0x4bcea0` tests.
fn pet_running(spell_id: u32, nibble: u32) -> ObjectStore {
    const AURA: u16 = 47;
    const AURAFLAGS: u16 = 95;
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (AURA, spell_id),
        (AURAFLAGS, nibble),
    ]))
}

/// A spell with an active icon, which the predicate requires.
fn toggle_spell() -> benilla_formats::SpellDisplay {
    benilla_formats::SpellDisplay {
        active_icon_id: 122,
        active_icon: Some("Interface\\Icons\\Ability_Druid_Cower".into()),
        ..spell("Cower", Some("Rank 1"))
    }
}

#[test]
fn a_pet_spell_shows_active_only_while_it_is_a_live_cancelable_aura_on_the_pet() {
    let slot = packed(2645, benilla_protocol::messages::PET_TYPE_SPELL_FIRST);
    let running = pet_running(2645, 0x3);
    let d = toggle_spell();

    assert_eq!(
        active_aura_press(slot, Some(&running), Some(&d)),
        Some(2645)
    );

    // No ActiveIconID: never active, so a press re-casts.
    let plain = spell("Growl", None);
    assert_eq!(active_aura_press(slot, Some(&running), Some(&plain)), None);
    // Live but not cancelable.
    let uncancelable = pet_running(2645, 0x2);
    assert_eq!(active_aura_press(slot, Some(&uncancelable), Some(&d)), None);
    // A different spell's aura, and no pet descriptor at all.
    assert_eq!(
        active_aura_press(slot, Some(&pet_running(768, 0x3)), Some(&d)),
        None
    );
    assert_eq!(active_aura_press(slot, None, Some(&d)), None);
    // A command slot with the same action: the type decides.
    assert_eq!(
        active_aura_press(packed(2645, PET_ACT_COMMAND), Some(&running), Some(&d)),
        None
    );
}

#[test]
fn an_active_pet_spell_draws_its_active_icon() {
    let slot = packed(2645, benilla_protocol::messages::PET_TYPE_SPELL_FIRST);
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let d = toggle_spell();

    let idle = slot_view(slot, &bar, Some(&d), None, false, false);
    assert_eq!(idle.texture, d.icon);
    let active = slot_view(slot, &bar, Some(&d), None, false, true);
    assert_eq!(active.texture, d.active_icon);
    assert_eq!(
        active.name, idle.name,
        "only the icon swaps — the name, rank and autocast flags are untouched"
    );
    assert!(
        !active.active,
        "and it is still not `isActive`: a spell slot pushes nil there on every path"
    );
}

/// The reference pushes nil when the chosen icon is not in `SpellIcon.dbc` (`0x4bdd50`).
#[test]
fn an_unresolvable_active_icon_hides_rather_than_falling_back() {
    let slot = packed(2645, benilla_protocol::messages::PET_TYPE_SPELL_FIRST);
    let bar = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let d = benilla_formats::SpellDisplay {
        active_icon_id: 9999,
        active_icon: None,
        ..spell("Cower", None)
    };
    assert!(slot_view(slot, &bar, Some(&d), None, false, true)
        .texture
        .is_none());
}

#[test]
fn the_menu_fork_reads_abandon_and_rename_off_the_right_bits() {
    // A freshly tamed hunter pet carries both.
    assert_eq!(menu_predicates(0x30), (true, true));
    // After its first rename the server clears the rename bit.
    assert_eq!(menu_predicates(0x20), (true, false));
    // A warlock's demon carries neither.
    assert_eq!(menu_predicates(0), (false, false));
    assert_eq!(menu_predicates(0x10), (false, true));
    // 0x8 is PLAYER_CONTROLLED, on every pet; 0x40 is PLUS_MOB.
    assert_eq!(menu_predicates(0x8 | 0x40), (false, false));
}

#[test]
fn only_the_same_pets_moving_timestamp_reads_as_a_rename() {
    // Nothing seen before: a login, or the pet just streamed.
    assert!(!was_renamed(None, (0xF14, Some(100))));
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xF14, Some(100))));
    assert!(was_renamed(Some((0xF14, Some(100))), (0xF14, Some(200))));
    // A stamp first arriving is a move: a pet named for the first time.
    assert!(was_renamed(Some((0xF14, None)), (0xF14, Some(200))));
    // A different pet is never a rename, whatever the stamps do.
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xABC, Some(200))));
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xABC, Some(100))));
}

/// `PetDismiss` (`0x4be4d0`) stages `0x07000003` for the bar's dispatcher.
#[test]
fn the_dismiss_word_is_the_carved_literal() {
    assert_eq!(
        PET_COMMAND_DISMISS | (u32::from(PET_ACT_COMMAND) << 24),
        0x0700_0003,
    );
    let entry = PetActionEntry::from(0x0700_0003u32);
    assert_eq!(entry.kind(), PET_ACT_COMMAND);
    assert_eq!(entry.action(), PET_COMMAND_DISMISS);
    assert!(!entry.is_spell(), "dismiss is a command, never a cast");
}

/// The reference fires `PET_BAR_UPDATE_COOLDOWN` from the pet's cooldown bank (`0x6e2e8e`).
#[test]
fn a_cooldown_alone_fires_the_cooldown_event_and_not_the_bar_update() {
    use bevy::prelude::*;
    use std::collections::HashMap;
    const CLAW: u32 = 3010;
    let claw = || benilla_formats::SpellDisplay {
        recovery_ms: 8_000,
        ..spell("Claw", None)
    };
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    bar.spells.bar[3] = packed(CLAW, PET_ACT_ENABLED);
    let mut app = App::new();
    app.init_resource::<crate::ui_script::UiClock>()
        .init_resource::<crate::net::GuidIndex>()
        .insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([(CLAW, claw())])),
            ..crate::ui_action::Spells::empty_for_tests()
        })
        .insert_resource(bar)
        .add_systems(Update, super::bar::feed_pet_bar);
    let script = benilla_ui::script::UiScript::new().unwrap();
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
        .unwrap();
    app.insert_non_send_resource(script);
    let seen = |app: &mut App| -> Vec<String> {
        app.update();
        let mut s = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        s.resolve();
        let out = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        out
    };
    assert_eq!(
        seen(&mut app),
        vec!["PET_BAR_UPDATE".to_string()],
        "the bar arrives"
    );
    assert_eq!(seen(&mut app), Vec::<String>::new(), "nothing moved");

    app.world_mut()
        .resource_mut::<PetBar>()
        .cooldowns
        .start_spell(CLAW, &claw(), 0, std::time::Instant::now());
    assert_eq!(
        seen(&mut app),
        vec!["PET_BAR_UPDATE_COOLDOWN".to_string()],
        "a cooldown alone is the bank's edge, not the bar's"
    );
    let (start, duration, enable): (f64, f64, i32) = app
        .world_mut()
        .non_send_resource_mut::<benilla_ui::script::UiScript>()
        .eval("return GetPetActionCooldown(4)")
        .unwrap();
    assert!(
        duration > 7.9 && duration < 8.1,
        "the triple was pushed: {start} {duration}"
    );
    assert_eq!(enable, 1);
}

/// `0x4bcc19` calls `0x4bd190`, which copies the toggled word into the pet spellbook.
#[test]
fn a_bar_autocast_toggle_reaches_the_pet_spellbook() {
    const CLAW: u32 = 16827;
    const GROWL: u32 = 2649;
    let mut bar = PetBar {
        spells: PetSpells {
            pet_guid: 0x2A,
            ..Default::default()
        },
        ..Default::default()
    };
    bar.spells.bar[3] = packed(CLAW, PET_ACT_ENABLED); // autocast allowed + on
    bar.spells.spells = vec![
        packed(GROWL, PET_ACT_ENABLED),
        packed(CLAW, PET_ACT_ENABLED),
    ];

    let flipped = toggle_slot_autocast(&mut bar, 4).expect("an autocastable slot toggles");
    assert!(!flipped.autocast_on());
    assert!(!bar.spells.bar[3].autocast_on(), "the bar slot flipped");
    assert!(
        !bar.spells.spells[1].autocast_on(),
        "…and the book's CLAW entry got the slot's whole word"
    );
    assert_eq!(bar.spells.spells[1].packed, flipped.packed);
    assert!(
        bar.spells.spells[0].autocast_on(),
        "a different spell's book entry is untouched"
    );

    toggle_slot_autocast(&mut bar, 4);
    assert!(bar.spells.spells[1].autocast_on());

    // `0x4bcbf1`: a word without bit 31 aborts before any write, book included.
    bar.spells.bar[5] = packed(3025, PET_ACT_PASSIVE);
    bar.spells.spells.push(packed(3025, PET_ACT_PASSIVE));
    assert!(toggle_slot_autocast(&mut bar, 6).is_none());
    assert_eq!(bar.spells.spells[2], packed(3025, PET_ACT_PASSIVE));
}

#[test]
fn the_session_end_tears_the_pet_bar_down() {
    let mut app = bevy::prelude::App::new();
    app.add_plugins(UiPetPlugin);
    {
        let mut bar = app.world_mut().resource_mut::<PetBar>();
        bar.spells.pet_guid = 0x2A;
        bar.spells.bar[3] = packed(16827, PET_ACT_ENABLED);
    }

    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![benilla_protocol::SessionEvent::Disconnected {
            reason: "socket".into(),
            end: benilla_protocol::SessionEnd::Lost,
        }],
    );

    let bar = app.world().resource::<PetBar>();
    assert_eq!(
        bar.spells.pet_guid, 0,
        "no pet bar carried into the next session"
    );
    assert!(bar.spells.bar[3].is_empty());
}
