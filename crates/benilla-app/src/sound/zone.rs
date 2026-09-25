//! Zone music and ambience: the area-driven schedulers over the kit player.
//!
//! `CurrentArea` (MCNK areaId) → `AreaTable` → `ZoneMusic`/`SoundAmbience`/`ZoneIntroMusicTable`
//! (parent-inherited, `AreaSoundCatalog::resolve`) → SoundEntries kits → streamed MP3/WAV.
//!
//! Music (`0x460040`–`0x460ca0`): same-zone tracks are spaced by a random per-phase silence
//! interval (`0x4601f0`). A zone-music change (`0x4602e0`) or world entry (`0x45ffc0`, at
//! `0x45ffdb`) arms the next start to −1 at `0x836400`, so the incoming track opens that tick
//! (`0x460240`) at full volume: no fade-in primitive (`0x7a57b0`, `0x7a5730`) is called on a
//! music slot.
//!
//! Server-pushed music (`SMSG_PLAY_MUSIC`) takes the same slot. The server re-pushes an event
//! track to loop it (the Darkmoon Faire every 5 s, vmangos `go_scripts.cpp:318`), so a push for
//! the kit already playing is a no-op, the reference's early-out at `0x460342`. Any other push
//! takes the slot as a zone-music change does; the reference's handler is untraced.
//!
//! Ambience (`0x460b00`): an area, interior, day/night or ghost change crossfades over 5.0 s;
//! submerge/emerge is instant (`0x458650` → `0x460af0`). Day is 05:30 ≤ t < 21:00 on the
//! server-synced clock, a hard step with no fade (`0x4578c0`, via the `0x642710` range check).

use bevy::prelude::*;

use benilla_formats::AreaSoundCatalog;

use crate::net::{ServerSoundKind, ServerSoundMessage};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::lighting::GameClock;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit_ext, KitRef, Latch, PlayExtras, SoundCategory, SoundKits};
use super::mixer::{self, StreamingSoundHandle};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The zone-to-audio catalog; absent when the client data did not load.
#[derive(Resource)]
pub(crate) struct AreaSounds(pub(crate) AreaSoundCatalog);

/// The race to exploration-jingle catalog (`ChrRaces.dbc` column 3), played on
/// `SMSG_EXPLORATION_EXPERIENCE`; absent when the client data did not load.
#[derive(Resource)]
pub(crate) struct ExplorationSounds(pub(crate) benilla_formats::ExplorationSoundCatalog);

/// How long after a cinematic ends the zone track comes back: `[0x836400] = tick + 0xbb8` at
/// `0x4603b0(0)`.
const CINEMATIC_MUSIC_RESUME_SECS: f64 = 3.0;

fn phase(clock: &GameClock) -> usize {
    if (330..1260).contains(&clock.minute) {
        0
    } else {
        1
    }
}

/// SoundEntries 4123 `UnderWaterLoop`, the submerged ambience bed.
const UNDERWATER_LOOP_KIT: u32 = 4123;

/// The outgoing music fade, the only fade on the music slot: `0x4602e0` →
/// `0x7a5a10(0x40800000 = 4.0f)`.
const MUSIC_FADE_OUT_MS: u64 = 4000;

/// The Lua `PlayMusic` slot's base volume, set with no fade under the MusicVolume slider
/// (`0x460450` → `0x7a5dc0`; the glue theme uses 0.8, `0x45aeb0`).
const LUA_MUSIC_VOLUME: f32 = 1.0;

/// What either Lua music verb leaves on the zone pump's clock: now + 6.000 s (`0x46049b`,
/// `[0x836400] = 0x42c010() + 0x1770`). Unconditional, even from cold: the write sits between
/// `0x460499`'s `test` and its `je`, so `StopMusic()` with nothing playing still pushes out a
/// start that was due.
const LUA_MUSIC_SCHEDULE_SECS: f64 = 6.0;

/// The ambience crossfade, both legs (`0x460b00`: out `0x7a5a10(0x40a00000 = 5.0f)`, in
/// `0x7a5dc0(0)` → `0x7a57b0(5.0f, kit vol)`); submerge/emerge is instant instead.
const AMBIENCE_TRANSITION_FADE_MS: u64 = 5000;

/// The linear fade-in of a new ambience bed, driven per frame because the per-frame volume feed
/// would override a handle-level ramp.
#[derive(Clone, Copy)]
struct FadeIn {
    start: f64,
    dur: f64,
}

impl FadeIn {
    fn new(start: f64, dur_ms: u64) -> Self {
        Self {
            start,
            dur: dur_ms as f64 / 1000.0,
        }
    }

    /// Gain in `[0, 1]` at `now`, holding at 1 once complete.
    fn gain(&self, now: f64) -> f32 {
        if self.dur <= 0.0 {
            return 1.0;
        }
        (((now - self.start) / self.dur) as f32).clamp(0.0, 1.0)
    }
}

