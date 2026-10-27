//! The world cursor's classifier: the hovered unit, GameObject or corpse resolves to a
//! [`CursorKind`] as the reference's `CGWorldFrame` classifier does (`0x4828d0`, unit branch
//! `0x482200`): the NPC service ladder, else loot, skin or attack by state, each grayed by its own
//! range gate. Loot rights never gray; they decide whether the loot cursor shows at all.

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};

use super::ring::Factions;
use super::{go_is_nearest, Hovered, HoveredObject};

/// The world cursor modes benilla shows, named by the reference's mode table (`0x853b8c`), whose
/// strings are the `Interface\Cursor\<Name>.blp` stems.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CursorKind {
    Point,
    Attack,
    Speak,
    /// Pickup(8): the vendor's pouch and the loot leg's base mode.
    Pickup,
    /// LootAll(16): the loot leg's triple pouch while the effective auto-loot is on; the vendor
    /// pouch never triples.
    LootAll,
    /// Interact(5): the gear, for a GameObject with no data-named cursor and for the innkeeper.
    Interact,
    Buy,
    /// Inspect(7): the UI's Ctrl-hover cursor (`ShowInspectCursor 0x48ac60`) and the world cursor
    /// over a readable TEXT(9) GameObject (`0x5f5890`).
    Inspect,
    Trainer,
    Taxi,
    Skin,
    /// Repair(17): never set by the world classifier; the UI's repair-mode base cursor
    /// (`ShowRepairCursor 0x4fbcc0`).
    Repair,
    /// Mail(15): a MAILBOX(19) or type-28 GameObject (`0x5f6840`, `0x5f6e30`).
    Mail,
    /// Mine(11): a GameObject whose lock's first `LockType` is Mining (3), named by LockType.dbc
    /// (`0x5f3070`).
    Mine,
    /// GatherHerbs(13): the same, for Herbalism (2).
    GatherHerbs,
    /// PickLock(14): the same, for Pick Lock (1); never grayed, since `0x5f3070` skips the usable
    /// gate for `LockType.Id == 1`.
    PickLock,
    /// Cast(2): the spell-targeting cursor (`0x4820f0`), which pre-empts the whole object
    /// classifier while a spell awaits a target; the classifier here never sets it.
    Cast,
    /// Custom(41): the texture FrameXML's `SetCursor` loaded by name (`0x523c40`), never set by
    /// the classifier; its picture is [`crate::cursor::CustomCursorName`]'s path.
    Custom,
}

impl CursorKind {
    /// The cursor's BLP stem in `Interface\Cursor\`.
    fn name(self) -> &'static str {
        match self {
            CursorKind::Point => "Point",
            CursorKind::Attack => "Attack",
            CursorKind::Speak => "Speak",
            CursorKind::Pickup => "Pickup",
            CursorKind::LootAll => "LootAll",
            CursorKind::Interact => "Interact",
            CursorKind::Buy => "Buy",
            CursorKind::Inspect => "Inspect",
            CursorKind::Trainer => "Trainer",
            CursorKind::Taxi => "Taxi",
            CursorKind::Skin => "Skin",
            CursorKind::Repair => "Repair",
            CursorKind::Mail => "Mail",
            CursorKind::Mine => "Mine",
            CursorKind::GatherHerbs => "GatherHerbs",
            CursorKind::PickLock => "PickLock",
            CursorKind::Cast => "Cast",
            // No `Interface\Cursor` stem: the platform drivers key it by its path.
            CursorKind::Custom => "Custom",
        }
    }
}

/// This frame's world cursor, written by [`classify_cursor`]; `unable` picks the grayed
/// `Unable<Name>` twin (the reference's `mode + 20`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct WorldCursor {
    pub(crate) kind: CursorKind,
    pub(crate) unable: bool,
}

impl Default for WorldCursor {
    fn default() -> Self {
        Self {
            kind: CursorKind::Point,
            unable: false,
        }
    }
}

impl WorldCursor {
    /// The BLP stem (`Attack`, `UnableAttack`), the key the platform cursor caches use.
    pub(crate) fn stem(&self) -> String {
        if self.unable && self.kind != CursorKind::Point {
            format!("Unable{}", self.kind.name())
        } else {
            self.kind.name().to_string()
        }
    }
}

/// `UNIT_NPC_FLAGS` bits, 1.12 values (vmangos `UnitDefines.h`); REPAIR (`0x4000`) is left out
/// because the ladder never tests it.
pub(crate) mod npc_flags {
    pub const GOSSIP: u32 = 0x1;
    pub const QUESTGIVER: u32 = 0x2;
    pub const VENDOR: u32 = 0x4;
    pub const FLIGHTMASTER: u32 = 0x8;
    pub const TRAINER: u32 = 0x10;
    pub const SPIRITHEALER: u32 = 0x20;
    pub const SPIRITGUIDE: u32 = 0x40;
    pub const INNKEEPER: u32 = 0x80;
    pub const BANKER: u32 = 0x100;
    pub const PETITIONER: u32 = 0x200;
    pub const TABARDDESIGNER: u32 = 0x400;
    pub const BATTLEMASTER: u32 = 0x800;
    pub const AUCTIONEER: u32 = 0x1000;
    pub const STABLEMASTER: u32 = 0x2000;
}

pub(super) const UNIT_FLAG_SKINNABLE: u32 = 0x0400_0000;
pub(crate) const UNIT_FLAG_PACIFIED: u32 = 0x0002_0000;

/// The hovered unit is one the unit dispatcher `0x60bea0` forks to its attack arm for (`0x60c192`):
/// alive or feigning, refused by `CanInteract` and passed by `CanAttack`. The sword also needs the
/// player's own legs ([`player_attack_legs`]), which the arm tests again inside.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AttackFork(pub(crate) bool);

/// The player's legs of the classifier's attack leg (`0x482648`–`0x4826a2`), in its order: not
/// dead or a ghost (`0x48264a`), not mounted (`0x482661`), not pacified (`0x48268f`) and in
/// control (`[0xb4b3e4]`, `0x48269b`). Any failure clears to Point (`0x4826cb`).
pub(crate) fn player_attack_legs(own: &ObjectStore, own_control_lost: bool) -> bool {
    !own.0.is_dead_or_ghost()
        && own.0.unit_mount_display_id() == 0
        && own.0.unit_flags() & UNIT_FLAG_PACIFIED == 0
        && !own_control_lost
}

/// NPC-service range, squared: gray beyond 5.5556 yd (`0xb4b32c` = `[0x804328]²`, checked
/// boundary-inclusive at `0x482320`). The NPC windows' walk-away close uses the same constant.
pub(crate) const SERVICE_RANGE_SQ: f32 = 30.864;
/// Attack's range, squared: gray beyond a fixed 10.45 yd (`0x80447c`, checked at `0x4826a7`), not
/// the melee reach.
const ATTACK_RANGE_SQ: f32 = 109.2025;
/// Melee interact reach `max(reachA + reachB + 1.333, 5.0)` (`0x80b058`, `0x80a1e8`): 5.0 is a
/// floor, so large creatures reach farther. Gates skin (`0x6e3480`) and loot (`CanLootNow
/// 0x5ec110`), center to center, boundary-inclusive.
pub(super) const MELEE_OFFSET: f32 = 1.333_33;
pub(super) const MELEE_FLOOR: f32 = 5.0;

pub(super) fn melee_reach(target: &ObjectStore, me: &ObjectStore) -> f32 {
    (target.0.unit_combat_reach() + me.0.unit_combat_reach() + MELEE_OFFSET).max(MELEE_FLOOR)
}

/// A corpse's interact reach, squared: a flat 5 yd, since a corpse has no combat reach and the
/// melee formula falls to its floor. `CanLootNow 0x5ec110` and the `CMSG_LOOT` sender `0x5df130`
/// both compare against 25.0.
pub(super) const CORPSE_INTERACT_RANGE_SQ: f32 = 25.0;

