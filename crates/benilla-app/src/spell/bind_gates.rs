//! `BindTarget 0x6e5b40`'s gates: the five silent exits a unit meets before any relation arm
//! (`6e5bd8`-`6e5ca2`). A refused unit binds nothing, so `ArmCast` falls to its next candidate,
//! the player behind `autoSelfCast`, and then to the enemy-word refusal or the targeting cursor.
//! The read-only mirror `SpellCanTargetUnit 0x6e6460` runs the same five (`6e6507`-`6e65ab`), so
//! the press, the cursor's click, `SpellTargetUnit` and the hover verdict share this one gate
//! through [`super::cast_target::unit_binds`].

use benilla_formats::SpellDisplay;
use benilla_protocol::ObjectFields;

use super::cast_target::{TargetRelations, CORPSE_WORD_BITS, TF_EXPLICIT_GATE};

/// `UNIT_FIELD_FLAGS & 0x10000` (vmangos `UNIT_FLAG_NON_ATTACKABLE_2`, `UnitDefines.h:561`), tested
/// at `6e5c16`-`6e5c1d`.
const UNIT_FLAG_NON_ATTACKABLE_2: u32 = 0x0001_0000;

/// The bind's alive test (`6e5c5f`, `6e5e55`) and the world pick's (`4806fd`, `480713 setg`,
/// [`super::targeting::PickFlags::admits`]): signed `UNIT_FIELD_HEALTH` above 0. An absent field
/// reads 0, as the reference's descriptor does, so a dead unit whose create sent no health is dead.
pub(super) fn unit_alive(fields: &ObjectFields) -> bool {
    fields.unit_health().unwrap_or(0) as i32 > 0
}

