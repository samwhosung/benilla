//! `SpellVisual.dbc` + `SpellVisualKit.dbc` + `SpellVisualEffectName.dbc` +
//! `SpellChainEffects.dbc` — the per-lifecycle-stage kit chain a cast plays
//! (decision 0099 phase 2's data plane; schemas pinned by decision 0107, the chain table by 0955).
//!
//! Layout — VERIFIED against build 5875 (WDBC header dump of the extracted files, 2026-07-04;
//! the client's loaders `0x5508e0`/`0x550690`/`0x550460`, corroborated by vmangos
//! `DBCStructure.h:613-640` `SpellVisualEntry` and our own header bytes):
//!
//! - **SpellVisual**: 2165 records × 16 fields × 64 B, all-`u32` (a 1-byte empty string block).
//!   Field 0 = id; **field 1 = precastKit · field 2 = castKit · field 3 = impactKit · field 4 =
//!   stateKit · field 5 = channelKit** — each a `SpellVisualKit` id, `0` = no kit at that stage.
//!   The missile block (decision 0099 phase 4 — every
//!   consumption in the client's missile spawn `0x60a3d0`): **field 7 = the missile's
//!   `SpellVisualEffectName` id** (`<1` → the ammo/weapon model; unresolvable → the client's
//!   literal `Spells\ErrorCube.mdx`), **field 9 = the destination-attachment ordinal** (an index
//!   into [`MISSILE_ATTACH_TABLE`], the client's `0x860a18`), and **field 10 = the missile's
//!   in-flight LOOP sound**
//!   ([`SoundEntries`] id — Fireball's `FireMissileLoop`, the thrown dagger's `WeaponLoop`; the
//!   client's per-missile loop handle `CMissile+0x44`). Field 6 (`hasMissile`)
//!   is never read by the missile spawn (its gate is `Spell.dbc` Speed alone) — its ONE reader
//!   is the GO dest one-shot's suppressor ([`VisualStages::missile_gate`], 0797); field 8
//!   (`missilePathType`) is dead-by-absence; **fields 11/12/13 are the dest-anchored block**
//!   ([`VisualStages::area_gate`]/[`VisualStages::area_effect`]/[`VisualStages::area_kit`] —
//!   `0x5d57c0`/`0x5d55c0`, 0797). Stage
//!   semantics (decision 0107 verdict 2): the stage sets *lifetime policy* only —
//!   every populated slot on a reached row fires, precast persisting (reaped spell-id-keyed)
//!   while cast/impact self-terminate.
//! - **SpellVisualKit**: 1772 records × 35 fields × 140 B, all-`u32`. Field 0 = id; **field 2 =
//!   the `AnimationData.dbc` animation id** (fed to the same `PlayAnimation` one-shot/overlay
//!   route as melee swings — decision 0107 verdict 3); **field 13 = a `SoundEntries.dbc` id**;
//!   **fields 3–11 are the nine `SpellVisualEffectName` emitter slots** (attach-point VFX —
//!   decision 0099 phase 3, read here since that phase). Each slot's attach tag is a compile-time
//!   immediate in the client's slot loop (`0x60edf0`, its push sites) and a **direct M2
//!   `AttachmentID`**: in kit-field order,
//!   [`KIT_SLOT_TAGS`]. **Field 12 (`kit+0x30`) is a TENTH effect slot** — read here as
//!   [`VisualKit::world_effect`] (decision 0848): its consumer reads as "the missile
//!   slot" (`0x60edf0` plays it inline via `0x61fcf0`, a `CEffect`-family node stamped with a
//!   missile marker), but the shipped table's population is
//!   **body/ground state models**, never projectiles: the aura-state family the nine slots miss
//!   (Frost Nova's kit 285 → `Frost_Nova_state.mdx`, Net's 744 → `Net_State.mdx`, Entangling
//!   Roots' 66, Web's 746) plus caster-feet rings on cast/impact kits (Thunderclap's 349,
//!   Flamestrike's 420, Vanish's 389). Same `CEffect` lifecycle as the nine (spell-id-tagged on
//!   the unit's `+0xb4` list, the stage sets lifetime — `0x614150`). Placement (`0x620c86`,
//!   decision 0850): no bone — a one-time **world plant** at the
//!   owner's position/facing/scale, [`WORLD_EFFECT_TAG`]. **Field 14 is the kit's CAMERA SHAKE**
//!   ([`VisualKit::shake`]) — a `SpellEffectCameraShakes.dbc` **group** id, not the "visual-group
//!   fallback" earlier notes called it: 58 shipped kits carry one and all 58 values land on that
//!   table's nine sparse ids, `Spell.dbc` 12816 is literally named *"Idom Rool Camera Shake"*, and
//!   the set reads as nothing else (kit 356 = War Stomp/Thunder Clap/Kodo Stomp, kit 348 = the
//!   Knockdown/Slam/Charge impacts, kit 138 = every summon). **Fields 15–34 are the four
//!   `CharProc` slots** — the
//!   *character* half of a kit (the body's own alpha/tint, as opposed to the attach-point emitters):
//!   five parallel 4-element arrays, `CharProcType[4]` @+0x3c then `CharParamZero/One/Two/Three[4]`
//!   @+0x4c/+0x5c/+0x6c/+0x7c (all 20 consumed by the
//!   dispatcher `0x60d7c0`, which `lea edi,[kit+0x6c]` walks four times reading `[edi-0x30]` as the
//!   type key). Read here as [`VisualKit::char_procs`]; the type semantics are
//!   [`char_proc_type`]'s.
//! - **SpellVisualEffectName**: 5 fields × 20 B. Field 0 = id; field 1 = a name string — a debug
//!   label, and the lookup key for the client's boot-time HARDCODED-effect matcher (`0x61f5b0`
//!   over a 14-string table: loot art, footsteps, breath, level-up…) — the `"HARDCODED *"` rows
//!   load into [`SpellVisualCatalog::hardcoded_effect`]'s name→path map (two consumers today: the
//!   corpse sparkle and the level-up ding); **field 2 = the effect model's `.mdx` path** (the
//!   column every consumer reads — fields 3/4 are dead-by-absence in the reference; the emitter
//!   *scale* comes from kit CharProc params × the quality tier, never from this table).
//! - **SpellChainEffects**: 18 records × 8 fields × 32 B — the **beam/arc** geometry a kit's chain
//!   `CharProc` draws (Chain Lightning's lightning, Drain Life's rope, C'Thun's eye beam). Its own
//!   module: [`chain_effects`], decision 0955.
//!
//! **The none-sentinel, found empirically here (not pinned by the reference):** on the real
//! table, "no value" for both field 2 (anim) and field 13 (sound) is written as **either `0` or
//! `0xFFFFFFFF`**, not consistently one or the other. Scanning all 1772 kits: 41 carry anim `0`,
//! 875 carry anim `0xFFFFFFFF`, 856 carry a real id (max 203, inside `AnimationData.dbc`'s 208
//! rows). Restricting to the 1433 kits actually reachable from a live `spell_template` visual
//! chain doesn't change the picture (692 are `-1` vs 26 `0` vs 715 real) — the `-1` form is the
//! majority for **impact**-stage kits specifically (966/1200: most magic impacts carry no body
//! reaction) while **cast**-stage kits are majority real (1168/1347: most casts do animate).
//! `sound` (field 13) shows the same dual encoding on a handful of rows (5 kits carry
//! `0xFFFFFFFF` alongside 647 carrying plain `0`). [`VisualKit::anim_id`]/[`VisualKit::sound`]
//! fold both forms to `None` so no consumer ever sees the raw sentinels.
//!
//! **Verified chain (Fireball, spell 133 → visual 67):** precast 30 / cast 38 / impact 286 /
//! state 0 / channel 0; kit 38 (cast) → anim 53 (`AnimationData` "SpellCastDirected") / sound
//! 1484; kit 286 (impact) → anim 9 (`AnimationData` "CombatWound" — a hit-reaction anim on the
//! target, corroborating decision 0107's wound-flinch ids 8–10) / sound 1507.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

