//! Cast target resolution: what goes in `CMSG_CAST_SPELL`'s target block (reference: `ArmCast
//! 0x6e5250`, `BindTarget 0x6e5b40`). The flag word is seeded from `Spell.dbc` `Targets` and
//! adjusted by the implicit-target switch (jump table `0x6e5484`), then:
//!
//! - word 0: commit at once with mask 0 and no guid, never the selection; the server fills the
//!   target from the spell's implicit targeting.
//! - `Attributes & 0x200`: the candidate is the equipped main hand (`0x6e5361`), bound as an item
//!   with no cursor; an empty hand refuses with "Your weapon hand is empty" (`0x6e504b`).
//! - otherwise each bit is cleared against the selection by its relation check, then against the
//!   player behind `autoSelfCast` (`0x6e53d7`); only a fully cleared word commits, as a unit guid.
//! - a word with item, lock, GameObject or location bits enters the targeting cursor carrying
//!   the whole word: its location (`0x6e6320`, `& 0x60`), item (`0x6e6330`, `& 0x4010`),
//!   GameObject (`0x6e62d0`, `& 0x4800`) and unit (`0x6e6460`) predicates can hold at once, and
//!   the click picks the leg.
//!
//! The unit hand cursor receives a residual unit word without the enemy bit. A word carrying
//! `0x80` instead raises the local no-target/invalid-target refusal; STRING bit 13 remains
//! unmodeled and refuses locally with "Invalid target".

use benilla_formats::SpellDisplay;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::target::{can_assist, can_attack, Factions, Selection};

/// `TARGET_FLAG_*` bits of the client's targeting flag word (`0xcecac0`).
const TF_UNIT: u16 = 0x0002;
const TF_UNIT_RAID: u16 = 0x0004;
const TF_UNIT_PARTY: u16 = 0x0008;
const TF_UNIT_ENEMY: u16 = 0x0080;
const TF_UNIT_ASSIST: u16 = 0x0100;
const TF_CORPSE_ENEMY: u16 = 0x0200;
const TF_EXPLICIT_GATE: u16 = 0x0400;
const TF_CORPSE_ALLY: u16 = 0x8000;
/// The unit-shaped bits a selected unit (alive or dead) can satisfy.
const UNIT_BITS: u16 = TF_UNIT
    | TF_UNIT_RAID
    | TF_UNIT_PARTY
    | TF_UNIT_ENEMY
    | TF_UNIT_ASSIST
    | TF_CORPSE_ENEMY
    | TF_EXPLICIT_GATE
    | TF_CORPSE_ALLY;

/// Client cast-failed reasons used when an enemy word cannot bind.
pub(crate) const ERR_NO_TARGET: u8 = 0x09;
pub(crate) const ERR_INVALID_TARGET: u8 = 0x0A;

/// `SPELL_FAILED_MAINHAND_EMPTY`, "Your weapon hand is empty": the reference's own refusal for an
/// imbue with an empty main hand (`0x6e5050`).
pub(crate) const ERR_MAINHAND_EMPTY: u8 = 0x2d;

/// The dest-location bit, the ground-cast wire mask (`BindLocation 0x6e60f0`).
const TF_DEST_LOCATION: u16 = 0x0040;
/// The source-location bit, `BindLocation 0x6e60f0`'s other arm (`0x6e6105`).
const TF_SOURCE_LOCATION: u16 = 0x0020;
/// `TargetingWantsLocation 0x6e6320`'s mask: the terrain click serves both location bits.
const LOCATION_BITS: u16 = TF_SOURCE_LOCATION | TF_DEST_LOCATION;
/// The item bit, asked by the bag and paper-doll click seams (`TargetingWantsItem 0x6e6330`).
const TF_ITEM: u16 = 0x0010;
/// `TARGET_FLAG_LOCKED`: in both the item mask `0x4010` and the GameObject mask `0x4800`, so one
/// lock spell ends on a bag item or a world object. It never reaches the wire: `BindTarget`
/// writes ITEM (`0x6e5f2e`) or GAMEOBJECT (`0x6e5f69`) instead, which vmangos accepts for a
/// lock spell (`Spell.cpp:6755`).
const TF_LOCKED: u16 = 0x4000;
/// `TARGET_FLAG_GAMEOBJECT`, the other bit of `TargetingWantsGameObject 0x6e62d0`'s `0x4800`.
const TF_GAMEOBJECT: u16 = 0x0800;
/// The bits `BindTarget`'s item arm admits and clears (`0x6e5f1e`, `0x6e5f36`).
const ITEM_ARM_BITS: u16 = TF_ITEM | TF_LOCKED;

