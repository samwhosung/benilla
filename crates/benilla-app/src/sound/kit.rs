//! The **kit player** — WoW's owned sound selection over the mixer seam (decision 0070).
//!
//! A play request names a `SoundEntries` kit (by id or name, mirroring the client's
//! `PlaySoundById`/`PlaySoundByName` surface `0x458850`/`0x458030`); this module does what the
//! client does between that call and the backend: the audibility gate, duplicate suppression,
//! the weighted variation pick with a depleting pool (`0x45bb70`/`0x45bd40`), per-shot
//! volume/pitch variation (`0x458c60`/`0x458da0`), and the per-frame channel pump (reap finished,
//! distance-cull, recompute `category · v · rolloff · near_field` and feed the channel volume —
//! the `0x7a4ad0`/`0x7a5000`/`0x7a5dc0` loop).
//!
//! Pinned 2026-07-03 (decision 0079): variation gates are separate DBC bits (0x400 pitch / 0x800
//! volume — raw-copied flag word, `0x45c080`), the draw is the mulhi scale [`math::variation_draw`]
//! (`0x455c70`), and **no Type→category table exists** — the client's volume category is set by
//! which play driver was invoked (`0x45ce60`/`0x45cf00`), so [`play_kit`] takes
//! the category from its caller (SFX for world/UI triggers, ambience/music for the scheduler
//! drivers). Remaining INTERIM: out-of-range looping channels stop (audible again = restart by
//! the trigger) rather than pause/resume-virtualize.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use bevy::prelude::*;
use kira::sound::PlaybackState;

use benilla_formats::{sound_kit_flags, SoundKitCatalog};

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::dev_state::DebugState;

use super::math;
use super::mixer::{self, StaticSoundData};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Which config slider scales a channel — the WoW volume categories (master is global, on the
/// main track). A property of the **call site** (which play driver fired — channel flag bits
/// 0x2 SFX / 0x8 ambience / fallback music, set by caller booleans, `0x45ce60`/`0x45cf00`), never
/// derived from the kit's `SoundType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SoundCategory {
    Sfx,
    Music,
    Ambience,
}

/// The depleting variation pool of one kit (`0x45bb70` pick + `0x45bd40` rebalance): each pick
/// decrements the chosen slot's remaining weight; when the pool empties it refills. With the
/// data's typical all-1 weights this is exactly "no repeats until every variation played".
struct PickState {
    remaining: Vec<u32>,
}

/// xorshift32 — a plain deterministic PRNG. The client draws from the shared engine CRandom
/// (cmath-owned); the *transform* of the draw is the fidelity surface, the generator is not.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// The kit catalog + play-side caches. Present only when the client data loaded.
#[derive(Resource)]
pub(crate) struct SoundKits {
    catalog: SoundKitCatalog,
    /// Decoded SFX by (lowercased) path — kit variations are short files, decoded once and
    /// cheaply cloned per play (the frames are shared). The client's SoundFileDataCache analogue.
    cache: HashMap<String, StaticSoundData>,
    pick: HashMap<u32, PickState>,
    rng: Rng,
}

impl SoundKits {
    /// One 32-bit draw off the kit player's own generator — the input to [`bark_chance_pass`].
    /// Shared with the per-shot volume/pitch variation draws, like the client's single stream.
    pub(super) fn roll(&mut self) -> u32 {
        self.rng.next()
    }

    /// The kit table itself, for the one consumer that must resolve ids **ahead of** playing them:
    /// [`super::vocal`]'s table build, which needs a kit's variation count (the reference reads
    /// `0x45cda0(kit) + 0x94` there for exactly the same reason).
    pub(super) fn catalog(&self) -> &SoundKitCatalog {
        &self.catalog
    }
}

/// One playing channel the pump owns — the client's ~0x90-byte channel struct, benilla-shaped.
pub(crate) struct ActiveChannel {
    pub(crate) kit: u32,
    /// The world entity this channel voices, when the trigger is entity-latched (the NPC
    /// greeting's per-unit live-handle latch `[unit+0xb1c]`, `0x60c28c`/`0x60c40a`). `None` for
    /// every other play. A tagged channel's liveness IS the
    /// latch: the pump reaps it when the sound stops, which is exactly the handle release.
    source: Option<Entity>,
    /// A source-tagged **looping** channel rides its unit (the client's tracked play `0x61fec0`):
    /// the pump refreshes `pos` from the source's transform each frame. One-shots stay where
    /// they fired.
    tracked: bool,
    handle: mixer::StaticSoundHandle,
    /// The spatial track keeping the 3D voice alive; `None` = 2D (main track).
    track: Option<mixer::SpatialTrackHandle>,
    pos: Option<Vec3>,
    /// Kit `MinDistance` — the rolloff knee.
    min_dist: f32,
    /// Kit `DistanceCutoff` — the cull/virtualize radius (0 = never cull).
    cutoff: f32,
    /// The per-shot volume `v` (base + variation) the mix multiplies each frame.
    v: f32,
    /// A driver-animated gain the mix multiplies each frame (default 1.0) — the fade lane for
    /// long-lived loops whose per-frame volume the pump owns (the liquid ambient loops' 5.0 s
    /// in/out ramps, decision 0506; a handle-level fade would be overwritten by the pump's
    /// `set_volume`). Written via [`set_source_kit_gain`].
    gain: f32,
    category: SoundCategory,
    /// The **voice bus** this channel occupies, for the concurrency cap ([`Bus`]).
    bus: Bus,
    /// Does this channel loop? A loop is a *bed* — an ambience, a tracked body loop — and the
    /// voice cap never steals one: cutting a bed leaves an audible hole that stays open, where
    /// stealing a one-shot costs at most the tail of a sound already being buried. (The reference
    /// makes no such distinction — it never steals *anything*; see [`claim_voice`] and decision
    /// 1563. The bed/one-shot split is ours, and only matters because we do steal.)
    looping: bool,
    /// This channel's **current effective amplitude** — `category · v · gain · rolloff ·
    /// near_field`, refreshed by [`pump_channels`] every frame. Cached rather than recomputed
    /// because the voice cap needs to rank every live channel by audibility on the play path,
    /// which is the one place that must not walk the world.
    amp: f32,
    /// Which per-unit latch this channel's liveness represents ([`Latch`]).
    latch: Latch,
}

/// **Which per-unit latch a channel holds** — the marker decision 1399 asked for, and the fix for
/// the conflation [`source_playing`] used to document.
///
/// The reference keeps two separate handles on a unit: `[unit+0xb1c]` for its greeting line and
/// `[unit+0xb20]` for its one-shot bark. benilla tags a channel with its `source` entity for
/// *three* unrelated reasons, though — a latch, a thing the pump must follow in flight, and a
/// thing a despawn must stop — and asking "is a channel tagged with this unit alive?" answered all
/// three at once. So a creature's body loop, a missile's travel loop or a water splash silently
/// held the greeting latch and muted a hello the reference would have played. It bit real data: 6
/// of the 4 509 displays that carry an `NPCSounds` greeting also resolve a `CreatureSoundData`
/// row with a nonzero `loop_sound`.
///
/// Splitting the *reason* out of the *tag* is what fixes it. `source` now means only "this channel
/// belongs to that entity"; this says what, if anything, it latches.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Latch {
    /// Tagged for ownership alone — the pump follows it, a despawn stops it, and it blocks
    /// nothing. Body loops, missile travel loops, liquid loops, spell holds.
    #[default]
    None,
    /// The NPC greeting line's `[unit+0xb1c]` (`0x60c28c`/`0x60c40a`): a unit with a greeting
    /// line still sounding refuses a new one.
    Greeting,
    /// The one-shot creature bark's `[unit+0xb20]` — the per-unit slot of the reference's bark
    /// dispatch `0x623a40` (5-way jump table `0x623afc`).
    ///
    /// **The payload is the reference's `[unit+0xb24]` — the latched bark STATE**, which is the
    /// interrupt rule's whole input: `0x623a82`/`0x623a88` abort a new bark whose state is `<=`
    /// the one already sounding, and let a strictly higher one through (stopping the old at
    /// `0x623a95`). It was left off until decision 2039 because the table's ordering was an open
    /// question and inventing an interrupt is worse than missing one; the five arms are now read
    /// out of the image dword for dword:
    ///
    /// | state | `CreatureSoundData` column | what |
    /// |---|---|---|
    /// | 0 | 10 | the HOSTILE aggro bark (`SMSG_AI_REACTION`) |
    /// | 1 | 28 | the pet's ORDER bark (`SMSG_PET_ACTION_SOUND` selector 0) |
    /// | 2 | 27 | the pet's ATTACK bark (`SMSG_PET_ACTION_SOUND` selector 1) |
    /// | 3 | — | plays nothing, and *still* stops and latches |
    /// | 4 | 6 | the death bark — the maximum, so nothing supersedes it |
    ///
    /// Carrying it on the channel is equivalent to carrying it on the unit, because the reference
    /// only ever consults `[0xb24]` while `[0xb20]` holds a live handle (`0x623a74`/`0x623a7d`
    /// jump past the comparison otherwise).
    Voice(u8),
    /// A **server-pushed object sound** live on this unit — `SMSG_PLAY_OBJECT_SOUND`
    /// (opcode `0x278`), the `AISOUNDDESC` pool at `[0xb05f38]`.
    ///
    /// The reference registers one of these per source GUID and then *queries* it from two
    /// places: `0x4591f0`, reached from `0x6234cb` (the 13-way class route) and `0x623a59` (the
    /// priority bark route). While an object sound is live on a unit, that unit's own vocals are
    /// **suppressed** — a scripted voice line is not talked over by the creature's grunts. The
    /// pool releases on the sound finishing or the emitter leaving `DistanceCutoff`
    /// (`0x457a50`), which is what this channel's liveness represents.
    ///
    /// Two limits, both byte-derived: the suppression applies to classes **0–3 and 8 only**
    /// (`0x6234bb`/`0x6234bf`/`0x6234c4`), and the **CGPlayer twin `0x62f880` omits the gate
    /// entirely**, so a player's vocals are never suppressed by one.
    ObjectSound,
}