/// The active fade-in gain for a slot, clearing the envelope once it reaches full.
fn fade_in_gain(fade: &mut Option<FadeIn>, now: f64) -> f32 {
    match fade {
        Some(f) => {
            let g = f.gain(now);
            if g >= 1.0 {
                *fade = None;
            }
            g
        }
        None => 1.0,
    }
}

/// Scheduler state and the held stream handles; non-Send, as the handles are not `Sync`.
pub(super) struct ZoneAudio {
    /// The `ZoneMusic` row currently scheduled (0 = none).
    zone_music: u32,
    /// The music slot: a zone track, an intro, or a server-pushed event track.
    music: Option<StreamingSoundHandle<kira::sound::FromFileError>>,
    music_watch: mixer::StreamWatch,
    /// The SoundEntries kit on the music slot (0 = none), what a repeat `SMSG_PLAY_MUSIC` is
    /// compared against.
    music_kit: u32,
    music_kit_vol: f32,
    /// The Lua `PlayMusic` slot, the reference's `[0xb06ccc]`: a second music stream beside the
    /// zone track's `[0xb06cc4]`, driven only by `PlayMusic`/`StopMusic`.
    lua_music: Option<StreamingSoundHandle<kira::sound::FromFileError>>,
    /// The file on that slot, kept to restart it: `0x7a5620` opens it with `SetLoopCount(-1)`
    /// (`0x7a5592`), and kira cannot loop a decode-stream.
    lua_music_path: Option<String>,
    lua_music_watch: mixer::StreamWatch,
    /// When the next zone track starts (`Time::elapsed_secs_f64`); `None` while one plays or the
    /// zone has no music.
    next_track_at: Option<f64>,
    /// Last frame's [`SoundConfig::music_suppressed`], for its edges.
    music_suppressed: bool,
    /// The looping ambience kit currently up (0 = none), its handle and base volume.
    ambience_kit: u32,
    ambience: Option<mixer::StaticSoundHandle>,
    ambience_kit_vol: f32,
    /// The incoming leg of an ambience crossfade, while the new bed ramps up.
    ambience_fade_in: Option<FadeIn>,
    /// Intro row id to last-played time (secs), against `MinDelayMinutes`.
    intro_last: std::collections::HashMap<u32, f64>,
    area: Option<u32>,
    /// The last WMO interior acted on, the override layer.
    interior: Option<super::interior::InteriorAudio>,
    /// Last submersion state; a flip makes the ambience swap instant.
    was_underwater: bool,
    rng: u32,
}

impl Default for ZoneAudio {
    fn default() -> Self {
        Self {
            zone_music: 0,
            music: None,
            music_watch: mixer::StreamWatch::new("zone music"),
            music_kit: 0,
            music_kit_vol: 1.0,
            lua_music: None,
            lua_music_path: None,
            lua_music_watch: mixer::StreamWatch::new("Lua music"),
            next_track_at: None,
            music_suppressed: false,
            ambience_kit: 0,
            ambience: None,
            ambience_kit_vol: 1.0,
            ambience_fade_in: None,
            intro_last: std::collections::HashMap::new(),
            area: None,
            interior: None,
            was_underwater: false,
            rng: 0x1234_5677,
        }
    }
}

impl ZoneAudio {
    fn rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// Whether a Lua `PlayMusic` track holds the override slot (`0x4603d6`); asked of the path,
    /// not the handle, so it stays true across the loop's restart gap.
    fn lua_slot_live(&self) -> bool {
        self.lua_music_path.is_some()
    }

    /// Uniform silence interval (ms) in `[min, max]` for the phase.
    fn silence_ms(&mut self, min: u32, max: u32) -> u32 {
        if max > min {
            min + self.rand() % (max - min + 1)
        } else {
            min
        }
    }
}

/// Startup: load the zone-audio and exploration-sound catalogs.
fn load_area_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_area_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} areas in the zone-audio catalog", cat.len());
            commands.insert_resource(AreaSounds(cat));
        }
        Err(e) => warn!("sound: zone-audio catalog failed to load: {e:#}"),
    }
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_exploration_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!(
                "sound: {} races in the exploration-sound catalog",
                cat.len()
            );
            commands.insert_resource(ExplorationSounds(cat));
        }
        Err(e) => warn!("sound: exploration-sound catalog failed to load: {e:#}"),
    }
}

