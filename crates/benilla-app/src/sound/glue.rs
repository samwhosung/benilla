//! Glue-screen music. The stock GlueXML's clicks are its own `PlaySound` calls.
//!
//! - Music: `GlueParent.lua`'s `CurrentGlueMusic`, which the stock screens' `PlayGlueMusic`
//!   streams from the login screen, kept across the glue screens. Entering the world does not end it (`0x46c258`): it plays through the map
//!   load and fades over 3.0 s once the load drains, still behind the loading screen.
//! - Credits music: `PlayCreditsMusic` (`0x46cef0` → `0x45b000`) swaps the theme for the seven
//!   city intro-moment tracks, played in turn ([`CREDITS_TRACKS`]).

use bevy::prelude::*;

use crate::client_state::ClientState;
use benilla_assets::WorldAssets;

use super::kit::SoundCategory;
use super::{mixer, SoundConfig, SoundOutput};

/// The theme's own volume, multiplied into the Music slider: `0x45aeb0` starts the glue stream at
/// 0.8 (`0x3f4ccccd`) on the MusicVolume category (flag word 2, bit `0x2` = `[0x87cef8]`).
const GLUE_MUSIC_VOLUME: f32 = 0.8;

/// The stop-fade armed when the world's load drains: 3.0 s (`[0x803248]`, handed to `0x45b050` by
/// `0x45aeb0`'s NULL arm), linear in amplitude (`0x7a5a50`). `StopGlueMusic`'s 1.0 s is a
/// movie-screen call the world entry never makes.
const GLUE_MUSIC_FADE_OUT_MS: u64 = 3000;

/// The stock glue screens' music verbs.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub(crate) enum GlueMusicRequest {
    /// `PlayGlueMusic(file)` (`0x45aeb0`): the same file still streaming plays on; another stops
    /// the old one over 3.0 s and starts at 0.8; an empty name restarts the remembered file if
    /// nothing streams.
    Play(String),
    /// `StopGlueMusic()` (`0x46cf00`): a 1.0 s fade (`0x45b050(1.0)`), the file still remembered.
    Stop,
    /// `PlayCreditsMusic()` (`0x46cef0` → `0x45b000`): the theme fades over 3.0 s
    /// (`0x45b050([0x803248])`), the city playlist's old stream over 1.0 s (`0x45adb0(0)`), and
    /// with `EnableMusic` on the playlist starts at its first track (`0x45ae50(0)`).
    Credits,
}

/// The playlist `0x45ae50` opens from, `[0x836028 + 4 * i]`: the seven city intro-moment tracks.
const CREDITS_TRACKS: [&str; 7] = [
    "Sound\\Music\\CityMusic\\Stormwind\\Stormwind_intro-moment.mp3",
    "Sound\\Music\\CityMusic\\Orgrimmar\\orgrimmar_intro-moment.mp3",
    "Sound\\Music\\CityMusic\\Darnassus\\Darnassus Intro.mp3",
    "Sound\\Music\\CityMusic\\Undercity\\undercityintro-moment.mp3",
    "Sound\\Music\\CityMusic\\Ironforge\\IronForge Intro.mp3",
    "Sound\\Music\\CityMusic\\Thunderbluff\\Thunderbluff Intro.mp3",
    "Sound\\Music\\CityMusic\\Ironforge\\tinkertownintro_moment.mp3",
];

/// The playlist's own volume before the Music slider: `0x45ae7c` hands `1.0f` to `0x7a5dc0`.
const CREDITS_VOLUME: f32 = 1.0;

/// `0x45adb0(0)`'s fade of the playlist's stream, `0x3f800000` pushed at `0x45add9`.
const CREDITS_FADE_OUT_MS: u64 = 1000;

/// Which track the playlist is on: started at the first (`0x45b032`), moved to the next, past the
/// last to the first, once a track ends (`0x45ae00` → `0x45ae50(index + 1)`, `index % 7`).
#[derive(Debug, Default, PartialEq, Eq)]
struct CreditsPlaylist {
    /// The track now playing, or `None` while the playlist is off.
    index: Option<usize>,
}

impl CreditsPlaylist {
    /// `0x45ae50(0)`: the first track.
    fn start(&mut self) -> &'static str {
        self.index = Some(0);
        CREDITS_TRACKS[0]
    }

    /// The track that ended (`0x45ae00`) gives way to the next, wrapping at the seventh.
    fn advance(&mut self) -> Option<&'static str> {
        let next = (self.index? + 1) % CREDITS_TRACKS.len();
        self.index = Some(next);
        Some(CREDITS_TRACKS[next])
    }

    /// The playlist is off: stopped (`0x45adb0`), or a track failed to open (`0x45ae33`).
    fn stop(&mut self) {
        self.index = None;
    }
}

