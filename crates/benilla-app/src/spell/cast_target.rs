//! The cast-arm's **target resolution** — what actually goes in `CMSG_CAST_SPELL`'s target block.
//!
//! Transcribes `Spell_C::ArmCast 0x6e5250` + `BindTarget 0x6e5b40`: the client seeds a targeting
//! flag_word from `Spell.dbc Targets` (`SpellRec+0x34`),
//! adjusts it with the implicit-target switch (`SpellRec+0x148`, jump-table `0x6e5484`), and then
//!
//! - **flag_word == 0** ⇒ the cast needs no target at all — commit immediately, wire mask
//!   `TARGET_FLAG_SELF (0)`, nothing follows (Ice Armor, Battle Shout, Feign Death…). The server
//!   fills the target from the spell's implicit targeting. The real client **never** ships the
//!   current selection for these — doing so is exactly the "Invalid target" bug this fixes.
//! - **nonzero** ⇒ a target is required: the binder satisfies each bit against the candidate with
//!   the matching object-layer relation (assist `0x6066f0` / attack `0x606980` / corpse `0x6067d0`),
//!   binds the guid and clears the bit; only a fully-cleared word commits (wire mask
//!   `TARGET_FLAG_UNIT (0x2)` + the bound guid).
//! - **`Attributes & 0x200`** ⇒ the candidate is not the selection at all but the caster's
//!   **equipped main hand** (`6e5361`–`6e5391`): the client picks the weapon itself and binds it
//!   as an ITEM target, so a shaman's Rockbiter / Flametongue / Frostbrand / Windfury Weapon —
//!   22 rows, four names, the whole 5875 family — casts straight onto the weapon with no cursor
//!   and no click. With the hand **empty** there is still no cursor: TryCast
//!   tests the same bit again on ArmCast's FALSE return (`6e504b`), clears the word and says
//!   *"Your weapon hand is empty"*.
//! - a candidate that satisfies nothing falls back to the **active player** — gated on the
//!   `autoSelfCast` CVar (name `0x870dc0`, gate `[0xceac34]+0x28` at `0x6e53d7`; registered with
//!   engine default `"0"`). The classic "buffing with an enemy targeted casts on yourself".
//! - still unbound ⇒ the ref leaves the nonzero flag_word standing, which *is* its targeting-cursor
//!   mode (`SpellIsTargeting 0x6e48a0` = word != 0 — the hand cursor, click to bind). That machine
//!   is unmodeled here (INTERIM): we refuse locally with the client's own error strings instead —
//!   `0x09` "You have no target." / `0x0A` "Invalid target" — and never ship an unbindable cast.
//!
//! **All three seams of the targeting cursor are modeled** — and the cursor carries the *word*
//! ([`CastWireTarget::Targeting`]), not a verdict about it, because the reference's three "wants"
//! predicates are three independent mask tests on the one word `0xcecac0` and **more than one can
//! be true at once**: `TargetingWantsLocation 0x6e6320` (`& 0x60`) → the terrain click (decision
//! 0792); `TargetingWantsItem 0x6e6330` (`& 0x4010`) → the bag / paper-doll click;
//! `TargetingWantsGameObject 0x6e62d0` (`ch & 0x48`, i.e. `& 0x4800`) → the world click on a
//! GameObject. A LOCKED spell — Opening, Pick Lock, Mining, Herb Gathering —
//! satisfies the last **two**, and the click decides which leg it was, exactly as `BindTarget
//! 0x6e5b40` decides: by the clicked object's typemask (`6e5f17` item bit 1, `6e5f52` GameObject
//! bit 5), each arm then re-testing the word. The terrain seam likewise serves **both** location
//! bits — `TargetingWantsLocation`'s mask is `0x60`, and `BindLocation 0x6e60f0` has an arm for
//! each (closing B388: Martin Fury's spell 265 is a pure `0x20` word, and reading
//! that seam as dest-only refused the only three shipped items that carry one). Still deferred,
//! refused-not-guessed: STRING bit 13, and the *unit* hand-cursor mode (the residual-unit-word
//! machine behind the autoSelfCast stand-in above).

