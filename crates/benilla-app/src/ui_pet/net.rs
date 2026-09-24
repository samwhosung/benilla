//! The pet's packet handlers (decisions 0982, 0988, 2039; in the net handler table since 2321,
//! moved out of the drain's pet arm file) — `SMSG_PET_SPELLS`, `SMSG_PET_MODE`,
//! `SMSG_PET_ACTION_FEEDBACK`, `SMSG_PET_CAST_FAILED` folded into [`PetBar`], the three
//! pet-feedback packets and the pet's voice.
//!
//! The whole file is short because `SMSG_PET_SPELLS` is not a delta — it **is** the bar, so
//! applying it is a replace. These four arms own the bar's CONTENTS; its lit state is
//! [`super`]'s, latched on the press because the server never answers one (that file's
//! module doc has the why). The only state this file writes is what actually arrives on the wire.
//!
//! The two refusal arms share the player's red-line **queues** — one drain, one sink, one
//! GlobalStrings lookup — but they do **not** share his message table, and believing they did is
//! what decision 2033 corrects. The reference gives each of these packets its own handler with its
//! own reason -> errorId map (`0x4bdb70` for the feedback byte, `0x6e8eb0` for the cast refusal),
//! and both maps exist precisely to say "your **pet** is dead / rooted / out of range" where the
//! player's says "you are". So the caster rides the queue as far as [`cast_fail`], which picks the
//! table; everything downstream of that stays pet-unaware, as it should be.
//!
//! [`cast_fail`]: crate::ui_action

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{PetMode, PetSpells};
use benilla_protocol::{SessionEvent, SessionEventKind};

use benilla_assets::coords::wow_to_bevy;

use super::PetBar;
use crate::net::{GuidIndex, NetHandlerApp, PetDismissSoundMessage, PetTalkMessage};
use crate::ui_action::{CastErrors, PetTameFailures, Spells, UiError, UiErrorKeys};

/// Register the pet's handlers — called from [`super::UiPetPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PetSpells, on_spells)
        .net_handler(K::PetMode, on_mode)
        .net_handler(K::PetActionFeedback, on_action_feedback)
        .net_handler(K::PetCastFailed, on_cast_failed)
        .net_handler(K::PetTameFailure, on_tame_failure)
        .net_handler(K::PetNameInvalid, on_refusal_line)
        .net_handler(K::PetBroken, on_refusal_line)
        .net_handler(K::PetActionSound, on_action_sound)
        .net_handler(K::PetDismissSound, on_dismiss_sound)
        .net_handler(K::Disconnected, on_session_end);
}

/// The pet bar dies with the socket — a listener on the session end (a second handler on the kind,
/// after the bridge's own teardown). The zero-guid `SMSG_PET_SPELLS` is the only in-session
/// teardown, and a dropped socket never sends one: without this the next session kept the old
/// pet's bar, pressed `CMSG_PET_ACTION` at a guid that no longer exists, and the stable read a live
/// pet. The same reset as the zero guid's, cooldown store included.
fn on_session_end(In(_): In<SessionEvent>, mut bar: ResMut<PetBar>) {
    *bar = PetBar::default();
}

/// The pet action bar (decision 0982) — server-authoritative, so PET_SPELLS is a wholesale
/// replace and its zero-guid form is the teardown.
fn on_spells(In(ev): In<SessionEvent>, catalog: Option<Res<Spells>>, mut bar: ResMut<PetBar>) {
    if let SessionEvent::PetSpells(spells) = ev {
        pet_spells(*spells, catalog.as_deref(), &mut bar);
    }
}

fn on_mode(In(ev): In<SessionEvent>, mut bar: ResMut<PetBar>) {
    if let SessionEvent::PetMode(mode) = ev {
        pet_mode(mode, &mut bar);
    }
}

fn on_action_feedback(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::PetActionFeedback { reason } = ev {
        pet_action_feedback(reason, &mut errors);
    }
}

fn on_cast_failed(In(ev): In<SessionEvent>, mut errors: ResMut<CastErrors>) {
    if let SessionEvent::PetCastFailed { spell_id, reason } = ev {
        pet_cast_failed(spell_id, reason, &mut errors);
    }
}

fn on_tame_failure(In(ev): In<SessionEvent>, mut failures: ResMut<PetTameFailures>) {
    if let SessionEvent::PetTameFailure { reason } = ev {
        pet_tame_failure(reason, &mut failures);
    }
}

fn on_refusal_line(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    match ev {
        SessionEvent::PetNameInvalid => pet_name_invalid(&mut errors),
        SessionEvent::PetBroken => pet_broken(&mut errors),
        _ => {}
    }
}