/// `StopGlueMusic`'s fade, `0x3f800000` pushed at `0x46cf00`.
const STOP_GLUE_MUSIC_FADE_MS: u64 = 1000;

/// The held glue-music stream and its starvation watch. The handle is kept through the stop-fade
/// so the watch still sees the stream during the world-entry load burst.
struct GlueMusic {
    handle: Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>,
    /// The file `PlayGlueMusic` last started (`0xb060a0`), the stock screens' theme.
    track: String,
    watch: mixer::StreamWatch,
    /// The stop-fade is running; the per-frame slider feed stands off so it cannot undo the ramp.
    fading: bool,
    /// The credits playlist (`[0xb06098]`), a stream of its own beside the theme's.
    credits: CreditsPlaylist,
    credits_handle: Option<GlueStream>,
}

type GlueStream = mixer::StreamingSoundHandle<kira::sound::FromFileError>;

impl Default for GlueMusic {
    fn default() -> Self {
        Self {
            handle: None,
            track: String::new(),
            watch: mixer::StreamWatch::new("glue music"),
            fading: false,
            credits: CreditsPlaylist::default(),
            credits_handle: None,
        }
    }
}

/// Open `file` off the loose files or the chain as a stream at `amp` (the category's slider
/// already multiplied in).
fn open_stream(
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    file: &str,
    amp: f32,
) -> Option<GlueStream> {
    let (Some(assets), Some(mixer_ref)) = (assets, out.mixer.as_mut()) else {
        return None;
    };
    // Every file FMOD opens is a loose-flagged open (`0x7a8860`).
    let bytes = assets
        .read_file_or_loose(file)
        .ok_or_else(|| anyhow::anyhow!("not in the loose files or the patch chain"));
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            debug!("glue music: {file} — {e:#}");
            return None;
        }
    };
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(amp))) {
        Ok(h) => {
            info!("glue music: {file}");
            Some(h)
        }
        Err(e) => {
            debug!("glue music: {e:#}");
            None
        }
    }
}

/// Open `file` off the chain as the held theme at the Music slider times 0.8.
fn open_glue_music(
    music: &mut GlueMusic,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    config: &SoundConfig,
    file: &str,
) {
    let amp = config.category_amp(SoundCategory::Music) * GLUE_MUSIC_VOLUME;
    if let Some(h) = open_stream(out, assets, file, amp) {
        music.handle = Some(h);
        music.fading = false;
        music.watch.reset();
    }
}

/// `0x45adb0(0)`: the playlist's stream fades over 1.0 s and the playlist is off. The handle
/// drops with its ramp armed, so the stream finishes fading on the backend.
fn stop_credits(music: &mut GlueMusic) {
    if let Some(mut h) = music.credits_handle.take() {
        h.stop(mixer::fade(CREDITS_FADE_OUT_MS));
    }
    music.credits.stop();
}

/// Open the playlist's track `file` at the Music slider times 1.0; a track that will not open
/// ends the playlist (`0x45ae33`).
fn open_credits_track(
    music: &mut GlueMusic,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    config: &SoundConfig,
    file: &str,
) {
    let amp = config.category_amp(SoundCategory::Music) * CREDITS_VOLUME;
    match open_stream(out, assets, file, amp) {
        Some(h) => music.credits_handle = Some(h),
        None => music.credits.stop(),
    }
}

/// Stop the held theme over `fade_ms`; the handle drops once the watch sees it stopped.
fn fade_glue_music(music: &mut GlueMusic, fade_ms: u64) {
    if let Some(h) = music.handle.as_mut() {
        h.stop(mixer::fade(fade_ms));
        music.fading = true;
    }
}

/// What a `PlayGlueMusic(file)` does against the remembered file and whether a stream is live
/// (`0x45aef9`-`0x45afdb`).
#[derive(Debug, PartialEq, Eq)]
enum PlayAction {
    Nothing,
    /// Start the remembered file again (an empty name, nothing streaming).
    Restart,
    /// Stop what streams over 3.0 s, remember `file` and start it.
    Switch,
}

fn play_action(file: &str, remembered: &str, streaming: bool) -> PlayAction {
    if file.is_empty() {
        if !streaming && !remembered.is_empty() {
            PlayAction::Restart
        } else {
            PlayAction::Nothing
        }
    } else if !remembered.is_empty() && file == remembered && streaming {
        PlayAction::Nothing
    } else {
        PlayAction::Switch
    }
}

