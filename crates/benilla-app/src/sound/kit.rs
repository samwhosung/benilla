//! The kit player: plays a `SoundEntries` kit by id or name (`PlaySoundById 0x458850`,
//! `PlaySoundByName 0x458030`) through the client's audibility, duplicate and per-bus gates, the
//! depleting weighted variation pick (`0x45bb70`/`0x45bd40`), per-shot volume and pitch variation
//! (`0x458c60`/`0x458da0`), and the per-frame channel pump (`0x7a4ad0`/`0x7a5000`/`0x7a5dc0`).
//!
//! The variation gates are separate kit flag bits (0x400 pitch, 0x800 volume, `0x45c080`) and the
//! draw is [`math::variation_draw`] (`0x455c70`). A looping channel beyond its cutoff stops here
//! and restarts on its next trigger; the reference mutes it past the cutoff and restarts it on
//! re-entry itself (`0x7a5095`, `0x7a51a2`).

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

/// The volume slider that scales a channel (master is global). Set by the caller, as the
/// reference's play drivers set channel flag 0x2 for music, 0x8 for ambience and SFX otherwise
/// (`0x45ce60`/`0x45cf00`); never derived from the kit's `SoundType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SoundCategory {
    Sfx,
    Music,
    Ambience,
}

/// One kit's depleting variation pool (`0x45bb70` pick, `0x45bd40` refill): with all-1 weights, no
/// repeat until every variation has played.
struct PickState {
    remaining: Vec<u32>,
}

/// xorshift32. The client draws from its own engine generator; only the transform of a draw is
/// matched, not the sequence.
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

/// The kit catalog and play-side caches; present only when the client data loaded.
#[derive(Resource)]
pub(crate) struct SoundKits {
    catalog: SoundKitCatalog,
    /// Decoded SFX by lowercased path, cloned per play with shared frames (SoundFileDataCache).
    cache: HashMap<String, StaticSoundData>,
    pick: HashMap<u32, PickState>,
    rng: Rng,
}

impl SoundKits {
    /// One 32-bit draw for [`bark_chance_pass`], off the stream the per-shot variations share.
    pub(super) fn roll(&mut self) -> u32 {
        self.rng.next()
    }

    /// The kit table, for [`super::vocal`]'s table build, which needs variation counts ahead of
    /// play (the reference reads `0x45cda0(kit) + 0x94` there).
    pub(super) fn catalog(&self) -> &SoundKitCatalog {
        &self.catalog
    }
}

/// One playing channel the pump owns, the client's channel struct.
pub(crate) struct ActiveChannel {
    pub(crate) kit: u32,
    /// The entity this channel belongs to; [`Latch`] says whether it also holds a latch, which
    /// the pump's reap releases when the sound stops.
    source: Option<Entity>,
    /// A source-tagged loop follows its unit (the tracked play `0x61fec0`); one-shots stay put.
    tracked: bool,
    handle: mixer::StaticSoundHandle,
    /// The spatial track keeping the 3D voice alive; `None` = 2D (main track).
    track: Option<mixer::SpatialTrackHandle>,
    pos: Option<Vec3>,
    /// Kit `MinDistance`, the rolloff knee.
    min_dist: f32,
    /// Kit `DistanceCutoff`, the cull radius (0 = never cull).
    cutoff: f32,
    /// The per-shot volume `v` (base + variation) the mix multiplies each frame.
    v: f32,
    /// A driver-animated gain (default 1.0), the fade lane for loops whose volume the pump owns,
    /// such as the liquid loops' 5.0 s ramps; a handle-level fade would be overwritten. Set by
    /// [`set_source_kit_gain`].
    gain: f32,
    category: SoundCategory,
    /// The voice bus this channel occupies, for the concurrency cap.
    bus: Bus,
    /// A loop is a bed, which the voice cap never steals ([`stealable`]).
    looping: bool,
    /// The effective amplitude, `category · v · gain · rolloff · near_field`, refreshed each frame
    /// by [`pump_channels`] so the voice cap can rank channels without walking the world.
    amp: f32,
    /// Which per-unit latch this channel's liveness represents ([`Latch`]).
    latch: Latch,
}

