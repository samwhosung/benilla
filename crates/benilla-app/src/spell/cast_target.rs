//! Cast target resolution: what goes in `CMSG_CAST_SPELL`'s target block (reference: `ArmCast
//! 0x6e5250`, `BindTarget 0x6e5b40`). The flag word is seeded from `Spell.dbc` `Targets` and
//! adjusted by the implicit-target switch (jump table `0x6e5484`), then:
//!
//! - word 0: commit at once with mask 0 and no guid, never the selection; the server fills the
//!   target from the spell's implicit targeting.
//! - `Attributes & 0x200`: the candidate is the equipped main hand (`0x6e5361`), bound as an item
//!   with no cursor; an empty hand refuses with "Your weapon hand is empty" (`0x6e504b`).
//! - otherwise the selection meets `BindTarget`'s gates ([`bind_gates`]), then its relation arms,
//!   the first that holds clearing its bit; then the player behind `autoSelfCast` (`0x6e53d7`) does
//!   the same. Only a word the arm leaves empty commits, as a unit guid.
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

use super::bind_gates::{bind_gates, unit_alive};
use super::group_relation::{GroupInputs, GroupRoster};
use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::target::{Factions, RelUnit, Relations, Selection, UnitLookup};
use benilla_protocol::messages::OwnerFallback;

/// `TARGET_FLAG_*` bits of the client's targeting flag word (`0xcecac0`).
const TF_UNIT: u16 = 0x0002;
const TF_UNIT_RAID: u16 = 0x0004;
const TF_UNIT_PARTY: u16 = 0x0008;
const TF_UNIT_ENEMY: u16 = 0x0080;
const TF_UNIT_ASSIST: u16 = 0x0100;
const TF_CORPSE_ENEMY: u16 = 0x0200;
pub(super) const TF_EXPLICIT_GATE: u16 = 0x0400;
const TF_CORPSE_ALLY: u16 = 0x8000;
/// The corpse words, `0x8600`: `BindTarget`'s dead-unit gate lets a dead unit through for any of
/// them (`6e5c7a`).
pub(super) const CORPSE_WORD_BITS: u16 = TF_CORPSE_ENEMY | TF_EXPLICIT_GATE | TF_CORPSE_ALLY;
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
    /// A refusal the reference raises in its cast tail after `ArmCast` returns false (`0x6e5045`):
    /// the empty weapon hand's (`0x6e504b`) and the standing enemy word's (`0x6e507b`).
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

/// What the binder's gates and relation checks read: the selected unit's store and the player's.
#[derive(Clone, Copy)]
pub(crate) struct TargetRelations<'a> {
    pub(crate) target_store: Option<&'a ObjectStore>,
    /// The target's charmer, creator or summoner store, for `CanAssist`'s `IsPvP` owner chase.
    pub(crate) target_owner_store: Option<&'a ObjectStore>,
    pub(crate) self_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
    /// The creature-type gate's inputs, for the candidate and for the player alike.
    pub(crate) types: CreatureTypeSources<'a>,
    /// The party and raid predicates' inputs (`0x606c20`, `0x606d20`).
    pub(crate) group: GroupInputs<'a>,
}

/// A unit's owner as `CanAssist`'s `IsPvP` chase (`0x5ee5a0`) and `0x606170` read it, its charmer
/// else its creator: that object's descriptor, if it has streamed (`0x468460`).
pub(super) fn owner_store<'a>(
    store: Option<&ObjectStore>,
    index: Option<&GuidIndex>,
    stores: &'a Query<&ObjectStore>,
) -> Option<&'a ObjectStore> {
    let owner = store?.0.unit_owner(OwnerFallback::CreatedBy)?;
    stores.get(*index?.0.get(&owner)?).ok()
}

