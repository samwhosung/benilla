//! The spell, the reference's `Spell_C`: the cast ladder and commit ([`cast_send`], `TryCast
//! 0x6e4b60` and `SendCast 0x6e54f0`), the target bind ([`cast_target`], `ArmCast 0x6e5250` and
//! `BindTarget 0x6e5b40`), the party and raid relations its arms ask ([`group_relation`],
//! `0x606c20`, `0x606d20`), the requirement validator ([`validator`], `0x6094f0`), the usable walk
//! ([`usable`], `IsSpellUsableNow 0x6e3d60`), the targeting cursor ([`targeting`], `0xcecac0`),
//! the in-flight slot, the cooldowns, the talent modifiers and their packet handlers ([`net`]).

use bevy::prelude::*;

use crate::ui_script::UiFeed;
use crate::ui_unit::UnitFeed;

mod bind_gates;
mod cast_send;
pub(crate) mod cast_target;
pub(crate) mod cooldowns;
pub(crate) mod group_relation;
mod inflight;
pub(crate) mod known;
mod mods;
pub(crate) mod net;
pub(crate) mod range_check;
mod range_units;
pub(crate) mod reads;
mod report;
pub(crate) mod targeting;
pub(crate) mod usable;
pub(crate) mod validator;

// The one cast path: every caster takes [`CastLadder`] and commits through [`CastCommit`];
// `send_spell_cast` is private to `cast_send`, so no second send path can exist.
pub(crate) use cast_send::{
    CastCommit, CastLadder, HeldCast, HeldForPick, PetGcdsOwed, ScriptCastPermit, TargetedBind,
    UnitAttackOwed, UnitPress,
};
pub(crate) use cast_target::AutoSelfCast;
pub(crate) use cooldowns::Cooldowns;
pub(crate) use inflight::{
    ActiveChannel, AutoRepeatActive, CastState, LocalMoveStart, PendingCast, QueuedMeleeSpell,
    SelfCancel, SPELL_INTERRUPT_MOVEMENT,
};
pub(crate) use mods::{
    CategoryPad, ModsDiff, SpellModifiers, OP_CAST_TIME, OP_COST, OP_GCD, OP_RADIUS,
};
#[cfg(test)]
pub(crate) use mods::{OP_COOLDOWN, OP_RANGE};
pub(crate) use range_units::{with_ranged_slot, RangeUnits, DEFAULT_REACH};
pub(crate) use reads::{SpellLevel, SpellReads, UnitField, Unrecorded};
pub(crate) use report::{
    CastCore, CastSlots, CursorCancel, Held, REASON_ERROR, REASON_TARGETS_DEAD,
};
// `TargetingWants` is exported for the ground reticle, which draws for the location word alone.
pub(crate) use targeting::{
    ground_cast_radius, CorpsePick, CursorPicks, PicksSelf, ScriptCursor, SpellTargeting,
    TargetingWants,
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
            .init_resource::<PetGcdsOwed>()
            .init_resource::<UnitAttackOwed>()
            .init_resource::<ScriptCastPermit>()
            .init_resource::<CursorPicks>()
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
                        .in_set(crate::ui_script::UiFeed)
                        .before(UnitFeed),
                    // The state push runs before the input pass's `ToggleGameMenu`, whose
                    // `SpellStopTargeting` applies inside its call. After the aura feed, whose
                    // resolver inputs list the units the hover verdict is asked for.
                    targeting::feed_targeting_to_vm
                        .in_set(UnitFeed)
                        .after(LocalCancel)
                        .after(crate::ui_aura::AuraEvents),
                    // Readers of the word, which the local cancel can clear.
                    (
                        targeting::publish_picks_self,
                        targeting::publish_corpse_pick,
                        targeting::publish_unit_pick,
                    )
                        .in_set(UiFeed)
                        .after(LocalCancel),
                    // The party slots and raid roster the party and raid words read, before
                    // the feed's targeting push asks them.
                    group_relation::publish_group_roster
                        .in_set(UiFeed)
                        .before(UnitFeed),
                ),
            );
        net::register(app);
    }
}

/// What the active player's level for a spell reads beside its skill slots: the skill line a spell
/// belongs to (`0x6de040`), and for the weapon-skill substitution ([`skill_spell`]) the spell
/// records and the item cache that names each hand's weapon. A missing one reads as the
/// reference's null paths: no line is level 0, no records or cache leave the spell as it is.
#[derive(Clone, Copy, Default)]
pub(crate) struct PlayerSkills<'a> {
    pub(crate) lines: Option<&'a benilla_formats::SkillLineCatalog>,
    pub(crate) spells: Option<&'a benilla_formats::SpellCatalog>,
    pub(crate) items: Option<&'a crate::items::Items>,
}