/// Which per-unit latch a channel holds. The reference keeps separate handles on a unit,
/// `[unit+0xb1c]` for its greeting and `[unit+0xb20]` for its bark; a `source` tag alone only
/// means the channel belongs to the unit, so a body loop never holds the greeting latch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Latch {
    /// Ownership only: the pump follows it, a despawn stops it, it blocks nothing (body, missile
    /// and liquid loops, spell holds).
    #[default]
    None,
    /// The greeting line's `[unit+0xb1c]` (`0x60c28c`/`0x60c40a`): a unit whose greeting still
    /// sounds refuses a new one.
    Greeting,
    /// The one-shot bark's `[unit+0xb20]`, the per-unit slot of the bark dispatch `0x623a40`
    /// (jump table `0x623afc`), carrying the bark state `[unit+0xb24]`. A new bark whose state is
    /// `<=` the sounding one is dropped; a higher one stops it and plays (`0x623a82`/`0x623a88`,
    /// stop at `0x623a95`).
    ///
    /// | state | `CreatureSoundData` column | what |
    /// |---|---|---|
    /// | 0 | 10 | the hostile aggro bark (`SMSG_AI_REACTION`) |
    /// | 1 | 28 | the pet's order bark (`SMSG_PET_ACTION_SOUND` selector 0) |
    /// | 2 | 27 | the pet's attack bark (`SMSG_PET_ACTION_SOUND` selector 1) |
    /// | 3 | none | plays nothing, still stops and latches |
    /// | 4 | 6 | the death bark, the maximum |
    ///
    /// The state is read only while `[0xb20]` is live (`0x623a74`/`0x623a7d`), so it can live on
    /// the channel.
    Voice(u8),
    /// A `SMSG_PLAY_OBJECT_SOUND` (`0x278`) live on this unit, the `AISOUNDDESC` pool at
    /// `[0xb05f38]`. While one is live the unit's own vocals of classes 0 to 3 and 8 are suppressed
    /// (`0x4591f0`, queried from `0x6234cb` and `0x623a59`; class test `0x6234bb`); the player twin
    /// `0x62f880` has no such gate. It ends with the sound or when the emitter leaves
    /// `DistanceCutoff` (`0x457a50`).
    ObjectSound,
}

/// The class-bark chance roll (`0x623520`): `r = MulHi32(101, rand32)` in 0..=100, pass iff
/// `threshold >= r`, so P = (threshold + 1) / 101. The reference's generator (`0x882664`, reseeded
/// from the tick at `0x402802`) is not reproduced: any 32-bit draw serves, the arithmetic matches.
pub(super) fn bark_chance_pass(threshold: u32, roll: u32) -> bool {
    ((101u64 * u64::from(roll)) >> 32) as u32 <= threshold
}

/// The class-5 (`$FDX` stand) threshold, 40 in both the creature table `0x8626d4` and the player
/// twin `0x86424c`. Only classes 0, 2 and 5 are below 100, so `$WNG`, `$WGG` (classes 7, 10) and
/// the ALERT bark (class 8) always pass.
pub(super) const STAND_CHANCE: u32 = 40;

/// The class-0 (exertion) threshold, `0x8626d4[0] = 70`; the player twin is 35, so a player grunts
/// about half as often. Class 1 (ExertionCritical) is 100 in both, so a crit always grunts.
pub(super) const EXERTION_CHANCE_CREATURE: u32 = 70;
/// The player twin of [`EXERTION_CHANCE_CREATURE`], `0x86424c[0]`.
pub(super) const EXERTION_CHANCE_PLAYER: u32 = 35;

/// The class-2 (ordinary injury) threshold, `0x8626d4[2] = 60`; the player twin is 30. The twin is
/// the victim's: the roll runs in its `[vtable+0x88]` (`0x623490` creature, `0x62f880` player),
/// on the route `0x624530` → `0x623520`/`0x62f940` → the column selector `0x623020`. Classes 3
/// (InjuryCritical) and 9 (InjuryCrushingBlow) are 100 in both and never rolled.
pub(super) const INJURY_CHANCE_CREATURE: u32 = 60;
/// The player twin of [`INJURY_CHANCE_CREATURE`], `0x86424c[2]`.
pub(super) const INJURY_CHANCE_PLAYER: u32 = 30;