impl TargetRelations<'_> {
    /// `CanAssist 0x6066f0(caster, candidate)`, which the caster always is for himself. The
    /// binder's assist arm and the world pick's relation leg both ask it.
    pub(super) fn assistable(&self, is_self: bool) -> bool {
        is_self
            || self
                .pair()
                .is_some_and(|(me, target)| self.relations().can_assist(&me, &target))
    }

    /// `CanAttack 0x606980(caster, candidate)`, never the caster. The binder's enemy arm and the
    /// world pick's relation leg both ask it.
    pub(super) fn attackable(&self, is_self: bool) -> bool {
        !is_self
            && self
                .pair()
                .is_some_and(|(me, target)| self.relations().can_attack(&me, &target))
    }

    /// The relation functions over the binder's units: the caster, the candidate and their owners.
    pub(super) fn relations(&self) -> Relations<'_> {
        Relations {
            factions: self.factions,
            reputations: self.reputations,
            me: self.group.self_guid,
            roster: self.group.roster,
            units: self,
        }
    }

    /// The caster and the candidate as the relation functions read them; `None` until both
    /// have streamed and their guids are known.
    pub(super) fn pair(&self) -> Option<(RelUnit, RelUnit)> {
        Some((
            self.unit_of(self.group.self_guid?, self.self_store?),
            self.unit_of(self.group.target_guid?, self.target_store?),
        ))
    }

    fn unit_of(&self, guid: u64, store: &ObjectStore) -> RelUnit {
        let unit = RelUnit::of(guid, store);
        match self.types.names {
            Some(names) => unit.with_record(store, names),
            None => unit,
        }
    }
}

/// The object manager as the binder holds it: the caster, the candidate and the charmer or
/// creator of each, whichever have streamed.
impl UnitLookup for TargetRelations<'_> {
    fn unit(&self, guid: u64) -> Option<RelUnit> {
        let owner = |s: Option<&ObjectStore>| s?.0.unit_owner(OwnerFallback::CreatedBy);
        let held = [
            (self.group.self_guid, self.self_store),
            (self.group.target_guid, self.target_store),
            (owner(self.self_store), self.group.self_owner_store),
            (owner(self.target_store), self.target_owner_store),
        ];
        let store = held
            .into_iter()
            .find_map(|(g, store)| store.filter(|_| guid != 0 && g == Some(guid)))?;
        store.is_unit().then(|| self.unit_of(guid, store))
    }
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
    /// The caster's stand state as `GetStandState 0x5ed570` reads it for the active player, the
    /// validator's standing rung's input (`0x609cad`).
    pub(crate) self_stand_state: u8,
    /// The caster's sheathe state `[unit+0xd40]`, the validator's unsheathed rung's input
    /// (`0x609ba9`).
    pub(crate) self_sheathe_state: u8,
    /// `IsDaytime 0x60a050` on the game clock, the validator's day and night rungs' input.
    pub(crate) daytime: bool,
    /// `0x612cc0`, the caster's answer to the entity outdoors test `0x6706f0`, the validator's
    /// place rungs' input.
    pub(crate) self_outdoors: bool,
    /// `[0xb4b3e4]` is clear: the start of melee the cast's tail makes returns at it
    /// (`0x5ecbc0`).
    pub(crate) self_control_lost: bool,
}

impl CastContext<'_> {
    /// Applies the self-cast modifier, `onSelf`: the caster replaces the selection as the bind
    /// candidate, guid and store both, the pair the autoSelfCast fallback swaps (`0x6e53d7`). It
    /// is the explicit guid `CastSpell` and `CastSpellByName` hand `ArmCast 0x6e5250` (`0x4b4345`,
    /// `0x4b4afa`, bound at `0x6e5393`); where `UseAction 0x4e5ee0` applies its own is untraced.
    pub(crate) fn self_bound(mut self, on_self: bool) -> Self {
        if on_self {
            self.selection_guid = self.self_guid;
            self.rel.target_store = self.rel.self_store;
            self.rel.group.target_guid = self.self_guid;
        }
        self
    }
}

/// Positions and the two units for the pre-send range refusal: the client's cast runs
/// `CanTargetUnit 0x6e4440` and `IsTargetInRange 0x6e47b0` before the commit, so an out-of-range
/// press refuses locally and none of the commit tail runs.
#[derive(Clone, Copy)]
pub(crate) struct RangeInputs {
    pub(crate) self_pos: Option<Vec3>,
    pub(crate) target_pos: Option<Vec3>,
    /// The caster's combat reach and motion.
    pub(crate) caster: benilla_formats::RangeUnit,
    /// The bound unit, which `GetMinMaxRange` reads for the ranged pad and the moving bonus.
    pub(crate) target: Option<benilla_formats::RangeUnit>,
}