use benilla_formats::SpellDisplay;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::net::{ObjectStore, Reputations, SelfGuid, SelfPlayer};
use crate::target::{can_attack, ring_reaction, Factions, Selection};

/// `TARGET_FLAG_*` bits of the targeting flag_word (`0xcecac0`), as `ArmCast 0x6e5250` sets them
/// and `BindTarget 0x6e5b40` consumes them. Only the bits the resolver consumes are named.
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

/// Client-side cast-failed reasons (the `CastErrors` strings): "You have no target." /
/// "Invalid target" — the INTERIM stand-ins for the unmodeled targeting-cursor mode.
pub(crate) const ERR_NO_TARGET: u8 = 0x09;
pub(crate) const ERR_INVALID_TARGET: u8 = 0x0A;

/// `SPELL_FAILED_MAINHAND_EMPTY` — "Your weapon hand is empty". The reference's own refusal for
/// an imbue pressed with nothing in the main hand, raised at `6e5050` off TryCast's bit-9 test
/// (`6e504b`). Unlike the two stand-ins above this is a *real* reference refusal site, which is
/// why it travels as [`CastWireTarget::RefusedAtArm`].
pub(crate) const ERR_MAINHAND_EMPTY: u8 = 0x2d;

/// The dest-location bit — the ground-cast wire mask (`BindLocation 0x6e60f0`'s bit-6 arm).
const TF_DEST_LOCATION: u16 = 0x0040;
/// The source-location bit — `BindLocation 0x6e60f0`'s **other** arm (`6e6105`–`6e6126`), tested
/// one instruction before the dest one and writing `SPELLCAST+0x30` instead of `+0x3c`. Together
/// the two are `TargetingWantsLocation 0x6e6320`'s `0x60`: the terrain click serves both.
const TF_SOURCE_LOCATION: u16 = 0x0020;
/// `TargetingWantsLocation 0x6e6320`'s whole mask (`6e6328: testb $0x60, %cl`) — the one
/// predicate the terrain seam asks the standing word.
const LOCATION_BITS: u16 = TF_SOURCE_LOCATION | TF_DEST_LOCATION;
/// The item bit — the *other* half of the targeting cursor. Its predicate twin is
/// `TargetingWantsItem 0x6e6330` (`flag_word & 0x4010`), which the bag and paper-doll click seams
/// consult before binding the clicked item ([`super::targeting`]).
const TF_ITEM: u16 = 0x0010;
/// `TARGET_FLAG_LOCKED` — the **shared** bit: it is in `TargetingWantsItem`'s `0x4010` *and* in
/// `TargetingWantsGameObject`'s `0x4800`, which is precisely how one armed lock spell serves both
/// legs. Mining, Herb Gathering, Opening, Pick Lock, Disarm Trap: one
/// spell, two clicks that can end it — a lockbox in your bag, or a lockable GameObject in the
/// world. Neither leg is a new wire shape; `BindTarget 0x6e5b40` picks its arm by the clicked
/// object's typemask and then **hardcodes** the outgoing bit — item arm `6e5f1e: testl $0x4010,
/// 0xcecac0` → `6e5f2e: orb $0x10, 0xceac5c` (ITEM); GameObject arm `6e5f60: testb $0x48, %ch` →
/// `6e5f69: orb $0x8, 0xceac5d` (GAMEOBJECT). `LOCKED` itself is never written to the wire mask
/// by either (decision 0939's census). vmangos agrees from the other side: `Spell::CheckCast`
/// accepts a LOCKED spell whose mask carries `TARGET_FLAG_ITEM | TARGET_FLAG_GAMEOBJECT`
/// (`Spell.cpp:6755`).
const TF_LOCKED: u16 = 0x4000;
/// `TARGET_FLAG_GAMEOBJECT` — the other bit of `TargetingWantsGameObject 0x6e62d0`'s `0x4800`, and
/// the one implicit arm 23 ORs in ([`cast_target_mask`]). A word carrying it arms the world click's
/// GameObject leg.
const TF_GAMEOBJECT: u16 = 0x0800;
/// The bits `BindTarget 0x6e5b40`'s **item** arm admits and then clears — `6e5f1e`'s
/// `testl $0x4010` and `6e5f36`'s `and $0xbfef` (bits 4 and 14). The same `0x4010` that
/// `TargetingWantsItem 0x6e6330` asks the standing word; here it is the *bind* side of it.
const ITEM_ARM_BITS: u16 = TF_ITEM | TF_LOCKED;

