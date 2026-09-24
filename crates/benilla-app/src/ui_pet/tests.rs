//! The pet system's unit tests — one module for all three concerns, because they share one set of
//! fixtures (a packed word, a slot view, a `PetSpells` state) and splitting them per module would
//! fork those three ways.

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

/// [`slot_view`] for a slot that is **not** showing active — the ordinary case, and the only
/// one the token/spell/cooldown tests care about. The showing-active leg has its own tests,
/// which call `slot_view` directly so the flag is visible at the call.
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

/// A command token returns GLOBAL NAMES for both name and texture, and is lit exactly when it
/// IS the pet's current command — the whole reason three command buttons can share one state.
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

/// The Attack button's second clause: the attack latch lights it and arms the call-off fork
/// **independently of the command state**, which is how a pet told to Follow can still show
/// Attack lit while it is on something.
#[test]
fn the_attack_latch_lights_attack_whatever_the_command_state_says() {
    let following = state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE);
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);

    let idle = view(attack, &following, None, None, false);
    assert!(!idle.active && !idle.attack_active);

    let on = view(attack, &following, None, None, true);
    assert!(on.active, "the latch lights it even on a FOLLOW command");
    assert!(on.attack_active, "and the next press calls the pet off");

    // The latch never reaches another command's button, however busy the pet is.
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

/// Reaction tokens use the `PET_MODE_*` keys — the state words — not the right-click menu's
/// `PET_AGGRESSIVE` imperatives. Identical in enUS, distinguishable only by key.
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

/// A DISABLED bar (bit 27) changes what both token classes report, and neither is a special
/// case we wrote: the reaction compare's left side is forced to Passive, and the command
/// compare — unmasked `state >> 8` — is put out of range of every command at once.
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

/// `GetPetActionsUsable` — bit 27 OR the pet's own crowd-control flags. The second half is the
/// leg benilla was missing: a feared/stunned/confused pet's bar greys.
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
    // POSSESSED is deliberately not one of them — that IS the pet-bar case.
    assert!(actions_usable(&bar, Some(0x0100_0000)));

    bar.spells.state |= PET_STATE_BAR_DISABLED;
    assert!(!actions_usable(&bar, Some(0)));
}

/// A spell slot: real name, rank subtext, icon PATH — and the autocast pair read off **bits
/// 31/30**, not the type byte. `isActive` is nil on every spell path.
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

    // A passive pet spell shows, but can never autocast — no ring, no sparkle.
    let passive = view(
        packed(3010, PET_ACT_PASSIVE),
        &bar,
        Some(&claw),
        None,
        false,
    );
    assert!(!passive.autocast_allowed && !passive.autocast_enabled);
}

/// The two routes to an empty-looking button: the client's zero WORD, and vmangos's own
/// `(0, ACT_DISABLED)` filler, which takes the spell branch and misses the catalog. Both must
/// draw nothing — reading either one wrong puts four "?" buttons mid-bar.
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
            // Draws nothing and still CARRIES its word (decision 1010) — this is the exact
            // slot the drop core hunts for as a relocation candidate (type 1, low 16 zero),
            // so zeroing it here would both send the wrong word and lose the candidate.
            packed: filler.packed,
            ..Default::default()
        },
        "…but spell id 0 resolves to nothing, so the button is still empty"
    );
}

/// A spell the catalog cannot name draws nothing rather than a nameless icon — and with it
/// goes the autocast pair, which the client also gates on the record resolving.
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

/// A type outside 1–7 is inert. The client's own default arm under-pushes here (`0x4bde6f`);
/// we answer the empty slot instead, and in particular never reach the spell catalog with an
/// arbitrary number.
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

/// The slot index map is 1-based and bounded — a stale index from the VM cannot read past the
/// ten words or wrap into slot 10.
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

/// The local latch, which exists because the server confirms none of it (module doc), written
/// with the client's own masks: a command press moves bits 8–15 and keeps byte 0 **and bit
/// 27**, a reaction press moves byte 0 only, a spell press moves neither — and DISMISS moves
/// nothing, because it ends the pet rather than becoming its standing order.
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