/// The equipment slots of the three hands `GetVirtualItem 0x5ec240` reads: main hand, off hand,
/// ranged (`0x5ec2db`, `0x5ec2aa`, `0x5ec25e`).
const HAND_SLOTS: [u8; 3] = [15, 16, 17];

/// The three hands as a [`SpellReads::item_search`] slot mask.
const HAND_SLOT_MASK: u32 = 1 << 15 | 1 << 16 | 1 << 17;

/// `Targets & 0x10` (`[rec+0x34]`), an item-targeted spell, which keeps its own skill
/// (`0x5ea600`).
const TARGETS_ITEM: u32 = 0x10;
/// `AttributesEx2 & 0x8000000` (`[rec+0x20]`), vmangos `SPELL_ATTR_EX2_IGNORE_WEAPONSKILL`
/// (`SpellDefines.h:895`), which keeps its own skill (`0x5ea60a`).
const ATTR_EX2_IGNORE_WEAPON_SKILL: u32 = 0x0800_0000;
/// `SPELL_EFFECT_WEAPON` 25, the weapon-skill spells' own effect, which keeps its own skill
/// (`0x5ea613`).
const SPELL_EFFECT_WEAPON: u32 = 0x19;

/// The class and subclass of the weapon in a hand of the active player, `GetVirtualItem
/// 0x5ec240(hand, raw)`: the visible entry of the hand's slot (`0x5f0d60`), none for an empty one
/// (`0x5ec300`), its class and subclass from the item cache, 0 and 0 on a miss (`0x62ea20`,
/// `0x62ea80`). Without `raw`, a visible flag with bit 0 is no weapon (`0x5ec2ee`), and a disarmed
/// player (`UNIT_FIELD_FLAGS & 0x200000`) has no main-hand weapon while it holds one
/// (`0x5ec2b8`-`0x5ec2d9`), and no off-hand weapon while only that hand holds one
/// (`0x5ec26c`-`0x5ec2a4`). The ranged hand has no disarm gate.
fn virtual_weapon(
    me: &benilla_protocol::ObjectFields,
    items: Option<&crate::items::Items>,
    hand: usize,
    raw: bool,
) -> Option<(u32, u32)> {
    if !raw && me.unit_flags() & crate::creature_anim::UNIT_FLAG_DISARMED != 0 {
        let weapon = |h| virtual_weapon(me, items, h, true).is_some_and(|(class, _)| class == 2);
        let hidden = match hand {
            0 => weapon(0),
            1 => !weapon(0) && weapon(1),
            _ => false,
        };
        if hidden {
            return None;
        }
    }
    let slot = HAND_SLOTS[hand];
    let entry = me.player_visible_item_entry(slot)?;
    if !raw && me.player_visible_item_flags(slot) & 0x1 != 0 {
        return None;
    }
    let record = items.and_then(|i| i.template(entry, 0, crate::query_cache::Ask::Read));
    Some(record.map_or((0, 0), |t| (t.class, t.subclass)))
}

/// `CGPlayer_C::GetSkillSpell 0x5ea5d0`: the spell whose skill line a spell's level reads. A spell
/// that needs a weapon (`EquippedItemClass` 2 with a subclass mask) takes the weapon-skill spell
/// of the first hand, main, off then ranged, whose weapon is class 2 of a subclass the mask holds
/// and the weapon-skill table names (`0x5ea62f`-`0x5ea680`); every other spell, and one no hand
/// fits, is itself. An item-targeted spell (`Targets & 0x10`), one that ignores weapon skill and a
/// weapon-skill spell keep their own. `reads` hears the hands when the hands are read.
pub(crate) fn skill_spell(
    me: &benilla_protocol::ObjectFields,
    skills: PlayerSkills,
    d: &benilla_formats::SpellDisplay,
    reads: &mut dyn SpellReads,
) -> u32 {
    if d.targets & TARGETS_ITEM != 0
        || d.attributes_ex2 & ATTR_EX2_IGNORE_WEAPON_SKILL != 0
        || d.effects[0] == SPELL_EFFECT_WEAPON
        || d.equipped_item_class != 2
        || d.equipped_item_subclass_mask == 0
    {
        return d.id;
    }
    reads.item_search(HAND_SLOT_MASK);
    (0..HAND_SLOTS.len())
        .find_map(|hand| {
            let (class, subclass) = virtual_weapon(me, skills.items, hand, false)?;
            // `shl edx,cl` takes the subclass mod 32 (`0x5ea656`).
            let fits =
                class == 2 && d.equipped_item_subclass_mask & 1u32.wrapping_shl(subclass) != 0;
            fits.then(|| skills.spells?.weapon_skill_spell(subclass))
                .flatten()
        })
        .unwrap_or(d.id)
}

