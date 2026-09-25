//! The trainer window's packet handlers (in the net handler table since 2318,
//! moved out of the drain's npc arm file) — the [`TrainerOpen`] session and the
//! [`TrainerErrors`] line queue the trainer feed ([`super`]) reads.

use benilla_protocol::messages::TrainerSpell;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{TrainerErrors, TrainerOpen};
use crate::net::NetHandlerApp;

/// Register the trainer handlers — called from [`super::UiTrainerPlugin`]. One per kind, plus
/// the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TrainerList, on_list)
        .net_handler(K::TrainerBuySucceeded, on_buy_succeeded)
        .net_handler(K::TrainerBuyFailed, on_buy_failed)
        // Five of the reference's twelve re-evaluation triggers arrive as packets (2333, 2336):
        // a spell learned, removed or superseded, a spell modifier, and the pet's spellbook. Second handlers on the
        // kind — the feed does the re-derivation later in the frame, so it reads the book as it
        // is after every handler on the packet has run, whatever the registration order.
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::PetSpells, on_pet_spells)
        .net_handler(K::SpellModifier, on_spell_modifier)
        .net_handler(K::Disconnected, on_session_end);
}

/// A spell modifier arrived (`SMSG_SET_FLAT/PCT_SPELL_MODIFIER` — the reference's two "talent"
/// callers, `0x6e999f`/`0x6e99bb`, 2336): the same re-derivation.
fn on_spell_modifier(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellModifier { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell entered the book while a trainer is open (`0x5e9fb7`): re-derive on the next feed.
fn on_spell_learned(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellLearned { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell left the book (`0x5ea25f`): the same.
fn on_spell_removed(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellRemoved { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A rank replaced another: the book moved, so the "known at a higher rank" answers may have.
fn on_spell_superceded(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellSuperceded { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// The pet's spellbook arrived or changed (`0x4bdaf5`): a pet-spell service may have flipped.
fn on_pet_spells(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::PetSpells(_)) {
        trainer_open.trigger_re_derive();
    }
}

fn on_list(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if let SessionEvent::TrainerList {
        trainer,
        trainer_type,
        services,
        greeting,
    } = ev
    {
        trainer_list(trainer, trainer_type, services, greeting, &mut trainer_open);
    }
}

fn on_buy_succeeded(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuySucceeded { trainer, spell_id } = ev {
        trainer_buy_succeeded(trainer, spell_id);
    }
}

fn on_buy_failed(In(ev): In<SessionEvent>, mut errors: ResMut<TrainerErrors>) {
    if let SessionEvent::TrainerBuyFailed { error, .. } = ev {
        trainer_buy_failed(error, &mut errors);
    }
}

/// An open trainer window dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    trainer_open.clear_session();
}

/// A trainer's service list (`SMSG_TRAINER_LIST`): fill the [`TrainerOpen`] the trainer feed
/// ([`super`]) reads.
fn trainer_list(
    trainer: u64,
    trainer_type: u32,
    services: Vec<TrainerSpell>,
    greeting: String,
    trainer_open: &mut TrainerOpen,
) {
    debug!(
        "net: trainer {trainer:#x} (type {trainer_type}) listed {} services",
        services.len()
    );
    trainer_open.open(trainer, trainer_type, services, greeting);
}

/// A trainer taught a service — confirmation only, and the reference **registers no handler for
/// this opcode** (0x1B3 is absent from the 387 opcodes registered via `0x5ab650`/`0x537a60`).
/// The spell itself lands via `SMSG_LEARNED_SPELL`, and that packet is one of the twelve triggers
/// of the state re-evaluator ([`super::reeval`]), which repaints the bought row
/// green→gray and unlocks the next rank from the player's own book. Until 2333 benilla answered
/// this packet by re-requesting the whole list — a round trip the reference never makes
/// (VERIFIED vmangos `HandleTrainerBuySpellOpcode`: the server never resends on its own either),
/// and the reason the player's filter kept dying on a purchase. Logged, nothing more.
fn trainer_buy_succeeded(trainer: u64, spell_id: u32) {
    debug!("net: trainer {trainer:#x} taught spell {spell_id}");
}

/// A trainer refused a purchase — the trainer window's error line.
fn trainer_buy_failed(error: u32, errors: &mut TrainerErrors) {
    debug!("net: trainer buy failed (code {error})");
    errors.0.push(error);
}