/// The **class-bark chance roll** (`0x623520`): the
/// reference draws `r = MulHi32(101, rand32) ∈ [0, 100]` and admits the bark iff
/// `threshold >= r` — inclusive, so `P = (threshold + 1) / 101`.
///
/// The client's generator is its own shared lagged generator (`0x882664`/`0x882668` over the
/// `.rdata` table `0x802700`), **not** the MSVCRT LCG the animation variation walk uses, and it is
/// reseeded from the millisecond tick at `0x402802`. The rule follows from that:
/// **reproduce the probability, never the sequence.** So this takes any 32-bit draw — ours is the
/// kit player's own xorshift ([`SoundKits::roll`]) — and only the arithmetic is faithful.
pub(super) fn bark_chance_pass(threshold: u32, roll: u32) -> bool {
    ((101u64 * u64::from(roll)) >> 32) as u32 <= threshold
}

/// The class-5 (`$FDX` stand) chance threshold: `0x8626d4[5] = 40` in the creature table and
/// `0x86424c[5] = 40` in the player twin — identical, so which twin dispatches cannot change the
/// answer. **P = 41/101 ≈ 40.6 %.**
///
/// The rest of the creature table is `{70, 100, 60, 100, 100, 40, 100, …}` (player twin
/// `{35, 100, 30, 100, 100, 40, 100, …}`), and only classes 0, 2 and 5 are ever rolled — every
/// other class carries 100, i.e. `P = 1`, which is why `$WNG`/`$WGG` (classes 7 and 10) and the
/// ALERT bark (class 8) are faithfully unconditional. Classes 0 and 2 are encoded beside this one
/// — see [`EXERTION_CHANCE_CREATURE`] and [`INJURY_CHANCE_CREATURE`].
pub(super) const STAND_CHANCE: u32 = 40;

/// The class-0 (**exertion**) chance thresholds, and the one place in the vocal tables where the
/// creature and player twins actually disagree — a player grunts about **half as often** as a
/// creature on the same swing.
///
/// `0x8626d4[0] = 70` (creature) and `0x86424c[0] = 35` (player), read as dwords straight out of
/// `WoW.exe`. With [`bark_chance_pass`]'s inclusive compare that is **P = 71/101 ≈ 70.3 %** for a
/// creature and **36/101 ≈ 35.6 %** for a player.
///
/// Class **1** (ExertionCritical) carries 100 in both twins, so a *critical* swing always grunts.
/// That asymmetry is the audible shape of the pair: ordinary swings thin out, crits never do.
pub(super) const EXERTION_CHANCE_CREATURE: u32 = 70;
/// The player twin of [`EXERTION_CHANCE_CREATURE`] — `0x86424c[0] = 35`.
pub(super) const EXERTION_CHANCE_PLAYER: u32 = 35;

/// The class-2 (**ordinary injury**) chance thresholds — the victim's wound grunt, and the third
/// and last rolled class (decision 2073). `0x8626d4[2] = 60` (creature) and `0x86424c[2] = 30`
/// (player): **P = 61/101 ≈ 60.4 %** and **31/101 ≈ 30.7 %**.
///
/// The twin is picked by the **victim's** own type, since the roll runs inside the victim's
/// `[vtable+0x88]` — `0x623490` for a creature, `0x62f880` for a player — so a player being hit
/// grunts about half as often as a creature taking the same blow, the same asymmetry class 0 has
/// on the attacker's side.
///
/// Classes **3** (InjuryCritical) and **9** (InjuryCrushingBlow) carry 100 in both twins and are
/// never rolled: a crit and a crushing blow always vocalise, an ordinary hit does not. That is the
/// audible shape — the wound grunt thins out under sustained melee while the big hits punch
/// through it — and playing class 2 unconditionally, which is what benilla did before 2073, is
/// about 40 % too many grunts on a creature and 70 % too many on a player.
///
/// The route that reaches these was the "still unpinned" clause above: `0x624530`'s tail →
/// `[vtable+0x88]` → the roll `0x623520` / `0x62f940` → the 13-way column selector `0x623020`
/// (`+0x0c/+0x10/+0x14` = DBC columns 3/4/5).
pub(super) const INJURY_CHANCE_CREATURE: u32 = 60;
/// The player twin of [`INJURY_CHANCE_CREATURE`] — `0x86424c[2] = 30`.
pub(super) const INJURY_CHANCE_PLAYER: u32 = 30;

/// The class-5 cooldown: **10 000 ms on ONE global timestamp** (`0x623290`,
/// `GetTickCount − [0xc4e0e4] − 0x2710`). Not per unit and not per class — a single window shared
/// by every creature in the world, so one crocodile's croak silences every other creature's stand
/// vocal for ten seconds. Stamped on an *allowed* attempt and **before** the column-is-zero bail,
/// so a silent or distance-culled `$FDX` burns the window for everyone too.
pub(super) const STAND_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

/// A **voice bus** — the reference's per-bus concurrency domain, and the index into its
/// compile-time cap table (`0x87ce60`).
///
/// This is **not** the volume category ([`SoundCategory`]) — VERIFIED orthogonal: the bus lives at
/// `[chan+0x84]` and takes 0..12, while the category is flag bits `0x2`/`0x8`/`0x10` in
/// `[chan+0x38]` selecting one of three master cells. A footstep and a spell impact are both SFX
/// and land on different buses; the music stream and a UI click are different categories on the
/// same bus 0.
///
/// A play whose bus is already at its cap is **refused outright** — byte-proven: the only exits
/// from the pre-play gate `0x7a66a0` are `mov al,1; ret` / `xor al,al; ret`, with no stop, steal,
/// queue or priority compare anywhere on the path, and the callers do not retry (they simply fail
/// to set their latch, so the next natural trigger tries again).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Bus(pub(super) u8);

impl Bus {
    /// Bus 0 — cap `0x7FFFFFFF`, i.e. **uncapped**. Where spell impacts, UI, music and ambience
    /// all live in the reference, so it is also benilla's default and the honest answer for any
    /// play whose bus we have not pinned. The `$CSS` **miss** whoosh is here too (bus 0,
    /// uncapped) — only a *connecting* swing takes the capped [`Self::WEAPON_SWING`]. The armor
    /// foley off `$FSD` is here as well, uncapped beside the capped step it accompanies.
    pub(super) const DEFAULT: Bus = Bus(0);

    /// Bus 1 — **cap 1**. The **error speech** line your own character says (`0x458250`,
    /// `0x4582d4 mov ebx,1`), and the reference's only tenant of this bus. Its cap of one is
    /// load-bearing rather than incidental: `0x458250` attempts the annoyed line *and then falls
    /// through and attempts the ordinary one*, and it is this cap that keeps the second attempt
    /// from being heard on top of the first (`super::vocal` records the whole cycle).
    pub(super) const ERROR_SPEECH: Bus = Bus(1);

    /// Bus 5 — **cap 1**. The attacker's exertion vocal (`CreatureSoundData.Exertion` = class 0,
    /// `ExertionCritical` = class 1), routed through `0x624786` → `0x623b10`. One exertion voice
    /// in the whole world at a time: a pack of mobs swinging together grunts *once*.
    ///
    /// (`0x623b70`'s creature-classification flag routes some units to bus **11** instead. The cap
    /// there is also 1, so it buys a private pool of the same size and never changes the answer —
    /// which is why benilla can route by column without resolving that bit.)
    pub(super) const EXERTION: Bus = Bus(5);

    /// Bus 6 — **cap 2**. The *connecting* swing's whoosh (`WeaponSwingSounds2`, `0x624c81` →
    /// `0x457f60` → `0x458890` with `ecx = 6`) — the sound a landed or defended melee swing
    /// makes as the weapon travels, which the **miss** whoosh on [`Self::DEFAULT`] replaces
    /// rather than joins: `0x624ca0` picks exactly one of the two by victimState.
    pub(super) const WEAPON_SWING: Bus = Bus(6);

    /// Bus 7 — **cap 2**. The victim's wound vocal (`Injury` = class 2, `InjuryCritical` = 3,
    /// `InjuryCrushingBlow` = 9, which is dead in shipped data: 0 of 406 rows populate it).
    /// Bus **12** is its classified twin, cap 2 as well.
    pub(super) const INJURY: Bus = Bus(7);

    /// Bus 8 — **cap 1**. The **local player's own** wound vocal: the CGPlayer twin substitutes
    /// this bus for classes 2/3/9 when the unit is the active player (`0x62f8c5..0x62f8f3`), so
    /// your own grunts get a private slot of one rather than sharing the world's two. It is the
    /// injury half only — your exertion still competes on [`Self::EXERTION`].
    pub(super) const SELF_INJURY: Bus = Bus(8);

    /// Bus 9 — **cap 6**. The terrain footstep off `$FSD` (`0x62342a` → `0x458380`) — one half
    /// of a footfall. The same `$FSD` fires the *armor foley* on the uncapped [`Self::DEFAULT`]
    /// first (`[vt+0x8c]` → `0x4584e0`), ahead of every gate below this one, so a creature with
    /// no footstep class still rustles.
    pub(super) const FOOTSTEP: Bus = Bus(9);

    /// Bus 10 — **cap 4**. The whole melee-contact family, which all contends for the same four
    /// voices: the generic `WeaponImpactSounds` hit (`0x624977` → `0x457ec0`), the natural-weapon
    /// `CustomAttack[n]` column that replaces it (`0x6248ea`), and the parry/block clang
    /// (`0x623640` → `0x457dc0`). **Not** the deflect clang, which is a fixed kit on bus 0.
    pub(super) const MELEE_IMPACT: Bus = Bus(10);
}

/// The reference's cap table — `.data` `0x87ce60`, **13 dwords, no writer anywhere in the image**
/// (the `cmp` at `0x7a66b5` is its only reference), so these are compile-time constants and this
/// array is a transcription of the bytes:
///
/// ```text
/// 87ce60  ffffff7f 01000000 02000000 02000000
/// 87ce70  01000000 01000000 02000000 02000000
/// 87ce80  01000000 06000000 04000000 01000000
/// 87ce90  02000000
/// ```
const BUS_CAP: [u32; 13] = [0x7fff_ffff, 1, 2, 2, 1, 1, 2, 2, 1, 6, 4, 1, 2];

