//! The range pre-check `CheckSpellTargetRange 0x6e4440` and the answer `IsActionInRange`
//! (`0x4e7550`) builds on it in `0x4e56f0`. The pre-check compares the distance only when a
//! relation the spell's targets name admits the target, so a heal held over a far enemy, or a spell
//! whose targets name no unit, passes without a compare.

use benilla_formats::{SpellDisplay, SpellRange};
use benilla_protocol::messages::ObjectType;

use super::bind_gates::unit_alive;
use super::cast_target::{CastContext, TargetRelations};
use super::SpellModifiers;
use crate::target::corpse_friendly;

/// The first effect's implicit targets for which `0x4e56f0` answers 1 on a pass, the zero bytes of
/// its table at `0x4e57cc` (indexed by the target less 6, `0x4e5786`-`0x4e5795`). Any other target
/// answers nil.
const IN_RANGE_TARGETS: [u32; 9] = [6, 21, 23, 25, 26, 35, 45, 57, 61];

/// The target mask `0x6e4440` tests: `Targets`' low word (`0x6e4488`), edited by each of the three
/// effects' first implicit target through the jump table at `0x6e4748` (`0x6e4497`-`0x6e44f7`).
pub(crate) fn precheck_mask(def: &SpellDisplay) -> u16 {
    let mut mask = def.targets as u16;
    for target in def.effect_implicit_target_a {
        match target {
            1 => mask &= !0x0400,
            5 => mask &= !0x8000,
            6 | 53 => mask |= 0x0080,
            21 | 45 => mask |= 0x0100,
            23 => mask |= 0x0800,
            25 | 63 => mask |= 0x0002,
            26 => mask |= 0x4000,
            35 => mask |= 0x0008,
            57 | 61 => mask |= 0x0004,
            _ => {}
        }
    }
    mask
}

/// `0x6e4440(player, spell, 0, 0)` on the selection: false only when a relation the mask names
/// admits the target and `in_range`, the compare `0x6e47b0`, fails. A selection whose object is not
/// held passes (`0x6e4482`). A unit passes untested when it carries `UNIT_FIELD_FLAGS & 0x10000`
/// (`0x6e4517`), when the spell's `TargetCreatureType` excludes it (`0x6e4524`-`0x6e4550`), when it
/// is alive and the mask has 0x400 (`0x6e4583`), or when it is dead and neither its dynamic flag
/// 0x20, a mask bit of 0x8600 nor `AttributesEx2 & 1` admits it (`0x6e4556`-`0x6e4579`). Else the
/// first relation that holds compares (`0x6e458c`-`0x6e4698`): party and assist (8), raid and
/// assist (4), assist (0x100), attack (0x80), a unit not immune to players (2), a dead player or
/// pet the player can assist (0x8000, which passes when the assist fails), a dead player the player
/// cannot assist (0x200). A game object compares under 0x4800 (`0x6e46a4`); a corpse the player
/// may assist under 0x8000 and one it may not under 0x200 (`0x6e46b2`-`0x6e46ff`).
pub(crate) fn passes_range_precheck(
    def: &SpellDisplay,
    rel: &TargetRelations,
    is_self: bool,
    in_range: impl FnOnce() -> bool,
) -> bool {
    let Some(store) = rel.target_store else {
        return true;
    };
    let mask = precheck_mask(def);
    let fields = &store.0;
    let is_player = fields.object_type() == Some(ObjectType::Player);
    let compares = if store.is_unit() {
        let alive = unit_alive(fields);
        if fields.unit_flags() & 0x0001_0000 != 0
            || !def.admits_creature_type(rel.types.of(store))
            || (alive && mask & 0x0400 != 0)
            || (!alive
                && !fields.unit_dynflag_dead()
                && mask & 0x8600 == 0
                && !def.allows_dead_target())
        {
            return true;
        }
        let assist = || rel.assistable(is_self);
        if mask & 0x0008 != 0 && rel.in_party(is_self) && assist()
            || mask & 0x0004 != 0 && rel.in_group(is_self) && assist()
            || mask & 0x0100 != 0 && assist()
            || mask & 0x0080 != 0 && rel.attackable(is_self)
            || mask & 0x0002 != 0 && fields.unit_flags() & 0x0100 == 0
        {
            true
        } else if mask & 0x8000 != 0 && !alive && (is_player || fields.unit_pet_number() != 0) {
            assist()
        } else {
            mask & 0x0200 != 0 && !alive && is_player && !assist()
        }
    } else if fields.object_type() == Some(ObjectType::GameObject) {
        mask & 0x4800 != 0
    } else if fields.created_as() == Some(ObjectType::Corpse) {
        let unclaimed = fields.corpse_flags() & 1 == 0;
        let friendly = || corpse_friendly(store, rel.factions, rel.self_store);
        (mask & 0x8000 != 0 && unclaimed && friendly())
            || (mask & 0x0200 != 0 && unclaimed && !friendly())
    } else {
        false
    };
    !compares || in_range()
}

/// `0x4e56f0`'s answer for a slot's spell once the player is held: 0 when the pre-check refuses the
/// selection (`0x4e5770`); 1 when it passes, the first effect's implicit target is in
/// [`IN_RANGE_TARGETS`] and a target is selected (`0x4e579c`); nil otherwise. `row` and `mods`
/// feed `GetMinMaxRange 0x6e3480`, which the compare hands the selection (`0x6e47ca`).
pub(crate) fn action_in_range(
    def: &SpellDisplay,
    ctx: &CastContext,
    row: Option<&SpellRange>,
    mods: &SpellModifiers,
) -> Option<bool> {
    let is_self = ctx.selection_guid.is_some() && ctx.selection_guid == ctx.self_guid;
    let passes = passes_range_precheck(def, &ctx.rel, is_self, || {
        ctx.range.refusal(def, row, mods).is_none()
    });
    if !passes {
        return Some(false);
    }
    (IN_RANGE_TARGETS.contains(&def.effect_implicit_target_a[0]) && ctx.selection_guid.is_some())
        .then_some(true)
}