impl RangeInputs {
    /// The one range compare: the squared 3D distance against `GetMinMaxRange 0x6e3480`'s bounds,
    /// "Out of range." (0x59) past max², "Target too close" (0x76) inside a nonzero min². The
    /// pre-send gate (`IsTargetInRange 0x6e47b0`) and the targeting cursor's unit leg (`0x6e6460`,
    /// `6e677c`–`6e6802`) both ask it. No row or no position passes; the server judges.
    pub(crate) fn refusal(
        &self,
        def: &SpellDisplay,
        row: Option<&benilla_formats::SpellRange>,
        mods: &super::SpellModifiers,
    ) -> Option<u8> {
        let dist_sq = self
            .self_pos
            .zip(self.target_pos)
            .map(|(a, b)| a.distance_squared(b));
        super::validator::cast_range_refusal(def, row, self.caster, self.target, dist_sq, mods)
    }
}

impl Default for RangeInputs {
    fn default() -> Self {
        Self {
            self_pos: None,
            target_pos: None,
            caster: benilla_formats::RangeUnit::still(super::DEFAULT_REACH),
            target: None,
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
    /// The party slots and raid roster as the frame began; a harness without them has no group.
    roster: Option<Res<'w, GroupRoster>>,
    /// The creature-type gate's template cache and form table; a harness without them resolves
    /// no type.
    names: Option<Res<'w, NameCache>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    self_transform: Query<'w, 's, &'static Transform, With<SelfPlayer>>,
    transforms: Query<'w, 's, &'static Transform>,
    /// `Option`: the body exists only in world, and the cast-result handler must fetch this param
    /// anywhere. With no body the moving gate passes and the server decides.
    player: Option<Res<'w, crate::player::Player>>,
    /// `[0xb4b3e4]`, for the cast tail's start of melee.
    own_control: Option<Res<'w, crate::player::OwnControlLost>>,
    /// The caster's and the selection's reach and motion, for the range gate.
    range_units: super::RangeUnits<'w, 's>,
    /// The caster's animation driver, which keeps its sheathe state.
    self_driver: Query<'w, 's, &'static crate::creature_anim::AnimDriver, With<SelfPlayer>>,
    /// The game clock the day and night rungs read; absent in a harness without one.
    clock: Option<Res<'w, crate::net::ServerTime>>,
    /// The player's answer to the entity outdoors test, which the place rungs read; absent
    /// outside the world.
    outdoors: Option<Res<'w, crate::player::PlayerOutdoors>>,
}

impl CastTargeting<'_, '_> {
    /// This frame's [`CastContext`].
    pub(crate) fn context(&self) -> CastContext<'_> {
        self.context_on(self.selection.target, self.selection.guid)
    }

    /// The context for an attempt passed a unit's guid (`0x6e4b60`'s last argument, as
    /// `InteractWithSpell 0x5f05e0` passes the body's): the range test (`0x6e4f26` → `0x6e4440`)
    /// and the arm's candidate (`0x6e5393`) take it in the selection's place. The arm's refusal of
    /// an enemy word with nothing bound picks its line off the selection itself (`0x6e50a3`), which
    /// this context no longer holds; Skinning's word (`Targets` 0x402, implicit target 25) has no
    /// enemy bit.
    pub(crate) fn context_at(&self, entity: Entity, guid: u64) -> CastContext<'_> {
        self.context_on(Some(entity), Some(guid))
    }