/// `GAMEOBJECT_TYPE_GENERIC`: decoration whose highlightable slot is constant false (`0x5f47f0`),
/// so it never shows an interact cursor.
pub(crate) const GO_TYPE_GENERIC: i32 = 5;
/// The transport family, TRANSPORT(11), MAP_OBJECT(14) and MO_TRANSPORT(15): both strategy slots
/// are constant false (vtables `0x80ba58`, `0x80b710`, `0x80b798`; `+0x14` is `0x5f5c70` or
/// `0x5f48b0`), so a boat or elevator never shows a cursor, a tooltip or a right-click use.
const GO_TYPE_TRANSPORT: i32 = 11;
const GO_TYPE_MAP_OBJECT: i32 = 14;
const GO_TYPE_MO_TRANSPORT: i32 = 15;
/// The marker set, SPELL_FOCUS(8), DUEL_ARBITER(16), FISHINGHOLE(25) and AURA_GENERATOR(30):
/// mouseover slot `+0xc` constant true, highlightable slot `+0x14` constant false (vtables
/// `0x80b8a8`, `0x80bb68`, `0x80bbf8`, `0x80c2a0`). An anvil or a duel flag tooltips and brightens
/// but shows the plain pointer and ignores the right-click (`OnUse 0x5f8660` asks `+0x14`).
const GO_TYPE_SPELL_FOCUS: i32 = 8;
const GO_TYPE_DUEL_ARBITER: i32 = 16;
const GO_TYPE_FISHINGHOLE: i32 = 25;
const GO_TYPE_AURA_GENERATOR: i32 = 30;
/// The highest type the type factory's jump table (`0x5f76cc`) covers; 21 GUARDPOST has no case
/// and takes `default:` with every out-of-range type.
const GO_TYPE_MAX: i32 = 30;
const GO_TYPE_GUARDPOST: i32 = 21;

/// Whether the type factory gives this type its `default:` strategy (`0x5f76a0`): a static
/// placeholder (vtable `0x80b188`) whose `+0x14` (`0x5f36f0`) and `+0xc` both answer false, so the
/// object shows nothing. No 1.12 data sends such a type; the flag predicate alone would pass it.
fn strategy_is_default(type_id: i32) -> bool {
    type_id == GO_TYPE_GUARDPOST || !(0..=GO_TYPE_MAX).contains(&type_id)
}
/// `GAMEOBJECT_TYPE_TEXT` (9), a readable book or plaque: the Inspect magnifier (`0x5f5890`).
pub(crate) const GO_TYPE_TEXT: i32 = 9;
/// MAILBOX(19) (`0x5f6840`) and type 28 (`0x5f6e30`) show Mail; no 1.12 data uses type 28.
/// RITUAL(18) shows Mail here, where the reference gives it the base cursor: its own vtable
/// `0x80bd10` keeps `0x5f3070`.
const GO_TYPE_RITUAL: i32 = 18;
pub(super) const GO_TYPE_MAILBOX: i32 = 19;
const GO_TYPE_28: i32 = 28;
/// `GAMEOBJECT_TYPE_FISHINGNODE` (17), the bobber: its strategy (vtable `0x80bc80`) overrides only
/// the highlightable slot.
pub(crate) const GO_TYPE_FISHINGNODE: i32 = 17;
/// `GAMEOBJECT_TYPE_CHAIR` (7): its own range predicate `0x5f5670` accepts within 3.0 yd
/// (`[0xc4d808]`, written only by `0x5f98b0` as 3.0 squared).
pub(crate) const GO_TYPE_CHAIR: i32 = 7;
/// `GAMEOBJECT_TYPE_QUESTGIVER` (2), TEXT (9), FLAGSTAND (24), FLAGDROP (26), MINIGAME (27): the
/// 5.5555553 yd range (`[0x80b07c]`, stored at `0x5f45c2`, `0x5f5852`, `0x5f6c12`, `0x5f6c72`,
/// `0x5f6cd2`).
const GO_RANGE_NEAR_SERVICE: f32 = 5.555_555_3;
/// The base handler's range (`[0x80b084]`, `0x5f2f67`, run first by every constructor).
const GO_RANGE_BASE: f32 = 5.0;
/// `GAMEOBJECT_TYPE_BINDER` (4): 10.0 yd (`[0x80b088]`, `0x5f478b`).
const GO_RANGE_BINDER: f32 = 10.0;
/// The chair's and the bobber's 100.0 yd (`[0x80b0b0]`, `0x5f562a`, `0x5f66d2`).
const GO_RANGE_FAR: f32 = 100.0;

/// A type's use range, `[handler+0xc]` (`0x5f2f50` stores 5.0 and nine types store another value
/// over it): the range `usable 0x5f3130` squares for its distance test and hands back. Type 21 and
/// any type above 30 take the static handler `0xc4d840`, which has none and whose slot 6
/// (`0x5f3700`) is never usable: 0.0.
const fn go_handler_range(type_id: i32) -> f32 {
    match type_id {
        2 | 9 | 24 | 26 | 27 => GO_RANGE_NEAR_SERVICE,
        4 => GO_RANGE_BINDER,
        GO_TYPE_CHAIR | GO_TYPE_FISHINGNODE => GO_RANGE_FAR,
        // AREADAMAGE stores a zeroed register (`0x5f4504`).
        12 => 0.0,
        0 | 1 | 3 | 5 | 6 | 8 | 10 | 11 | 13..=16 | 18..=20 | 22 | 23 | 25 | 28..=30 => {
            GO_RANGE_BASE
        }
        _ => 0.0,
    }
}

/// The range slot 6 hands back, which the walk to a too-far object stops short of (`0x5f86b0`):
/// `[handler+0xc]`, written before the opener spell's range (`0x5f32f4`) so that never reaches it.
/// The chair's replaces it with 3.0 (`0x5f569d`, `[0x80b0a0]`) only when its base test passed
/// (`0x5f5689 jne`); past that test it keeps the 100.0.
pub(super) fn go_use_range(type_id: i32, base_passed: bool) -> f32 {
    if type_id == GO_TYPE_CHAIR && base_passed {
        3.0
    } else {
        go_handler_range(type_id)
    }
}

/// The squared distance the base test of `usable 0x5f3130` allows (`0x5f3346`): the handler's
/// range, or the maximum range of the spell that opens the object's lock (`0x5f3328`).
pub(super) fn go_reach_sq(type_id: i32, opener_range: Option<f32>) -> f32 {
    let range = opener_range.unwrap_or_else(|| go_handler_range(type_id));
    range * range
}

/// The chair's seat test, 3.0 yd squared (`[0xc4d808]`, `0x5f571a`).
const CHAIR_SEAT_RANGE_SQ: f32 = 9.0;

/// Whether `dist_sq` passes `usable 0x5f3130`'s distance test, error 0xe1 otherwise: within
/// [`go_reach_sq`], equal and NaN passing (`0x5f336c`-`0x5f3379`). The chair then runs its own seat
/// test (`0x5f5670`), which is strict: `fcomp` at `0x5f571a` and `jnp` at `0x5f572b` succeed only
/// on C0 alone, so equal and NaN fail. Measured to the object, where the reference (seats from
/// `0x5f5760`) and vmangos (`GameObject.cpp:2231-2236`) measure to the nearest seat, so only a
/// multi-seat bench differs, by up to half its span.
pub(super) fn go_in_reach(dist_sq: f32, type_id: i32, opener_range: Option<f32>) -> bool {
    go_base_in_reach(dist_sq, type_id, opener_range)
        && (type_id != GO_TYPE_CHAIR || dist_sq < CHAIR_SEAT_RANGE_SQ)
}

/// The base test alone (`0x5f3346`), before the chair's seat test.
pub(super) fn go_base_in_reach(dist_sq: f32, type_id: i32, opener_range: Option<f32>) -> bool {
    dist_sq.is_nan() || dist_sq <= go_reach_sq(type_id, opener_range)
}

/// `usable 0x5f3130`'s state arm (`0x5f3392`-`0x5f33ac`), after the lock arm and ahead of the
/// distance test: without an opener spell, `GAMEOBJECT_STATE` 2 (`go+0x27c`) refuses with error
/// 0xe5, `ERR_USE_DESTROYED`. A spell that opens the lock skips it (`0x5f3315`).
pub(super) fn go_destroyed(state: u32, opener_spell: bool) -> bool {
    state == benilla_formats::GO_STATE_ACTIVE_ALTERNATIVE && !opener_spell
}

/// Why `usable 0x5f3130`'s player arms, ahead of its lock and range arms, or the door's own slot
/// ahead of them (`0x5f4360`), refuse a GameObject. The cursor grays on any (the type's slot 6 at
/// `0x5f3093`, the Inspect and Mail cursors' `0x5f58a8`, `0x5f6858`), and the click raises
/// [`PlayerRefusal::error_key`] (`0x5f874b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlayerRefusal {
    /// A door with autoClose resting opposite its start (`0x5f42f0`): error 0xe3.
    Moving,
    /// Dead or a ghost (`IsDeadOrGhost 0x605f30` at `0x5f315f`), for every type: error 0x7e.
    Dead,
    /// The player-control flag `[0xb4b3e4]` clear (`0x5f316e`): error 0x7f.
    LockedOut,
    /// Mounted (`0x5f31a8`) at an object without a `Lock.dbc` row (`0x5f8180`, a pointer test, so
    /// an all-empty row counts) that is no MAILBOX (`0x5f31bb`): error 0x19f.
    Mounted,
    /// Shapeshifted (`0x60e9f0` at `0x5f31e1`) into a form without flag `0x8` (`0x5f3210`), at an
    /// object without a lock row: error 0x1a0.
    Shapeshifted,
    /// `UNIT_FLAG_IN_COMBAT` (`0x5f3237`) at an object whose notInCombat column is set
    /// (`0x5f5970`): error 0x1a1.
    InCombat,
    /// `UNIT_FIELD_FLAGS` bit 31 (`0x5f3272`) at a chest or an object whose noDamageImmune column
    /// is set (`0x5f59a0`): error 0x169.
    Immune,
}