/// **The director's report: clicking a lit Follow or Passive un-toggles it.**
///
/// The CheckButton flips itself before `OnClick` runs and the reference's first line is
/// `this:SetChecked(0)`, so every press starts by taking the light off. `0x4bc940`/`0x4bc960`
/// put it back by signalling `PET_BAR_UPDATE` **unconditionally** — no old-vs-new compare — so
/// pressing the mode the pet is already in still repaints. Our feed dedups on what it pushed,
/// so without a counter in that key the light had no way back.
///
/// A press that the reference does NOT signal must not bump: DISMISS and a spell reach
/// `0x4bd444` straight, and a hunter's ATTACK skips `0x4bd429` with the latch it never raises.
#[test]
fn every_press_the_reference_signals_forces_a_repaint() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let signals = |b: &PetBar| b.bar_signals;

    // Pressing the mode the pet is ALREADY in: nothing about the state moves, and the signal
    // still fires. This is the whole bug.
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

    // A POSSESSED unit's Attack does signal — `0x4bd429`, one instruction before the latch.
    latch_press(&mut bar, packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND), true);
    assert_ne!(signals(&bar), before);
}

/// ATTACK is the one command press that can raise the attack latch — **and only while we are
/// possessing the unit** (`0x4bd420`). On an ordinary pet bar the arm is unreachable, which is
/// the whole reason its button never checks.
#[test]
fn only_a_possessed_units_attack_press_raises_the_latch() {
    let mut bar = PetBar {
        spells: state(PET_COMMAND_FOLLOW, PET_REACT_DEFENSIVE),
        ..Default::default()
    };
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    assert!(!bar.attacking);

    // A hunter's pet: the press goes out (the caller sends regardless) and latches nothing.
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

/// `0x5ee5a0` itself — the compare that decides all of the above.
///
/// This is the fact the pet bar was missing: the reference is not asking "is this my pet", it
/// is asking "is this the unit I am *driving*". A hunter pet answers no on the flag, so no
/// hunter press ever reaches the latch.
#[test]
fn the_latch_gate_is_possession_not_ownership() {
    const FLAGS: u16 = 46;
    const CHARMEDBY: u16 = 10;
    const CREATEDBY: u16 = 14;
    let unit =
        |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
    let me = Some(0x77u64);

    // An ordinary hunter pet: ours, in combat, not possessed. The answer is no.
    let hunter_pet = unit(&[
        (CREATEDBY, 0x77),
        (CREATEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert!(!possessing(Some(&hunter_pet), me));

    // The same pet under Eyes of the Beast — possessed, and now the latch arm is live.
    let driven = unit(&[
        (CREATEDBY, 0x77),
        (CREATEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_POSSESSED),
    ]);
    assert!(possessing(Some(&driven), me));

    // A mind-controlled mob: CHARMEDBY is the primary leg, so it needs no CREATEDBY at all.
    let charmed = unit(&[(CHARMEDBY, 0x77), (CHARMEDBY + 1, 0), (FLAGS, 0x0100_0000)]);
    assert!(possessing(Some(&charmed), me));

    // Somebody ELSE's possessed unit is not ours to latch for.
    let theirs = unit(&[
        (CHARMEDBY, 0x99),
        (CHARMEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_POSSESSED),
    ]);
    assert!(!possessing(Some(&theirs), me));

    // No descriptor is no possession — the honest reading of missing data, and the same
    // posture the rest of the bar takes.
    assert!(!possessing(None, me));
}

/// **The director's report, twice over: the Attack button is never a toggle.**
///
/// Reported by eye against the reference client — *"there is no way in the ref client for the
/// attack button to stay lit up/toggled like the follow or stay… it's simply a button to send
/// the pet"* — and the first fix was wrong because it took the latch's *lifetime* to be the
/// question. It is not: the latch never rises at all on a pet bar, because `0x4bd420` gates it
/// on possession. This drives a whole pet's worth of ATTACK presses and asserts the button
/// stays dark through every one.
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

    // Stay is still reachable as a mode — the command byte was never hijacked.
    latch_press(&mut bar, packed(PET_COMMAND_STAY, PET_ACT_COMMAND), false);
    assert_eq!(bar.spells.command_state(), PET_COMMAND_STAY);
}

/// The **possess** bar is the other half of the same button, and it does light — which is why
/// the `isActive` clause, `IsPetAttackActive` and `PetStopAttack` are all still modelled rather
/// than deleted as dead weight.
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

    // The call-off: `PetStopAttack` clears the latch, and the latch was the ONLY thing lighting
    // the button — so it goes out.
    bar.attacking = false;
    let called_off = view(attack, &bar.spells, None, None, bar.attacking);
    assert!(!called_off.active);
    assert!(!called_off.attack_active);
}

/// **A refused ATTACK costs everything** — no packet, no latch, no light. The reference's veto
/// jumps to the function epilogue rather than the shared send (`0x4bd414 je 0x4bd4c6`), so a
/// pet that is dead, stunned, feared, confused, pacified or charmed away simply does not take
/// the order, and the button must not report that it did.
///
/// This is the join decision 0998 could not test: the gate and the latch, composed.
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

    // The same press with the gate clear is the ordinary attack, unchanged. The two gates are
    // NOT symmetric: this one costs the packet, possession costs only the latch.
    assert!(commit_press(&mut bar, attack, false, true));
    assert!(bar.attacking);
    assert!(view(attack, &bar.spells, None, None, bar.attacking).active);
}

/// **Touching your target calls the pet off** — `0x493910`'s `0x493a18`, the second call site
/// of `PetStopAttack`'s core and the one benilla was missing.
///
/// This is the mechanism behind the director's report that Attack is not a toggle. Without it
/// the latch had only two ways down (the lit button's own second press and a new pet), so an
/// order lit the button for the rest of the pet's life and read exactly like the Stay/Follow
/// modes it is supposed to contrast with.
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

    // Selecting when nothing was selected is NOT a clear — `0x493937` returns before anything.
    assert!(!old_target_cleared(None, Some(7)));
    // Replacing one target with another IS, and so is dropping it: the guard is on the OLD
    // selection existing, never on the new one being empty.
    assert!(old_target_cleared(Some(7), Some(9)));
    assert!(old_target_cleared(Some(7), None));
    // A no-op re-select never reaches the clear at all (`0x493540`'s own dedup).
    assert!(!old_target_cleared(Some(7), Some(7)));

    assert!(stop_pet_attack(&mut bar, &commands));
    assert!(!bar.attacking, "the latch is down");
    assert!(
        matches!(rx.try_recv(), Ok(ClientCommand::PetStopAttack { pet_guid }) if pet_guid == 0xF14),
        "and the server is told, with the BAR's guid"
    );

    // The button is dark now, because the latch was the only thing lighting it.
    let attack = packed(PET_COMMAND_ATTACK, PET_ACT_COMMAND);
    assert!(!view(attack, &bar.spells, None, None, bar.attacking).active);

    // Re-clearing is a no-op: `0x4bd65e` returns before the send when the latch is already
    // down, so a target change per second does not spray packets.
    assert!(!stop_pet_attack(&mut bar, &commands));
    assert!(rx.try_recv().is_err());
}