pub mod chain_effects;

pub use chain_effects::{ChainEffect, ChainProc, CHAIN_MAX_BEAMS};

const SPELL_VISUAL: &str = "DBFilesClient\\SpellVisual.dbc";
const SPELL_VISUAL_KIT: &str = "DBFilesClient\\SpellVisualKit.dbc";
const SPELL_VISUAL_EFFECT_NAME: &str = "DBFilesClient\\SpellVisualEffectName.dbc";

const SPELL_VISUAL_FIELDS: usize = 16;
const SPELL_VISUAL_KIT_FIELDS: usize = 35;

/// The nine kit emitter slots' attach tags, **in kit-field order** (fields 3–11 ↔ this array's
/// indices 0–8). Compile-time immediates in the client's slot loop (`0x60edf0`), each a
/// **direct M2 `AttachmentID`**: Head(0x14), Chest(0x22),
/// Base(0x13), LeftHand(0x15), RightHand(0x16), Breath(0x11), Special1–3(0x17–0x19).
pub const KIT_SLOT_TAGS: [u16; 9] = [0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19];

/// The sentinel tag [`VisualKit::effects`] yields for [`VisualKit::world_effect`] (kit field
/// 12) — the client's own **−1**: `0x61fcf0` passes no attach tag at all (`0x61fd23: push -1`,
/// `node+0x24` stays the ctor's −1), which in the placement walk `0x620be0` skips the entire
/// bone pipeline and instead triggers a **one-time world plant** (`0x620c86`): transform =
/// `translate(owner position) × yaw(owner facing) × scale(owner scale)`, baked at spawn — the
/// model does NOT ride a bone and does not turn with the unit afterwards (decision 0848).
/// Consumers key on this tag to plant in world space rather than attach.
pub const WORLD_EFFECT_TAG: u16 = u16::MAX;

/// The missile **destination-attachment** ordinal table — the client's `0x860a18`, dumped
/// byte-for-byte (11 live entries): [`KIT_SLOT_TAGS`] in
/// kit-field order plus `0xf`/`0x10` at ordinals 9/10. `SpellVisual` field 9 indexes here to the
/// M2 attach tag the missile homes to on a live target ([`VisualStages::missile_attach`]).
pub const MISSILE_ATTACH_TABLE: [u16; 11] = [
    0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19, 0xf, 0x10,
];

/// The DBC's alternate "no value" encoding for a foreign-key-ish column (module docs) — folded to
/// `None` alongside plain `0`.
const NONE_SENTINEL: u32 = u32::MAX;

/// The four parallel `CharProc` slots a `SpellVisualKit` row carries (module docs; the client's
/// dispatcher `0x60d7c0` walks exactly four).
pub const KIT_CHAR_PROCS: usize = 4;

/// One of a kit's four `CharProc` slots: a **type key + its four float params**, the character-half
/// of a kit (the body's own render properties, not an attach-point emitter). The client's dispatcher
/// `0x60d7c0` switches on [`Self::ty`] through a byte translation table (`0x60dc20`) into a 9-case
/// jump table (`0x60dbfc`); each case reads whichever params it wants — the alpha and tint procs
/// both read `params[0]` (`0x60d972`, `0x60d840`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharProc {
    /// `CharProcType[i]` — the dispatch key ([`char_proc_type`] names the ones we model). Never a
    /// none-sentinel: an empty slot is `None` at the [`VisualKit::char_procs`] level.
    pub ty: i32,
    /// `CharParamZero/One/Two/Three[i]`, in that order — float columns (the client loads them with
    /// `fld` and rounds where it wants an integer, as the tint proc does with its packed RGB).
    pub params: [f32; 4],
}

impl CharProc {
    /// Param `i` through the client's **small-int decode** — see [`char_proc_small_int`]. The
    /// procs that want an integer out of a float column all use this one idiom.
    pub fn small_int(&self, i: usize) -> u32 {
        char_proc_small_int(self.params[i])
    }