/// Whether `unit` passes the gates for `word`. `is_caster` is the unit being the cast's caster
/// (`6e5bd8`-`6e5bee`); the unit's own fields come from `rel.target_store`, and a candidate with
/// no streamed store has none to read, so the relation arms alone decide it. Refused when:
///
/// - the unit is the caster and the spell has `AttributesEx & 0x80000` (`6e5bf7`);
/// - the unit has `UNIT_FIELD_FLAGS & 0x10000` (`6e5c1d`);
/// - the spell's `TargetCreatureType` mask excludes the unit's creature type (`6e5c36`, `6e5c53`);
/// - the unit is dead and neither `UNIT_DYNAMIC_FLAGS & 0x20`, a word in `0x8600` nor
///   `AttributesEx2 & 1` admits it (`6e5c8b`);
/// - the unit is alive and the word has `0x400` (`6e5ca2`).
pub(super) fn bind_gates(
    def: &SpellDisplay,
    word: u16,
    is_caster: bool,
    rel: &TargetRelations,
) -> bool {
    if is_caster && def.excludes_caster() {
        return false;
    }
    let Some(store) = rel.target_store else {
        return true;
    };
    let fields = &store.0;
    if fields.unit_flags() & UNIT_FLAG_NON_ATTACKABLE_2 != 0 {
        return false;
    }
    if !def.admits_creature_type(rel.types.of(store)) {
        return false;
    }
    if unit_alive(fields) {
        word & TF_EXPLICIT_GATE == 0
    } else {
        fields.unit_dynflag_dead() || word & CORPSE_WORD_BITS != 0 || def.allows_dead_target()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_type::CreatureTypeSources;
    use crate::names::{CreatureRecord, NameCache};
    use crate::net::{ObjectStore, Reputations};
    use benilla_protocol::ObjectFields;

    /// Absolute descriptor indices: `OBJECT_FIELD_ENTRY`, `UNIT_FIELD_BYTES_0` (the race byte),
    /// `UNIT_FIELD_HEALTH`, `UNIT_FIELD_FLAGS` and `UNIT_DYNAMIC_FLAGS`.
    const ENTRY: u16 = 3;
    const BYTES_0: u16 = 36;
    const HEALTH: u16 = 22;
    const FLAGS: u16 = 46;
    const DYNAMIC_FLAGS: u16 = 143;

    const WORD_UNIT: u16 = 0x0002;
    const WORD_ASSIST: u16 = 0x0100;
    const WORD_DEAD_ONLY: u16 = 0x0402;
    const WORD_CORPSE_ALLY: u16 = 0x8000;

    /// A store with the fields a case sets, health 100 unless it says otherwise.
    fn unit(extra: &[(u16, u32)]) -> ObjectStore {
        let mut pairs = extra.to_vec();
        if !pairs.iter().any(|&(index, _)| index == HEALTH) {
            pairs.push((HEALTH, 100));
        }
        ObjectStore(ObjectFields::from_pairs(&pairs))
    }

    fn passes(def: &SpellDisplay, word: u16, is_caster: bool, store: &ObjectStore) -> bool {
        passes_with(def, word, is_caster, store, CreatureTypeSources::default())
    }

    fn passes_with(
        def: &SpellDisplay,
        word: u16,
        is_caster: bool,
        store: &ObjectStore,
        types: CreatureTypeSources,
    ) -> bool {
        static EMPTY: Reputations = Reputations(Vec::new(), Vec::new());
        let rel = TargetRelations {
            target_store: Some(store),
            target_owner_store: None,
            self_store: None,
            factions: None,
            reputations: &EMPTY,
            types,
            group: Default::default(),
        };
        bind_gates(def, word, is_caster, &rel)
    }

    fn spell() -> SpellDisplay {
        SpellDisplay::default()
    }

    #[test]
    fn an_ordinary_live_unit_passes() {
        assert!(passes(&spell(), WORD_UNIT, false, &unit(&[])));
        assert!(passes(&spell(), WORD_ASSIST, true, &unit(&[])));
    }

    /// `6e5bf7`: the caster, and only the caster, under `AttributesEx & 0x80000`.
    #[test]
    fn the_caster_is_refused_under_a_self_excluding_spell() {
        let excluding = SpellDisplay {
            attributes_ex: 0x0008_0000,
            ..spell()
        };
        assert!(!passes(&excluding, WORD_ASSIST, true, &unit(&[])));
        assert!(passes(&excluding, WORD_ASSIST, false, &unit(&[])));
        assert!(passes(&spell(), WORD_ASSIST, true, &unit(&[])));
        // Before any store is read: an unstreamed caster is still the caster.
        let rel = TargetRelations {
            target_store: None,
            target_owner_store: None,
            self_store: None,
            factions: None,
            reputations: &Reputations(Vec::new(), Vec::new()),
            types: CreatureTypeSources::default(),
            group: Default::default(),
        };
        assert!(!bind_gates(&excluding, WORD_ASSIST, true, &rel));
        assert!(bind_gates(&excluding, WORD_ASSIST, false, &rel));
    }

    /// `6e5c1d`: bit 16 of `UNIT_FIELD_FLAGS`, and only that bit.
    #[test]
    fn a_unit_flagged_0x10000_is_refused() {
        assert!(!passes(
            &spell(),
            WORD_ASSIST,
            false,
            &unit(&[(FLAGS, 0x0001_0000)])
        ));
        assert!(!passes(
            &spell(),
            WORD_ASSIST,
            true,
            &unit(&[(FLAGS, 0x0001_1008)])
        ));
        for other in [0x0000_ffff, 0x0002_0000, 0xfffe_ffff] {
            assert!(
                passes(&spell(), WORD_ASSIST, false, &unit(&[(FLAGS, other)])),
                "flags {other:#x}"
            );
        }
    }

    /// `6e5c26`-`6e5c53`: a mask excludes a unit of another type, a unit of no type, and lets
    /// the right type through, the type read the reference's way.
    #[test]
    fn a_creature_type_mask_refuses_the_wrong_type() {
        let hibernate = SpellDisplay {
            target_creature_type: 0x3,
            ..spell()
        };
        let mut names = NameCache::default();
        for (entry, creature_type) in [(69u32, 1u32), (70, 7)] {
            names.insert_creature(
                entry,
                Some(CreatureRecord {
                    name: String::new(),
                    subname: None,
                    creature_type,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        }
        let types = CreatureTypeSources {
            names: Some(&names),
            forms: None,
        };
        let check = |def: &SpellDisplay, store: &ObjectStore| {
            passes_with(def, WORD_UNIT, false, store, types)
        };
        assert!(check(&hibernate, &unit(&[(ENTRY, 69)])), "a Beast");
        assert!(!check(&hibernate, &unit(&[(ENTRY, 70)])), "a Humanoid");
        assert!(
            !check(&hibernate, &unit(&[(ENTRY, 71)])),
            "a creature no template has answered for is type 0"
        );
        assert!(check(&spell(), &unit(&[(ENTRY, 70)])), "no mask, any type");
        assert!(check(&spell(), &unit(&[(ENTRY, 71)])), "no mask, no type");
        // A player is Humanoid by race, so Hibernate refuses it and Mind Control (Humanoid) takes it.
        let player = unit(&[(BYTES_0, 1)]);
        let mind_control = SpellDisplay {
            target_creature_type: 0x40,
            ..spell()
        };
        assert!(!check(&hibernate, &player));
        assert!(check(&mind_control, &player));
    }

    /// `6e5c6a`-`6e5c8b`: a dead unit is refused unless the dynamic flag, a corpse word or
    /// `AttributesEx2 & 1` admits it; health is the signed field, and absent reads 0.
    #[test]
    fn a_dead_unit_is_refused_unless_something_admits_it() {
        let dead = unit(&[(HEALTH, 0)]);
        assert!(!passes(&spell(), WORD_ASSIST, false, &dead));
        assert!(!passes(&spell(), WORD_ASSIST, true, &dead));
        assert!(!passes(&spell(), WORD_UNIT, false, &dead));
        let absent = ObjectStore(ObjectFields::default());
        assert!(
            !passes(&spell(), WORD_ASSIST, false, &absent),
            "a dead unit's create sends no health"
        );
        let negative = unit(&[(HEALTH, 0x8000_0000)]);
        assert!(!passes(&spell(), WORD_ASSIST, false, &negative), "signed");

        assert!(
            passes(
                &spell(),
                WORD_ASSIST,
                false,
                &unit(&[(HEALTH, 0), (DYNAMIC_FLAGS, 0x20)])
            ),
            "UNIT_DYNAMIC_FLAGS bit 5"
        );
        for word in [WORD_CORPSE_ALLY, 0x0200, WORD_DEAD_ONLY] {
            assert!(passes(&spell(), word, false, &dead), "word {word:#06x}");
        }
        let raise_dead = SpellDisplay {
            attributes_ex2: 1,
            ..spell()
        };
        assert!(passes(&raise_dead, WORD_ASSIST, false, &dead));
        let other_ex2 = SpellDisplay {
            attributes_ex2: 0xffff_fffe,
            ..spell()
        };
        assert!(!passes(&other_ex2, WORD_ASSIST, false, &dead));
        assert!(!passes(
            &spell(),
            WORD_ASSIST,
            false,
            &unit(&[(HEALTH, 0), (DYNAMIC_FLAGS, 0xffff_ffdf)])
        ));
    }

    /// `6e5c9a`-`6e5ca2`: a live unit is refused when the word has `0x400`, whatever else it has.
    #[test]
    fn a_living_unit_is_refused_under_the_0x400_word() {
        let alive = unit(&[]);
        assert!(!passes(&spell(), WORD_DEAD_ONLY, false, &alive));
        assert!(!passes(&spell(), 0x0400, false, &alive));
        assert!(!passes(&spell(), WORD_DEAD_ONLY, true, &alive));
        assert!(passes(&spell(), WORD_UNIT, false, &alive));
        // AttributesEx2 & 1 and a corpse bit other than 0x400 exempt only the dead.
        let raise_dead = SpellDisplay {
            attributes_ex2: 1,
            ..spell()
        };
        assert!(!passes(&raise_dead, WORD_DEAD_ONLY, false, &alive));
        assert!(passes(&spell(), WORD_CORPSE_ALLY, false, &alive));
        assert!(
            passes(&spell(), WORD_DEAD_ONLY, false, &unit(&[(HEALTH, 0)])),
            "the same word at a corpse"
        );
    }

    /// The gates at the press, through `resolve_cast_target`: a unit they refuse binds nothing,
    /// so the selection falls to the player behind `autoSelfCast`, and with nothing bound an
    /// enemy word refuses "Invalid target" and any other word raises the cursor. Each case's
    /// control is the same unit passing the gate, which binds.
    mod ladder {
        use super::*;
        use crate::spell::cast_target::{
            cast_target_mask, resolve_cast_target, CastCandidates, CastWireTarget,
            ERR_INVALID_TARGET,
        };
        use crate::spell::targeting::corpse_fixture as fx;

        const ME: u64 = 1;
        const THEM: u64 = 42;
        const TEMPLATE: u16 = 35;
        /// A creature entry whose cached template is a Humanoid (type 7) and one that is a
        /// Beast (type 1).
        const HUMANOID_ENTRY: u32 = 69;
        const BEAST_ENTRY: u32 = 70;

        fn spell_at(targets: u32, implicit: u32) -> SpellDisplay {
            SpellDisplay {
                targets,
                implicit_target_a1: implicit,
                ..Default::default()
            }
        }

        /// A heal: the assist word, `Targets 0` with implicit target 21.
        fn heal() -> SpellDisplay {
            spell_at(0, 21)
        }

        /// A player-controlled unit of faction template 1 (`0x8`), so [`fx::factions`]' Human pair
        /// is friendly to itself and `CanAssist` passes on its player-controlled arm. `extra`
        /// overrides a default; health is 100 unless it says otherwise.
        fn player(extra: &[(u16, u32)]) -> ObjectStore {
            let mut fields =
                std::collections::BTreeMap::from([(TEMPLATE, 1), (FLAGS, 0x8), (HEALTH, 100)]);
            fields.extend(extra.iter().copied());
            let pairs: Vec<_> = fields.into_iter().collect();
            ObjectStore(ObjectFields::from_pairs(&pairs))
        }

        fn cached(creature_type: u32) -> CreatureRecord {
            CreatureRecord {
                name: String::new(),
                subname: None,
                creature_type,
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
            }
        }

        fn names() -> NameCache {
            let mut names = NameCache::default();
            names.insert_creature(HUMANOID_ENTRY, Some(cached(7)));
            names.insert_creature(BEAST_ENTRY, Some(cached(1)));
            names
        }

        /// The wire result for `def` with `target` selected (`selection_is_self`: the player is
        /// the selection), the player being `me`.
        fn press_as(
            me: &ObjectStore,
            def: &SpellDisplay,
            target: &ObjectStore,
            selection_is_self: bool,
            auto_self_cast: bool,
        ) -> CastWireTarget {
            let factions = fx::factions();
            let names = names();
            let rel = TargetRelations {
                target_store: Some(target),
                target_owner_store: None,
                self_store: Some(me),
                factions: Some(&factions),
                reputations: &Reputations(Vec::new(), Vec::new()),
                types: CreatureTypeSources {
                    names: Some(&names),
                    forms: None,
                },
                group: Default::default(),
            };
            let candidates = CastCandidates {
                selection: Some(if selection_is_self { ME } else { THEM }),
                caster: Some(ME),
                main_hand_item: None,
            };
            resolve_cast_target(Some(def), &candidates, auto_self_cast, &rel)
        }

        /// [`press_as`] with a living, unflagged player.
        fn press(
            def: &SpellDisplay,
            target: &ObjectStore,
            selection_is_self: bool,
            auto_self_cast: bool,
        ) -> CastWireTarget {
            press_as(&player(&[]), def, target, selection_is_self, auto_self_cast)
        }

        /// A dead friendly player selected with a heal: the selection binds nothing. With
        /// `autoSelfCast` on the player is bound instead; with it off the assist word is left for
        /// the cursor, as for any relation failure.
        #[test]
        fn a_dead_friendly_selection_falls_to_you_or_the_cursor() {
            let alive = player(&[]);
            let dead = player(&[(HEALTH, 0)]);
            for auto in [false, true] {
                assert_eq!(
                    press(&heal(), &alive, false, auto),
                    CastWireTarget::Unit(THEM),
                    "the living friendly binds (autoSelfCast {auto})"
                );
            }
            assert_eq!(
                press(&heal(), &dead, false, true),
                CastWireTarget::Unit(ME),
                "the corpse is skipped and the player takes the heal"
            );
            assert_eq!(
                press(&heal(), &dead, false, false),
                CastWireTarget::Targeting(WORD_ASSIST),
                "with autoSelfCast off the cursor comes up, as for a relation failure"
            );
            // A store that never streamed a health field is dead in the reference's descriptor.
            let unstreamed = ObjectStore(ObjectFields::from_pairs(&[(TEMPLATE, 1), (FLAGS, 0x8)]));
            assert_eq!(
                press(&heal(), &unstreamed, false, false),
                CastWireTarget::Targeting(WORD_ASSIST)
            );
            // `AttributesEx2 & 1` and `UNIT_DYNAMIC_FLAGS & 0x20` keep a corpse a candidate.
            let raise = SpellDisplay {
                attributes_ex2: 1,
                ..heal()
            };
            assert_eq!(
                press(&raise, &dead, false, false),
                CastWireTarget::Unit(THEM)
            );
            let feigning = player(&[(HEALTH, 0), (DYNAMIC_FLAGS, 0x20)]);
            assert_eq!(
                press(&heal(), &feigning, false, false),
                CastWireTarget::Unit(THEM)
            );
        }

        /// Hibernate at a unit of another type binds nothing, so the enemy word refuses locally
        /// with "Invalid target"; a Beast binds.
        #[test]
        fn a_creature_type_mask_refuses_the_wrong_type_locally() {
            let hibernate = SpellDisplay {
                target_creature_type: 0x3,
                ..spell_at(0, 6)
            };
            let creature = |entry| {
                ObjectStore(ObjectFields::from_pairs(&[
                    (ENTRY, entry),
                    (HEALTH, 100),
                    (TEMPLATE, 0),
                ]))
            };
            assert_eq!(
                press(&hibernate, &creature(BEAST_ENTRY), false, true),
                CastWireTarget::Unit(THEM)
            );
            for entry in [HUMANOID_ENTRY, 999] {
                assert_eq!(
                    press(&hibernate, &creature(entry), false, true),
                    CastWireTarget::Refused(ERR_INVALID_TARGET),
                    "entry {entry}: a Humanoid, and a creature with no cached template"
                );
            }
            assert_eq!(
                press(&spell_at(0, 6), &creature(HUMANOID_ENTRY), false, true),
                CastWireTarget::Unit(THEM),
                "the same Humanoid without a mask"
            );
        }

        /// A self-excluding spell with yourself selected: you are not bound, not even by
        /// `autoSelfCast`, so the assist word raises the cursor.
        #[test]
        fn a_self_excluding_spell_never_binds_the_caster() {
            let excluding = SpellDisplay {
                attributes_ex: 0x0008_0000,
                ..heal()
            };
            let me = player(&[]);
            for auto in [false, true] {
                assert_eq!(
                    press(&excluding, &me, true, auto),
                    CastWireTarget::Targeting(WORD_ASSIST),
                    "autoSelfCast {auto}"
                );
            }
            assert_eq!(
                press(&excluding, &player(&[]), false, false),
                CastWireTarget::Unit(THEM),
                "another friendly still binds"
            );
            assert_eq!(
                press(&heal(), &me, true, false),
                CastWireTarget::Unit(ME),
                "without the attribute the caster is the selection"
            );
        }

        /// `UNIT_FIELD_FLAGS & 0x10000` binds nothing: a flagged selection falls to the player
        /// behind `autoSelfCast`, and a flagged player has no fallback left.
        #[test]
        fn a_unit_flagged_0x10000_binds_nothing() {
            let flagged = || player(&[(FLAGS, 0x0001_0008)]);
            assert_eq!(
                press(&heal(), &flagged(), false, false),
                CastWireTarget::Targeting(WORD_ASSIST)
            );
            assert_eq!(
                press(&heal(), &flagged(), false, true),
                CastWireTarget::Unit(ME),
                "the selection is skipped, the player binds"
            );
            assert_eq!(
                press_as(&flagged(), &heal(), &flagged(), true, true),
                CastWireTarget::Targeting(WORD_ASSIST),
                "neither the selection nor the fallback"
            );
        }

        /// The `0x400` word (Skinning, `Targets 0x402`): a living unit is refused, a dead one
        /// binds and loses the bit.
        #[test]
        fn the_0x400_word_binds_only_the_dead() {
            let skinning = spell_at(0x402, 0);
            assert_eq!(cast_target_mask(&skinning), WORD_DEAD_ONLY);
            let creature =
                |health| ObjectStore(ObjectFields::from_pairs(&[(HEALTH, health), (TEMPLATE, 0)]));
            assert_eq!(
                press(&skinning, &creature(100), false, false),
                CastWireTarget::Targeting(WORD_DEAD_ONLY),
                "a living unit binds nothing; the word is left for the cursor"
            );
            assert_eq!(
                press(&skinning, &creature(0), false, false),
                CastWireTarget::Unit(THEM)
            );
        }
    }
}