/// How many copies of **one kit** may sound at once, for the rows the reference leaves ungated by
/// flag 0x20. See the gate in [`play_kit_ext`] for the measurement that set it, and decision 1560
/// for why this is ours to add rather than a fidelity port.
const SAME_KIT_MAX: usize = 2;

/// The reference's own per-id suppressor: kit flag `0x20` set AND a same-kit instance still
/// audible (`0x458f40` lifts `SoundEntries +0x7c` bit 0x20 into the FMOD flags word, consumed by
/// the pre-play gate `0x7a66a0`). Named rather than inlined so the tests read the *gate*, not a
/// copy of it.
fn no_duplicates_blocks(dedupe_exempt: bool, flags: u32, live_same_kit: usize) -> bool {
    !dedupe_exempt && flags & sound_kit_flags::NO_DUPLICATES != 0 && live_same_kit > 0
}

/// [`SAME_KIT_MAX`], decision 1560's looser fallback for the 1 847 rows the reference left
/// ungated. Both suppressors belong to the **one-shot lane**; see [`PlayExtras::dedupe_exempt`]
/// for the caller that is exempt from them and why.
fn same_kit_cap_blocks(dedupe_exempt: bool, live_same_kit: usize) -> bool {
    !dedupe_exempt && live_same_kit >= SAME_KIT_MAX
}

/// The reference's **global voice ceiling** — the number this whole hunt came down to.
///
/// `FSOUND_Init(44100, 12, 0x82)` (`0x7a492b`; the `SoundSoftwareChannels` CVar's own default is
/// `"12"`). Hardware voices are a second bank of up to 12, but `FSOUND_SetMaxHardwareChannels` is
/// **forced to 0** whenever `FSOUND_GetDriverCaps` reports no hardware mixing — which is every
/// host this decade — so the ceiling is exactly 12 on any host.
///
/// **This ceiling is what actually bounds the reference's mix, and we never had it.** The
/// mass-buff case plays through `0x458870` with no cap and no dedupe — what bounds it is the
/// device ceiling, and nothing else. A probe capture measured benilla at **42 simultaneous
/// voices**, over 12 for 23 % of the run, which is what made the summed mix ask for +13.4 dBFS
/// and the limiter (1551) pull the whole mix down by up to 13.5 dB a quarter of the time. The
/// clipping became pumping; the director heard no improvement, correctly. Decision 1557.
///
/// The count covers **everything the device is mixing** — music and ambience included, because
/// the reference's bus 0 (cap `INT_MAX`) is where zone music, ambience and the liquid loops all
/// land, and they occupy FMOD channels like anything else. See [`SoundOutput::live_voices`].
pub(crate) const SOFTWARE_CHANNELS: usize = 12;

/// Make room for one more voice, or refuse. Returns `true` if the caller may start a sound.
///
/// Under the ceiling this is a length check and nothing else. At the ceiling it is a **steal**:
/// the quietest live one-shot loses its slot to a newcomer that is louder than it is, and a
/// newcomer quieter than everything already playing is simply dropped. "Keep the loudest twelve"
/// is the rule that makes a hard ceiling sound like a mix rather than like a lottery — the
/// alternative (refuse whatever arrives last) silences the sword swinging in your face because
/// twelve distant footsteps got there first.
///
/// **This is a deliberate divergence, and decision 1563 read the bytes that make it one.** The
/// rule below — lowest first, ties broken by lowest amplitude — *is* FMOD's own allocator
/// (`fmod.dll 0x100268e6`–`0x1002690e`). What the reference does is switch that allocator off:
/// every WoW voice is an `FSOUND_Stream`, `FSOUND_Stream_Create` stamps its sample priority
/// **256** (`0x1002be47`), and the steal scan skips anything `>= 256` (`0x100268e9`). So the
/// reference's 13th concurrent sound is not stolen for — it is **silently dropped**.
///
/// We let the allocator run instead. Dropping outright is faithful but worse to listen to, and
/// with 1560's copy cap doing the crowd control the steal is rare and lands on the quietest thing
/// in the mix. 1557 chose this when FMOD was still unread; 1563 kept it with the divergence named.
fn claim_voice(out: &mut SoundOutput, candidate_amp: f32) -> bool {
    if out.live_voices() >= SOFTWARE_CHANNELS {
        // Reap first. [`pump_channels`] drops finished channels once a frame, but plays happen in
        // several stages and a sound that ended earlier this frame is still in the list — so
        // without this the budget would count ghosts and refuse real sounds, intermittently and
        // in exactly the busy moments the cap is for. Only on the crowded path: the common case
        // stays a length check.
        out.channels
            .retain(|c| c.handle.state() != PlaybackState::Stopped);
    }
    let stealable = out
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| stealable(c.looping, c.latch))
        .map(|(i, c)| (i, c.amp));
    match pick_voice_slot(stealable, out.live_voices(), candidate_amp) {
        VoiceSlot::Free => true,
        VoiceSlot::Steal(i) => {
            // Ending a live waveform at an arbitrary sample is a step to zero — i.e. a click, the
            // exact defect this whole area keeps producing. Fade it (decision 1026's `declick`).
            out.channels[i].handle.stop(mixer::declick());
            out.channels.swap_remove(i);
            out.voices_stolen += 1;
            true
        }
        VoiceSlot::Denied => {
            out.voices_denied += 1;
            false
        }
    }
}

/// What [`claim_voice`] decided.
/// May the voice cap take this channel's slot? Two exclusions, for two different reasons.
///
/// **Beds** (`looping`) — cutting an ambience or a body loop leaves an audible hole that stays
/// open, where stealing a one-shot costs at most the tail of a sound already being buried.
///
/// **Latch holders** (`source`) — and this one is a correctness bug, not a taste call. A
/// source-tagged channel's *liveness is the latch*: the NPC greeting's `[unit+0xb1c]` and the
/// creature bark's `[unit+0xb20]` are held for exactly as long as the sound plays, and released
/// when the pump reaps it. Stealing one **releases the latch early**, so the very next packet is
/// free to re-fire — and the thing those latches exist to stop is precisely a burst of repeats
/// (creature.rs measured a bear's aggro roar firing 63 times in two minutes, up to thirty
/// overlapping copies, when it was played ungated). A voice cap that manufactures that under
/// load is worse than no voice cap: it breaks hardest in exactly the crowded moment it was added
/// for.
///
/// This exposure is **ours alone** — decision 1563 read `fmod.dll` and the reference never steals
/// any voice at all (every WoW channel is a stream at priority 256, the one value the allocator
/// refuses), so nothing there can release a latch early. It arrived with 1557's steal and is
/// closed here.
///
/// The cost is nil in practice: only five play paths tag a source at all (the greeting line, the
/// creature bark, the body loop, the missile travel loop and the liquid loop), and three of those
/// are already `looping`.
fn stealable(looping: bool, latch: Latch) -> bool {
    !looping && latch == Latch::None
}

#[derive(Debug, PartialEq, Eq)]
enum VoiceSlot {
    /// Under the ceiling — just play.
    Free,
    /// At the ceiling; this channel index loses its slot.
    Steal(usize),
    /// At the ceiling and nothing live is quieter — drop the new sound.
    Denied,
}

/// The voice-cap decision, pure over `(index, amplitude)` of the **stealable** (non-looping) live
/// channels — so the policy that decides what the player does and does not hear is testable
/// without an audio device, like [`bus_at_cap`] beside it.
///
/// The comparison is **strict**: a newcomer must be genuinely *louder* than the quietest thing
/// playing to take its slot. That matters for the exact case this cap exists for — a mass buff
/// lands five sample-identical copies at the same amplitude in one frame, and a non-strict test
/// would let each new copy evict the previous one forever, spending the whole budget churning
/// between identical sounds. Strict, the first ones through hold their slots and the rest are
/// dropped, which is both cheaper and what you want to hear.
fn pick_voice_slot(
    stealable: impl Iterator<Item = (usize, f32)>,
    live_voices: usize,
    candidate_amp: f32,
) -> VoiceSlot {
    if live_voices < SOFTWARE_CHANNELS {
        return VoiceSlot::Free;
    }
    match stealable.min_by(|a, b| a.1.total_cmp(&b.1)) {
        Some((i, amp)) if amp < candidate_amp => VoiceSlot::Steal(i),
        _ => VoiceSlot::Denied,
    }
}

/// Is `bus` already carrying its cap's worth of live channels? Pure over the live buses, so the
/// whole gate is testable without a device (the caps are the reference's, and getting one wrong
/// silences a class of sound outright).
fn bus_at_cap(live: impl Iterator<Item = Bus>, bus: Bus) -> bool {
    let cap = BUS_CAP[usize::from(bus.0)];
    // Bus 0's cap is `0x7FFFFFFF` — reachable only in principle, so skip the walk entirely. It is
    // also where every unpinned play lands, which makes this the common path.
    cap != BUS_CAP[0] && live.filter(|b| *b == bus).count() as u32 >= cap
}