    /// This slot decoded as the **weapon swing trail** proc, or `None` when it is not one.
    ///
    /// The dispatcher's type-8 arm (`0x60d80a`–`0x60d83b`) is three `_ftol` calls and two stores,
    /// and it is the one proc that uses **plain truncation** rather than
    /// [`char_proc_small_int`] — so this decode must not borrow that idiom:
    ///
    /// ```text
    /// 60d80a  fld [edi-0x20] ; call 0x40a2b0    ; ftol(CharParamZero)
    /// 60d812  fld [edi+0x10] ; call 0x40a2b0    ; ftol(CharParamThree)
    /// 60d823  shl eax,0x18 ; or ecx,eax
    /// 60d828  mov [esi+0xd1c],ecx               ; unit+0xd1c = zero | (three << 24)
    /// 60d82e  fld [edi]      ; call 0x40a2b0    ; ftol(CharParamTwo)
    /// 60d835  mov [esi+0xd20],eax               ; unit+0xd20 = two
    /// ```
    ///
    /// `CharParamOne` is **not read by this arm** — stated because every shipped type-8 row
    /// carries `20.0` there and a reader will look for its use.
    ///
    /// `None` for a zero duration: `0x5fe494 cmp eax,edi` / `je 0x5fe4b7` is the reference's own
    /// test, and a zero-duration arm fires nothing.
    pub fn as_weapon_trail(&self) -> Option<TrailProc> {
        if self.ty != char_proc_type::WEAPON_TRAIL {
            return None;
        }
        let ftol = |f: f32| f.trunc() as i32 as u32;
        let duration_ms = ftol(self.params[2]);
        (duration_ms != 0).then(|| TrailProc {
            packed: ftol(self.params[0]) | (ftol(self.params[3]) << 24),
            duration_ms,
        })
    }

    /// This slot decoded as a chain/beam proc, or `None` when it is not one — either the type key
    /// is not a chain key, or it is but `CharParamZero` decodes to `0`, which names no
    /// `SpellChainEffects` row and is how the shipped table writes an unused slot (`chain`'s
    /// module docs; the client's own null-row test at `0x6ecc2e` is the same no-op).
    pub fn as_chain(&self) -> Option<ChainProc> {
        if !char_proc_type::is_chain(self.ty) {
            return None;
        }
        let effect_id = self.small_int(0);
        (effect_id != 0).then(|| ChainProc {
            effect_id,
            beams: self.small_int(1).min(CHAIN_MAX_BEAMS),
            flag: self.small_int(2) != 0,
            ty: self.ty,
        })
    }
}

/// The client's decode of a small integer stored in a `CharProc` **float** param column:
/// `bits(param + 512.0f) >> 14 & 0xff`. Adding 512 forces the exponent, parking the integer part
/// in known mantissa bits; the shift and mask lift it back out. Used at `0x5d55c0` (the
/// dynobject shard index) and identically by the chain proc for all three of its integer params
/// (`0x60db19`–`0x60db6d`, decision 0955).
///
/// The client applies **no bounds check** of its own — `mov cl,al` takes the byte and indexes with
/// it — so callers clamp or bounds-test as the consumer requires.
pub fn char_proc_small_int(param: f32) -> u32 {
    (param + 512.0).to_bits() >> 14 & 0xff
}

/// A [`char_proc_type::WEAPON_TRAIL`] slot decoded — the two words the dispatcher latches onto the
/// unit (`unit+0xd1c` / `unit+0xd20`) and the trail object copies into its `SWING` record
/// (`+0x608` / `+0x60c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailProc {
    /// `ftol(CharParamZero) | (ftol(CharParamThree) << 24)` — a packed **`0xAARRGGBB`**, kept as
    /// the one dword the reference actually stores. Every shipped `CharParamZero` is a
    /// `0x00RRGGBB` under `2^24`, so the OR never collides; read the channels through [`Self::rgb`]
    /// and [`Self::alpha`] rather than re-deriving the shifts.
    pub packed: u32,
    /// `ftol(CharParamTwo)` — how long the trail keeps **appending**, in milliseconds. Never `0`
    /// (that is the reference's own no-fire test, so it decodes to `None`). 600 on the ordinary
    /// melee kits, 1000 on Charge, **10000** on Whirlwind's whole spin.
    pub duration_ms: u32,
}

impl TrailProc {
    /// The trail's colour, `[R, G, B]` — kit 324's `0xf82929` is `[248, 41, 41]`.
    pub fn rgb(&self) -> [u8; 3] {
        [
            (self.packed >> 16) as u8,
            (self.packed >> 8) as u8,
            self.packed as u8,
        ]
    }

    /// The trail's starting alpha, `0..=255`. `100` on every shipped row (≈ 39 %) — and it is a
    /// *clock* as much as an opacity: the fade rate, the retained segment count and the
    /// termination test are all `alpha`-derived (`0x6c6560`).
    pub fn alpha(&self) -> u8 {
        (self.packed >> 24) as u8
    }
}

