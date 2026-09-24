//! Glue-screen audio (decision 0423's polish pass) — the two sounds of the pre-world screens:
//!
//! - **Clicks**: the char select/create screens emit [`GlueSound`] messages naming the SoundEntries
//!   kits the 1.12 GlueXML plays verbatim (`gsCharacterCreationClass`, `gsCharacterSelectionEnterWorld`,
//!   …); [`play_glue_sounds`] drains them into the kit player as 2D SFX — the same shape as
//!   [`super::ui`]'s `PlaySound` seam, but from the glue screens (which have no Lua VM).
//! - **Music**: the glue title theme (`GlueParent.lua`'s `CurrentGlueMusic`,
//!   `Sound\Music\GlueScreenMusic\wow_main_theme.mp3`), streamed from the login screen on
//!   (decision 0539) and kept across the glue screens (the ref keeps it through select ⇄ create).
//!   **The click into the world does not end it** (1550/1553, `0x46c258`): it plays unbroken
//!   through the whole map load, and the stop is armed by the *load draining* — a 3.0 s fade, still
//!   behind the loading screen. Re-entering the glue after a logout starts it again.

use bevy::prelude::*;

use crate::char_select::ClientState;
use benilla_assets::{LockRecover, WorldAssets};

use super::kit::{self, KitRef, SoundCategory, SoundKits};
use super::{mixer, SoundConfig, SoundOutput};

/// The glue music file — a frozen fact of the 1.12 GlueXML (`GlueParent.lua`, `CurrentGlueMusic`).
const GLUE_MUSIC: &str = "Sound\\Music\\GlueScreenMusic\\wow_main_theme.mp3";

/// The theme's own volume, under the Music slider: **0.8** — `0x45aeb0` starts the glue stream at
/// `0x3f4ccccd` on both its arms, where the city-intro playlist and the in-world Lua `PlayMusic`
/// use 1.0f. It is a plain scalar on the MusicVolume
/// category (the stream's flag word is 2, bit `0x2` = `[0x87cef8]`), exactly like a zone track's
/// kit volume, so it multiplies rather than replaces the slider.
const GLUE_MUSIC_VOLUME: f32 = 0.8;

/// The stop-fade armed when the world's load drains: **3.0 s**, `[0x803248]` in `.rdata`
/// (`0x40400000`) — the constant `0x45aeb0`'s NULL arm hands `0x45b050`, and the fade is **linear
/// in amplitude**, not in dB (`0x7a5a50` ramps FMOD's 0–255 level). Not 1.0 s: that is
/// `StopGlueMusic`'s literal, and `StopGlueMusic` is a `"movie"`-screen call the world entry never
/// makes.
const GLUE_MUSIC_FADE_OUT_MS: u64 = 3000;

