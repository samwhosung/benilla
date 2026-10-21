//! The ladder with a unit other than the active player as its caster: the pet bar's
//! client-targeted and possessed presses, which call the attempt themselves (`0x4bd385`:
//! `0x6e4b60(unit, spell, 0, guid)`), and `SendCast`'s tail for such a caster.
//!
//! The rungs read the caster through the context ([`cast_target::CastTargeting::context_for_caster`]);
//! the loot window (`0x6126f0`), the reagents and totems (`0x6e400e`), the equipped item
//! (`0x6e40e0`) and the combo points (`0x6e4e5b`) pass any caster but the active player, the
//! ranged rung asks the caster's own `GetVirtualItem` (vtable slot 38), and the charm rung compares
//! the charmer with the active player (`0x60994d`). The validator's cooldown rung queries the
//! player's history whatever the caster (`0x60952e`, `0x609569`: `0x6e2ea0(spell, 0)`), and its
//! reputation rung the player's standing (`0x609f29` → `0x4d6370`).

use std::time::Instant;

use benilla_protocol::messages::{OwnerFallback, UseItemTarget};
use bevy::prelude::*;

use crate::net::Objects;

use super::{cast_target, CastCommit, CastCore, CastLadder, TargetedBind};

/// The GCDs a unit caster's sends started in the pet's history (`0x6e2de0(spell, 1)` from
/// `0x6e58fb`), which the pet bar keeps ([`crate::ui_pet::PetBar`]); it takes them before its
/// next feed. Each is the spell and the instant of its send.
#[derive(Resource, Default)]
pub(crate) struct PetGcdsOwed(Vec<(u32, Instant)>);

impl PetGcdsOwed {
    /// The GCDs owed since the last take, in send order.
    pub(crate) fn take(&mut self) -> Vec<(u32, Instant)> {
        std::mem::take(&mut self.0)
    }
}

/// The attack a unit caster's send owes: TryCast's tail calls `StartAttack 0x6131a0` on the caster
/// at its target (`0x6e51c9`-`0x6e51de`) for a spell that starts melee from its send, unless the
/// caster is attacking already (`0x60ecb0`). Its caller, which holds the pet bar, applies it
/// before the next call reads anything ([`crate::ui_pet::PetPress::begin_attack_from_cast`]).
/// The caster and the target.
#[derive(Resource, Default)]
pub(crate) struct UnitAttackOwed(pub(super) Option<(u64, u64)>);

impl UnitAttackOwed {
    /// The attack owed since the last take.
    pub(crate) fn take(&mut self) -> Option<(u64, u64)> {
        self.0.take()
    }
}

/// What the attempt reads besides its caster when that caster is not the active player.
pub(super) struct UnitCaster<'a> {
    /// The active player's guid (`0x468550`).
    pub(super) active: Option<u64>,
    pub(super) pet_gcds: Option<&'a mut PetGcdsOwed>,
    /// The caster's `[unit+0xc48]` (`0x60ecb0`), which the pick (`0x6e4eeb`) and the attack tail
    /// (`0x6e51cb`) test.
    pub(super) engaged: bool,
    pub(super) attack: Option<&'a mut UnitAttackOwed>,
}

impl TargetedBind {
    /// The targets block this bind writes (`0x7e4e10`), which every opcode of `SendCast` ends in.
    pub(super) fn block(self) -> UseItemTarget {
        match self {
            TargetedBind::Dest(dest) => UseItemTarget::Dest(dest),
            TargetedBind::Source(src) => UseItemTarget::Source(src),
            TargetedBind::Item(guid) => UseItemTarget::Item(guid),
            TargetedBind::TradeItem(slot) => UseItemTarget::TradeItem(slot),
            TargetedBind::Object(guid) => UseItemTarget::Object(guid),
            TargetedBind::Unit(guid) => UseItemTarget::Unit(guid),
            TargetedBind::Corpse(guid, target) => UseItemTarget::Corpse(target, guid),
        }
    }
}

