//! The cinematic narration: the race intro's voice-over, held by a handle so a skip cuts it off.
//!
//! Each race fly-by's `CinematicCamera` row names one streamed `<Race>Narration.mp3` kit (type
//! 31); the two non-intro rows name id 0. A dropped kira handle keeps playing, so the handle is
//! held here. The reference keeps the narration in `[0xb4e274]` (written at `0x48ef3e`) and
//! releases it on shot advance (`0x48efef`), stop/ESC (`0x48f055`) and local abort (`0x490b8d`);
//! this module follows [`Cinematic`]'s published shot the same way.

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::mixer::{self, StreamingSoundHandle};
use super::{kit::SoundKits, SoundOutput};
use crate::cinematic::Cinematic;

/// A stopped narration is cut with no fade: every release of `[0xb4e274]` is plain. The 0.25 s
/// fade the reference schedules at both edges (`0x4c0d10`, `[0x804550]`) is a screen fade.
const CUT_FADE_MS: u64 = 0;

/// The narration channel: which shot it belongs to, and its live handle.
#[derive(Default)]
pub(super) struct CinematicVoice {
    /// `(run, shot index)` of the playing narration. Keyed on the run, not the sequence id:
    /// re-triggering the sequence on screen restarts the picture and must restart the voice.
    shot: Option<(u64, usize)>,
    handle: Option<StreamingSoundHandle<kira::sound::FromFileError>>,
}

impl CinematicVoice {
    /// Whether the device is still mixing this voice; a narration shorter than its shot ends
    /// `Stopped` on its own.
    fn is_live(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|h| !matches!(h.state(), kira::sound::PlaybackState::Stopped))
    }

    fn stop(&mut self) {
        self.shot = None;
        if let Some(mut h) = self.handle.take() {
            h.stop(mixer::fade(CUT_FADE_MS));
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    // In `Present`, after the cinematic driver in `Input` has settled this frame's shot. Gated on
    // `SoundKits`, which exists only once the install's kits load: without it the bare `ResMut`
    // panics.
    app.add_systems(
        Update,
        drive_narration
            .in_set(WorldStage::Present)
            .run_if(resource_exists::<SoundKits>),
    );
}

/// The narration channel, plus its line in the voice budget: each stream owner rewrites its own
/// count in `SoundOutput` every frame from its live handle, after the body has settled it.
fn drive_narration(
    cine: Option<Res<Cinematic>>,
    voice: Local<CinematicVoice>,
    out: NonSendMut<SoundOutput>,
    kits: ResMut<SoundKits>,
    assets: Option<Res<WorldAssets>>,
) {
    let (mut voice, mut out, mut kits) = (voice, out, kits);
    narrate(&cine, &mut voice, &mut out, &mut kits, &assets);
    out.cinematic_streams = usize::from(voice.is_live());
}

fn narrate(
    cine: &Option<Res<Cinematic>>,
    voice: &mut CinematicVoice,
    out: &mut SoundOutput,
    kits: &mut SoundKits,
    assets: &Option<Res<WorldAssets>>,
) {
    let playing = cine.as_deref().and_then(Cinematic::playing_shot);
    let Some((run, index, sound_id)) = playing else {
        // No cinematic, or it just ended or was skipped: cut whatever was narrating.
        voice.stop();
        return;
    };
    if voice.shot == Some((run, index)) {
        return;
    }
    voice.stop();
    voice.shot = Some((run, index));
    if sound_id == 0 {
        return;
    }
    let Some(assets) = assets else { return };
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return;
    };
    let Some((path, kit_vol)) = kits.pick_stream(sound_id) else {
        warn!("cinematic voice: sound {sound_id} has no file");
        return;
    };
    let bytes = {
        let chain = assets.chain.lock_recover();
        chain.read(&path)
    };
    let data = match bytes.and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            warn!("cinematic voice: {path} — {e:#}");
            return;
        }
    };
    // No category slider applies: the narration opens (`0x48ef29` → `0x458a40` → `0x45ce60`)
    // with flags `0x15`, and bit `0x10` skips the category multiply at `0x7a5dc0`, so its gain is
    // the kit volume flat. Only bit `0x2` bypasses the `MasterSoundEffects` gate (`0x7a529c`),
    // so "Enable Sound Effects" still silences it; that gate rides the master track, which
    // re-reads `SoundConfig::enabled` every frame, never this starting amp.
    let amp = kit_vol;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(amp))) {
        Ok(h) => {
            info!("cinematic voice: {path}");
            voice.handle = Some(h);
        }
        Err(e) => warn!("cinematic voice: {path} — {e:#}"),
    }
}