/// The class-5 cooldown: 10 000 ms on one global timestamp shared by every unit (`0x623290`,
/// `[0xc4e0e4]`), stamped on an allowed attempt before the column-is-zero bail, so a silent or
/// culled `$FDX` burns it too.
pub(super) const STAND_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

/// A voice bus, the reference's concurrency domain and index into its cap table (`0x87ce60`). Not
/// the volume category: the bus is `[chan+0x84]` (0..12), the category flag bits in `[chan+0x38]`.
/// A play on a full bus is refused, with no steal or queue (the pre-play gate `0x7a66a0` returns 1
/// or 0), and callers do not retry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Bus(pub(super) u8);

impl Bus {
    /// Bus 0, uncapped: spell impacts, UI, music, ambience, the `$CSS` miss whoosh and the `$FSD`
    /// armor foley, and every play not routed elsewhere.
    pub(super) const DEFAULT: Bus = Bus(0);

    /// Bus 1, cap 1: your character's error speech (`0x458250`, `0x4582d4`), its only tenant.
    /// `0x458250` tries the annoyed line and then the ordinary one; this cap keeps the second from
    /// sounding over the first.
    pub(super) const ERROR_SPEECH: Bus = Bus(1);

    /// Bus 5, cap 1: the attacker's exertion vocal (classes 0 and 1, `0x624786` → `0x623b10`), one
    /// in the world at a time. `0x623b70` routes some units to bus 11, also cap 1, so routing by
    /// column gives the same answer.
    pub(super) const EXERTION: Bus = Bus(5);

    /// Bus 6, cap 2: a connecting swing's whoosh (`WeaponSwingSounds2`, `0x624c81` → `0x457f60` →
    /// `0x458890`); `0x624ca0` plays it or the miss whoosh on bus 0, never both.
    pub(super) const WEAPON_SWING: Bus = Bus(6);

    /// Bus 7, cap 2: the victim's wound vocal (classes 2, 3 and 9; no shipped row populates 9).
    /// Bus 12 is its classified twin, also cap 2.
    pub(super) const INJURY: Bus = Bus(7);

    /// Bus 8, cap 1: the local player's own wound vocal, which the player twin substitutes for
    /// classes 2, 3 and 9 (`0x62f8c5..0x62f8f3`). Exertion stays on [`Self::EXERTION`].
    pub(super) const SELF_INJURY: Bus = Bus(8);

    /// Bus 9, cap 6: the terrain footstep off `$FSD` (`0x62342a` → `0x458380`). The same `$FSD`
    /// first plays the armor foley on bus 0 (`[vt+0x8c]` → `0x4584e0`).
    pub(super) const FOOTSTEP: Bus = Bus(9);

    /// Bus 10, cap 4: melee contact, the `WeaponImpactSounds` hit (`0x624977` → `0x457ec0`), the
    /// `CustomAttack[n]` column that replaces it (`0x6248ea`) and the parry or block clang
    /// (`0x623640` → `0x457dc0`). The deflect clang is a fixed kit on bus 0.
    pub(super) const MELEE_IMPACT: Bus = Bus(10);
}

/// The reference's cap table, `.data` `0x87ce60`: 13 dwords with no writer, read only at
/// `0x7a66b5`.
const BUS_CAP: [u32; 13] = [0x7fff_ffff, 1, 2, 2, 1, 1, 2, 2, 1, 6, 4, 1, 2];

/// How many copies of one kit may sound at once, for kits without the `NO_DUPLICATES` flag.
/// Deviation: the reference leaves those kits uncapped; sample-aligned copies of one file sum
/// coherently (five are one sound +14 dB), so a mass buff distorts.
const SAME_KIT_MAX: usize = 2;

/// The reference's per-kit suppressor: kit flag 0x20 and a same-kit channel still live
/// (`0x458f40` lifts it into the flags word the pre-play gate `0x7a66a0` reads).
fn no_duplicates_blocks(dedupe_exempt: bool, flags: u32, live_same_kit: usize) -> bool {
    !dedupe_exempt && flags & sound_kit_flags::NO_DUPLICATES != 0 && live_same_kit > 0
}

