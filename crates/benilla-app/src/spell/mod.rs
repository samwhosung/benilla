//! The spell, the reference's `Spell_C`: the cast ladder and commit ([`cast_send`], `TryCast
//! 0x6e4b60` and `SendCast 0x6e54f0`), the target bind ([`cast_target`], `ArmCast 0x6e5250` and
//! `BindTarget 0x6e5b40`), the requirement validator ([`validator`], `0x6094f0`), the usable walk
//! ([`usable`], `IsSpellUsableNow 0x6e3d60`), the targeting cursor ([`targeting`], `0xcecac0`),
//! the in-flight slot, the cooldowns, the talent modifiers and their packet handlers ([`net`]).

use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

mod cast_send;
pub(crate) mod cast_target;
pub(crate) mod cooldowns;
mod inflight;
mod mods;
pub(crate) mod net;
pub(crate) mod targeting;
pub(crate) mod usable;
pub(crate) mod validator;

// The one cast path: every caster takes [`CastLadder`] and commits through [`CastCommit`];
// `send_spell_cast` is private to `cast_send`, so no second send path can exist.
pub(crate) use cast_send::{CastCommit, CastLadder, TargetedBind};
pub(crate) use cast_target::AutoSelfCast;
pub(crate) use cooldowns::Cooldowns;
pub(crate) use inflight::{
    inflight, ActiveChannel, AutoRepeatActive, LocalMoveStart, PendingCast, QueuedMeleeSpell,
    SPELL_INTERRUPT_MOVEMENT,
};
pub(crate) use mods::{SpellModifiers, OP_COST};
// `TargetingWants` is exported for the ground reticle, which draws for the location word alone.
pub(crate) use targeting::{ground_cast_radius, SpellTargeting, TargetingWants};

/// The local self-cancel's set: a reader of the in-flight state orders `.after(LocalCancel)` so a
/// cast ended by a move, jump or Esc drops its cast bar the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LocalCancel;

pub(crate) struct SpellPlugin;

impl Plugin for SpellPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<ActiveChannel>()
            .init_resource::<LocalMoveStart>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<Cooldowns>()
            .init_resource::<SpellModifiers>()
            .init_resource::<AutoSelfCast>()
            .init_resource::<SpellTargeting>()
            .init_resource::<targeting::EnchantConfirmItem>()
            .add_observer(cast_target::on_cvar)
            .add_systems(
                Update,
                (
                    inflight::local_self_cancel
                        .in_set(UnitFeed)
                        .in_set(LocalCancel),
                    mods::track_class_family
                        .after(WorldStage::Net)
                        .before(UnitFeed),
                    // The state push runs before the input pass's `ToggleGameMenu` and the drain
                    // after it, so an Esc cancel lands before next frame's cursor reads the mode.
                    targeting::feed_targeting_to_vm.in_set(UnitFeed),
                    targeting::drain_stop_targeting.after(UiInput),
                    targeting::drain_spell_target_unit.after(UiInput),
                    // The item-target commit (`0x495d60`): after the input pass so a bag click
                    // binds the same frame; outside the target chain, as its clicks never reach
                    // the world.
                    targeting::commit_item_cast_on_pick.after(UiInput),
                ),
            )
            .add_systems(OnEnter(ClientState::InWorld), mods::clear_on_world_enter);
        net::register(app);
    }
}