/// The per-frame scheduler: area and phase changes, the silence timer, the next track, and the
/// stream volumes.
fn zone_audio(
    mut zone: NonSendMut<ZoneAudio>,
    mut out: NonSendMut<SoundOutput>,
    areas: Option<Res<AreaSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
    clock: Res<GameClock>,
    time: Res<Time>,
    // The stream watch compares against wall time, not the paced virtual clock.
    real: Res<Time<bevy::time::Real>>,
    world: benilla_world::world_point::WorldPoint,
    interior: Res<super::interior::CurrentInterior>,
    weather: Res<super::weather::WeatherAmbience>,
    self_store: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    let (Some(areas), Some(mut kits), Some(assets)) = (areas, kits, assets) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let phase = phase(&clock);
    let zone = &mut *zone;

    // The cinematic's music stop: a cut, then a resume at +3.000 s; ambience is untouched.
    // Ahead of the cover's hold arm, which returns, so the edge is not deferred to the reveal.
    apply_music_suppression(zone, config.music_suppressed, now);

    // The loading cover kills the world soundscape, as the reference's loading screen sits over
    // a torn-down world; resetting `area`/`interior` makes the reveal a fresh zone entry.
    if config.world_hold {
        stop_world_soundscape(zone, "loading cover");
        return;
    }

    // ---- area or interior change (the WMO row overrides the terrain chain where nonzero) ----
    let inside = interior.0;
    let area = world.area();
    if area != zone.area || inside != zone.interior {
        zone.area = area;
        zone.interior = inside;
        let resolved = area.and_then(|id| areas.0.resolve(id));
        let (mut music_row, mut intro) = match &resolved {
            Some(r) => (r.music.map(|m| m.id).unwrap_or(0), r.intro),
            None => (0, None),
        };
        if let Some(i) = inside {
            if i.zone_music != 0 {
                music_row = i.zone_music;
            }
            if i.intro_sound != 0 {
                intro = areas.0.intro(i.intro_sound);
            }
        }

        if music_row != zone.zone_music {
            zone.zone_music = music_row;
            // The fade finishes on the audio thread after the handle drops.
            if let Some(mut h) = zone.music.take() {
                h.stop(mixer::fade(MUSIC_FADE_OUT_MS));
            }
            zone.next_track_at = None;
            // The intro preempts the zone track, at full, if off cooldown.
            let mut slot_taken = false;
            if let Some(intro) = intro {
                let ok_at = zone
                    .intro_last
                    .get(&intro.id)
                    .map(|t| t + intro.min_delay_minutes as f64 * 60.0);
                if ok_at.is_none_or(|t| now >= t)
                    && intro.sound_id != 0
                    && start_music_stream(
                        zone,
                        &mut out,
                        &mut kits,
                        &assets,
                        &config,
                        intro.sound_id,
                    )
                {
                    zone.intro_last.insert(intro.id, now);
                    slot_taken = true;
                }
            }
            // Otherwise the zone track starts now, on a change and on world entry alike (the −1
            // that `0x4602e0` and `0x45ffc0` arm); the silence interval is same-zone spacing only.
            if !slot_taken {
                if let Some(kit) = zone_music_row(&areas.0, music_row).map(|m| m.sounds[phase]) {
                    if kit != 0 {
                        start_music_stream(zone, &mut out, &mut kits, &assets, &config, kit);
                    }
                }
            }
        }
    }

    // ---- ambience: the reference's selector (`0x460bd0`) ranks ghost > submerged > weather >
    // interior/zone day/night; the ghost bed is the kit named "Ghost", its swap the 5.0 s
    // crossfade (`0x458680` → `0x460c20`) ----
    let ghost = self_store
        .single()
        .is_ok_and(|store| store.0.player_is_ghost());
    let desired = if ghost {
        kits.id_by_name("Ghost").unwrap_or(0)
    } else if world.submersion().is_water() {
        UNDERWATER_LOOP_KIT
    } else if weather.0 != 0 && world.area_interior().is_none() {
        // `0x460bf7`–`0x460c17`: with the zonetext indoor bit `[0xb06d44]` clear, the weather's
        // SoundEntries is the bed; indoors the selector ignores it, and the swap crossfades.
        weather.0
    } else if let Some(kit) = zone
        .interior
        .filter(|i| i.ambience != 0)
        .and_then(|i| areas.0.ambience_row(i.ambience))
        .map(|a| a.kits[phase])
        .filter(|k| *k != 0)
    {
        kit
    } else {
        zone.area
            .and_then(|id| areas.0.resolve(id))
            .and_then(|r| r.ambience.map(|a| a.kits[phase]))
            .unwrap_or(0)
    };
    if desired != zone.ambience_kit {
        // Submerge/emerge takes `0x460b00`'s no-fade branch (`0x458650` → `0x460af0(1)`).
        let fade_ms = if world.submersion().is_water() != zone.was_underwater {
            0
        } else {
            AMBIENCE_TRANSITION_FADE_MS
        };
        swap_ambience(
            zone, &mut out, &mut kits, &assets, &config, desired, fade_ms, now,
        );
    }
    zone.was_underwater = world.submersion().is_water();

    // ---- music transport ----
    // Reap a finished track and schedule the next after a silence interval (`0x4600b6`).
    if let Some(h) = &zone.music {
        if h.state() == kira::sound::PlaybackState::Stopped {
            zone.music = None;
            let zm = zone.zone_music;
            zone.next_track_at =
                next_track_time(zone, &areas.0, zm, phase, now, config.zone_music_no_delay);
        }
    }
    // The deadline passed: start the next track. Under a live Lua slot the reference's pump bails
    // before reading the deadline (`0x460057`) while this consumes it and is refused at the slot;
    // the same outcome, since only the verb ends the override and it always rewrites the deadline.
    if zone.next_track_at.is_some_and(|t| now >= t) && zone.music.is_none() {
        zone.next_track_at = None;
        if let Some(m) = zone_music_row(&areas.0, zone.zone_music) {
            let kit = m.sounds[phase];
            if kit != 0 {
                start_music_stream(zone, &mut out, &mut kits, &assets, &config, kit);
            }
        }
    }

    // ---- per-frame volumes (sliders are live) ----
    if let Some(h) = &mut zone.music {
        h.set_volume(
            mixer::amp_to_db(config.category_amp(SoundCategory::Music) * zone.music_kit_vol),
            mixer::glide(),
        );
    }
    // A track fading out on a dropped handle is not watched.
    if let Some(h) = &zone.music {
        zone.music_watch.feed(h, f64::from(real.delta_secs()));
    }
    // The incoming leg of the ambience crossfade.
    let ambience_gain = fade_in_gain(&mut zone.ambience_fade_in, now);
    if let Some(h) = &mut zone.ambience {
        h.set_volume(
            mixer::amp_to_db(
                config.category_amp(SoundCategory::Ambience)
                    * zone.ambience_kit_vol
                    * ambience_gain,
            ),
            mixer::glide(),
        );
    }
}

