//! The relation feed: the copy of the relation functions the Lua API asks
//! ([`RelationSnapshot`]), pushed into the VM.

use std::collections::HashMap;
use std::rc::Rc;

use bevy::prelude::*;

use crate::net::{Guid, ObjectStore};
use crate::target::relations::RelationSnapshot;
use crate::target::{RelUnit, RelationWorld};
use crate::ui_script::VmMemo;

/// The units [`feed_unit_relations`] last pushed into a VM.
#[derive(Default)]
pub(super) struct RelationMemo {
    units: HashMap<u64, RelUnit>,
    /// The active player, whose own entity may carry no `Guid`.
    me: Option<RelUnit>,
}

/// Push the relation functions over every held unit, ahead of the unit feeds that fire events
/// (`PLAYER_TARGET_CHANGED`, `UNIT_FACTION`), so a handler that asks `UnitReaction` or
/// `UnitCanAttack` judges this frame's units. A unit is read again only when its descriptor moved
/// or a creature record arrived, and the copy is pushed only when a unit's relation facts, the set
/// of units or a relation input moved.
pub(super) fn feed_unit_relations(
    mut hosted_vm: crate::script_calls::HostedVm,
    world: RelationWorld,
    units: Query<(&Guid, Ref<ObjectStore>)>,
    mut memo: Local<VmMemo<RelationMemo>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let (memo, vm_reset) = memo.get_reset(script);
        let reread = world.records_changed();
        let mut moved = false;
        let mut held = 0;
        for (guid, store) in &units {
            if !store.is_unit() {
                continue;
            }
            held += 1;
            if !reread && !store.is_changed() && memo.units.contains_key(&guid.0) {
                continue;
            }
            let unit = world.unit_of(guid.0, &store);
            moved |= memo.units.insert(guid.0, unit) != Some(unit);
        }
        // A unit gone since: start over from the held ones.
        if memo.units.len() != held {
            memo.units = units
                .iter()
                .filter(|(_, s)| s.is_unit())
                .map(|(g, s)| (g.0, world.unit_of(g.0, &s)))
                .collect();
            moved = true;
        }
        let me = world.me();
        moved |= std::mem::replace(&mut memo.me, me) != me;
        if !(moved || vm_reset || world.inputs_changed()) {
            return;
        }
        let mut units = memo.units.clone();
        if let Some(me) = me {
            units.insert(me.guid, me);
        }
        script.set_unit_relations(Some(Rc::new(RelationSnapshot {
            units,
            factions: world.factions().cloned(),
            reputations: world.reputations().clone(),
            me: world.me_guid(),
            roster: world.roster().cloned().unwrap_or_default(),
        })));
    })
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::UnitState;

    use super::*;
    use crate::names::NameCache;
    use crate::net::{GuidIndex, Reputations, SelfGuid, SelfPlayer};
    use crate::spell::group_relation::GroupRoster;
    use crate::target::relations::fixture::*;
    use benilla_ui::script::UiScript;

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
        feed(&mut world);
        world
    }

    /// One frame of the feed, its memory kept from the last.
    fn feed(world: &mut World) {
        world.run_system_cached(feed_unit_relations).unwrap();
    }

    fn ask(world: &World, call: &str) -> Option<i64> {
        world
            .non_send_resource::<UiScript>()
            .eval::<Option<i64>>(&format!("return {call}"))
            .unwrap()
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
                ask(&world, &format!(r#"UnitReaction("{a}", "{b}")"#)),
                Some(2)
            );
            assert_eq!(
                ask(&world, &format!(r#"UnitIsEnemy("{a}", "{b}")"#)),
                Some(1)
            );
            assert_eq!(
                ask(&world, &format!(r#"UnitCanAttack("{a}", "{b}")"#)),
                Some(1)
            );
        }
        // The duel ends: the teams clear, and two humans are friends that cannot attack.
        let foe_entity = world.resource::<GuidIndex>().0[&FOE];
        *world.get_mut::<ObjectStore>(foe_entity).unwrap() = store_of(&player(FOE, HUMAN));
        feed(&mut world);
        assert_eq!(ask(&world, r#"UnitReaction("target", "player")"#), Some(5));
        assert_eq!(ask(&world, r#"UnitCanAttack("player", "target")"#), None);
        // The pet's owner moved too: it answers with him.
        assert_eq!(ask(&world, r#"UnitCanAttack("player", "mouseover")"#), None);
    }

    /// A contested player: a guard whose template turns on contested players is hostile to him
    /// and attacks him, while his own reaction toward the guard stays the templates', friendly.
    #[test]
    fn a_contested_player_is_the_guards_enemy_one_way() {
        let me = flagged(player(ME, HUMAN), CONTESTED);
        let world = world(me, &[creature(GUARD_NPC, GUARD)]);
        assert_eq!(ask(&world, r#"UnitReaction("npc", "player")"#), Some(2));
        assert_eq!(ask(&world, r#"UnitCanAttack("npc", "player")"#), Some(1));
        assert_eq!(ask(&world, r#"UnitReaction("player", "npc")"#), Some(5));
        assert_eq!(ask(&world, r#"UnitCanAttack("player", "npc")"#), None);
    }

    /// Two free-for-all players attack each other, and a charmed player of our side cooperates
    /// with nobody (`0x606bab`), while an uncharmed one does.
    #[test]
    fn free_for_all_and_a_charm_reach_the_verbs() {
        let me = flagged(player(ME, HUMAN), FFA);
        let world = world(me, &[flagged(player(FOE, HUMAN), FFA)]);
        assert_eq!(ask(&world, r#"UnitCanAttack("target", "player")"#), Some(1));
        assert_eq!(ask(&world, r#"UnitIsFriend("player", "target")"#), None);
        assert_eq!(
            ask(&world, r#"UnitCanCooperate("player", "target")"#),
            Some(1)
        );
        let charmed = RelUnit {
            charmed_by: FOE_PET,
            ..player(FOE, HUMAN)
        };
        let world = self::world(player(ME, HUMAN), &[charmed]);
        assert_eq!(ask(&world, r#"UnitCanCooperate("player", "target")"#), None);
        assert_eq!(ask(&world, r#"UnitCanCooperate("target", "player")"#), None);
    }

    /// `UnitIsPVP` asks `IsPvP` of a held unit (`0x5164d1`), as the selector's PvP split does: a pet
    /// answers with its owner's flag, its own clear or set, and a unit under bit 8 answers no,
    /// though its owner and itself are flagged. The pushed snapshots carry no PvP flag here.
    #[test]
    fn unit_is_pvp_is_the_owners_flag_on_a_pet_and_none_under_bit_8() {
        let foe = with_flags(player(FOE, HUMAN), PVP);
        let world = self::world(player(ME, HUMAN), &[foe, pet_of(FOE_PET, &foe)]);
        assert_eq!(ask(&world, r#"UnitIsPVP("target")"#), Some(1));
        assert_eq!(
            ask(&world, r#"UnitIsPVP("mouseover")"#),
            Some(1),
            "the owner's"
        );
        assert_eq!(ask(&world, r#"UnitIsPVP("npc")"#), None, "not held");
        let plain = player(FOE, HUMAN);
        let world = self::world(
            player(ME, HUMAN),
            &[plain, with_flags(pet_of(FOE_PET, &plain), PVP)],
        );
        assert_eq!(ask(&world, r#"UnitIsPVP("mouseover")"#), None, "its own");
        let thrall = RelUnit {
            charmed_by: FOE,
            ..with_flags(player(FOE_PET, HUMAN), PVP | IMMUNE_TO_PLAYER)
        };
        let world = self::world(player(ME, HUMAN), &[foe, thrall]);
        assert_eq!(ask(&world, r#"UnitIsPVP("mouseover")"#), None, "bit 8");
    }

    /// A unit that leaves the object manager leaves the relation functions: the verbs answer nil.
    #[test]
    fn a_despawned_unit_answers_nil() {
        let mut world = world(player(ME, HUMAN), &[creature(GUARD_NPC, ORC)]);
        assert_eq!(ask(&world, r#"UnitReaction("npc", "player")"#), Some(2));
        let e = world.resource::<GuidIndex>().0[&GUARD_NPC];
        world.despawn(e);
        feed(&mut world);
        assert_eq!(ask(&world, r#"UnitReaction("npc", "player")"#), None);
    }
}