/// The stock glue screens' music verbs.
fn drive_glue_music_requests(
    mut requests: MessageReader<GlueMusicRequest>,
    mut music: NonSendMut<GlueMusic>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
) {
    for request in requests.read() {
        match request {
            GlueMusicRequest::Play(file) => {
                // `0x45aeb0` stops the playlist before anything else (`0x45aeb5`).
                stop_credits(&mut music);
                let streaming = music.handle.is_some() && !music.fading;
                match play_action(file, &music.track, streaming) {
                    PlayAction::Nothing => {}
                    PlayAction::Restart => {
                        let track = music.track.clone();
                        open_glue_music(&mut music, &mut out, assets.as_deref(), &config, &track);
                    }
                    PlayAction::Switch => {
                        fade_glue_music(&mut music, GLUE_MUSIC_FADE_OUT_MS);
                        music.track = file.clone();
                        open_glue_music(&mut music, &mut out, assets.as_deref(), &config, file);
                    }
                }
            }
            GlueMusicRequest::Stop => fade_glue_music(&mut music, STOP_GLUE_MUSIC_FADE_MS),
            GlueMusicRequest::Credits => {
                fade_glue_music(&mut music, GLUE_MUSIC_FADE_OUT_MS);
                stop_credits(&mut music);
                // `EnableMusic` off (`0x45b01c`) ends it there: nothing plays.
                if config.music_enabled {
                    let first = music.credits.start();
                    open_credits_track(&mut music, &mut out, assets.as_deref(), &config, first);
                }
            }
        }
    }
}

/// Arm the theme's 3.0 s fade when the world's load drains.
///
/// The reference's trigger: `CGlueMgr::Update` state 8 spins each frame after entry
/// (`[0xb41d94] == 1`) while the AsyncFileLoader (`0x443e20`) has reads outstanding; the frame it
/// drains it calls `0x45aeb0(NULL)` → `0x45b050(3.0f)`, then sends `CMSG_PLAYER_LOGIN`. benilla's
/// analogue is `world_hold`'s falling edge ([`crate::loading_screen`]), acted on only in the world
/// so the logout blackout's drop does not fade the theme the glue just restarted.
fn hand_off_glue_music(
    config: Res<SoundConfig>,
    state: Res<State<ClientState>>,
    mut music: NonSendMut<GlueMusic>,
    mut was_covered: Local<bool>,
) {
    let covered = config.world_hold;
    let fell = std::mem::replace(&mut *was_covered, covered) && !covered;
    if !fell || *state.get() != ClientState::InWorld {
        return;
    }
    if let Some(h) = music.handle.as_mut() {
        info!("glue music: the world's load drained — {GLUE_MUSIC_FADE_OUT_MS} ms fade");
        h.stop(mixer::fade(GLUE_MUSIC_FADE_OUT_MS));
        music.fading = true;
    }
}

/// The starvation watch over the held theme, and the handle's drop once it reaches `Stopped`.
/// On the glue screens the next `PlayGlueMusic` restarts it; in the world it runs out and stays
/// out. The credits playlist's callback rides along ([`watch_credits`]).
fn watch_glue_music(
    mut music: NonSendMut<GlueMusic>,
    // Wall time: the watch compares against the stream's own position, and the paced virtual
    // clock is snapped and capped by `frame_pace`.
    time: Res<Time<bevy::time::Real>>,
    config: Res<SoundConfig>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
) {
    let music = &mut *music;
    watch_credits(music, &mut out, assets.as_deref(), &config);
    // The Music slider rescales the playing theme in place (the `MusicVolume` re-apply walker
    // `0x7a6660(ecx=2)` matches the glue stream), except while the stop-fade runs.
    if !music.fading {
        if let Some(h) = music.handle.as_mut() {
            h.set_volume(
                mixer::amp_to_db(config.category_amp(SoundCategory::Music) * GLUE_MUSIC_VOLUME),
                mixer::glide(),
            );
        }
    }
    let Some(h) = music.handle.as_ref() else {
        return;
    };
    if h.state() == kira::sound::PlaybackState::Stopped {
        // Logged: the theme outlives world entry, and nothing else reports when it stops.
        info!("glue music: stream ended");
        music.handle = None;
        music.fading = false;
        music.watch.reset();
        return;
    }
    music.watch.feed(h, f64::from(time.delta_secs()));
}

