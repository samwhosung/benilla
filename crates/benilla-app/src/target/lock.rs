//! The GameObject lock chain: the reference's resolver `0x5f83d0`, its per-slot Action gate
//! (`0x5f81d0`) and the refusal-toast routing (`0x5f3427`). The reference asks the one resolver
//! from `usable 0x5f3130` (the cursor's grey twin, and whether a right-click sends) and from the
//! USE sender `0x5f33e0` (`CMSG_GAMEOBJ_USE`, an `OPEN_LOCK` cast or a toast), so the icon and the
//! click agree by construction.
//!
//! The Action gate judges each `Lock.dbc` slot on the object's state and its `GO_FLAG_LOCKED` bit
//! ([`benilla_formats::LockSlot::available`]). It alone keeps a keyed door such as Scholomance's
//! shut: its spare Quick Open slot (Action 0), which 6247 "Opening" satisfies for every character,
//! applies only while the door is not flagged locked.
//!
//! A slot is satisfied when the matched spell's `OPEN_LOCK` value
//! ([`benilla_formats::SpellDisplay::open_lock_skill`]) reaches the requirement (`0x5f850f`),
//! `Skill[i]` or `GAMEOBJECT_LEVEL × 5` when that is zero (`0x5f84be`). The value's level term is
//! the player's skill in the spell's own line (`0x6e384d → 0x6e3130 → [vtbl+0xa8] = 0x5ea690`,
//! [`spell_skill_value`]), not the character level.

use std::collections::BTreeSet;

use benilla_formats::{LockSlot, LOCK_KEY_ITEM, LOCK_KEY_SKILL};
use bevy::prelude::*;

use crate::net::ObjectStore;

/// The lock chain's data as one [`SystemParam`]: the ask-once GameObject and item template caches
/// (the item's for a key's name and ON_USE spell), `Lock.dbc`, `LockType.dbc` and the spell
/// catalog. The `Option` members are absent without client data.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GoLockInputs<'w, 's> {
    // `Res`: a miss's ask-once template request marks itself through `&self`.
    pub(crate) templates: Res<'w, crate::go_templates::GameObjectTemplates>,
    pub(crate) locks: Option<Res<'w, crate::go_templates::Locks>>,
    pub(crate) lock_types: Option<Res<'w, crate::go_templates::LockTypes>>,
    pub(crate) spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// The spell to skill-line hop of [`spell_skill_value`]; absent, the skill reads 0.
    pub(crate) skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    pub(crate) items: Res<'w, crate::items::Items>,
    /// The object index the key-item scan walks the bags through.
    pub(crate) objects: crate::net::Objects<'w, 's>,
}

/// The wire facts the Action gate and the requirement fallback read, gathered by the caller.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GoFacts {
    /// The stored `GAMEOBJECT_STATE` (`go+0x27c`), from [`crate::go_anim::go_state`].
    pub(crate) state: u32,
    /// `GAMEOBJECT_FLAGS & GO_FLAG_LOCKED (0x2)`.
    pub(crate) flag_locked: bool,
    /// `GAMEOBJECT_LEVEL`, the zero-`Skill` fallback's base. vmangos sets it only on transports
    /// (`GameObject.cpp:247`), so there the fallback asks nothing and the server gates.
    pub(crate) level: u32,
}

/// The resolver's verdict: the reference's return and its spell-id out-param in one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LockOutcome {
    /// No `Lock.dbc` row (`0x5f8180` null) or every slot empty: opens by `CMSG_GAMEOBJ_USE`.
    Unlocked,
    /// A skill slot satisfied by this known `OPEN_LOCK` spell, cast at the object.
    OpenBySpell(u32),
    /// A key slot whose item we carry, by the key's entry; the reference casts the item's ON_USE
    /// spell (`0x5d8c80`).
    OpenByKey(u32),
    /// A lock no slot satisfies: the client-local refusal, no packet.
    Unmet,
}

