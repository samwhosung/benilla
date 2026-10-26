//! The item dispatcher's control gate exemption (`0x5d8ec0`-`0x5d8f6a`): without control of the
//! character, an item still goes if its on-use spell grants immunity to the aura that took the
//! control, so a trinket breaks fear and a charm. The VM judges the gate in the call, asking the
//! host for the exemption off the caster as of the call
//! ([`benilla_ui::script::ControlBypassQuery`], [`super::live`]).

#[cfg(test)]
use std::collections::HashSet;

use benilla_formats::{cc_exemption, SpellCatalog, SpellDisplay};
use benilla_protocol::ObjectFields;

/// `UNIT_FIELD_FLAGS` bits 22 and 23 (vmangos `UnitDefines.h`): confused and fleeing.
const UNIT_FLAG_CONFUSED: u32 = 0x0040_0000;
const UNIT_FLAG_FLEEING: u32 = 0x0080_0000;

/// `AttributesEx` bit 15, which the immunity matcher `0x6e9d70` requires of the cast spell
/// (`0x6e9d7e`-`0x6e9d83`): without it no spell can pass, so only these are candidates.
#[cfg(test)]
const ATTR_EX_BIT_15: u32 = 0x0000_8000;

/// Whether `cast` passes the lost-control gate for `caster`, the three arms of the item
/// dispatcher (`0x5d8ef9`-`0x5d8f66`) and the usability predicate (`0x6e3dd0`-`0x6e3e4c`) alike.
/// Each helper of the reference asks its scanner (`0x6e9ca0`) about the caster's aura slots:
/// fleeing (flag bit 23) is `0x6e9f20`, aura type 7; confused (bit 22) `0x6e9f40`, type 5; charmed
/// (`UNIT_FIELD_CHARMEDBY` nonzero, whoever holds it) `0x6e9f60`, types 6, 177 and 2. A spell
/// passes if any applicable arm exempts it, the arms ORed, and the scanner exempts when an aura
/// of the type is on the caster and the spell's immunity effects cover every such effect
/// ([`benilla_formats::grants_immunity`]).
pub(crate) fn exempts_lost_control(
    catalog: &SpellCatalog,
    cast: &SpellDisplay,
    caster: &ObjectFields,
) -> bool {
    let flags = caster.unit_flags();
    let scan =
        |ty: u32| cc_exemption(cast, caster.unit_aura_ids(), ty, |id| catalog.get(id)).exempt;
    (flags & UNIT_FLAG_FLEEING != 0 && scan(7))
        || (flags & UNIT_FLAG_CONFUSED != 0 && scan(5))
        || (caster.unit_charmed_by().is_some() && [6, 177, 2].into_iter().any(scan))
}

/// The on-use spells that pass the gate now ([`exempts_lost_control`]).
#[cfg(test)]
fn control_bypass_spells(
    catalog: &SpellCatalog,
    candidates: &[u32],
    caster: &ObjectFields,
) -> HashSet<u32> {
    candidates
        .iter()
        .filter_map(|&id| catalog.get(id).map(|cast| (id, cast)))
        .filter(|(_, cast)| exempts_lost_control(catalog, cast, caster))
        .map(|(id, _)| id)
        .collect()
}