/// `PET_ATTACK_START`/`STOP` read the pet's **server-owned** in-combat flag, and only for a
/// unit we own — the correction to decision 0990, which derived them from the local click
/// latch. The owner test uses SUMMONEDBY as its fallback, which is the callback's own choice
/// and not the one `0x5ee5a0` makes for the same-shaped read.
#[test]
fn the_attack_events_read_the_pets_combat_flag_not_the_click_latch() {
    const FLAGS: u16 = 46;
    const CHARMEDBY: u16 = 10;
    const SUMMONEDBY: u16 = 12;
    let unit =
        |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
    let me = Some(0x77u64);

    // Ours by SUMMONEDBY, fighting / not fighting.
    let mine = |flags: u32| unit(&[(SUMMONEDBY, 0x77), (SUMMONEDBY + 1, 0), (FLAGS, flags)]);
    assert_eq!(
        pet_combat_flag(&mine(UNIT_FLAG_PET_IN_COMBAT), me),
        Some(true)
    );
    assert_eq!(pet_combat_flag(&mine(0), me), Some(false));
    // An unrelated flag bit is not combat — the callback tests one bit.
    assert_eq!(pet_combat_flag(&mine(0x1000), me), Some(false));

    // Somebody else's minion never fires, however hard it is fighting.
    let theirs = unit(&[
        (SUMMONEDBY, 0x99),
        (SUMMONEDBY + 1, 0),
        (FLAGS, UNIT_FLAG_PET_IN_COMBAT),
    ]);
    assert_eq!(pet_combat_flag(&theirs, me), None);

    // CHARMEDBY wins over SUMMONEDBY: a mob WE mind-controlled is ours even though it was
    // summoned by nobody, and a minion charmed AWAY from us stops being ours.
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

/// Only the ATTACK order consults the validator. The reference's type-7 arm branches exactly
/// twice, and every other action — the two modes, DISMISS, and anything `>= 4` — leaves down a
/// path that sends unconditionally. Gating them too would make a stunned pet impossible to
/// dismiss or to put back on Follow, which is not what the binary does.
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
    // A REACTION slot whose action happens to equal ATTACK's 2 (that is Aggressive) is a
    // different word entirely — the type byte decides, never the action alone.
    assert!(!is_attack_order(packed(
        PET_REACT_AGGRESSIVE,
        PET_ACT_REACTION
    )));
}

