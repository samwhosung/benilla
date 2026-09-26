//! `SpellVisual.dbc`, `SpellVisualKit.dbc`, `SpellVisualEffectName.dbc` and
//! `SpellChainEffects.dbc`: what a cast shows at each stage (the client's loaders `0x5508e0`,
//! `0x550690` and `0x550460`; vmangos `SpellVisualEntry`, `DBCStructure.h:613-640`). A stage sets
//! lifetime only: every populated slot of a reached kit fires, precast persists until reaped by
//! spell id, and cast and impact end on their own.

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

/// Each emitter slot's M2 `AttachmentID`, kit fields 3-11 in order, as the client's slot loop
/// (`0x60edf0`) pushes them: head, chest, base, left hand, right hand, breath, special 1-3.
pub const KIT_SLOT_TAGS: [u16; 9] = [0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19];

/// The tag for kit field 12, the client's -1: `0x61fcf0` passes no attach tag (`0x61fd23`), so the
/// placement walk (`0x620be0`) plants the model once in world space at the owner's position, facing
/// and scale (`0x620c86`); it rides no bone and does not turn with the unit.
pub const WORLD_EFFECT_TAG: u16 = u16::MAX;

/// The client's missile destination attachments (`0x860a18`): `SpellVisual` field 9 indexes it
/// for the M2 attachment the missile homes to on a live target.
pub const MISSILE_ATTACH_TABLE: [u16; 11] = [
    0x14, 0x22, 0x13, 0x15, 0x16, 0x11, 0x17, 0x18, 0x19, 0xf, 0x10,
];

/// The data writes "no value" in an id column as 0 or as this, inconsistently; both read `None`.
const NONE_SENTINEL: u32 = u32::MAX;

/// `CharProc` slots per kit; the dispatcher (`0x60d7c0`) walks exactly four.
pub const KIT_CHAR_PROCS: usize = 4;

/// One `CharProc` slot, a type key and four float params. The dispatcher (`0x60d7c0`) switches on
/// the type through a byte table (`0x60dc20`) into nine cases (`0x60dbfc`), each reading the params
/// it wants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharProc {
    /// `CharProcType[i]`; never -1, as an empty slot is `None` in the kit.
    pub ty: i32,
    /// `CharParamZero..Three[i]`: floats the client loads with `fld`, rounding for an integer.
    pub params: [f32; 4],
}

impl CharProc {
    pub fn small_int(&self, i: usize) -> u32 {
        char_proc_small_int(self.params[i])
    }

    /// This slot as a weapon swing trail. The type-8 arm (`0x60d80a`) truncates with `_ftol`, not
    /// the small-int decode: `unit+0xd1c = zero | three << 24`, `unit+0xd20 = two`. `CharParamOne`
    /// is not read; it holds 20.0 on 22 of the 34 trail kits and 0 to 25 on the rest. A zero
    /// duration fires nothing (`0x5fe494`), so it reads as `None`.
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

    /// This slot as a beam; `None` too when `CharParamZero` decodes to 0, which names no row and is
    /// how the data writes an unused slot (the client's null-row test, `0x6ecc2e`).
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

/// The client's read of a small integer from a `CharProc` float param:
/// `bits(param + 512.0) >> 14 & 0xff`, where adding 512 fixes the exponent so the integer sits in
/// known mantissa bits. Used for the dynobject shard index (`0x5d55c0`) and the chain proc's params
/// (`0x60db19`); the client does no bounds check, so callers must.
pub fn char_proc_small_int(param: f32) -> u32 {
    (param + 512.0).to_bits() >> 14 & 0xff
}

/// A decoded weapon trail: the two words latched on the unit (`unit+0xd1c`, `unit+0xd20`) and
/// copied into the trail's `SWING` record (`+0x608`, `+0x60c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrailProc {
    /// `0xAARRGGBB`, the dword the reference stores; every shipped `CharParamZero` is below 2^24,
    /// so the OR never collides.
    pub packed: u32,
    /// How long the trail keeps appending, in ms: 600 on ordinary melee kits, 1000 on Charge,
    /// 10000 on Whirlwind.
    pub duration_ms: u32,
}

impl TrailProc {
    pub fn rgb(&self) -> [u8; 3] {
        [
            (self.packed >> 16) as u8,
            (self.packed >> 8) as u8,
            self.packed as u8,
        ]
    }

    /// The starting alpha, 100 on 19 of the 34 trail kits; the fade rate, the segment count and
    /// the end test all derive from it (`0x6c6560`).
    pub fn alpha(&self) -> u8 {
        (self.packed >> 24) as u8
    }
}