/// The [`SAME_KIT_MAX`] cap, part of the one-shot lane ([`PlayExtras::dedupe_exempt`]).
fn same_kit_cap_blocks(dedupe_exempt: bool, live_same_kit: usize) -> bool {
    !dedupe_exempt && live_same_kit >= SAME_KIT_MAX
}

/// The reference's global voice ceiling: `FSOUND_Init(44100, 12, 0x82)` (`0x7a492b`, the
/// `SoundSoftwareChannels` default). The hardware bank is forced to 0 without hardware mixing, so
/// it is 12 on any host. It counts every channel, music and ambience included
/// ([`SoundOutput::live_voices`]).
pub(crate) const SOFTWARE_CHANNELS: usize = 12;

/// Make room for one more voice or refuse; `true` if the caller may start a sound. At the ceiling
/// the quietest live one-shot loses its slot to a louder newcomer, and a newcomer quieter than
/// everything is dropped.
///
/// Deviation: the reference never steals. Every WoW voice is an `FSOUND_Stream` at priority 256
/// (`fmod.dll 0x1002be47`) and FMOD's steal scan skips `>= 256` (`0x100268e9`), so its 13th sound
/// is dropped. FMOD's own allocator rule (`0x100268e6`) runs here instead, because dropping the
/// newest sound silences a near one in favour of distant ones.
fn claim_voice(out: &mut SoundOutput, candidate_amp: f32) -> bool {
    if out.live_voices() >= SOFTWARE_CHANNELS {
        // Reap first: a channel that ended earlier this frame would still count against the
        // ceiling until the pump runs.
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
            // Stopping a waveform mid-sample clicks; fade it.
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

/// Can the voice cap steal this channel? Never a loop, whose loss leaves a hole that stays open,
/// and never a latch holder: its liveness is the unit's latch, so stealing it would let the unit
/// fire again at once.
fn stealable(looping: bool, latch: Latch) -> bool {
    !looping && latch == Latch::None
}

/// What [`claim_voice`] decided.
#[derive(Debug, PartialEq, Eq)]
enum VoiceSlot {
    /// Under the ceiling.
    Free,
    /// At the ceiling; this channel index loses its slot.
    Steal(usize),
    /// At the ceiling and nothing live is quieter: drop the new sound.
    Denied,
}

/// The voice-cap decision over the stealable channels' `(index, amplitude)`. Strict: a newcomer
/// must be louder than the quietest, so identical copies of a mass buff do not evict each other.
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

/// Is `bus` already carrying its cap of live channels?
fn bus_at_cap(live: impl Iterator<Item = Bus>, bus: Bus) -> bool {
    let cap = BUS_CAP[usize::from(bus.0)];
    // Bus 0 is effectively unlimited: skip the walk on the common path.
    cap != BUS_CAP[0] && live.filter(|b| *b == bus).count() as u32 >= cap
}

/// The optional half of [`play_kit_ext`]'s play surface.
#[derive(Clone, Copy, Default)]
pub(super) struct PlayExtras {
    /// An explicit variation, bypassing the depleting pool (the client's `variant != -1`); the NPC
    /// greeting cycler drives it.
    pub(super) variant: Option<usize>,
    /// The entity the channel belongs to ([`ActiveChannel`]'s `source`).
    pub(super) source: Option<Entity>,
    /// Loop regardless of the kit's 0x200 flag, for drivers whose column is the loop authority:
    /// every `CreatureSoundData` column-23 kit is named `*Loop*` yet half lack 0x200. The
    /// reference's loop start `0x461d80` is untraced.
    pub(super) force_loop: bool,
    /// Skip `NO_DUPLICATES` and [`SAME_KIT_MAX`], the one-shot lane's gates (`0x458f40` →
    /// `0x7a66a0`). The ambient emitter pool ([`super::emitter_pool`]) keeps one entry per kit
    /// (`0x461e60`) and opens through `0x7a5680` → `0x7a54d0`, which never reaches that gate;
    /// gated, a 0x220 kit such as `NightElfStreetLampLoop` could not replace its own fade-out.
    pub(super) dedupe_exempt: bool,
    /// Which per-unit latch this play takes; a `source` alone takes none.
    pub(super) latch: Latch,
    /// The voice bus this play competes on; defaults to the uncapped bus 0.
    pub(super) bus: Bus,
    /// `0x458890`'s per-shot multiplier (1.0 at every site but one), applied inside
    /// [`math::variation_volume`] before the 0..1 clamp and distance attenuation.
    pub(super) volume_mult: Volume,
}

/// A per-shot volume multiplier defaulting to 1.0, so `PlayExtras` can derive `Default`.
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

/// Resolve, gate, pick, decode, play. `pos: None` is 2D; `category` is the caller's
/// ([`SoundCategory`]). `Ok(false)` means a gate dropped it, which is not an error.
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

/// [`play_kit`] with [`PlayExtras`] (`0x458f90(kit, variant, posPtr)`). Returns whether a channel
/// opened, as the play core `0x45ce60` returns the channel or 0; [`super::vocal`]'s escalation
/// counter reads it.
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
    // No sound starts under the loading screen ([`SoundConfig::world_hold`]): the reference's
    // world load blocks, so the events these would voice pass unheard.
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
    // Selection-time audibility (`0x45cdf0`): a positional kit beyond its cutoff never allocates.
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

    // The per-bus cap, the pre-play gate `0x7a66a0`'s always-on arm, before the duplicate walk
    // (`0x7a66b5` precedes `0x7a66c4`). It counts allocated channels (`[0xcf553c + 4*bus]`), and a
    // distance cull releases one, as the pump's reap does here.
    if bus_at_cap(out.channels.iter().map(|c| c.bus), bus) {
        return Ok(false);
    }

    // Duplicate suppression, the gate's other arm: kit flag 0x20 and a same-kit channel still
    // live drop the play (`0x458f40` lifts `SoundEntries +0x7c` bit 0x20 into the flags word).
    let live_same_kit = out.channels.iter().filter(|c| c.kit == id).count();
    if no_duplicates_blocks(dedupe_exempt, flags, live_same_kit) {
        return Ok(false);
    }

    // The same-kit cap ([`SAME_KIT_MAX`]) for kits the flag leaves ungated.
    if same_kit_cap_blocks(dedupe_exempt, live_same_kit) {
        out.copies_dropped += 1;
        return Ok(false);
    }

    // An explicit index when the caller drives the cycle, else the depleting weighted pick.
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

    // Per-shot variation: 0x800 volume (`0x458c60`), 0x400 pitch (`0x458da0`). No 1.12 kit sets
    // 0x800.
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

    // The global ceiling goes last: it alone can stop another sound, and must not for a play a
    // cheaper gate drops.
    if !claim_voice(out, amp) {
        return Ok(false);
    }

    // No audio device (`WOW_NOSOUND=1`, CI, a lost device) is a no-op: `sound::plugin` warns once
    // at startup, and `false` already means dropped.
    let Some(mixer) = out.mixer.as_mut() else {
        return Ok(false);
    };
    let (track, handle) = match pos {
        Some(p) => {
            // `EAXDef 0` has no `SoundSamplePreferences` row and the reference gives it no reverb
            // (`0x45cdc0`/`0x7a5bf0`), so NPC voice lines (every `SoundType 17` row) stay dry.
            let (t, h) = mixer.play_3d(data, p, eax_def != 0)?;
            (Some(t), h)
        }
        None => (None, mixer.play_2d(data)?),
    };
    // Every play, named with its wet or dry class, at `RUST_LOG=benilla_app::sound=debug`.
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
    // And on the probe's timeline when one is recording.
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

/// The state of `unit`'s live bark: `[unit+0xb20]` live and `[unit+0xb24]`; `None` is a free slot
/// (`0x623a74`/`0x623a7d`). The body loop (`0x623800`) and the greeting are separate handles.
pub(super) fn unit_voice_state(out: &SoundOutput, unit: Entity) -> Option<u8> {
    out.channels.iter().find_map(|c| match c.latch {
        Latch::Voice(state) if c.source == Some(unit) => Some(state),
        _ => None,
    })
}

/// Stop whatever holds `unit`'s bark slot (`0x623a95 call 0x7a5700`). It runs on every admitted
/// bark before the column is read, so a bark with column 0 still silences the one it supersedes
/// (`0x623aee`).
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

/// Does this channel hold `unit`'s bark slot (`[unit+0xb20]`)? Disjoint from
/// [`occupies_greeting_latch`].
pub(super) fn occupies_voice_slot(source: Option<Entity>, latch: Latch, unit: Entity) -> bool {
    source == Some(unit) && matches!(latch, Latch::Voice(_))
}

/// Does this channel hold `unit`'s greeting latch (`[unit+0xb1c]`)?
pub(super) fn occupies_greeting_latch(source: Option<Entity>, latch: Latch, unit: Entity) -> bool {
    source == Some(unit) && latch == Latch::Greeting
}

/// Is a server-pushed object sound live on `unit` (the `AISOUNDDESC` pool query `0x4591f0`)? See
/// [`Latch::ObjectSound`].
pub(super) fn object_sound_playing(out: &SoundOutput, unit: Entity) -> bool {
    out.channels
        .iter()
        .any(|c| c.source == Some(unit) && c.latch == Latch::ObjectSound)
}

/// Is `source`'s greeting line still live (`[unit+0xb1c]`, `0x60c28c`/`0x60c40a`)? A unit whose
/// greeting sounds refuses a new one; the pump's reap releases it. Other owned channels do not
/// count.
pub(super) fn source_playing(out: &SoundOutput, source: Entity) -> bool {
    out.channels
        .iter()
        .any(|c| occupies_greeting_latch(c.source, c.latch, source))
}

/// Is `source` playing kit `kit_id`? The creature body loop's not-already-playing gate (`0x623800`
/// latches its own channel); kit-scoped so a greeting or spell hold never masks it.
pub(super) fn source_kit_playing(out: &SoundOutput, source: Entity, kit_id: u32) -> bool {
    out.channels
        .iter()
        .any(|c| c.source == Some(source) && c.kit == kit_id)
}

/// The kit's name, or `None` with no `SoundEntries` row: the reference's `0x45cda0(id)` null test,
/// which permanently fails a doodad emitter pool entry (`[+0xE00] = -id`).
pub(super) fn kit_name(kits: &SoundKits, id: u32) -> Option<&str> {
    kits.catalog.get(id).map(|k| k.name.as_str())
}

/// Whether kit `id` loops (flag 0x200), the client's `0x458830` split between tracked loops and
/// one-shots.
pub(super) fn kit_looping(kits: &SoundKits, id: u32) -> bool {
    kits.catalog
        .get(id)
        .is_some_and(|k| k.flags & sound_kit_flags::LOOPING != 0)
}

/// Set the fade gain of `source`'s channel on `kit_id` ([`ActiveChannel::gain`]), applied by the
/// pump next frame. No-op if the channel is gone.
pub(super) fn set_source_kit_gain(out: &mut SoundOutput, source: Entity, kit_id: u32, gain: f32) {
    for c in &mut out.channels {
        if c.source == Some(source) && c.kit == kit_id {
            c.gain = gain.clamp(0.0, 1.0);
        }
    }
}

/// Force-stop `source`'s channels on kit `kit_id`: a looping spell kit rides the tracked play
/// `0x61fec0` and dies with its effect (`0x614150`). Kit-scoped so the caster's greeting survives.
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

/// Force-stop every channel tagged with `source` on despawn, the unit teardown stop (`0x5fbb6c`).
/// Returns how many it stopped, so the doodad reaper in [`super::anim_events`] can tell a reaped
/// loop from a silent source.
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

/// The `PlaySoundFile` path: a raw file with no gates and no variation, volume 1.0, 2D on the
/// caller's category, through the kit decode cache. Kit id 0 keeps it out of kit dedupe.
pub(crate) fn play_file(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    path: &str,
    category: SoundCategory,
) -> Result<()> {
    // The loading-screen hold, as in [`play_kit_ext`].
    if config.world_hold {
        return Ok(());
    }
    let data = kits.sfx(assets, path)?;
    let amp = config.category_amp(category);
    let data = data.volume(mixer::amp_to_db(amp));
    if !claim_voice(out, amp) {
        return Ok(());
    }
    // No device is a no-op, as in [`play_kit_ext`].
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
    /// A kit id by its `PlaySoundByName` key (the `0x458030` name registry), such as the ghost
    /// tracks "Ghost" and "GhostMusic".
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

    /// A kit's variation count (0 for unknown or file-less); the NPC greeting cycler uses it to
    /// know when repeat interacts reach the pissed line.
    pub(super) fn variations(&self, kit: u32) -> usize {
        self.catalog.get(kit).map_or(0, |k| k.files.len())
    }

    /// `(file path, base volume)` of a picked variation, for zone music and ambience, which stream
    /// the file instead of decoding it.
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

    /// A decoded kit file, read and decoded whole on a cache miss.
    fn sfx(&mut self, assets: &WorldAssets, path: &str) -> Result<StaticSoundData> {
        let key = path.to_ascii_lowercase();
        if let Some(d) = self.cache.get(&key) {
            return Ok(d.clone());
        }
        // The chain, then an addon's loose file ([`benilla_assets::read_chain_or_loose`]):
        // `PlaySoundFile` is by path, and addon audio lives on disk, never in an MPQ.
        let bytes = assets
            .read_file_or_loose(path)
            .with_context(|| format!("reading {path}"))?;
        let data = mixer::sfx_from_bytes(bytes)?;
        self.cache.insert(key, data.clone());
        DECODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(data)
    }
}

/// First-play decodes so far, each a main-thread read and decode; `FPS_PROBE` reads it per frame
/// to annotate a slow frame.
pub(crate) static DECODES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Startup: load the kit catalog off the chain; on failure there is no resource and every play
/// site tolerates that.
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

/// The per-frame channel pump (`0x7a4ad0`): reap finished channels, follow tracked sources, cull
/// positional ones beyond cutoff, recompute each volume.
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
        // A tracked loop follows its unit (`0x61fec0`); a despawned source keeps its last
        // position until the despawn reaper stops the channel.
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
        // Write only on change: `set_volume` queues an audio-thread command even when the value
        // is unchanged. `ch.amp` mirrors the handle's volume from `play` on.
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
            // Beyond cutoff the reference mutes the channel (`0x7a5095`); here it stops.
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
            // Glide, not snap: a step is a click, and a hitch steps every live channel at once.
            ch.handle
                .set_volume(mixer::amp_to_db(ch.amp), mixer::glide());
        }
        true
    });
}