/// What the wire's target block should carry for this cast.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CastWireTarget {
    /// Word 0: mask 0, no guid; the server resolves the target implicitly.
    SelfImplicit,
    /// Mask `TARGET_FLAG_UNIT` (0x2) and this guid, possibly the player's own.
    Unit(u64),
    /// Mask `TARGET_FLAG_ITEM` (0x10) and this item guid, sent on the press: only the main-hand
    /// imbue binds an item from the cast itself; every other item target comes from a click.
    Item(u64),
    /// No send yet: enter the targeting cursor with this flag word (reference: word != 0 is
    /// `IsTargeting 0x6e48a0`). Each click seam tests the word with its own mask, so one word can
    /// serve several: Opening and Pick Lock take a bag item or a world GameObject.
    Targeting(u16),
    /// Do not send; show this client error.
    Refused(u8),
    /// A refusal the reference raises in its cast tail after `ArmCast` returns false (`0x6e5045`).
    /// That tail runs after the requirement validator `0x6094f0`, so it fires at the cursor-entry
    /// point in [`super::send_spell_cast`], not at the resolver's return.
    RefusedAtArm(u8),
}

/// Where `ArmCast 0x6e5250` takes its candidate guid from (`0x6e5361`-`0x6e539f`). The main hand
/// is taken instead of the selection, never before it; `caster` is the separate `autoSelfCast`
/// fallback (`0x6e53d7`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CastCandidates {
    /// The current selection (the client's `0xb4e2d8`); benilla never passes an explicit guid.
    pub(crate) selection: Option<u64>,
    pub(crate) caster: Option<u64>,
    /// The equipped main hand's item guid (`[player+0x1d3c][15]`), only when it resolves to a
    /// streamed item object (`0x468460` at `0x6e53bc`).
    pub(crate) main_hand_item: Option<u64>,
}

/// What the binder's relation checks read: the selected unit's store and the player's.
#[derive(Clone, Copy)]
pub(crate) struct TargetRelations<'a> {
    pub(crate) target_store: Option<&'a ObjectStore>,
    /// The target's charmer, creator or summoner store, for `CanAssist`'s `IsPvP` owner chase.
    pub(crate) target_owner_store: Option<&'a ObjectStore>,
    pub(crate) self_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
}

/// The targeting inputs [`super::send_spell_cast`] resolves with, built by both cast callers.
pub(crate) struct CastContext<'a> {
    pub(crate) selection_guid: Option<u64>,
    pub(crate) self_guid: Option<u64>,
    pub(crate) auto_self_cast: bool,
    pub(crate) rel: TargetRelations<'a>,
    /// The local range gate's inputs (`IsTargetInRange 0x6e47b0`).
    pub(crate) range: RangeInputs,
    /// The main hand's item guid off the player descriptor (`PLAYER_FIELD_INV_SLOT_HEAD + 2*15`);
    /// the send path checks that it names a streamed item.
    pub(crate) main_hand_item: Option<u64>,
    /// The caster's movement flags (the client's `[unit+0x9e8]`), read by the requirement
    /// validator's moving gate.
    pub(crate) self_move_flags: u32,
}

/// Positions and combat reaches for the pre-send range refusal: the client's cast runs
/// `CanTargetUnit 0x6e4440` and `IsTargetInRange 0x6e47b0` before the commit, so an out-of-range
/// press refuses locally and none of the commit tail runs.
#[derive(Clone, Copy)]
pub(crate) struct RangeInputs {
    pub(crate) self_pos: Option<Vec3>,
    pub(crate) target_pos: Option<Vec3>,
    pub(crate) self_reach: f32,
    pub(crate) target_reach: Option<f32>,
}

impl Default for RangeInputs {
    fn default() -> Self {
        Self {
            self_pos: None,
            target_pos: None,
            // The descriptor's default combat reach.
            self_reach: 1.5,
            target_reach: None,
        }
    }
}