/// The `CharProc` type keys benilla models, out of the dispatcher's nine cases (`0x60d7c0`).
pub mod char_proc_type {
    /// Body tint (`0x60d840`): `round(params[0])` is `0x00RRGGBB`, stored with alpha `0xff` in a
    /// node on `unit+0xce0`; each frame the head node's RGB / 255 goes to `model+0x184..0x18c`.
    pub const TINT: i32 = 1;
    /// Body translucency (`0x60d972`): `params[0]` is the aura's alpha factor, in a spell-keyed
    /// node linked at the head of `unit+0xb50`. The unit's alpha is `baseAlpha` times the head
    /// node's factor alone, never a product over the list (`0x60d180`), ramped over 1000 ms
    /// (`0x614f80`). Stealth's 0.3 and the ghost's 0.5 are this proc.
    pub const ALPHA: i32 = 14;
    /// Body animation rate (`0x60db7e`), the freeze: `SetBoneAnimSpeed` (`0x712910`) sets
    /// `params[0]` as the rate of the mount's bone 0, the body's key bone 4 when it has one
    /// (`0x711d20`) and the body's bone 0, re-basing so the current frame holds. Arming an
    /// animation never resets it (`0x7121a0`), so rate 0 holds the pose for the aura's life; the
    /// old rates return when the node expires (`0x6203e0`). Of the 15 kits a live spell reaches,
    /// 14 ship 0 (Ice Block, Freeze, Petrify, …) and kit 1744 ships 8947848.0 (`0x888888`),
    /// passed through as is.
    pub const ANIM_RATE: i32 = 11;

    /// Weapon swing trail (`0x60d80a`), the ribbon between a weapon's `$WTB` and `$WTT` markers.
    /// It latches on the unit: the next `PlayAnimation` (`0x5fe2f0`, reading at `0x5fe48e`) hands
    /// it to both weapon-hand trails and clears it, so it fires once per arm. For 21 of the 34
    /// kits carrying it (Heroic Strike's 324, Hamstring's 3050, Sunder's 557) it is the whole
    /// visual: they fill no effect slot.
    pub const WEAPON_TRAIL: i32 = 8;

    /// A beam, as the data keys it on channel-stage kits (Drain Life, Mind Flay). It reaches the
    /// same dispatcher case as [`CHAIN_CAST`] (`0x60da79`); behaviour keys off
    /// [`ChainProc::flag`](crate::ChainProc::flag).
    pub const CHAIN_CHANNEL: i32 = 0;
    /// A beam, as the data keys it on cast-stage kits (Chain Lightning, Chain Heal).
    pub const CHAIN_CAST: i32 = 12;

    /// Whether this type reaches the beam case. Both chain keys do (`0x60dc20`), so ask this rather
    /// than compare with one constant.
    pub fn is_chain(ty: i32) -> bool {
        ty == CHAIN_CHANNEL || ty == CHAIN_CAST
    }
}

/// One `SpellVisual.dbc` row: the kit ids for the five stages (fields 1-5, 0 for none), then the
/// missile and dest-anchored columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisualStages {
    pub precast: u32,
    pub cast: u32,
    pub impact: u32,
    pub state: u32,
    pub channel: u32,
    /// Field 7: the missile's `SpellVisualEffectName` id; 0 flies the ammo or weapon model, and an
    /// id with no row the client's `Spells\ErrorCube.mdx` (`0x60a3d0`). A missile exists whenever
    /// `Spell.dbc` Speed > 0, whatever this holds.
    pub missile_model: u32,
    /// Field 9: the index into [`MISSILE_ATTACH_TABLE`].
    pub missile_attach: u32,
    /// Field 10: the `SoundEntries.dbc` loop the missile plays in flight (`CMissile+0x40`, its
    /// volume shaped by distance at `0x61d790`), stopped when it arrives.
    pub missile_sound: Option<u32>,
    /// Field 14: the `SoundEntries.dbc` id the `$TRD` anim event plays at a craft's strike frame
    /// (`0x62faa0`), such as mining's pick clang, sound 1143.
    pub strike_sound: Option<u32>,
    /// Field 6: `SMSG_SPELL_GO` spawns the dest one-shot at the packet's dest only when this is 0
    /// and [`Self::area_effect`] is set (`0x6e8088`); the missile spawn ignores it.
    pub missile_gate: u32,
    /// Field 11: a DynamicObject shows its own model only when this is nonzero (`0x5d57c0`).
    pub area_gate: u32,
    /// Field 12: a `SpellVisualEffectName` id, not a kit: a DynamicObject's own model and the dest
    /// one-shot's.
    pub area_effect: u32,
    /// Field 13: a DynamicObject's kit, for its emitters (`0x5d55c0` scans it for CharProc type 9)
    /// and its looping sound.
    pub area_kit: u32,
}