/// The `CharProc` type keys benilla models, by name. The full key space is the dispatcher's 9 cases
/// (`0x60d7c0`); these are the ones a live 5875 **state** kit uses — every other key is either
/// cast-stage-only or unused by the shipped table (see `charprocs` in `benilla-extract`).
pub mod char_proc_type {
    /// **Body TINT** (`0x60d840`): `round(params[0])` is a packed `0x00RRGGBB`, OR'd with
    /// `0xff000000` into the tint node (`node+0x7c`), list `unit+0xce0`. Per-frame the head node's
    /// RGB goes `×1/255` into `model+0x184/188/18c` (the ghost's pale blue-white `0xFF8CB9FD`).
    pub const TINT: i32 = 1;
    /// **Body TRANSLUCENCY** (`0x60d972`): `params[0]` is this aura's alpha factor, stored in a
    /// spell-id-keyed node (`node+0x78`) on the unit's list `unit+0xb50`. The unit's effective alpha
    /// is `baseAlpha × Π(node alphas)` (`0x60d180`), ramped to over 1000 ms by
    /// `StartAlphaFade 0x614f80`. Stealth's 0.3 and the ghost aura's 0.5 are both this proc.
    pub const ALPHA: i32 = 14;
    /// **Body ANIMATION RATE** (`0x60db7e`) — the freeze. `params[0]` is a playback *rate*, written
    /// straight onto the unit's model clocks by `SetBoneAnimSpeed 0x712910`: the mount's bone 0
    /// (`[unit+0xdc]`, `-1`), the body's **key-bone 4** (SpineLow — the upper-body split track that
    /// carries cast/emote one-shots) when the model has one (`0x711d20(4)` gates it), and the body's
    /// bone 0 (`-1`). The previous rates are saved in the node (`+0x60`/`+0x64`/`+0x68`) and written
    /// back when it expires with the aura (`0x6203e0`, gated on the node's own `+0x2c & 0x4`).
    ///
    /// The rate is the per-bone `+0xb0` the animation clock multiplies its window by
    /// (`0x7145ae`: `t = trunc(f32(window) × rate) + bias`), and **arming an animation never
    /// touches it** — op4 `0x7121a0`'s success leg writes `+0xa4`/cursors and leaves `+0xb0` alone
    /// (only its disarm leg and `0x712910` write it). So the value persists across every re-arm for
    /// the aura's whole life, and `0x712910` re-bases `bias` so the *current frame* is preserved
    /// across the rate change: **rate 0 holds the pose exactly where it was.**
    ///
    /// 15 kits carry it; 14 ship `params[0] = 0` — Ice Block (kit 3709), Freeze, Freeze Solid,
    /// Flash Freeze, Petrify, Encage, Stilled. Kit 1744 (Stoned / Petrification / Thadius Spawn)
    /// ships `8947848.0` = `0x888888`, which reads like a packed grey that landed in the rate
    /// column; it is passed through, not special-cased.
    pub const ANIM_RATE: i32 = 11;

    /// **Weapon SWING TRAIL** (`0x60d80a`) — the ribbon a melee ability smears between its
    /// weapon model's `$WTB` and `$WTT` event markers. The arm is a **latch on the unit**, not a
    /// same-call effect: the proc writes `unit+0xd1c` (the packed colour) and `unit+0xd20` (the
    /// duration in ms), and the unit's **next** `PlayAnimation` (`0x5fe2f0`, the field's sole
    /// reader at `0x5fe48e`) hands both to each of its two weapon-hand trail objects and clears
    /// the duration — so it fires exactly once per arm. See [`CharProc::as_weapon_trail`].
    ///
    /// 34 of the 1772 kits carry it and for **18 of them it is the entire visual** — kit 324
    /// (Heroic Strike / Overpower / Mortal Strike / Bloodthirst / Rend, 218 spells), 3050
    /// (Hamstring), 557 (Sunder Armor) name no emitter slot at all, so a client without this proc
    /// renders nothing for them beyond the swing animation.
    pub const WEAPON_TRAIL: i32 = 8;

    /// **Chain / beam visual**, the key the shipped table uses on **channel**-stage kits — Drain
    /// Life's rope, Mind Flay's mana beam, Health Funnel, C'Thun's eye beam. Both this and
    /// [`CHAIN_CAST`] land on the same dispatcher case (`0x60da79`); the name records the data's
    /// convention, and behaviour keys off [`ChainProc::flag`](crate::ChainProc::flag). See the
    /// `chain_effects` module docs for the whole mechanism (decision 0955).
    pub const CHAIN_CHANNEL: i32 = 0;
    /// **Chain / beam visual**, the key the shipped table uses on **cast**-stage kits — Chain
    /// Lightning, Chain Heal, Chain Burn, Shrink Ray, the elemental Weakness debuffs. See
    /// [`CHAIN_CHANNEL`].
    pub const CHAIN_CAST: i32 = 12;

    /// Whether this type key reaches the beam case. **Both** chain keys do, and the dispatcher's
    /// translation table (`0x60dc20`) draws no distinction between them — so every consumer asks
    /// this rather than comparing against one constant.
    pub fn is_chain(ty: i32) -> bool {
        ty == CHAIN_CHANNEL || ty == CHAIN_CAST
    }
}

/// One `SpellVisual.dbc` row: the five lifecycle-stage `SpellVisualKit` ids a cast may fire
/// through (`0` = no kit at that stage, this crate's usual absent-FK convention) + the missile
/// block's live columns (model, dest-attach, flight sound — module docs; phase 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisualStages {
    pub precast: u32,
    pub cast: u32,
    pub impact: u32,
    pub state: u32,
    pub channel: u32,
    /// Field 7: the projectile's `SpellVisualEffectName` id (`0` here covers the client's `<1`
    /// ammo-fallback gate — the raw column is never negative on the real table). The missile
    /// *exists* whenever `Spell.dbc` Speed > 0, model or not.
    pub missile_model: u32,
    /// Field 9: the destination-attachment ordinal — index [`MISSILE_ATTACH_TABLE`] for the M2
    /// attach tag the missile homes to on a live target.
    pub missile_attach: u32,
    /// Field 10: the `SoundEntries.dbc` id the missile sounds while it travels — a LOOPING whoosh
    /// on the thrown/ranged projectiles (the thrown dagger's `WeaponLoop.wav`, id 3318), the
    /// client's per-missile loop handle (`CMissile+0x44`; its volume is proximity
    /// -shaped by `0x61d790`). `None` = the row names no flight sound. Rung by
    /// `crate::entities::missile`, stopped when the projectile arrives.
    pub missile_sound: Option<u32>,
    /// Field 14: the `SoundEntries.dbc` id the `$TRD` anim event rings at the work/craft strike
    /// keyframe — the client's `$TRD` handler `0x62faa0` resolves the in-flight spell's visual to
    /// this 16-dword row and plays its dword 14 (`[row+0x38]`; decision 0562). Mining's visual 93
    /// carries 1143 "Mining Impact" (the pick clang), Herb's 91 carries 1142, the smithing
    /// crafts' 395 carries 1143 (the hammer). `None` = no strike sound.
    pub strike_sound: Option<u32>,
    /// Field 6 (`+0x18`): the missile gate the dest one-shot checks — `SMSG_SPELL_GO` spawns a
    /// CEffect at the packet's DEST point only when this is **0** and [`Self::area_effect`] is
    /// set (the client's `0x6e8088`–`0x6e8143`). Nonzero =
    /// the missile owns the arrival, no dest one-shot.
    pub missile_gate: u32,
    /// Field 11 (`+0x2c`): the gate on a DynamicObject's **own** `.mdx` (visual A) — the model
    /// resolve at `0x5d57c0` requires this ≠ 0 before reading [`Self::area_effect`].
    pub area_gate: u32,
    /// Field 12 (`+0x30`): the `SpellVisualEffectName` id of the **dest-anchored model** — a
    /// DynamicObject's own `.mdx` (visual A, gated by [`Self::area_gate`]) and the GO dest
    /// one-shot's model (gated by [`Self::missile_gate`] == 0). NOT the kit table.
    pub area_effect: u32,
    /// Field 13 (`+0x34`): the `SpellVisualKit` id of the **area kit** — a DynamicObject's
    /// emitter chain (`0x5d55c0`: CharProcType scan for 9 → the hardcoded shard-model table)
    /// and its looping area sound (the kit's own field-13 SoundEntries). `0` = none.
    pub area_kit: u32,
}