impl CastLadder<'_, '_> {
    /// The pet bar's press through the attempt with `caster`, the bar's unit, at the press's
    /// target (`0x4bd378`-`0x4bd385`): the ladder as a player's press runs it, held at the attack
    /// pick with the unit as its actor (`0x6e4ef4`), sent as `CMSG_PET_CAST_SPELL`.
    pub(crate) fn send_pet(&mut self, spell_id: u32, ctx: &cast_target::CastContext, caster: u64) {
        let held = super::HeldCast {
            spell_id,
            on_self: false,
            unit: Some(super::UnitPress {
                caster,
                target: ctx.selection_guid,
            }),
        };
        self.send_bound(spell_id, ctx, CastCommit::Pet { caster }, None, Some(held));
    }

    /// The attack a unit caster's send owes ([`UnitAttackOwed`]), if one does.
    pub(crate) fn take_unit_attack(&mut self) -> Option<(u64, u64)> {
        self.unit_attack
            .as_deref_mut()
            .and_then(UnitAttackOwed::take)
    }

    /// [`send_tail`] for a cursor's commit of a unit caster's cast.
    pub(super) fn unit_send_tail(&mut self, spell_id: u32, caster: u64, now: Instant) {
        let def = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
        };
        // A cursor's commit is past TryCast, whose attack tail it never reaches.
        let unit = UnitCaster {
            active: self.active.as_ref().and_then(|g| g.0),
            pet_gcds: self.pet_gcds.as_deref_mut(),
            engaged: false,
            attack: None,
        };
        send_tail(
            spell_id,
            caster,
            def,
            &self.objects,
            &mut core,
            &mut self.cooldowns,
            unit,
            &self.spell_mods,
            now,
        );
    }
}

/// `SendCast`'s tail after a unit caster's packet (`0x6e5877`-`0x6e58fb`). The send's own signal
/// and precast are the active player's (`0x6e5877`-`0x6e5898`). The attempt set the item flag
/// `0xcecaa4` only for the active player, so the spell it pushed is popped (`0x6e58a2`-`0x6e58bd`).
/// The GCD goes into the pet's history when the caster's charmed-by guid, else its summoned-by
/// guid, is the active player's (`0x6e58c2`-`0x6e58f3`), else into the player's (`0x6e58fb`). The
/// caster's ranged stance and an auto-repeat's arm (`0x6e591c`-`0x6e5976`) are not applied here.
#[allow(clippy::too_many_arguments)] // the tail's inputs, as `send_spell_cast` holds them
pub(super) fn send_tail(
    spell_id: u32,
    caster: u64,
    def: Option<&benilla_formats::SpellDisplay>,
    objects: &Objects,
    core: &mut CastCore,
    cooldowns: &mut crate::spell::Cooldowns,
    unit: UnitCaster,
    spell_mods: &crate::spell::SpellModifiers,
    now: Instant,
) {
    core.pop_if(spell_id);
    let Some(d) = def else {
        return;
    };
    let owner = objects
        .store(caster)
        .and_then(|s| s.0.unit_owner(OwnerFallback::SummonedBy));
    if owner.is_none() || owner != unit.active {
        cooldowns.start_gcd(spell_id, d, now, spell_mods);
        return;
    }
    // A world with no pet bar keeps no pet history to owe.
    if let Some(owed) = unit.pet_gcds {
        owed.0.push((spell_id, now));
    }
}

#[cfg(test)]
mod tests {
    use benilla_formats::SpellDisplay;
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    use super::super::tests::{ctx, send, world};
    use super::*;
    use crate::net::{ClientCommand, GuidIndex, ObjectStore, SelfGuid};
    use crate::target::Factions;
    use crate::ui_action::CastFail;
    use crate::ui_cast::CastReports;

    const ME: u64 = 0x10;
    const PET: u64 = 0x2a;
    const AS_PET: CastCommit = CastCommit::Pet { caster: PET };

    fn spells(rows: impl IntoIterator<Item = (u32, SpellDisplay)>) -> crate::ui_action::Spells {
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(rows.into_iter().collect());
        spells
    }