impl VisualStages {
    /// The client's ranged weapon-visual merge (`0x60d450`): a ranged spell (`Attributes & 0x2`)
    /// fills each field its own row leaves 0 from the equipped ranged weapon's row. State, channel
    /// and the dest-anchored fields are never merged, and the missile gate comes across as a
    /// literal 1, bringing the model with it. Hunter shots animate this way: Multi-Shot and the
    /// stings carry precast = cast = 0 and take LoadBow (kit 7) and AttackBow (kit 164) from the
    /// bow's `ItemDisplayInfo` column-10 visual, 5 on most bows.
    #[must_use]
    pub fn merged_over_weapon(&self, weapon: &VisualStages) -> VisualStages {
        let fill = |own: u32, w: u32| if own == 0 { w } else { own };
        let mut out = *self;
        out.precast = fill(self.precast, weapon.precast); // 60d4d6
        out.cast = fill(self.cast, weapon.cast); // 60d4e3
        out.impact = fill(self.impact, weapon.impact); // 60d4f0
        if self.missile_gate == 0 && weapon.missile_gate != 0 {
            out.missile_gate = 1; // 60d50b: the literal, not the weapon's value
            out.missile_model = weapon.missile_model; // 60d512
        }
        out.missile_attach = fill(self.missile_attach, weapon.missile_attach); // 60d525
        out.missile_sound = self.missile_sound.or(weapon.missile_sound); // 60d532
        out.strike_sound = self.strike_sound.or(weapon.strike_sound); // 60d53f
        out
    }
}

/// One `SpellVisualKit.dbc` row.
// No `Eq`: the `CharProc` params are floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VisualKit {
    /// Field 2: the `AnimationData.dbc` id played on the unit.
    pub anim_id: Option<u16>,
    /// Field 13: the `SoundEntries.dbc` id.
    pub sound: Option<u32>,
    /// Fields 3-11: `SpellVisualEffectName` ids, slot `i` attaching at [`KIT_SLOT_TAGS`]`[i]`.
    pub effect_slots: [Option<u32>; 9],
    /// Field 12, a tenth effect slot the client plays inline (`0x61fcf0`) as a `CEffect` node
    /// marked as a world plant (`node+0x2c | 0x2`), with the nine slots' lifecycle (`0x614150`).
    /// The data puts state models here (Frost Nova's ice, Net, Entangling Roots) and caster-feet
    /// rings (Thunderclap), never a projectile.
    pub world_effect: Option<u32>,
    /// Field 14: a `SpellEffectCameraShakes.dbc` group id, never a `CameraShakes` row
    /// ([`crate::SpellShakeGroup`]); all 58 shipped values land on that table.
    pub shake: Option<u32>,
    /// Fields 15-34, transposed out of five parallel arrays. The dynobject emitter takes the first
    /// type-9 slot: `params[0]` is its shard model index, `params[1]` the emit rate the graphics
    /// quality scales (`0x6eb930`).
    pub char_proc_slots: [Option<CharProc>; KIT_CHAR_PROCS],
}

impl VisualKit {
    /// The filled `CharProc` slots, in slot order.
    pub fn char_procs(&self) -> impl Iterator<Item = CharProc> + '_ {
        self.char_proc_slots.iter().flatten().copied()
    }

    /// `params[0]` of the first slot of type `ty`: the alpha factor or the packed tint.
    pub fn char_proc_param(&self, ty: i32) -> Option<f32> {
        self.char_procs().find(|p| p.ty == ty).map(|p| p.params[0])
    }

    /// The first slot that decodes as a weapon trail; no shipped kit has two.
    pub fn trail_proc(&self) -> Option<TrailProc> {
        self.char_procs().find_map(|p| p.as_weapon_trail())
    }

    /// The first slot that decodes as a beam; no shipped kit has two.
    pub fn chain_proc(&self) -> Option<ChainProc> {
        self.char_procs().find_map(|p| p.as_chain())
    }

    /// `(attachment id, effect id)` for each populated slot in field order, then the world effect
    /// at [`WORLD_EFFECT_TAG`]; the client fires them all at every stage (`0x60edf0`).
    pub fn effects(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.effect_slots
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.map(|id| (KIT_SLOT_TAGS[i], id)))
            .chain(self.world_effect.map(|id| (WORLD_EFFECT_TAG, id)))
    }
}