/// The optional half of the client's full play surface — the extras [`play_kit_ext`] carries
/// beyond "which kit, where, which category". Grouped rather than passed as a row of positional
/// booleans: there were already eight of them behind a `too_many_arguments` waiver, and the bus
/// would have been the ninth.
#[derive(Clone, Copy, Default)]
pub(super) struct PlayExtras {
    /// An **explicit variation index** (`Some(i)` = play `files[i]`, bypassing the depleting
    /// random pool — the client's `variant != -1`; the NPC-greeting sequence cycler drives this).
    pub(super) variant: Option<usize>,
    /// A **source entity tag** — the played channel is tagged so its liveness serves as that
    /// entity's per-unit latch ([`source_playing`]).
    pub(super) source: Option<Entity>,
    /// Loop regardless of the kit's own 0x200 flag, for the drivers whose *column* is the loop
    /// authority (the creature body-loop; see [`play_kit_ext`]'s own note).
    pub(super) force_loop: bool,
    /// **Skip the per-id duplicate suppressors** — the kit's own `NO_DUPLICATES` flag and
    /// [`SAME_KIT_MAX`]. Those are the **one-shot lane's** gate (`0x458f40` lifting `SoundEntries`
    /// bit 0x20 into the FMOD flags word, consumed by `0x7a66a0`), and a caller that already
    /// guarantees one channel per kit by construction must not be held to it a second time. The
    /// ambient emitter pool ([`super::emitter_pool`]) is that caller: it dedupes **structurally**,
    /// one entry per SoundEntries id (`0x461e60`), and its opens go
    /// through `0x7a5680` → `0x7a54d0`, which never reaches that gate at all.
    ///
    /// It is not a nicety. `NightElfStreetLampLoop` is Flags **0x220**, so with the gate applied a
    /// pool entry could not replace its own 3.0 s fade-out — the lamp's hum would drop out for
    /// three seconds every time its entry was re-admitted past the cap, and 2 776 of the 4 623
    /// shipped rows carry the bit.
    pub(super) dedupe_exempt: bool,
    /// Which per-unit latch this play takes, if any ([`Latch`]). Defaults to [`Latch::None`] —
    /// tagging a `source` is about ownership, and taking a latch has to be asked for.
    pub(super) latch: Latch,
    /// The **voice bus** this play competes on ([`Bus`]). Defaults to the uncapped bus 0.
    pub(super) bus: Bus,
    /// The caller's **per-shot volume multiplier** — `0x458890`'s own last argument, which every
    /// site but one passes as `1.0`. It lands where the reference puts it: inside
    /// [`math::variation_volume`]'s `mult`, i.e. *before* the `[0,1]` clamp and ahead of distance
    /// attenuation, so it scales the kit's authored volume rather than the final amplitude.
    /// [`Volume::default`] is that `1.0`.
    pub(super) volume_mult: Volume,
}

/// A per-shot volume multiplier that defaults to unity — [`PlayExtras::volume_mult`]'s type.
/// A newtype only so `PlayExtras` can keep deriving `Default`, which a bare `f32` field would
/// silence at 0.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Volume(pub(super) f32);

impl Default for Volume {
    fn default() -> Self {
        Self(1.0)
    }
}

/// A kit to play, by id or by `PlaySoundByName` name.
pub(crate) enum KitRef<'a> {
    Id(u32),
    Name(&'a str),
}

/// Resolve → gate → pick → decode → play. `pos: None` is the 2D path (UI, self sounds);
/// `category` is the caller's volume bucket (module docs — the client's per-driver flag bits).
/// Silently succeeds without playing when the kit is out of range or duplicate-suppressed
/// (matching the client: gates are not errors) — `Ok(false)` is that outcome, see
/// [`play_kit_ext`]'s return.
pub(crate) fn play_kit(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_ref: KitRef<'_>,
    pos: Option<Vec3>,
    category: SoundCategory,
) -> Result<bool> {
    play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        kit_ref,
        pos,
        category,
        PlayExtras::default(),
    )
}

/// [`play_kit`] with the extras the client's full play surface carries (`0x458f90(kit,
/// variant, posPtr)` + the greeting's channel-handle store): an **explicit variation index**
/// (`Some(i)` = play `files[i]`, bypassing the depleting random pool — the client's `variant !=
/// -1`; the NPC-greeting sequence cycler drives this), a **source entity tag** (the played
/// channel is tagged so its liveness serves as that entity's per-unit latch — [`source_playing`]),
/// and **`force_loop`** — loop regardless of the kit's own 0x200 flag, for the drivers whose
/// *column* is the loop authority (the creature body-loop: every CreatureSoundData col-23 kit is
/// authored `*Loop*` yet half omit 0x200 — INTERIM, the client's loop-start `0x461d80` flag
/// handling is unpinned; decision record with the drone build), and a **voice category** — the
/// category latched on the unit's one-shot bark slot ([`ActiveChannel::voice`]); `Some` only for
/// the barks the reference stores in `[unit+0xb20]`.
///
/// **Returns whether a channel actually opened** — the reference's own answer: its play core
/// `0x45ce60` hands back the FMOD channel, `0` when any gate refused (an unresolvable kit, the
/// per-bus cap, the duplicate walk), and callers that care read that zero. Almost none do, and
/// `Ok(false)` reads exactly like `Ok(())` did for them; [`super::vocal`]'s escalation counter is
/// the one place the distinction is the mechanism.
pub(super) fn play_kit_ext(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    kit_ref: KitRef<'_>,
    pos: Option<Vec3>,
    category: SoundCategory,
    extras: PlayExtras,
) -> Result<bool> {
    let PlayExtras {
        variant,
        source,
        force_loop,
        dedupe_exempt,
        latch,
        bus,
        volume_mult: Volume(mult),
    } = extras;
    // The cover's audio hold ([`SoundConfig::world_hold`]): while the loading screen is up, no
    // new sound starts — checked before anything allocates, so the per-frame retry drivers (the
    // creature body-loop, the liquid loops) cost one bool while held and start clean at the
    // reveal. A dropped one-shot under an opaque cover is a sound the reference never played:
    // its world load blocks, and the events these sounds voice happen unheard.
    if config.world_hold {
        return Ok(false);
    }
    let kit = match kit_ref {
        KitRef::Id(id) => kits.catalog.get(id),
        KitRef::Name(name) => kits.catalog.by_name(name),
    }
    .ok_or_else(|| anyhow!("unknown sound kit"))?;

    let (id, volume, flags, min_dist, cutoff, eax_def) = (
        kit.id,
        kit.volume,
        kit.flags,
        kit.min_distance,
        kit.distance_cutoff,
        kit.eax_def,
    );
    // Selection-time audibility (0x45cdf0): positional kits out of cutoff never allocate —
    // checked first, before the weight pool, so a per-frame retry driver (the creature body-loop
    // reconciler) costs a distance test and nothing more while its unit is out of range.
    let d_sq = pos.map(|p| math::dist_sq(listener, p));
    if let Some(d_sq) = d_sq {
        if cutoff > 0.0 && !math::audible(d_sq, cutoff) {
            return Ok(false);
        }
    }

    let weights: Vec<u32> = kit.files.iter().map(|(_, w)| *w).collect();
    if weights.is_empty() {
        return Ok(false); // a kit with no files is playable-as-nothing, not an error
    }

    // The **per-bus concurrency cap** — the always-on arm of the pre-play gate `0x7a66a0`, checked
    // *before* the duplicate walk exactly as the binary does (`0x7a66ae/b5` precede `0x7a66c4`).
    // The count is of **allocated** channels, not audible ones, which is what `out.channels` holds:
    // the reference's `[0xcf553c + 4*bus]` keeps a paused or muted channel's slot and releases a
    // distance-culled one only because the cull hands the channel back — and benilla's pump reaps a
    // culled channel out of this list on the same edge.
    if bus_at_cap(out.channels.iter().map(|c| c.bus), bus) {
        return Ok(false);
    }

    // Duplicate suppression: the FMOD pre-play gate 0x7a66a0 drops a play when kit flag 0x20 is
    // set AND a same-kit instance is still audible (0x458f40 lifts SoundEntries +0x7c bit 0x20
    // into the FMOD flags word). The gate's OTHER arm — an always-on per-category concurrent cap
    // (count[0xcf553c] >= limit[0x87ce60], 13 categories) — stays a deferral until that limit
    // table is read out.
    let live_same_kit = out.channels.iter().filter(|c| c.kit == id).count();
    if no_duplicates_blocks(dedupe_exempt, flags, live_same_kit) {
        return Ok(false);
    }

    // **The coherent-copy cap** (decision 1560) — the fallback for the rows the reference leaves
    // ungated above.
    //
    // A probe capture of the director's own reported case measured the loudest moment of a
    // 71-second session at **3.73x full scale**, and it was `HolyProtection` x5 in one second with
    // only 10 voices live — so the 12-voice ceiling (1557) cannot touch it. Five copies of one
    // 0 dBFS file started in the same frame are *sample-aligned*, so they sum **coherently**: the
    // result is not five sounds, it is one sound +14 dB. That is inaudible as density and very
    // audible as distortion — the worst possible trade, and the reason a mass buff was the thing
    // the director reported first.
    //
    // The reference already caps same-kit concurrency at **1** — for the 2 776 of 4 623 rows that
    // carry flag 0x20. It simply never gated the other 1 847, and `HolyProtection` (3116,
    // `Flags 0x0000`) is one of them. So this is not a new idea imposed on the data; it is the
    // reference's own idea applied, more loosely, where the data left a hole. Two rather than one
    // deliberately: it keeps some sense that several things happened (and cannot silence two
    // genuinely distinct mobs' impacts), while removing the coherent stack that does the damage.
    if same_kit_cap_blocks(dedupe_exempt, live_same_kit) {
        out.copies_dropped += 1;
        return Ok(false);
    }

    // The variation: an explicit index when the caller drives the cycle (the client's
    // `variant != -1`), else the weighted pick with depletion.
    let pick = match variant {
        Some(i) if i < weights.len() => i,
        Some(_) => return Ok(false), // out-of-range explicit variant: playable-as-nothing
        None => kits.pick_variation(id, &weights),
    };
    let path = kits.catalog.get(id).expect("resolved above").files[pick]
        .0
        .clone();

    // Decode (cached).
    let data = kits.sfx(assets, &path)?;

    // Per-shot variation — separate DBC gates: 0x800 volume (`0x458c60`), 0x400 pitch (`0x458da0`)
    // (no 5875 kit sets 0x800, so volume variation is dormant in this build's data — the gate is
    // faithful).
    let v = if flags & sound_kit_flags::VARY_VOLUME != 0 {
        let draw = math::variation_draw(kits.rng.next());
        math::variation_volume(Some(draw), volume, mult)
    } else {
        math::variation_volume(None, volume, mult)
    };

    let atten = d_sq.map_or(1.0, |d| {
        math::fmod_rolloff(d, min_dist) * near_field(d, cutoff)
    });
    let amp = config.category_amp(category) * v * atten;
    let mut data = data.volume(mixer::amp_to_db(amp));
    if flags & sound_kit_flags::VARY_PITCH != 0 {
        let draw = math::variation_draw(kits.rng.next());
        let freq = math::variation_pitch_freq(draw);
        data = data.playback_rate(freq as f64 / data.sample_rate as f64);
    }
    let looping = force_loop || flags & sound_kit_flags::LOOPING != 0;
    if looping {
        data = data.loop_region(..);
    }

    // The global ceiling ([`SOFTWARE_CHANNELS`]) — last of the gates, because it is the only one
    // that can *stop another sound*, and it must not do that for a play the cheaper gates above
    // were going to drop anyway.
    if !claim_voice(out, amp) {
        return Ok(false);
    }

    // **No device is a no-op, not an error.** Running silent (`WOW_NOSOUND=1`, CI, an unattended
    // probe — or a player whose device is genuinely gone) is a startup fact `sound::plugin`
    // already warns about ONCE. Returning `Err` here turned that one fact into a warn PER EVENT
    // at every one of this module's ~25 call sites: a 75 s lava probe logged 3975 identical
    // `no audio device` lines, 89% of the file, which is why `liquid_loop` grew a guard of its
    // own — and a probe sweep still footstepped one warn every half-second. The fix belongs here,
    // at the one place that knows, rather than as a guard repeated at each caller. `false` is the
    // signal this function already uses for "the gates dropped it", which is exactly what
    // happened.
    let Some(mixer) = out.mixer.as_mut() else {
        return Ok(false);
    };
    let (track, handle) = match pos {
        Some(p) => {
            // `EAXDef 0` = no `SoundSamplePreferences` row = the reference's NULL-slot skip at
            // `0x45cdc0`/`0x7a5bf0`: the channel never gets reverb properties, so it stays dry
            // however wet the zone is. That is what keeps NPC voice lines (all 275 `SoundType 17`
            // rows are `EAXDef 0`) out of an interior's reverb — decision 1155, bug B236.
            let (t, h) = mixer.play_3d(data, p, eax_def != 0)?;
            (Some(t), h)
        }
        None => (None, mixer.play_2d(data)?),
    };
    // Every actual play, named. The one question the sound subsystem could not answer about itself
    // was "what just made that noise?" — a report of an unexpected sound had no trace to read, only
    // a guess at which trigger fired. `RUST_LOG=benilla_app::sound=debug` now answers it, and since
    // 1155 it also answers "…and does it take the interior's reverb?" — the `EAXDef` wet/dry class
    // is invisible in the audio itself, so a report of an unexpected echo needs it in the trace.
    let name = kits.catalog.get(id).map_or("?", |k| k.name.as_str());
    let cat = match category {
        SoundCategory::Sfx => "sfx",
        SoundCategory::Music => "music",
        SoundCategory::Ambience => "ambience",
    };
    let spatial = match pos {
        Some(_) if eax_def != 0 => "3d wet",
        Some(_) => "3d dry",
        None => "2d",
    };
    debug!("sound: play kit {id} ({name}) {cat} {spatial}");
    // …and onto the probe's timeline when one is recording (decision 1556), so a capture answers
    // "what was playing when the mix went past full scale" instead of only "it did".
    if let Some(probe) = out.probe.as_ref() {
        probe.note_play(id, name, cat, spatial);
    }
    out.channels.push(ActiveChannel {
        kit: id,
        source,
        tracked: looping && source.is_some(),
        handle,
        track,
        pos,
        min_dist,
        cutoff,
        v,
        gain: 1.0,
        category,
        looping,
        amp,
        bus,
        latch,
    });
    Ok(true)
}