impl LockOutcome {
    /// `usable 0x5f3130`'s lock arm: not usable only for [`Self::Unmet`] with `GO_FLAG_LOCKED` set
    /// (`0x5f32a6`). With the flag clear the arm is skipped, so a herb node you cannot gather keeps
    /// the lit `GatherHerbs` cursor and toasts on the click.
    pub(super) fn blocks_usable(self, flag_locked: bool) -> bool {
        flag_locked && self == LockOutcome::Unmet
    }
}

/// The Action gate's inputs off a hovered GameObject's store and its resolved state. `None` gives
/// the wire defaults, under which every gated slot is inapplicable rather than free.
pub(crate) fn go_facts(go: Option<(&ObjectStore, u32)>) -> GoFacts {
    match go {
        Some((store, state)) => GoFacts {
            state,
            flag_locked: store.0.gameobject_flags() & GO_FLAG_LOCKED != 0,
            level: store.0.gameobject_level(),
        },
        None => GoFacts {
            state: benilla_formats::GO_STATE_ACTIVE,
            flag_locked: false,
            level: 0,
        },
    }
}

/// `GO_FLAG_LOCKED` (vmangos `GameObjectDefines.h:75`): selects the Action 1 slots and arms
/// `usable`'s lock check (`0x5f32a6`).
pub(crate) const GO_FLAG_LOCKED: u32 = 0x2;

/// The reference's lock resolver `0x5f83d0`. Walks the eight `Lock.dbc` slots in order:
/// - SKILL (2): past the Action gate, scan the known spells for an `OPEN_LOCK` effect whose
///   `EffectMiscValue` is the slot's `Index`; a match sets `matched_spell` before the value test
///   (`0x5f84f8`), then its effect value is compared to the requirement (`0x5f850f`).
/// - KEY (1): past the Action gate, look for the key item in our bags and keyring.
/// - NONE (0): skipped.
///
/// A SKILL or KEY slot makes the lock real even when the Action gate rejects it (`[ebp-1] = 1`
/// before `0x5f81d0`), so a door whose only opener is gated out refuses instead of falling through
/// to `CMSG_GAMEOBJ_USE`. Whether `matched_spell` is set routes the toast (`0xdf` or `0xe0`).
///
/// The first sufficient match is cast, so the scan order picks between two sufficient openers,
/// such as 6478 "Opening" and 22810 "Opening - No Text" on LockType 13, which every character
/// knows. `known` is a [`BTreeSet`], so ours scans in ascending spell id; the reference scans its
/// opener list (`0xb700b0`) in the order `SMSG_INITIAL_SPELLS` listed them, later learns at the
/// tail, and vmangos sends that packet in `std::unordered_map` order (`Player.cpp:3458`,
/// `Player.h:113`).
pub(crate) fn resolve_lock(
    slots: &[LockSlot],
    known: &BTreeSet<u32>,
    spells: Option<&crate::ui_action::Spells>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    me: Option<&ObjectStore>,
    objects: &crate::net::Objects,
    go: GoFacts,
    matched_spell: &mut Option<u32>,
) -> LockOutcome {
    let mut real = false;
    for slot in slots {
        match slot.key_type {
            LOCK_KEY_SKILL => {
                real = true;
                if !slot.available(go.state, go.flag_locked) {
                    continue;
                }
                let Some(spells) = spells else { continue };
                for &id in known {
                    let Some(spell) = spells.catalog.get(id) else {
                        continue;
                    };
                    if spell.open_lock_type() != Some(slot.index) {
                        continue;
                    }
                    // `0x5f84f8`: set on the LockType match, before the value test.
                    matched_spell.get_or_insert(id);
                    let skill = spell_skill_value(me, skill_lines, id);
                    let provides = spell.open_lock_skill(skill).unwrap_or(0);
                    if provides >= required_skill(slot, go.level) {
                        return LockOutcome::OpenBySpell(id);
                    }
                }
            }
            LOCK_KEY_ITEM => {
                real = true;
                if !slot.available(go.state, go.flag_locked) {
                    continue;
                }
                if me.is_some_and(|s| holds_item(&s.0, objects, slot.index)) {
                    return LockOutcome::OpenByKey(slot.index);
                }
            }
            _ => {}
        }
    }
    if real {
        LockOutcome::Unmet
    } else {
        LockOutcome::Unlocked
    }
}

