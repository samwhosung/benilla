//! The context-sensitive **world cursor** — the classifier half.
//!
//! Each frame, the hovered unit resolves to a [`CursorKind`] the way the real client's
//! `CGWorldFrame` classifier does (`0x4828d0` → unit branch `0x482200`):
//! - an **interactable NPC** (service flags, not hostile) → the `UNIT_NPC_FLAGS` service ladder
//!   (`0x482336..0x4824e3`), lowest bit wins — the full statically-unrolled map is
//!   [`service_cursor`]. Notably vendor → **Pickup** (the pouch), innkeeper → **Interact**,
//!   banker/auctioneer → **Buy**; REPAIR (0x4000) is *never consulted* — a repair-only unit falls
//!   through to the attack/clear leg (real repairers all carry VENDOR too).
//! - otherwise **loot / skin / attack** keyed on state: dead + `UNIT_DYNFLAG_LOOTABLE` →
//!   **Pickup or LootAll** — the loot leg's mode is `8 + (keyDown(0) ? 8 : 0)` (`0x48252c`):
//!   the triple-pouch LootAll(16) while the auto-loot modifier is held, generalized since 0961's
//!   auto-loot to the EFFECTIVE state (`autoLootDefault` XOR shift — [`loot_cursor`]); dead +
//!   `UNIT_FLAG_SKINNABLE` → Skin; alive and attackable → Attack.
//! - **`Unable*` (grayed) by a different gate per mode** (byte-verified): NPC services gray beyond
//!   **5.5556 yd** (`0x482320`); attack beyond a fixed **10.45 yd** (`0x4826a7`); skin outside the
//!   melee interact reach `max(reachA + reachB + 1.333, 5.0)` — the 5.0 is a **floor**, not a cap
//!   (`0x6e3480` for skin; the same formula inline in `CanLootNow 0x5ec110` @ `0x5ec142..0x5ec1c8`
//!   for loot, center-to-center, boundary-inclusive — director-measured ~5 yd, byte-confirmed).
//!   Loot *rights* never gray — they gate whether the loot cursor shows at all; the mid-loot state
//!   block and the open-loot-window able-override are not modeled.
//!
//! **Both branch predicates are the reference's own, not thresholds** (decision 1674): the
//! service/loot split is `CanInteract 0x6067f0` (`0x482310`, via the `CanInteractNow 0x606880`
//! wrapper) and the sword is `CanAttack 0x606980` (`0x48269a`) — [`super::can_interact`] /
//! [`super::can_attack`], both byte-verified complete. That matters because each reads
//! `UnitReaction` in the **player → unit** direction, which answers a reputation-slot faction with
//! the **at-war bit** and never the standing. The reaction-rank approximation these legs used to
//! carry read the *other* direction, and drew the sword over every not-at-war neutral faction —
//! a Cenarion Circle druid, a Booty Bay goblin, an Argent Dawn quartermaster. (The questgiver
//! bit's own quest-status gate, `0x5df490`, and the auto-loot Pickup/LootAll split, `0x41f8f0`,
//! both used to be listed here as unmodelled; both are modelled now — [`questgiver_has_quest`],
//! [`loot_cursor`].)

use bevy::prelude::*;

use crate::net::{ObjectStore, Reputations, SelfPlayer};

use super::ring::Factions;
use super::{go_is_nearest, Hovered, HoveredObject};

/// The blp-name set of world cursor modes benilla can currently trigger (named off the client's own
/// mode table `0x853b8c` — the strings are the `Interface\Cursor\<Name>.blp` stems).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CursorKind {
    Point,
    Attack,
    Speak,
    /// Pickup(8) — the vendor's pouch AND the loot leg's base mode.
    Pickup,
    /// LootAll(16) — the loot leg's triple pouch while the EFFECTIVE auto-loot is on
    /// (`autoLootDefault` XOR the held shift, 0961's own rule — in 1.12 the held key alone was
    /// the whole mechanism, `8 + (keyDown(0) ? 8 : 0)` @ `0x48252c`). Loot only: the vendor
    /// pouch never triples.
    LootAll,
    /// Interact(5) — the generic gear. A GameObject base type's cursor when it carries no
    /// data-named cursor (a door, button, chest, keyed/keyless lock, fishing, …); also the
    /// innkeeper service.
    Interact,
    Buy,
    /// Inspect(7) — the magnifier. The UI's Ctrl-hover cursor (`ShowInspectCursor 0x48ac60`,
    /// overlaid by [`crate::cursor`]) **and** the world cursor over a
    /// readable TEXT(9) GameObject plaque (`0x5f5890`).
    Inspect,
    Trainer,
    Taxi,
    Skin,
    /// Repair(17) — never set by the world classifier (the ladder skips the REPAIR bit); it is
    /// the UI's repair-mode base cursor (`ShowRepairCursor 0x4fbcc0`'s locked mode), overlaid by
    /// [`crate::cursor`].
    Repair,
    /// Mail(15) — a MAILBOX(19) / RITUAL(18) / type-28 GameObject (the shared
    /// `0x5f6840`/`0x5f6e30` behavior). The mailbox's own cursor, not the gear.
    Mail,
    /// Mine(11) — a GameObject whose lock's first `LockType` is Mining (3). A LockType.dbc
    /// data-named cursor (`0x5f3070`), resolved off the GO's lock, not a fixed type.
    Mine,
    /// GatherHerbs(13) — a GameObject whose lock's first `LockType` is Herbalism (2). Also a
    /// LockType.dbc data-named cursor.
    GatherHerbs,
    /// PickLock(14) — a GameObject whose lock's first `LockType` is Pick Lock (`LockType.Id == 1`).
    /// The one data-named GO cursor that is **never grayed** (`0x5f3070`: `LockType.Id == 1` skips
    /// the usable gate), since any rogue can attempt the lock.
    PickLock,
    /// Cast(2) — the spell-targeting cursor (`0x4820f0`): while a
    /// spell awaits a target, dispatcher step 2 pre-empts the WHOLE object classifier with
    /// Cast/UnableCast. Never set by the classifier here — it is [`crate::cursor`]'s
    /// armed-enchant-pick overlay (the one spell-targeting state benilla ships).
    Cast,
}

impl CursorKind {
    /// The cursor's BLP stem in `Interface\Cursor\` (the client's mode-name table strings).
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
        }
    }
}

/// The resolved world cursor for this frame — what the OS cursor should show. Written by
/// [`classify_cursor`] (after hover), read by [`crate::cursor`]'s platform drivers. `unable`
/// selects the grayed `Unable<Name>` twin (`mode + 20` in the client's enum — out of range).
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
    /// The BLP file stem (`Attack` / `UnableAttack` / …) — the key the platform cursor caches use.
    pub(crate) fn stem(&self) -> String {
        if self.unable && self.kind != CursorKind::Point {
            format!("Unable{}", self.kind.name())
        } else {
            self.kind.name().to_string()
        }
    }
}

/// Vanilla `UNIT_NPC_FLAGS` bits (vmangos `UnitDefines.h`, 1.12 values — later expansions differ).
/// REPAIR (0x4000) exists but the classifier never consults it (falls `je 0x4826cb`).
/// `pub(crate)` so the right-click dispatch ([`super::click`]) reuses BANKER to split the shared
/// Buy cursor kind (banker vs auctioneer) without a duplicate table (decision 0604), and so a live
/// probe scans for a service NPC by the same bits the cursor classifies with instead of keeping a
/// private copy ([`crate::capture::ProbeCharterPlugin`] and PETITIONER) — a duplicated flag table
/// is how B249's icon map went stale.
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

/// `UNIT_FLAG_SKINNABLE` in `UNIT_FIELD_FLAGS` (vanilla).
pub(super) const UNIT_FLAG_SKINNABLE: u32 = 0x0400_0000;

/// NPC-service range gate: gray beyond 5.5556 yd (squared 30.864 — the client's `0xb4b32c` cell,
/// `[0x804328]²`; checked at `0x482320`, boundary-inclusive). Shared with the merchant window's
/// out-of-range auto-close ([`crate::ui_merchant`]) so the window closes exactly where the cursor
/// says the vendor is out of service.
pub(crate) const SERVICE_RANGE_SQ: f32 = 30.864;
/// Attack's fixed range gate: gray beyond 10.45 yd (squared 109.2025, const `0x80447c`, checked at
/// `0x4826a7` — *not* the melee reach; that gates skin/insignia only).
const ATTACK_RANGE_SQ: f32 = 109.2025;
/// The melee interact reach offset + floor (`0x80b058` / `0x80a1e8`): reach = **max**(rA + rB +
/// 1.333, 5.0) — the 5.0 is a floor (`fcomp`-then-keep-larger, `0x6e35bf` / `0x5ec1a4`), so small
/// pairs always get 5 yd and large creatures reach *farther*. Gates skin (`0x6e3480`) and loot
/// (inline in `CanLootNow 0x5ec110`); distance is center-to-center, boundary-inclusive.
const MELEE_OFFSET: f32 = 1.333_33;
const MELEE_FLOOR: f32 = 5.0;

