//! `SpellChainEffects.dbc` — the **beam/arc** half of the spell-visual family (decision 0955):
//! Chain Lightning's lightning, Drain Life's rope of soul, Mind Flay's mana beam, C'Thun's eye
//! beam, Chain Heal's arc. A whole rendering system that hangs off one `SpellVisualKit` column
//! group we were already reading and one CharProc key we were throwing away.
//!
//! ## How a spell reaches a beam
//!
//! `Spell.dbc` → `SpellVisual` → the stage's `SpellVisualKit` → one of the kit's four **CharProc**
//! slots whose type is [`char_proc_type::CHAIN_CHANNEL`](super::char_proc_type::CHAIN_CHANNEL) (0)
//! or [`CHAIN_CAST`](super::char_proc_type::CHAIN_CAST) (12) → that slot's params → **this table**.
//! There is no other path: `SpellChainEffects` has exactly one consumer in the whole binary
//! (`0x6ecbd0 CreateChainVisual`), reached through one wrapper (`0x6ecb90`) from one call site
//! (`0x60db6d`, inside the CharProc dispatcher `0x60d7c0`).
//!
//! The dispatcher's own translation table settles which keys get there. It is a two-level switch —
//! a 16-byte type→case table at **`0x60dc20`** feeding a 9-entry address table at **`0x60dbfc`** —
//! and reading it byte-for-byte gives, for CharProcType 0..15:
//!
//! ```text
//! 0→case0  1→case1  2..5→default  6→case2  7→default  8→case3  9,10→default
//! 11→case4  12→case0  13→case5  14→case6  15→case7
//! cases: [0x60da79, 0x60d840, 0x60da55, 0x60d80a, 0x60db7e, 0x60d8df, 0x60d972, 0x60d9e8, 0x60dbdd]
//! ```
//!
//! which reproduces the three keys already verified elsewhere (1 → `0x60d840` tint, 11 →
//! `0x60db7e` anim-rate, 14 → `0x60d972` alpha — `crate::spell_visual::char_proc_type`) and shows
//! **0 and 12 both routing to case 0, `0x60da79`** — the beam case, whose tail is the
//! `CreateChainVisual` call. That is why [`super::char_proc_slot`] may not fold type `0` to "empty":
//! type 0 is the channel beam, and folding it away silently discarded 34 of the 48 live beams in
//! the shipped table.
//!
//! ## The params (`0x60db19`–`0x60db6d`)
//!
//! Three of the slot's four param columns are read, each through the client's **small-int decode**
//! `bits(param + 512.0f) >> 14 & 0xff` — the same idiom already pinned at `0x5d55c0` for the
//! dynobject shard index ([`super::char_proc_small_int`]):
//!
//! | column | decoded to |
//! |---|---|
//! | `CharParamZero` | the **`SpellChainEffects` id** — this table's key |
//! | `CharParamOne`  | the **beam count**, clamped to `≤ 3` by `0x6ecbd0` ([`CHAIN_MAX_BEAMS`]) |
//! | `CharParamTwo`  | a **boolean flag** (`setne`), stored at the beam node's `+0x48` |
//!
//! **The decode is corroborated by prediction, not just by reading:** run it over all 68 type-0/12
//! slots in the shipped `SpellVisualKit.dbc` and every one of the 48 non-zero results lands on a
//! real row of this 18-row table — no misses, no out-of-range — and each names the *right* texture:
//! Drain Mana and Mind Flay → `ManaBeam`, Drain Soul → `Beam_Purple`, Drain Life and Health Funnel
//! → `SoulBeam`, Chain Heal → `HealBeam`, Chain Lightning → `Lightning`, Shrink Ray →
//! `ManaBurnBeam`, C'Thun's Eye Beam → `SoulBeam`. The other 20 decode to id `0`, which has no row
//! — the client's own bounds check (`jl` / `cmp maxId` / null-row test at `0x6ecc0e`–`0x6ecc30`)
//! no-ops them, so they are padding and [`ChainProc`] reports them as absent.
//!
//! The shipped split is a *data* convention, not a code fork: type 0 appears on **channel**-stage
//! kits with the flag decoding to 1, type 12 on **cast**-stage kits with the flag decoding to 0.
//! The dispatcher itself cannot tell them apart, so a consumer keys off [`ChainProc::flag`], never
//! off the type constant's name.
//!
//! ## Layout — VERIFIED against build 5875
//!
//! **18 records × 8 fields × 32 B** (header dumped from the extracted file; the reference shows
//! the same shape from the loader's own field-count assert, plus the client-DB globals
//! `records 0xc0d848 / idIndex 0xc0d850 / maxId 0xc0d854`, loader `0x54e980`). Ids are
//! `1..=13, 15, 17..=20` — note `14` and `16` are absent, so an id is a lookup, never an index.
//!
//! **The columns** (decision 0955). Two of the community names this module first carried were
//! **wrong**, and both mattered:
//!
//! | field | name | meaning |
//! |---|---|---|
//! | 1 | `AvgSegLen` | segment length, with a floor: `n = trunc(len / field1 + 2.0)` segments, `n+1` points (`0x7af713`, the `2.0f` at `0x801628`) — so even a 1-yard beam gets 2 segments |
//! | 2 | `Width` | **HALF**-width — the visible ribbon is `2 × field2` yards |
//! | 3 | `NoiseScale` | jitter amplitude as a **fraction of beam length** (`len × field3`), **re-rolled every frame** (`0x7b0950` dirties the geometry per frame), with a `0.75/0.25` advection blend against the previous roll |
//! | 4 | ~~`TexCoordScale`~~ **`ScrollPeriod`** | **not a scale at all** — the scroll **period in seconds**: `phase = fmod(phase + dt, field4)` (`0x7af9d7`), `u = -(phase / field4)` (`0x7b055f`). A **negative** value reverses the scroll direction, magnitude unchanged — which is why the four *drain* rows ship `-0.5`: the texture flows back toward the caster |
//! | 5 | ~~`SegDuration`~~ **`BoltLife`** (ms) | how long ONE HOP burns — `Bolt[i].end = Bolt[i].start + field5` (`0x6ec9eb`) — and the whole object's expiry, `now + boltCount × field5` (`0x6ecd30`). Per **hop**, not per subdivision segment. The client also stores a seconds-scaled copy (`× 0.001f`, the constant at `0x801360`) which it then **never reads** |
//! | 6 | ~~`SegDelay`~~ **`BoltStagger`** (ms) | the delay between HOPS lighting up — `Bolt[i].start = t0 + i × field6` (`0x6ec9da`), its one consumer. It never reaches the *renderer*, which is what makes it look dead from the geometry side; it is what makes a 3-hop Chain Lightning arc **outward** rather than appearing all at once |
//! | 7 | `Texture` | the beam's texture |
//!
//! The beam is **never tinted**: its colour word is always `0xFFFFFFFF`. Render state, set in
//! `CLightning::Render 0x7afcb0`: **additive `SRC_ALPHA/ONE`, two-sided, depth-write
//! OFF, fog off, emissive white, alpha-test `GEQUAL 1/255`**.
//!
//! **Geometry:** a chain is one polyline of `count+1` **nodes** and `count` **hops** running
//! **caster → t1 → t2 → t3**, never a fan and never one beam per target (the client's own `Bolt`
//! records name `idxA = i`, `idxB = i+1`). Each hop is then subdivided on its own by field 1. The
//! endpoints are **re-resolved from the live units every frame** (`0x6ec460`), so a beam tracks a
//! moving caster and a moving target. The caster's end anchors at its `$CSL` attachment or,
//! failing that, `0.75 × modelHeight × modelScale` (`0x6ec6f0`); every other endpoint anchors at
//! an M2 attachment resolved from the spell's own `SpellVisual` field 9 — the **same
//! dest-attachment ordinal the missile uses** ([`MISSILE_ATTACH_TABLE`]) — with attachment 34 as
//! the fallback.
//!
//! **Lifetime** is the flag's, and only the flag's: a beam is dead when `flag == 0 && now >=
//! expiry`. So a **cast** beam (flag 0) self-terminates after `hopCount × BoltLife`, and a
//! **channel** beam (flag 1) never expires by time at all — it ends only when the owning kit node
//! is swept (`LightningObject::Stop 0x6ece10`, one caller image-wide).
//!
//! **The strand count is not a variation knob.** All three strands are constructed from
//! byte-identical arguments — no seed, no index, no stagger. They differ only in heap identity,
//! and so in the draws each takes from the shared PRNG. (Construction VERIFIED; that this is what
//! produces the braided look is INFERRED.)
//!
//! Id **15** is degenerate — width, noise, scroll period and both timings are all `0`, with only
//! `AvgSegLen = 5.0` and a texture. No shipped kit reaches it. Read the table with
//! `benilla-extract <Data> chaincensus`, which prints it alongside every kit that draws a beam;
//! it is also what caught a hand-decode of `SegDelay` being wrong.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const SPELL_CHAIN_EFFECTS: &str = "DBFilesClient\\SpellChainEffects.dbc";
const SPELL_CHAIN_EFFECTS_FIELDS: usize = 8;

