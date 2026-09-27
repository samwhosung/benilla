//! The targeting cursor's corpse leg: a released player's corpse under a word carrying a corpse
//! bit, `0x8000` (Resurrection, Redemption, Rebirth) or `0x200`. A dead player who has not released
//! is a unit, and the unit leg takes him.
//!
//! - The pick ([`pick_admits`]): `0x480816` admits a corpse only with pick flag `0x40`, which while
//!   targeting only a word in `0x8600` sets (`0x6e6230`), and then only a corpse `0x6e6260` passes.
//! - The cursor ([`super::SpellTargeting::can_target_corpse`]): `0x6e6460`'s corpse leg
//!   (`6e6719`–`6e676d`), then its range tail.
//! - The click ([`bind_target_corpse`]): `BindTarget 0x6e5b40`'s corpse arm (`6e5f89`–`6e6020`),
//!   then the merge's range test, as for a unit.
//!
//! All three read two facts of the corpse, [`CorpseFacts`]: bones are never a spell target, and the
//! reaction gate `0x6067d0` splits the ally bit from the enemy bit.

use bevy::prelude::*;

use benilla_protocol::messages::CorpseTarget;

use crate::net::ObjectStore;
use crate::spell::cast_send::TargetedBind;
use crate::target::Factions;

/// `TARGET_FLAG_CORPSE_ALLY` and `TARGET_FLAG_CORPSE_ENEMY`, as the flag word carries them.
const WORD_ALLY: u16 = 0x8000;
const WORD_ENEMY: u16 = 0x0200;

/// What the corpse legs read of a corpse.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CorpseFacts {
    /// `CORPSE_FIELD_FLAGS & 1`, `CORPSE_FLAG_BONES` (`[desc+0x74]`).
    pub(crate) bones: bool,
    /// `0x6067d0(caster, corpse)`, [`crate::target::corpse_friendly`].
    pub(crate) friendly: bool,
}

impl CorpseFacts {
    pub(super) fn of(
        store: &ObjectStore,
        factions: Option<&Factions>,
        self_store: Option<&ObjectStore>,
    ) -> Self {
        Self {
            bones: store.0.corpse_is_bones(),
            friendly: crate::target::corpse_friendly(store, factions, self_store),
        }
    }
}

/// [`pick_admits`] over a corpse's store, for [`crate::target`]'s hover.
pub(crate) fn corpse_pick_admits(
    word: u16,
    store: &ObjectStore,
    factions: Option<&Factions>,
    self_store: Option<&ObjectStore>,
) -> bool {
    pick_admits(word, CorpseFacts::of(store, factions, self_store))
}

/// `0x6e6260`, the pick's corpse filter while targeting: a word with the ally bit takes a corpse
/// that is not bones and that `0x6067d0` passes (`6e6291`–`6e62a7`); only a word without it tries
/// the enemy bit, on a corpse that is not bones and that `0x6067d0` fails (`6e62ad`–`6e62c6`).
/// Both bits are inside `0x8600`, so a corpse this admits was already a candidate (`0x6e6230`).
fn pick_admits(word: u16, f: CorpseFacts) -> bool {
    if word & WORD_ALLY != 0 {
        !f.bones && f.friendly
    } else {
        word & WORD_ENEMY != 0 && !f.bones && !f.friendly
    }
}

/// `BindTarget 0x6e5b40`'s corpse arm: the ally arm (`6e5f97`–`6e5fda`) binds a corpse that is not
/// bones and that `0x6067d0` passes, then the enemy arm (`6e5fdc`–`6e6020`) one it fails. Each
/// writes its bit to the wire mask (`6e5fc7`, `6e600c`), parks the corpse's guid and clears its bit
/// from the word. `None` binds nothing, silently, and the cursor stays (`6e6026`). The read-only
/// mirror `0x6e6460` (`6e6719`–`6e676d`) passes exactly the corpses this binds.
fn bind_arm(word: u16, f: CorpseFacts) -> Option<CorpseTarget> {
    if f.bones {
        None
    } else if word & WORD_ALLY != 0 && f.friendly {
        Some(CorpseTarget::Ally)
    } else if word & WORD_ENEMY != 0 && !f.friendly {
        Some(CorpseTarget::Enemy)
    } else {
        None
    }
}