fn on_action_sound(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut talks: MessageWriter<PetTalkMessage>,
) {
    if let SessionEvent::PetActionSound { pet_guid, talk } = ev {
        pet_action_sound(pet_guid, talk, &index, &mut talks);
    }
}

fn on_dismiss_sound(In(ev): In<SessionEvent>, mut sounds: MessageWriter<PetDismissSoundMessage>) {
    if let SessionEvent::PetDismissSound { model_id, position } = ev {
        pet_dismiss_sound(model_id, position, &mut sounds);
    }
}

/// `SMSG_PET_SPELLS` — replace the whole bar, and reseed the pet's own cooldown store from the
/// packet's tail.
///
/// A **zero guid** is the teardown (`Player::RemovePetActionBar`): the bar goes away and the
/// cooldown store goes with it, because the next pet is a different unit with different timers.
/// Everything else is a wholesale replace — including a re-send from the same pet, which is how a
/// learned spell, a mode change or an autocast toggle actually reaches the bar.
fn pet_spells(spells: PetSpells, catalog: Option<&Spells>, bar: &mut PetBar) {
    if spells.pet_guid == 0 {
        if bar.spells.pet_guid != 0 {
            debug!("net: pet bar torn down");
        }
        *bar = PetBar::default();
        return;
    }
    // A pet-GUID CHANGE clears the attack latch (`0x4bc8ce`, the client's own single writer of the
    // pet guid does it unconditionally) — a freshly summoned pet is not attacking, whatever the
    // last one was doing. A re-send from the SAME pet leaves it alone: a learned spell must not
    // silently call the pet off.
    if bar.spells.pet_guid != spells.pet_guid {
        bar.attacking = false;
    }
    debug!(
        "net: pet bar for {:#x} — react {} command {} {}, {} known spell(s), {} cooldown(s)",
        spells.pet_guid,
        spells.react_state(),
        spells.command_state(),
        if spells.bar_disabled() {
            "DISABLED"
        } else {
            "usable"
        },
        spells.spells.len(),
        spells.cooldowns.len(),
    );
    // The ten words, spelled out. Worth its own line rather than a raw hex dump: every question
    // this packet raises ("why is that slot empty", "why is nothing lit", "is that spell in the
    // catalog") is answered by seeing the type/action pair beside the name we resolved for it, and
    // reading that off `0xc1000bc2` by hand is exactly the step nobody does.
    debug!(
        "net: pet bar slots — {}",
        spells
            .bar
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let what = match e.kind() {
                    benilla_protocol::messages::PET_ACT_COMMAND => format!("cmd {}", e.action()),
                    benilla_protocol::messages::PET_ACT_REACTION => format!("react {}", e.action()),
                    _ if e.is_empty() => "empty".to_string(),
                    _ => match catalog.and_then(|c| c.catalog.get(e.action())) {
                        Some(d) => format!("{} ({})", d.name, e.action()),
                        None => format!("spell {} NOT IN CATALOG", e.action()),
                    },
                };
                format!("{}:{what}/{:#04x}", i + 1, e.kind())
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    // The spell LIST, spelled out the same way and for the same reason (decision 1032). It is a
    // different question from the bar's — "why is the pet book shorter than the packet said" — and
    // the answer is always one of three things this line names: the id resolves to no `Spell.dbc`
    // record, it carries `DO_NOT_DISPLAY` (the book's whole add-gate, `0x4b2f90`), or it is in.
    // vmangos sends the pet's runtime passives here alongside its real spells, so the gap between
    // the two counts is usually large and entirely correct.
    debug!(
        "net: pet spellbook — {}",
        spells
            .spells
            .iter()
            .map(|e| {
                match catalog.and_then(|c| c.catalog.get(e.action())) {
                    Some(d) if d.in_pet_book() => format!("{} ({})", d.name, e.action()),
                    Some(d) => format!("{} ({}) DO_NOT_DISPLAY", d.name, e.action()),
                    None => format!("{} NOT IN CATALOG", e.action()),
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let now = Instant::now();
    bar.cooldowns = crate::spell::Cooldowns::default();
    for cd in &spells.cooldowns {
        let display = catalog.and_then(|c| c.catalog.get(cd.spell_id));
        bar.cooldowns.seed_pet(cd, display, now);
    }
    bar.spells = spells;
}

/// `SMSG_PET_MODE` — the react/command state alone. Applied only when it names the pet whose bar
/// we actually hold: a mode packet for a unit we have no bar for has nothing to write into, and
/// taking its state anyway would light a reaction button on the wrong pet's bar.
fn pet_mode(mode: PetMode, bar: &mut PetBar) {
    if bar.spells.pet_guid == 0 || bar.spells.pet_guid != mode.pet_guid {
        return;
    }
    debug!("net: pet mode — state {:#010x}", mode.state);
    // The whole dword, verbatim — the client stores this packet and `SMSG_PET_SPELLS`' state field
    // through the same writer (`0x4bc930`), and bit 27 can only ever arrive this way.
    bar.spells.state = mode.state;
}

/// `SMSG_PET_ACTION_FEEDBACK` — one reason byte for a refused order, queued onto the red line by
/// GlobalStrings key ([`UiErrorKeys`], the `DisplayError` route). An unrecognised code queues
/// nothing, exactly as an absent key shows nothing.
fn pet_action_feedback(reason: u8, errors: &mut UiErrorKeys) {
    debug!("net: pet action feedback {reason}");
    if let Some(key) = pet_feedback_key(reason) {
        errors.0.push(UiError::key(key));
    }
}

/// The `SMSG_PET_ACTION_FEEDBACK` reason → the message-catalog row the reference raises for it.
///
/// **The whole map, read off the handler** (`0x4bdb70`, decision 2033). The byte is decremented
/// and bounded — `dec eax; cmp eax,0x3; ja` — then indexes the four-entry jump table at
/// `0x4bdbe8`, so `0` and anything from `5` up fall past every arm and display nothing. Each arm
/// is a bare `push <errorId>; call CGGameUI::DisplayError 0x496720`, with no argument and no
/// state write: this opcode is a message and nothing else.
///
/// | byte | vmangos `PetFeedback` | errorId | catalog row |
/// |---|---|---|---|
/// | `1` | `FEEDBACK_PET_DEAD` | `0x150` | `ERR_PET_SPELL_DEAD` |
/// | `2` | `FEEDBACK_NOTHING_TO_ATT` | `0x0a0` | `ERR_NO_ATTACK_TARGET` |
/// | `3` | `FEEDBACK_CANT_ATT_TARGET` | `0x0a1` | `ERR_INVALID_ATTACK_TARGET` |
/// | `4` | `FEEDBACK_NO_PATH_TO` | `0x151` | `ERR_PET_SPELL_NOPATH` |
///
/// The four line up one-for-one with `Pet.h`'s enum and with its four `SendPetActionFeedback`
/// call sites. **That is what the two-entry map this replaces got wrong**: it read the sends as
/// "exactly two" and paired them with `1`/`2`, so a dead pet's refusal said "No path available
/// for your pet", a targetless attack said "Out of range", and `3`/`4` said nothing at all.
///
/// **`ERR_PET_SPELL_NOPATH` has no `GlobalStrings.lua` string in 5875**, so a no-path order is
/// silent in the reference too — its own data-suppression face, not a gap here. The key
/// `PET_SPELL_NOPATH` ("No path available for your pet") *does* exist, which is what made the old
/// map look right on screen; it is not this row's key, and nothing in the shipped client raises
/// it.
fn pet_feedback_key(reason: u8) -> Option<&'static str> {
    Some(match reason {
        1 => "ERR_PET_SPELL_DEAD",
        2 => "ERR_NO_ATTACK_TARGET",
        3 => "ERR_INVALID_ATTACK_TARGET",
        4 => "ERR_PET_SPELL_NOPATH",
        _ => return None,
    })
}

/// `SMSG_PET_TAME_FAILURE` — the reason byte, straight onto [`PetTameFailures`] for the drain to
/// resolve (decision 2039).
///
/// **Not only taming, despite the name**: vmangos raises it for Call Pet with no pet available
/// (`SpellEffects.cpp:3167`), Revive Pet on a live pet (`Spell.cpp:6136`), and any summon while a
/// pet is already out (`Spell.cpp:5463`) — which is why the reason vocabulary carries
/// `PETTAME_NOPETAVAILABLE`, `PETTAME_DEAD` and `PETTAME_NOTDEAD`, three values the taming spell
/// itself can never produce.
///
/// No local state moves: the refusal is the server's last word on a spell that never took, and
/// the client's own cast bookkeeping was already unwound by the `SMSG_CAST_RESULT` that came with
/// it.
fn pet_tame_failure(reason: u8, failures: &mut PetTameFailures) {
    debug!(
        "net: pet tame failure {reason} ({})",
        benilla_protocol::messages::pet_tame_failure_key(reason)
    );
    failures.0.push(reason);
}

/// `SMSG_PET_NAME_INVALID` — the server refused the rename; raise `ERR_INVALID_PETNAME`.
///
/// The packet carries nothing at all, so the opcode IS the message — which is exactly what the
/// reference's arm does (`0x5e3e33`: `push 0xf7; call 0x496720`, no body read). Decision 1066
/// recorded the opposite ("a refused rename silently does nothing"), attributing
/// `SMSG_PET_BROKEN`'s handler to this opcode; the two are different functions.
///
/// Nothing else moves: the rename was optimistic-free by design (1066), so there is no local name
/// to roll back, and the popup has already closed.
fn pet_name_invalid(errors: &mut UiErrorKeys) {
    debug!("net: pet name refused");
    errors.0.push(UiError::key("ERR_INVALID_PETNAME"));
}

/// `SMSG_PET_BROKEN` — the pet's loyalty hit zero and it left; raise `ERR_PET_BROKEN`
/// ("Your pet has run away").
///
/// Empty body, and the reference's handler `0x4bdc00` is five instructions that read none of it:
/// the *bar* teardown is not this packet's job. vmangos sends it immediately before
/// `Unsummon(PET_SAVE_AS_DELETED)` (`Pet.cpp:822`), so the zero-guid `SMSG_PET_SPELLS` that
/// actually clears the bar arrives on its own heels — which is why touching the bar here would be
/// a second, racing teardown rather than a fix.
fn pet_broken(errors: &mut UiErrorKeys) {
    debug!("net: pet ran away");
    errors.0.push(UiError::key("ERR_PET_BROKEN"));
}

/// `SMSG_PET_ACTION_SOUND` — the pet's voice: resolve the guid, hand the selector to the sound
/// layer (decision 2039).
///
/// The reference resolves the packet's guid through the object manager and **drops the packet on
/// a miss** (`0x604101 je`), which is what the index lookup below is: a bark for a unit that has
/// not streamed in has nothing to play from and no position to play at.
///
/// The selector is not validated here — [`crate::sound::creature`]'s reader is where the
/// two-armed `cmp` lives, because that is where the reference has it too (`0x604106`/`0x60411c`).
fn pet_action_sound(
    pet_guid: u64,
    talk: u32,
    index: &GuidIndex,
    talks: &mut MessageWriter<PetTalkMessage>,
) {
    debug!("net: pet talk {talk} from {pet_guid:#x}");
    if let Some(&unit) = index.0.get(&pet_guid) {
        talks.write(PetTalkMessage { unit, talk });
    }
}

/// `SMSG_PET_DISMISS_SOUND` — the parting sound's model id and point, converted into Bevy space
/// for the sound layer (decision 2039).
///
/// The `+1.0` on `z` is the reference's own (`0x6041d0 fadd [0x7ff9d8]`), applied **before** the
/// basis change because it is a WoW-space offset: the point on the wire is where the pet stood,
/// and the kit sounds a yard above it.
fn pet_dismiss_sound(
    model_id: u32,
    position: [f32; 3],
    sounds: &mut MessageWriter<PetDismissSoundMessage>,
) {
    debug!("net: pet dismissed — model {model_id} at {position:?}");
    sounds.write(PetDismissSoundMessage {
        model_id,
        pos: wow_to_bevy([position[0], position[1], position[2] + 1.0]),
    });
}

/// `SMSG_PET_CAST_FAILED` — the pet's cast refusal, queued onto the SAME red line as our own
/// through [`CastErrors`], but **marked as the pet's** so the display picks the reference's pet
/// message table rather than the player's (`push_pet`, decision 2033).
///
/// The reference handles this opcode in its own function, `Spell_C::HandlePetCastFailed`
/// (`0x6e8eb0`), which is not the player's `0x6e1a00` with a flag — it is a separate switch over
/// a separate 142-byte index table. Ten reasons resolve differently there, six of them to the
/// `ERR_PET_SPELL_*` rows that exist for no other purpose ("Your pet is in combat." where the
/// player reads "You are in combat"). The queue is shared; the table is not.
///
/// Two things it deliberately does NOT do, both because the caster is the pet:
///
/// - **Touch our cast state.** There is no pending cast of ours to revert, no GCD of ours to
///   clear and no button of ours to unflash — the three things `cast_result`'s own failure path
///   does. Reusing that path would have made a pet's refused Growl cancel the player's cast bar.
/// - **Write a combat-log line.** `0x6e1a00` calls the log formatter `0x62c360` beside its
///   `DisplayError`; `0x6e8eb0` calls neither it nor the error-sound `0x458a50` — its whole call
///   set is the two packet readers, `0x496720`, and the string plumbing behind it. So a pet's
///   refusal never prints "You fail to cast Growl: ..." in the log, and the drain's combat twin
///   skips it.
fn pet_cast_failed(spell_id: u32, reason: Option<u8>, errors: &mut CastErrors) {
    debug!("net: pet cast failed — spell {spell_id} reason {reason:?}");
    if let Some(reason) = reason {
        errors.push_pet(spell_id, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{PetActionEntry, PET_ACT_COMMAND, PET_COMMAND_ATTACK};

    fn a_bar(guid: u64) -> PetSpells {
        let mut s = PetSpells {
            pet_guid: guid,
            state: 0x0101, // react Defensive, command Follow
            ..Default::default()
        };
        s.bar[0] = PetActionEntry::from(PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24));
        s
    }

    /// The teardown clears the bar AND its cooldown store — the next pet is a different unit, so
    /// carrying its predecessor's timers would sweep the new pet's buttons for no reason.
    #[test]
    fn a_zero_guid_tears_the_whole_bar_down() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);
        assert_eq!(bar.spells.pet_guid, 0x2A);

        pet_spells(PetSpells::default(), None, &mut bar);
        assert_eq!(bar.spells, PetSpells::default());
        assert!(!bar.has_bar());
    }

    /// A mode packet for a DIFFERENT unit is dropped — it would otherwise light a reaction button
    /// on the bar we are holding for somebody else's pet.
    #[test]
    fn pet_mode_only_writes_its_own_bar() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);

        pet_mode(
            PetMode {
                pet_guid: 0x99,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 1, "a stranger's mode is ignored");

        pet_mode(
            PetMode {
                pet_guid: 0x2A,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 2);
        assert_eq!(bar.spells.command_state(), 2);
        // The bar itself is untouched — PET_MODE carries no slots.
        assert_eq!(bar.spells.bar[0].action(), PET_COMMAND_ATTACK);
    }

    /// The attack latch is cleared by a pet CHANGE and survives a re-send from the same pet — the
    /// client's own single pet-guid writer does exactly this (`0x4bc8ce`). A learned spell must
    /// not silently call the pet off; a new pet must not inherit the last one's attack.
    #[test]
    fn the_attack_latch_survives_a_resend_and_dies_on_a_new_pet() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);
        bar.attacking = true;

        pet_spells(a_bar(0x2A), None, &mut bar);
        assert!(bar.attacking, "the same pet re-sending keeps its attack");

        pet_spells(a_bar(0x99), None, &mut bar);
        assert!(!bar.attacking, "a different pet is not attacking");
    }

    /// The whole feedback map, in the reference's own order (`0x4bdbe8`) — and the bound that
    /// makes `0` and everything from `5` up draw nothing rather than putting a bare number on the
    /// red line.
    ///
    /// Written as the four keys in sequence rather than four asserts because the defect this
    /// replaces was an *ordering* one: a two-entry map that paired the right vocabulary with the
    /// wrong bytes still looked plausible read one row at a time.
    #[test]
    fn the_feedback_map_is_the_jump_table_at_0x4bdbe8() {
        let mut errors = UiErrorKeys::default();
        for reason in [0u8, 1, 2, 3, 4, 5, 200] {
            pet_action_feedback(reason, &mut errors);
        }
        assert_eq!(
            errors.0.iter().map(|e| e.key).collect::<Vec<_>>(),
            [
                "ERR_PET_SPELL_DEAD",
                "ERR_NO_ATTACK_TARGET",
                "ERR_INVALID_ATTACK_TARGET",
                "ERR_PET_SPELL_NOPATH",
            ]
        );
    }

    /// Every arm names a real `DisplayError` row — which is the check the old map could not have
    /// passed, and was never asked to: neither `PET_SPELL_NOPATH` nor `SPELL_FAILED_OUT_OF_RANGE`
    /// is a catalog key, and `message_keys`' source walk only reads `ERR_*` literals, so both
    /// slipped past it. Naming the rows is also what gives these lines their surface and their
    /// error-speech: `ERR_NO_ATTACK_TARGET` and `ERR_INVALID_ATTACK_TARGET` carry type tags
    /// `0x26`/`0x0b`, so the client speaks them.
    #[test]
    fn every_feedback_arm_is_a_catalog_row() {
        for reason in 1..=4u8 {
            let key = pet_feedback_key(reason).expect("an arm");
            let row = benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a message-catalog row"));
            assert_eq!(
                row.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is the red line"
            );
        }
    }
}