/// The two edges of [`SoundConfig::music_suppressed`] on the music slot: down is a cut, since
/// `0x7a5700` stop-and-destroy takes no duration; up resumes at +3.000 s.
fn apply_music_suppression(zone: &mut ZoneAudio, suppressed: bool, now: f64) {
    if suppressed == zone.music_suppressed {
        return;
    }
    zone.music_suppressed = suppressed;
    // A live Lua track is paused and resumed, and nothing else: `0x4603b0` writes the flag
    // (`0x4603ba`), finds the slot live at `0x4603d6`, pauses it (`0x7a5ac0`) and returns
    // (`0x4603e0`), so the zone stream is not stopped and `[0x836400]` is never rearmed.
    if zone.lua_slot_live() {
        if let Some(h) = zone.lua_music.as_mut() {
            if suppressed {
                h.pause(mixer::declick());
                info!("Lua music: paused for a cinematic");
            } else {
                h.resume(mixer::declick());
                info!("Lua music: resumed");
            }
        }
        return;
    }
    if suppressed {
        if let Some(mut h) = zone.music.take() {
            h.stop(mixer::fade(0));
            info!("zone music: cut for a cinematic");
        }
        // The next track starts at position 0, behind this one's baseline.
        zone.music_watch.reset();
        zone.next_track_at = None;
    } else {
        zone.next_track_at = Some(now + CINEMATIC_MUSIC_RESUME_SECS);
        info!("zone music: resumes in {CINEMATIC_MUSIC_RESUME_SECS:.1}s");
    }
}

fn zone_music_row(cat: &AreaSoundCatalog, _id: u32) -> Option<&benilla_formats::ZoneMusicEntry> {
    cat.zone_music(_id)
}

/// When the next track starts after this one ends, the reference's `0x4601f0` (called only from
/// the natural-end reap `0x4600b6`): `now` plus the row's random per-phase silence interval, or
/// `now` under `SoundZoneMusicNoDelay` (`0x42c010`). That CVar removes only the same-zone gap; a
/// zone change is immediate either way (`0x460346`). Its `== 0 → now + 6000 ms` arm is not a cold
/// start: world entry never reaches this function.
fn next_track_time(
    zone: &mut ZoneAudio,
    cat: &AreaSoundCatalog,
    music_row: u32,
    phase: usize,
    now: f64,
    no_delay: bool,
) -> Option<f64> {
    if music_row == 0 {
        return None;
    }
    if no_delay {
        return Some(now);
    }
    let interval =
        zone_music_row(cat, music_row).map(|m| (m.silence_min[phase], m.silence_max[phase]));
    Some(match interval {
        Some((min, max)) => now + zone.silence_ms(min, max) as f64 / 1000.0,
        None => now + 6.0,
    })
}

/// Whether the music slot is already running `kit_id`, the reference's early-out at `0x460342`
/// (`cmp [0xb06cbc],eax; je`). A `Stopped` handle not yet reaped does not hold the slot.
fn slot_holds(music_kit: u32, kit_id: u32, slot: Option<kira::sound::PlaybackState>) -> bool {
    music_kit == kit_id && slot.is_some_and(|s| s != kira::sound::PlaybackState::Stopped)
}