/// `GetSpellLevel 0x6e3130` for the active player: `CGPlayer_C::GetSpellSkillLevel 0x5ea690` over
/// 5 (`0x6e3195`). The skill read is the total (`0x5ea520`) of the `SkillLineAbility` line of the
/// spell [`skill_spell`] names (`0x5ea69b`-`0x5ea6d7`), capped at five times that spell's
/// `maxLevel` when it has one (`0x5ea6dc`-`0x5ea6ea`); no player, no record or no line is 0
/// (`0x5ea6f0`). `reads` hears the line and the hands.
pub(crate) fn player_spell_level(
    me: Option<&crate::net::ObjectStore>,
    skills: PlayerSkills,
    d: &benilla_formats::SpellDisplay,
    reads: &mut dyn SpellReads,
) -> u32 {
    let Some(store) = me else { return 0 };
    let id = skill_spell(&store.0, skills, d, reads);
    let record = if id == d.id {
        Some(d)
    } else {
        skills.spells.and_then(|c| c.get(id))
    };
    let Some(record) = record else { return 0 };
    let (race, class) = store.race_class();
    let Some(line) = skills.lines.and_then(|c| c.spell_to_line(id, race, class)) else {
        return 0;
    };
    reads.skill_line(line);
    record.skill_level(skill_line_value(me, line))
}

/// The player's skill in `spell_id`'s own line: `0x5ea690` hops spell to SkillLineAbility line
/// (`0x6de040`), then `0x5ea520` reads that line's `PLAYER_SKILL_INFO` slot. Every missing input
/// reads 0, like the reference's null paths. It is [`player_spell_level`]'s skill for a spell that
/// needs no weapon, which the lock openers, skinning and the heals' wording read.
pub(crate) fn spell_skill_value(
    me: Option<&crate::net::ObjectStore>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    spell_id: u32,
) -> u32 {
    let (race, class) = me.map_or((0, 0), |s| s.race_class());
    let Some(line) = skill_lines.and_then(|c| c.spell_to_line(spell_id, race, class)) else {
        return 0;
    };
    skill_line_value(me, line)
}