/// What the wire's target block should carry for this cast.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CastWireTarget {
    /// flag_word 0 — mask `TARGET_FLAG_SELF (0)`, no guid (the server resolves implicitly).
    SelfImplicit,
    /// A bound unit — mask `TARGET_FLAG_UNIT (0x2)` + this guid (possibly the player's own,
    /// via the autoSelfCast fallback).
    Unit(u64),
    /// A bound **item** — mask `TARGET_FLAG_ITEM (0x10)` + this item guid, sent on the press with
    /// no cursor in between. `BindTarget 0x6e5b40`'s non-unit item arm (`6e5f15`ff) reached
    /// straight from the cast-arm, which is the shape only the main-hand auto-pick below produces
    /// ([`CastCandidates::main_hand_item`]): every *other* item target in the game arrives through
    /// the targeting cursor's click seam instead ([`super::targeting`]).
    Item(u64),
    /// The cast awaits a click — no send yet: enter the targeting-cursor mode with **this standing
    /// flag_word** ([`super::targeting`]), which is the reference's own state (`0xcecac0` != 0 *is*
    /// `IsTargeting 0x6e48a0`). Each click seam then asks the word its own question, so one word
    /// can serve more than one seam: terrain (`& 0x60` → mask `0x40` + the point), bag / paper doll
    /// (`& 0x4010` → mask `0x10` + the item guid), world GameObject (`& 0x4800` → mask `0x800` +
    /// the GO guid). Blizzard and Flamestrike arm the first; poisons, oils and enchants the second;
    /// Opening and Pick Lock arm the second **and** the third at once.
    Targeting(u16),
    /// Nothing bindable — do NOT send; surface this client error instead. These are the INTERIM
    /// stand-ins for the unmodeled *unit* hand-cursor mode (module docs): the reference has no
    /// refusal at all where these fire, it leaves the word standing and asks for a click. So they
    /// stay where the resolver returns them.
    Refused(u8),
    /// A refusal the **reference actually raises**, from TryCast's tail after `ArmCast 0x6e5250`
    /// returns FALSE (`6e5045`–`6e507b`). That tail sits *below* the requirement validator
    /// `0x6094f0`, in the same place as the targeting-cursor arm it is the sibling of — so this
    /// one is parked and fired at the cursor-entry point, not at the resolver's return
    /// ([`super::send_spell_cast`]).
    RefusedAtArm(u8),
}

/// The three places `ArmCast 0x6e5250` can take its candidate guid from (`6e5361`–`6e539f`),
/// in the reference's own priority order. Named rather than positional because they are three
/// interchangeable `Option<u64>`s and a swap between them is silent.
///
/// The reference resolves **one** of these, not a sequence: the main-hand leg is taken *instead
/// of* the selection, never before it. (Our walk still tries `caster` after `selection` — that is
/// the autoSelfCast fallback at `6e53d7`, a separate rung further down.)
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CastCandidates {
    /// The current selection — the ref's global `0xb4e2d8` (`6e539f`), and the explicit-guid
    /// argument that precedes it (`6e5393`; every benilla caster passes zero there).
    pub(crate) selection: Option<u64>,
    /// The active player, for the autoSelfCast fallback (`6e53d7`).
    pub(crate) caster: Option<u64>,
    /// The equipped main hand's item guid — `[player+0x1d3c][15]`, filtered to a guid that
    /// actually resolves to a live object (the ref's `0x468460(typemask 1)` at `6e53bc`).
    /// `None` when the slot is empty or the item has not streamed, which is the ref's
    /// esi:edi = 0 and ends in the item cursor exactly as it does here.
    pub(crate) main_hand_item: Option<u64>,
}