/// A **corpse object**'s interact reach, squared — `25.0`, i.e. a flat **5.0 yd**.
///
/// The same `max(rA + rB + 1.333, 5.0)` shape as [`MELEE_FLOOR`] above, evaluated for a corpse:
/// a corpse descriptor carries no `UNIT_FIELD_COMBATREACH` at all, so the sum can never clear the
/// floor and the gate is the floor. Two independent byte facts agree on the number — the cursor
/// leg's `CanLootNow 0x5ec110`, and the `CMSG_LOOT` sender `0x5df130`, which compares `dist²`
/// against a literal `25.0`. Written as the constant rather than the formula because the formula's
/// other term does not exist here.
/// Boundary-inclusive, center-to-center, like every other reach gate in this file.
const CORPSE_INTERACT_RANGE_SQ: f32 = 25.0;

/// `GAMEOBJECT_TYPE_GENERIC` (vmangos `SharedDefines.h`) — world decoration whose highlightable
/// predicate is constant-false, so it never shows an interact cursor (`0x5f47f0`).
pub(crate) const GO_TYPE_GENERIC: i32 = 5;
/// The transport family — TRANSPORT(11), MAP_OBJECT(14), MO_TRANSPORT(15): their strategy vtables'
/// highlightable slot (+0x14) is constant-false too (`32 c0 c3` — vtable dump from the 5875 binary:
/// `0x80ba58+0x14`→`0x5f5c70`, `0x80b710`/`0x80b798+0x14`→`0x5f48b0`), so a boat / zeppelin /
/// elevator never shows a cursor, tooltip, or right-click USE.
const GO_TYPE_TRANSPORT: i32 = 11;
const GO_TYPE_MAP_OBJECT: i32 = 14;
const GO_TYPE_MO_TRANSPORT: i32 = 15;
/// The **marker set** — SPELL_FOCUS(8), DUEL_ARBITER(16), FISHINGHOLE(25), AURA_GENERATOR(30).
/// Their strategy vtables are the mirror image of the transports': `+0xc` (mouseover eligibility) is
/// a constant `b0 01 c3` = `mov al,1; ret` and `+0x14` (**highlightable**) a constant
/// `32 c0 c3` = `xor al,al; ret` (vtables `0x80b8a8`/`0x80bb68`/`0x80bbf8`/`0x80c2a0`, `+0xc`
/// targets `0x5f57e0`/`0x5f65f0`/`0x5f6660`/`0x5f6ea0`, `+0x14` targets
/// `0x5f57f0`/`0x5f6600`/`0x5f6670`/`0x5f6eb0` — const bytes read for all four).
///
/// So they hover but never *interact*: the anvil, the forge, the campfire, the duel flag, the fishing
/// school publish a mouseover (gold-name tooltip) and then show **cursor mode 0** — the plain
/// pointer, not the Interact gear — take no `+64` brighten, and swallow the right-click
/// (`OnUse 0x5f8660` gates on the same `+0x14`). Without this arm they fell through to
/// [`highlightable_flags`], whose flag/faction terms a SPELL_FOCUS passes trivially (`flags 0x0`),
/// and every anvil in the world wore the gear.
const GO_TYPE_SPELL_FOCUS: i32 = 8;
const GO_TYPE_DUEL_ARBITER: i32 = 16;
const GO_TYPE_FISHINGHOLE: i32 = 25;
const GO_TYPE_AURA_GENERATOR: i32 = 30;
/// The highest GAMEOBJECT_TYPE_ID the type factory has a `case` for. The jump table `0x5f76cc`
/// covers 0..=30; **21 GUARDPOST has no case of its own** and falls to `default:` with everything
/// out of range — the arms' `__LINE__` pushes step three apart across the 30 real types with none
/// for 21.
const GO_TYPE_MAX: i32 = 30;
/// `GAMEOBJECT_TYPE_GUARDPOST` (21) — see [`strategy_is_default`].
const GO_TYPE_GUARDPOST: i32 = 21;

/// Whether the type factory hands this type the **default** strategy — the shared static placeholder
/// rather than a type of its own. Type 21 and every out-of-range id take the `default:` arm
/// `0x5f76a0`, which allocates nothing: `mov dword ptr [esi+0x210], 0xc4d840` parks one static in
/// `.bss` (given vtable `0x80b188` by the initializer `0x5f36e0`) and logs `"BADBASEGAMEOBJECT|%d"`.
/// That vtable's `+0x14` is `0x5f36f0` = `xor al,al`, and its `+0xc` forwards to it — so a
/// default-strategy object is neither highlightable nor mouseover-eligible: it shows nothing at all.
///
/// Unobservable on this server — vanilla ships no type-21 template and a 1.12 server sends nothing
/// out of range — but modelled rather than left to the flag predicate, which would answer `true`
/// for an all-zero descriptor.
fn strategy_is_default(type_id: i32) -> bool {
    type_id == GO_TYPE_GUARDPOST || !(0..=GO_TYPE_MAX).contains(&type_id)
}
/// `GAMEOBJECT_TYPE_TEXT` (9) — a readable book/plaque/sign; its per-type behavior shows the
/// **Inspect** magnifier (`0x5f5890`), not the gear. `pub(super)` so the
/// right-click dispatch ([`super::act_on_right_click`]) routes it to the client-side reader off the
/// one type constant (decision 1105), the same shape as the mailbox — and `pub(crate)` beyond that
/// so the inspector's GO card can report the readable head against the same constant.
pub(crate) const GO_TYPE_TEXT: i32 = 9;
/// The three GameObject types that show the **Mail** cursor: RITUAL(18)
/// and MAILBOX(19) share one behavior (`0x5f6840`), and type 28 (`0x5f6e30`) resolves to Mail too.
/// Type 28 has no live 1.12 data but is included for byte-fidelity with the factory switch.
const GO_TYPE_RITUAL: i32 = 18;
/// `pub(super)` so the right-click dispatch ([`super::act_on_right_click`]) reuses the one type
/// constant to route a mailbox to the client-side window open (decision 0544), not a duplicate.
pub(super) const GO_TYPE_MAILBOX: i32 = 19;
const GO_TYPE_28: i32 = 28;
/// Interim GameObject interact-range gray (decision 0236): reuse the ~5.56 yd service reach until the
/// size-dependent GO interact distance is byte-pinned. Squared, boundary-inclusive like the unit gates.
const GO_INTERACT_RANGE_SQ: f32 = SERVICE_RANGE_SQ;
/// `GAMEOBJECT_TYPE_FISHINGNODE` (17) — the fishing bobber. Its strategy (vtable `0x80bc80`)
/// overrides exactly one consumed slot: highlightable is the channel-ownership gate
/// ([`highlightable_flags`]'s type-17 arm); everything else is the shared base.
pub(crate) const GO_TYPE_FISHINGNODE: i32 = 17;
/// The per-type interact range the shared `usable 0x5f3130` compares — the strategy ctor's
/// `[strat+0xc]` constant, **squared at the compare**, boundary-inclusive. FISHINGNODE's ctor sets
/// **100.0 yd** (`0x5f66b0`, `[0x80b0b0]`) — the bobber is effectively un-range-gated for any real
/// cast, never the ~5.56 yd reach. Other types stay on the 0236 interim (the base default is 5.0
/// and several per-type ctors override it — 5.5556/10.0/…; their population is a later byte-pin).
/// `GAMEOBJECT_TYPE_CHAIR` (7) — the second member of the per-type table (decision 1464). Its
/// `+0x18` predicate is its own `0x5f5670`, which reports **3.0 yd**
/// and accepts on `dist² < 9.0` against `[0xc4d808]` — whose only writer image-wide is the static
/// initializer `0x5f98b0` squaring that same 3.0. Chair-exclusive, and it independently reproduces
/// vmangos's `MAX_SITCHAIRUSE_DISTANCE`, which is why it is safe to hold as *client* law rather
/// than a transcribed server number (the B158 trap).
pub(crate) const GO_TYPE_CHAIR: i32 = 7;
fn go_interact_range_sq(type_id: i32) -> f32 {
    match type_id {
        GO_TYPE_FISHINGNODE => 100.0 * 100.0,
        // The chair is the one type whose reach is *shorter* than the 0236 interim, so it is the one
        // where the interim was visibly wrong: from 3 to ~5.56 yd we showed a live interact cursor
        // and sent a `CMSG_GAMEOBJ_USE` the server drops on the floor (its own chair arm refuses
        // past 3.0 to the nearest seat), so the click did nothing and said nothing. Now the cursor
        // greys where the reference greys it.
        //
        // **Measured to the GameObject, not to the seat.** The reference runs its compare per seat,
        // over the 5 slots `0x5f5760` builds at template bind; vmangos likewise measures to
        // `GetClosestChairSlotPosition`. Seats spread along `orientation + PI/2` at
        // `size·i − size·(slots−1)/2`, so for a 1-slot chair — the common case, and the B79 chair —
        // the seat *is* the object and the two agree exactly; a long bench differs by at most half
        // its span. Closing that gap needs the seat table, which needs `data0`/`data1` off the GO
        // template; it is a refinement of this number, not a correction to it.
        GO_TYPE_CHAIR => 9.0,
        _ => GO_INTERACT_RANGE_SQ,
    }
}
/// `GameObjectFlags` bits consulted by the highlightable gate `0x5f2f80` (decision 0243):
/// `0x1` IN_USE (busy) and `0x10` NO_INTERACT both suppress interaction; their union is the fast reject.
const GO_FLAG_IN_USE_OR_NO_INTERACT: u32 = 0x11;
/// `GO_FLAG_INTERACT_COND` (`0x4`) — the object is usable **only** when its per-player activate dyn-flag
/// is set. This is the quest gate: a quest chest/goober carries it, an ordinary door does not.
const GO_FLAG_INTERACT_COND: u32 = 0x4;
/// `GO_DYNFLAG_LO_ACTIVATE` (`0x1` in `GAMEOBJECT_DYN_FLAGS`) — the per-player "usable for me now" bit the
/// server sets from `GameObject::ActivateToQuest` (sparkle). Consulted only under `INTERACT_COND`.
const GO_DYNFLAG_ACTIVATE: u32 = 0x1;