/// Open a music kit on the music slot at full volume (`0x460240` → `0x7a5dc0`), replacing what
/// is there; whether a stream started.
fn start_music_stream(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    config: &SoundConfig,
    kit_id: u32,
) -> bool {
    // Nothing opens the slot during a cinematic, whoever asks: the reference's pump dies at its
    // first instruction (`0x460040`, `[0xb06cc8]`). Refusing here also keeps an intro from being
    // stamped as played.
    if config.music_suppressed {
        return false;
    }
    // Nor while a Lua track holds the override slot: the reference gates on the slot itself in
    // the pump (`0x460050`), the zone-music change `0x4602e0`, the enable toggle `0x460420`, the
    // play-by-id `0x460520` and `0x4603b0`'s pause leg.
    if zone.lua_slot_live() {
        return false;
    }
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return false;
    };
    let Some((path, kit_vol)) = kits.pick_stream(kit_id) else {
        return false;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(&path)
    };
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            warn!("zone music: {path} — {e:#}");
            return false;
        }
    };
    let start_amp = config.category_amp(SoundCategory::Music) * kit_vol;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(start_amp))) {
        Ok(h) => {
            info!("zone music: {path}");
            // One stream per slot: a dropped kira handle keeps playing, so the old one fades.
            if let Some(mut outgoing) = zone.music.replace(h) {
                outgoing.stop(mixer::fade(MUSIC_FADE_OUT_MS));
            }
            zone.music_kit = kit_id;
            zone.music_kit_vol = kit_vol;
            true
        }
        Err(e) => {
            warn!("zone music: {path} — {e:#}");
            false
        }
    }
}

/// The Lua music verb, the reference's `0x460450`: `PlayMusic` (`0x458720`) calls it with a name
/// and `StopMusic` (`0x458770`) with NULL, in this order:
///
/// 1. [`take_lua_music_slot`], the shared head and the whole NULL arm;
/// 2. the branch at `0x4604a0`;
/// 3. [`open_lua_music`], looping at full volume; a name that does not resolve returns here
///    (`0x4604bd`), leaving the zone track playing;
/// 4. only if the stream opened, the zone track fades out over 4.0 s (`0x4604d4`–`0x4604e8`).
///
/// `assets` and `path` are `None` for the NULL arm.
fn set_lua_music(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    config: &SoundConfig,
    path: Option<&str>,
    now: f64,
) {
    take_lua_music_slot(zone, now);
    let (Some(path), Some(assets)) = (path, assets) else {
        return;
    };
    // The reference re-reads its own slot (`0x4604cb`/`0x4604d2`) and the zone's (`0x4604da`).
    if open_lua_music(zone, out, assets, config, path) {
        // The same `0x7a5a10(4.0f)` a zone-music change makes.
        if let Some(mut h) = zone.music.take() {
            h.stop(mixer::fade(MUSIC_FADE_OUT_MS));
            info!("zone music: faded out under a Lua PlayMusic");
        }
    }
}

/// `0x460450`'s shared head: fade this slot out over 4.0 s (`0x460480`, so a second `PlayMusic`
/// crossfades), clear it (`0x460485`) and rearm the zone pump. It never touches the zone track:
/// `[0xb06cc4]` appears only past the NULL branch.
fn take_lua_music_slot(zone: &mut ZoneAudio, now: f64) {
    if let Some(mut h) = zone.lua_music.take() {
        h.stop(mixer::fade(MUSIC_FADE_OUT_MS));
        info!("Lua music: stopped");
    }
    zone.lua_music_path = None;
    zone.lua_music_watch.reset();
    zone.next_track_at = Some(now + LUA_MUSIC_SCHEDULE_SECS);
}

/// Open a Lua music stream on the override slot, by path and looping (`0x4604a7`–`0x4604bf`,
/// through `0x7a5620`'s looping arm); [`lua_music`] reopens here at the natural end.
///
/// Opened paused under a cinematic: the reference still opens the file and gates only the
/// deferred `_FSOUND_Stream_PlayEx` (`0x4604f2`/`0x4604f9`), starting it when music comes back.
fn open_lua_music(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    assets: &WorldAssets,
    config: &SoundConfig,
    path: &str,
) -> bool {
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return false;
    };
    // The chain, then the addon folder: an addon's own track has an `Interface\AddOns\…` path.
    let Some(bytes) = assets.read_file_or_loose(path) else {
        // Silent, as the reference's; the name is used verbatim (`0x458720` appends nothing).
        debug!("Lua music: {path} — not in the patch chain or the AddOns folder");
        return false;
    };
    let data = match mixer::stream_from_bytes(bytes) {
        Ok(d) => d,
        Err(e) => {
            debug!("Lua music: {path} — {e:#}");
            return false;
        }
    };
    let start_amp = config.category_amp(SoundCategory::Music) * LUA_MUSIC_VOLUME;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(start_amp))) {
        Ok(mut h) => {
            info!("Lua music: {path}");
            if config.music_suppressed {
                h.pause(mixer::fade(0));
                info!("Lua music: held — a cinematic has music disabled");
            }
            // Only the loop's reopen finds a stream here; the verb already faded the old one.
            if let Some(mut outgoing) = zone.lua_music.replace(h) {
                outgoing.stop(mixer::declick());
            }
            zone.lua_music_path = Some(path.to_owned());
            zone.lua_music_watch.reset();
            true
        }
        Err(e) => {
            warn!("Lua music: {path} — {e:#}");
            false
        }
    }
}