/// A pet's `[vtbl+0xa8]`, `CGUnit`'s `0x60cd80`: `UNIT_FIELD_LEVEL × 5`, which
/// [`benilla_formats::SpellDisplay::skill_level`] caps at `maxLevel × 5` and divides by 5 as
/// `0x60cdb2`-`0x60cdb9` and `0x6e3195` do. No unit reads 0 (`0x6e31a4`).
pub(crate) fn pet_skill_value(pet: Option<&benilla_protocol::ObjectFields>) -> u32 {
    pet.and_then(|p| p.unit_level())
        .unwrap_or(0)
        .saturating_mul(5)
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

    /// `GetSpellSkillLevel 0x5ea690` reads the line of the spell `GetSkillSpell 0x5ea5d0` names: a
    /// spell needing a sword or a mace scales with the wielded weapon's skill (Swords 300, Maces
    /// 100), not its own line's, capped by the weapon-skill spell's `maxLevel` (0, none), not its
    /// own (10). An off hand fits when the main hand does not, a disarmed main hand is skipped,
    /// and an item-targeted spell, a hand of the wrong class and an uncached record keep the
    /// spell's own line.
    #[test]
    fn a_spells_level_reads_the_line_its_casters_race_indexes() {
        use benilla_formats::{SkillLineCatalog, SpellDisplay};
        use benilla_protocol::messages::FIELD_PLAYER_SKILL_INFO_1_1 as SKILLS;
        // `UNIT_FIELD_BYTES_0`: race, then class.
        const FIELD_UNIT_BYTES_0: u16 = 36;
        const HUMAN_LINE: u32 = 26;
        const ORC_LINE: u32 = 256;
        let d = SpellDisplay {
            id: 900_200,
            max_level: 60,
            ..Default::default()
        };
        // `0x5ea6c8` hands `0x6de040` the player's race and class: a human's row and an orc's.
        let lines = SkillLineCatalog::from_spell_lines([])
            .with_race_class_flags([(HUMAN_LINE, 0), (ORC_LINE, 0)])
            .with_race_ability_rows([(900_200, HUMAN_LINE, 0x1, 0), (900_200, ORC_LINE, 0x2, 0)]);
        let level = |race: u32| {
            let store = crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_UNIT_BYTES_0, race | 1 << 8),
                (SKILLS, HUMAN_LINE),
                (SKILLS + 1, 50 | 300 << 16),
                (SKILLS + 3, ORC_LINE),
                (SKILLS + 4, 100 | 300 << 16),
            ]));
            let skills = PlayerSkills {
                lines: Some(&lines),
                ..Default::default()
            };
            player_spell_level(Some(&store), skills, &d, &mut Unrecorded)
        };
        assert_eq!(level(1), 10, "a human reads its own row's line, 50");
        assert_eq!(level(2), 20, "an orc reads its own, 100");
        assert_eq!(level(3), 0, "a dwarf has no row");
    }

    #[test]
    fn a_weapon_spell_scales_with_the_wielded_weapons_skill() {
        use benilla_formats::{SkillLineCatalog, SpellCatalog, SpellDisplay};
        use benilla_protocol::messages::FIELD_PLAYER_SKILL_INFO_1_1 as SKILLS;
        const SWORDS: u32 = 43;
        const MACES: u32 = 54;
        const OWN: u32 = 26;
        // The weapon-skill spells: `Attributes & 0x40`, class 2, one subclass bit each.
        let weapon_skill = |id, subclass: u32| SpellDisplay {
            id,
            attributes: 0x40,
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << subclass,
            effects: [0x19, 0, 0],
            ..Default::default()
        };
        let strike = || SpellDisplay {
            id: 900_100,
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 7 | 1 << 4,
            max_level: 10,
            ..Default::default()
        };
        let catalog = SpellCatalog::from_displays(
            [
                (201, weapon_skill(201, 7)),
                (198, weapon_skill(198, 4)),
                (900_100, strike()),
            ]
            .into(),
        );
        let lines =
            SkillLineCatalog::from_spell_lines([(201, SWORDS), (198, MACES), (900_100, OWN)]);
        let mut items = crate::items::Items::default();
        for (entry, class, subclass) in [(1001, 2, 7), (1002, 2, 4), (1003, 4, 6)] {
            items.insert_template(
                entry,
                Some(benilla_protocol::messages::ItemInfo {
                    class,
                    subclass,
                    ..crate::items::test_template("Weapon")
                }),
            );
        }
        // Visible entries in the main and off hand (`PLAYER_VISIBLE_ITEM_16_0`, `_17_0`).
        let level = |main: u32, off: u32, flags: u32, d: &SpellDisplay| {
            let store = crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (46, flags),
                (258 + 2 + 12 * 15, main),
                (258 + 2 + 12 * 16, off),
                (SKILLS, SWORDS),
                (SKILLS + 1, 300 | 300 << 16),
                (SKILLS + 3, MACES),
                (SKILLS + 4, 100 | 300 << 16),
                (SKILLS + 6, OWN),
                (SKILLS + 7, 40 | 300 << 16),
            ]));
            let skills = PlayerSkills {
                lines: Some(&lines),
                spells: Some(&catalog),
                items: Some(&items),
            };
            player_spell_level(Some(&store), skills, d, &mut Unrecorded)
        };
        assert_eq!(
            level(1001, 0, 0, &strike()),
            60,
            "a sword: Swords 300, uncapped"
        );
        assert_eq!(level(1002, 0, 0, &strike()), 20, "a mace: Maces 100");
        assert_eq!(level(1003, 1002, 0, &strike()), 20, "the off hand's mace");
        assert_eq!(level(0, 0, 0, &strike()), 8, "no weapon: its own line, 40");
        assert_eq!(
            level(1004, 0, 0, &strike()),
            8,
            "an uncached weapon reads class 0"
        );
        // Disarmed while the main hand holds a weapon: that hand is none.
        assert_eq!(level(1001, 1002, 0x0020_0000, &strike()), 20);
        assert_eq!(level(1001, 0, 0x0020_0000, &strike()), 8);
        let item_targeted = SpellDisplay {
            targets: 0x10,
            ..strike()
        };
        assert_eq!(
            level(1001, 0, 0, &item_targeted),
            8,
            "Targets & 0x10 keeps its own"
        );
    }
}
