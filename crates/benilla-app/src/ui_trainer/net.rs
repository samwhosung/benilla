//! The trainer window's packet handlers: the list fills [`TrainerOpen`] and puts its
//! `TRAINER_SHOW` on the lane; five of the state pass's twelve callers put its `TRAINER_UPDATE`
//! there ([`super::pass_owed`]), the descriptor watches and the item callbacks the rest.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{TrainerLane, TrainerOpen};
use crate::net::NetHandlerApp;

/// Register the trainer handlers, one per kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::TrainerList, on_list)
        .net_handler(K::TrainerBuySucceeded, on_buy_succeeded)
        .net_handler(K::TrainerBuyFailed, on_buy_failed)
        // Five of the twelve callers of the state pass, as second handlers on their kinds, each
        // after the handler that changed what the pass reads: a learned spell (`0x5e9fb7`), a
        // removed one (`0x5ea25f`), a superseded one (two, `0x5e63a4` and `0x5e64f1`), the pet's
        // spellbook (`0x4bdaf5`) and the flat and percent spell modifiers (`0x6e999f`, `0x6e99bb`).
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::PetSpells, on_pet_spells)
        .net_handler(K::SpellModifier, on_spell_modifier)
        .net_handler(K::Disconnected, on_session_end);
}

/// The window and the lane a caller of the state pass `0x4d7d40` puts its pass on.
#[derive(bevy::ecs::system::SystemParam)]
struct Pass<'w> {
    open: Res<'w, TrainerOpen>,
    lane: ResMut<'w, TrainerLane>,
}

impl Pass<'_> {
    fn owed(&mut self, called: bool) {
        if called {
            super::pass_owed(Some(&self.open), Some(&mut self.lane));
        }
    }
}

fn on_spell_learned(In(ev): In<SessionEvent>, mut pass: Pass) {
    pass.owed(matches!(ev, SessionEvent::SpellLearned { .. }));
}

fn on_spell_removed(In(ev): In<SessionEvent>, mut pass: Pass) {
    pass.owed(matches!(ev, SessionEvent::SpellRemoved { .. }));
}

/// `0x5e6330`: with the active player, the old rank's unlearn passes 1 and runs no pass
/// (`0x5e6392`), the new rank's learn runs one (`0x5e63a4` → `0x5e9fb7`); then every arm runs the
/// handler's own (`0x5e64f1`).
fn on_spell_superceded(
    In(ev): In<SessionEvent>,
    mut pass: Pass,
    me: Query<(), With<crate::net::SelfPlayer>>,
) {
    let superceded = matches!(ev, SessionEvent::SpellSuperceded { .. });
    pass.owed(superceded && !me.is_empty());
    pass.owed(superceded);
}

fn on_pet_spells(In(ev): In<SessionEvent>, mut pass: Pass) {
    pass.owed(matches!(ev, SessionEvent::PetSpells(_)));
}

fn on_spell_modifier(In(ev): In<SessionEvent>, mut pass: Pass) {
    pass.owed(matches!(ev, SessionEvent::SpellModifier { .. }));
}

fn on_list(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::TrainerList {
        trainer,
        trainer_type,
        services,
        greeting,
    } = ev
    {
        debug!(
            "net: trainer {trainer:#x} (type {trainer_type}) listed {} services",
            services.len()
        );
        commands.queue(move |world: &mut World| {
            // `ReadTrainerList`'s test (`0x5dcc85`-`0x5dcca9`): the trainer asked (`0xc4d788`),
            // else the open gossip window's NPC (`0xbc3f58`, 0 with none open); any other list
            // opens nothing (`0x5dcd44`).
            let asked = world.resource::<TrainerOpen>().asked == trainer;
            let gossip = world
                .get_resource::<crate::ui_gossip::GossipState>()
                .and_then(|g| g.npc)
                .unwrap_or(0)
                == trainer;
            if !asked && !gossip {
                debug!("net: trainer {trainer:#x}'s list was not asked for — dropped");
                return;
            }
            super::open_trainer(world, trainer, trainer_type, services, greeting);
        });
    }
}

/// `SMSG_TRAINER_BUY_SUCCEEDED`, logged only: the reference registers no handler for it
/// (`0x5ab650`/`0x537a60`), the bought row repaints on `SMSG_LEARNED_SPELL`'s pass, and vmangos
/// sends no fresh list (`NPCHandler.cpp:262-343`).
fn on_buy_succeeded(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuySucceeded { trainer, spell_id } = ev {
        debug!("net: trainer {trainer:#x} taught spell {spell_id}");
    }
}

/// `SMSG_TRAINER_BUY_FAILED` (`0x5e5f10`): its line to the console only, inside the handler.
fn on_buy_failed(In(ev): In<SessionEvent>) {
    if let SessionEvent::TrainerBuyFailed { error, .. } = ev {
        info!(
            "ui_trainer: {} (code {error})",
            super::train_error_log_line(error)
        );
    }
}

/// An open trainer window ends with the session, its records with it; the getters' item callbacks
/// end with the item cache's ([`crate::items::Items::clear_session`]).
fn on_session_end(In(_): In<SessionEvent>, mut open: ResMut<TrainerOpen>) {
    open.clear_session();
}
