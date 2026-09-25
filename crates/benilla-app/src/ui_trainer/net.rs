//! The trainer window's packet handlers, filling [`TrainerOpen`] and [`TrainerErrors`].

use benilla_protocol::messages::TrainerSpell;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{TrainerErrors, TrainerOpen};
use crate::net::NetHandlerApp;

/// Register the trainer handlers, one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TrainerList, on_list)
        .net_handler(K::TrainerBuySucceeded, on_buy_succeeded)
        .net_handler(K::TrainerBuyFailed, on_buy_failed)
        // Five of the twelve re-evaluation triggers, as second handlers on their kinds; the feed
        // re-derives after every handler has run, so registration order does not matter.
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::PetSpells, on_pet_spells)
        .net_handler(K::SpellModifier, on_spell_modifier)
        .net_handler(K::Disconnected, on_session_end);
}

/// `SMSG_SET_FLAT/PCT_SPELL_MODIFIER`, the two talent triggers (`0x6e999f`/`0x6e99bb`).
fn on_spell_modifier(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellModifier { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell entered the book (`0x5e9fb7`).
fn on_spell_learned(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellLearned { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A spell left the book (`0x5ea25f`).
fn on_spell_removed(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellRemoved { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// A rank replaced another, so the known-at-a-higher-rank answers may have moved.
fn on_spell_superceded(In(ev): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    if matches!(ev, SessionEvent::SpellSuperceded { .. }) {
        trainer_open.trigger_re_derive();
    }
}

/// The pet's spellbook arrived or changed (`0x4bdaf5`).
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

/// An open trainer window ends with the session.
fn on_session_end(In(_): In<SessionEvent>, mut trainer_open: ResMut<TrainerOpen>) {
    trainer_open.clear_session();
}

/// `SMSG_TRAINER_LIST`: open the window on the trainer's services.
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

/// `SMSG_TRAINER_BUY_SUCCEEDED`, logged only: the reference registers no handler for it
/// (`0x5ab650`/`0x537a60`), the bought row repaints on `SMSG_LEARNED_SPELL`'s re-evaluation, and
/// vmangos sends no fresh list (`NPCHandler.cpp:262-343`).
fn trainer_buy_succeeded(trainer: u64, spell_id: u32) {
    debug!("net: trainer {trainer:#x} taught spell {spell_id}");
}

/// `SMSG_TRAINER_BUY_FAILED`: queue the code for the feed to log.
fn trainer_buy_failed(error: u32, errors: &mut TrainerErrors) {
    debug!("net: trainer buy failed (code {error})");
    errors.0.push(error);
}