/// The client's own clamp on a chain proc's beam count (`0x6ecbd0`: `cmp esi,0x3` → `jbe` →
/// `mov [ebp+0x14],3`). Only one shipped kit asks for more than one beam — 6397 "Chain Burn",
/// which asks for exactly 3.
pub const CHAIN_MAX_BEAMS: u32 = 3;

/// One `SpellChainEffects.dbc` row — the shape and animation of one beam. Every column's meaning
/// is VERIFIED (module docs); two of the community names were wrong and are not used here.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainEffect {
    /// Field 1 — the target length of one segment. The subdivision has a floor of two:
    /// `segments = trunc(length / avg_seg_len + 2.0)`, points = `segments + 1`.
    pub avg_seg_len: f32,
    /// Field 2 — the beam's **half**-width; the drawn ribbon spans `2 × this` yards.
    pub half_width: f32,
    /// Field 3 — per-joint jitter amplitude as a **fraction of the beam's length**, re-rolled
    /// **every frame** and blended `0.75` previous / `0.25` new. `0.001` on the beams that read as
    /// ropes, `0.04`/`0.05` on the lightnings.
    pub noise_scale: f32,
    /// Field 4 — the texture **scroll period in seconds** (`u = -(phase / this)`, `phase`
    /// advancing by `dt` modulo this). **Negative reverses the direction**, magnitude unchanged —
    /// the four drain rows ship `-0.5`, flowing the texture back toward the caster.
    pub scroll_period_s: f32,
    /// Field 5 — milliseconds. How long **one hop** burns (`Bolt.end = Bolt.start + this`), and
    /// through it the whole beam's expiry, `now + hops × this`. Binds **only a one-shot (cast)
    /// beam** — a channel beam ignores it and lives until swept.
    pub bolt_life_ms: u32,
    /// Field 6 — milliseconds. The **stagger between hops**: hop `i` lights at `t0 + i × this`
    /// (`0x6ec9da`, its one consumer), so a 3-hop cast arcs outward instead of appearing whole.
    /// `200` on most rows, `300` on id 1, `0` on the three rows no kit reaches. Irrelevant to a
    /// single-hop beam, and to a channel beam (whose per-hop window is bypassed entirely).
    pub bolt_stagger_ms: u32,
    /// Field 7 — the beam's texture, as the DBC stores it (`Textures\SpellChainEffects\*.blp`).
    pub texture: String,
}