impl VisualStages {
    /// The client's **ranged weapon-visual merge** — a per-FIELD zero-fill, not a per-row
    /// fallback (`0x60d450`, byte-read end to end for decision 0986; this SUPERSEDES 0370's
    /// row-level reading, whose disassembly stopped at the null-own-visual arm).
    ///
    /// The resolver reaches the fill from **both** arms, and they converge on one block:
    ///
    /// ```text
    /// 60d4b4  test esi,esi ; jne 0x60d555   ; esi = the spell's OWN SpellVisual row
    /// 60d4bc  xor eax,eax                   ; -- null-own arm: zero outKit ...
    /// 60d4c6  rep stosd                     ;    ... then fall through into the fill
    /// 60d555  rep movsd                     ; -- own arm: copy the OWN row into outKit ...
    /// 60d561  jmp 0x60d4d2                  ;    ... and jump INTO the same fill
    /// 60d4d2  test ebx,ebx ; je 0x60d54c    ; ebx = the WEAPON's SpellVisual row (0 if none)
    /// 60d4d6..60d54c                        ; per field: `if out.F == 0 { out.F = weapon.F }`
    /// ```
    ///
    /// So a RANGED-attribute spell (`Attributes & 0x2`) that **has** its own visual still borrows
    /// the equipped ranged weapon's row for every slot its own row leaves at zero. That is how a
    /// hunter shot animates at all: Serpent Sting (visual 3179), Multi-Shot (567), Arcane Shot
    /// (3299), Aimed/Concussive (3180), Viper/Scorpid Sting (3181/3219) and Volley (3300) every
    /// one carry `precast = cast = 0` beside a populated impact + missile block, and take
    /// **LoadBow (kit 7) → AttackBow (kit 164)** off the bow's `ItemDisplayInfo` col-10 visual 5
    /// (thrown → 98, gun → 224). Read as a row-level fallback they got no caster fire clip at all.
    ///
    /// The merged set is exactly the client's eight sites. The two it pointedly does NOT touch are
    /// [`Self::state`] and [`Self::channel`] (`+0x10`/`+0x14`), nor the dest-anchored
    /// `+0x2c/+0x30/+0x34` block. Field 8 (`missilePathType`, `+0x20`) is merged there too; it is
    /// dead-by-absence, so this struct does not carry it.
    ///
    /// The missile pair is the one non-uniform site (`60d4fd`–`60d517`): the gate is written as a
    /// **literal 1**, not the weapon's own value, and the model only comes across with it.
    #[must_use]
    pub fn merged_over_weapon(&self, weapon: &VisualStages) -> VisualStages {
        let fill = |own: u32, w: u32| if own == 0 { w } else { own };
        let mut out = *self;
        out.precast = fill(self.precast, weapon.precast); // 60d4d6
        out.cast = fill(self.cast, weapon.cast); // 60d4e3
        out.impact = fill(self.impact, weapon.impact); // 60d4f0
        if self.missile_gate == 0 && weapon.missile_gate != 0 {
            out.missile_gate = 1; // 60d50b — the literal, not the weapon's value
            out.missile_model = weapon.missile_model; // 60d512
        }
        out.missile_attach = fill(self.missile_attach, weapon.missile_attach); // 60d525
        out.missile_sound = self.missile_sound.or(weapon.missile_sound); // 60d532
        out.strike_sound = self.strike_sound.or(weapon.strike_sound); // 60d53f
        out
    }
}