/// Everything a cast-sending system needs to build a [`CastContext`].
#[derive(SystemParam)]
pub(crate) struct CastTargeting<'w, 's> {
    pub(crate) selection: Res<'w, Selection>,
    pub(crate) self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Option<Res<'w, GuidIndex>>,
    self_guid: Res<'w, SelfGuid>,
    auto_self_cast: Res<'w, AutoSelfCast>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    self_transform: Query<'w, 's, &'static Transform, With<SelfPlayer>>,
    transforms: Query<'w, 's, &'static Transform>,
    /// `Option`: the body exists only in world, and the cast-result handler must fetch this param
    /// anywhere. With no body the moving gate passes and the server decides.
    player: Option<Res<'w, crate::player::Player>>,
}

impl CastTargeting<'_, '_> {
    /// This frame's [`CastContext`].
    pub(crate) fn context(&self) -> CastContext<'_> {
        let target_store = self.selection.target.and_then(|e| self.stores.get(e).ok());
        let target_owner_store = target_store
            .and_then(|store| {
                store
                    .0
                    .unit_owner(benilla_protocol::messages::OwnerFallback::CreatedBy)
            })
            .and_then(|guid| self.index.as_ref()?.0.get(&guid).copied())
            .and_then(|entity| self.stores.get(entity).ok());
        CastContext {
            selection_guid: self.selection.guid,
            self_guid: self.self_guid.0,
            auto_self_cast: self.auto_self_cast.0,
            rel: TargetRelations {
                target_store,
                target_owner_store,
                self_store: self.self_store.iter().next(),
                factions: self.factions.as_deref(),
                reputations: &self.reputations,
            },
            range: RangeInputs {
                self_pos: self.self_transform.iter().next().map(|t| t.translation),
                target_pos: self
                    .selection
                    .target
                    .and_then(|e| self.transforms.get(e).ok())
                    .map(|t| t.translation),
                self_reach: self
                    .self_store
                    .iter()
                    .next()
                    .map_or(1.5, |s| s.0.unit_combat_reach()),
                target_reach: target_store.map(|s| s.0.unit_combat_reach()),
            },
            main_hand_item: self
                .self_store
                .iter()
                .next()
                .and_then(|s| s.0.player_inv_slot(EQUIPMENT_SLOT_MAINHAND))
                .filter(|&g| g != 0),
            self_move_flags: self.player.as_deref().map_or(0, |p| p.move_flags()),
        }
    }
}

/// `EQUIPMENT_SLOT_MAINHAND` (the client reads it at `0x6e538b`, offset `0x78 == 15 * 8`).
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;

/// The `autoSelfCast` CVar (name `0x870dc0`, read at `0x6e53d7`; reference default `"0"`).
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct AutoSelfCast(pub(crate) bool);

/// `autoSelfCast`'s change callback: a flag.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut auto: ResMut<AutoSelfCast>) {
    if ev.is("autoSelfCast") {
        auto.0 = ev.flag();
    }
}

/// The flag word: `Targets`, then one arm keyed on `EffectImplicitTargetA[0]` (`0x6e525a`-
/// `0x6e52ef`); every implicit target not matched below leaves the word as it is.
pub(crate) fn cast_target_mask(def: &SpellDisplay) -> u16 {
    let mut word = def.targets as u16;
    match def.implicit_target_a1 {
        1 => word &= !TF_EXPLICIT_GATE,
        5 => word &= !TF_CORPSE_ALLY,
        6 | 53 => word |= TF_UNIT_ENEMY,
        // 16 (ground target) sets a cursor-mode flag, not a word bit; [`resolve_cast_target`]
        // tests it.
        21 | 45 => word |= TF_UNIT_ASSIST,
        23 => word |= 0x0800,
        25 | 63 => word |= TF_UNIT,
        26 => word |= 0x4000,
        35 => word |= TF_UNIT_PARTY,
        57 | 61 => word |= TF_UNIT_RAID,
        _ => {}
    }
    word
}