/// The playlist's per-frame callback (`0x45ae00`): the Music slider rescales the playing track,
/// and a track that has run out gives way to the next.
fn watch_credits(
    music: &mut GlueMusic,
    out: &mut SoundOutput,
    assets: Option<&WorldAssets>,
    config: &SoundConfig,
) {
    let Some(h) = music.credits_handle.as_mut() else {
        return;
    };
    if h.state() != kira::sound::PlaybackState::Stopped {
        h.set_volume(
            mixer::amp_to_db(config.category_amp(SoundCategory::Music) * CREDITS_VOLUME),
            mixer::glide(),
        );
        return;
    }
    music.credits_handle = None;
    if let Some(next) = music.credits.advance() {
        open_credits_track(music, out, assets, config, next);
    }
}

/// Report the glue theme's voice and the playlist's into the global budget; the theme plays into
/// `InWorld` too.
fn report_stream_voices(music: NonSend<GlueMusic>, mut out: NonSendMut<super::SoundOutput>) {
    let live = |h: &Option<GlueStream>| {
        h.as_ref()
            .is_some_and(|h| h.state() != kira::sound::PlaybackState::Stopped)
    };
    out.glue_streams = usize::from(live(&music.handle)) + usize::from(live(&music.credits_handle));
}

pub(super) fn plugin(app: &mut App) {
    app.add_message::<GlueMusicRequest>()
        .insert_non_send_resource(GlueMusic::default())
        .add_systems(
            Update,
            (
                // The stock screens' `PlayGlueMusic`/`StopGlueMusic` drive it.
                drive_glue_music_requests
                    .after(crate::ui_script::glue_mgr::drive_glue_mgr)
                    .before(watch_glue_music),
                // No run condition: the theme plays into `InWorld`, and the entry load burst is
                // the window the watch exists for.
                hand_off_glue_music,
                watch_glue_music.after(hand_off_glue_music),
                report_stream_voices,
            ),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x45ae50(0)` opens the first of the seven tracks and `0x45ae00` steps `index % 7` on each
    /// end, so the eighth play is the first again; a stopped playlist steps nowhere.
    #[test]
    fn the_credits_playlist_starts_first_and_wraps_after_the_seventh() {
        let mut list = CreditsPlaylist::default();
        assert_eq!(list.advance(), None);
        assert_eq!(list.start(), CREDITS_TRACKS[0]);
        let order: Vec<_> = (0..7).map(|_| list.advance().unwrap()).collect();
        let expect: Vec<_> = (1..=7).map(|i| CREDITS_TRACKS[i % 7]).collect();
        assert_eq!(order, expect);
        assert_eq!(list.index, Some(0));
        list.stop();
        assert_eq!(list.advance(), None);
        assert_eq!(
            list.start(),
            CREDITS_TRACKS[0],
            "a restart is the first track again"
        );
    }

    /// The seven names in `[0x836028]` order: Stormwind, Orgrimmar, Darnassus, Undercity,
    /// Ironforge, Thunder Bluff, then Tinker Town.
    #[test]
    fn the_credits_tracks_are_the_seven_city_intro_moments() {
        let cities = [
            "Stormwind",
            "Orgrimmar",
            "Darnassus",
            "Undercity",
            "Ironforge",
            "Thunderbluff",
        ];
        for (track, city) in CREDITS_TRACKS.iter().zip(cities) {
            assert!(
                track.starts_with(&format!("Sound\\Music\\CityMusic\\{city}\\")),
                "{track}"
            );
        }
        assert!(CREDITS_TRACKS[6].ends_with("tinkertownintro_moment.mp3"));
    }

    /// `0x45aeb0`'s three legs: an empty name restarts the remembered file only when nothing
    /// streams; the remembered file still streaming plays on; anything else switches.
    #[test]
    fn play_glue_music_restarts_keeps_or_switches() {
        let theme = "Sound\\Music\\GlueScreenMusic\\wow_main_theme.mp3";
        assert_eq!(play_action("", theme, false), PlayAction::Restart);
        assert_eq!(play_action("", theme, true), PlayAction::Nothing);
        assert_eq!(play_action("", "", false), PlayAction::Nothing);
        assert_eq!(play_action(theme, theme, true), PlayAction::Nothing);
        assert_eq!(play_action(theme, theme, false), PlayAction::Switch);
        assert_eq!(play_action(theme, "", false), PlayAction::Switch);
        assert_eq!(play_action("other.mp3", theme, true), PlayAction::Switch);
    }
}