/// One `SpellVisualKit.dbc` row's body-animation + sound + the nine attach-point emitter slots
/// (the columns consumed through decision 0099 phase 3; see the module doc for what remains).
// No `Eq`: the `CharProc` params are floats (the client's own `fld` columns).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VisualKit {
    /// The `AnimationData.dbc` id this kit plays on the unit (`None` = no animation for this kit
    /// — see the module doc's none-sentinel finding; common on impact kits).
    pub anim_id: Option<u16>,
    /// The `SoundEntries.dbc` kit id this stage rings (`None` = silent).
    pub sound: Option<u32>,
    /// The nine emitter slots (kit fields 3–11): each a `SpellVisualEffectName` id, in kit-field
    /// order — slot `i` attaches at [`KIT_SLOT_TAGS`]`[i]`. Both none-sentinels fold to `None`
    /// like [`Self::anim_id`]. Iterate populated slots via [`Self::effects`].
    pub effect_slots: [Option<u32>; 9],
    /// Kit field 12 (`kit+0x30`) — the **tenth effect slot** (module docs; decision 0848). The
    /// client plays it inline in `PlaySpellVisualKit` via `0x61fcf0`, same `CEffect` lifecycle as
    /// the nine bone-attach slots. On the shipped table it carries the kit's body/ground **state
    /// model** — the frozen ice, the net, the roots, the Thunderclap ring — never a projectile
    /// (the real missile is `SpellVisual` field 7's, [`VisualStages::missile_model`]). Folded
    /// into [`Self::effects`] at [`WORLD_EFFECT_TAG`].
    pub world_effect: Option<u32>,
    /// Kit field 14 (`kit+0x38`) — the **`SpellEffectCameraShakes.dbc` group** this kit shakes the
    /// camera with (`None` = no shake). Community name `ShakeID`; the value is a *group* id, never a
    /// `CameraShakes` row id — see [`crate::SpellShakeGroup`]. 58 of the 1772 shipped kits carry
    /// one, and every value lands on the group table's nine sparse ids `{3, 4, 5, 6, 7, 26, 66}`.
    pub shake: Option<u32>,
    /// The four `CharProc` slots (kit fields 15–34, transposed out of the five parallel arrays).
    /// An unfilled slot is `None` — see [`char_proc_slot`] for the sentinel law; iterate the filled
    /// ones via [`Self::char_procs`]. **Two consumers, two halves of the same table:** the *body*
    /// procs an aura's state kit installs (type 14 translucency / type 1 tint — `benilla`'s
    /// `aura_visual`, decision 0806), and the **dynobj emitter chain**, which scans for the FIRST
    /// type **9** slot — there `params[0]` encodes the shard-model table index (the exact small-int
    /// decode `bits(params[0] + 512.0) >> 14 & 0xff`, `0x5d55c0`) and `params[1]` is the emit rate
    /// the graphics-quality factor multiplies (`0x6eb930`, decision 0797).
    pub char_proc_slots: [Option<CharProc>; KIT_CHAR_PROCS],
}

impl VisualKit {
    /// The filled `CharProc` slots, in slot order. The client walks all four unconditionally and
    /// lets the dispatch table drop the unnamed keys; we drop the empty *slots* here and leave the
    /// key switch to the consumer.
    pub fn char_procs(&self) -> impl Iterator<Item = CharProc> + '_ {
        self.char_proc_slots.iter().flatten().copied()
    }

    /// The `params[0]` of this kit's first `CharProc` slot of type `ty`, if any — the shape both
    /// modelled procs want (a single float: the alpha factor, the packed tint).
    pub fn char_proc_param(&self, ty: i32) -> Option<f32> {
        self.char_procs().find(|p| p.ty == ty).map(|p| p.params[0])
    }

    /// This kit's weapon-trail proc — the **first** slot that decodes to one, matching the
    /// dispatcher's in-order walk (no shipped kit carries two type-8 slots). `None` when the kit
    /// arms no trail. See [`char_proc_type::WEAPON_TRAIL`].
    pub fn trail_proc(&self) -> Option<TrailProc> {
        self.char_procs().find_map(|p| p.as_weapon_trail())
    }

    /// This kit's chain/beam proc — the **first** slot that decodes to one, matching the client's
    /// dispatcher walk (it runs every slot in order; no shipped kit carries two). `None` when the
    /// kit draws no beam. See [`chain_effects`] for the mechanism (decision 0955).
    pub fn chain_proc(&self) -> Option<ChainProc> {
        self.char_procs().find_map(|p| p.as_chain())
    }

    /// The populated emitter slots as `(M2 attachment id, SpellVisualEffectName id)` pairs, in
    /// kit-field order — the client fires **all** populated slots at **every** stage (stage sets
    /// lifetime policy only, `0x60edf0`) — then [`Self::world_effect`]
    /// last, under the [`WORLD_EFFECT_TAG`] world-plant sentinel (decisions 0848/0850).
    pub fn effects(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.effect_slots
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.map(|id| (KIT_SLOT_TAGS[i], id)))
            .chain(self.world_effect.map(|id| (WORLD_EFFECT_TAG, id)))
    }
}

/// `SpellVisual.dbc` × `SpellVisualKit.dbc` × `SpellVisualEffectName.dbc`, each kept in its own
/// id space (three different tables' keys — never conflate them, mirroring
/// [`crate::AreaSoundCatalog`]'s per-table map split).
pub struct SpellVisualCatalog {
    visuals: HashMap<u32, VisualStages>,
    kits: HashMap<u32, VisualKit>,
    /// `SpellVisualEffectName` id → the effect model's `.mdx` path (field 2 — the table's one
    /// column the kit slots consume).
    effect_paths: HashMap<u32, String>,
    /// The `"HARDCODED *"`-named rows, name → (record id, path) — the client's engine-spawned effect set,
    /// resolved BY NAME once at boot exactly like this (`0x61f5b0` matches a 14-string baked
    /// table against the name column: loot art, footsteps, breath, level-up…). Three consumers
    /// today: "HARDCODED Loot Art"
    /// (id 14 → `Particles\LootFX.mdl`, the corpse sparkle), "HARDCODED Unit Level Up"
    /// (id 21 → `Spells\LevelUp\LevelUp.mdl`, the ding) and "HARDCODED Mount Poof"
    /// (id 1185 → `Spells\DruidMorph_Impact_Base.mdx`, the mount-up cloud — decision 0927).
    hardcoded: HashMap<String, (u32, String)>,
    /// `SpellChainEffects` id → the beam's geometry/animation row ([`chain_effects`], decision 0955).
    /// Reached only through a kit's [`VisualKit::chain_proc`].
    chain_effects: HashMap<u32, ChainEffect>,
}

impl SpellVisualCatalog {
    /// Build a catalog from explicit visual/kit tables — for tests and synthetic fixtures (the
    /// [`crate::SpellCatalog::from_displays`] convention). The live path is
    /// [`load_spell_visual_catalog`]. Carries no effect paths and no hardcoded rows.
    pub fn from_tables(visuals: HashMap<u32, VisualStages>, kits: HashMap<u32, VisualKit>) -> Self {
        Self::from_tables_with_paths(visuals, kits, HashMap::new())
    }