/// A kit's chain CharProc, decoded — what [`super::VisualKit::chain_proc`] yields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainProc {
    /// `CharParamZero` decoded — the [`ChainEffect`] id. Never `0`: an id that names no row is
    /// the client's own no-op, and this type is only built for a slot that decodes to a real id.
    pub effect_id: u32,
    /// `CharParamOne` decoded, clamped to [`CHAIN_MAX_BEAMS`] as `0x6ecbd0` clamps it. `0` is
    /// possible on the shipped table and means the client draws nothing (`jbe` straight out).
    pub beams: u32,
    /// `CharParamTwo` decoded to a bool, exactly as the client's `setne` does. `true` on every
    /// channel-stage kit, `false` on every cast-stage one — the only thing that actually
    /// distinguishes the two chain proc types (module docs).
    pub flag: bool,
    /// The proc's type key as the table stores it — `0` or `12`. Kept for instruments and for
    /// the record; behaviour must key off [`Self::flag`], since the dispatcher does not fork.
    pub ty: i32,
}

/// `SpellChainEffects.dbc` — 8 fields / 32-byte records in build 5875.
fn chain_effects_schema() -> Schema {
    let mut s = Schema::new("SpellChainEffects");
    let mut add = |name: &str, ty| s.add_field(SchemaField::new(name.to_string(), ty));
    add("ID", FieldType::UInt32);
    add("AvgSegLen", FieldType::Float32);
    add("HalfWidth", FieldType::Float32);
    add("NoiseScale", FieldType::Float32);
    add("ScrollPeriod", FieldType::Float32);
    add("BoltLife", FieldType::UInt32);
    add("BoltStagger", FieldType::UInt32);
    add("Texture", FieldType::String);
    debug_assert_eq!(s.fields.len(), SPELL_CHAIN_EFFECTS_FIELDS);
    s
}

/// Read `SpellChainEffects.dbc` off the patch chain. A row with an empty texture string is kept —
/// the constructor never reads the texture, so a textureless row is the renderer's problem, not
/// the loader's, and dropping it here would hide a data fact.
pub(super) fn load(chain: &mut Chain) -> Result<HashMap<u32, ChainEffect>> {
    let bytes = chain
        .read_file(SPELL_CHAIN_EFFECTS)
        .with_context(|| format!("reading {SPELL_CHAIN_EFFECTS}"))?;
    let set = parse(&bytes, chain_effects_schema(), "SpellChainEffects.dbc")?;
    let mut rows = HashMap::with_capacity(set.records().len());
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            ChainEffect {
                avg_seg_len: f32_at(r, 1).unwrap_or(0.0),
                half_width: f32_at(r, 2).unwrap_or(0.0),
                noise_scale: f32_at(r, 3).unwrap_or(0.0),
                scroll_period_s: f32_at(r, 4).unwrap_or(0.0),
                bolt_life_ms: u32_at(r, 5).unwrap_or(0),
                bolt_stagger_ms: u32_at(r, 6).unwrap_or(0),
                texture: str_at(&set, r, 7).unwrap_or_default(),
            },
        );
    }
    Ok(rows)
}