/// The spell visual tables, each in its own id space.
pub struct SpellVisualCatalog {
    visuals: HashMap<u32, VisualStages>,
    kits: HashMap<u32, VisualKit>,
    /// `SpellVisualEffectName` id → model path (field 2).
    effect_paths: HashMap<u32, String>,
    /// The `"HARDCODED *"` rows, name → (id, path): the effects the engine spawns itself, which the
    /// client resolves by name at boot against 14 baked strings (`0x61f5b0`).
    hardcoded: HashMap<String, (u32, String)>,
    chain_effects: HashMap<u32, ChainEffect>,
}

impl SpellVisualCatalog {
    /// A catalog from explicit tables, for tests; no effect paths, hardcoded rows or beams.
    pub fn from_tables(visuals: HashMap<u32, VisualStages>, kits: HashMap<u32, VisualKit>) -> Self {
        Self::from_tables_with_paths(visuals, kits, HashMap::new())
    }

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

    /// Seed one `SpellChainEffects` row, for tests.
    #[must_use]
    pub fn with_chain_effect(mut self, id: u32, effect: ChainEffect) -> Self {
        self.chain_effects.insert(id, effect);
        self
    }

    /// Seed one `"HARDCODED …"` row, for tests. The id matters: it is half the reference's
    /// same-slot replace key (`0x6208e0`), so an engine-spawned effect dedups like a kit slot's.
    #[must_use]
    pub fn with_hardcoded(mut self, name: &str, id: u32, path: &str) -> Self {
        self.hardcoded
            .insert(name.to_string(), (id, path.to_string()));
        self
    }

    /// A `SpellVisual.dbc` row by id, `Spell.dbc` column 115.
    pub fn stages(&self, visual_id: u32) -> Option<&VisualStages> {
        self.visuals.get(&visual_id)
    }

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

    /// Every `SpellVisual` row as `(id, stages)`, in no order, for whole-table census tools.
    pub fn visuals(&self) -> impl Iterator<Item = (u32, &VisualStages)> + '_ {
        self.visuals.iter().map(|(id, s)| (*id, s))
    }

    /// Every `SpellVisualKit` id, ascending, for whole-table census tools.
    pub fn kit_ids(&self) -> impl Iterator<Item = u32> + '_ {
        let mut ids: Vec<u32> = self.kits.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
    }

    pub fn kit_len(&self) -> usize {
        self.kits.len()
    }

    /// A `SpellVisualEffectName` id's model path; an empty path reads as `None`.
    pub fn effect_path(&self, effect_id: u32) -> Option<&str> {
        self.effect_paths
            .get(&effect_id)
            .map(String::as_str)
            .filter(|p| !p.is_empty())
    }

    /// An engine-spawned effect's `(id, model path)` by its baked name, compared without case as
    /// the client's boot resolve (`0x61f5b0`) does.
    pub fn hardcoded_effect(&self, name: &str) -> Option<(u32, &str)> {
        self.hardcoded
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, (id, path))| (*id, path.as_str()))
            .filter(|(_, p)| !p.is_empty())
    }

    /// The lootable-corpse sparkle, `"HARDCODED Loot Art"`.
    pub fn loot_art_effect(&self) -> Option<(u32, &str)> {
        self.hardcoded_effect("HARDCODED Loot Art")
    }

    /// A `SpellChainEffects` row by id; `None` is the client's null-row no-op.
    pub fn chain_effect(&self, id: u32) -> Option<&ChainEffect> {
        self.chain_effects.get(&id)
    }

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

/// `SpellVisualEffectName`: id, name, model path, and two columns the client never reads.
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

/// `SpellVisualKit`: fields 0-14 are ids, 15-18 the signed `CharProcType` keys and 19-34 the
/// float params, which the dispatcher (`0x60d7c0`) loads with `fld`.
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

/// Kit field 15, `CharProcType[0]` (`+0x3c`).
const CHAR_PROC_TYPE_FIELD: usize = 15;
/// Kit field 19, `CharParamZero[0]` (`+0x4c`); `CharParamOne..Three[0]` follow at every
/// [`KIT_CHAR_PROCS`] fields (`+0x5c`, `+0x6c`, `+0x7c`).
const CHAR_PROC_PARAM_FIELD: usize = CHAR_PROC_TYPE_FIELD + KIT_CHAR_PROCS;

/// Kit `CharProc` slot `i`. The empty slot is -1; 0 is a real key, the channel beam, which the
/// dispatcher's table (`0x60dc20`) routes to its beam case (`0x60da79`). A type-0 slot that is
/// padding decodes `CharParamZero` to 0, which the client's null-row test (`0x6ecc2e`) no-ops.
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

fn some_unless_none(v: u32) -> Option<u32> {
    (v != 0 && v != NONE_SENTINEL).then_some(v)
}

/// Read the four spell visual tables off the patch chain.
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
            // Only the rows the client's boot resolve can hit, those named "HARDCODED …".
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