/// Does `unit` hold a **live one-shot voice channel** — the reference's `[unit+0xb20]` handle,
/// nonzero-gated — and if so, at which
/// **state**? `Some(state)` is the pair `[0xb20]` live + `[0xb24]`; `None` is a free slot, which
/// is `0x623a74`/`0x623a7d`'s "allowed, no comparison" path.
///
/// Scoped to channels that actually latched a state, which is the whole point — the reference
/// keeps this handle separate from the combat drone (`0x623800` carries its own latch) and from
/// the greeting line (`[unit+0xb1c]`), so a humming elemental or a talking quest-giver must not
/// mute its own barks.
pub(super) fn unit_voice_state(out: &SoundOutput, unit: Entity) -> Option<u8> {
    out.channels.iter().find_map(|c| match c.latch {
        Latch::Voice(state) if c.source == Some(unit) => Some(state),
        _ => None,
    })
}

/// Stop whatever holds `unit`'s voice slot — the reference's `0x623a95 call 0x7a5700` on
/// `&[unit+0xb20]`, which runs on **every** admitted bark, before the column is even read. So a
/// bark whose column is `0` still silences the one it superseded: the handle is overwritten with
/// the (null) result of playing nothing (`0x623aee`).
pub(super) fn stop_unit_voice(out: &mut SoundOutput, unit: Entity) {
    out.channels.retain_mut(|c| {
        if occupies_voice_slot(c.source, c.latch, unit) {
            c.handle.stop(mixer::declick());
            false
        } else {
            true
        }
    });
}

/// Does one channel, described by its `(source, voice)` identity, occupy `unit`'s **voice** slot
/// (`[unit+0xb20]`) — as opposed to its greeting latch ([`occupies_greeting_latch`],
/// `[unit+0xb1c]`)? The two are disjoint by construction, which is the whole point of separating
/// them: a bark and a greeting line are different handles in the reference and must not mute each
/// other.
pub(super) fn occupies_voice_slot(source: Option<Entity>, latch: Latch, unit: Entity) -> bool {
    source == Some(unit) && matches!(latch, Latch::Voice(_))
}

/// The complement — the greeting latch's own test (see [`source_playing`] for what else currently
/// lands in it).
pub(super) fn occupies_greeting_latch(source: Option<Entity>, latch: Latch, unit: Entity) -> bool {
    source == Some(unit) && latch == Latch::Greeting
}

/// Is a channel tagged with `source` still live? The NPC-greeting per-unit latch (`[unit+0xb1c]`
/// nonzero-gate, `0x60c28c`/`0x60c40a`): a unit with a greeting line still sounding refuses a new
/// one. Release is automatic — the pump reaps the channel when the sound stops.
///
/// **Voice-latched channels are excluded**, because in the reference they are a *different
/// handle*: the greeting owns `[unit+0xb1c]`, the one-shot bark owns `[unit+0xb20]`
/// ([`unit_voice_playing`]). Conflating them would let a unit's aggro roar mute its own greeting
/// line, which the client never does.
///
/// **So is everything else tagged with `source`** — and that is what [`Latch`] fixed. This used
/// to ask "is any channel tagged with this unit alive?", which the body loop (`0x623800`'s own
/// latch), the missile travel loop, the water splash and the spell hold all answered yes to, so
/// any of them masked a greeting the reference would have played. It bit real data: 6 of the
/// 4 509 displays that carry an `NPCSounds` greeting also resolve a `CreatureSoundData` row with
/// a nonzero `loop_sound` (1303, 10006, 10045, 10699, 11912, 12769), and every one of those
/// creatures was permanently unable to say hello. Decision 1399 raised the per-latch marker as
/// the honest fix; it is in, and this now asks only about the channel that actually holds
/// `[unit+0xb1c]`.
/// Is a **server-pushed object sound** live on `unit` — the reference's `AISOUNDDESC` pool query
/// `0x4591f0`? See [`Latch::ObjectSound`] for what it gates and what it does not.
pub(super) fn object_sound_playing(out: &SoundOutput, unit: Entity) -> bool {
    out.channels
        .iter()
        .any(|c| c.source == Some(unit) && c.latch == Latch::ObjectSound)
}

pub(super) fn source_playing(out: &SoundOutput, source: Entity) -> bool {
    out.channels
        .iter()
        .any(|c| occupies_greeting_latch(c.source, c.latch, source))
}

/// Is a channel tagged with `source` playing kit `kit_id`? The kit-scoped latch — the creature
/// body-loop's "not already playing" gate (`0x623800`'s latch is its own channel handle; the
/// channel's liveness here is exactly that). Kit-scoped so a unit's greeting line or spell hold
/// never masks its body loop.
pub(super) fn source_kit_playing(out: &SoundOutput, source: Entity, kit_id: u32) -> bool {
    out.channels
        .iter()
        .any(|c| c.source == Some(source) && c.kit == kit_id)
}

/// The kit's catalog name, or `None` when the id resolves to no `SoundEntries` row — the
/// reference's `0x45cda0(id)` null test, which is what permanently fails a doodad emitter pool
/// entry (`[+0xE00] = -id`). The pool is outside this module, and the catalog is not.
pub(super) fn kit_name(kits: &SoundKits, id: u32) -> Option<&str> {
    kits.catalog.get(id).map(|k| k.name.as_str())
}

/// Whether kit `id` is a LOOPING kit (`SoundEntries` flag 0x200) — the client's `0x458830` test
/// that splits tracked-loop playback from fire-and-forget one-shots (decision 0107).
pub(super) fn kit_looping(kits: &SoundKits, id: u32) -> bool {
    kits.catalog
        .get(id)
        .is_some_and(|k| k.flags & sound_kit_flags::LOOPING != 0)
}

/// Force-stop `source`'s channels playing kit `kit_id` — the spell-hold loop's reap (decision
/// 0107: a LOOPING kit sound rides the client's *tracked* play `0x61fec0` and dies with its
/// effect at `0x614150`, unlike a fire-and-forget one-shot). Kit-scoped so a caster's other
/// tagged channels (its greeting line) survive the cast ending.
/// Set the driver-animated gain of the channel tagged `(source, kit_id)` — the fade lane for
/// pump-owned loops ([`ActiveChannel::gain`]; the liquid ambient loops' 5.0 s ramps, decision
/// 0506). The pump folds it into the next frame's volume. No-op if the channel is gone.
pub(super) fn set_source_kit_gain(out: &mut SoundOutput, source: Entity, kit_id: u32, gain: f32) {
    for c in &mut out.channels {
        if c.source == Some(source) && c.kit == kit_id {
            c.gain = gain.clamp(0.0, 1.0);
        }
    }
}