impl PlayerRefusal {
    /// The `DisplayError` key, `None` for 0x19f, whose `ERR_NOT_WHILE_MOUNTED` has no
    /// GlobalStrings text, so `0x4945b0` drops it unshown.
    pub(super) fn error_key(self) -> Option<&'static str> {
        match self {
            PlayerRefusal::Moving => Some("ERR_USE_OBJECT_MOVING"),
            PlayerRefusal::Dead => Some("ERR_PLAYER_DEAD"),
            PlayerRefusal::LockedOut => Some("ERR_CLIENT_LOCKED_OUT"),
            PlayerRefusal::Mounted => None,
            PlayerRefusal::Shapeshifted => Some("ERR_NOT_WHILE_SHAPESHIFTED"),
            PlayerRefusal::InCombat => Some("ERR_NOT_IN_COMBAT"),
            PlayerRefusal::Immune => Some("ERR_USE_CANT_IMMUNE"),
        }
    }
}

/// `UNIT_FLAG_IN_COMBAT` (`[unit+0xa0]` bit 19, `0x5f323d`).
const UNIT_FLAG_IN_COMBAT: u32 = 0x8_0000;
/// `[unit+0xa0]` bit 31 (`0x5f3278`), which `0x5f3130` reads as immune.
const UNIT_FLAG_IMMUNE: u32 = 0x8000_0000;
/// `GAMEOBJECT_TYPE_DOOR` (0), whose handler (vtable `0x80b2a0`) puts `0x5f4360` in slot 6.
const GO_TYPE_DOOR: i32 = 0;
/// `GAMEOBJECT_TYPE_CHEST` (3), which `0x5f59a0` refuses while immune whatever its columns.
const GO_TYPE_CHEST: i32 = 3;

/// What `usable`'s player arms and the door's slot read: our player and the object.
pub(super) struct GoUse<'a> {
    pub(super) me: &'a ObjectStore,
    /// `[0xb4b3e4]` clear ([`crate::player::OwnControlLost`]).
    pub(super) control_lost: bool,
    /// `SpellShapeshiftForm.dbc`'s rows, for the form's flag `0x8`.
    pub(super) forms: Option<&'a std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>>,
    pub(super) go: &'a ObjectStore,
    pub(super) anim: Option<&'a crate::go_anim::GoAnim>,
    /// The template, for the columns; `None` in flight, which `0x5f8150` reads as 0.
    pub(super) template: Option<&'a crate::go_templates::GoTemplate>,
    /// The object's `Lock.dbc` row exists (`0x5f8180`).
    pub(super) has_lock_row: bool,
}

/// [`PlayerRefusal`] for our player at an object, in the reference's order.
pub(super) fn go_player_refusal(u: &GoUse) -> Option<PlayerRefusal> {
    use crate::go_templates::GoColumn;
    let column = |id| u.template.map_or(0, |t| t.column(&u.go.0, id));
    let go_type = u.go.0.gameobject_type_id();
    // The door's slot 6 runs ahead of `0x5f3130` (`0x5f4366`): with autoClose, a door that
    // starts open refuses at substate 1, one that starts closed at substate 3.
    if go_type == GO_TYPE_DOOR && column(GoColumn::AutoClose) != 0 {
        let blocked = if column(GoColumn::StartOpen) != 0 {
            crate::go_anim::ANIM_CLOSED
        } else {
            crate::go_anim::ANIM_OPENED
        };
        if crate::go_anim::substate_anim(u.anim, u.go) == Some(blocked) {
            return Some(PlayerRefusal::Moving);
        }
    }
    let me = &u.me.0;
    if me.is_dead_or_ghost() {
        return Some(PlayerRefusal::Dead);
    }
    if u.control_lost {
        return Some(PlayerRefusal::LockedOut);
    }
    // A lock row skips the mount and form arms (`0x5f319c`).
    if !u.has_lock_row {
        if me.unit_mount_display_id() != 0 && go_type != GO_TYPE_MAILBOX {
            return Some(PlayerRefusal::Mounted);
        }
        let barred = u.forms.is_some_and(|forms| {
            crate::ui_session::form_bars_interaction(forms, me.unit_shapeshift_form())
        });
        if barred {
            return Some(PlayerRefusal::Shapeshifted);
        }
    }
    let flags = me.unit_flags();
    if flags & UNIT_FLAG_IN_COMBAT != 0 && column(GoColumn::NotInCombat) != 0 {
        return Some(PlayerRefusal::InCombat);
    }
    if flags & UNIT_FLAG_IMMUNE != 0
        && (go_type == GO_TYPE_CHEST || column(GoColumn::NoDamageImmune) != 0)
    {
        return Some(PlayerRefusal::Immune);
    }
    None
}

/// `GameObjectFlags` IN_USE (`0x1`) and NO_INTERACT (`0x10`), which `0x5f2f80` rejects together.
const GO_FLAG_IN_USE_OR_NO_INTERACT: u32 = 0x11;
/// `GO_FLAG_INTERACT_COND`: usable only while the per-player activate bit is set, the quest gate.
const GO_FLAG_INTERACT_COND: u32 = 0x4;
/// `GO_DYNFLAG_LO_ACTIVATE` in `GAMEOBJECT_DYN_FLAGS`, set per player by the server
/// (`GameObject::ActivateToQuest`); read only under `INTERACT_COND`.
const GO_DYNFLAG_ACTIVATE: u32 = 0x1;

/// The types whose highlightable slot `+0x14` is constant false, so `0x5f2f80` never runs.
fn strategy_never_highlightable(type_id: i32) -> bool {
    strategy_is_default(type_id)
        || matches!(
            type_id,
            GO_TYPE_GENERIC
                | GO_TYPE_TRANSPORT
                | GO_TYPE_MAP_OBJECT
                | GO_TYPE_MO_TRANSPORT
                | GO_TYPE_SPELL_FOCUS
                | GO_TYPE_DUEL_ARBITER
                | GO_TYPE_FISHINGHOLE
                | GO_TYPE_AURA_GENERATOR
                | GO_TYPE_AUCTIONHOUSE
                | GO_TYPE_CAPTURE_POINT
        )
}

/// `GAMEOBJECT_TYPE_AUCTIONHOUSE` (20): both slots false (`0x5f68a0`), so it shows nothing and
/// `OnUse` returns unsent (`0x5f8673`); the auctioneer players click is a unit.
const GO_TYPE_AUCTIONHOUSE: i32 = 20;
/// `GAMEOBJECT_TYPE_CAPTURE_POINT` (29): never a cursor (`0x5f6d40`), but a tooltip and brighten
/// when its highlight column `data[19]` is set (`0x5f6d80`), GENERIC's rule. No 1.12 data uses it.
const GO_TYPE_CAPTURE_POINT: i32 = 29;
/// Inputs of the two types whose `+0x14` is a predicate of its own rather than `0x5f2f80` or a
/// constant; the caller resolves them, and each is ignored for every other type.
#[derive(Clone, Copy, Default)]
pub(crate) struct GoOverrides {
    /// FISHINGNODE: the local player is channeling at this bobber.
    pub(crate) channel_owned: bool,
    /// MEETINGSTONE: this stone's area is the one we are queued at.
    pub(crate) meeting_stone_queued: bool,
}

/// `GAMEOBJECT_TYPE_MEETINGSTONE` (23): its `+0x14` (`0x5f6990`) is only
/// `template.data[2] != [0xb72038]`, the queued area; it never calls `0x5f2f80`.
pub(super) const GO_TYPE_MEETINGSTONE: i32 = 23;