/// A pet with one aura in slot 0. `AURAFLAGS` is nibble-packed 8 slots to the u32, so slot 0's
/// nibble is the low four bits: `0x2` is an effect-index bit (what makes the slot *live*) and
/// `0x1` is `AFLAG_CANCELABLE` — the bit `0x4bcea0` actually tests.
fn pet_running(spell_id: u32, nibble: u32) -> ObjectStore {
    const AURA: u16 = 47;
    const AURAFLAGS: u16 = 95;
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (AURA, spell_id),
        (AURAFLAGS, nibble),
    ]))
}

/// A spell with an active icon — the shape the predicate needs before it can ever fire.
fn toggle_spell() -> benilla_formats::SpellDisplay {
    benilla_formats::SpellDisplay {
        active_icon_id: 122,
        active_icon: Some("Interface\\Icons\\Ability_Druid_Cower".into()),
        ..spell("Cower", Some("Rank 1"))
    }
}

/// The showing-active predicate, all four of its ways to say no. The pet-side twin of
/// `0x4e55f0` is the *same function* we already had, so what these assert is the wiring: the
/// pet's store goes in, and the three tests are the reference's three.
#[test]
fn a_pet_spell_shows_active_only_while_it_is_a_live_cancelable_aura_on_the_pet() {
    let slot = packed(2645, benilla_protocol::messages::PET_TYPE_SPELL_FIRST);
    let running = pet_running(2645, 0x3);
    let d = toggle_spell();

    assert_eq!(
        active_aura_press(slot, Some(&running), Some(&d)),
        Some(2645)
    );

    // No ActiveIconID: never a toggle — and that gate is on the SEND too, so this spell
    // re-casts rather than cancelling however live its aura is (the reference's own quirk).
    let plain = spell("Growl", None);
    assert_eq!(active_aura_press(slot, Some(&running), Some(&plain)), None);
    // Live but NOT cancelable (effect-index bit only).
    let uncancelable = pet_running(2645, 0x2);
    assert_eq!(active_aura_press(slot, Some(&uncancelable), Some(&d)), None);
    // A different spell's aura, and no pet descriptor at all.
    assert_eq!(
        active_aura_press(slot, Some(&pet_running(768, 0x3)), Some(&d)),
        None
    );
    assert_eq!(active_aura_press(slot, None, Some(&d)), None);
    // A COMMAND slot whose action equals the spell id is a different word entirely — the type
    // byte decides here exactly as it does for the ATTACK gate.
    assert_eq!(
        active_aura_press(packed(2645, PET_ACT_COMMAND), Some(&running), Some(&d)),
        None
    );
}

/// The icon swap itself: `ActiveIconID`'s texture replaces `SpellIconID`'s while the spell is
/// running, and the button keeps everything else it had.
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

/// An active spell whose `ActiveIconID` does not resolve in `SpellIcon.dbc` pushes **nil**,
/// not the inactive art. The reference looks up whichever id the predicate chose and gives up
/// if that lookup fails (`0x4bdd50`) — falling back would draw "not running" on a running
/// spell, which is worse than drawing nothing.
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

/// **The menu's fork, and the direction of each mask** (decision 1066).
///
/// This is the test that exists because the two bits are one nibble apart and the failure is
/// silent both ways: a hunter offered *Dismiss* loses the taming chain's only exit, and a warlock
/// offered *Abandon* is offered a row the reference never shows them.
#[test]
fn the_menu_fork_reads_abandon_and_rename_off_the_right_bits() {
    // A freshly tamed hunter pet carries both: abandon/rename/paperdoll show, dismiss hides.
    assert_eq!(menu_predicates(0x30), (true, true));
    // …and after its first rename the server clears only the rename bit.
    assert_eq!(menu_predicates(0x20), (true, false));
    // A warlock's demon carries neither — the whole menu flips to Dismiss.
    assert_eq!(menu_predicates(0), (false, false));
    // The masks are not interchangeable, which is the point of the test.
    assert_eq!(menu_predicates(0x10), (false, true));
    // Neighbouring flags are not these two: 0x8 is PLAYER_CONTROLLED (on every pet) and 0x40 is
    // PLUS_MOB. Reading the pair as a byte rather than as two bits would swallow both.
    assert_eq!(menu_predicates(0x8 | 0x40), (false, false));
}