/// `BindTarget 0x6e5b40`'s unit branch: clear every flag-word bit the candidate satisfies.
///
/// The assist bit asks `CanAssist 0x6066f0`: selectable, friendly or better, and an NPC's owner
/// (or the NPC itself) PvP-enabled. This keeps friendly ambient NPCs and critters unbindable.
/// The enemy bit asks `CanAttack` (`0x606980`). Party and raid (`0x606c20`, `0x606d20`) accept
/// only the player; the corpse check (`0x6067d0`) is assistable with health 0 here.
fn clear_satisfied_bits(word: u16, is_self: bool, rel: &TargetRelations) -> u16 {
    let mut word = word;
    let assist = is_self
        || can_assist(
            rel.target_store,
            rel.factions,
            rel.reputations,
            rel.self_store,
            |_| rel.target_owner_store.cloned(),
        );
    let dead = rel
        .target_store
        .is_some_and(|s| s.0.unit_health() == Some(0));
    if word & TF_UNIT_PARTY != 0 && is_self {
        word &= !TF_UNIT_PARTY;
    }
    if word & TF_UNIT_RAID != 0 && is_self {
        word &= !TF_UNIT_RAID;
    }
    if word & TF_UNIT_ASSIST != 0 && assist {
        word &= !TF_UNIT_ASSIST;
    }
    if word & TF_UNIT_ENEMY != 0
        && !is_self
        && can_attack(
            rel.target_store,
            rel.factions,
            rel.reputations,
            rel.self_store,
        )
    {
        word &= !TF_UNIT_ENEMY;
    }
    // Bit 1 has no relation check: any unit binds. Bit 10 is cleared by any candidate but self.
    if word & TF_UNIT != 0 {
        word &= !TF_UNIT;
    }
    if word & TF_EXPLICIT_GATE != 0 && !is_self {
        word &= !TF_EXPLICIT_GATE;
    }
    if word & TF_CORPSE_ALLY != 0 && assist && dead {
        word &= !TF_CORPSE_ALLY;
    }
    if word & TF_CORPSE_ENEMY != 0 && !is_self && dead {
        word &= !TF_CORPSE_ENEMY;
    }
    word
}

/// Whether `BindTarget 0x6e5b40`'s unit arm clears the whole standing word for this unit.
/// The initial selection, a world click and `SpellTargetUnit` all use this one predicate.
pub(super) fn unit_word_binds(word: u16, is_self: bool, rel: &TargetRelations) -> bool {
    word & UNIT_BITS != 0 && clear_satisfied_bits(word, is_self, rel) == 0
}