pub(super) fn stop_source_kit(out: &mut SoundOutput, source: Entity, kit_id: u32) {
    out.channels.retain_mut(|c| {
        if c.source == Some(source) && c.kit == kit_id {
            c.handle.stop(mixer::declick());
            false
        } else {
            true
        }
    });
}

/// Force-stop every channel tagged with `source` — the unit-teardown stop (`0x5fbb6c`: the client
/// stops + clears the greeting handle when the unit's record is torn down). Called on despawn.
/// Returns **how many channels it actually stopped** — the caller's only way to tell "this source
/// held a loop and we reaped it" from "this source was silent", which is the pair a streaming
/// retest has to distinguish (the doodad reaper in [`super::anim_events`]).
pub(super) fn stop_source(out: &mut SoundOutput, source: Entity) -> usize {
    let before = out.channels.len();
    out.channels.retain_mut(|c| {
        if c.source == Some(source) {
            c.handle.stop(mixer::declick());
            false
        } else {
            true
        }
    });
    before - out.channels.len()
}

/// The `PlaySoundFile` path — a raw file play, no kit: no audibility/duplicate gates, no
/// variation, base volume 1.0, 2D on the caller's category slider. Rides the same decode cache
/// as kit files (addon files are short SFX; a kit id of 0 keeps it invisible to kit dedup).
pub(crate) fn play_file(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    path: &str,
    category: SoundCategory,
) -> Result<()> {
    // The cover's audio hold — same gate as [`play_kit_ext`], same reason.
    if config.world_hold {
        return Ok(());
    }
    let data = kits.sfx(assets, path)?;
    let amp = config.category_amp(category);
    let data = data.volume(mixer::amp_to_db(amp));
    if !claim_voice(out, amp) {
        return Ok(());
    }
    // No device is a no-op here too — see the note in [`play_kit_ext`].
    let Some(mixer) = out.mixer.as_mut() else {
        return Ok(());
    };
    let handle = mixer.play_2d(data)?;
    if let Some(probe) = out.probe.as_ref() {
        probe.note_play(0, path, "sfx", "2d");
    }
    out.channels.push(ActiveChannel {
        kit: 0,
        source: None,
        tracked: false,
        handle,
        track: None,
        pos: None,
        min_dist: 0.0,
        cutoff: 0.0,
        v: 1.0,
        gain: 1.0,
        category,
        looping: false,
        amp,
        bus: Bus::DEFAULT,
        latch: Latch::None,
    });
    Ok(())
}

/// Near-field ramp, tolerant of the `cutoff == 0` non-positional sentinel.
fn near_field(d_sq: f32, cutoff: f32) -> f32 {
    if cutoff > 0.0 {
        math::near_field_atten(d_sq, cutoff)
    } else {
        1.0
    }
}

impl SoundKits {
    /// Resolve a SoundEntries kit id by its `PlaySoundByName` key — the client's name-hash
    /// registry lookup (`0x458030` family). The ghost ambience/music tracks are named entries
    /// ("Ghost"/"GhostMusic" — the reference's zone music/ambience + the 0308 death dispatch).
    pub(crate) fn id_by_name(&self, name: &str) -> Option<u32> {
        self.catalog.by_name(name).map(|k| k.id)
    }

    pub(crate) fn new(catalog: SoundKitCatalog) -> Self {
        Self {
            catalog,
            cache: HashMap::new(),
            pick: HashMap::new(),
            rng: Rng(0x9e37_79b9),
        }
    }

    /// A kit's variation count (0 = unknown or file-less kit). The NPC-greeting sequence cycler
    /// needs the hello kit's size to know when repeat interacts overflow into the pissed line.
    pub(super) fn variations(&self, kit: u32) -> usize {
        self.catalog.get(kit).map_or(0, |k| k.files.len())
    }

    /// Pick a variation for a kit and return `(file path, kit base volume)` — the entry for the
    /// **streaming** consumers (zone music/ambience open the file as a stream instead of a decoded
    /// SFX; same depleting pick, same catalog). `None` when the kit is unknown or file-less.
    pub(super) fn pick_stream(&mut self, kit_id: u32) -> Option<(String, f32)> {
        let kit = self.catalog.get(kit_id)?;
        if kit.files.is_empty() {
            return None;
        }
        let volume = kit.volume;
        let weights: Vec<u32> = kit.files.iter().map(|(_, w)| *w).collect();
        let pick = self.pick_variation(kit_id, &weights);
        let path = self.catalog.get(kit_id)?.files[pick].0.clone();
        Some((path, volume))
    }

    /// `0x45bb70`: weighted-random pick over the kit's *remaining* weight pool, refilled when
    /// exhausted (`0x45bd40`).
    fn pick_variation(&mut self, kit: u32, weights: &[u32]) -> usize {
        if weights.len() == 1 {
            return 0;
        }
        let st = self.pick.entry(kit).or_insert_with(|| PickState {
            remaining: weights.to_vec(),
        });
        let mut total: u32 = st.remaining.iter().sum();
        if total == 0 {
            st.remaining.copy_from_slice(weights);
            total = st.remaining.iter().sum();
        }
        if total == 0 {
            return 0; // all-zero weights: degenerate data, take the first
        }
        let r = self.rng.next() % total;
        let mut acc = 0u32;
        for (i, w) in st.remaining.iter().enumerate() {
            acc += w;
            if acc > r {
                st.remaining[i] -= 1;
                return i;
            }
        }
        st.remaining.len() - 1
    }

    /// Decoded sound for a kit file, via the cache (whole-file chain read + decode on miss —
    /// short SFX; music streams through the scheduler path instead).
    fn sfx(&mut self, assets: &WorldAssets, path: &str) -> Result<StaticSoundData> {
        let key = path.to_ascii_lowercase();
        if let Some(d) = self.cache.get(&key) {
            return Ok(d.clone());
        }
        // **Two stores, one rule** — the chain, then an addon's own loose file
        // ([`benilla_assets::read_chain_or_loose`], decision 1322's resolver): `PlaySoundFile` is
        // a by-path verb, and the audio an addon ships lives on disk, never in an MPQ.
        let bytes = assets
            .read_file_or_loose(path)
            .with_context(|| format!("reading {path}"))?;
        let data = mixer::sfx_from_bytes(bytes)?;
        self.cache.insert(key, data.clone());
        DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(data)
    }
}

/// First-play decodes so far — a chain read plus a decode on the main thread, once per
/// distinct kit file per session. `FPS_PROBE` reads it per frame to annotate a tail frame
/// (a raid's buff wave meets dozens of distinct spell sounds in its first minute).
pub(crate) static DECODES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Startup: load the kit catalog off the chain (absent → no resource; every play site tolerates
/// that, the same optional-catalog rule as `Creatures`).
pub(super) fn load_sound_kits(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_sound_kit_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("sound: {} kits loaded", catalog.len());
            commands.insert_resource(SoundKits::new(catalog));
        }
        Err(e) => warn!("sound: SoundEntries failed to load — kits disabled: {e:#}"),
    }
}

/// The per-frame channel pump (`0x7a4ad0` shape): reap finished channels, follow tracked sources,
/// distance-cull the positional ones, recompute each live channel's `category · v · rolloff ·
/// near_field` volume.
pub(super) fn pump_channels(
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    transforms: Query<&Transform, Without<Camera3d>>,
) {
    let listener = listener.pos;
    out.channels.retain_mut(|ch| {
        if ch.handle.state() == PlaybackState::Stopped {
            return false;
        }
        // The tracked-channel follow (`0x61fec0`): a source-tagged loop rides its unit — refresh
        // the emitter position from the live transform (a despawned source keeps the last pos for
        // the frame until the despawn reaper stops the channel).
        if ch.tracked {
            if let Some(p) = ch
                .source
                .and_then(|s| transforms.get(s).ok())
                .map(|t| t.translation)
            {
                if ch.pos != Some(p) {
                    ch.pos = Some(p);
                    if let Some(track) = ch.track.as_mut() {
                        mixer::set_track_position(track, p);
                    }
                }
            }
        }
        // Compare before the write, on both arms (1362's no-op-write law, the audio edition):
        // `set_volume` is a command into the audio thread's queue whether or not the value
        // moved, and on a still frame every live channel's amp is bit-identical to last
        // frame's — the glide an earlier write started completes on its own. The handle's
        // volume is `amp_to_db(ch.amp)` from `play` onward, so the field IS the handle's state.
        let Some(p) = ch.pos else {
            // 2D: only the category slider can move under a live channel.
            let amp = config.category_amp(ch.category) * ch.v * ch.gain;
            if amp != ch.amp {
                ch.amp = amp;
                ch.handle
                    .set_volume(mixer::amp_to_db(ch.amp), mixer::glide());
            }
            return true;
        };
        let d_sq = math::dist_sq(listener, p);
        if ch.cutoff > 0.0 && !math::audible(d_sq, ch.cutoff) {
            // Beyond cutoff: the client virtualizes; our one-shots just stop (module docs).
            ch.handle.stop(mixer::declick());
            return false;
        }
        let amp = config.category_amp(ch.category)
            * ch.v
            * ch.gain
            * math::fmod_rolloff(d_sq, ch.min_dist)
            * near_field(d_sq, ch.cutoff);
        if amp != ch.amp {
            ch.amp = amp;
            // Glides, not snaps (decision 1026): this is the per-frame gain feed, and a step here
            // is a click. It is also the one that scales — every live channel steps together when
            // a frame hitches, which is what a "crack fest" under OBS actually was.
            ch.handle
                .set_volume(mixer::amp_to_db(ch.amp), mixer::glide());
        }
        true
    });
}