    /// [`Self::from_tables`] plus effect paths, for fixtures exercising the kit → effect-model
    /// chain. The live path is [`load_spell_visual_catalog`].
    pub fn from_tables_with_paths(
        visuals: HashMap<u32, VisualStages>,
        kits: HashMap<u32, VisualKit>,
        effect_paths: HashMap<u32, String>,
    ) -> Self {
        Self {
            visuals,
            kits,
            effect_paths,
            hardcoded: HashMap::new(),
            chain_effects: HashMap::new(),
        }
    }

    /// Seed one `SpellChainEffects` row, for fixtures exercising the beam chain. The live path is
    /// [`load_spell_visual_catalog`].
    #[must_use]
    pub fn with_chain_effect(mut self, id: u32, effect: ChainEffect) -> Self {
        self.chain_effects.insert(id, effect);
        self
    }

    /// Seed one `"HARDCODED …"` name → (record id, model path), for fixtures exercising the
    /// engine-spawned effects (the loot sparkle, the level-up ding, the mount poof). The live
    /// path is the boot name-resolve inside [`load_spell_visual_catalog`], which is what
    /// `0x61f5b0` does. The **id** is carried because it is half the reference's same-slot
    /// replace key (`0x6208e0`; decision 2057) — an engine-spawned effect dedups exactly like a
    /// kit slot's.
    #[must_use]
    pub fn with_hardcoded(mut self, name: &str, id: u32, path: &str) -> Self {
        self.hardcoded
            .insert(name.to_string(), (id, path.to_string()));
        self
    }

    /// A `SpellVisual.dbc` row's stage kits by its id (`Spell.dbc` column 115, `spells::SpellDisplay::visual`).
    pub fn stages(&self, visual_id: u32) -> Option<&VisualStages> {
        self.visuals.get(&visual_id)
    }

    /// A `SpellVisualKit.dbc` row by its id (one of [`VisualStages`]'s five fields).
    pub fn kit(&self, kit_id: u32) -> Option<&VisualKit> {
        self.kits.get(&kit_id)
    }

    /// The number of `SpellVisual` rows loaded.
    pub fn len(&self) -> usize {
        self.visuals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visuals.is_empty()
    }

    /// Every loaded `SpellVisual` row as `(id, stages)`, unordered — for whole-table census
    /// instruments (`benilla-extract shakecensus` walks it to find which effect models a
    /// **DynamicObject** can host). Runtime consumers resolve one by id through [`Self::stages`].
    pub fn visuals(&self) -> impl Iterator<Item = (u32, &VisualStages)> + '_ {
        self.visuals.iter().map(|(id, s)| (*id, s))
    }

    /// Every loaded `SpellVisualKit` id, ascending — for whole-table census instruments
    /// (`benilla-extract charprocs`). Runtime consumers resolve a kit by id through [`Self::kit`].
    pub fn kit_ids(&self) -> impl Iterator<Item = u32> + '_ {
        let mut ids: Vec<u32> = self.kits.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
    }

    /// The number of `SpellVisualKit` rows loaded.
    pub fn kit_len(&self) -> usize {
        self.kits.len()
    }

    /// A `SpellVisualEffectName` id's effect-model `.mdx` path (one of a kit's
    /// [`VisualKit::effects`] slot values). `None` for an unknown id or an empty path.
    pub fn effect_path(&self, effect_id: u32) -> Option<&str> {
        self.effect_paths
            .get(&effect_id)
            .map(String::as_str)
            .filter(|p| !p.is_empty())
    }

    /// An engine-spawned hardcoded effect's `(record id, model path)`, by the client's own baked
    /// name string ([`SpellVisualCatalog::hardcoded`]). Mirrors the client's boot name-resolve
    /// (`0x61f5b0`, stricmp-family — hence the case-insensitive compare over the tiny set);
    /// `None` when the shipped table names no such row. The id rides along because every consumer
    /// hands it to the same-slot replace (`0x6208e0`, decision 2057).
    pub fn hardcoded_effect(&self, name: &str) -> Option<(u32, &str)> {
        self.hardcoded
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, (id, path))| (*id, path.as_str()))
            .filter(|(_, p)| !p.is_empty())
    }

    /// The `"HARDCODED Loot Art"` row — the lootable-corpse sparkle, riding the shared hardcoded
    /// map (one name-resolve mechanism for the whole engine-spawned set).
    pub fn loot_art_effect(&self) -> Option<(u32, &str)> {
        self.hardcoded_effect("HARDCODED Loot Art")
    }

    /// A `SpellChainEffects` row by its id — the beam a kit's [`VisualKit::chain_proc`] names.
    /// `None` for an id with no row, which is exactly the client's own null-row no-op and is how
    /// the shipped table's padding slots resolve ([`chain_effects`]).
    pub fn chain_effect(&self, id: u32) -> Option<&ChainEffect> {
        self.chain_effects.get(&id)
    }

    /// The number of `SpellChainEffects` rows loaded (18 on the shipped table).
    pub fn chain_effect_len(&self) -> usize {
        self.chain_effects.len()
    }
}

fn n_u32_schema(name: &str, fields: usize) -> Schema {
    let mut s = Schema::new(name);
    for i in 0..fields {
        s.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32));
    }
    s
}