/// The GameObject types whose strategy vtable **overrides** the highlightable slot (`+0x14`) with a
/// constant `xor al,al` — so `0x5f2f80` is never reached for them and they are never highlightable,
/// whatever their flags, faction or activate bit say. **Eleven of the thirty-one types**, byte-read
/// off the 5875 vtables: GENERIC(5) `0x5f47f0` · the transports 11/14/15 · the marker set
/// 8/16/25/30 · AUCTIONHOUSE(20) `0x5f68a0` · CAPTURE_POINT(29) `0x5f6d40` · and the default
/// strategy ([`strategy_is_default`]) `0x5f36f0`.
///
/// This is the whole per-type half of the predicate, kept as one named list because the reference
/// keeps it as one vtable slot: adding a type here removes its cursor, its brighten, its right-click
/// USE and its pick priority together, exactly as the binary does.
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

/// `GAMEOBJECT_TYPE_AUCTIONHOUSE` (20) — its `+0x14` is `0x5f68a0`, a bare `xor al,al; ret`, and its
/// `+0xc` forwards straight into it. So an auction-house **GameObject** shows nothing at all: no
/// cursor, no tooltip, no brighten, and `OnUse 0x5f8660` returns at `0x5f8673` without sending. (The
/// auctioneer you actually click in 1.12 is a *unit* with the AUCTIONEER service bit — a different
/// branch entirely — which is why this reads as a surprise and isn't one.)
const GO_TYPE_AUCTIONHOUSE: i32 = 20;
/// `GAMEOBJECT_TYPE_CAPTURE_POINT` (29) — `+0x14` = `0x5f6d40` const-false, so never a cursor; but
/// its `+0xc` is `0x5f6d80` = `data[19] != 0`, byte-for-byte GENERIC's shape at a different slot.
/// So it carries **GENERIC's exact law**: tooltip and brighten iff its highlight column is set,
/// never an interact cursor. Ships in no 1.12 data.
const GO_TYPE_CAPTURE_POINT: i32 = 29;
/// The inputs of the **own-predicate** `+0x14` slots — the two types (so far) whose highlightable
/// slot is neither the shared `0x5f2f80` nor a constant, and so reads state the shared gate never
/// touches. Resolved by the caller, like the reference resolves them from the active player and the
/// UI globals; each field is inert for every type but its own.
///
/// A struct rather than a tail of bare booleans on purpose: the signature was growing one `bool` per
/// override discovered (17 FISHINGNODE, then 23 MEETINGSTONE), and three adjacent same-typed
/// arguments is a swap waiting to happen at a call site. [`Default`] is "no override applies", which
/// is what every test and every non-override type wants.
#[derive(Clone, Copy, Default)]
pub(crate) struct GoOverrides {
    /// FISHINGNODE(17): the local player is channeling at exactly this bobber
    /// ([`fishing_channel_owned`]).
    pub(crate) channel_owned: bool,
    /// MEETINGSTONE(23): this stone's area IS the area we are queued at
    /// ([`meeting_stone_queued`]).
    pub(crate) meeting_stone_queued: bool,
}

/// `GAMEOBJECT_TYPE_MEETINGSTONE` (23) — the second type with its **own** `+0x14`
/// (`0x5f6990`), alongside FISHINGNODE(17). It never calls `0x5f2f80`: no faction term, no
/// `GAMEOBJECT_FLAGS`, no INTERACT_COND, no DYN_FLAGS. It is exactly
/// `template.data[2] (areaID) != [0xb72038]` — see [`highlightable_flags`]'s `meeting_stone_queued`.
pub(super) const GO_TYPE_MEETINGSTONE: i32 = 23;