/// A glue-screen `PlaySound` — the SoundEntries kit name the 1.12 GlueXML plays for this click.
#[derive(Message)]
pub(crate) struct GlueSound(pub(crate) &'static str);

/// The held glue-music stream (non-`Sync` handle — non-Send state, like [`super::zone`]'s), plus
/// its starvation watch (decision 1109). The handle is held through the stop-fade, not dropped at
/// it: the fade rides out under the world-entry load burst, which is exactly the crackle-prone
/// window, and [`watch_glue_music`] can only see a stream it still holds. Since 1550 the theme
/// itself rides that whole burst too — all the more reason for the watch to be able to see it.
struct GlueMusic {
    handle: Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>>,
    watch: mixer::StreamWatch,
    /// The stop-fade is armed and running. The per-frame slider feed stands off while it is set,
    /// so a live `MusicVolume` change cannot stomp the ramp back up to full mid-fade — the hazard
    /// [`super::zone`]'s ambience crossfade documents from the other side.
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

/// Drain glue clicks into the kit player (2D SFX, by name — the client's name-registry path).
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

/// While on the select screen, start the title theme if it isn't up. Runs per-frame (cheap early
/// return once playing) rather than on the state edge: the app *boots* into CharSelect, and the
/// initial `OnEnter` fires before the mixer/chain exist — an edge-triggered start misses it.
fn start_glue_music(
    mut music: NonSendMut<GlueMusic>,
    mut out: NonSendMut<SoundOutput>,
    assets: Option<Res<WorldAssets>>,
    config: Res<SoundConfig>,
) {
    if music.handle.is_some() && !music.fading {
        return; // still playing (select ⇄ create hops re-enter CharSelect)
    }
    // A theme caught mid-fade is NOT "still playing": returning to the glue inside the 3 s ramp
    // must bring it back at once. The reference gets there by bookkeeping — the enter-world stop
    // clears the cached track name (`0x45afec`), so `SetGlueScreen`'s next `PlayGlueMusic` misses
    // the same-name early-out, stops the outgoing stream and opens a fresh one. Same outcome here:
    // the old handle is dropped below with its ramp already armed, so it finishes fading on the
    // backend while the new stream starts at full.
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

/// **The world's load drained — arm the theme's fade** (decision 1553, correcting 1550's trigger).
///
/// This is the reference's own trigger, and it is not a music event at all: `CGlueMgr::Update`
/// state 8 runs a *second* pass every frame after the entry (`[0xb41d94] == 1`), spinning on the
/// AsyncFileLoader (`0x443e20`) while any read is outstanding; the frame the queue drains it calls
/// `0x45aeb0(NULL)` → `0x45b050(3.0f)` and then sends `CMSG_PLAYER_LOGIN`. So the theme sounds
/// through the click and the whole map load, and dies on a 3.0 s fade armed *behind* the loading
/// screen — never on the click (1550's own `stop_glue_music`), and never on the first world track
/// (1550's replacement, which was the right *feel* found from a misread of the branch).
///
/// benilla's analogue of "the async queue drained" is the cover's own clear condition — every
/// wanted tile spawned, placements up, colliders quiet ([`crate::loading_screen`]) — so the fade is
/// armed on `world_hold`'s **falling edge**. The edge is tracked every frame but only acted on
/// **in the world**, which is what excludes the logout blackout: that cover drops on the frame the
/// state leaves `InWorld` (0738), and firing there would fade the theme the glue had just restarted.
///
/// The handle is deliberately kept through the fade — the [`GlueMusic`] docs say why — and reaped
/// by [`watch_glue_music`] once the fade lands on `Stopped`.
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

/// Per-frame stream health + handle reaping: the starvation watch (decision 1109) over the held
/// theme, and the drop once it reaches `Stopped` — after the handoff fade lands, or at the theme's
/// natural end. On the glue screens that end re-arms [`start_glue_music`], which brings the theme
/// back from the top — the login screen no longer falls silent for good after one play-through.
/// **In the world it does not**, and that asymmetry is the point of [`start_glue_music`]'s run
/// condition: a theme that outlives a music-less zone must run out, not loop under it (1550).
fn watch_glue_music(
    mut music: NonSendMut<GlueMusic>,
    // **`Real`, not the generic clock** — the watch compares elapsed time against the audio
    // stream's own position, so it must be fed WALL time. `Res<Time>` is the paced virtual clock:
    // `frame_pace` median-snaps it and caps it at `max_delta`, so during a loading burst it
    // neither matches wall nor errs in a predictable direction, and the "ms of injected silence"
    // it produced was not a measurement of anything.
    time: Res<Time<bevy::time::Real>>,
    config: Res<SoundConfig>,
) {
    let music = &mut *music;
    // The Music slider is live on this stream, as it is on the world's (the
    // `MusicVolume` handler's re-apply walker `0x7a6660(ecx=2)` matches the glue wrapper, so moving
    // the slider rescales the playing theme in place — mid-loading-screen included). Stand off
    // while the stop-fade runs; see [`GlueMusic::fading`].
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
        // The theme's end, said out loud — the counterpart to the `glue music: <path>` start line.
        // With the theme now outliving world entry (1550), "did it actually stop, or is it playing
        // under the world?" is a real question about a stream nothing else reports on, and it must
        // be answerable from an ordinary log rather than from a mix capture.
        info!("glue music: stream ended");
        music.handle = None;
        music.fading = false;
        music.watch.reset();
        return;
    }
    music.watch.feed(h, f64::from(time.delta_secs()));
}

/// Report the glue theme's voice into the global budget (decision 1557) — it rides `InWorld`
/// since 1550, so it occupies a channel there like anything else.
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
                // The theme starts at the login screen (the ref's `AccountLogin_OnShow` sets the
                // same `wow_main_theme` — decision 0539) and keeps across the glue screens.
                start_glue_music
                    .run_if(in_state(ClientState::Login).or(in_state(ClientState::CharSelect))),
                // No run condition on either: the theme rides `InWorld` now (1550), so its handoff
                // and the watch both have to keep running there — and the entry load burst is
                // exactly the window the watch exists for (1109).
                hand_off_glue_music,
                watch_glue_music
                    .after(start_glue_music)
                    .after(hand_off_glue_music),
                report_stream_voices,
            ),
        );
}
