//! The ladder with a unit other than the active player as its caster: the pet bar's
//! client-targeted and possessed presses, which call the attempt themselves (`0x4bd385`:
//! `0x6e4b60(unit, spell, 0, guid)`), and `SendCast`'s tail for such a caster.
//!
//! The rungs read the caster through the context ([`cast_target::CastTargeting::context_for_caster`]);
//! the reagents and totems (`0x6e400e`), the equipped item (`0x6e40e0`) and the combo points
//! (`0x6e4e5b`) pass any caster but the active player, and the charm rung compares the charmer
//! with the active player (`0x60994d`). The validator's cooldown rung queries the player's history
//! whatever the caster (`0x60952e`, `0x609569`: `0x6e2ea0(spell, 0)`).

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

/// What the attempt reads besides its caster when that caster is not the active player.
pub(super) struct UnitCaster<'a> {
    /// The active player's guid (`0x468550`).
    pub(super) active: Option<u64>,
    pub(super) pet_gcds: Option<&'a mut PetGcdsOwed>,
}

impl TargetedBind {
    /// The targets block this bind writes (`0x7e4e10`), which every opcode of `SendCast` ends in.
    pub(super) fn block(self) -> UseItemTarget {
        match self {
            TargetedBind::Dest(dest) => UseItemTarget::Dest(dest),
            TargetedBind::Source(src) => UseItemTarget::Source(src),
            TargetedBind::Item(guid) => UseItemTarget::Item(guid),
            TargetedBind::Object(guid) => UseItemTarget::Object(guid),
            TargetedBind::Unit(guid) => UseItemTarget::Unit(guid),
            TargetedBind::Corpse(guid, target) => UseItemTarget::Corpse(target, guid),
        }
    }
}

impl CastLadder<'_, '_> {
    /// The pet bar's press through the attempt with `caster`, the bar's unit, at the press's
    /// target (`0x4bd378`-`0x4bd385`): the ladder as a player's press runs it, with no attack
    /// pick held, sent as `CMSG_PET_CAST_SPELL`.
    pub(crate) fn send_pet(&mut self, spell_id: u32, ctx: &cast_target::CastContext, caster: u64) {
        self.send_bound(spell_id, ctx, CastCommit::Pet { caster }, None, None);
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
        let unit = UnitCaster {
            active: self.active.as_ref().and_then(|g| g.0),
            pet_gcds: self.pet_gcds.as_deref_mut(),
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