/// The client's GameObject **highlightable** predicate (decision 0243,
/// `0x5f2f80`) over its wire flags: whether the object shows an interact cursor / is clickable at all.
/// The types whose vtable constant-falses the slot never get here at all
/// ([`strategy_never_highlightable`]); of the rest, a busy (IN_USE) or NO_INTERACT object isn't
/// highlightable, and an **INTERACT_COND** object (the quest gate) is highlightable only when the
/// server has set its per-player **activate** dyn-flag — so a quest chest sparkles and opens only once
/// the quest is held, while an ordinary door (no INTERACT_COND) is always highlightable regardless of
/// its (zero) activate bit. `usable` (lock / range / player-state → the grayed twin) rides on top and
/// is a later refinement.
///
/// `channel_owned` is the FISHINGNODE override's one input (`0x5f6710`): whether the active
/// player's `UNIT_FIELD_CHANNEL_OBJECT`
/// equals **this** GO's guid. The bobber is highlightable only to the player currently channeling
/// at exactly it — NOT a `CREATED_BY` compare — then tail-jumps into this shared gate. Someone
/// else's bobber (or yours after the channel drops) produces nothing at all: no cursor, no
/// tooltip/brighten (the `+0xc` thunk), and a silent dead right-click. Ignored for every other type.
///
/// `meeting_stone_queued` is MEETINGSTONE(23)'s override (`0x5f6990`): the slot is
/// `template.data[2] (areaID) != [0xb72038]`, and
/// `0xb72038` is the area the player is currently queued at — zero-initialized at `0x4c9eec`,
/// written only by the meeting-stone server-message handler `0x4ca230`, and read back by the Lua
/// binding `IsInMeetingStoneQueue`. So a stone is highlightable **unless it is the stone you are
/// already queued at**, and nothing else about it matters. The caller resolves the equality (as it
/// does `channel_owned`) against the live queue — [`crate::ui_dialog_verbs::MeetingStone::area`],
/// benilla's `[0xb72038]` (decision 2283; it was a hardcoded `0` while no queue existed, which is
/// the reference's own not-queued value and so was right for exactly as long as we never queued).
/// Ignored for every other type.
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
    // MEETINGSTONE's slot REPLACES the shared gate rather than guarding it — so this returns
    // outright and never falls through to the faction / flags / activate terms below.
    if type_id == GO_TYPE_MEETINGSTONE {
        return !overrides.meeting_stone_queued;
    }
    // The FACTION term (`0x5f2f80` @ `0x5f3026/29`, decision 0764). `reaction` is the GameObject's
    // reaction **toward us** ([`go_reaction`]); the ordinary test is `> 1`, i.e. anything but
    // hostile. `None` = unresolvable (no faction catalog / our store not streamed) and passes, so a
    // data gap never blanks the world.
    let ordinary_faction_ok = |r: Option<u8>| r.is_none_or(|r| r > 1);
    if type_id == GO_TYPE_TRAP {
        // TRAP alone INVERTS it (`0x5f2fc6/c9: cmp eax,1; jg` — reject when greater): a trap is
        // highlightable only to whoever it is hostile to. Everything below still applies.
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

/// `GAMEOBJECT_TYPE_TRAP` (6) — the one type whose faction term is inverted.
const GO_TYPE_TRAP: i32 = 6;

/// A GameObject's reaction **toward us** — the reference's `0x5f7fd0` → `0x606530` → `0x606640`
/// chain, whose direction is GO→player (`this` = the GO's own `FactionTemplate` row, the argument =
/// the player; `0x606640` tests `self.enemyGroupMask & other.ourMask`). On the client's 1/3/4 scale.
///
/// `None` when it cannot be resolved (no catalog, our store not streamed, either template missing) —
/// callers treat that as "no opinion" and pass. A GameObject with **no** faction resolves NEUTRAL(3),
/// which is the binary's own default (`0x5f8025: mov eax,3`).
///
/// INTERIM, and narrower than the unit path on purpose: the reference's `0x606530` also has a
/// player-controlled/reputation arm, but every GameObject faction that ships (114 monster, 35
/// friendly-to-players, 14, 1375) resolves through the plain template comparator, so the
/// reputation branch is not modelled here. `GAMEOBJECT_CREATED_BY` (a player-summoned object taking
/// its creator's reaction) is likewise not modelled.
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

/// The **mouseover-eligibility** virtual `[obj->vtbl+0x54]` — the gate that decides whether a picked
/// object becomes the mouseover **at all** (decision 0762).
///
/// This sits one level above everything the GO tooltip used to be reasoned from. The per-frame
/// classifier `CGWorldFrame::UpdateMouseoverCursor 0x4828d0` calls it on the picked object and, when
/// it answers false, publishes the **null** mouseover GUID (`0x482985 test eax,eax; je 0x4829ed` →
/// `0x482090(0,0)` + `ResetCursor 0x523d30`). The publisher `0x492890`, the tooltip builder
/// `0x52aa20` and the +64 brighten `0x4945e0` are then never reached: **no tooltip, no highlight, no
/// cursor — nothing.** That is the "some GameObjects show nothing at all on hover" report.
///
/// For a GameObject, slot `+0x54` is `0x5f8620`, which tail-calls the **per-GO-TYPE strategy slot
/// `+0xc`** — and that slot is *not* uniform:
///
/// | `[strat_vtbl+0xc]` | types |
/// |---|---|
/// | `0x5f9db0` = `jmp [+0x14]` = **[`highlightable_flags`] itself** | 0 DOOR · 1 BUTTON · 2 QUESTGIVER · 3 CHEST · 4 BINDER · 6 TRAP · 7 CHAIR · 9 TEXT · 10 GOOBER · 12 AREADAMAGE · 13 CAMERA · 17 FISHINGNODE · 18 RITUAL · 19 MAILBOX · 20 AUCTIONHOUSE · 22 SPELLCASTER · 23 MEETINGSTONE · 24 FLAGSTAND · 26 FLAGDROP · 27 MINI_GAME · 28 LOTTERY_KIOSK · + the default |
/// | the **highlight column** — `0x5f4830` = `data[1] != 0` / `0x5f6d80` = `data[19] != 0` | 5 GENERIC · 29 CAPTURE_POINT |
/// | `mov al,1` — always | 8 SPELL_FOCUS · 16 DUEL_ARBITER · 25 FISHINGHOLE · 30 AURA_GENERATOR |
/// | `xor al,al` — never | 11 TRANSPORT · 14 MAP_OBJECT · 15 MO_TRANSPORT |
///
/// **This slot is not the cursor's.** For the marker set the two slots point opposite ways — `+0xc`
/// constant-TRUE, `+0x14` constant-FALSE ([`GO_TYPE_SPELL_FOCUS`]) — so an anvil is *fully* hovered
/// (mouseover published, gold-name tooltip) and *not at all* interactable (plain pointer, no
/// brighten, no USE). Eligibility is never a stand-in for [`highlightable_flags`], and neither is
/// its negation.
///
/// **This refutes the old "the tooltip is not gated by highlightable" reading**, which generalised
/// **GENERIC's** behaviour to all 31 types. The
/// signpost half survives (GENERIC really does answer from `data[1]`, and 1387 of 1870 shipped
/// type-5 templates carry `data1 = 1`); the chest / IN_USE / INTERACT_COND half is wrong, and this
/// is why a pre-quest Stone of Binding or a Stratholme portcullis shows nothing in the reference.
///
/// `highlight_column` is the template slot the two data-driven types read — GENERIC's `data[1]`,
/// CAPTURE_POINT's `data[19]`. `None` = the ask-once template hasn't answered, which reads as
/// eligible so a signpost isn't blank for the first frames of its query.
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
        // Everything else forwards `+0xc` into its own `+0x14` (`0x5f9db0`) — including
        // AUCTIONHOUSE(20), whose `+0x14` is const-false, and MEETINGSTONE(23), whose is its own
        // predicate. Both fall out of this one arm because the forward is literal.
        _ => highlightable_flags(type_id, flags, dyn_flags, reaction, overrides),
    }
}

/// [`highlightable_flags`] read off a hovered GameObject's descriptor store. An absent
/// `GAMEOBJECT_TYPE_ID` is the wire default `0` = DOOR (vmangos omits the zero field), so a door
/// resolves to a highlightable type rather than being wrongly rejected as "unknown".
///
/// Gates the **cursor**, the `+64` **brighten**, the right-click **USE** (`OnUse 0x5f8660` calls this
/// same `+0x14` first) and the GO pick's **pass-2 priority** (decision 1071: classify `0x480c90`
/// ranks a highlightable GameObject `1` via `0x5f8800`, else `0`) — the four consumers of one vtable
/// slot. It is **not** the mouseover publish, which is the sibling slot `+0xc`
/// ([`mouseover_eligible`]): the two agree for most types and deliberately disagree for GENERIC(5)
/// (a signpost tooltips while showing no cursor — 0349's reference close-up) and for the marker set
/// (an anvil tooltips while showing no cursor either).
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

/// MEETINGSTONE(23)'s highlightable override, resolved for one GameObject: the reference's
/// `template.data[2] != [0xb72038]`, expressed as its negation so the caller passes the same shape
/// of boolean as [`fishing_channel_owned`].
///
/// `queued_area` is benilla's `[0xb72038]` — [`crate::ui_dialog_verbs::MeetingStone::area`], stored
/// unconditionally by every `SMSG 0x295` and `0` when we are not queued (decision 2283).
///
/// A template that hasn't answered yet resolves `false` (⇒ highlightable), the same permissive
/// default the highlight column takes — a stone isn't dead for the first frames of its query. Note a
/// stone whose `data[2]` really is **0** is *not* highlightable when we are unqueued, which is the
/// binary's own `0 != 0` and not an edge case worth smoothing away.
pub(crate) fn meeting_stone_queued(area: Option<u32>, queued_area: u32) -> bool {
    area.is_some_and(|a| a == queued_area)
}

/// The FISHINGNODE highlightable override's input (`0x5f6710`): is the local player currently
/// channeling at exactly this GameObject — `self.UNIT_FIELD_CHANNEL_OBJECT == go_guid`. `false`
/// when the self store hasn't streamed or the guid is unknown: an unverifiable bobber shows
/// nothing, matching the reference's no-active-player early-false.
pub(crate) fn fishing_channel_owned(
    self_store: Option<&ObjectStore>,
    go_guid: Option<u64>,
) -> bool {
    match (self_store, go_guid) {
        (Some(s), Some(g)) => s.0.unit_channel_object() == Some(g),
        _ => false,
    }
}

/// A `LockType.dbc` **CursorName** stem → the [`CursorKind`] it names — the client's
/// `CursorModeFromName` step (`0x523d40`) over the only three cursor-
/// bearing lock kinds in 5875. An unknown/empty name resolves to `None`, which the base GO path
/// reads as "the generic Interact gear."
fn cursor_kind_from_lock_name(name: &str) -> Option<CursorKind> {
    match name {
        "PickLock" => Some(CursorKind::PickLock),
        "GatherHerbs" => Some(CursorKind::GatherHerbs),
        "Mine" => Some(CursorKind::Mine),
        _ => None,
    }
}