/// The wire target for casting `def`. An unknown spell sends the selection as is, or no target
/// without one, and the server validates.
pub(crate) fn resolve_cast_target(
    def: Option<&SpellDisplay>,
    cand: &CastCandidates,
    auto_self_cast: bool,
    rel: &TargetRelations,
) -> CastWireTarget {
    let Some(def) = def else {
        return match cand.selection {
            Some(guid) => CastWireTarget::Unit(guid),
            None => CastWireTarget::SelfImplicit,
        };
    };
    let word = cast_target_mask(def);
    if word == 0 {
        return CastWireTarget::SelfImplicit;
    }
    // Implicit target 16 enters the cursor before any candidate bind (`0x6e535b`), but after the
    // word-0 commit above (`0x6e5338` comes first).
    if def.implicit_target_a1 == 16 {
        // The reference flags this outside the word; the click seams read only the word, so the
        // DEST bit carries it (shipped arm-16 rows already have it in `Targets`).
        return CastWireTarget::Targeting(word | TF_DEST_LOCATION);
    }
    // `Attributes & 0x200` takes the main hand as the candidate (`0x6e5361`-`0x6e5391`). This
    // must precede the non-unit cursor defer below: an item candidate clears bit 4.
    if def.targets_main_hand_item() {
        match cand.main_hand_item {
            // `BindTarget`'s item arm clears bits 4 and 14 (`0x6e5f35`) and the cleared word
            // commits at once (`0x6e60c1`).
            Some(item) if word & ITEM_ARM_BITS != 0 && word & !ITEM_ARM_BITS == 0 => {
                return CastWireTarget::Item(item)
            }
            // Empty hand: not a cursor. The cast tail re-tests the attribute (`0x6e504b`), clears
            // the word (`0x6e5057`) and raises 0x2d, jumping over the cursor arm (`0x6e50ab`).
            None => return CastWireTarget::RefusedAtArm(ERR_MAINHAND_EMPTY),
            // A residual the item arm cannot clear waits at the cursor in the reference; no
            // shipped row has one (all 22 carry `Targets == 0x10`).
            Some(_) => {}
        }
    }
    // A non-unit bit enters the cursor with the whole word. The reference only reaches the cursor
    // after the unit walk fails (`0x6e50c8`), so deferring first matches it only because no unit
    // candidate clears these bits; a location word with a unit bit beside it (0x42, NPC-cast
    // spell 28218) binds the unit first in the reference, so it refuses here instead.
    if word & !UNIT_BITS != 0 {
        if word & LOCATION_BITS != 0 && word & UNIT_BITS == 0
            || word & (TF_ITEM | TF_LOCKED | TF_GAMEOBJECT) != 0
        {
            return CastWireTarget::Targeting(word);
        }
        return CastWireTarget::Refused(ERR_INVALID_TARGET);
    }
    // The selection (`0xb4e2d8`, `0x6e539f`).
    if let Some(guid) = cand.selection {
        let is_self = cand.caster == Some(guid);
        if unit_word_binds(word, is_self, rel) {
            return CastWireTarget::Unit(guid);
        }
    }
    // The fallback: the active player (`0x6e53d7`), behind autoSelfCast.
    if auto_self_cast {
        if let Some(guid) = cand.caster {
            let self_rel = TargetRelations {
                target_store: rel.self_store,
                ..*rel
            };
            if unit_word_binds(word, true, &self_rel) {
                return CastWireTarget::Unit(guid);
            }
        }
    }
    // A standing enemy word refuses locally. Every other residual unit word proceeds to
    // `BindTarget`'s hand cursor.
    if word & TF_UNIT_ENEMY != 0 {
        CastWireTarget::Refused(if cand.selection.is_some() {
            ERR_INVALID_TARGET
        } else {
            ERR_NO_TARGET
        })
    } else {
        CastWireTarget::Targeting(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selection and caster candidates, with an empty main hand.
    fn cands(selection: Option<u64>, caster: Option<u64>) -> CastCandidates {
        CastCandidates {
            selection,
            caster,
            main_hand_item: None,
        }
    }

    fn spell(targets: u32, implicit: u32) -> SpellDisplay {
        SpellDisplay {
            targets,
            implicit_target_a1: implicit,
            ..Default::default()
        }
    }

    #[test]
    fn auto_self_cast_boots_at_the_reference_default() {
        assert!(
            !AutoSelfCast::default().0,
            "autoSelfCast defaults to the reference's disabled state"
        );
    }

    #[test]
    fn mask_follows_the_arm_map() {
        assert_eq!(cast_target_mask(&spell(0, 1)), 0, "Ice Armor: self");
        assert_eq!(
            cast_target_mask(&spell(0, 20)),
            0,
            "Battle Shout: no-op arm"
        );
        assert_eq!(cast_target_mask(&spell(0, 6)), TF_UNIT_ENEMY, "Fireball");
        assert_eq!(
            cast_target_mask(&spell(0, 21)),
            TF_UNIT_ASSIST,
            "Arcane Intellect"
        );
        assert_eq!(
            cast_target_mask(&spell(0x8000, 21)),
            TF_CORPSE_ALLY | TF_UNIT_ASSIST,
            "a Targets seed ORs with the switch"
        );
        assert_eq!(
            cast_target_mask(&spell(0x402, 0)),
            TF_UNIT | TF_EXPLICIT_GATE,
            "Skinning: seed only"
        );
    }

    /// Both stores are present because `CanAttack` refuses outright when either side is missing.
    #[test]
    fn resolution_wire_shapes() {
        use benilla_protocol::ObjectFields;
        // `UNIT_FIELD_FLAGS` bit 3 (player-controlled) on us alone selects `CanAttack`'s mixed
        // arm; with no catalog the reaction resolves neutral, so the enemy bit clears.
        let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(46, 1 << 3)]));
        let it = crate::net::ObjectStore(ObjectFields::from_pairs(&[(35, 0)]));
        let rel = TargetRelations {
            target_store: Some(&it),
            target_owner_store: None,
            self_store: Some(&me),
            factions: None,
            reputations: &Reputations(Vec::new()),
        };
        let ice_armor = spell(0, 1);
        assert_eq!(
            resolve_cast_target(Some(&ice_armor), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::SelfImplicit,
            "a self spell ignores the selection entirely"
        );
        let fireball = spell(0, 6);
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(None, Some(1)), true, &rel),
            CastWireTarget::Refused(ERR_NO_TARGET),
            "enemy words retain the no-target refusal"
        );
        // Neutral (3) is attackable by the mixed arm's `< 4` and not assistable by `>= 4`.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
        let intellect = spell(0, 21);
        assert!(
            !unit_word_binds(TF_UNIT_ASSIST, false, &rel),
            "the cursor's unit binder must reject the same neutral unit as the initial cast arm"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "a friendly-required cast on a non-friend falls back to self"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), false, &rel),
            CastWireTarget::Targeting(TF_UNIT_ASSIST),
            "autoSelfCast off leaves the friendly word for the unit cursor"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(None, Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "no selection at all still self-falls-back"
        );
        // A hostile-required cast never self-binds.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(None, Some(1)), false, &rel),
            CastWireTarget::Refused(ERR_NO_TARGET)
        );
        // Unknown spell: the selection passes through.
        assert_eq!(
            resolve_cast_target(None, &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
    }

    #[test]
    fn assist_word_requires_the_npc_pvp_flag_not_only_a_friendly_reaction() {
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("FactionTemplate.dbc"),
        );
        // Faction templates 35 and 1 are a real friendly pair. Field 46 is UNIT_FIELD_FLAGS.
        let me = ObjectStore(ObjectFields::from_pairs(&[(35, 1), (46, 0x8)]));
        let quiet = ObjectStore(ObjectFields::from_pairs(&[(35, 35), (46, 0)]));
        let pvp = ObjectStore(ObjectFields::from_pairs(&[(35, 35), (46, 0x1000)]));
        let reputations = Reputations(Vec::new());
        let rel = |target| TargetRelations {
            target_store: Some(target),
            target_owner_store: None,
            self_store: Some(&me),
            factions: Some(&factions),
            reputations: &reputations,
        };

        assert!(
            !unit_word_binds(TF_UNIT_ASSIST, false, &rel(&quiet)),
            "a friendly ambient NPC is not assistable"
        );
        assert!(
            unit_word_binds(TF_UNIT_ASSIST, false, &rel(&pvp)),
            "the same friendly NPC becomes assistable with UNIT_FLAG_PVP"
        );
    }

    /// Flamestrike (implicit 16) defers even with a selection, Blizzard (implicit 28) by its
    /// bare DEST word; a word of 0 still commits first (`0x6e5338`).
    #[test]
    fn ground_masks_enter_targeting_mode() {
        let flamestrike = spell(0x40, 16);
        assert_eq!(
            resolve_cast_target(
                Some(&flamestrike),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::Targeting(TF_DEST_LOCATION)
        );
        let blizzard = spell(0x40, 28);
        assert_eq!(
            resolve_cast_target(Some(&blizzard), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(TF_DEST_LOCATION)
        );
        let self_commit_with_ground_arm = spell(0, 16);
        assert_eq!(
            resolve_cast_target(
                Some(&self_commit_with_ground_arm),
                &cands(None, Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::SelfImplicit,
            "word==0 commits before the arm-16 defer — the reference's order"
        );
    }

    /// A bare SOURCE word raises the cursor like DEST (`0x6e6320` tests `& 0x60`); spell 265,
    /// `Targets 0x20` with implicit 15, is the shipped case.
    #[test]
    fn the_source_location_word_enters_targeting_mode() {
        let area_death = spell(0x20, 15);
        assert_eq!(
            resolve_cast_target(
                Some(&area_death),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::Targeting(TF_SOURCE_LOCATION)
        );
        assert_eq!(
            resolve_cast_target(Some(&area_death), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(TF_SOURCE_LOCATION),
            "and with nothing selected — autoSelfCast has no location to give either"
        );
        // Both bits, which `BindLocation` takes over two clicks; no shipped row carries it.
        let both = spell(0x60, 0);
        assert_eq!(
            resolve_cast_target(Some(&both), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(LOCATION_BITS)
        );
    }

    /// STRING (bit 13) has no seam; `0x42` (spell 28218) would bind its unit first in the
    /// reference.
    #[test]
    fn non_unit_masks_refuse() {
        for targets in [0x2000u32, 0x42] {
            let s = spell(targets, 0);
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(Some(42), Some(1)), true, &rel_none()),
                CastWireTarget::Refused(ERR_INVALID_TARGET),
                "Targets {targets:#x} must stay refused"
            );
        }
    }

    /// 100 of the 103 LOCKED rows carry implicit arm 23 (word `0x4800`) and one arm 25
    /// (`0x4002`): the seam tests are masks, so each raises the cursor with its whole word.
    #[test]
    fn the_seam_predicates_are_masks_and_the_word_travels() {
        for (targets, implicit) in [
            (0x10u32, 0),
            (0x4000, 0),
            (0x4000, 23),
            (0x4000, 25),
            (0x0, 23),
        ] {
            let s = spell(targets, implicit);
            let word = cast_target_mask(&s);
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(Some(42), Some(1)), true, &rel_none()),
                CastWireTarget::Targeting(word),
                "Targets {targets:#x} + implicit arm {implicit} must raise the cursor with its word"
            );
            assert_eq!(
                resolve_cast_target(Some(&s), &cands(None, Some(1)), false, &rel_none()),
                CastWireTarget::Targeting(word),
                "no selection and no autoSelfCast change nothing — no unit clears these bits"
            );
        }
        // The shipped lock word carries both seams' bits.
        let opening = cast_target_mask(&spell(0x4000, 23));
        assert_eq!(opening & (TF_ITEM | TF_LOCKED), TF_LOCKED);
        assert_eq!(opening & (TF_GAMEOBJECT | TF_LOCKED), opening);
    }

    /// Rockbiter Weapon 8017 (`Targets 0x10`, `Attributes 0x50200`, implicit 0) binds the main
    /// hand whatever the selection; an empty hand refuses; the same word without the attribute
    /// raises the item cursor.
    #[test]
    fn the_imbue_attribute_binds_the_main_hand_without_a_cursor() {
        let rockbiter = SpellDisplay {
            targets: 0x10,
            attributes: 0x0005_0200,
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            ..Default::default()
        };
        assert!(rockbiter.targets_main_hand_item());
        const WEAPON: u64 = 0xdead_beef;
        let armed = CastCandidates {
            selection: Some(42),
            caster: Some(1),
            main_hand_item: Some(WEAPON),
        };
        assert_eq!(
            resolve_cast_target(Some(&rockbiter), &armed, true, &rel_none()),
            CastWireTarget::Item(WEAPON),
            "the imbue binds the equipped weapon on the press"
        );
        assert_eq!(
            resolve_cast_target(
                Some(&rockbiter),
                &CastCandidates {
                    selection: None,
                    ..armed
                },
                false,
                &rel_none()
            ),
            CastWireTarget::Item(WEAPON),
            "neither the selection nor autoSelfCast has anything to do with it"
        );
        // Unarmed: the cast tail's second test of the bit (`0x6e504b`) refuses, skipping the
        // cursor arm (`0x6e50ab`).
        assert_eq!(
            resolve_cast_target(
                Some(&rockbiter),
                &cands(Some(42), Some(1)),
                true,
                &rel_none()
            ),
            CastWireTarget::RefusedAtArm(ERR_MAINHAND_EMPTY),
            "no main hand — \"Your weapon hand is empty\", never the item cursor"
        );
        // A stone, poison, oil or enchant: the same word without the attribute.
        let stone = SpellDisplay {
            targets: 0x10,
            attributes: 0x0004_0000,
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            ..Default::default()
        };
        assert!(!stone.targets_main_hand_item());
        assert_eq!(
            resolve_cast_target(Some(&stone), &armed, true, &rel_none()),
            CastWireTarget::Targeting(TF_ITEM),
            "without the attribute the cursor comes up even with a weapon equipped"
        );
    }

    fn rel_none() -> TargetRelations<'static> {
        static EMPTY: Reputations = Reputations(Vec::new());
        TargetRelations {
            target_store: None,
            target_owner_store: None,
            self_store: None,
            factions: None,
            reputations: &EMPTY,
        }
    }
}