/// Everything the binder's relation checks read. Both call sites (action bar, spellbook) bundle
/// the same resources; the stores are the *selected* unit's and the player's.
#[derive(Clone, Copy)]
pub(crate) struct TargetRelations<'a> {
    pub(crate) target_store: Option<&'a ObjectStore>,
    pub(crate) self_store: Option<&'a ObjectStore>,
    pub(crate) factions: Option<&'a Factions>,
    pub(crate) reputations: &'a Reputations,
}

/// The targeting inputs [`super::send_spell_cast`] resolves with, bundled by its two callers
/// (the action bar's drain, the spellbook's) so the ONE cast path owns the whole ArmCast walk.
pub(crate) struct CastContext<'a> {
    pub(crate) selection_guid: Option<u64>,
    pub(crate) self_guid: Option<u64>,
    pub(crate) auto_self_cast: bool,
    pub(crate) rel: TargetRelations<'a>,
    /// The local range gate's inputs (`IsTargetInRange 0x6e47b0` over `GetMinMaxRange
    /// 0x6e3480`) — the caster's and the selection's position + combat reach.
    pub(crate) range: RangeInputs,
    /// The equipped main hand's item guid, straight off the player descriptor —
    /// `PLAYER_FIELD_INV_SLOT_HEAD + 2*15`, the client's `[player+0x1d3c][15]`. `None` when the
    /// slot is empty. The *liveness* filter (does the guid name a streamed item object?) is the
    /// send path's, where the item cache is in scope.
    pub(crate) main_hand_item: Option<u64>,
    /// The caster's live CMovement flags word (the client's `[unit+0x9e8]`,
    /// [`crate::creature_anim::move_flags`] layout) — what the requirement validator's moving
    /// gate reads at cast initiation.
    pub(crate) self_move_flags: u32,
}

/// Positions + combat reaches for the pre-send range refusal ([`super::send_spell_cast`]'s
/// `cast_range_refusal` leg — the client's `TryCast` runs `CanTargetUnit 0x6e4440` →
/// `IsTargetInRange 0x6e47b0` BEFORE the cast commit, so an out-of-range/too-close press
/// refuses locally and none of the commit tail (the ranged sheath snap included) runs.
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
            // The descriptor default reach (the state feed's own fallback).
            self_reach: 1.5,
            target_reach: None,
        }
    }
}

/// Everything a cast-sending system needs to build a [`CastContext`], as ONE [`SystemParam`] —
/// both drains stay under Bevy's system-arity ceiling and can't drift apart on inputs.
#[derive(SystemParam)]
pub(crate) struct CastTargeting<'w, 's> {
    pub(crate) selection: Res<'w, Selection>,
    pub(crate) self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_guid: Res<'w, SelfGuid>,
    auto_self_cast: Res<'w, AutoSelfCast>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    self_transform: Query<'w, 's, &'static Transform, With<SelfPlayer>>,
    transforms: Query<'w, 's, &'static Transform>,
    /// `Option`: the body exists only in world, and `HandleCastResult` is a one-shot handler
    /// that must fetch its parameters on any built client (the reply casts the
    /// `modalNextSpell` chain through the ladder). No body, no movement: the moving gate passes
    /// and the server stays the net, as for every other untestable input.
    player: Option<Res<'w, crate::player::Player>>,
}