    fn context_on(&self, target: Option<Entity>, guid: Option<u64>) -> CastContext<'_> {
        let target_store = target.and_then(|e| self.stores.get(e).ok());
        let owner_store = |store| owner_store(store, self.index.as_deref(), &self.stores);
        let self_store = self.self_store.iter().next();
        CastContext {
            selection_guid: guid,
            self_guid: self.self_guid.0,
            auto_self_cast: self.auto_self_cast.0,
            rel: TargetRelations {
                target_store,
                target_owner_store: owner_store(target_store),
                self_store,
                factions: self.factions.as_deref(),
                reputations: &self.reputations,
                types: CreatureTypeSources {
                    names: self.names.as_deref(),
                    forms: self.spells.as_deref().map(|s| &s.forms),
                    ..Default::default()
                },
                group: GroupInputs {
                    self_guid: self.self_guid.0,
                    target_guid: guid,
                    self_owner_store: owner_store(self_store),
                    roster: self.roster.as_deref(),
                },
            },
            range: RangeInputs {
                self_pos: self.self_transform.iter().next().map(|t| t.translation),
                target_pos: target
                    .and_then(|e| self.transforms.get(e).ok())
                    .map(|t| t.translation),
                caster: self.range_units.caster(),
                target: target.and_then(|e| self.range_units.unit(e)),
            },
            main_hand_item: self
                .self_store
                .iter()
                .next()
                .and_then(|s| s.0.player_inv_slot(EQUIPMENT_SLOT_MAINHAND))
                .filter(|&g| g != 0),
            self_move_flags: self.player.as_deref().map_or(0, |p| p.move_flags()),
            self_stand_state: {
                let byte = self_store.map_or(0, |s| s.0.unit_stand_state());
                self.player.as_deref().map_or(byte, |p| p.stand_state(byte))
            },
            // The cached state, seeded from `UNIT_FIELD_BYTES_2` byte 0 and moved by the local
            // setter `0x611cf0` ahead of its echo; the descriptor's byte until it is driven.
            self_sheathe_state: self
                .self_driver
                .iter()
                .next()
                .and_then(|d| d.sheath_state())
                .or_else(|| self_store.and_then(|s| s.0.unit_sheath_state()))
                .unwrap_or(0),
            daytime: self.clock.as_deref().is_some_and(|c| c.0.is_daytime()),
            self_outdoors: self.outdoors.as_deref().is_none_or(|o| o.0),
            self_control_lost: self.own_control.as_deref().is_some_and(|c| c.0),
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

/// `UNIT_FIELD_FLAGS & 0x100` (vmangos `UNIT_FLAG_IMMUNE_TO_PLAYER`, `UnitDefines.h:553`), which
/// refuses the `0x2` arm (`0x6e5df2`-`0x6e5e03`).
const UNIT_FLAG_IMMUNE_TO_PLAYER: u32 = 0x0000_0100;

/// `BindTarget 0x6e5b40`'s unit branch past the gates, one else-if chain (`0x6e5cad`-`0x6e5ee7`)
/// that `SpellCanTargetObject 0x6e6460` asks in the same order (`0x6e65b1`-`0x6e66dc`). The first
/// arm whose test holds clears its own bit, and on a dead unit `0x400`, or `0x8600` for the `0x2`
/// arm, and the word left is returned; `None` when no arm holds, and when the `0x8000` arm's
/// `CanAssist` refuses, which ends the walk (`0x6e5e7f`).
///
/// | order | bit | the arm's test |
/// |---|---|---|
/// | 1 | `0x8` | `0x606c20`, then `CanAssist 0x6066f0` |
/// | 2 | `0x4` | `0x606d20`, then `CanAssist` |
/// | 3 | `0x100` | `CanAssist` |
/// | 4 | `0x80` | `CanAttack 0x606980` |
/// | 5 | `0x2` | [`UNIT_FLAG_IMMUNE_TO_PLAYER`] clear |
/// | 6 | `0x8000` | dead, and a player or `UNIT_FIELD_PETNUMBER` set; then `CanAssist` |
/// | 7 | `0x200` | dead, a player, and `CanAssist` refused |
///
/// `CanAssist` asks selectable, friendly or better, and an NPC's owner (or the NPC itself)
/// PvP-enabled, which keeps friendly ambient NPCs and critters unbindable. The party test is the
/// player, a member of his party or a unit that answers to one; the raid test counts the raid
/// roster as well as the party slots. A unit with no streamed descriptor reads alive, no player
/// and no flags.
fn bind_arm(word: u16, is_self: bool, rel: &TargetRelations) -> Option<u16> {
    let fields = rel.target_store.map(|s| &s.0);
    let dead = fields.is_some_and(|f| !unit_alive(f));
    let player = fields
        .is_some_and(|f| f.object_type() == Some(benilla_protocol::messages::ObjectType::Player));
    let pet = fields.is_some_and(|f| f.unit_pet_number() != 0);
    let flags = fields.map_or(0, |f| f.unit_flags());
    // Arms 1 to 4 clear `0x400` beside their bit on a dead unit (`0x6e5ddb`, `0x6e5d89`).
    let bind = |bit: u16| Some(word & !(if dead { bit | TF_EXPLICIT_GATE } else { bit }));
    if word & TF_UNIT_PARTY != 0 && rel.in_party(is_self) && rel.assistable(is_self) {
        return bind(TF_UNIT_PARTY);
    }
    if word & TF_UNIT_RAID != 0 && rel.in_group(is_self) && rel.assistable(is_self) {
        return bind(TF_UNIT_RAID);
    }
    if word & TF_UNIT_ASSIST != 0 && rel.assistable(is_self) {
        return bind(TF_UNIT_ASSIST);
    }
    if word & TF_UNIT_ENEMY != 0 && rel.attackable(is_self) {
        return bind(TF_UNIT_ENEMY);
    }
    if word & TF_UNIT != 0 && flags & UNIT_FLAG_IMMUNE_TO_PLAYER == 0 {
        return Some(
            word & !(if dead {
                TF_UNIT | CORPSE_WORD_BITS
            } else {
                TF_UNIT
            }),
        );
    }
    if word & TF_CORPSE_ALLY != 0 && dead && (player || pet) {
        return rel.assistable(is_self).then_some(word & !TF_CORPSE_ALLY);
    }
    if word & TF_CORPSE_ENEMY != 0 && dead && player && !rel.assistable(is_self) {
        return Some(word & !TF_CORPSE_ENEMY);
    }
    None
}

/// Whether an arm binds this unit and leaves the standing word empty, which commits the cast
/// (`0x6e60c1`).
fn unit_word_binds(word: u16, is_self: bool, rel: &TargetRelations) -> bool {
    word & UNIT_BITS != 0 && bind_arm(word, is_self, rel) == Some(0)
}

/// Whether `BindTarget 0x6e5b40`'s unit branch binds this unit under the standing word: the gates
/// ([`bind_gates`]), then the relation checks. The initial selection, the `autoSelfCast` fallback,
/// a world click, `SpellTargetUnit` and the hover verdict all ask this one predicate. An unknown
/// spell has no row to gate on and leaves the relation checks alone.
pub(super) fn unit_binds(
    def: Option<&SpellDisplay>,
    word: u16,
    is_self: bool,
    rel: &TargetRelations,
) -> bool {
    def.is_none_or(|def| bind_gates(def, word, is_self, rel)) && unit_word_binds(word, is_self, rel)
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
    // The selection (`0xb4e2d8`, `0x6e539f`). A unit the gates or the relation checks refuse binds
    // nothing, so the fallback below gets its turn.
    if let Some(guid) = cand.selection {
        let is_self = cand.caster == Some(guid);
        if unit_binds(Some(def), word, is_self, rel) {
            return CastWireTarget::Unit(guid);
        }
    }
    // The fallback: the active player (`0x6e53d7`), behind autoSelfCast, through the same gates.
    if auto_self_cast {
        if let Some(guid) = cand.caster {
            let self_rel = TargetRelations {
                target_store: rel.self_store,
                target_owner_store: None,
                group: GroupInputs {
                    target_guid: Some(guid),
                    ..rel.group
                },
                ..*rel
            };
            if unit_binds(Some(def), word, true, &self_rel) {
                return CastWireTarget::Unit(guid);
            }
        }
    }
    // A standing enemy word is refused by the attempt's tail once the arm returns false
    // (`0x6e507b`-`0x6e50a9`): "Invalid target" with a selection, else "You have no target", with
    // the spell in flight. Every other residual unit word proceeds to `BindTarget`'s hand cursor.
    if word & TF_UNIT_ENEMY != 0 {
        CastWireTarget::RefusedAtArm(if cand.selection.is_some() {
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
        // arm; with no catalog the reaction resolves neutral, so the enemy bit clears. Both carry
        // `UNIT_FIELD_HEALTH` (22): a store with none reads dead to the gates.
        let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(22, 100), (46, 1 << 3)]));
        let it = crate::net::ObjectStore(ObjectFields::from_pairs(&[(22, 100), (35, 0)]));
        let rel = TargetRelations {
            target_store: Some(&it),
            target_owner_store: None,
            self_store: Some(&me),
            factions: None,
            reputations: &Reputations::default(),
            types: Default::default(),
            group: GroupInputs {
                self_guid: Some(1),
                target_guid: Some(42),
                ..Default::default()
            },
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
            CastWireTarget::RefusedAtArm(ERR_NO_TARGET),
            "enemy words retain the no-target refusal, the arm's"
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
            CastWireTarget::RefusedAtArm(ERR_NO_TARGET)
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
        let reputations = Reputations::default();
        let rel = |target| TargetRelations {
            target_store: Some(target),
            target_owner_store: None,
            self_store: Some(&me),
            factions: Some(&factions),
            reputations: &reputations,
            types: Default::default(),
            group: GroupInputs {
                self_guid: Some(1),
                target_guid: Some(2),
                ..Default::default()
            },
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

    /// A dead unit's bind clears the `0x400` bit whoever it is (`6e5d89`, `6e5ddb`); a living
    /// unit's never does, and the gate refuses it before the relation checks are asked.
    #[test]
    fn a_dead_units_bind_clears_the_0x400_bit() {
        use benilla_protocol::ObjectFields;

        let health = |hp| ObjectStore(ObjectFields::from_pairs(&[(22, hp)]));
        let (corpse, live) = (health(0), health(100));
        let rel = |target| TargetRelations {
            target_store: Some(target),
            ..rel_none()
        };
        for is_self in [false, true] {
            assert!(
                unit_word_binds(0x402, is_self, &rel(&corpse)),
                "a corpse, self {is_self}"
            );
            assert!(
                !unit_word_binds(0x402, is_self, &rel(&live)),
                "a living unit keeps the bit, self {is_self}"
            );
        }
    }

    /// The validator's caster reads: the stand state through `GetStandState 0x5ed570`, which with no
    /// local state is the descriptor's byte; the sheathe state `[unit+0xd40]` (`0x609ba9`), the
    /// descriptor's until the driver holds one; and `IsDaytime 0x60a050`, night with no clock.
    #[test]
    fn the_context_reads_the_posture_the_sheathe_and_the_clock() {
        use bevy::ecs::system::RunSystemOnce;
        let read = |stand: u32, sheathe: u32, hour: Option<i32>| {
            let mut world = World::new();
            world.init_resource::<crate::spell::AutoSelfCast>();
            world.insert_resource(Reputations::default());
            world.insert_resource(SelfGuid(Some(7)));
            world.insert_resource(Selection::default());
            if let Some(hour) = hour {
                let mut clock = crate::net::GameTime::new(0);
                let time = benilla_protocol::WowTime {
                    minute: 0,
                    hour,
                    weekday: 0,
                    day: 17,
                    month: 9,
                    year: 26,
                    spare: 0,
                };
                clock.set(time, 0);
                world.insert_resource(crate::net::ServerTime(clock));
            }
            // `UNIT_FIELD_BYTES_1` (138) byte 0 and `UNIT_FIELD_BYTES_2` (164) byte 0.
            let fields =
                benilla_protocol::ObjectFields::from_pairs(&[(138, stand), (164, sheathe)]);
            world.spawn((SelfPlayer, ObjectStore(fields)));
            world
                .run_system_once(|targeting: CastTargeting| {
                    let c = targeting.context();
                    (c.self_stand_state, c.self_sheathe_state, c.daytime)
                })
                .expect("the context builds")
        };
        assert_eq!(read(1, 0, Some(12)), (1, 0, true));
        assert_eq!(read(0, 2, Some(3)), (0, 2, false));
        assert_eq!(read(8, 1, None), (8, 1, false), "no clock reads night");
    }

    /// The place rungs' input is the player's answer to `0x6706f0`
    /// ([`crate::player::PlayerOutdoors`]); a world without it reads outdoors, as a null entity
    /// does (`0x6706fa`).
    #[test]
    fn the_context_reads_the_players_outdoors_answer() {
        use bevy::ecs::system::RunSystemOnce;
        let read = |outdoors: Option<bool>| {
            let mut world = World::new();
            world.init_resource::<crate::spell::AutoSelfCast>();
            world.insert_resource(Reputations::default());
            world.insert_resource(SelfGuid(Some(7)));
            world.insert_resource(Selection::default());
            if let Some(o) = outdoors {
                world.insert_resource(crate::player::PlayerOutdoors(o));
            }
            world.spawn((SelfPlayer, ObjectStore(Default::default())));
            world
                .run_system_once(|targeting: CastTargeting| targeting.context().self_outdoors)
                .expect("the context builds")
        };
        assert!(!read(Some(false)));
        assert!(read(Some(true)));
        assert!(read(None));
    }

    /// `BindTarget`'s unit branch is one else-if chain (`0x6e5cad`-`0x6e5ee7`): the first arm whose
    /// test holds clears its own bit, and on a dead unit `0x400`, or `0x8600` for the `0x2` arm;
    /// with none holding the unit binds nothing (`0x6e60dc`).
    #[test]
    fn the_unit_branch_binds_through_the_first_arm_that_holds() {
        use benilla_protocol::ObjectFields;
        // Fields 2 `OBJECT_FIELD_TYPE` (0x19, a player), 22 health, 46 `UNIT_FIELD_FLAGS`, 139
        // `UNIT_FIELD_PETNUMBER`.
        let unit = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
        let rel = |target| TargetRelations {
            target_store: Some(target),
            ..rel_none()
        };
        // The 0x2 arm: `UNIT_FLAG_IMMUNE_TO_PLAYER` refuses it (`0x6e5e03`), Dispel Magic at a
        // companion pet.
        let immune = unit(&[(22, 100), (46, 0x100)]);
        let plain = unit(&[(22, 100)]);
        assert!(!unit_word_binds(0x2, false, &rel(&immune)));
        assert!(unit_word_binds(0x2, false, &rel(&plain)));
        // Its dead leg clears 0x8600 (`0x6e5e33`): Revive's 0x8002 on a corpse nobody assists.
        let corpse = unit(&[(22, 0)]);
        assert!(unit_word_binds(0x8002, false, &rel(&corpse)));
        // A bare 0x400 has no arm.
        assert!(!unit_word_binds(0x400, false, &rel(&corpse)));
        // 0x8000: dead, and a player or a pet (`0x6e5e5c`-`0x6e5e72`), then `CanAssist`, so
        // Resurrection binds no dead guard.
        let dead_player = unit(&[(2, 0x19), (22, 0)]);
        let dead_pet = unit(&[(22, 0), (139, 3)]);
        assert!(!unit_word_binds(0x8000, true, &rel(&corpse)), "a creature");
        assert!(unit_word_binds(0x8000, true, &rel(&dead_player)));
        assert!(unit_word_binds(0x8000, true, &rel(&dead_pet)));
        // 0x200: dead, a player, and `CanAssist` refused (`0x6e5ead`-`0x6e5ee7`).
        assert!(unit_word_binds(0x200, false, &rel(&dead_player)));
        assert!(
            !unit_word_binds(0x200, false, &rel(&corpse)),
            "not a player"
        );
        assert!(
            !unit_word_binds(0x200, true, &rel(&dead_player)),
            "assistable"
        );
        // The 0x8000 arm's refusal ends the walk before 0x200 (`0x6e5e7f`).
        assert!(!unit_word_binds(0x8200, false, &rel(&dead_player)));
        // The first arm wins: the assist arm binds and leaves 0x2 standing, so nothing commits.
        let me = unit(&[(2, 0x19), (22, 100)]);
        assert_eq!(bind_arm(0x0102, true, &rel(&me)), Some(0x0002));
        assert!(!unit_word_binds(0x0102, true, &rel(&me)));
    }

    fn rel_none() -> TargetRelations<'static> {
        static EMPTY: Reputations = Reputations::from_slots(Vec::new());
        TargetRelations {
            target_store: None,
            target_owner_store: None,
            self_store: None,
            factions: None,
            reputations: &EMPTY,
            types: Default::default(),
            group: Default::default(),
        }
    }
}