/// The **data-named** cursor for a base-type GameObject's lock (the base `0x5f3070`): the GO
/// template's `lockId` → the `Lock.dbc` row → its **first** requirement slot's index (the client
/// reads `[lockRow+0x24]` = `Index[0]` unconditionally, no scan) → the `LockType.dbc` **CursorName**.
/// `None` when there's no lock, no client data, or the LockType has no distinct cursor — the caller
/// falls back to the Interact gear. A [`CursorKind::PickLock`] result *is* the `LockType.Id == 1`
/// signal the classifier keys on to skip the grayed twin ("Pick Lock" is the only name that maps
/// there), so no separate flag is needed.
fn go_lock_cursor(
    lock_id: u32,
    locks: Option<&crate::go_templates::Locks>,
    lock_types: Option<&crate::go_templates::LockTypes>,
) -> Option<CursorKind> {
    if lock_id == 0 {
        return None;
    }
    let slots = locks?.0.slots(lock_id)?;
    let lock_type_id = slots[0].index; // Index[0] — the client's single, first-slot read.
    let name = lock_types?.0.cursor_name(lock_type_id)?;
    cursor_kind_from_lock_name(name)
}

/// The GameObject cursor kind (`GetCursorMode 0x5f8760`), given a **highlightable** GO's type and
/// its resolved lock cursor. The per-type behaviors that override the base gear: TEXT(9) → the
/// Inspect magnifier; RITUAL(18)/MAILBOX(19)/type-28 → Mail. Every other type is the base behavior:
/// its data-named lock cursor (Mine/GatherHerbs/PickLock) if it has one, else the generic Interact.
fn go_cursor_kind(type_id: i32, lock_cursor: Option<CursorKind>) -> CursorKind {
    match type_id {
        GO_TYPE_TEXT => CursorKind::Inspect,
        GO_TYPE_RITUAL | GO_TYPE_MAILBOX | GO_TYPE_28 => CursorKind::Mail,
        _ => lock_cursor.unwrap_or(CursorKind::Interact),
    }
}

/// The QUESTGIVER leg's own gate — the bit alone is not enough. The ladder calls `0x5df490(unit)`,
/// which is `NPC_FLAGS bit 1` **AND** the cached quest status `[unit+0xcb8] ∉ {0, 1}` (`+0xcb8`
/// is the `SMSG_QUESTGIVER_STATUS` cache, written by `0x607440` and with
/// only three writers image-wide). So NONE(0) and UNAVAILABLE(1) do **not** make a unit talkable;
/// every other status does.
///
/// This is what keeps a questgiver-flagged NPC with nothing to offer from being clickable at all —
/// and it is load-bearing far beyond the cursor. Melika Isenstrider (vmangos entry 6778) is flagged
/// QUESTGIVER, carries no other service bit, and has zero rows in `creature_questrelation`: without
/// this gate she classifies Speak, we send `CMSG_GOSSIP_HELLO`, and vmangos answers the resulting
/// `DEFAULT_GOSSIP_MESSAGE` text query with eight literal `"Greetings $N"` blocks
/// (`QueryHandler.cpp:210-217`) — an empty gossip frame carrying a placeholder greeting, on an NPC
/// the reference never opens anything for. That was the whole of the reported "the client invents
/// 'Greetings NAME'" bug: the text was genuine, the *asking* was ours.
///
/// `None` (no status ever sent) reads as no quest: the server sends the status unprompted for every
/// questgiver in range, so its absence means the unit isn't offering us one.
pub(crate) fn questgiver_has_quest(quest_status: Option<u32>) -> bool {
    use benilla_protocol::messages::dialog_status::{NONE, UNAVAILABLE};
    !matches!(quest_status, None | Some(NONE) | Some(UNAVAILABLE))
}