impl CastTargeting<'_, '_> {
    /// The current frame's [`CastContext`] — the selection's and player's stores resolved.
    pub(crate) fn context(&self) -> CastContext<'_> {
        let target_store = self.selection.target.and_then(|e| self.stores.get(e).ok());
        CastContext {
            selection_guid: self.selection.guid,
            self_guid: self.self_guid.0,
            auto_self_cast: self.auto_self_cast.0,
            rel: TargetRelations {
                target_store,
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

/// Equipment slot 15 — `EQUIPMENT_SLOT_MAINHAND`. The client reaches it as byte offset `0x78`
/// into the player's inventory guid table (`6e538b: mov esi,[ecx+0x78]`, `0x78 == 15 * 8`).
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;

/// The `autoSelfCast` knob (the ref's CVar, default `"0"`). benilla defaults it **on** — a named
/// deviation: with it off, an unbindable friendly cast falls into the ref's targeting-cursor
/// machine, which benilla doesn't model yet, leaving no path at all. Flip the default to the
/// ref's when spell targeting-cursor mode lands.
#[derive(bevy::prelude::Resource)]
pub(crate) struct AutoSelfCast(pub(crate) bool);

/// `autoSelfCast`'s change callback: a flag.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut auto: ResMut<AutoSelfCast>) {
    if ev.is("autoSelfCast") {
        auto.0 = ev.flag();
    }
}

impl Default for AutoSelfCast {
    fn default() -> Self {
        Self(true)
    }
}

/// The cast-arm's flag_word seed + implicit-target switch (`0x6e5250` @ `6e525a`–`6e52ef`):
/// `flag_word = Targets`, then one arm keyed on `EffectImplicitTargetA[0]`. The full arm map:
/// 1→clr bit10, 5→clr bit15, 6/53→set bit7, 16→ground-target,
/// 21/45→set bit8, 23→set bit11, 25/63→set bit1, 26→set bit14, 35→set bit3, 57/61→set bit2;
/// every other enum is the default no-op arm.
pub(crate) fn cast_target_mask(def: &SpellDisplay) -> u16 {
    let mut word = def.targets as u16;
    match def.implicit_target_a1 {
        1 => word &= !TF_EXPLICIT_GATE,
        5 => word &= !TF_CORPSE_ALLY,
        6 | 53 => word |= TF_UNIT_ENEMY,
        // 16 (ground-target) sets the cursor-mode flag (the ref's `bl`), not a word bit — the
        // location bits 0x60 usually arrive via `Targets` itself; both resolve to
        // `GroundTargeting` in [`resolve_cast_target`].
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

/// `BindTarget 0x6e5b40`'s unit branch for one candidate: clear every flag_word bit the unit
/// satisfies (each bit its own relation check, in the binder's priority order); the caller
/// commits only on a fully-cleared word.
///
/// Relation stand-ins, named: assist (`CanAssist 0x6066f0`) is approximated as reaction rank ≥ 4
/// (friendly) — the same `UnitReaction` core the ring and `can_attack` share — until CanAssist
/// is pinned; party/raid (`0x606c20`/`0x606d20`) accept only the player himself until groups
/// exist; the corpse predicate (`0x6067d0`) is "assistable and health 0".
fn clear_satisfied_bits(word: u16, is_self: bool, rel: &TargetRelations) -> u16 {
    let mut word = word;
    let reaction = ring_reaction(
        rel.factions,
        rel.reputations,
        rel.target_store,
        rel.self_store,
    );
    let assist = is_self || reaction >= 4;
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
    // Generic UNIT (bit 1) — the binder's check is the unit-flag leg, no relation: any resolved
    // unit binds. The explicit-selection gate (bit 10) carries no guid of its own; a real
    // explicit candidate discharges it alongside any unit bind.
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

/// Resolve the wire target for casting `def` with `selection` as the current target — the
/// ArmCast walk. `None` def (unknown spell) keeps the legacy shape: the raw selection, or
/// self-implicit without one (the server still validates).
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
    // Arm 16's ground fast-defer (`6e52db` sets bl, `6e535b` returns-to-cursor): a ground-arm
    // spell (Flamestrike) drops into targeting-cursor mode BEFORE any candidate bind — but only
    // after the word==0 immediate-commit above, whose order the ref fixes (`6e5338` precedes the
    // bl test).
    if def.implicit_target_a1 == 16 {
        // The ref's arm-16 defer is a cursor-mode flag (`bl`), not a word bit — but our seams read
        // only the word, so the DEST bit is what says "the terrain click owns this one". The two
        // agree on shipped data (arm-16 rows carry `Targets & 0x40` anyway); the OR is what keeps
        // them agreeing if one ever doesn't.
        return CastWireTarget::Targeting(word | TF_DEST_LOCATION);
    }
    // ArmCast's FIRST candidate leg (`6e5361`–`6e5391`) — the one that makes a
    // weapon imbue cast-and-done. A player caster pressing a spell with `Attributes & 0x200`
    // takes its candidate from the equipped **main hand** (`[player+0x1d3c][15]`) rather than
    // from the explicit guid or the selection, and hands it straight to `BindTarget 0x6e5b40`.
    // The caster-is-a-player test at `6e5361` is structural here: this resolver only ever runs
    // for the local player's own press.
    //
    // It must sit ABOVE the non-unit defer below, which is where every item word currently ends
    // up. That defer's premise — "no candidate can clear these bits" — is true only of *unit*
    // candidates; an item candidate clears bit 4, which is precisely why Rockbiter and Windfury
    // Weapon were raising a cursor over a weapon the client already knew how to pick.
    if def.targets_main_hand_item() {
        match cand.main_hand_item {
            // `BindTarget`'s item arm: `6e5f1e: testl $0x4010` admits it, `6e5f2e` ORs
            // `TARGET_FLAG_ITEM` into the wire mask, `6e5f35` clears word bits 4 and 14 (`and
            // $0xbfef`). A word that clears to zero commits on the spot (`6e60c1` → `SendCast
            // 0x6e54f0`).
            Some(item) if word & ITEM_ARM_BITS != 0 && word & !ITEM_ARM_BITS == 0 => {
                return CastWireTarget::Item(item)
            }
            // **Nothing in the main hand — and this is NOT a cursor** (correcting
            // 1552). The guid reads 0, `0x468460` resolves no object, `BindTarget` bails writing
            // nothing, and ArmCast returns FALSE. TryCast's tail then tests the SAME attribute
            // bit a second time (`6e504b: test ch,0x2`), **clears the flag_word outright**
            // (`6e5057: mov WORD ds:0xcecac0,0`) and raises reason `0x2d` — jumping clean over
            // the cursor arm at `6e50ab`. "Your weapon hand is empty", no cursor, no packet.
            None => return CastWireTarget::RefusedAtArm(ERR_MAINHAND_EMPTY),
            // The main hand is there but the word carries a residual the item arm cannot
            // discharge. The ref binds the item, `BindTarget` returns TRUE, ArmCast returns TRUE
            // — so TryCast's tail is never reached and the word waits for the rest at the cursor.
            // Unreachable on shipped data: all 22 attribute-bearing rows are `Targets == 0x10`
            // exactly (censused in `benilla-formats`), which is what the defer below assumes.
            Some(_) => {}
        }
    }
    // Bits outside the unit family (item/gameobject/location/string) have no candidate here.
    // A location word — Blizzard's bare `Targets = 0x40`, Martin Fury's bare `0x20` — is the
    // targeting cursor's location half: in the ref it falls out of the
    // failed bind walk into cursor mode (`6e50c8`), and deferring before the unit walk is
    // byte-equivalent **only while no unit bit stands beside it**, which is why the location arm
    // below carries that condition where the item/GO one is a bare mask test. One shipped row
    // breaks the old "never combined" reading (`Summon Fallout Slime` 28218, word `0x42`,
    // NPC-cast): there the ref binds the unit at arm time and the cursor takes only the residue,
    // so that word keeps the refusal rather than guessing at a shape nothing can reach.
    if word & !UNIT_BITS != 0 {
        // Hand the whole word to the cursor when any seam can serve it — written as the
        // reference's own three predicates rather than as equalities on bare words. The reference
        // never decides this at arm time at all: it lets the bind walk fail, leaves the word
        // standing, and asks at *click* time. Forking here is byte-equivalent for a structural
        // reason, not a data one — **no unit candidate can ever clear any of these bits**
        // ([`clear_satisfied_bits`] only clears the unit family), so a word carrying one always
        // survives the walk and always ends in cursor mode.
        //
        // Which is exactly why the word travels instead of a verdict about it. The LOCKED family's
        // word is not bare: 100 of its 103 rows carry implicit arm 23, whose overlay ORs in
        // `TF_GAMEOBJECT`, and one carries arm 25 (`TF_UNIT`). An equality test refused every one
        // of them (0928's live probe: a Dull Iron Key drew "Invalid target" instead of the
        // cursor) — and a single-verdict enum then had to *choose* item-or-world for a word the
        // reference lets satisfy both. Both bits ride along; the click picks the leg.
        if word & LOCATION_BITS != 0 && word & UNIT_BITS == 0
            || word & (TF_ITEM | TF_LOCKED | TF_GAMEOBJECT) != 0
        {
            return CastWireTarget::Targeting(word);
        }
        return CastWireTarget::Refused(ERR_INVALID_TARGET);
    }
    // Candidate: the current selection — ArmCast's explicit-guid leg (`6e5393`) falling through
    // to the current-target global `0xb4e2d8` (`6e539f`). Reached only when the main-hand leg
    // above did not claim the cast.
    if let Some(guid) = cand.selection {
        let is_self = cand.caster == Some(guid);
        if clear_satisfied_bits(word, is_self, rel) == 0 {
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
            if clear_satisfied_bits(word, true, &self_rel) == 0 {
                return CastWireTarget::Unit(guid);
            }
        }
    }
    // The ref's residual-word targeting-cursor mode, refused locally (module docs).
    CastWireTarget::Refused(if cand.selection.is_some() {
        ERR_INVALID_TARGET
    } else {
        ERR_NO_TARGET
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two unit-side candidates, named — the third (`main_hand_item`) is `None` for every
    /// test that predates the imbue leg.
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

    /// The switch arms against their pinned rows: self/party-area spells zero out, single-enemy
    /// sets the hostile bit, single-friend the assist bit, and the `Targets` seed survives.
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

    /// The three wire shapes without any world state: mask 0 self-commits (never the selection —
    /// the Battle Shout/Ice Armor bug), unit masks refuse without a candidate, and the no-target
    /// vs wrong-target refusals use the client's two error strings.
    ///
    /// Both stores are present but empty, because `CanAttack` refuses outright when either side is
    /// missing (1674) and this test is about the wire shapes, not about that posture.
    #[test]
    fn resolution_wire_shapes() {
        use benilla_protocol::ObjectFields;
        // `UNIT_FIELD_FLAGS` bit 3 (player-controlled) on us alone selects `CanAttack`'s mixed
        // arm; with no catalog the reaction resolves neutral, so the enemy bit clears.
        let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(46, 1 << 3)]));
        let it = crate::net::ObjectStore(ObjectFields::from_pairs(&[(35, 0)]));
        let rel = TargetRelations {
            target_store: Some(&it),
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
            CastWireTarget::Refused(ERR_NO_TARGET)
        );
        // With no catalog the reaction resolves neutral (3): the mixed arm's `< 4` is attackable,
        // and `CanAssist`'s `>= 4` is not.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
        let intellect = spell(0, 21);
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "a friendly-required cast on a non-friend falls back to self"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(Some(42), Some(1)), false, &rel),
            CastWireTarget::Refused(ERR_INVALID_TARGET),
            "autoSelfCast off: the fallback is gated"
        );
        assert_eq!(
            resolve_cast_target(Some(&intellect), &cands(None, Some(1)), true, &rel),
            CastWireTarget::Unit(1),
            "no selection at all still self-falls-back"
        );
        // A hostile-required cast never self-binds: player fails CanAttack.
        assert_eq!(
            resolve_cast_target(Some(&fireball), &cands(None, Some(1)), false, &rel),
            CastWireTarget::Refused(ERR_NO_TARGET)
        );
        // Unknown spell: the legacy passthrough.
        assert_eq!(
            resolve_cast_target(None, &cands(Some(42), Some(1)), true, &rel),
            CastWireTarget::Unit(42)
        );
    }

    /// The ground family resolves to `GroundTargeting` by BOTH routes: the
    /// arm-16 fast-defer (Flamestrike: `Targets 0x40`, implicit 16 — and even with a selection,
    /// the ref defers before any candidate bind) and the bare DEST word falling out of the bind
    /// walk (Blizzard: `Targets 0x40`, implicit 28 = default arm). The word==0 immediate commit
    /// still precedes the arm-16 check, as the ref orders them (`6e5338` before the bl test).
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
            "word==0 commits before the arm-16 defer — the ref's order"
        );
    }

    /// **The SOURCE word raises the cursor, exactly as the DEST word does** (
    /// closing B388). `TargetingWantsLocation 0x6e6320` is `word & 0x60` — one predicate over both
    /// bits — and `BindLocation 0x6e60f0` has an arm for each, so a bare `0x20` is a terrain click
    /// waiting to happen, not a refusal. Martin Fury's spell 265 (`Targets 0x20`, implicit 15 =
    /// default arm) is the shipped case; a selection changes nothing, since no unit candidate can
    /// clear a location bit.
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
        // The pair, the shape `BindLocation` walks across two clicks. No 5875 row carries it; the
        // resolver still hands the whole word over rather than choosing a leg here.
        let both = spell(0x60, 0);
        assert_eq!(
            resolve_cast_target(Some(&both), &cands(None, Some(1)), true, &rel_none()),
            CastWireTarget::Targeting(LOCATION_BITS)
        );
    }

    /// What still refuses rather than shipping a guess — the machines named in the module docs.
    /// STRING (bit 13) has no seam at all; a location bit with a **unit** bit beside it (`0x42`,
    /// `Summon Fallout Slime` 28218, NPC-cast) is the one shipped word where deferring before the
    /// unit walk would NOT be byte-equivalent: the reference binds the unit at arm time and the
    /// cursor takes only the residue.
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

    /// The seam predicates are MASKS — `word & 0x4010` for the bag click, `word & 0x4800` for the
    /// world click — so every word carrying one of their bits raises the cursor, not just the bare
    /// ones. That distinction is the whole of the LOCKED family: 100 of its 103 rows carry implicit
    /// arm 23, whose overlay ORs the GAMEOBJECT bit into the word (`0x4800`), and one carries arm
    /// 25 (`0x4002`). An equality test refused all of them, which is what a live key-on-lockbox
    /// probe caught.
    ///
    /// And the **word travels**: the resolver hands the cursor the whole thing rather than a
    /// verdict, so a `0x4800` word can answer the bag click *and* the world click. Deciding here
    /// would have to pick one, and the reference picks neither until the click lands.
    ///
    /// A selected unit changes nothing in any case: none of these is a bit a unit candidate can
    /// clear, so the walk can never discharge the word. The word==0 immediate commit still precedes
    /// all of it.
    #[test]
    fn the_seam_predicates_are_masks_and_the_word_travels() {
        // Raw `Targets`, then the shapes `cast_target_mask`'s implicit overlay actually produces on
        // the shipped LOCKED rows (arm 23 → `|0x800`, arm 25 → `|TF_UNIT`), then the bare
        // GAMEOBJECT word arm 23 puts on a spell whose `Targets` carries nothing else.
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
        // The shipped lock word: one cursor, and it carries both seams' bits at once.
        let opening = cast_target_mask(&spell(0x4000, 23));
        assert_eq!(opening & (TF_ITEM | TF_LOCKED), TF_LOCKED);
        assert_eq!(opening & (TF_GAMEOBJECT | TF_LOCKED), opening);
    }

    /// The weapon-imbue leg (ledger B308): `Attributes & 0x200` makes the cast
    /// bind the equipped main hand and send on the press — no cursor, no click on the bag.
    ///
    /// Rockbiter Weapon 8017's real shape: `Targets 0x10`, `Attributes 0x50200`, implicit arm 0.
    /// The three things that must stay true around it are the interesting part: the selection is
    /// irrelevant (a targeted enemy neither breaks the imbue nor gets it cast at them), an empty
    /// main hand falls back to the ordinary item cursor rather than sending a guess, and the
    /// *same word without the attribute* — every poison, oil, stone and enchant — still raises the
    /// cursor exactly as before.
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
        // Unarmed: esi:edi = 0 resolves no object and the bind writes nothing, so ArmCast
        // returns FALSE — and TryCast's SECOND test of the same bit (`6e504b`) clears the word
        // and refuses. Emphatically **not** a cursor: that arm (`6e50ab`) is jumped over. This
        // assertion is decision 1554 correcting 1552, which had it as the item cursor.
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
        // The attribute is the whole difference: a sharpening stone / poison / oil / enchant is
        // the same `Targets 0x10` word and must still ask for its item.
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
            self_store: None,
            factions: None,
            reputations: &EMPTY,
        }
    }
}
