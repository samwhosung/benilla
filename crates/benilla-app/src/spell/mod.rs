//! **The spell** — `Spell_C`, the reference's cast unit: the
//! cast ladder and its commit ([`cast_send`] — `TryCast 0x6e4b60` → `SendCast 0x6e54f0`), the
//! target bind ([`cast_target`] — `ArmCast 0x6e5250` / `BindTarget 0x6e5b40`), the requirement
//! validator's rungs ([`validator`] — `0x6094f0`), the usable walk ([`usable`] —
//! `IsSpellUsableNow 0x6e3d60`), the targeting cursor and its flag_word ([`targeting`] —
//! `0xcecac0`), the in-flight slot ([`inflight`]), the cooldown store ([`cooldowns`]), the
//! talent modifier tables ([`mods`]) and the packet handlers that fold into them ([`net`]).
//!
//! What the reference delegates stays out, with its owner: the spell table
//! (`crate::ui_action::Spells`, the DBC's), the action store and the error text
//! (`crate::ui_action` — the `ui` boundary), the aura durations (`crate::ui_aura`), the pet bar
//! (`crate::ui_pet`), the cast bar's feed (`crate::ui_cast`). Decisions 2328 (the state) and 2330
//! (the ladder).

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

// The one cast path, and the only way in: every caster surface takes [`CastLadder`] as a single
// SystemParam and commits through [`CastCommit`] — `send_spell_cast` itself is private to
// `cast_send`, so a second send path cannot be written by accident.
pub(crate) use cast_send::{CastCommit, CastLadder, TargetedBind};
pub(crate) use cast_target::AutoSelfCast;
pub(crate) use cooldowns::Cooldowns;
pub(crate) use inflight::{
    inflight, ActiveChannel, AutoRepeatActive, LocalMoveStart, PendingCast, QueuedMeleeSpell,
    SPELL_INTERRUPT_MOVEMENT,
};
pub(crate) use mods::{SpellModifiers, OP_COST};
// The target chain registers the cursor pre-empt + the click commits, and the spellbook/stance/
// craft drains thread the mode through the one cast-send path. `TargetingWants`
// travels with it because the chain also holds a *seam-specific* consumer — the ground reticle,
// which draws for the location word alone.
pub(crate) use targeting::{ground_cast_radius, SpellTargeting, TargetingWants};

/// The local self-cancel's slot in the frame: anything that reads the in-flight state after a
/// move/jump/Esc may have ended the cast this tick orders `.after(LocalCancel)` — the cast-bar
/// feed does, so the bar drops the same frame the cast does (2328).
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
                    // The targeting mode's ESC-chain halves: the state push
                    // rides the feeds (before the input pass runs `ToggleGameMenu`), the
                    // trigger drain follows it — same frame, so an ESC's cancel lands before
                    // the next frame's cursor drive reads the mode. The cursor pre-empt, the
                    // right-press cancel, and the click commits register in the TARGET chain
                    // (ordering against the classifier and the select click is theirs to own).
                    targeting::feed_targeting_to_vm.in_set(UnitFeed),
                    targeting::drain_stop_targeting.after(UiInput),
                    // The item half's commit — the bag / paper-doll click seam's
                    // `0x495d60`. A UI drain like the others: after the input pass, so a click
                    // this frame binds this frame. It is deliberately NOT in the target chain —
                    // the clicks it consumes never reach the world.
                    targeting::commit_item_cast_on_pick.after(UiInput),
                ),
            )
            .add_systems(OnEnter(ClientState::InWorld), mods::clear_on_world_enter);
        net::register(app);
    }
}