/// Whether a GameObject shows an interact cursor at all: `0x5f2f80` plus the per-type overrides.
/// An INTERACT_COND object passes only while the server has set its per-player activate bit: the
/// quest gate. The FISHINGNODE override (`0x5f6710`) compares our channel object, not
/// `CREATED_BY`, then runs the shared gate.
fn highlightable_flags(
    type_id: i32,
    flags: u32,
    dyn_flags: u32,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    if strategy_never_highlightable(type_id) {
        return false;
    }
    if type_id == GO_TYPE_FISHINGNODE && !overrides.channel_owned {
        return false;
    }
    // MEETINGSTONE's slot replaces the shared gate, so none of the terms below apply to it.
    if type_id == GO_TYPE_MEETINGSTONE {
        return !overrides.meeting_stone_queued;
    }
    // The faction term (`0x5f3026`): the GameObject's reaction toward us must beat hostile. An
    // unresolved reaction passes, so a data gap never blanks the world.
    let ordinary_faction_ok = |r: Option<u8>| r.is_none_or(|r| r > 1);
    if type_id == GO_TYPE_TRAP {
        // TRAP inverts it (`0x5f2fc6`): highlightable only to whoever it is hostile to.
        if !reaction.is_none_or(|r| r == 1) {
            return false;
        }
    } else if !ordinary_faction_ok(reaction) {
        return false;
    }
    if flags & GO_FLAG_IN_USE_OR_NO_INTERACT != 0 {
        return false;
    }
    if flags & GO_FLAG_INTERACT_COND != 0 && dyn_flags & GO_DYNFLAG_ACTIVATE == 0 {
        return false;
    }
    true
}

/// `GAMEOBJECT_TYPE_TRAP` (6), the one type whose faction term is inverted.
const GO_TYPE_TRAP: i32 = 6;

/// A GameObject's reaction toward us on the 1/3/4 scale: the reference's `0x5f7fd0` → `0x606530` →
/// `0x606640` chain, the GameObject's template toward the player's. No faction is neutral
/// (`0x5f8025`); `None` when unresolvable, which callers pass. Only the template comparator runs:
/// every shipped GameObject faction (114, 35, 14 and 1375) resolves through it, so the reference's
/// reputation arm and a `GAMEOBJECT_CREATED_BY` creator's reaction are not applied.
pub(crate) fn go_reaction(
    factions: Option<&Factions>,
    go_faction: u32,
    self_store: Option<&ObjectStore>,
) -> Option<u8> {
    if go_faction == 0 {
        return Some(benilla_formats::Reaction::Neutral as u8);
    }
    let catalog = factions?.catalog();
    let go_tpl = catalog.template(go_faction)?;
    let self_tpl = catalog.template(self_store?.0.unit_faction_template()?)?;
    Some(go_tpl.reaction_toward(self_tpl) as u8)
}

/// Whether a picked object becomes the mouseover at all (`[vtbl+0x54]`): false publishes the null
/// mouseover (`0x482985`), so no tooltip (`0x52aa20`), brighten (`0x4945e0`) or cursor. For a
/// GameObject it is the type's slot `+0xc` (`0x5f8620`), not the cursor's slot. `highlight_column`
/// is GENERIC's `data[1]` or CAPTURE_POINT's `data[19]` (`0x5f4830`, `0x5f6d80`); `None`, a
/// template not yet answered, reads eligible.
pub(crate) fn mouseover_eligible(
    type_id: i32,
    flags: u32,
    dyn_flags: u32,
    highlight_column: Option<bool>,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    match type_id {
        GO_TYPE_TRANSPORT | GO_TYPE_MAP_OBJECT | GO_TYPE_MO_TRANSPORT => false,
        GO_TYPE_SPELL_FOCUS
        | GO_TYPE_DUEL_ARBITER
        | GO_TYPE_FISHINGHOLE
        | GO_TYPE_AURA_GENERATOR => true,
        GO_TYPE_GENERIC | GO_TYPE_CAPTURE_POINT => highlight_column.unwrap_or(true),
        // Every other type's `+0xc` forwards to its own `+0x14` (`0x5f9db0`).
        _ => highlightable_flags(type_id, flags, dyn_flags, reaction, overrides),
    }
}

/// [`highlightable_flags`] off a GameObject's descriptor; an absent `GAMEOBJECT_TYPE_ID` is the
/// wire default 0, DOOR. This slot gates the cursor, the right-click use (`OnUse 0x5f8660`) and
/// the pick's pass-2 priority (`0x480c90` via `0x5f8800`); the tooltip and the brighten follow the
/// mouseover slot, [`mouseover_eligible`].
pub(crate) fn go_highlightable(
    store: &ObjectStore,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    highlightable_flags(
        store.0.gameobject_type_id(),
        store.0.gameobject_flags(),
        store.0.gameobject_dynamic_flags(),
        reaction,
        overrides,
    )
}

/// MEETINGSTONE's override for one stone, `template.data[2] == [0xb72038]`. `queued_area` is the
/// reference's global: 0 until the `SMSG_MEETINGSTONE_SETQUEUE` handler (`0x4ca230`) writes it
/// (zeroed at `0x4c9eec`). An unanswered template reads not queued; a `data[2]` of 0 matches the
/// unqueued 0, as in the reference.
pub(crate) fn meeting_stone_queued(area: Option<u32>, queued_area: u32) -> bool {
    area.is_some_and(|a| a == queued_area)
}

/// The FISHINGNODE override's input (`0x5f6710`): our `UNIT_FIELD_CHANNEL_OBJECT` is this
/// GameObject. False when either is unknown, as the reference answers false with no active player.
pub(crate) fn fishing_channel_owned(
    self_store: Option<&ObjectStore>,
    go_guid: Option<u64>,
) -> bool {
    match (self_store, go_guid) {
        (Some(s), Some(g)) => s.0.unit_channel_object() == Some(g),
        _ => false,
    }
}

/// A `LockType.dbc` CursorName to its cursor, the reference's `CursorModeFromName` (`0x523d40`)
/// over the three cursor-bearing lock types in 1.12.
fn cursor_kind_from_lock_name(name: &str) -> Option<CursorKind> {
    match name {
        "PickLock" => Some(CursorKind::PickLock),
        "GatherHerbs" => Some(CursorKind::GatherHerbs),
        "Mine" => Some(CursorKind::Mine),
        _ => None,
    }
}

/// The data-named cursor for a GameObject's lock (`0x5f3070`): the `Lock.dbc` row's first slot
/// only (`[lockRow+0x24]`, no scan), then that `LockType.dbc` row's CursorName.
fn go_lock_cursor(
    lock_id: u32,
    locks: Option<&crate::go_templates::Locks>,
    lock_types: Option<&crate::go_templates::LockTypes>,
) -> Option<CursorKind> {
    if lock_id == 0 {
        return None;
    }
    let slots = locks?.0.slots(lock_id)?;
    let lock_type_id = slots[0].index;
    let name = lock_types?.0.cursor_name(lock_type_id)?;
    cursor_kind_from_lock_name(name)
}

/// A highlightable GameObject's cursor (`GetCursorMode 0x5f8760`): TEXT and the Mail types by
/// type, any other its lock's data-named cursor or the gear.
fn go_cursor_kind(type_id: i32, lock_cursor: Option<CursorKind>) -> CursorKind {
    match type_id {
        GO_TYPE_TEXT => CursorKind::Inspect,
        GO_TYPE_RITUAL | GO_TYPE_MAILBOX | GO_TYPE_28 => CursorKind::Mail,
        _ => lock_cursor.unwrap_or(CursorKind::Interact),
    }
}

/// The QUESTGIVER bit's own gate (`0x5df490`): the cached `SMSG_QUESTGIVER_STATUS`
/// (`[unit+0xcb8]`) is neither NONE nor UNAVAILABLE, and a missing one reads as NONE since the
/// server sends it unprompted. Without the gate a quest-less questgiver opens an empty gossip frame
/// that vmangos fills with placeholder "Greetings $N" text (`QueryHandler.cpp:313-319`).
pub(crate) fn questgiver_has_quest(quest_status: Option<u32>) -> bool {
    use benilla_protocol::messages::dialog_status::{NONE, UNAVAILABLE};
    !matches!(quest_status, None | Some(NONE) | Some(UNAVAILABLE))
}

/// The service ladder (`0x482336..0x4824e3`, unrolled): lowest set bit wins. `None` when no tested
/// bit is set, repair-only units included: bit 14 is never tested.
fn service_cursor(service: u32, quest_status: Option<u32>) -> Option<CursorKind> {
    use npc_flags::*;
    // Bits 0 and 1 both give Speak, so they fold into one test; only bit 1 is gated.
    if service & GOSSIP != 0 || (service & QUESTGIVER != 0 && questgiver_has_quest(quest_status)) {
        Some(CursorKind::Speak)
    } else if service & VENDOR != 0 {
        Some(CursorKind::Pickup)
    } else if service & FLIGHTMASTER != 0 {
        Some(CursorKind::Taxi)
    } else if service & TRAINER != 0 {
        Some(CursorKind::Trainer)
    } else if service & (SPIRITHEALER | SPIRITGUIDE) != 0 {
        Some(CursorKind::Speak)
    } else if service & INNKEEPER != 0 {
        Some(CursorKind::Interact)
    } else if service & BANKER != 0 {
        Some(CursorKind::Buy)
    } else if service & (PETITIONER | TABARDDESIGNER | BATTLEMASTER) != 0 {
        Some(CursorKind::Speak)
    } else if service & AUCTIONEER != 0 {
        Some(CursorKind::Buy)
    } else if service & STABLEMASTER != 0 {
        Some(CursorKind::Speak)
    } else {
        None
    }
}