/// Drop the Lua stream on a teardown: a short declick and no schedule rearm. Idempotent.
fn drop_lua_music(zone: &mut ZoneAudio) {
    if let Some(mut h) = zone.lua_music.take() {
        h.stop(mixer::fade(250));
    }
    zone.lua_music_path = None;
    zone.lua_music_watch.reset();
}

/// Crossfade the looping ambience bed to a new kit (0 = stop) over `fade_ms`: the old bed fades
/// out on the backend while the new one starts silent under the per-frame envelope.
fn swap_ambience(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    config: &SoundConfig,
    kit_id: u32,
    fade_ms: u64,
    now: f64,
) {
    if kit_id == zone.ambience_kit {
        return;
    }
    if let Some(mut h) = zone.ambience.take() {
        h.stop(mixer::fade(fade_ms));
    }
    zone.ambience_kit = kit_id;
    zone.ambience_fade_in = None;
    if kit_id == 0 {
        return;
    }
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return;
    };
    let Some((path, kit_vol)) = kits.pick_stream(kit_id) else {
        return;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(&path)
    };
    // A static loop, not a stream (`mixer::loop_from_bytes`).
    let data = match bytes.and_then(mixer::loop_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            warn!("ambience: {path} — {e:#}");
            return;
        }
    };
    let start_amp = if fade_ms > 0 {
        0.0
    } else {
        config.category_amp(SoundCategory::Ambience) * kit_vol
    };
    match mixer_ref.play_2d(data.volume(mixer::amp_to_db(start_amp))) {
        Ok(h) => {
            info!("ambience: {path}");
            zone.ambience = Some(h);
            zone.ambience_kit_vol = kit_vol;
            if fade_ms > 0 {
                zone.ambience_fade_in = Some(FadeIn::new(now, fade_ms));
            }
        }
        Err(e) => warn!("ambience: {path} — {e:#}"),
    }
}

/// The Lua music slot: this frame's queued verbs, the loop, the slider and the watch.
/// Unconditional: the stream is the caller's, not the world soundscape's.
fn lua_music(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut zone: NonSendMut<ZoneAudio>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
    // The clock `zone_audio` reads `next_track_at` against.
    time: Res<Time>,
    // Wall time for the stream watch.
    real: Res<Time<bevy::time::Real>>,
) {
    let zone = &mut *zone;
    let now = time.elapsed_secs_f64();
    let requests = script.map(|mut s| s.take_music()).unwrap_or_default();
    for req in requests {
        match req {
            // The NULL arm always lands, and moves the pump's clock even from cold.
            benilla_ui::script::MusicRequest::Stop => {
                set_lua_music(zone, &mut out, None, &config, None, now)
            }
            benilla_ui::script::MusicRequest::Play(path) => {
                // Under the loading cover a start is dropped, not deferred: the reference's
                // blocking load plays nothing.
                let Some(assets) = assets.as_deref().filter(|_| !config.world_hold) else {
                    debug!("Lua music: {path} dropped — no assets, or the loading cover is up");
                    continue;
                };
                set_lua_music(zone, &mut out, Some(assets), &config, Some(&path), now);
            }
        }
    }

    // The loop: `SetLoopCount(-1)` in the reference, an explicit restart here.
    if zone
        .lua_music
        .as_ref()
        .is_some_and(|h| h.state() == kira::sound::PlaybackState::Stopped)
    {
        zone.lua_music = None;
        if let (Some(path), Some(assets)) = (zone.lua_music_path.clone(), assets.as_deref()) {
            open_lua_music(zone, &mut out, assets, &config, &path);
        }
    }

    // The Music slider is live here too (`0x7a6660(ecx=2)`, the MusicVolume re-apply walker).
    if let Some(h) = zone.lua_music.as_mut() {
        h.set_volume(
            mixer::amp_to_db(config.category_amp(SoundCategory::Music) * LUA_MUSIC_VOLUME),
            mixer::glide(),
        );
    }
    if let Some(h) = zone.lua_music.as_ref() {
        zone.lua_music_watch.feed(h, f64::from(real.delta_secs()));
    }
}