/// The player's skill in `spell_id`'s own line, the opener value's level term: `0x5ea690` hops
/// spell to SkillLineAbility line (`0x6de040`), then `0x5ea520` reads that line's
/// `PLAYER_SKILL_INFO` slot. Every missing input reads 0, like the reference's null paths.
fn spell_skill_value(
    me: Option<&ObjectStore>,
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

/// `0x5f8260`, the targeting cursor's question for the pending spell (`[0xceac58]`) over a
/// GameObject, from the object dispatcher `0x4828d0` through `0x6e6460`. Unlike [`resolve_lock`]
/// it matches the spell's own `OPEN_LOCK` effects against each slot's `Index` and compares no skill
/// values, so the cursor lights on a chest the skill cannot open and the server refuses the click;
/// no `Lock.dbc` row (`0x5f8180` null) is false, so the cursor greys over a mailbox.
///
/// Not built: the `SPELL_EFFECT_OPEN_LOCK_ITEM` (59) arm, which matches the cast item's entry
/// against a KEY slot; the catalog lacks its per-effect id.
pub(crate) fn spell_opens_lock(
    slots: &[LockSlot],
    spell: &benilla_formats::SpellDisplay,
    go: GoFacts,
) -> bool {
    let Some(lock_type) = spell.open_lock_type() else {
        return false;
    };
    slots.iter().any(|slot| {
        slot.key_type == LOCK_KEY_SKILL
            && slot.index == lock_type
            && slot.available(go.state, go.flag_locked)
    })
}

/// The slot's requirement: `Skill[i]`, or `GAMEOBJECT_LEVEL × 5` when that is zero
/// (`0x5f84be`..`0x5f84ca`; `0x5f3490`..`0x5f349f` recomputes it for the `0xe0` toast's `%d`).
pub(crate) fn required_skill(slot: &LockSlot, go_level: u32) -> i32 {
    if slot.skill != 0 {
        slot.skill as i32
    } else {
        (go_level * 5) as i32
    }
}

/// Whether we carry item `entry`, by the reference's own walker, [`crate::ui_items::find_item`]
/// (`0x622270 → 0x622420`, mode 0: equipment, bags, backpack and keyring).
fn holds_item(
    store: &benilla_protocol::messages::ObjectFields,
    objects: &crate::net::Objects,
    entry: u32,
) -> bool {
    crate::ui_items::find_item(
        store,
        objects,
        entry,
        crate::ui_items::ItemSearch::default(),
    )
    .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{GO_STATE_ACTIVE, GO_STATE_READY, LOCK_KEY_NONE};

    fn skill_slot(index: u32, skill: u32, action: u32) -> LockSlot {
        LockSlot {
            key_type: LOCK_KEY_SKILL,
            index,
            skill,
            action,
        }
    }

    /// `0x5f8260` against `0x5f83d0`: the cursor's question differs from the resolver's.
    #[test]
    fn the_cursor_lock_predicate_ignores_skill_and_refuses_the_lockless() {
        use benilla_formats::SpellDisplay;
        // Pick Lock: LockType 1, Lockpicking.
        let pick_lock = SpellDisplay {
            open_lock: Some(benilla_formats::OpenLock {
                effect: 0,
                lock_type: 1,
            }),
            ..SpellDisplay::default()
        };
        let unlocked = GoFacts {
            state: GO_STATE_READY,
            flag_locked: true,
            level: 60,
        };

        // A Lockpicking slot demanding 300 skill, Action 1 (applies while flagged locked).
        let matching = [skill_slot(1, 300, 1), LockSlot::default()];
        assert!(
            spell_opens_lock(&matching, &pick_lock, unlocked),
            "the right LockType through an applicable slot lights the cursor"
        );

        // No skill compare: a slot far beyond Pick Lock still lights, and the server refuses.
        let brutal = [skill_slot(1, 9999, 1), LockSlot::default()];
        assert!(
            spell_opens_lock(&brutal, &pick_lock, unlocked),
            "0x5f8260 compares no skill values — an out-of-reach lock still lights"
        );

        // Wrong LockType (3, Mining): a lockpick greys over an ore vein.
        let mining = [skill_slot(3, 0, 1), LockSlot::default()];
        assert!(!spell_opens_lock(&mining, &pick_lock, unlocked));

        // The shared Action gate: Action 0 applies only while not flagged locked.
        assert!(!spell_opens_lock(
            &[skill_slot(1, 0, 0)],
            &pick_lock,
            unlocked
        ));
        assert!(spell_opens_lock(
            &[skill_slot(1, 0, 0)],
            &pick_lock,
            GoFacts {
                flag_locked: false,
                ..unlocked
            }
        ));

        // No lock row is false, unlike `resolve_lock`: no Pick Lock on a mailbox.
        assert!(!spell_opens_lock(&[], &pick_lock, unlocked));
        assert!(!spell_opens_lock(
            &[LockSlot::default()],
            &pick_lock,
            unlocked
        ));

        // A spell with no OPEN_LOCK effect opens nothing, whatever the lock says.
        assert!(!spell_opens_lock(
            &matching,
            &SpellDisplay::default(),
            unlocked
        ));

        // A KEY slot belongs to the unbuilt effect-59 arm.
        let key = [LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 1,
            skill: 0,
            action: 1,
        }];
        assert!(!spell_opens_lock(&key, &pick_lock, unlocked));
    }

    #[test]
    fn zero_skill_falls_back_to_go_level_times_five() {
        assert_eq!(required_skill(&skill_slot(3, 0, 0), 0), 0);
        assert_eq!(required_skill(&skill_slot(3, 0, 0), 20), 100);
        // A nonzero Skill wins; the level is not read.
        assert_eq!(required_skill(&skill_slot(1, 280, 1), 60), 280);
    }

    /// A level-60 with 1 Mining is refused a 250-skill vein; missing data reads skill 0, and the
    /// `0x5ea520` sum counts both bonus halves.
    #[test]
    fn the_lock_value_tracks_the_players_skill_not_their_level() {
        use benilla_formats::{OpenLock, SkillLineCatalog, SpellCatalog, SpellDisplay};
        use benilla_protocol::messages::{ObjectFields, FIELD_PLAYER_SKILL_INFO_1_1};

        // Mining 2575, real shape: `−1 + 1 + 5.0·Δ`, baseLevel 0, LockType 3.
        let mining = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 3,
                effect: 0,
            }),
            effect_base_points: [-1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays([(2575, mining)].into_iter().collect()),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // Spell → line: Mining is SkillLine 186.
        let lines = SkillLineCatalog::from_spell_lines([(2575, 186)]);
        let known = BTreeSet::from([2575]);
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        // A 250-skill vein, available (Action 0, READY, not flagged).
        let mut vein = [LockSlot::default(); 8];
        vein[0] = skill_slot(3, 250, 0);
        let facts = GoFacts {
            state: GO_STATE_READY,
            flag_locked: false,
            level: 0,
        };
        // A level-60 store (`UNIT_FIELD_LEVEL` 34) with Mining at `value` and the bonuses in the
        // third word, as `PlayerSkillSlot` packs them.
        let store_with_mining = |value: u32, bonus_word: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (34, 60),
                (FIELD_PLAYER_SKILL_INFO_1_1, 186),
                (FIELD_PLAYER_SKILL_INFO_1_1 + 1, value | (300 << 16)),
                (FIELD_PLAYER_SKILL_INFO_1_1 + 2, bonus_word),
            ]))
        };
        let resolve = |store: Option<&ObjectStore>, lines: Option<&SkillLineCatalog>| {
            resolve_lock(
                &vein,
                &known,
                Some(&spells),
                lines,
                store,
                &objects,
                facts,
                &mut None,
            )
        };

        // 300 Mining opens; 1 Mining is refused.
        let skilled = store_with_mining(300, 0);
        assert_eq!(
            resolve(Some(&skilled), Some(&lines)),
            LockOutcome::OpenBySpell(2575)
        );
        let unskilled = store_with_mining(1, 0);
        assert_eq!(resolve(Some(&unskilled), Some(&lines)), LockOutcome::Unmet);

        // The `0x5ea520` sum counts both bonus halves: 235 + 10 temp + 5 perm = 250, exactly
        // enough.
        let buffed = store_with_mining(235, 10 | (5 << 16));
        assert_eq!(
            resolve(Some(&buffed), Some(&lines)),
            LockOutcome::OpenBySpell(2575)
        );

        // No skill-line catalog, or no store, reads skill 0: refused.
        assert_eq!(resolve(Some(&skilled), None), LockOutcome::Unmet);
        assert_eq!(resolve(None, Some(&lines)), LockOutcome::Unmet);
    }

    /// The lock is marked real (`[ebp-1] = 1`) before `0x5f81d0` is asked.
    #[test]
    fn a_gated_out_slot_still_makes_the_lock_real() {
        let slots = [
            skill_slot(10, 0, 0), // Quick Open: Action 0, gated out on a flagged-locked door
            LockSlot::default(),
        ];
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut matched = None;
        let out = resolve_lock(
            &slots,
            &BTreeSet::new(),
            None,
            None,
            None,
            &objects,
            GoFacts {
                state: GO_STATE_READY,
                flag_locked: true,
                level: 0,
            },
            &mut matched,
        );
        assert_eq!(out, LockOutcome::Unmet);
        assert!(
            out.blocks_usable(true),
            "a flagged-locked unmet lock grays the cursor"
        );
        // With the flag clear it blocks nothing: lit cursor, toast on click.
        assert!(!out.blocks_usable(false));
    }

    /// Shipped `Lock.dbc` and `Spell.dbc` values, which `benilla-formats`'
    /// `real_lock_catalog_reads_the_action_column` and
    /// `real_spell_catalog_computes_the_lock_skill_an_opener_provides` pin against the files.
    #[test]
    fn a_keyed_door_refuses_the_universally_known_opening_spell() {
        use benilla_formats::{OpenLock, SpellCatalog, SpellDisplay};

        // Scholomance Door, lock 1159: key 13704, Pick Lock 280, Quick Open, Quick Close, Blasting
        // 300; the template ships `GO_FLAG_LOCKED`.
        let mut scholomance = [LockSlot::default(); 8];
        scholomance[0] = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 13704,
            skill: 0,
            action: 1,
        };
        scholomance[1] = skill_slot(1, 280, 1);
        scholomance[2] = skill_slot(10, 0, 0);
        scholomance[3] = skill_slot(11, 0, 2);
        scholomance[4] = skill_slot(16, 300, 1);

        // Two openers with their real value inputs: 6247 "Opening", which every character is
        // created with, and Pick Lock, whose value tracks the Lockpicking skill.
        let opening = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 10,
                effect: 0,
            }),
            effect_base_points: [99, 0, 0],
            effect_base_dice: [1, 0, 0],
            ..Default::default()
        };
        let pick_lock = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 1,
                effect: 0,
            }),
            effect_base_points: [4, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            base_level: 1,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(6247, opening), (1804, pick_lock)].into_iter().collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let locked_shut = GoFacts {
            state: GO_STATE_READY,
            flag_locked: true,
            level: 0,
        };

        // A character who knows only "Opening" is refused.
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut matched,
            ),
            LockOutcome::Unmet,
        );
        assert_eq!(
            matched, None,
            "a gated-out slot never even reaches the known-spell scan"
        );

        // With the flag clear the same door opens to the same spell: the gate refuses, not the
        // value test.
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                GoFacts {
                    flag_locked: false,
                    ..locked_shut
                },
                &mut None,
            ),
            LockOutcome::OpenBySpell(6247),
        );

        // The flag selects Pick Lock's Action 1 slot, but with no store the skill reads 0, its
        // flat 5 < 280 refuses, and the out-param is still written (`0x5f84f8`).
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &scholomance,
                &BTreeSet::from([1804]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut matched,
            ),
            LockOutcome::Unmet,
        );
        assert_eq!(
            matched,
            Some(1804),
            "a LockType match writes the out-param before the value test"
        );

        // The Searing Gorge gate, lock 84, has no Action 0 slot.
        let mut searing_gorge = [LockSlot::default(); 8];
        searing_gorge[0] = LockSlot {
            key_type: LOCK_KEY_ITEM,
            index: 5396,
            skill: 0,
            action: 1,
        };
        searing_gorge[1] = skill_slot(1, 225, 1);
        assert_eq!(
            resolve_lock(
                &searing_gorge,
                &BTreeSet::from([6247]),
                Some(&spells),
                None,
                None,
                &objects,
                locked_shut,
                &mut None,
            ),
            LockOutcome::Unmet,
        );

        // A Copper Vein, lock 38 (one Mining slot, Skill 0, Action 0), unflagged and READY, opens
        // for a miner.
        let mining = SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 3,
                effect: 0,
            }),
            effect_base_points: [-1, 0, 0],
            effect_base_dice: [1, 0, 0],
            effect_real_points_per_level: [5.0, 0.0, 0.0],
            ..Default::default()
        };
        let with_mining = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays([(2575, mining)].into_iter().collect()),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        let mut vein = [LockSlot::default(); 8];
        vein[0] = skill_slot(3, 0, 0);
        assert_eq!(
            resolve_lock(
                &vein,
                &BTreeSet::from([2575]),
                Some(&with_mining),
                None,
                None,
                &objects,
                GoFacts {
                    state: GO_STATE_READY,
                    flag_locked: false,
                    level: 0
                },
                &mut None,
            ),
            LockOutcome::OpenBySpell(2575),
        );
    }

    /// Two sufficient openers on one LockType: the lower spell id wins, `known` being ordered.
    /// Lock 43 (a Hyacinth Mushroom's) has one SKILL slot, LockType 13 "Open Kneeling", Skill 0,
    /// Action 0, and `playercreateinfo_spell` grants both 6478 "Opening" and 22810 "Opening - No
    /// Text" to every race and class.
    #[test]
    fn two_sufficient_openers_pick_the_lower_spell_id() {
        use benilla_formats::{OpenLock, SpellCatalog, SpellDisplay};

        let mut mushroom = [LockSlot::default(); 8];
        mushroom[0] = skill_slot(13, 0, 0);

        // Both "Opening" spells: OPEN_LOCK on type 13, flat value 100 (base 99 + dice 1), identical
        // in every input the resolver reads, so only the visit order decides.
        let kneeling_opener = || SpellDisplay {
            open_lock: Some(OpenLock {
                lock_type: 13,
                effect: 0,
            }),
            effect_base_points: [99, 0, 0],
            effect_base_dice: [1, 0, 0],
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(6478, kneeling_opener()), (22810, kneeling_opener())]
                    .into_iter()
                    .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let unlocked = GoFacts {
            state: GO_STATE_READY,
            flag_locked: false,
            level: 0,
        };

        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &mushroom,
                &BTreeSet::from([6478, 22810]),
                Some(&spells),
                None,
                None,
                &objects,
                unlocked,
                &mut matched,
            ),
            LockOutcome::OpenBySpell(6478),
            "6478 \"Opening\", never 22810 \"Opening - No Text\""
        );
        assert_eq!(matched, Some(6478), "the toast's out-param agrees");

        // Insertion order cannot change it: the set is ordered.
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &mushroom,
                &BTreeSet::from([22810, 6478]),
                Some(&spells),
                None,
                None,
                &objects,
                unlocked,
                &mut matched,
            ),
            LockOutcome::OpenBySpell(6478),
        );
    }

    #[test]
    fn an_empty_row_is_unlocked() {
        let slots = [LockSlot::default(); 8];
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let mut matched = None;
        assert_eq!(
            resolve_lock(
                &slots,
                &BTreeSet::new(),
                None,
                None,
                None,
                &objects,
                GoFacts {
                    state: GO_STATE_ACTIVE,
                    flag_locked: false,
                    level: 0
                },
                &mut matched,
            ),
            LockOutcome::Unlocked
        );
        assert_eq!(slots[0].key_type, LOCK_KEY_NONE);
    }
}