/// The world pick's corpse admission as the frame began, which [`crate::target`]'s hover reads:
/// `None` outside targeting, where the pick flags carry `0x5c` and so `0x40`, and every corpse is a
/// candidate (`0x480816`); while targeting, the standing word, through [`pick_admits`].
#[derive(Resource, Default)]
pub(crate) struct CorpsePick(pub(crate) Option<u16>);

/// Publish [`CorpsePick`] before the input pass, beside [`super::PicksSelf`].
pub(crate) fn publish_corpse_pick(
    targeting: Res<super::SpellTargeting>,
    mut pick: ResMut<CorpsePick>,
) {
    let now = targeting.0.as_ref().map(|t| t.word);
    if pick.0 != now {
        pick.0 = now;
    }
}

impl super::SpellTargeting {
    /// `0x6e6460`'s corpse leg with the range flag the hover passes (`48290b`): [`bind_arm`]'s
    /// test, then min² ≤ d² ≤ max² (`6e668c`).
    pub(crate) fn can_target_corpse(&self, entity: Entity, checks: &super::BindChecks) -> bool {
        self.0.as_ref().is_some_and(|t| {
            checks
                .corpse_facts(entity)
                .is_some_and(|f| bind_arm(t.word, f).is_some())
                && checks.corpse_range_refusal(t.spell_id, entity).is_none()
        })
    }
}

/// `BindTarget 0x6e5b40`'s corpse arm for the world click (`0x493540` at `4935d5`): a corpse no
/// arm binds leaves the cursor up; otherwise the merge's range test and the commit, with the
/// corpse's guid and its bit (`0x7e4e10` writes the guid for any mask bit in `0x8a02`).
///
/// The reference sends only once the word is empty (`6e60c1`), so a word the corpse bit does not
/// empty waits for another click with the corpse parked. No player spell has one (every shipped
/// `0x8000` word with another bit is an NPC or test spell), and as the unit arm does, this binds
/// only a click that empties the word.
pub(super) fn bind_target_corpse(
    ladder: &mut crate::spell::CastLadder,
    checks: &super::BindChecks,
    entity: Entity,
    guid: u64,
) {
    let Some((spell_id, commit, word)) = ladder.ground.pending() else {
        return;
    };
    let Some(bit) = checks.corpse_facts(entity).and_then(|f| bind_arm(word, f)) else {
        return;
    };
    if word & !bit.target_flag() != 0 {
        return;
    }
    let range = checks.corpse_range_refusal(spell_id, entity);
    super::merge(
        ladder,
        spell_id,
        commit,
        range,
        TargetedBind::Corpse(guid, bit),
    );
}

/// A human caster, and a human corpse and an orc corpse toward him, on the shipped rows of
/// `FactionTemplate.dbc` 1 and 2 and `ChrRaces.dbc` races 1 and 2.
#[cfg(test)]
pub(crate) mod fixture {
    use std::collections::HashMap;

    use benilla_formats::{FactionCatalog, FactionTemplate};
    use benilla_protocol::ObjectFields;

    use crate::net::ObjectStore;
    use crate::target::Factions;

    pub(crate) const HUMAN: u8 = 1;
    pub(crate) const ORC: u8 = 2;
    /// Resurrection rank 1: `Targets 0x8000`, range row 4.
    pub(crate) const RESURRECTION: u32 = 2006;
    /// A corpse guid, `HIGHGUID_CORPSE` 0xF101 (vmangos `ObjectGuid.h:76`).
    pub(crate) const CORPSE: u64 = 0xF101_0000_0000_002A;

    /// Templates 1 (Human) and 2 (Orc): group, friend and enemy masks 3/2/12 and 5/4/10.
    pub(crate) fn factions() -> Factions {
        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        Factions::from_catalog(FactionCatalog::from_rows(
            HashMap::from([(1, tpl(1, 3, 2, 12)), (2, tpl(2, 5, 4, 10))]),
            HashMap::from([(HUMAN, 1), (ORC, 2)]),
        ))
    }