/// The spells that can pass at all: those with `AttributesEx` bit 15.
#[cfg(test)]
fn bypass_candidates(catalog: &SpellCatalog) -> Vec<u32> {
    catalog
        .iter()
        .filter(|(_, d)| d.attributes_ex & ATTR_EX_BIT_15 != 0)
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEAR_AURA: u32 = 5782;
    const INSIGNIA: u32 = 7744;
    /// `UNIT_FIELD_FLAGS` at descriptor index 46, `UNIT_FIELD_AURA` from 47.
    const FLAGS: u16 = 46;
    const AURA_0: u16 = 47;

    /// Fear: one `APPLY_AURA` effect of aura type 7, mechanic 5.
    fn fear() -> SpellDisplay {
        let mut d = SpellDisplay {
            mechanic: 5,
            ..Default::default()
        };
        d.effects[0] = 6;
        d.effect_apply_aura[0] = 7;
        d
    }

    /// A spell that grants mechanic immunity to `mechanic`, `AttributesEx` bit 15 set.
    fn immunity_to(mechanic: i32) -> SpellDisplay {
        let mut d = SpellDisplay {
            attributes_ex: ATTR_EX_BIT_15,
            ..Default::default()
        };
        d.effects[0] = 6;
        d.effect_apply_aura[0] = 77;
        d.effect_misc_value[0] = mechanic;
        d
    }

    fn catalog() -> SpellCatalog {
        SpellCatalog::from_displays(
            [
                (FEAR_AURA, fear()),
                (INSIGNIA, immunity_to(5)),
                (9999, immunity_to(12)),
                (9998, SpellDisplay::default()),
            ]
            .into(),
        )
    }

    fn caster(flags: u32, auras: &[u32]) -> ObjectFields {
        let mut pairs = vec![(FLAGS, flags)];
        for (i, a) in auras.iter().enumerate() {
            pairs.push((AURA_0 + i as u16, *a));
        }
        ObjectFields::from_pairs(&pairs)
    }

    /// Feared by an aura of mechanic 5: the spell granting immunity to that mechanic passes, one
    /// granting another mechanic's or none does not.
    #[test]
    fn only_a_spell_with_the_matching_immunity_passes_a_fear() {
        let set = control_bypass_spells(
            &catalog(),
            &bypass_candidates(&catalog()),
            &caster(UNIT_FLAG_FLEEING, &[FEAR_AURA]),
        );
        assert_eq!(set, HashSet::from([INSIGNIA]));
    }

    /// Only a spell with `AttributesEx` bit 15 can ever pass the matcher (`0x6e9d81`), so only
    /// those are walked.
    #[test]
    fn the_candidates_are_the_bit_15_spells() {
        let mut ids = bypass_candidates(&catalog());
        ids.sort_unstable();
        assert_eq!(ids, vec![INSIGNIA, 9999]);
    }

    /// Nothing to exempt without the unit flag, even with the aura on the caster: the arms are
    /// gated on the flags (`0x5d8ef9`, `0x5d8f26`) and the charm owner (`0x5d8f4d`).
    #[test]
    fn an_arm_needs_its_unit_flag() {
        assert!(control_bypass_spells(
            &catalog(),
            &bypass_candidates(&catalog()),
            &caster(0, &[FEAR_AURA])
        )
        .is_empty());
        // A confusion flag asks the confusion arm (aura type 5), which a fear aura does not feed.
        assert!(control_bypass_spells(
            &catalog(),
            &bypass_candidates(&catalog()),
            &caster(UNIT_FLAG_CONFUSED, &[FEAR_AURA])
        )
        .is_empty());
    }

    /// A charm is read off `UNIT_FIELD_CHARMEDBY`, whoever the charmer is, over types 6, 177
    /// and 2.
    #[test]
    fn a_charm_asks_its_three_aura_types() {
        let mut charm = SpellDisplay {
            mechanic: 1,
            ..Default::default()
        };
        charm.effects[0] = 6;
        charm.effect_apply_aura[0] = 2;
        let cat = SpellCatalog::from_displays(
            [
                (100, charm),
                (INSIGNIA, immunity_to(1)),
                (9999, immunity_to(5)),
            ]
            .into(),
        );
        // `UNIT_FIELD_CHARMEDBY` is index 10.
        let mut pairs = vec![(10u16, 0x77u32), (AURA_0, 100)];
        pairs.push((FLAGS, 0));
        let fields = ObjectFields::from_pairs(&pairs);
        assert_eq!(
            control_bypass_spells(&cat, &bypass_candidates(&cat), &fields),
            HashSet::from([INSIGNIA])
        );
    }

    /// The gate asks the host for the exemption off the caster at the call: feared, with the
    /// immunity trinket's spell on the item, the use goes; the fear's flag and aura lifted, with
    /// control still out, the same call is refused, with no push between.
    #[test]
    fn the_gate_reads_the_caster_at_the_call() {
        use benilla_ui::script::{ContainerSlot, ContainerState, ScriptCall, UiScript};
        use bevy::prelude::*;

        let mut world = World::new();
        world.init_resource::<crate::net::Reputations>();
        world.insert_resource(crate::ui_action::Spells {
            catalog: catalog(),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let mut script = UiScript::new().unwrap();
        script.set_container(
            0,
            Some(ContainerState {
                name: None,
                num_slots: 16,
                slots: [(
                    1,
                    ContainerSlot {
                        guid: 0x11,
                        item_id: 871,
                        count: 1,
                        item_class: Some(0),
                        use_spell: INSIGNIA,
                        ..Default::default()
                    },
                )]
                .into(),
            }),
        );
        script.set_player_control(false);
        world.insert_non_send_resource(script);
        let me = world
            .spawn((
                crate::net::SelfPlayer,
                crate::net::ObjectStore(caster(UNIT_FLAG_FLEEING, &[FEAR_AURA])),
            ))
            .id();
        let refused = |world: &mut World| {
            let ((), calls) =
                crate::ui_unit::live::eval_lent_calls::<()>(world, "UseContainerItem(0, 1)");
            match calls.as_slice() {
                [ScriptCall::UseContainerItem(_)] => false,
                [ScriptCall::DisplayError("ERR_CLIENT_LOCKED_OUT")] => true,
                other => panic!("one use or its refusal, not {other:?}"),
            }
        };
        assert!(!refused(&mut world), "the trinket breaks the fear");
        world
            .entity_mut(me)
            .insert(crate::net::ObjectStore(caster(0, &[])));
        assert!(refused(&mut world), "nothing exempts it now");
    }
}