/// Server-pushed sounds (`SMSG_PLAY_SOUND`/`PLAY_MUSIC`/`PLAY_OBJECT_SOUND`): kits go through
/// the kit player, music takes the zone music slot until it ends.
fn server_sounds(
    mut msgs: MessageReader<ServerSoundMessage>,
    mut zone: NonSendMut<ZoneAudio>,
    mut out: NonSendMut<SoundOutput>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    transforms: Query<&Transform, Without<Camera3d>>,
) {
    if msgs.is_empty() {
        return;
    }
    // Under the cover a push is dropped, not deferred; recurring pushes re-arrive after it.
    if config.world_hold {
        msgs.clear();
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for m in msgs.read() {
        match m.kind {
            ServerSoundKind::Music => {
                // A push for the track already on the slot is a no-op.
                if slot_holds(
                    zone.music_kit,
                    m.sound_id,
                    zone.music.as_ref().map(|h| h.state()),
                ) {
                    continue;
                }
                start_music_stream(&mut zone, &mut out, &mut kits, &assets, &config, m.sound_id);
            }
            ServerSoundKind::Sound2d | ServerSoundKind::ObjectSound => {
                // At the source entity when it is streamed to us; otherwise flat.
                let pos = m
                    .source
                    .and_then(|e| transforms.get(e).ok())
                    .map(|t| t.translation);
                // An object sound registers on its unit (the reference's `AISOUNDDESC` pool,
                // `0x458fb0` → `0x459120`), so the vocal gates (`0x4591f0`) keep the creature's
                // grunts off a scripted voice line.
                let source = match m.kind {
                    ServerSoundKind::ObjectSound => m.source,
                    _ => None,
                };
                let latch = if source.is_some() {
                    Latch::ObjectSound
                } else {
                    Latch::None
                };
                if let Err(e) = play_kit_ext(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener,
                    KitRef::Id(m.sound_id),
                    pos,
                    SoundCategory::Sfx,
                    PlayExtras {
                        source,
                        latch,
                        ..default()
                    },
                ) {
                    warn!("server sound {}: {e:#}", m.sound_id);
                }
            }
        }
    }
}

/// `OnExit(InWorld)`: the world soundscape dies with the world. The reference destroys its world
/// sound state outright; here the beds stop on a 250 ms declick, not the musical fades.
/// `intro_last` and the RNG survive a relog, as process-global statics do in the reference.
fn leave_world(mut zone: NonSendMut<ZoneAudio>) {
    stop_world_soundscape(&mut zone, "left world");
    // The Lua slot only here: a loading cover leaves the caller's track playing.
    drop_lua_music(&mut zone);
}

/// Stop the beds and reset the scheduler to cold, on leaving the world or under the loading
/// cover. Idempotent.
fn stop_world_soundscape(zone: &mut ZoneAudio, reason: &str) {
    // Fast enough to read as a cut, long enough not to pop.
    const WORLD_TEARDOWN_FADE_MS: u64 = 250;
    let had = zone.music.is_some() || zone.ambience.is_some();
    if let Some(mut h) = zone.music.take() {
        h.stop(mixer::fade(WORLD_TEARDOWN_FADE_MS));
    }
    // The next track starts at position 0, behind this one's baseline.
    zone.music_watch.reset();
    if let Some(mut h) = zone.ambience.take() {
        h.stop(mixer::fade(WORLD_TEARDOWN_FADE_MS));
    }
    zone.zone_music = 0;
    zone.music_kit = 0;
    zone.music_kit_vol = 1.0;
    zone.next_track_at = None;
    zone.ambience_kit = 0;
    zone.ambience_kit_vol = 1.0;
    zone.ambience_fade_in = None;
    zone.area = None;
    zone.interior = None;
    zone.was_underwater = false;
    // Else the next entry sees a falling edge left from this session's cinematic.
    zone.music_suppressed = false;
    if had {
        info!("sound: world soundscape stopped ({reason})");
    }
}

/// Report this module's live stream voices into the global budget, recounted from the held
/// handles every frame; a released outgoing ambience bed is not counted.
fn report_stream_voices(zone: NonSend<ZoneAudio>, mut out: NonSendMut<super::SoundOutput>) {
    let live = |s: kira::sound::PlaybackState| s != kira::sound::PlaybackState::Stopped;
    out.zone_streams = usize::from(zone.music.as_ref().is_some_and(|h| live(h.state())))
        + usize::from(zone.lua_music.as_ref().is_some_and(|h| live(h.state())))
        + usize::from(zone.ambience.as_ref().is_some_and(|h| live(h.state())));
}

pub(super) fn plugin(app: &mut App) {
    app.insert_non_send_resource(ZoneAudio::default())
        .add_systems(Startup, load_area_sounds.after(AssetSet::Open))
        .add_systems(
            Update,
            (server_sounds, zone_audio)
                .chain()
                .run_if(super::world_audio_live)
                .in_set(WorldStage::Present),
        )
        // Before the zone scheduler, so this frame's `PlayMusic` already holds the slot, and
        // after the VM tick, whose output it drains.
        .add_systems(
            Update,
            lua_music
                .before(server_sounds)
                .after(crate::ui_script::UiInput)
                // After the dev mute chord, so the gain feed hears it the same frame.
                .after(super::toggle_mute)
                .in_set(WorldStage::Present),
        )
        // Unconditional, so the count falls to zero when the soundscape stops.
        .add_systems(Update, report_stream_voices)
        .add_systems(
            OnExit(crate::char_select::ClientState::InWorld),
            leave_world,
        );
}

#[cfg(test)]
mod tests {
    use super::{
        apply_music_suppression, slot_holds, take_lua_music_slot, ZoneAudio,
        CINEMATIC_MUSIC_RESUME_SECS, LUA_MUSIC_SCHEDULE_SECS,
    };
    use kira::sound::PlaybackState;

    #[test]
    fn a_cinematic_cuts_the_music_and_brings_it_back_three_seconds_later() {
        let mut zone = ZoneAudio {
            zone_music: 42,
            next_track_at: Some(100.0),
            ..ZoneAudio::default()
        };

        apply_music_suppression(&mut zone, true, 10.0);
        assert!(zone.music_suppressed);
        assert_eq!(
            zone.next_track_at, None,
            "a suppressed pump must schedule nothing"
        );

        apply_music_suppression(&mut zone, true, 90.0);
        assert_eq!(zone.next_track_at, None);

        // Up: +3.000 s, and the zone row is remembered.
        apply_music_suppression(&mut zone, false, 112.5);
        assert!(!zone.music_suppressed);
        assert_eq!(zone.next_track_at, Some(115.5));
        assert_eq!(zone.zone_music, 42);
    }

    #[test]
    fn a_cinematic_borrows_a_lua_music_track_and_leaves_the_schedule_alone() {
        let mut zone = ZoneAudio {
            zone_music: 42,
            next_track_at: Some(100.0),
            lua_music_path: Some("Sound\\Music\\x.mp3".into()),
            ..ZoneAudio::default()
        };

        apply_music_suppression(&mut zone, true, 10.0);
        assert!(
            zone.music_suppressed,
            "the flag is written before the early return, so the pump dies either way"
        );
        assert_eq!(
            zone.next_track_at,
            Some(100.0),
            "the zone schedule is neither cleared nor rearmed"
        );

        apply_music_suppression(&mut zone, false, 112.5);
        assert!(!zone.music_suppressed);
        assert_eq!(
            zone.next_track_at,
            Some(100.0),
            "the +3.000 s resume belongs to the other edge — `[0x836400]` is never rearmed here"
        );
        assert!(
            zone.lua_slot_live(),
            "and the track is still the caller's, across both edges"
        );
    }

    #[test]
    fn the_resume_delay_is_the_references_own_third_number() {
        assert!((CINEMATIC_MUSIC_RESUME_SECS - 3.0).abs() < f64::EPSILON);
    }

    /// `StopMusic()` is `PlayMusic(NULL)` (`0x458770` → `0x460450`): the pump write is
    /// unconditional, and the zone track is not what it stops.
    #[test]
    fn the_lua_music_verb_rearms_the_zone_pump_at_six_seconds_even_from_cold() {
        assert!((LUA_MUSIC_SCHEDULE_SECS - 6.0).abs() < f64::EPSILON);
        const { assert!(LUA_MUSIC_SCHEDULE_SECS != CINEMATIC_MUSIC_RESUME_SECS) };

        // From cold: nothing on the slot, a zone track due this tick.
        let mut zone = ZoneAudio {
            zone_music: 42,
            next_track_at: Some(10.0),
            ..ZoneAudio::default()
        };
        take_lua_music_slot(&mut zone, 10.0);
        assert_eq!(
            zone.next_track_at,
            Some(16.0),
            "a stop from cold still moves the pump — and clobbers a start that was due now"
        );
        assert!(!zone.lua_slot_live());

        let mut zone = ZoneAudio {
            zone_music: 42,
            music_kit: 8440,
            lua_music_path: Some("Sound\\Music\\x.mp3".into()),
            ..ZoneAudio::default()
        };
        take_lua_music_slot(&mut zone, 100.0);
        assert!(!zone.lua_slot_live(), "the caller's track is over");
        assert_eq!(zone.next_track_at, Some(106.0));
        assert_eq!(
            (zone.music_kit, zone.zone_music),
            (8440, 42),
            "zone music is not what this verb stops"
        );
    }

    /// 8440 is the Darkmoon Faire music the server re-pushes every 5 s.
    #[test]
    fn repeat_music_push_only_no_ops_while_the_track_plays() {
        assert!(slot_holds(8440, 8440, Some(PlaybackState::Playing)));
        assert!(!slot_holds(8440, 8440, Some(PlaybackState::Stopped)));
        assert!(!slot_holds(8440, 8440, None));
        assert!(!slot_holds(8440, 4123, Some(PlaybackState::Playing)));
    }
}