/// The loot leg's mode, `8 + (keyDown(0) ? 8 : 0)` in the reference (`0x48252c`): LootAll while
/// the effective auto-loot, `autoLootDefault` XOR shift, is on. `autoLootDefault` is not a 1.12
/// CVar; with it off the held key alone decides, as in 1.12.
fn loot_cursor(auto_loot: bool, shift_held: bool) -> CursorKind {
    if auto_loot != shift_held {
        CursorKind::LootAll
    } else {
        CursorKind::Pickup
    }
}

/// Whether a corpse becomes the mouseover (`[CGCorpse_C vtbl+0x54]` = `0x5d76d0`, asked at
/// `0x482982`): a body always, your own included; a bone pile only while it has loot. A refused
/// corpse is still picked (`0x480816`) but shows no name and no brighten.
pub(crate) fn corpse_mouseover_eligible(store: &ObjectStore) -> bool {
    !store.0.corpse_is_bones() || store.0.corpse_lootable()
}

/// What the corpse classifier `0x482740` reads. It has two legs and no own-corpse leg: your own
/// body sets neither flag and reads Point.
#[derive(Debug, Clone, Copy, Default)]
struct CorpseFacts {
    /// `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0 (`0x5d6e20`).
    lootable: bool,
    /// `CORPSE_FIELD_FLAGS` bit 5, `CORPSE_FLAG_LOOTABLE`: the PvP insignia.
    insignia: bool,
    /// `[0xb700e8]`, latched by learning a `SPELL_EFFECT_SKIN_PLAYER_CORPSE` spell.
    skin_latch: bool,
    /// `d2`, centre to centre.
    dist_sq: f32,
    /// Any loot target held (`[player+0x1d28/2c]` nonzero), which `CanLootNow` refuses
    /// (`0x5ec1f6`).
    loot_held: bool,
    /// The already-looting override: `[player+0x1d28/2c] == corpse GUID`.
    looting_this: bool,
    /// `CanAutoInteract`, which `CanLootNow` asks first and then skips its range (`0x5ec11b`).
    can_auto_interact: bool,
    auto_loot: bool,
    /// Key 0, shift.
    shift_held: bool,
}

fn corpse_cursor(f: CorpseFacts) -> Option<(CursorKind, bool)> {
    // Leg 1, lootable: able where `CanLootNow 0x5ec110` allows (`0x4827d7`), its reach and no loot
    // target held, or while this corpse's loot window is open (`0x4827e3`); Pickup or LootAll by
    // key 0 (`0x41f8f0` at `0x4827f8`), grayed at `0x482818`.
    if f.lootable {
        let can_loot_now =
            !f.loot_held && (f.can_auto_interact || f.dist_sq <= CORPSE_INTERACT_RANGE_SQ);
        let able = can_loot_now || f.looting_this;
        return Some((loot_cursor(f.auto_loot, f.shift_held), !able));
    }
    // Leg 2, the PvP insignia (`0x482875..0x4828a2`), inert in 1.12.1 as no player holds a
    // latching spell. Not applied: its third term `!0x6067d0(player, corpse)`, and the faction's
    // SkinAlliance(20) or SkinHorde(19) cursor (`0x80439c`), shown here as Skin.
    if f.insignia && f.skin_latch {
        return Some((CursorKind::Skin, f.dist_sq > CORPSE_INTERACT_RANGE_SQ));
    }
    None // the reference's "else clear": Point
}