/// Drain the debug panel's "play kit" request (id or name — or a raw file path: a query with a
/// path separator plays through [`play_file`], the `PlaySoundFile` path, so the by-path plumbing
/// is ear-testable without an addon).
pub(super) fn apply_kit_debug(
    mut debug: ResMut<DebugState>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    // Read before borrowing mutably: a `&mut` through `ResMut` marks `DebugState` changed, and
    // this ran every frame — so every still-frame gate that reads `debug.is_changed()`
    // (decision 1979) saw a changed debug state on every frame of every run.
    if !debug.sound.play_kit {
        return;
    }
    let s = &mut debug.sound;
    if !std::mem::take(&mut s.play_kit) {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        warn!("sound debug: kit catalog not loaded");
        return;
    };
    let query = s.kit_query.trim().to_owned();
    if query.contains(['\\', '/']) {
        match play_file(
            &mut kits,
            &assets,
            &mut out,
            &config,
            &query,
            SoundCategory::Sfx,
        ) {
            Ok(()) => info!("sound debug: file \"{query}\" played"),
            Err(e) => warn!("sound debug: file \"{query}\" — {e:#}"),
        }
        return;
    }
    let kit_ref = match query.parse::<u32>() {
        Ok(id) => KitRef::Id(id),
        Err(_) => KitRef::Name(&query),
    };
    let listener = listener.pos;
    // `copies` fires the kit N times in ONE frame — the overlap probe (decision 1551). Five copies
    // of kit 3116 is mass Fortitude on a full party: same kit, same instant, sample-aligned. The
    // per-kit gates still apply, so a kit carrying the 0x20 no-duplicate bit collapses to one
    // however many copies are asked for — which is the honest answer for that kit.
    let copies = s.play_copies.max(1);
    for _ in 0..copies {
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            match kit_ref {
                KitRef::Id(id) => KitRef::Id(id),
                KitRef::Name(n) => KitRef::Name(n),
            },
            None,
            SoundCategory::Sfx,
        ) {
            warn!("sound debug: kit \"{query}\" — {e:#}");
            return;
        }
    }
    info!("sound debug: kit \"{query}\" played x{copies}");
}

/// `OnExit(InWorld)`: every live kit channel dies with the world — one-shots mid-flight and
/// entity-latched loops alike (their emitters are being torn down in this same transition). The
/// glue screens' own clicks only start after this edge, so the blanket stop is exact.
fn stop_all_channels(mut out: NonSendMut<SoundOutput>) {
    let n = out.channels.len();
    for ch in &mut out.channels {
        ch.handle.stop(mixer::declick());
    }
    out.channels.clear();
    if n > 0 {
        info!("sound: {n} kit channel(s) stopped (left world)");
    }
}

