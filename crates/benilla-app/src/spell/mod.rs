//! The spell, the reference's `Spell_C`: the cast ladder and commit ([`cast_send`], `TryCast
//! 0x6e4b60` and `SendCast 0x6e54f0`), the target bind ([`cast_target`], `ArmCast 0x6e5250` and
//! `BindTarget 0x6e5b40`), the requirement validator ([`validator`], `0x6094f0`), the usable walk
//! ([`usable`], `IsSpellUsableNow 0x6e3d60`), the targeting cursor ([`targeting`], `0xcecac0`),
//! the in-flight slot, the cooldowns, the talent modifiers and their packet handlers ([`net`]).

use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::ui_script::{UiFeed, UiInput};
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
pub(crate) use cast_send::{CastCommit, CastLadder, HeldCast, HeldForPick, TargetedBind};
pub(crate) use cast_target::AutoSelfCast;
pub(crate) use cooldowns::Cooldowns;
pub(crate) use inflight::{
    inflight, ActiveChannel, AutoRepeatActive, LocalMoveStart, PendingCast, QueuedMeleeSpell,
    SelfCancel, SPELL_INTERRUPT_MOVEMENT,
};
#[cfg(test)]
pub(crate) use mods::OP_COOLDOWN;
pub(crate) use mods::{SpellModifiers, OP_CAST_TIME, OP_COST, OP_GCD, OP_RADIUS};
// `TargetingWants` is exported for the ground reticle, which draws for the location word alone.
pub(crate) use targeting::{
    ground_cast_radius, CorpsePick, PicksSelf, ScriptCursor, SpellTargeting, TargetingWants,
};

/// A script cast's inputs, the cast tail the action bar uses, for the spellbook's and the stance
/// bar's calls ([`crate::script_calls`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptCast<'w, 's> {
    pub(crate) targeting: cast_target::CastTargeting<'w, 's>,
    pub(crate) ladder: CastLadder<'w, 's>,
}

/// The local self-cancel's set: a reader of the in-flight state orders `.after(LocalCancel)` so a
/// cast ended by a move or jump drops its cast bar the same frame.
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
            .init_resource::<HeldForPick>()
            .init_resource::<targeting::EnchantConfirmItem>()
            .init_resource::<targeting::PicksSelf>()
            .init_resource::<targeting::CorpsePick>()
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
                    // The state push runs before the input pass's `ToggleGameMenu`, whose
                    // `SpellStopTargeting` lands with the script calls after it. After the
                    // old-target clear, the pet bar's writer in the feed: `"pet"` resolves off
                    // the bar.
                    targeting::feed_targeting_to_vm
                        .in_set(UnitFeed)
                        .after(crate::ui_pet::pet_stop_on_old_target_clear),
                    (
                        targeting::publish_picks_self,
                        targeting::publish_corpse_pick,
                    )
                        .in_set(UiFeed),
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

/// The player's skill in `spell_id`'s own line, the input of every per-level term: `0x5ea690`
/// hops spell to SkillLineAbility line (`0x6de040`), then `0x5ea520` reads that line's
/// `PLAYER_SKILL_INFO` slot. Every missing input reads 0, like the reference's null paths.
pub(crate) fn spell_skill_value(
    me: Option<&crate::net::ObjectStore>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    spell_id: u32,
) -> u32 {
    let Some(line) = skill_lines.and_then(|c| c.spell_to_line(spell_id)) else {
        return 0;
    };
    let Some(store) = me else { return 0 };
    line_skill_value(
        (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
            .filter_map(|slot| store.0.player_skill(slot)),
        line,
    )
}

/// `0x5ea520`'s sum on the line's first slot: `value + temp_bonus + perm_bonus`
/// (`0x5ea56d`..`0x5ea580`), floored at 0 since the bonuses are signed.
fn line_skill_value(
    slots: impl Iterator<Item = benilla_protocol::messages::PlayerSkillSlot>,
    line: u32,
) -> u32 {
    for s in slots {
        if u32::from(s.skill_id) == line {
            let v = i32::from(s.value) + i32::from(s.temp_bonus) + i32::from(s.perm_bonus);
            return v.max(0) as u32;
        }
    }
    0
}

/// Each skill slot's line and effective value, all [`spell_skill_value`] reads: a text built
/// against one snapshot is rebuilt when the next differs.
pub(crate) type SkillSnapshot =
    [(u16, u32); benilla_protocol::messages::PLAYER_SKILL_SLOTS as usize];

/// The [`SkillSnapshot`] of the player; all zeros without one.
pub(crate) fn skill_snapshot(me: Option<&crate::net::ObjectStore>) -> SkillSnapshot {
    std::array::from_fn(|slot| {
        me.and_then(|s| s.0.player_skill(slot as u8))
            .map_or((0, 0), |s| {
                (
                    s.skill_id,
                    line_skill_value(std::iter::once(s), s.skill_id.into()),
                )
            })
    })
}