/// `SpellVisualEffectName`'s 5-field schema: id · name (string — the HARDCODED-effect lookup
/// key) · model `.mdx` path (string) · two dead columns (module docs).
fn effect_name_schema() -> Schema {
    let mut s = Schema::new("SpellVisualEffectName");
    for i in 0..5 {
        let ty = if i == 1 || i == 2 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// `SpellVisualKit`'s 35-field schema: fields 0–14 are the ids/FKs (`u32`), **15–18 the signed
/// `CharProcType` keys** and **19–34 the four float param arrays** — the client loads the params
/// with `fld` (the dispatcher `0x60d7c0`), so they are typed here rather than bit-cast at
/// every read.
fn kit_schema() -> Schema {
    let mut s = Schema::new("SpellVisualKit");
    for i in 0..SPELL_VISUAL_KIT_FIELDS {
        let ty = match i {
            CHAR_PROC_TYPE_FIELD..CHAR_PROC_PARAM_FIELD => FieldType::Int32,
            f if f >= CHAR_PROC_PARAM_FIELD => FieldType::Float32,
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Kit field 15 — `CharProcType[0]`, the first of the four type keys (@+0x3c).
const CHAR_PROC_TYPE_FIELD: usize = 15;
/// Kit field 19 — `CharParamZero[0]`, the first param column (@+0x4c); `CharParamOne/Two/Three[0]`
/// follow every [`KIT_CHAR_PROCS`] fields (@+0x5c/+0x6c/+0x7c).
const CHAR_PROC_PARAM_FIELD: usize = CHAR_PROC_TYPE_FIELD + KIT_CHAR_PROCS;

/// One kit's `CharProc` slot `i`, transposed out of the five parallel arrays.
///
/// **The empty-slot sentinel is `-1`, and `0` is a REAL key** (corrected by decision 0955). This
/// used to fold `<= 0` to `None`, on the premise that "the dispatcher's translation table sends
/// everything it doesn't name to the no-op case". The table names `0`: reading `0x60dc20`
/// byte-for-byte, type **0 routes to case 0 (`0x60da79`) — the chain/beam case**, the same case
/// type 12 reaches. Folding it away silently discarded **34 of the 48 live beams** in the shipped
/// table, which is the whole of B161 ("Chain Lightning has no chain effect") and every missing
/// drain/channel beam with it.
///
/// A type-`0` slot that is genuinely padding still costs nothing: its `CharParamZero` decodes to
/// `0`, which names no `SpellChainEffects` row, and both the client (`0x6ecc2e`'s null-row test)
/// and [`CharProc::as_chain`] no-op it. 20 of the 54 shipped type-0 slots are that case.
fn char_proc_slot(r: &benilla_dbc::Record, i: usize) -> Option<CharProc> {
    let ty = i32_at(r, CHAR_PROC_TYPE_FIELD + i)?;
    if ty < 0 {
        return None;
    }
    let mut params = [0.0; 4];
    for (p, param) in params.iter_mut().enumerate() {
        *param = f32_at(r, CHAR_PROC_PARAM_FIELD + p * KIT_CHAR_PROCS + i).unwrap_or(0.0);
    }
    Some(CharProc { ty, params })
}

/// Fold the DBC's dual none-encoding (module docs) to `Option<u32>`.
fn some_unless_none(v: u32) -> Option<u32> {
    (v != 0 && v != NONE_SENTINEL).then_some(v)
}

/// Read `SpellVisual.dbc` + `SpellVisualKit.dbc` off the patch chain.
pub fn load_spell_visual_catalog(chain: &mut Chain) -> Result<SpellVisualCatalog> {
    let sv_bytes = chain
        .read_file(SPELL_VISUAL)
        .context("reading SpellVisual.dbc")?;
    let sv_set = parse(
        &sv_bytes,
        n_u32_schema("SpellVisual", SPELL_VISUAL_FIELDS),
        "SpellVisual.dbc",
    )?;
    let mut visuals = HashMap::with_capacity(sv_set.records().len());
    for r in sv_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let g = |i: usize| u32_at(r, i).unwrap_or(0);
        visuals.insert(
            id,
            VisualStages {
                precast: g(1),
                cast: g(2),
                impact: g(3),
                state: g(4),
                channel: g(5),
                missile_model: g(7),
                missile_attach: g(9),
                missile_sound: u32_at(r, 10).and_then(some_unless_none),
                strike_sound: u32_at(r, 14).and_then(some_unless_none),
                missile_gate: g(6),
                area_gate: g(11),
                area_effect: g(12),
                area_kit: g(13),
            },
        );
    }

    let svk_bytes = chain
        .read_file(SPELL_VISUAL_KIT)
        .context("reading SpellVisualKit.dbc")?;
    let svk_set = parse(&svk_bytes, kit_schema(), "SpellVisualKit.dbc")?;
    let mut kits = HashMap::with_capacity(svk_set.records().len());
    for r in svk_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let anim_id = u32_at(r, 2).and_then(some_unless_none).map(|a| a as u16);
        let sound = u32_at(r, 13).and_then(some_unless_none);
        let mut effect_slots = [None; 9];
        for (i, slot) in effect_slots.iter_mut().enumerate() {
            *slot = u32_at(r, 3 + i).and_then(some_unless_none);
        }
        let mut char_proc_slots = [None; KIT_CHAR_PROCS];
        for (i, slot) in char_proc_slots.iter_mut().enumerate() {
            *slot = char_proc_slot(r, i);
        }
        kits.insert(
            id,
            VisualKit {
                anim_id,
                sound,
                effect_slots,
                world_effect: u32_at(r, 12).and_then(some_unless_none),
                shake: u32_at(r, 14).and_then(some_unless_none),
                char_proc_slots,
            },
        );
    }

    let sven_bytes = chain
        .read_file(SPELL_VISUAL_EFFECT_NAME)
        .context("reading SpellVisualEffectName.dbc")?;
    let sven_set = parse(
        &sven_bytes,
        effect_name_schema(),
        "SpellVisualEffectName.dbc",
    )?;
    let mut effect_paths = HashMap::with_capacity(sven_set.records().len());
    let mut hardcoded = HashMap::new();
    for r in sven_set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(path) = str_at(&sven_set, r, 2) {
            // The engine-spawned set is looked up by the name column — keep exactly the rows
            // the client's own boot name-resolve can hit (the "HARDCODED " prefix; its
            // matchers are stricmp-family, so lookups compare case-insensitively).
            if let Some(name) = str_at(&sven_set, r, 1).filter(|n| n.starts_with("HARDCODED ")) {
                hardcoded.insert(name, (id, path.clone()));
            }
            effect_paths.insert(id, path);
        }
    }

    let chain_effects = chain_effects::load(chain).context("reading SpellChainEffects.dbc")?;

    Ok(SpellVisualCatalog {
        visuals,
        kits,
        effect_paths,
        hardcoded,
        chain_effects,
    })
}

#[cfg(test)]
mod tests;