/// Resolve this frame's [`WorldCursor`] over the hover.
pub(super) fn classify_cursor(
    hovered: Res<Hovered>,
    hovered_object: Res<HoveredObject>,
    inputs: CursorInputs,
    (mut cursor, mut fork): (ResMut<WorldCursor>, ResMut<AttackFork>),
) {
    let (want, want_fork) = classify(&inputs, &hovered, &hovered_object);
    if *cursor != want {
        *cursor = want;
    }
    if *fork != want_fork {
        *fork = want_fork;
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct CursorInputs<'w, 's> {
    relations: super::RelationWorld<'w, 's>,
    lock: Res<'w, crate::player::MoverLock>,
    // The attack leg's `[0xb4b3e4]` clear.
    own_control: Res<'w, crate::player::OwnControlLost>,
    approach: Res<'w, crate::player::Approach>,
    #[allow(clippy::type_complexity)]
    units: Query<
        'w,
        's,
        (
            &'static Transform,
            Option<&'static ObjectStore>,
            Option<&'static crate::go_anim::GoAnim>,
        ),
    >,
    self_q: Query<'w, 's, (&'static Transform, &'static ObjectStore), With<SelfPlayer>>,
    go_inputs: super::lock::GoLockInputs<'w, 's>,
    opener_reach: super::lock::OpenerReach<'w, 's>,
    player_actions: Res<'w, crate::ui_action::PlayerActions>,
    // `[0xb700e4]` and `[0xb700e8]`, the skin legs' learned-spell latches.
    learned: Res<'w, crate::ui_action::LearnedAbilities>,
    quest: Res<'w, crate::ui_quest::QuestGiver>,
    loot_cfg: Res<'w, crate::ui_loot::LootConfig>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    // `[player+0x1d28/2c]`, the object whose loot window is open.
    loot_latch: Res<'w, crate::ui_loot::LootLatch>,
    // `[0xb72038]`, the queued area; absent in a headless build, which reads as not queued.
    stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
}

/// The cursor and the attack fork over a pick; no pick, or nothing resolvable, reads Point, the
/// classifier's clear (`0x523d30`), which [`crate::cursor`] shows as the base mode.
pub(super) fn classify(
    inputs: &CursorInputs,
    hovered: &Hovered,
    hovered_object: &HoveredObject,
) -> (WorldCursor, AttackFork) {
    let CursorInputs {
        relations,
        lock,
        own_control,
        approach,
        units,
        self_q,
        go_inputs,
        opener_reach,
        player_actions,
        learned,
        quest,
        loot_cfg,
        keys,
        loot_latch,
        stone,
    } = inputs;
    let can_auto_interact = crate::player::can_auto_interact(
        approach.enabled,
        self_q.single().ok().map(|(_, s)| s),
        lock,
    );
    // A GameObject that is not highlightable clears the cursor, as the reference's handler does.
    let resolve_go = || {
        let (go_tf, store, anim) = units.get(hovered_object.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let reaction = go_reaction(
            relations.factions(),
            store.0.gameobject_faction(),
            Some(self_store),
        );
        // Read before the gate: MEETINGSTONE's override is a template slot.
        let tmpl = go_inputs.templates.of(&store.0);
        let overrides = GoOverrides {
            channel_owned: fishing_channel_owned(Some(self_store), hovered_object.guid),
            meeting_stone_queued: meeting_stone_queued(
                tmpl.and_then(|t| t.meeting_stone).map(|m| m.area),
                stone.as_deref().map_or(0, |s| s.area),
            ),
        };
        if !go_highlightable(store, reaction, overrides) {
            return None;
        }
        // Until its template answers, a locked GameObject reads the gear.
        let lock_id = tmpl.map_or(0, |t| t.lock_id);
        let lock_cursor = go_lock_cursor(
            lock_id,
            go_inputs.locks.as_deref(),
            go_inputs.lock_types.as_deref(),
        );
        let go_type = store.0.gameobject_type_id();
        let kind = go_cursor_kind(go_type, lock_cursor);
        let dist_sq = go_tf.translation.distance_squared(self_tf.translation);
        let has_lock_row = lock_id != 0
            && go_inputs
                .locks
                .as_deref()
                .is_some_and(|l| l.0.slots(lock_id).is_some());
        let refused = go_player_refusal(&GoUse {
            me: self_store,
            control_lost: own_control.0,
            forms: go_inputs.spells.as_deref().map(|s| &s.forms),
            go: store,
            anim,
            template: tmpl,
            has_lock_row,
        })
        .is_some();
        // `usable 0x5f3130`'s player arms, lock arm (`0x5f32a6`), then range arm (`0x5f330c`).
        // The lock arm needs `GO_FLAG_LOCKED`, so an herb the player cannot gather stays lit and
        // refuses on the click, while a padlocked door grays.
        let facts = super::lock::go_facts(Some((store, crate::go_anim::go_state(anim, store))));
        let outcome = go_inputs
            .locks
            .as_deref()
            .and_then(|l| l.0.slots(lock_id).filter(|_| lock_id != 0))
            .map(|slots| {
                super::lock::resolve_lock(
                    slots,
                    &player_actions.openers,
                    go_inputs.spells.as_deref(),
                    go_inputs.skill_lines.as_ref().map(|s| &s.catalog),
                    Some(self_store),
                    &go_inputs.objects,
                    facts,
                    &mut None,
                )
            });
        let lock_unmet = outcome.is_some_and(|o| o.blocks_usable(facts.flag_locked));
        // The distance test's range is the opener spell's when one opens the lock (`0x5f3328`).
        let opener = outcome.map_or_else(Default::default, |o| opener_reach.opener(go_inputs, o));
        let unable = kind != CursorKind::PickLock
            && (refused
                || lock_unmet
                || go_destroyed(facts.state, opener.spell)
                || !go_in_reach(dist_sq, go_type, opener.range));
        Some((kind, unable))
    };
    // The reference picks once over all objects; benilla picks units and GameObjects apart and
    // classifies whichever is nearer under the cursor.
    let attack_fork = std::cell::Cell::new(false);
    let resolve_unit = || {
        let (unit_tf, store, _) = units.get(hovered.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let dist_sq = unit_tf.translation.distance_squared(self_tf.translation);
        let reach = melee_reach(store, self_store);
        let in_melee = dist_sq <= reach * reach;

        let dead = store.0.unit_is_dead();
        if dead {
            if store.0.unit_lootable() {
                // Lit where `CanLootNow 0x5ec110` allows (`0x4824fe`), its melee reach and no loot
                // target held, or over the unit being looted (`0x48250a`).
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                let held = loot_latch.0;
                let can_loot_now = held.is_none() && (can_auto_interact || in_melee);
                let looting_this = held.is_some() && held == hovered.guid;
                return Some((
                    loot_cursor(loot_cfg.auto_loot, shift),
                    !(can_loot_now || looting_this),
                ));
            }
            // Skin also needs the learn latch `[0xb700e4 + 4×isPlayerTarget]` (`0x482589`): a
            // player who has learned no Skinning spell sees Point.
            if store.0.unit_flags() & UNIT_FLAG_SKINNABLE != 0 && learned.skinning.is_some() {
                return Some((CursorKind::Skin, !in_melee));
            }
            return None; // a plain corpse: Point
        }

        let rel = relations.get();
        let (me, unit) = (relations.me()?, relations.unit_of(hovered.guid?, store));
        // `CanInteract 0x6067f0`, asked at `0x482310` through `CanInteractNow 0x606880`, picks
        // the ladder; it is not a reaction threshold.
        if rel.can_interact(&me, &unit) {
            let status = hovered.guid.and_then(|g| quest.status(g));
            // No matching bit clears the cursor (`je 0x4826cb`) without reaching the attack leg:
            // Point, never the sword.
            return service_cursor(store.0.unit_npc_flags(), status)
                .map(|kind| (kind, dist_sq > SERVICE_RANGE_SQ));
        }
        // `CanAttack 0x606980` (`0x482680`), the predicate TAB, the combat flash and
        // `UnitCanAttack` share; not attackable clears the cursor (`0x4826cb`): Point.
        if !rel.can_attack(&me, &unit) {
            return None;
        }
        attack_fork.set(true);
        player_attack_legs(self_store, own_control.0)
            .then_some((CursorKind::Attack, dist_sq > ATTACK_RANGE_SQ))
    };
    // The corpse classifier `0x482740`.
    let resolve_corpse = || {
        let (corpse_tf, store, _) = units.get(hovered.corpse?).ok()?;
        let store = store?;
        let (self_tf, _) = self_q.single().ok()?;
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        corpse_cursor(CorpseFacts {
            lootable: store.0.corpse_lootable(),
            insignia: store.0.corpse_pvp_insignia(),
            skin_latch: learned.skin_player_corpse.is_some(),
            dist_sq: corpse_tf.translation.distance_squared(self_tf.translation),
            loot_held: loot_latch.0.is_some(),
            looting_this: loot_latch.0.is_some() && loot_latch.0 == hovered.corpse_guid,
            can_auto_interact,
            auto_loot: loot_cfg.auto_loot,
            shift_held: shift,
        })
    };
    let resolved = if go_is_nearest(hovered, hovered_object) {
        resolve_go()
    } else if hovered.corpse.is_some() {
        resolve_corpse()
    } else {
        resolve_unit()
    };
    let (kind, unable) = resolved.unwrap_or((CursorKind::Point, false));
    (WorldCursor { kind, unable }, AttackFork(attack_fork.get()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;

    /// `CanLootNow 0x5ec110` skips its range under `CanAutoInteract` (`0x5ec11b`), so the click
    /// that walks there finds a lit pouch.
    #[test]
    fn click_to_move_lights_a_far_corpses_pouch() {
        let far = CorpseFacts {
            lootable: true,
            dist_sq: 400.0,
            ..CorpseFacts::default()
        };
        assert_eq!(corpse_cursor(far), Some((CursorKind::Pickup, true)));
        assert_eq!(
            corpse_cursor(CorpseFacts {
                can_auto_interact: true,
                ..far
            }),
            Some((CursorKind::Pickup, false))
        );
    }

    #[test]
    fn the_corpse_classifier_has_two_legs_and_no_own_corpse_leg() {
        let near = 25.0; // exactly the gate, which is inclusive
        let far = 25.01;
        let bones = |dist_sq| CorpseFacts {
            lootable: true,
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(
            corpse_cursor(bones(near)),
            Some((CursorKind::Pickup, false))
        );
        assert_eq!(
            corpse_cursor(CorpseFacts {
                auto_loot: true,
                ..bones(near)
            }),
            Some((CursorKind::LootAll, false))
        );
        assert_eq!(corpse_cursor(bones(far)), Some((CursorKind::Pickup, true)));
        assert_eq!(
            corpse_cursor(CorpseFacts {
                looting_this: true,
                ..bones(far)
            }),
            Some((CursorKind::Pickup, false))
        );
        let insignia = |skin_latch, dist_sq| CorpseFacts {
            insignia: true,
            skin_latch,
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(corpse_cursor(insignia(false, near)), None);
        assert_eq!(
            corpse_cursor(insignia(true, near)),
            Some((CursorKind::Skin, false))
        );
        assert_eq!(
            corpse_cursor(insignia(true, far)),
            Some((CursorKind::Skin, true))
        );
        // The reference tests lootable first.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia: true,
                skin_latch: true,
                ..bones(near)
            }),
            Some((CursorKind::Pickup, false))
        );
        // Your own corpse sets neither flag.
        let mine = |dist_sq| CorpseFacts {
            skin_latch: true,
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(corpse_cursor(mine(near)), None);
        assert_eq!(
            corpse_cursor(CorpseFacts {
                auto_loot: true,
                shift_held: true,
                ..mine(far)
            }),
            None
        );
    }

    #[test]
    fn the_loot_pouch_triples_with_the_effective_auto_loot() {
        assert_eq!(loot_cursor(false, false), CursorKind::Pickup);
        assert_eq!(loot_cursor(true, false), CursorKind::LootAll);
        // Shift inverts both ways; with the setting off it is 1.12's whole mechanism.
        assert_eq!(loot_cursor(false, true), CursorKind::LootAll);
        assert_eq!(loot_cursor(true, true), CursorKind::Pickup);
        let far = WorldCursor {
            kind: CursorKind::LootAll,
            unable: true,
        };
        assert_eq!(far.stem(), "UnableLootAll");
    }

    #[test]
    fn stems_name_the_shipped_blps() {
        let attack = WorldCursor {
            kind: CursorKind::Attack,
            unable: false,
        };
        assert_eq!(attack.stem(), "Attack");
        let far = WorldCursor {
            kind: CursorKind::Attack,
            unable: true,
        };
        assert_eq!(far.stem(), "UnableAttack");
        let point = WorldCursor {
            kind: CursorKind::Point,
            unable: true,
        };
        assert_eq!(point.stem(), "Point");
    }

    #[test]
    fn mouseover_eligibility_matches_the_per_type_slot_table() {
        for t in [
            GO_TYPE_SPELL_FOCUS,
            GO_TYPE_DUEL_ARBITER,
            GO_TYPE_FISHINGHOLE,
            GO_TYPE_AURA_GENERATOR,
        ] {
            assert!(mouseover_eligible(
                t,
                GO_FLAG_INTERACT_COND,
                0,
                None,
                None,
                GoOverrides::default()
            ));
            assert!(mouseover_eligible(
                t,
                0x10,
                0,
                None,
                None,
                GoOverrides::default()
            ));
        }
        for t in [GO_TYPE_TRANSPORT, GO_TYPE_MAP_OBJECT, GO_TYPE_MO_TRANSPORT] {
            assert!(!mouseover_eligible(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                None,
                None,
                GoOverrides::default()
            ));
        }
        assert!(mouseover_eligible(
            GO_TYPE_GENERIC,
            0,
            0,
            Some(true),
            None,
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_GENERIC,
            0,
            0,
            Some(false),
            None,
            GoOverrides::default()
        ));
        assert!(
            mouseover_eligible(GO_TYPE_GENERIC, 0, 0, None, None, GoOverrides::default()),
            "template not answered yet reads eligible, so a signpost isn't blank while it queries"
        );
        assert!(!mouseover_eligible(
            0,
            GO_FLAG_INTERACT_COND,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            GO_FLAG_INTERACT_COND,
            GO_DYNFLAG_ACTIVATE,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            3,
            0x10,
            0,
            None,
            None,
            GoOverrides::default()
        )); // NO_INTERACT chest
        assert!(!mouseover_eligible(
            3,
            0x1,
            0,
            None,
            None,
            GoOverrides::default()
        )); // IN_USE chest
            // A door hostile to us, like the Deadmines Factory Door (faction 114, flags 0x20).
        assert!(!mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(1),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(4),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_TRAP,
            0,
            0,
            None,
            Some(1),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_TRAP,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            19,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        )); // mailbox
    }

    #[test]
    fn highlightable_gates_the_quest_object_but_not_the_plain_door() {
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default())); // DOOR, no flags
        assert!(!highlightable_flags(
            GO_TYPE_GENERIC,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        for t in [GO_TYPE_TRANSPORT, GO_TYPE_MAP_OBJECT, GO_TYPE_MO_TRANSPORT] {
            assert!(!highlightable_flags(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                None,
                GoOverrides::default()
            ));
        }
        for t in [
            GO_TYPE_SPELL_FOCUS,
            GO_TYPE_DUEL_ARBITER,
            GO_TYPE_FISHINGHOLE,
            GO_TYPE_AURA_GENERATOR,
        ] {
            assert!(!highlightable_flags(t, 0, 0, None, GoOverrides::default()));
            assert!(!highlightable_flags(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                Some(3),
                GoOverrides::default()
            ));
        }
        // The marker set's two slots disagree: a tooltip, but no interact cursor.
        assert!(mouseover_eligible(
            GO_TYPE_SPELL_FOCUS,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_SPELL_FOCUS,
            0,
            0,
            Some(3),
            GoOverrides::default()
        ));
        for t in [GO_TYPE_GUARDPOST, GO_TYPE_MAX + 1, 99, -1] {
            assert!(strategy_is_default(t), "type {t} takes the default arm");
            assert!(!highlightable_flags(t, 0, 0, None, GoOverrides::default()));
            assert!(!mouseover_eligible(
                t,
                0,
                0,
                None,
                None,
                GoOverrides::default()
            ));
        }
        for t in (0..=GO_TYPE_MAX).filter(|t| *t != GO_TYPE_GUARDPOST) {
            assert!(!strategy_is_default(t), "type {t} has its own strategy");
        }
        assert!(!highlightable_flags(
            GO_TYPE_AUCTIONHOUSE,
            0,
            GO_DYNFLAG_ACTIVATE,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_AUCTIONHOUSE,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(true),
            Some(3),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(false),
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            3,
            0x1,
            0,
            None,
            GoOverrides::default()
        )); // CHEST, IN_USE
        assert!(!highlightable_flags(
            3,
            0x10,
            0,
            None,
            GoOverrides::default()
        )); // CHEST, NO_INTERACT
        assert!(!highlightable_flags(
            3,
            GO_FLAG_INTERACT_COND,
            0,
            None,
            GoOverrides::default()
        )); // quest chest, no quest → clear
        assert!(highlightable_flags(
            3,
            GO_FLAG_INTERACT_COND,
            GO_DYNFLAG_ACTIVATE,
            None,
            GoOverrides::default()
        )); // quest chest, quest held → usable
    }

    #[test]
    fn the_meeting_stone_answers_only_the_queued_area() {
        let queued = GoOverrides {
            meeting_stone_queued: true,
            ..Default::default()
        };
        assert!(highlightable_flags(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            queued
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            None,
            queued
        ));
        for (flags, dyn_flags, reaction) in [
            (0x10, 0, Some(1)),
            (0x1, 0, Some(1)),
            (GO_FLAG_INTERACT_COND, 0, Some(1)),
        ] {
            assert!(
                highlightable_flags(
                    GO_TYPE_MEETINGSTONE,
                    flags,
                    dyn_flags,
                    reaction,
                    GoOverrides::default()
                ),
                "flags {flags:#x} must not reach a meeting stone — its slot never calls 0x5f2f80"
            );
        }
        assert!(!meeting_stone_queued(None, 0));
        assert!(!meeting_stone_queued(Some(1519), 0));
        assert!(meeting_stone_queued(Some(1519), 1519));
        assert!(!meeting_stone_queued(Some(1519), 1517));
        assert!(!meeting_stone_queued(None, 1519));
        // The reference's own `0 != 0`: a stone whose `data[2]` is 0 matches while unqueued.
        assert!(meeting_stone_queued(Some(0), 0));
    }

    #[test]
    fn the_bobber_is_channel_gated_and_reaches_a_hundred_yards() {
        assert!(highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(!highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        // NO_INTERACT still applies after the channel check.
        assert!(!highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0x10,
            0,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default()));
        assert_eq!(go_reach_sq(GO_TYPE_FISHINGNODE, None), 100.0 * 100.0);
        assert_eq!(go_reach_sq(GO_TYPE_CHAIR, None), 100.0 * 100.0);
        assert!(!fishing_channel_owned(None, Some(7)));
        assert!(!fishing_channel_owned(None, None));
    }

    /// `[handler+0xc]` by type (`0x5f2f67` and the nine stores over it), as the table in
    /// benilla-re's gameobjecttypes gives it.
    #[test]
    fn each_type_has_its_handlers_use_range() {
        let base = [
            0, 1, 3, 5, 6, 8, 10, 11, 13, 14, 15, 16, 18, 19, 20, 22, 23, 25, 28, 29, 30,
        ];
        for t in base {
            assert_eq!(go_use_range(t, true), 5.0, "type {t}");
        }
        for t in [2, 9, 24, 26, 27] {
            assert_eq!(go_use_range(t, true), 5.555_555_3, "type {t}");
        }
        assert_eq!(go_use_range(4, true), 10.0);
        assert_eq!(go_use_range(17, true), 100.0);
        assert_eq!(go_use_range(12, true), 0.0);
        // The chair hands back 3.0 after its 100.0 base test; the static handler has none.
        assert_eq!(go_use_range(GO_TYPE_CHAIR, true), 3.0);
        assert_eq!(
            go_use_range(GO_TYPE_CHAIR, false),
            100.0,
            "past the base test"
        );
        assert_eq!(go_use_range(21, true), 0.0);
        assert_eq!(go_use_range(31, true), 0.0);
    }

    /// The Hyjal gate's defect: a door lit from beyond its 5.0 yd (`0x5f3346`).
    #[test]
    fn a_door_is_in_reach_to_five_yards_and_no_farther() {
        assert!(!go_in_reach(5.2 * 5.2, 0, None), "5.2 yd");
        assert!(go_in_reach(4.9 * 4.9, 0, None), "4.9 yd");
        assert!(go_in_reach(25.0, 0, None), "equality passes");
        assert!(go_in_reach(f32::NAN, 0, None), "a NaN passes");
        // A quest giver keeps 5.5555553, a binder 10.
        assert!(go_in_reach(5.5 * 5.5, 2, None));
        assert!(!go_in_reach(5.6 * 5.6, 2, None));
        assert!(go_in_reach(9.9 * 9.9, 4, None));
        assert!(!go_in_reach(10.1 * 10.1, 4, None));
    }

    /// `0x5f3328`: the range of the spell that opens the lock replaces the handler's for the test.
    #[test]
    fn an_opener_spells_range_replaces_the_handlers() {
        assert!(go_in_reach(7.9 * 7.9, 0, Some(8.0)));
        assert!(go_in_reach(64.0, 0, Some(8.0)), "equality passes");
        assert!(!go_in_reach(8.1 * 8.1, 0, Some(8.0)));
        assert!(
            !go_in_reach(4.0, 0, Some(1.0)),
            "a shorter spell shortens it"
        );
        // The chair's own 3.0 yd test still follows.
        assert!(!go_in_reach(4.0 * 4.0, GO_TYPE_CHAIR, Some(100.0)));
        assert!(go_in_reach(2.9 * 2.9, GO_TYPE_CHAIR, None));
    }

    /// `0x5f571a`/`0x5f572b`: the seat test succeeds only strictly below 3.0 yd squared, so exactly
    /// 9.0 and a NaN fail, where the base test passes both.
    #[test]
    fn the_chairs_seat_test_is_strict() {
        assert!(!go_in_reach(9.0, GO_TYPE_CHAIR, None), "exactly 3.0 yd");
        assert!(go_in_reach(
            f32::from_bits(9.0f32.to_bits() - 1),
            GO_TYPE_CHAIR,
            None
        ));
        assert!(!go_in_reach(f32::NAN, GO_TYPE_CHAIR, None));
        assert!(go_in_reach(9.0, 0, None), "a door at 3.0 yd, unaffected");
    }

    /// `0x5f3395`: state 2 refuses unless a spell opens the lock.
    #[test]
    fn state_two_refuses_without_an_opener_spell() {
        use benilla_formats::{GO_STATE_ACTIVE, GO_STATE_READY};
        assert!(go_destroyed(2, false));
        assert!(!go_destroyed(2, true));
        assert!(!go_destroyed(GO_STATE_ACTIVE, false));
        assert!(!go_destroyed(GO_STATE_READY, false));
    }

    #[test]
    fn melee_reach_floors_at_five() {
        // A typical pair, 1.5 + 1.5 + 1.333, is lifted to the 5 yd floor.
        assert_eq!((1.5_f32 + 1.5 + MELEE_OFFSET).max(MELEE_FLOOR), 5.0);
        assert!(((3.0_f32 + 3.0 + MELEE_OFFSET).max(MELEE_FLOOR) - 7.333_33).abs() < 1e-4);
    }

    #[test]
    fn service_ladder_matches_the_unrolled_binary() {
        use npc_flags::*;
        // A quest on offer, so the QUESTGIVER row passes its gate.
        let has = Some(dialog_status::AVAILABLE);
        assert_eq!(service_cursor(GOSSIP, None), Some(CursorKind::Speak));
        assert_eq!(service_cursor(QUESTGIVER, has), Some(CursorKind::Speak));
        assert_eq!(service_cursor(VENDOR, None), Some(CursorKind::Pickup));
        assert_eq!(service_cursor(FLIGHTMASTER, None), Some(CursorKind::Taxi));
        assert_eq!(service_cursor(TRAINER, None), Some(CursorKind::Trainer));
        assert_eq!(service_cursor(SPIRITHEALER, None), Some(CursorKind::Speak));
        assert_eq!(service_cursor(INNKEEPER, None), Some(CursorKind::Interact));
        assert_eq!(service_cursor(BANKER, None), Some(CursorKind::Buy));
        assert_eq!(service_cursor(BATTLEMASTER, None), Some(CursorKind::Speak));
        assert_eq!(service_cursor(AUCTIONEER, None), Some(CursorKind::Buy));
        assert_eq!(service_cursor(STABLEMASTER, None), Some(CursorKind::Speak));
        assert_eq!(
            service_cursor(GOSSIP | VENDOR, None),
            Some(CursorKind::Speak)
        );
        assert_eq!(
            service_cursor(INNKEEPER | BANKER, None),
            Some(CursorKind::Interact)
        );
        // REPAIR (`0x4000`) is never tested.
        assert_eq!(service_cursor(0x4000, None), None);
        assert_eq!(service_cursor(0, None), None);
    }

    #[test]
    fn the_ladder_falls_out_to_point_not_to_the_sword() {
        use npc_flags::*;
        assert_eq!(service_cursor(REPAIR_ONLY, None), None);
        assert_eq!(service_cursor(QUESTGIVER, Some(dialog_status::NONE)), None);
        // `resolve_unit`'s own expression for the branch: a `None` stays `None`, which reads Point.
        let unable = 20.0 > SERVICE_RANGE_SQ;
        assert_eq!(
            service_cursor(REPAIR_ONLY, None).map(|kind| (kind, unable)),
            None,
            "repair-only reads Point, not UnableAttack"
        );
        assert_eq!(
            service_cursor(GOSSIP, None).map(|kind| (kind, unable)),
            Some((CursorKind::Speak, false)),
            "…while a bit that IS consulted still reaches its cursor"
        );
    }

    /// The sword's player legs (`0x48264a`–`0x4826a2`): a corpse, a ghost at health 1, a rider, a
    /// pacified player and one without control each see the pointer.
    #[test]
    fn the_sword_needs_a_live_unmounted_unpacified_player_in_control() {
        // Health 22, max health 28, unit flags 46, mount display 133, player flags 190.
        let own = |pairs: &[(u16, u32)]| {
            let mut all = vec![(22, 100), (28, 100)];
            all.extend_from_slice(pairs);
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&all))
        };
        assert!(player_attack_legs(&own(&[]), false));
        assert!(!player_attack_legs(&own(&[(22, 0)]), false), "dead");
        assert!(
            !player_attack_legs(&own(&[(22, 1), (190, 0x10)]), false),
            "a ghost"
        );
        assert!(!player_attack_legs(&own(&[(133, 1147)]), false), "mounted");
        assert!(
            !player_attack_legs(&own(&[(46, UNIT_FLAG_PACIFIED)]), false),
            "pacified"
        );
        assert!(!player_attack_legs(&own(&[]), true), "no control");
    }

    /// `UNIT_NPC_FLAGS` REPAIR, the service bit the ladder never tests.
    const REPAIR_ONLY: u32 = 0x4000;

    #[test]
    fn questgiver_flag_alone_is_not_talkable() {
        use npc_flags::*;
        for status in [
            None,
            Some(dialog_status::NONE),
            Some(dialog_status::UNAVAILABLE),
        ] {
            assert_eq!(
                service_cursor(QUESTGIVER, status),
                None,
                "status {status:?} must not classify Speak"
            );
        }
        for status in [
            dialog_status::CHAT,
            dialog_status::INCOMPLETE,
            dialog_status::REWARD_REP,
            dialog_status::AVAILABLE,
            dialog_status::REWARD_OLD,
            dialog_status::REWARD2,
        ] {
            assert_eq!(
                service_cursor(QUESTGIVER, Some(status)),
                Some(CursorKind::Speak),
                "status {status} must classify Speak"
            );
        }
        assert_eq!(
            service_cursor(GOSSIP | QUESTGIVER, None),
            Some(CursorKind::Speak)
        );
        assert_eq!(
            service_cursor(QUESTGIVER | VENDOR, None),
            Some(CursorKind::Pickup)
        );
    }

    #[test]
    fn lock_names_resolve_to_the_three_data_cursors() {
        assert_eq!(
            cursor_kind_from_lock_name("PickLock"),
            Some(CursorKind::PickLock)
        );
        assert_eq!(
            cursor_kind_from_lock_name("GatherHerbs"),
            Some(CursorKind::GatherHerbs)
        );
        assert_eq!(cursor_kind_from_lock_name("Mine"), Some(CursorKind::Mine));
        assert_eq!(cursor_kind_from_lock_name(""), None);
        assert_eq!(cursor_kind_from_lock_name("Fishing"), None);
    }

    #[test]
    fn go_cursor_kind_maps_type_then_lock() {
        // MAILBOX(19), RITUAL(18) and type 28 take Mail whatever their lock.
        assert_eq!(go_cursor_kind(19, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(18, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(28, Some(CursorKind::Mine)), CursorKind::Mail);
        assert_eq!(go_cursor_kind(9, None), CursorKind::Inspect); // TEXT
        for t in [0, 1, 3, 10, 6, 24] {
            assert_eq!(go_cursor_kind(t, None), CursorKind::Interact);
        }
        assert_eq!(go_cursor_kind(3, Some(CursorKind::Mine)), CursorKind::Mine);
        assert_eq!(
            go_cursor_kind(3, Some(CursorKind::GatherHerbs)),
            CursorKind::GatherHerbs
        );
        assert_eq!(
            go_cursor_kind(3, Some(CursorKind::PickLock)),
            CursorKind::PickLock
        );
    }

    #[test]
    fn go_cursor_stems_name_the_shipped_blps() {
        for (kind, stem) in [
            (CursorKind::Mail, "Mail"),
            (CursorKind::Mine, "Mine"),
            (CursorKind::GatherHerbs, "GatherHerbs"),
            (CursorKind::PickLock, "PickLock"),
        ] {
            assert_eq!(
                WorldCursor {
                    kind,
                    unable: false
                }
                .stem(),
                stem
            );
            assert_eq!(
                WorldCursor { kind, unable: true }.stem(),
                format!("Unable{stem}")
            );
        }
    }
}