/// The per-bit service ladder (`0x482336..0x4824e3`, statically unrolled): lowest set bit wins.
/// `None` = no *consulted* bit set — the unit
/// falls through to the attack/clear leg (this is where repair-only units land: bit 14 is never
/// tested in the binary).
///
/// `quest_status` is the unit's last `SMSG_QUESTGIVER_STATUS` (`None` = never sent), which gates the
/// QUESTGIVER leg — see [`questgiver_has_quest`].
fn service_cursor(service: u32, quest_status: Option<u32>) -> Option<CursorKind> {
    use npc_flags::*;
    // Bits 0 and 1 both land on Speak, and bit 0 is tested first, so the two rows fold into one
    // condition without changing a single outcome — the same folding the SPIRITHEALER/SPIRITGUIDE
    // and PETITIONER/TABARDDESIGNER/BATTLEMASTER rows already use. Only bit 1 carries a gate.
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

/// The loot leg's mode split (`8 + (keyDown(0) ? 8 : 0)` @ `0x48252c`): the single pouch, or
/// the triple LootAll while the EFFECTIVE auto-loot is on — the 0961 rule (`autoLootDefault`
/// XOR the held modifier) so the cursor always announces what the click will actually do. In
/// 1.12 the held key alone *was* the effective state (no CVar existed).
fn loot_cursor(auto_loot: bool, shift_held: bool) -> CursorKind {
    if auto_loot != shift_held {
        CursorKind::LootAll
    } else {
        CursorKind::Pickup
    }
}

/// Is this corpse **moused over at all** — the reference's `[CGCorpse_C vtbl+0x54]` = `0x5d76d0`,
/// asked at `0x482982` by the mouseover publisher.
///
/// A **real body** (BONES clear) qualifies **unconditionally — owner irrelevant**, so your own
/// corpse is moused over like anyone else's. Only a **bone pile with nothing to take** is dropped;
/// the base occupant it falls through to is `0x469fd0`, a constant false, which is the same reason
/// an item lying in the world is never moused over.
///
/// This is a *separate* gate from pick eligibility (`0x480816`) — a rejected corpse is still
/// picked, it simply publishes no mouseover, so it gets no name plate and no brighten. Ours reads
/// it the same way: the pick is unconditional, this decides what the hover *shows*.
pub(crate) fn corpse_mouseover_eligible(store: &ObjectStore) -> bool {
    !store.0.corpse_is_bones() || store.0.corpse_lootable()
}

/// What a hovered **corpse object** classifies to — the reference's corpse classifier `0x482740`,
/// as a pure decision so it can be tested the way the
/// service ladder is. `esi` = player, `edi` = corpse, `d2` = centre-to-centre distance².
///
/// Two legs, and there is **no third** — in particular no own-corpse leg. Your own body carries
/// neither flag, fails both tests, and falls out to the reference's "else clear": the plain
/// pointer. (Recovering it is decision 0308's `RECOVER_CORPSE` route, not a world cursor.)
/// What the corpse classifier reads — one struct rather than seven positional bools, so the call
/// sites (and the test) name each byte fact instead of counting commas.
#[derive(Debug, Clone, Copy, Default)]
struct CorpseFacts {
    /// `0x5d6e20` — `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0.
    lootable: bool,
    /// `CORPSE_FIELD_FLAGS` bit 5 — `CORPSE_FLAG_LOOTABLE`, the PvP insignia.
    insignia: bool,
    /// `[0xb700e8]` — the `SPELL_EFFECT_SKIN_PLAYER_CORPSE` learn-time latch.
    skin_latch: bool,
    /// `d2`, centre to centre.
    dist_sq: f32,
    /// The already-looting override: `[player+0x1d28/2c] == corpse GUID`.
    looting_this: bool,
    /// The effective auto-loot's two terms (0961): the knob…
    auto_loot: bool,
    /// …and the live key-0.
    shift_held: bool,
}

fn corpse_cursor(f: CorpseFacts) -> Option<(CursorKind, bool)> {
    // Leg 1 — lootable by me. Able-vs-Unable is `CanLootNow 0x5ec110(player, corpse)` — the same
    // range+state gate as the unit loot leg — OR the already-looting override, which keeps the lit
    // pouch on the object whose loot window is open even as you drift out of reach. The mode is
    // Pickup(8)/LootAll(16) by the held key-0 (`0x41f8f0`, `0x4827f8`); both fail →
    // UnablePickup(28)/UnableLootAll(36) at `0x482818`. Distance grays exactly as on units.
    if f.lootable {
        let able = f.dist_sq <= CORPSE_INTERACT_RANGE_SQ || f.looting_this;
        return Some((loot_cursor(f.auto_loot, f.shift_held), !able));
    }
    // Leg 2 — the PvP-insignia / skin-player-corpse leg (`0x482875..0x4828a2`): the flag **and**
    // the learn latch **and** `!0x6067d0(player, corpse)`, then the reach gate picks
    // SkinAlliance(20)/SkinHorde(19) or their Unable twins.
    //
    // Modelled with its real precondition rather than omitted, so it stays inert for the
    // reference's own reason: `[0xb700e8]` is latched only by learning a spell whose `Effect[0]`
    // is `SPELL_EFFECT_SKIN_PLAYER_CORPSE`, and no 1.12.1 player holds one. Benilla ships one Skin
    // cursor rather than the faction pair (`[0x80439c + 4×0x5efe00(player)]`) — a distinction with
    // no consequence while the leg is unreachable.
    if f.insignia && f.skin_latch {
        return Some((CursorKind::Skin, f.dist_sq > CORPSE_INTERACT_RANGE_SQ));
    }
    None // "Else clear" — a plain body under the cursor is Point.
}

/// Resolve this frame's [`WorldCursor`] from the hovered unit — the reference's classifier order:
/// interactable-NPC service ladder, else loot/skin/attack by state, each grayed by its own range
/// gate. No hover (or anything unresolvable) → Point.
#[allow(clippy::type_complexity)]
pub(super) fn classify_cursor(
    hovered: Res<Hovered>,
    hovered_object: Res<HoveredObject>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    mut cursor: ResMut<WorldCursor>,
    units: Query<(
        &Transform,
        Option<&ObjectStore>,
        Option<&crate::go_anim::GoAnim>,
    )>,
    self_q: Query<(&Transform, &ObjectStore), With<SelfPlayer>>,
    // The GameObject cursor is data-driven (decision 0236, `0x5f8760`): the ask-once
    // template (its `lockId`) + Lock.dbc + LockType.dbc name the cursor. All absent without client
    // data or before a hovered GO's template answers — a lock-bearing GO then reads as the gear.
    // The same param carries the lock chain `usable` consults (decision 0752) — one
    // [`super::lock::GoLockInputs`], so the cursor and the click ask literally the same question.
    go_inputs: super::lock::GoLockInputs,
    player_actions: Res<crate::ui_action::PlayerActions>,
    // `[0xb700e4]`/`[0xb700e8]` — the skin leg's learned-ability precondition (decision 0752).
    learned: Res<crate::ui_action::LearnedAbilities>,
    // The QUESTGIVER leg's gate reads the per-guid `SMSG_QUESTGIVER_STATUS` store — see
    // [`questgiver_has_quest`].
    quest: Res<crate::ui_quest::QuestGiver>,
    // The loot leg's Pickup/LootAll split (0965): the auto-loot knob (0961) + the live shift.
    loot_cfg: Res<crate::ui_loot::LootConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    // `[player+0x1d28/2c]` — the open loot session's object. The corpse leg's already-looting
    // override reads it (the reference keeps the lit pouch on the corpse you are looting even
    // once you have drifted past the range gate).
    loot_latch: Res<crate::ui_loot::LootLatch>,
    // `[0xb72038]` — the meeting-stone queue, MEETINGSTONE(23)'s own highlightable term
    // (decision 2283). `Option` because a headless build mounts no UI dialog verbs; absent reads
    // as the reference's zero-initialized global, i.e. not queued.
    stone: Option<Res<crate::ui_dialog_verbs::MeetingStone>>,
) {
    // A **highlightable** GameObject shows its **data-driven** cursor (`0x5f8760`): a
    // mailbox's Mail, a plaque's Inspect, a vein's Mine / herb's GatherHerbs / picked lock's PickLock
    // (off its LockType), else the generic Interact gear — each grayed out of interact range (except
    // PickLock, never grayed). A non-highlightable GO yields no cursor, like the reference handler's
    // clear (decision 0243): a GENERIC decoration, a busy/NO_INTERACT object, or a quest object whose
    // per-player activate bit the server hasn't set (INTERACT_COND without the quest). The usable-
    // grayed twin is still the interim distance gate (decision 0243); the client's fuller `usable`
    // (lock satisfaction, player-state) is later.
    let resolve_go = || {
        let (go_tf, store, anim) = units.get(hovered_object.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let reaction = go_reaction(
            factions.as_deref(),
            store.0.gameobject_faction(),
            Some(self_store),
        );
        // The ask-once template is read BEFORE the gate now: MEETINGSTONE's override is a template
        // slot, so the gate itself needs it (the lock cursor below still does too).
        let tmpl = hovered_object.guid.and_then(|g| go_inputs.templates.get(g));
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
        // The type's own cursor wins; a base type reads its lock's data-named cursor (needs the
        // ask-once template — a not-yet-answered GO falls back to the gear until it arrives).
        let lock_id = tmpl.map_or(0, |t| t.lock_id);
        let lock_cursor = go_lock_cursor(
            lock_id,
            go_inputs.locks.as_deref(),
            go_inputs.lock_types.as_deref(),
        );
        let kind = go_cursor_kind(store.0.gameobject_type_id(), lock_cursor);
        let dist_sq = go_tf.translation.distance_squared(self_tf.translation);
        // `usable 0x5f3130`, the two arms we model — the LOCK arm first, as the binary orders
        // them (`0x5f32a6` before the range check at `0x5f330c`), then the interim range gray.
        // The lock arm runs **only when `GO_FLAG_LOCKED` is set**: that is why an
        // ungatherable herb node keeps its lit GatherHerbs cursor and only refuses on the click,
        // while a padlocked door grays. `PickLock` never grays (`0x5f3070`: `LockType.Id == 1`
        // skips the usable gate), so the rogue's affordance still reads as available.
        let facts = super::lock::go_facts(Some((store, crate::go_anim::go_state(anim, store))));
        let lock_unmet = go_inputs
            .locks
            .as_deref()
            .and_then(|l| l.0.slots(lock_id).filter(|_| lock_id != 0))
            .is_some_and(|slots| {
                super::lock::resolve_lock(
                    slots,
                    &player_actions.spells,
                    go_inputs.spells.as_deref(),
                    go_inputs.skill_lines.as_ref().map(|s| &s.catalog),
                    Some(self_store),
                    &go_inputs.objects,
                    facts,
                    &mut None,
                )
                .blocks_usable(facts.flag_locked)
            });
        let unable = kind != CursorKind::PickLock
            && (lock_unmet || dist_sq > go_interact_range_sq(store.0.gameobject_type_id()));
        Some((kind, unable))
    };
    // The reference makes one pick over all CGObjects and switches on type; benilla picks unit and
    // GameObject separately, then classifies whichever is nearer under the cursor.
    let resolve_unit = || {
        let (unit_tf, store, _) = units.get(hovered.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let dist_sq = unit_tf.translation.distance_squared(self_tf.translation);
        // Melee interact reach (loot + skin's gate): both units' combat reach + the offset,
        // floored at 5 yd. Boundary-inclusive, center-to-center.
        let reach = (store.0.unit_combat_reach() + self_store.0.unit_combat_reach() + MELEE_OFFSET)
            .max(MELEE_FLOOR);
        let in_melee = dist_sq <= reach * reach;

        let dead = store.0.unit_is_dead();
        if dead {
            if store.0.unit_lootable() {
                // The loot mode split `8 + (keyDown(0) ? 8 : 0)` @ `0x48252c`, over 0961's
                // effective auto-loot ([`loot_cursor`]). Gray by the byte-verified gate
                // (inline in `CanLootNow 0x5ec110`): the same melee interact reach as skin —
                // ~5 yd vs a normal mob (director-measured, confirmed).
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                return Some((loot_cursor(loot_cfg.auto_loot, shift), !in_melee));
            }
            // The skin leg's SECOND precondition: the flag alone is not enough —
            // the reference also requires the learn-time latch `[0xb700e4 + 4×isPlayerTarget]` to
            // be non-null (`0x482589`), i.e. **the player must have
            // learned a Skinning spell**. A non-skinner gets no knife on a skinnable corpse; the
            // corpse falls through to Point like any other unlootable one (decision 0752).
            if store.0.unit_flags() & UNIT_FLAG_SKINNABLE != 0 && learned.skinning.is_some() {
                return Some((CursorKind::Skin, !in_melee));
            }
            return None; // a plain corpse: Point
        }

        // `CanInteract 0x6067f0` — the classifier's own gate (`0x482310`, through the
        // `CanInteractNow 0x606880` wrapper), and it is not a reaction threshold: service bits
        // plus BOTH reaction directions >= neutral, the player->unit one answered by the AT-WAR
        // bit. TRUE goes to the ladder, FALSE to the loot/skin/attack block.
        if super::can_interact(
            Some(store),
            factions.as_deref(),
            &reputations,
            Some(self_store),
        ) {
            let status = hovered.guid.and_then(|g| quest.status(g));
            // A unit that reaches the ladder and matches no consulted bit — repair-only, or a
            // QUESTGIVER with nothing on offer — falls `je 0x4826cb` straight to the cursor
            // CLEAR. It never reaches the attack leg, so it reads Point, not the sword.
            return service_cursor(store.0.unit_npc_flags(), status)
                .map(|kind| (kind, dist_sq > SERVICE_RANGE_SQ));
        }
        // `CanAttack 0x606980` (`0x48269a`) + the fixed 10.45 yd gray. The predicate is shared
        // with TAB, the combat flash and `UnitCanAttack`, so the sword can never disagree with
        // what a click or a spell will actually be allowed to do.
        if super::can_attack(
            Some(store),
            factions.as_deref(),
            &reputations,
            Some(self_store),
        ) {
            return Some((CursorKind::Attack, dist_sq > ATTACK_RANGE_SQ));
        }
        None // nothing matched → the reference's `0x4826cb` cursor-clear: Point
    };
    // The **corpse classifier** `0x482740`, whose whole decision is [`corpse_cursor`] — the store
    // reads and the distance are all that happens here.
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
            looting_this: loot_latch.0.is_some() && loot_latch.0 == hovered.corpse_guid,
            auto_loot: loot_cfg.auto_loot,
            shift_held: shift,
        })
    };
    let resolved = if go_is_nearest(&hovered, &hovered_object) {
        resolve_go()
    } else if hovered.corpse.is_some() {
        resolve_corpse()
    } else {
        resolve_unit()
    };
    let (kind, unable) = resolved.unwrap_or((CursorKind::Point, false));
    let want = WorldCursor { kind, unable };
    if *cursor != want {
        *cursor = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;

    /// The corpse classifier `0x482740` has two legs and no more — and neither of them is your
    /// own body, which is what makes a corpse read as the plain pointer until something *else*
    /// offers to recover it.
    #[test]
    fn the_corpse_classifier_has_two_legs_and_no_own_corpse_leg() {
        let near = 25.0; // exactly the gate — boundary-inclusive, like every reach in this file
        let far = 25.01;
        let bones = |dist_sq| CorpseFacts {
            lootable: true,
            dist_sq,
            ..CorpseFacts::default()
        };
        // Leg 1: lootable bones. Pickup vs LootAll by the effective auto-loot, grayed by range.
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
        // …and the already-looting override keeps it lit past the gate.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                looting_this: true,
                ..bones(far)
            }),
            Some((CursorKind::Pickup, false))
        );
        // Leg 2 needs BOTH the insignia flag and the learn-time latch. The flag alone is nothing —
        // which is why a battleground body shows no knife to a player who cannot skin one, and why
        // this leg is inert in 1.12.1: nothing latches `[0xb700e8]`.
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
        // Leg 1 wins outright when both flags are up — the binary tests lootable FIRST and returns.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia: true,
                skin_latch: true,
                ..bones(near)
            }),
            Some((CursorKind::Pickup, false))
        );
        // **Your own corpse**: neither bit is set. Both legs miss, the classifier clears, and the
        // cursor is the plain pointer at any range and with anything held.
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

    /// The loot pouch follows the EFFECTIVE auto-loot (0961: setting XOR shift) — the cursor
    /// and the click can never disagree about what a pick will do.
    #[test]
    fn the_loot_pouch_triples_with_the_effective_auto_loot() {
        assert_eq!(loot_cursor(false, false), CursorKind::Pickup);
        assert_eq!(loot_cursor(true, false), CursorKind::LootAll);
        // Shift inverts BOTH ways (era's AUTOLOOTTOGGLE; vanilla's whole mechanism).
        assert_eq!(loot_cursor(false, true), CursorKind::LootAll);
        assert_eq!(loot_cursor(true, true), CursorKind::Pickup);
        // The grayed twin rides the same stem.
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
        // Point has no grayed twin — unable never redirects it.
        let point = WorldCursor {
            kind: CursorKind::Point,
            unable: true,
        };
        assert_eq!(point.stem(), "Point");
    }

    /// The mouseover-eligibility table (decision 0762) — the gate that decides whether an object
    /// becomes the mouseover at all, so a false here is "no tooltip, no brighten, no cursor".
    #[test]
    fn mouseover_eligibility_matches_the_per_type_slot_table() {
        // The constant-TRUE slots (`mov al,1`) ignore their flags entirely.
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
        // The three constant-FALSE slots (`xor al,al`) — the transport family is never a mouseover.
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
        // GENERIC answers from its template's `data[1]` alone — the signpost that hovers vs the
        // scenery beside it that does not. Not from `highlightable_flags`, which rejects GENERIC.
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
        // Everything else IS highlightable — which is the whole correction. A pre-quest
        // INTERACT_COND object (the Stone of Binding, a Stratholme portcullis at flags 0x24) and an
        // IN_USE / NO_INTERACT object show NOTHING, where we used to tooltip them.
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
            // The FACTION term (decision 0764): a door whose template is hostile to us is not
            // eligible — no cursor, no tooltip, no brighten. This is Deadmines' Factory Door
            // (faction 114, flags 0x20), which the director could still hover and open.
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
        // TRAP inverts it: hostile-to-us is exactly the case a trap DOES highlight for.
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
        // An unresolvable reaction never blanks the world.
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        // A plain door is still eligible — the everyday case must not regress.
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
        // An ordinary unlocked door (no INTERACT_COND) is highlightable regardless of its zero activate
        // bit — plain doors must never gray out.
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default())); // DOOR, no flags
                                                                             // A GENERIC decoration is never highlightable.
        assert!(!highlightable_flags(
            GO_TYPE_GENERIC,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        // Neither is the transport family (the byte-dumped constant-false +0x14 slots): no gear,
        // no tooltip, no USE on a boat / elevator / map object — flags can't make them so.
        for t in [GO_TYPE_TRANSPORT, GO_TYPE_MAP_OBJECT, GO_TYPE_MO_TRANSPORT] {
            assert!(!highlightable_flags(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                None,
                GoOverrides::default()
            ));
        }
        // Nor the marker set, whose `+0x14` is the same byte-dumped `xor al,al` — an anvil / forge /
        // brazier (SPELL_FOCUS, flags 0x0, faction 0 ⇒ NEUTRAL, every flag term passing) is exactly
        // the case that used to reach the predicate and come back true, wearing the Interact gear.
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
        // And the two slots point OPPOSITE ways for the marker set: hovered (tooltip) but never
        // interactable (no cursor, no brighten, no USE). The reported anvil, both halves at once.
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
        // The DEFAULT strategy — type 21, and everything outside the factory's 0..=30 jump table.
        // It shows NOTHING (both slots false), where the flag predicate would have answered `true`
        // for exactly the all-zero descriptor an unknown type arrives with.
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
        // …and each of the 30 real cases has its own strategy — 21 is the table's only hole.
        for t in (0..=GO_TYPE_MAX).filter(|t| *t != GO_TYPE_GUARDPOST) {
            assert!(!strategy_is_default(t), "type {t} has its own strategy");
        }
        // AUCTIONHOUSE(20) — `+0x14` = `0x5f68a0`, a bare `xor al,al`, and `+0xc` forwards INTO it,
        // so the auction-house GameObject shows nothing at all. (The auctioneer you click is a
        // unit; this type is not that.) Flags can't make it so.
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
        // CAPTURE_POINT(29) carries GENERIC's law at a different slot: never a cursor, but a
        // tooltip iff its highlight column (`data[19]`) is set. The two slots disagree by design,
        // exactly as they do for type 5 — so assert BOTH, or the pairing is untested.
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
        // A busy (IN_USE) or NO_INTERACT object is not highlightable.
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
            // The quest gate: an INTERACT_COND object is highlightable only with the activate bit set — the
            // exact bug, a quest chest without the quest.
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

    /// MEETINGSTONE(23)'s own `+0x14` (`0x5f6990`): `template.data[2] (areaID) != [0xb72038]`, and
    /// **nothing else** — it never calls `0x5f2f80`, so none of the shared gate's terms apply.
    #[test]
    fn the_meeting_stone_answers_only_the_queued_area() {
        let queued = GoOverrides {
            meeting_stone_queued: true,
            ..Default::default()
        };
        // Unqueued (the only state benilla can be in today) → highlightable.
        assert!(highlightable_flags(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        // Queued at THIS stone → not highlightable: no cursor, and (via the `+0xc` forward) no
        // tooltip or brighten either.
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
        // The slot REPLACES the shared gate — it does not ride on top of it. Every term that would
        // reject any other type is inert here: hostile faction, NO_INTERACT, IN_USE, and an
        // INTERACT_COND with no activate bit all still leave an unqueued stone highlightable.
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
        // The caller's half: the resolved boolean is `data[2] == the queued area`, with an
        // unanswered template reading as not-queued so a stone isn't dead while it queries. The
        // right-hand side is the LIVE queue since 2283 — unqueued (`0`) leaves every stone
        // highlightable, and queueing at one area silences exactly that area's stones.
        assert!(!meeting_stone_queued(None, 0));
        assert!(!meeting_stone_queued(Some(1519), 0));
        assert!(meeting_stone_queued(Some(1519), 1519));
        assert!(!meeting_stone_queued(Some(1519), 1517));
        assert!(!meeting_stone_queued(None, 1519));
        // The binary's own `0 != 0`: a `data[2] == 0` stone reads as the stone we are "queued at"
        // while we are not queued at all.
        assert!(meeting_stone_queued(Some(0), 0));
    }

    #[test]
    fn the_bobber_is_channel_gated_and_reaches_a_hundred_yards() {
        // The FISHINGNODE highlightable override (`0x5f6710`):
        // only the player whose UNIT_FIELD_CHANNEL_OBJECT names exactly this GO passes — someone
        // else's bobber (or yours after the channel drops) shows nothing at all.
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
        // The pass tail-jumps the SHARED gate, so the flags still apply on top.
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
        // The mouseover thunk (`+0xc` → `+0x14`) rides the same predicate: no tooltip either.
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
        // Every other type ignores the channel input entirely.
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default()));
        // The per-type range: the bobber's ctor constant is 100.0 yd (squared at the compare) —
        // effectively un-range-gated — while everything else stays on the interim reach.
        assert_eq!(go_interact_range_sq(GO_TYPE_FISHINGNODE), 100.0 * 100.0);
        assert_eq!(go_interact_range_sq(0), GO_INTERACT_RANGE_SQ);
        // The chair's own 3.0 yd (1464) — the one type that reaches *less* far than the interim, so
        // the interim was the visible bug: a live cursor and a dropped packet from 3 to 5.56 yd.
        assert_eq!(go_interact_range_sq(GO_TYPE_CHAIR), 9.0);
        assert!(
            go_interact_range_sq(GO_TYPE_CHAIR) < GO_INTERACT_RANGE_SQ,
            "the chair reaches SHORTER than the interim — a longer one would restore the bug"
        );
        // The channel compare itself: owned iff self's channel object is exactly this guid.
        assert!(!fishing_channel_owned(None, Some(7)));
        assert!(!fishing_channel_owned(None, None));
    }

    #[test]
    fn melee_reach_floors_at_five() {
        // The byte-verified floor (`0x80a1e8` is a max, not a min): a typical player-vs-mob pair
        // (1.5 + 1.5 + 1.333 = 4.333) is lifted to 5 yd — the threshold the director measured.
        assert_eq!((1.5_f32 + 1.5 + MELEE_OFFSET).max(MELEE_FLOOR), 5.0);
        // Large creatures reach *farther* than 5 — the floor never cuts a big sum down.
        assert!(((3.0_f32 + 3.0 + MELEE_OFFSET).max(MELEE_FLOOR) - 7.333_33).abs() < 1e-4);
    }

    #[test]
    fn service_ladder_matches_the_unrolled_binary() {
        use npc_flags::*;
        // A quest to offer, so the QUESTGIVER row behaves like the rest of the ladder here; the
        // gate itself is `questgiver_flag_alone_is_not_talkable`.
        let has = Some(dialog_status::AVAILABLE);
        // The rows the reference pins per byte address.
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
        // Lowest bit wins: a gossiping vendor speaks; an innkeeper-banker interacts.
        assert_eq!(
            service_cursor(GOSSIP | VENDOR, None),
            Some(CursorKind::Speak)
        );
        assert_eq!(
            service_cursor(INNKEEPER | BANKER, None),
            Some(CursorKind::Interact)
        );
        // REPAIR (0x4000) is never consulted — repair-only falls to the attack/clear leg.
        assert_eq!(service_cursor(0x4000, None), None);
        assert_eq!(service_cursor(0, None), None);
    }

    /// **A unit that reaches the ladder and matches nothing falls to the cursor CLEAR, never to
    /// the sword** (`je 0x4826cb`, past the attack leg's `0x48269a`). This is the second half of
    /// 1674: the ladder's fall-out used to drop into benilla's attack leg, so a repair-only NPC —
    /// or a QUESTGIVER with nothing on offer — read as attackable purely because its reaction was
    /// neutral. `service_cursor` returning `None` inside the interactable branch must mean Point.
    #[test]
    fn the_ladder_falls_out_to_point_not_to_the_sword() {
        use npc_flags::*;
        // Both shapes that reach the ladder and match no consulted bit.
        assert_eq!(service_cursor(REPAIR_ONLY, None), None);
        assert_eq!(service_cursor(QUESTGIVER, Some(dialog_status::NONE)), None);
        // The classifier's own expression for that branch: `Option::map` over the ladder, so a
        // `None` returns `None` from `resolve_unit` and the caller's `unwrap_or` lands on Point.
        // (Written as the identity it is, so a future edit that adds an `else` fall-through to the
        // attack leg has to delete this test to compile a different shape.)
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

    /// `UNIT_NPC_FLAGS` REPAIR — the one service bit the ladder never tests.
    const REPAIR_ONLY: u32 = 0x4000;

    /// The QUESTGIVER leg's `0x5df490` gate: the bit alone never makes a unit talkable. This is the
    /// "client invents 'Greetings NAME'" bug at its root — a questgiver-flagged NPC with nothing to
    /// offer must fall out of the ladder entirely, so we never send `CMSG_GOSSIP_HELLO` and never
    /// open the empty gossip frame the server would answer with a placeholder greeting.
    #[test]
    fn questgiver_flag_alone_is_not_talkable() {
        use npc_flags::*;
        // Melika Isenstrider's exact shape: QUESTGIVER, no other service bit, nothing on offer.
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
        // Every other status is a quest worth talking about — `[unit+0xcb8] ∉ {0, 1}`.
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
        // The gate is the QUESTGIVER leg's alone: a quest-less unit that also gossips still speaks
        // (bit 0 is tested first and carries no gate), and a quest-less vendor still shows Pickup
        // rather than falling out of the ladder.
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
        // The only cursor-bearing LockType CursorNames in 5875 (byte-confirmed in the LockType
        // catalog test) — the client's `CursorModeFromName` over them.
        assert_eq!(
            cursor_kind_from_lock_name("PickLock"),
            Some(CursorKind::PickLock)
        );
        assert_eq!(
            cursor_kind_from_lock_name("GatherHerbs"),
            Some(CursorKind::GatherHerbs)
        );
        assert_eq!(cursor_kind_from_lock_name("Mine"), Some(CursorKind::Mine));
        // Anything else (an empty CursorName, or an unmodeled name) → no data cursor → Interact.
        assert_eq!(cursor_kind_from_lock_name(""), None);
        assert_eq!(cursor_kind_from_lock_name("Fishing"), None);
    }

    #[test]
    fn go_cursor_kind_maps_type_then_lock() {
        // MAILBOX(19), RITUAL(18) and type 28 all show Mail — regardless of any (irrelevant) lock.
        assert_eq!(go_cursor_kind(19, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(18, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(28, Some(CursorKind::Mine)), CursorKind::Mail);
        // TEXT(9) plaque → the Inspect magnifier.
        assert_eq!(go_cursor_kind(9, None), CursorKind::Inspect);
        // Base types: a door(0)/button(1)/chest(3)/goober(10) with no data cursor → the Interact gear.
        for t in [0, 1, 3, 10, 6, 24] {
            assert_eq!(go_cursor_kind(t, None), CursorKind::Interact);
        }
        // A base type carrying a data-named lock cursor shows it (a chest over a vein/herb/picked lock).
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
        // The new GO cursor kinds resolve to real `Interface\Cursor\<stem>.blp` stems (all present
        // in 5875, confirmed by extraction); their grayed twins prepend Unable, except Point.
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