/// The rename signal: a pet's cached name is dropped when *that pet's* timestamp moves, and on no
/// other transition. The guid half of the pair is what keeps the NEXT pet from reading as a
/// rename of the last one.
#[test]
fn only_the_same_pets_moving_timestamp_reads_as_a_rename() {
    // Nothing seen before — a login, or the pet's object just streamed.
    assert!(!was_renamed(None, (0xF14, Some(100))));
    // The same pet, the same stamp: the ordinary frame, which is nearly all of them.
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xF14, Some(100))));
    // The same pet, a moved stamp: the rename.
    assert!(was_renamed(Some((0xF14, Some(100))), (0xF14, Some(200))));
    // A field that had never been sent starting to arrive is also a move — a pet named for the
    // first time goes from absent to a stamp, and its cached name is just as stale.
    assert!(was_renamed(Some((0xF14, None)), (0xF14, Some(200))));
    // A DIFFERENT pet is never a rename, whatever the stamps do — dismiss one pet, summon
    // another, and the second must not have its name dropped on the first's account.
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xABC, Some(200))));
    assert!(!was_renamed(Some((0xF14, Some(100))), (0xABC, Some(100))));
}

/// **Dismiss is a bar press with no button.** `PetDismiss 0x4be4d0` opens no packet: it stages
/// the packed word `0x07000003` and hands it to the same dispatcher every pet-bar click uses, so it
/// leaves as `CMSG_PET_ACTION`.
///
/// The literal is pinned here against the word the constants build, because the two ways of saying
/// it are the two halves of the finding, and the first draft of this feature sent
/// `CMSG_PET_ABANDON` for both rows — which vmangos would have honoured, silently.
#[test]
fn the_dismiss_word_is_the_carved_literal() {
    assert_eq!(
        PET_COMMAND_DISMISS | (u32::from(PET_ACT_COMMAND) << 24),
        0x0700_0003,
    );
    // …and it decodes back as the command token it is, through the client's own masked read.
    let entry = PetActionEntry::from(0x0700_0003u32);
    assert_eq!(entry.kind(), PET_ACT_COMMAND);
    assert_eq!(entry.action(), PET_COMMAND_DISMISS);
    assert!(!entry.is_spell(), "dismiss is a command, never a cast");
}

/// The bar's two events are two edges (1953): a change of the slots fires `PET_BAR_UPDATE`; a
/// change of the cooldown triples alone fires `PET_BAR_UPDATE_COOLDOWN` — the reference's
/// cooldown-subsystem fire for the pet bank (`0x6e2e8e`) — and the
/// pushed state carries the new triple either way.
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

/// **A bar toggle reaches the spellbook**: after the
/// local write-back, `0x4bcc19` calls `0x4bd190(&bar[slot])`, which scans the raw pet-spell
/// array backwards for the entry equal to the slot under `& 0x3FFFFFFF` and copies the slot's
/// FULL word into it. The book renders from that array, so without the copy the Pet tab kept
/// the old autocast ring until the next `SMSG_PET_SPELLS`. The book→bar direction is
/// `ui_pet_book::flip_autocast` (decision 1032); this is its twin.
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

    // And back on: the copy is the full word, both directions.
    toggle_slot_autocast(&mut bar, 4);
    assert!(bar.spells.spells[1].autocast_on());

    // `0x4bcbf1`: a word without bit 31 aborts before anything is written, book included.
    bar.spells.bar[5] = packed(3025, PET_ACT_PASSIVE);
    bar.spells.spells.push(packed(3025, PET_ACT_PASSIVE));
    assert!(toggle_slot_autocast(&mut bar, 6).is_none());
    assert_eq!(bar.spells.spells[2], packed(3025, PET_ACT_PASSIVE));
}

/// **The pet bar dies with the session.** Only the zero-guid `SMSG_PET_SPELLS` cleared it, and a
/// dropped socket never sends one — so the next session kept the old pet's bar: presses went out
/// as `CMSG_PET_ACTION` to a guid that no longer exists, and the stable read `has_live_pet`.
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