    /// The attempt for `spell_id` as `commit`, with `fields` over a living caster's descriptor and
    /// `factions` the catalog the reputation rung reads: what it refused.
    fn press(
        world: &mut World,
        spell_id: u32,
        commit: CastCommit,
        fields: &[(u16, u32)],
        factions: Option<&'static Factions>,
    ) -> Vec<CastFail> {
        let mut pairs = vec![(22, 100)];
        pairs.extend_from_slice(fields);
        let store: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&pairs))));
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let base = ctx();
                let with_caster = cast_target::CastContext {
                    rel: cast_target::TargetRelations {
                        self_store: Some(store),
                        factions,
                        ..base.rel
                    },
                    ..base
                };
                ladder.send(spell_id, &with_caster, commit);
            })
            .expect("the ladder runs as a one-shot system");
        world.resource_mut::<CastReports>().take_refusals()
    }

    /// The casts sent since the last look, as `(caster, spell)`: the unit's in
    /// `CMSG_PET_CAST_SPELL`, the player's `CMSG_CAST_SPELL` as caster 0.
    fn casts(rx: &Receiver<ClientCommand>) -> Vec<(u64, u32)> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::PetCastSpell {
                    caster_guid,
                    spell_id,
                    ..
                } => Some((caster_guid, spell_id)),
                ClientCommand::CastSpell { spell_id, .. } => Some((0, spell_id)),
                _ => None,
            })
            .collect()
    }

    /// autoSelfCast's fallback binds the active player whoever casts (`0x6e53d7`-`0x6e5400`:
    /// `0x468550`, typemask `0x10`), tested against the caster: a pet's friendly spell with nothing
    /// selected goes out at us, which the pet may assist, never at the pet.
    #[test]
    fn a_unit_casters_auto_self_cast_binds_the_active_player() {
        use crate::target::relations::fixture::{factions, HUMAN};
        const MEND: u32 = 136;
        let (mut world, rx) = world();
        // Implicit target 21, a friend: the assist word alone.
        let mend = SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        };
        world.insert_resource(spells([(MEND, mend)]));
        let factions: &'static Factions = Box::leak(Box::new(factions()));
        // Both alive (22) under the Human template (`UNIT_FIELD_FACTIONTEMPLATE`, 35).
        let living = || ObjectStore(ObjectFields::from_pairs(&[(22, 100), (35, HUMAN)]));
        let pet: &'static ObjectStore = Box::leak(Box::new(living()));
        world.insert_resource(SelfGuid(Some(ME)));
        let me = world.spawn(living()).id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let base = ctx();
                let as_pet = cast_target::CastContext {
                    self_guid: Some(PET),
                    auto_self_cast: true,
                    rel: cast_target::TargetRelations {
                        self_store: Some(pet),
                        factions: Some(factions),
                        group: crate::spell::group_relation::GroupInputs {
                            self_guid: Some(PET),
                            ..Default::default()
                        },
                        ..base.rel
                    },
                    ..base
                };
                ladder.send(MEND, &as_pet, AS_PET);
            })
            .expect("the ladder runs as a one-shot system");
        let sent: Vec<_> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::PetCastSpell { target, .. } => Some(target),
                _ => None,
            })
            .collect();
        assert_eq!(sent, [UseItemTarget::Unit(ME)]);
    }

    /// The loot rungs (`0x6e4ca2`-`0x6e4d31`) read the caster's loot guid through `0x6126f0`,
    /// which is 0 for any caster but the active player (`0x5fa6d0`): with a window latched, the
    /// player's Disenchant is refused `0x82` and a unit's goes out.
    #[test]
    fn a_unit_caster_has_no_loot_window() {
        const DISENCHANT: u32 = 13262;
        let (mut world, rx) = world();
        let disenchant = SpellDisplay {
            effects: [99, 0, 0],
            ..Default::default()
        };
        world.insert_resource(spells([(DISENCHANT, disenchant)]));
        world.insert_resource(crate::ui_loot::LootLatch(Some(0xF110_0000_0000_0042)));

        send(&mut world, DISENCHANT, CastCommit::Spell);
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            [CastFail::local(DISENCHANT, 0x82)]
        );
        assert!(casts(&rx).is_empty());

        send(&mut world, DISENCHANT, AS_PET);
        assert!(world
            .resource_mut::<CastReports>()
            .take_refusals()
            .is_empty());
        assert_eq!(casts(&rx), [(PET, DISENCHANT)]);
    }

    /// The ranged rung (`0x6e4e13`) asks the caster's own `GetVirtualItem`, vtable slot 38, for
    /// hand 2. A creature's (`0x605e30`) is null with no display id in
    /// `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY + 2` (`0x605f10`), whatever else its descriptor holds; a
    /// player's, possessed ones included (`0x5ec240`), reads its visible slot 17 (`0x5f0d60`).
    #[test]
    fn a_unit_casters_ranged_rung_asks_its_own_weapon() {
        const THROW: u32 = 2764;
        const DISPLAY_RANGED: u16 = 37 + 2;
        const VISIBLE_RANGED_ENTRY: u16 = 258 + 12 * 17 + 2;
        const PLAYER_TYPE: (u16, u32) = (2, 0x19);
        let (mut world, rx) = world();
        let throw = SpellDisplay {
            attributes: 0x2,
            ..Default::default()
        };
        world.insert_resource(spells([(THROW, throw)]));
        let refused = [CastFail::local(THROW, 0x18)];

        let creature_with_slot_17 = [(VISIBLE_RANGED_ENTRY, 2504)];
        assert_eq!(
            press(&mut world, THROW, AS_PET, &creature_with_slot_17, None),
            refused,
            "a creature's hand 2 is its display id"
        );
        assert_eq!(
            press(&mut world, THROW, AS_PET, &[(DISPLAY_RANGED, 5260)], None),
            []
        );
        assert_eq!(
            press(
                &mut world,
                THROW,
                AS_PET,
                &[PLAYER_TYPE, (DISPLAY_RANGED, 5260)],
                None
            ),
            refused,
            "a possessed player's is its visible slot"
        );
        assert_eq!(
            press(
                &mut world,
                THROW,
                AS_PET,
                &[PLAYER_TYPE, (VISIBLE_RANGED_ENTRY, 2504)],
                None
            ),
            []
        );
        assert_eq!(casts(&rx), [(PET, THROW), (PET, THROW)]);
    }

    /// The reputation rung (`0x609f1f`) asks `GetFactionStanding 0x4d6370` with no caster: the
    /// active player's base for its race and class plus the wire, whoever casts. A human player
    /// Friendly by the faction's base passes the floor for a unit of another race, which as the
    /// player's own descriptor would be refused `0x8f`.
    #[test]
    fn a_unit_casters_reputation_rung_reads_the_players_standing() {
        const FACTION: u32 = 529;
        const WARD: u32 = 17670;
        // `UNIT_FIELD_BYTES_0` (36): race in byte 0, class in byte 1.
        const HUMAN_WARRIOR: (u16, u32) = (36, 0x0101);
        const ORC_WARRIOR: (u16, u32) = (36, 0x0102);
        let (mut world, rx) = world();
        let ward = SpellDisplay {
            min_faction_id: FACTION,
            // Friendly, whose floor is 3000.
            min_reputation: 4,
            ..Default::default()
        };
        world.insert_resource(spells([(WARD, ward)]));
        // Friendly by its base for a human (race bit 1) alone.
        let catalog =
            benilla_formats::FactionCatalog::from_rows(Default::default(), Default::default())
                .with_reputation(FACTION, 0, 0x1, 3000);
        let factions: &'static Factions = Box::leak(Box::new(Factions::from_catalog(catalog)));
        world.insert_resource(SelfGuid(Some(ME)));
        let me = world
            .spawn(ObjectStore(ObjectFields::from_pairs(&[HUMAN_WARRIOR])))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);

        assert_eq!(
            press(&mut world, WARD, AS_PET, &[ORC_WARRIOR], Some(factions)),
            []
        );
        assert_eq!(casts(&rx), [(PET, WARD)]);
        assert_eq!(
            press(
                &mut world,
                WARD,
                CastCommit::Spell,
                &[ORC_WARRIOR],
                Some(factions)
            ),
            [CastFail::local(WARD, 0x8f)]
        );
        assert!(casts(&rx).is_empty());
    }
}