    /// A human caster, template 1, with this `UNIT_FIELD_COMBATREACH` (field 130).
    pub(crate) fn caster(reach: f32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(35, 1), (130, reach.to_bits())]))
    }

    /// A released player's corpse: the race in `CORPSE_FIELD_BYTES_1` byte 1 (field 32) and
    /// `CORPSE_FLAG_BONES` in `CORPSE_FIELD_FLAGS` (field 35).
    pub(crate) fn corpse(race: u8, bones: bool) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (32, u32::from(race) << 8),
            (35, u32::from(bones)),
        ]))
    }

    /// Resurrection on range row 4, 0 to 30 yd.
    pub(crate) fn spells() -> crate::ui_action::Spells {
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
            RESURRECTION,
            benilla_formats::SpellDisplay {
                range_index: 4,
                targets: 0x8000,
                ..Default::default()
            },
        )]));
        spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([(
            4,
            benilla_formats::SpellRange {
                min: 0.0,
                max: 30.0,
                flags: 0,
            },
        )]));
        spells
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BONES: CorpseFacts = CorpseFacts {
        bones: true,
        friendly: true,
    };
    const ALLY: CorpseFacts = CorpseFacts {
        bones: false,
        friendly: true,
    };
    const ENEMY: CorpseFacts = CorpseFacts {
        bones: false,
        friendly: false,
    };

    /// `0x6e6260`: bones never, the ally bit only a friendly corpse, the enemy bit only an
    /// unfriendly one, and only on a word without the ally bit.
    #[test]
    fn the_pick_admits_a_corpse_as_0x6e6260_does() {
        assert!(pick_admits(0x8000, ALLY), "Resurrection over a friend");
        assert!(!pick_admits(0x8000, ENEMY), "never an enemy's corpse");
        assert!(!pick_admits(0x8000, BONES), "never bones");
        assert!(pick_admits(0x0200, ENEMY));
        assert!(!pick_admits(0x0200, ALLY));
        assert!(!pick_admits(
            0x0200,
            CorpseFacts {
                bones: true,
                friendly: false
            }
        ));
        // Both bits: the ally arm decides alone (`6e6291 jns`), so no enemy corpse.
        assert!(pick_admits(0x8200, ALLY));
        assert!(!pick_admits(0x8200, ENEMY));
        // A word with no corpse bit admits none, `0x400` inside `0x8600` included.
        for word in [0x0002, 0x0100, 0x0400, 0x0040, 0x4800] {
            assert!(!pick_admits(word, ALLY), "{word:#06x}");
            assert!(!pick_admits(word, ENEMY), "{word:#06x}");
        }
    }

    /// `BindTarget`'s corpse arm, which `0x6e6460`'s corpse leg mirrors: the ally arm then the
    /// enemy arm, each on a corpse that is not bones.
    #[test]
    fn the_bind_takes_the_arm_the_reaction_picks() {
        assert_eq!(bind_arm(0x8000, ALLY), Some(CorpseTarget::Ally));
        assert_eq!(
            bind_arm(0x8000, ENEMY),
            None,
            "a hostile corpse under 0x8000"
        );
        assert_eq!(bind_arm(0x8000, BONES), None, "bones");
        assert_eq!(bind_arm(0x0200, ENEMY), Some(CorpseTarget::Enemy));
        assert_eq!(bind_arm(0x0200, ALLY), None);
        // Both bits: unlike the pick, the enemy arm still runs after the ally arm fails.
        assert_eq!(bind_arm(0x8200, ENEMY), Some(CorpseTarget::Enemy));
        assert_eq!(bind_arm(0x8200, ALLY), Some(CorpseTarget::Ally));
        for word in [0x0002, 0x0100, 0x0400, 0x0040] {
            assert_eq!(bind_arm(word, ALLY), None, "{word:#06x}");
        }
    }
}
