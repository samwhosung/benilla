//! The spell, the reference's `Spell_C`: the cast ladder and commit ([`cast_send`], `TryCast
//! 0x6e4b60` and `SendCast 0x6e54f0`), the target bind ([`cast_target`], `ArmCast 0x6e5250` and
//! `BindTarget 0x6e5b40`), the party and raid relations its arms ask ([`group_relation`],
//! `0x606c20`, `0x606d20`), the requirement validator ([`validator`], `0x6094f0`), the usable walk
//! ([`usable`], `IsSpellUsableNow 0x6e3d60`), the targeting cursor ([`targeting`], `0xcecac0`),
//! the in-flight slot, the cooldowns, the talent modifiers and their packet handlers ([`net`]).

use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

mod bind_gates;
mod cast_send;
pub(crate) mod cast_target;
pub(crate) mod cooldowns;
mod group_relation;
mod inflight;
mod mods;
pub(crate) mod net;
mod range_units;
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
pub(crate) use mods::{ModsDiff, SpellModifiers, OP_CAST_TIME, OP_COST, OP_GCD, OP_RADIUS};
#[cfg(test)]
pub(crate) use mods::{OP_COOLDOWN, OP_RANGE};
pub(crate) use range_units::{RangeUnits, DEFAULT_REACH};
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
            .init_resource::<targeting::UnitPick>()
            .init_resource::<group_relation::GroupRoster>()
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
                    // the bar. After the aura feed, whose resolver inputs list the units the
                    // hover verdict is asked for.
                    targeting::feed_targeting_to_vm
                        .in_set(UnitFeed)
                        .after(crate::ui_pet::pet_stop_on_old_target_clear)
                        .after(crate::ui_aura::AuraEvents),
                    (
                        targeting::publish_picks_self,
                        targeting::publish_corpse_pick,
                        targeting::publish_unit_pick,
                    )
                        .in_set(UiFeed),
                    // The party slots and raid roster the party and raid words read, before
                    // the feed's targeting push asks them.
                    group_relation::publish_group_roster
                        .in_set(UiFeed)
                        .before(UnitFeed),
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
    skill_line_value(me, line)
}

/// The player's skill in `line`, [`spell_skill_value`]'s second half: 0 without a player or a
/// slot for it.
pub(crate) fn skill_line_value(me: Option<&crate::net::ObjectStore>, line: u32) -> u32 {
    let Some(store) = me else { return 0 };
    line_skill_value(
        (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
            .filter_map(|slot| store.0.player_skill(slot)),
        line,
    )
}

/// `0x5ea520`'s sum on the line's first slot (`0x5ea3f0`): the value, plus the bonus word's high
/// half read unsigned when the value is positive (`0x5ea571`-`0x5ea580`), plus its low half read
/// signed (`0x5ea5ac`-`0x5ea5ba`), floored at 0 (`0x5ea5bc`).
fn line_skill_value(
    slots: impl Iterator<Item = benilla_protocol::messages::PlayerSkillSlot>,
    line: u32,
) -> u32 {
    for s in slots {
        if u32::from(s.skill_id) == line {
            let mut v = i32::from(s.value);
            if v > 0 {
                v += i32::from(s.perm_bonus as u16);
            }
            return (v + i32::from(s.temp_bonus)).max(0) as u32;
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

/// The skill lines whose value differs between two snapshots: a line with no slot reads 0
/// ([`skill_line_value`]), and the first slot of a line is the one read.
pub(crate) fn changed_skill_lines(prev: &SkillSnapshot, now: &SkillSnapshot) -> Vec<u32> {
    let by_line = |snapshot: &SkillSnapshot| {
        let mut lines = std::collections::HashMap::new();
        for &(line, value) in snapshot.iter().filter(|(line, _)| *line != 0) {
            lines.entry(u32::from(line)).or_insert(value);
        }
        lines
    };
    let (prev, now) = (by_line(prev), by_line(now));
    let value =
        |lines: &std::collections::HashMap<u32, u32>, line| lines.get(&line).copied().unwrap_or(0);
    prev.keys()
        .chain(now.keys())
        .copied()
        .filter(|&line| value(&prev, line) != value(&now, line))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::PlayerSkillSlot;

    fn slot(value: u16, temp_bonus: i16, perm_bonus: i16) -> PlayerSkillSlot {
        PlayerSkillSlot {
            skill_id: 43,
            step: 0,
            value,
            max: 300,
            temp_bonus,
            perm_bonus,
        }
    }

    /// A line's change is its value's, wherever its slot sits: a moved slot is no change, a line
    /// gained or lost is one, and a slot at 0 is a line at 0.
    #[test]
    fn the_changed_lines_are_those_whose_value_moved() {
        let snapshot = |slots: &[(u16, u32)]| -> SkillSnapshot {
            let mut s = [(0, 0); benilla_protocol::messages::PLAYER_SKILL_SLOTS as usize];
            s[..slots.len()].copy_from_slice(slots);
            s
        };
        let base = snapshot(&[(43, 100), (44, 50), (54, 0)]);
        assert!(changed_skill_lines(&base, &base).is_empty());
        assert!(
            changed_skill_lines(&base, &snapshot(&[(54, 0), (44, 50), (43, 100)])).is_empty(),
            "a reordered slot table changes no line"
        );
        assert_eq!(
            changed_skill_lines(&base, &snapshot(&[(43, 101), (44, 50)])),
            [43]
        );
        assert_eq!(
            changed_skill_lines(&base, &snapshot(&[(43, 100), (44, 50), (95, 1)])),
            [95],
            "a gained line, and the slot at 0 that went away is no change"
        );
        assert_eq!(
            changed_skill_lines(&base, &snapshot(&[(43, 100)])),
            [44],
            "a lost line reads 0"
        );
    }

    /// The high bonus half counts only over a positive value, and unsigned; the low half is
    /// signed and always counts; the sum floors at 0.
    #[test]
    fn the_skill_value_reads_its_bonus_halves_as_the_reference_does() {
        let value = |s: PlayerSkillSlot| super::line_skill_value(std::iter::once(s), 43);
        assert_eq!(value(slot(100, 0, 5)), 105);
        assert_eq!(value(slot(0, 0, 5)), 0);
        assert_eq!(value(slot(0, 7, 5)), 7);
        assert_eq!(value(slot(100, -20, 0)), 80);
        assert_eq!(value(slot(10, -20, 0)), 0);
        assert_eq!(value(slot(1, 0, -1)), 65536);
        assert_eq!(
            super::line_skill_value(std::iter::once(slot(100, 0, 0)), 44),
            0
        );
    }
}