/// Drop the decoded-SFX cache on a cross-map transition (`world_map::MapChange` — see its doc):
/// kit variations are decoded on demand, so a new map's soundscape rebuilds its own working set
/// while the old map's decodes stop occupying RAM forever (the #bugs teleport leak). Playing
/// channels own their frames (`StaticSoundData` clones share them), so nothing audible cuts.
fn evict_kit_cache(
    mut changes: MessageReader<benilla_world::world_map::MapChange>,
    kits: Option<ResMut<SoundKits>>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    if let Some(mut k) = kits {
        k.cache.clear();
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_sound_kits.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                pump_channels.in_set(benilla_world::schedule::WorldStage::Present),
                apply_kit_debug,
                evict_kit_cache,
            ),
        )
        .add_systems(
            OnExit(crate::char_select::ClientState::InWorld),
            stop_all_channels,
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap table is a transcription of `.data` `0x87ce60`, so it is pinned as bytes, not as
    /// intent: 13 dwords, bus 0 effectively unlimited, and the four buses benilla actually routes
    /// onto carrying the numbers the binary carries.
    #[test]
    fn the_cap_table_is_the_bytes_at_0x87ce60() {
        assert_eq!(BUS_CAP.len(), 13);
        assert_eq!(BUS_CAP, [0x7fff_ffff, 1, 2, 2, 1, 1, 2, 2, 1, 6, 4, 1, 2]);
    }

    /// The gate refuses at the cap and not before it, per bus, independently — and bus 0 never
    /// refuses however loaded it gets, which is what keeps spell impacts, UI, music and ambience
    /// behaving exactly as they did.
    #[test]
    fn a_bus_refuses_at_its_cap_and_only_its_own() {
        let live = |bus: u8, n: usize| std::iter::repeat_n(Bus(bus), n);

        // Bus 0 is uncapped: a thousand live channels still admit the next one.
        assert!(!bus_at_cap(live(0, 1000), Bus::DEFAULT));

        // A cap-1 bus (5 = exertion) admits the first and refuses the second.
        assert!(!bus_at_cap(std::iter::empty(), Bus(5)));
        assert!(bus_at_cap(live(5, 1), Bus(5)));

        // A cap-2 bus (7 = injury) admits two.
        assert!(!bus_at_cap(live(7, 1), Bus(7)));
        assert!(bus_at_cap(live(7, 2), Bus(7)));

        // The capped melee/footstep pair, at their own numbers.
        assert!(!bus_at_cap(live(10, 3), Bus(10)));
        assert!(bus_at_cap(live(10, 4), Bus(10)));
        assert!(!bus_at_cap(live(9, 5), Bus(9)));
        assert!(bus_at_cap(live(9, 6), Bus(9)));

        // Buses are independent domains: a saturated bus 5 does not gate bus 7.
        assert!(!bus_at_cap(live(5, 9), Bus(7)));
    }

    /// The depleting pool (`0x45bb70`/`0x45bd40`): with the data's typical all-1 weights, every
    /// variation plays exactly once per cycle (no repeats until the pool empties), then the pool
    /// refills and the next cycle again covers all of them.
    #[test]
    fn depleting_pick_covers_all_variations_each_cycle() {
        let mut kits = SoundKits::new(benilla_formats::SoundKitCatalog::empty_for_tests());
        let weights = [1u32, 1, 1, 1, 1];
        for cycle in 0..3 {
            let mut seen = [false; 5];
            for _ in 0..5 {
                let i = kits.pick_variation(7, &weights);
                assert!(!seen[i], "repeat within cycle {cycle}: index {i}");
                seen[i] = true;
            }
            assert!(seen.iter().all(|&s| s), "cycle {cycle} covered all 5");
        }
    }

    /// The class-bark chance arithmetic (`r = MulHi32(101, rand32)`, admit iff `threshold >= r`).
    /// Two properties matter and both are byte-derived: the compare is **inclusive**, so
    /// `threshold = 100` is a tautology — which is the whole reason `$WNG`, `$WGG` and the ALERT
    /// bark are faithfully ungated — and `threshold = 40` admits 41 of the 101 buckets.
    #[test]
    fn the_bark_chance_is_inclusive_and_100_is_a_tautology() {
        // 100 never refuses, at either end of the draw space.
        for roll in [0, 1, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
            assert!(bark_chance_pass(100, roll), "threshold 100 refused {roll}");
        }
        // 40 admits exactly buckets 0..=40 of 101 — measured over the bucket boundaries, not
        // sampled, so this pins the arithmetic rather than a distribution.
        let bucket = |r: u32| ((101u64 * u64::from(r)) >> 32) as u32;
        assert_eq!(bucket(0), 0);
        assert_eq!(bucket(u32::MAX), 100);
        let admitted = (0..=100)
            .filter(|&b| {
                // the first draw landing in bucket b
                let r = ((u64::from(b) << 32) / 101) as u32 + 1;
                bucket(r) == b && bark_chance_pass(STAND_CHANCE, r)
            })
            .count();
        assert_eq!(admitted, 41, "P = 41/101 for the class-5 stand vocal");
    }

    /// The exertion pair's shape: **a crit always grunts, an ordinary swing does not, and a
    /// player grunts about half as often as a creature.** These are the only vocal thresholds
    /// where the creature and player twins disagree (`0x8626d4[0] = 70` vs `0x86424c[0] = 35`,
    /// read as dwords out of `WoW.exe`), and class 1 carries 100 in both.
    #[test]
    fn a_crit_always_grunts_and_a_player_grunts_half_as_often() {
        let bucket = |r: u32| ((101u64 * u64::from(r)) >> 32) as u32;
        let admitted = |threshold: u32| {
            (0..=100)
                .filter(|&b| {
                    let r = ((u64::from(b) << 32) / 101) as u32 + 1;
                    bucket(r) == b && bark_chance_pass(threshold, r)
                })
                .count()
        };
        assert_eq!(
            admitted(EXERTION_CHANCE_CREATURE),
            71,
            "P = 71/101 ≈ 70.3 %"
        );
        assert_eq!(admitted(EXERTION_CHANCE_PLAYER), 36, "P = 36/101 ≈ 35.6 %");
        // Roughly half, which is the audible point of the split.
        assert!(admitted(EXERTION_CHANCE_PLAYER) * 2 <= admitted(EXERTION_CHANCE_CREATURE) + 2);
        // Class 1 (ExertionCritical) is 100 in both twins — combat.rs skips the roll entirely on
        // a crit, and this is why that shortcut is faithful rather than a convenience.
        assert_eq!(admitted(100), 101, "a critical swing always grunts");
    }

    /// The injury pair's shape (decision 2073), and the reason the victim's grunt thins out:
    /// **class 2 is rolled, classes 3 and 9 are not.** `0x8626d4[2] = 60` for a creature victim
    /// and `0x86424c[2] = 30` for a player one — so a creature vocalises about three hits in
    /// five, a player fewer than one in three, while every crit and every crushing blow sounds.
    ///
    /// The numbers are the point: benilla played all three unconditionally before 2073, which is
    /// 40 of every 100 creature grunts too many and 70 of every 100 on a player.
    #[test]
    fn an_ordinary_wound_grunt_is_rolled_but_a_crit_and_a_crush_always_sound() {
        let bucket = |r: u32| ((101u64 * u64::from(r)) >> 32) as u32;
        let admitted = |threshold: u32| {
            (0..=100)
                .filter(|&b| {
                    let r = ((u64::from(b) << 32) / 101) as u32 + 1;
                    bucket(r) == b && bark_chance_pass(threshold, r)
                })
                .count()
        };
        assert_eq!(admitted(INJURY_CHANCE_CREATURE), 61, "P = 61/101 ≈ 60.4 %");
        assert_eq!(admitted(INJURY_CHANCE_PLAYER), 31, "P = 31/101 ≈ 30.7 %");
        // Classes 3 (InjuryCritical) and 9 (InjuryCrushingBlow) carry 100 in both twins, which is
        // why combat.rs skips the roll on a crit and a crushing blow rather than rolling a 100.
        assert_eq!(
            admitted(100),
            101,
            "a crit and a crushing blow always sound"
        );
        // The injury pair is strictly quieter than the exertion pair on the same twin — the
        // attacker grunts more often than the victim does, in both directions.
        assert!(admitted(INJURY_CHANCE_CREATURE) < admitted(EXERTION_CHANCE_CREATURE));
        assert!(admitted(INJURY_CHANCE_PLAYER) < admitted(EXERTION_CHANCE_PLAYER));
    }

    /// **The two per-unit handles are disjoint** (decision 1399): the one-shot bark occupies
    /// `[unit+0xb20]`, the greeting line `[unit+0xb1c]`, and neither may answer the other's
    /// question. This is the regression guard for the way the voice slot was added — tagging the
    /// bark's channel with its unit made it visible to the greeting latch, which would have let a
    /// creature's aggro roar silence its own hello.
    #[test]
    fn the_voice_slot_and_the_greeting_latch_are_disjoint() {
        let bear = Entity::from_raw_u32(1).expect("valid entity id");
        let other = Entity::from_raw_u32(2).expect("valid entity id");
        let bark = (Some(bear), Latch::Voice(0));
        let greet = (Some(bear), Latch::Greeting);
        // The case decision 1399 was actually about: a channel tagged with the unit for
        // *ownership* — its body loop, a missile's travel loop, a water splash — takes no latch
        // at all. Before the marker these were indistinguishable from the greeting line, so a
        // creature with a body loop could never say hello.
        let body_loop = (Some(bear), Latch::None);

        assert!(occupies_voice_slot(bark.0, bark.1, bear));
        assert!(!occupies_greeting_latch(bark.0, bark.1, bear));
        assert!(occupies_greeting_latch(greet.0, greet.1, bear));
        assert!(!occupies_voice_slot(greet.0, greet.1, bear));

        assert!(
            !occupies_greeting_latch(body_loop.0, body_loop.1, bear),
            "a body loop must not hold the greeting latch — 6 of the 4509 greeting displays \
             also carry a nonzero loop_sound, and every one of them was mute"
        );
        assert!(!occupies_voice_slot(body_loop.0, body_loop.1, bear));

        // Neither slot is world-global: a bark on one unit says nothing about another.
        assert!(!occupies_voice_slot(bark.0, bark.1, other));
        assert!(!occupies_greeting_latch(greet.0, greet.1, other));

        // An untagged channel (every ordinary one-shot) is in neither slot.
        assert!(!occupies_voice_slot(None, Latch::None, bear));
        assert!(!occupies_greeting_latch(None, Latch::None, bear));
    }

    /// The ceiling is the reference's, not a number we liked: `FSOUND_Init(44100, 12, 0x82)`,
    /// verified on the binary at `0x7a492b`, with the
    /// hardware bank forced to 0 on any host without hardware mixing. Pinned so a later "let's
    /// raise it a bit" has to argue with the byte-fact rather than drift past it.
    #[test]
    fn the_ceiling_is_the_references_twelve() {
        assert_eq!(SOFTWARE_CHANNELS, 12);
    }

    /// Under the ceiling the cap is invisible — no walk, no steal, whatever the amplitudes are.
    #[test]
    fn under_the_ceiling_everything_plays() {
        let live = [(0, 0.9f32), (1, 0.8)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS - 1, 0.001),
            VoiceSlot::Free,
            "a near-silent sound still plays while there is room"
        );
    }

    /// At the ceiling, the **quietest** one-shot loses — not the oldest, not the newest. This is
    /// the rule that makes a hard cap sound like a mix: the sword in your face beats twelve
    /// footsteps down the corridor, whichever started first.
    #[test]
    fn at_the_ceiling_the_quietest_one_shot_loses_to_a_louder_newcomer() {
        // Index 2 is the quietest; index 0 is the oldest and must survive.
        let live = [(0, 0.50f32), (1, 0.30), (2, 0.05), (3, 0.40)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS, 0.9),
            VoiceSlot::Steal(2)
        );
    }

    /// A newcomer quieter than everything playing is dropped rather than stealing. Twelve loud
    /// sounds are what the player is actually listening to; a distant thirteenth is not worth
    /// silencing one of them.
    #[test]
    fn at_the_ceiling_a_quieter_newcomer_is_dropped() {
        let live = [(0, 0.50f32), (1, 0.30), (2, 0.20)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS, 0.10),
            VoiceSlot::Denied
        );
    }

    /// **The mass-buff case, as arithmetic.** Prayer of Fortitude lands five sample-identical
    /// copies of one 0 dBFS file in a single frame, at identical amplitude. The comparison is
    /// strict precisely so this converges: equal-loudness newcomers are dropped instead of
    /// evicting each other in a loop that would spend the entire budget churning between
    /// indistinguishable sounds. This is the exact case the director reported.
    #[test]
    fn identical_copies_do_not_churn_the_budget() {
        let live: Vec<(usize, f32)> = (0..12).map(|i| (i, 0.7)).collect();
        assert_eq!(
            pick_voice_slot(live.iter().copied(), SOFTWARE_CHANNELS, 0.7),
            VoiceSlot::Denied,
            "an equally-loud copy must not evict its own twin"
        );
    }

    /// A loop is a bed — an ambience, a tracked body loop — and cutting one leaves a hole that
    /// stays open, where a stolen one-shot costs at most a tail. So loops are never offered as
    /// victims, and a ceiling reached entirely by beds simply drops the newcomer.
    #[test]
    fn beds_are_never_stolen() {
        // `claim_voice` filters loops out before this point, so "only loops live" reaches the
        // decision as an empty stealable set.
        assert_eq!(
            pick_voice_slot(std::iter::empty(), SOFTWARE_CHANNELS, 1.0),
            VoiceSlot::Denied
        );
    }

    /// A channel that holds a per-unit latch is never a steal victim — and unlike the bed rule
    /// above, this one is correctness, not taste.
    ///
    /// A source-tagged channel's liveness **is** the latch (`[unit+0xb1c]` for the greeting line,
    /// `[unit+0xb20]` for the bark). Steal it and the latch releases early, so the next packet on
    /// that unit is free to re-fire — turning the voice cap into a *repeat generator* under
    /// exactly the load it exists to handle. `creature.rs` measured the ungated version of that:
    /// a bear's aggro roar 63 times in two minutes, up to thirty overlapping copies.
    #[test]
    fn a_channel_holding_a_units_latch_is_never_stolen() {
        assert!(
            stealable(false, Latch::None),
            "an ordinary one-shot is the whole point of the steal"
        );
        assert!(
            !stealable(false, Latch::Greeting),
            "a greeting line holds [unit+0xb1c] — stealing it lets the unit re-greet at once"
        );
        assert!(
            !stealable(false, Latch::Voice(0)),
            "a bark holds [unit+0xb20] — stealing it lets the unit re-bark at once"
        );
        assert!(
            !stealable(true, Latch::None),
            "beds are held for their own reason"
        );
    }

    /// **The director's reported case, as arithmetic.** Prayer of Fortitude lands `HolyProtection`
    /// on five party members inside one frame. A probe capture measured exactly that as the
    /// loudest moment of a whole session — 3.73x full scale with only 10 voices live, so the
    /// 12-voice ceiling never even applied. Sample-aligned copies of one 0 dBFS file sum
    /// *coherently*: five of them is one sound +14 dB, not five sounds.
    ///
    /// The cap is on **live copies of the same kit**, so the fan-out collapses to
    /// [`SAME_KIT_MAX`] however many targets the buff had.
    #[test]
    fn a_mass_buff_collapses_to_the_same_kit_cap() {
        // What the gate sees: N already-live copies of kit 3116, asked for one more. The real
        // predicate, not a restatement of it — a mirrored copy here would keep passing after the
        // gate itself changed.
        let live_copies = |n: usize| same_kit_cap_blocks(false, n);
        assert!(!live_copies(0), "the first copy always plays");
        assert!(
            !live_copies(1),
            "so does the second — two still reads as 'several'"
        );
        for already in SAME_KIT_MAX..=8 {
            assert!(
                live_copies(already),
                "copy {} of a five-target buff must be dropped, not stacked",
                already + 1
            );
        }
    }

    /// **The lane split.** Both suppressors are the ONE-SHOT lane's (`0x458f40` → `0x7a66a0`);
    /// the ambient emitter pool's opens go through `0x7a5680` → `0x7a54d0` and never reach them,
    /// because that lane already guarantees one channel per SoundEntries id structurally.
    ///
    /// `NightElfStreetLampLoop` is the case that proves it matters rather than tidies: Flags
    /// **0x220**, so an un-exempt pool entry could not replace its own 3.0 s fade-out — the lamp's
    /// hum would drop out for three seconds every time its entry came back under the cap.
    #[test]
    fn the_emitter_pool_lane_is_exempt_from_the_one_shot_suppressors() {
        const LAMP: u32 = 0x220;
        assert!(
            no_duplicates_blocks(false, LAMP, 1),
            "the one-shot lane still honours the reference's own 0x20"
        );
        assert!(
            !no_duplicates_blocks(true, LAMP, 1),
            "a pool entry must be able to replace its own fading channel"
        );
        assert!(
            !same_kit_cap_blocks(true, 8),
            "…and the coherent-copy fallback is the same lane's, so it is exempt too"
        );
        assert!(
            same_kit_cap_blocks(false, SAME_KIT_MAX),
            "the exemption is scoped to the caller that asks for it, not global"
        );
    }

    /// The looser cap only exists for rows the reference left ungated. A row carrying flag 0x20 is
    /// still capped at **one** by the byte-verified gate above it, and this must not loosen that.
    #[test]
    fn the_reference_no_duplicate_flag_still_wins() {
        // A const block: the relationship is a compile-time fact, not a runtime one.
        const { assert!(SAME_KIT_MAX > 1) };
        assert_eq!(sound_kit_flags::NO_DUPLICATES, 0x20);
    }
}
