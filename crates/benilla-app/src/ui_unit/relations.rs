//! The relation functions the six relation verbs ask, as of the call: the host's answer
//! ([`super::live::UnitFeedLive`]) over the units the object manager holds then. These tests drive
//! the verbs through it, and a harness with no host to lend seats a copy instead ([`seat`]).

use std::collections::HashMap;
use std::rc::Rc;

use bevy::prelude::*;

use crate::net::{Guid, ObjectStore};
use crate::target::relations::RelationSnapshot;

/// Seat a copy of the relation functions over every held unit, each frame: the stand-in, for a
/// harness that lends no host, for the host's answers at the call.
pub(crate) fn seat(
    mut vm: crate::script_calls::VmWith<crate::target::RelationWorld<'static, 'static>>,
    units: Query<(&Guid, &ObjectStore)>,
) {
    vm.turn(|script, world| {
        let Some(script) = script else {
            return;
        };
        let mut held: HashMap<u64, _> = units
            .iter()
            .filter(|(_, s)| s.is_unit())
            .map(|(g, s)| (g.0, world.unit_of(g.0, s)))
            .collect();
        if let Some(me) = world.me() {
            held.insert(me.guid, me);
        }
        script.set_unit_relations(Some(Rc::new(RelationSnapshot {
            units: held,
            factions: world.factions().cloned(),
            reputations: world.reputations().clone(),
            me: world.me_guid(),
            roster: world.roster().cloned().unwrap_or_default(),
        })));
    })
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use benilla_ui::script::{UiScript, UnitState};

    use crate::names::NameCache;
    use crate::net::{Guid, GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
    use crate::spell::group_relation::GroupRoster;
    use crate::target::relations::fixture::*;
    use crate::target::RelUnit;

    const ME: u64 = 0x10;
    const FOE: u64 = 0x11;
    const FOE_PET: u64 = 0xF140_0000_0000_0012;
    const GUARD_NPC: u64 = 0xF130_0000_0000_0013;
    const ARBITER: u64 = 0xF110_0000_0000_0001;

    /// The world the feed reads: the fixture's tables, the active player `me` and `units`, each
    /// indexed by guid; the VM's `player`, `target`, `mouseover` and `npc` name `me`, `FOE`,
    /// `FOE_PET` and `GUARD_NPC`.
    fn world(me: RelUnit, units: &[RelUnit]) -> World {
        let mut world = World::new();
        world.insert_resource(factions());
        world.init_resource::<Reputations>();
        world.init_resource::<NameCache>();
        world.init_resource::<GroupRoster>();
        world.insert_resource(SelfGuid(Some(me.guid)));
        let mut index = GuidIndex::default();
        let e = world.spawn((SelfPlayer, Guid(me.guid), store_of(&me))).id();
        index.0.insert(me.guid, e);
        for u in units {
            let e = world.spawn((Guid(u.guid), store_of(u))).id();
            index.0.insert(u.guid, e);
        }
        world.insert_resource(index);
        let mut script = UiScript::new().unwrap();
        for (token, guid) in [
            ("player", ME),
            ("target", FOE),
            ("mouseover", FOE_PET),
            ("npc", GUARD_NPC),
        ] {
            script.set_unit(
                token,
                Some(UnitState {
                    exists: true,
                    guid,
                    ..Default::default()
                }),
            );
        }
        world.insert_non_send_resource(script);
        world
    }

    /// `call`'s answer with the host lent, the units as the world holds them now.
    fn ask(world: &mut World, call: &str) -> Option<i64> {
        crate::ui_unit::live::eval_lent(world, &format!("return {call}"))
    }

    /// A duel through the Lua verbs: the opponent and his pet are hostile and attackable from
    /// either side, so `UnitIsEnemy` and `UnitCanAttack` answer both argument orders, and the
    /// duel's end, a descriptor write, is in the next answer.
    #[test]
    fn the_verbs_answer_a_duel_both_ways_and_follow_its_end() {
        let me = duelling(player(ME, HUMAN), ARBITER, 1);
        let foe = duelling(player(FOE, HUMAN), ARBITER, 2);
        let pet = pet_of(FOE_PET, &foe);
        let mut world = world(me, &[foe, pet]);
        for (a, b) in [
            ("player", "target"),
            ("target", "player"),
            ("player", "mouseover"),
            ("mouseover", "player"),
        ] {
            assert_eq!(
                ask(&mut world, &format!(r#"UnitReaction("{a}", "{b}")"#)),
                Some(2)
            );
            assert_eq!(
                ask(&mut world, &format!(r#"UnitIsEnemy("{a}", "{b}")"#)),
                Some(1)
            );
            assert_eq!(
                ask(&mut world, &format!(r#"UnitCanAttack("{a}", "{b}")"#)),
                Some(1)
            );
        }
        // The duel ends: the teams clear, and two humans are friends that cannot attack.
        let foe_entity = world.resource::<GuidIndex>().0[&FOE];
        *world.get_mut::<ObjectStore>(foe_entity).unwrap() = store_of(&player(FOE, HUMAN));
        assert_eq!(
            ask(&mut world, r#"UnitReaction("target", "player")"#),
            Some(5)
        );
        assert_eq!(
            ask(&mut world, r#"UnitCanAttack("player", "target")"#),
            None
        );
        // The pet's owner moved too: it answers with him.
        assert_eq!(
            ask(&mut world, r#"UnitCanAttack("player", "mouseover")"#),
            None
        );
    }

    /// A contested player: a guard whose template turns on contested players is hostile to him
    /// and attacks him, while his own reaction toward the guard stays the templates', friendly.
    #[test]
    fn a_contested_player_is_the_guards_enemy_one_way() {
        let me = flagged(player(ME, HUMAN), CONTESTED);
        let mut world = world(me, &[creature(GUARD_NPC, GUARD)]);
        assert_eq!(ask(&mut world, r#"UnitReaction("npc", "player")"#), Some(2));
        assert_eq!(
            ask(&mut world, r#"UnitCanAttack("npc", "player")"#),
            Some(1)
        );
        assert_eq!(ask(&mut world, r#"UnitReaction("player", "npc")"#), Some(5));
        assert_eq!(ask(&mut world, r#"UnitCanAttack("player", "npc")"#), None);
    }

    /// Two free-for-all players attack each other, and a charmed player of our side cooperates
    /// with nobody (`0x606bab`), while an uncharmed one does.
    #[test]
    fn free_for_all_and_a_charm_reach_the_verbs() {
        let me = flagged(player(ME, HUMAN), FFA);
        let mut world = world(me, &[flagged(player(FOE, HUMAN), FFA)]);
        assert_eq!(
            ask(&mut world, r#"UnitCanAttack("target", "player")"#),
            Some(1)
        );
        assert_eq!(ask(&mut world, r#"UnitIsFriend("player", "target")"#), None);
        assert_eq!(
            ask(&mut world, r#"UnitCanCooperate("player", "target")"#),
            Some(1)
        );
        let charmed = RelUnit {
            charmed_by: FOE_PET,
            ..player(FOE, HUMAN)
        };
        let mut world = self::world(player(ME, HUMAN), &[charmed]);
        assert_eq!(
            ask(&mut world, r#"UnitCanCooperate("player", "target")"#),
            None
        );
        assert_eq!(
            ask(&mut world, r#"UnitCanCooperate("target", "player")"#),
            None
        );
    }

    /// `UnitIsPVP` asks `IsPvP` of a held unit (`0x5164d1`), as the selector's PvP split does: a pet
    /// answers with its owner's flag, its own clear or set, and a unit under bit 8 answers no,
    /// though its owner and itself are flagged. The pushed snapshots carry no PvP flag here.
    #[test]
    fn unit_is_pvp_is_the_owners_flag_on_a_pet_and_none_under_bit_8() {
        let foe = with_flags(player(FOE, HUMAN), PVP);
        let mut world = self::world(player(ME, HUMAN), &[foe, pet_of(FOE_PET, &foe)]);
        assert_eq!(ask(&mut world, r#"UnitIsPVP("target")"#), Some(1));
        assert_eq!(
            ask(&mut world, r#"UnitIsPVP("mouseover")"#),
            Some(1),
            "the owner's"
        );
        assert_eq!(ask(&mut world, r#"UnitIsPVP("npc")"#), None, "not held");
        let plain = player(FOE, HUMAN);
        let mut world = self::world(
            player(ME, HUMAN),
            &[plain, with_flags(pet_of(FOE_PET, &plain), PVP)],
        );
        assert_eq!(
            ask(&mut world, r#"UnitIsPVP("mouseover")"#),
            None,
            "its own"
        );
        let thrall = RelUnit {
            charmed_by: FOE,
            ..with_flags(player(FOE_PET, HUMAN), PVP | IMMUNE_TO_PLAYER)
        };
        let mut world = self::world(player(ME, HUMAN), &[foe, thrall]);
        assert_eq!(ask(&mut world, r#"UnitIsPVP("mouseover")"#), None, "bit 8");
    }

    /// A unit that leaves the object manager leaves the relation functions: the verbs answer nil.
    #[test]
    fn a_despawned_unit_answers_nil() {
        let mut world = world(player(ME, HUMAN), &[creature(GUARD_NPC, ORC)]);
        assert_eq!(ask(&mut world, r#"UnitReaction("npc", "player")"#), Some(2));
        let e = world.resource::<GuidIndex>().0[&GUARD_NPC];
        world.despawn(e);
        assert_eq!(ask(&mut world, r#"UnitReaction("npc", "player")"#), None);
    }
}
