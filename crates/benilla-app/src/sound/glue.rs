//! Glue-screen audio: the clicks and the title theme.
//!
//! - Clicks: the char select and create screens emit [`GlueSound`] with the SoundEntries kit names
//!   the 1.12 GlueXML plays, drained into the kit player as 2D SFX.
//! - Music: `GlueParent.lua`'s `CurrentGlueMusic`, streamed from the login screen and kept across
//!   the glue screens. Entering the world does not end it (`0x46c258`): it plays through the map
//!   load and fades over 3.0 s once the load drains, still behind the loading screen.

use bevy::prelude::*;

use crate::char_select::ClientState;
use benilla_assets::{LockRecover, WorldAssets};

use super::kit::{self, KitRef, SoundCategory, SoundKits};
use super::{mixer, SoundConfig, SoundOutput};

const GLUE_MUSIC: &str = "Sound\\Music\\GlueScreenMusic\\wow_main_theme.mp3";

/// The theme's own volume, multiplied into the Music slider: `0x45aeb0` starts the glue stream at
/// 0.8 (`0x3f4ccccd`) on the MusicVolume category (flag word 2, bit `0x2` = `[0x87cef8]`).
const GLUE_MUSIC_VOLUME: f32 = 0.8;

/// The stop-fade armed when the world's load drains: 3.0 s (`[0x803248]`, handed to `0x45b050` by
/// `0x45aeb0`'s NULL arm), linear in amplitude (`0x7a5a50`). `StopGlueMusic`'s 1.0 s is a
/// movie-screen call the world entry never makes.
const GLUE_MUSIC_FADE_OUT_MS: u64 = 3000;

/// A glue-screen `PlaySound`: the SoundEntries kit name the 1.12 GlueXML plays for this click.
#[derive(Message)]
pub(crate) struct GlueSound(pub(crate) &'static str);

/// The held glue-music stream and its starvation watch. The handle is kept through the stop-fade
/// so the watch still sees the stream during the world-entry load burst.
struct GlueMusic {
    handle: Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>,
    watch: mixer::StreamWatch,
    /// The stop-fade is running; the per-frame slider feed stands off so it cannot undo the ramp.
    fading: bool,
}

impl Default for GlueMusic {
    fn default() -> Self {
        Self {
            handle: None,
            watch: mixer::StreamWatch::new("glue music"),
            fading: false,
        }
    }
}

/// Drain glue clicks into the kit player as 2D SFX, by kit name.
fn play_glue_sounds(
    mut msgs: MessageReader<GlueSound>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        msgs.clear();
        return;
    };
    for msg in msgs.read() {
        if let Err(e) = kit::play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            Vec3::ZERO,
            KitRef::Name(msg.0),
            None,
            SoundCategory::Sfx,
        ) {
            debug!("sound(glue): {} — {e:#}", msg.0);
        }
    }
}

/// On the glue screens, start the title theme if it is not up. Per-frame, not on the state edge:
/// the app boots into the glue before the mixer and chain exist, so the first `OnEnter` is early.
fn start_glue_music(
    mut music: NonSendMut<GlueMusic>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
) {
    if music.handle.is_some() && !music.fading {
        return; // still playing (select ⇄ create re-enters CharSelect)
    }
    // A theme mid-fade restarts at once: the enter-world stop clears the cached track name
    // (`0x45afec`), so the next `PlayGlueMusic` opens a fresh stream. The old handle drops below
    // with its ramp armed and finishes fading on the backend.
    let (Some(assets), Some(mixer_ref)) = (assets, out.mixer.as_mut()) else {
        return;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(GLUE_MUSIC)
    };
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            debug!("glue music: {GLUE_MUSIC} — {e:#}");
            return;
        }
    };
    let amp = config.category_amp(SoundCategory::Music) * GLUE_MUSIC_VOLUME;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(amp))) {
        Ok(h) => {
            info!("glue music: {GLUE_MUSIC}");
            music.handle = Some(h);
            music.fading = false;
            music.watch.reset();
        }
        Err(e) => debug!("glue music: {e:#}"),
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
/// On the glue screens [`start_glue_music`] then restarts it; in the world it runs out and stays
/// out.
fn watch_glue_music(
    mut music: NonSendMut<GlueMusic>,
    // Wall time: the watch compares against the stream's own position, and the paced virtual
    // clock is snapped and capped by `frame_pace`.
    time: Res<Time<bevy::time::Real>>,
    config: Res<SoundConfig>,
) {
    let music = &mut *music;
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

/// Report the glue theme's voice into the global budget; it plays into `InWorld` too.
fn report_stream_voices(music: NonSend<GlueMusic>, mut out: NonSendMut<super::SoundOutput>) {
    out.glue_streams = usize::from(
        music
            .handle
            .as_ref()
            .is_some_and(|h| h.state() != kira::sound::PlaybackState::Stopped),
    );
}

pub(super) fn plugin(app: &mut App) {
    app.add_message::<GlueSound>()
        .insert_non_send_resource(GlueMusic::default())
        .add_systems(
            Update,
            (
                play_glue_sounds,
                // The theme starts at the login screen (`AccountLogin_OnShow` sets the same
                // `wow_main_theme`) and keeps across the glue screens.
                start_glue_music
                    .run_if(in_state(ClientState::Login).or(in_state(ClientState::CharSelect))),
                // No run condition: the theme plays into `InWorld`, and the entry load burst is
                // the window the watch exists for.
                hand_off_glue_music,
                watch_glue_music
                    .after(start_glue_music)
                    .after(hand_off_glue_music),
                report_stream_voices,
            ),
        );
}