/// Drain the debug panel's "play kit" request: an id, a name, or a path, which plays through
/// [`play_file`].
pub(super) fn apply_kit_debug(
    mut debug: ResMut<DebugState>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    // Read before borrowing mutably: `&mut` through `ResMut` marks `DebugState` changed, which
    // the still-frame gates read.
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
    // `copies` plays the kit N times in one frame (five of kit 3116 is a mass Fortitude on a full
    // party); the per-kit gates still apply.
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

/// `OnExit(InWorld)`: every live kit channel dies with the world; the glue screens' clicks start
/// after this edge.
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

/// Drop the decoded-SFX cache on a map change so the old map's decodes do not pile up; playing
/// channels share their frames, so nothing audible cuts.
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

    /// Pinned as the 13 dwords at `.data` `0x87ce60`.
    #[test]
    fn the_cap_table_is_the_bytes_at_0x87ce60() {
        assert_eq!(BUS_CAP.len(), 13);
        assert_eq!(BUS_CAP, [0x7fff_ffff, 1, 2, 2, 1, 1, 2, 2, 1, 6, 4, 1, 2]);
    }

    /// Each bus refuses at its own cap and not before; bus 0 never refuses.
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

    /// With all-1 weights every variation plays once per cycle (`0x45bb70`/`0x45bd40`).
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

    /// The compare is inclusive: threshold 100 always passes, 40 admits 41 of the 101 buckets.
    #[test]
    fn the_bark_chance_is_inclusive_and_100_is_a_tautology() {
        // 100 never refuses, at either end of the draw space.
        for roll in [0, 1, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
            assert!(bark_chance_pass(100, roll), "threshold 100 refused {roll}");
        }
        // Walked over the bucket boundaries, not sampled.
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

    /// A crit always grunts; an ordinary swing is rolled, a player half as often as a creature.
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
        // Roughly half.
        assert!(admitted(EXERTION_CHANCE_PLAYER) * 2 <= admitted(EXERTION_CHANCE_CREATURE) + 2);
        // Class 1 (ExertionCritical) is 100 in both twins, so combat.rs skips the roll on a crit.
        assert_eq!(admitted(100), 101, "a critical swing always grunts");
    }

    /// Class 2 is rolled (`0x8626d4[2] = 60`, `0x86424c[2] = 30`); classes 3 and 9 always sound.
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
        // Classes 3 and 9 are 100 in both twins, so combat.rs skips the roll for them.
        assert_eq!(
            admitted(100),
            101,
            "a crit and a crushing blow always sound"
        );
        // The victim grunts less often than the attacker, on either twin.
        assert!(admitted(INJURY_CHANCE_CREATURE) < admitted(EXERTION_CHANCE_CREATURE));
        assert!(admitted(INJURY_CHANCE_PLAYER) < admitted(EXERTION_CHANCE_PLAYER));
    }

    /// The bark slot (`[unit+0xb20]`) and the greeting latch (`[unit+0xb1c]`) are disjoint.
    #[test]
    fn the_voice_slot_and_the_greeting_latch_are_disjoint() {
        let bear = Entity::from_raw_u32(1).expect("valid entity id");
        let other = Entity::from_raw_u32(2).expect("valid entity id");
        let bark = (Some(bear), Latch::Voice(0));
        let greet = (Some(bear), Latch::Greeting);
        // A channel the unit only owns (body loop, missile loop, splash) takes no latch.
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

    /// `FSOUND_Init(44100, 12, 0x82)` at `0x7a492b`.
    #[test]
    fn the_ceiling_is_the_references_twelve() {
        assert_eq!(SOFTWARE_CHANNELS, 12);
    }

    /// Under the ceiling everything plays, whatever the amplitudes.
    #[test]
    fn under_the_ceiling_everything_plays() {
        let live = [(0, 0.9f32), (1, 0.8)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS - 1, 0.001),
            VoiceSlot::Free,
            "a near-silent sound still plays while there is room"
        );
    }

    /// At the ceiling the quietest one-shot loses, not the oldest.
    #[test]
    fn at_the_ceiling_the_quietest_one_shot_loses_to_a_louder_newcomer() {
        // Index 2 is the quietest; index 0 is the oldest and must survive.
        let live = [(0, 0.50f32), (1, 0.30), (2, 0.05), (3, 0.40)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS, 0.9),
            VoiceSlot::Steal(2)
        );
    }

    /// A newcomer quieter than everything playing is dropped.
    #[test]
    fn at_the_ceiling_a_quieter_newcomer_is_dropped() {
        let live = [(0, 0.50f32), (1, 0.30), (2, 0.20)];
        assert_eq!(
            pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS, 0.10),
            VoiceSlot::Denied
        );
    }

    /// Equal-amplitude copies of a mass buff are dropped, not evicting each other in a loop.
    #[test]
    fn identical_copies_do_not_churn_the_budget() {
        let live: Vec<(usize, f32)> = (0..12).map(|i| (i, 0.7)).collect();
        assert_eq!(
            pick_voice_slot(live.iter().copied(), SOFTWARE_CHANNELS, 0.7),
            VoiceSlot::Denied,
            "an equally-loud copy must not evict its own twin"
        );
    }

    /// A ceiling reached entirely by loops drops the newcomer.
    #[test]
    fn beds_are_never_stolen() {
        // `claim_voice` filters loops out, so only loops live is an empty stealable set.
        assert_eq!(
            pick_voice_slot(std::iter::empty(), SOFTWARE_CHANNELS, 1.0),
            VoiceSlot::Denied
        );
    }

    /// A latch holder is never stolen: that would release the latch and let the unit re-fire.
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

    /// Prayer of Fortitude's five copies of `HolyProtection` in one frame collapse to
    /// [`SAME_KIT_MAX`].
    #[test]
    fn a_mass_buff_collapses_to_the_same_kit_cap() {
        // N live copies of kit 3116, asked for one more, through the real predicate.
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

    /// The emitter pool's lane skips the one-shot suppressors (`0x458f40` → `0x7a66a0`): a 0x220
    /// lamp loop must replace its own 3.0 s fade-out.
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

    /// A kit with flag 0x20 is still capped at one; the looser cap does not loosen it.
    #[test]
    fn the_reference_no_duplicate_flag_still_wins() {
        const { assert!(SAME_KIT_MAX > 1) };
        assert_eq!(sound_kit_flags::NO_DUPLICATES, 0x20);
    }
}
