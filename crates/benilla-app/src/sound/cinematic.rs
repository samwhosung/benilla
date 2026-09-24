//! The cinematic **narration** — the race intro's voice-over, and the handle that lets an ESC cut
//! it off mid-sentence.
//!
//! Every race fly-by names a `SoundEntries.dbc` id on its `CinematicCamera` row, and all eight are
//! the same thing: a single streamed `Sound\CinematicVoices\<Race>Narration.mp3` (kit type 31,
//! "DwarfFlyByNarration" and its siblings). The two non-intro rows — `PalantirOfAzora` and
//! `Scry_cam` — name id 0 and play nothing.
//!
//! **Why this lives in the sound module rather than in [`crate::cinematic`].** Starting a stream
//! and holding its handle is sound-internal (`pick_stream`, the mixer, the category amp are all
//! `pub(super)` here), and the handle is the whole point: a dropped kira handle keeps playing, so
//! a skipped cinematic whose narration was merely forgotten would keep narrating over the game for
//! another minute. The reference has exactly the same problem and the same answer — it keeps the
//! cinematic's sound in a dedicated global (`[0xb4e274]`) and *releases* it on **three** paths,
//! which are all of its references besides the write at `0x48ef3e`: `0x48efef` on the shot
//! advance — and therefore on the ordinary end, since every shipped sequence is a single shot —
//! `0x48f055` on the stop/ESC, and `0x490b8d` in the local-abort teardown. The first was missing
//! from this list; the behaviour was not (a shot change and an ended cinematic both stop the voice
//! below, which is the same three edges).
//!
//! So this module watches [`Cinematic`]'s published shot and follows it: a new shot starts its
//! narration, and the shot going away stops it. That keeps the coupling one-way — the cinematic
//! plugin never learns what a mixer is.

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::mixer::{self, StreamingSoundHandle};
use super::{kit::SoundKits, SoundOutput};
use crate::cinematic::Cinematic;

/// A cut-off narration is **cut**, with no fade at all: every release on this path
/// (`0x48efef` shot-advance, `0x48f055` stop/ESC, `0x490b8d` local abort) is a plain release of
/// `[0xb4e274]`, and there is **no audio fade anywhere in the cinematic** — the 0.25 s fade the
/// reference schedules at both edges (`0x4c0d10`, `[0x804550]`) is a *screen* fade, which is what
/// decision 1695 deferred. benilla's 250 ms declick was a guess at a fade the reference does not
/// have.
const CUT_FADE_MS: u64 = 0;

/// The narration channel: which shot it belongs to, and its live handle.
#[derive(Default)]
pub(super) struct CinematicVoice {
    /// `(run, shot index)` of the shot whose narration is playing — the identity that decides
    /// whether the current shot's audio is already running. Keyed on the **run** rather than the
    /// sequence id: re-triggering the sequence already on screen restarts the picture at `t = 0`,
    /// and a sequence-keyed identity would have matched and left the voice running from wherever
    /// it had reached.
    shot: Option<(u64, usize)>,
    handle: Option<StreamingSoundHandle<kira::sound::FromFileError>>,
}

impl CinematicVoice {
    /// Is this owner holding a voice the device is actually mixing? The budget's question — see
    /// [`drive_narration`]. `Stopped` covers a stream that reached its end on its own, which is
    /// the ordinary case for a narration shorter than its shot.
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
    // In `Present`, after the cinematic driver in `Input` has settled this frame's shot: the
    // narration follows the picture rather than racing it.
    // Gated on the kit catalog: `SoundKits` is inserted only once the install's kits have
    // loaded, and a run with no install (an instrument leg pointed at the wrong data path, 2211)
    // reached this system's bare `ResMut<SoundKits>` and panicked the app instead of playing
    // nothing. The narration needs a catalog to play from; without one there is nothing to do.
    app.add_systems(
        Update,
        drive_narration
            .in_set(WorldStage::Present)
            .run_if(resource_exists::<SoundKits>),
    );
}

/// The narration channel, plus its line in the voice budget.
///
/// The **budget** half is why this wrapper exists. `SoundOutput::live_voices()` — the number
/// [`super::kit::SOFTWARE_CHANNELS`] bounds, and which gates real play decisions, not just a
/// readout — summed the kit channels, the zone streams and the glue streams. The narration is a
/// *third* long-lived stream owner and it was in none of them, so for the whole length of a
/// cinematic the client believed it had one more free voice than it did. `SoundOutput`'s own doc
/// already states the law this now follows: "each owner **rewrites its own** every frame from its
/// own live handles", because a shared counter with two writers drifts the first time a fade is
/// interrupted. So the count is written here, once, from this owner's handle — after the body
/// below has settled it, never from a stale read.
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
        // No cinematic (or it just ended/was skipped) — cut whatever was narrating.
        voice.stop();
        return;
    };
    if voice.shot == Some((run, index)) {
        return;
    }
    // A new shot: the previous one's narration, if any, does not carry over.
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
    // **No category slider applies to this channel at all**. The narration is opened at
    // `0x48ef29` → `0x458a40` → `0x45ce60` with behaviour
    // flags `0x15`: `or eax,4` (`0x45ce9e`) and `or eax,0x10` (`0x45cea8`), while the music bit
    // `or eax,2` at `0x45ceb2` is **skipped**. Bit `0x10` is read at `0x7a5dc0`
    // (`test al,0x10; jne 0x7a5e1a`) and takes the channel around the category multiply entirely —
    // so Sound, Music and Ambience sliders all leave it alone, and its gain is the kit volume flat
    // (`__ftol(0.69 · 1.0 · 255)` = 175/255 on the shipped narrations).
    //
    // It is **not** exempt from the master, though: only bit `0x2` bypasses the
    // `MasterSoundEffects` gate at `0x7a529c`, and this channel does not set it — so unchecking
    // "Enable Sound Effects" silences the narration outright. [`SoundConfig::enabled`] is that
    // checkbox here.
    //
    // The old reading — the Sfx category, "the same one every NPC voice line takes" — was a
    // reasonable guess at where a player's expectation points, and it was wrong: this channel has
    // no category.
    // **The kit volume only** — the `MasterSoundEffects` gate is carried by the master track, not
    // baked in here. `apply_master_volume` drives master to 0 whenever `enabled` is off (or the
    // dev mute is on) and back up when it returns, so folding `config.enabled` into a one-shot
    // starting amp was redundant in the silencing direction and *wrong* in the other: a player who
    // started a cinematic with sound off and turned it back on mid-narration got everything else
    // back and a narration that stayed silent for the rest of the shot. The reference re-reads its
    // gate (`0x7a529c`) every tick; the master track is where benilla re-reads it.
    let amp = kit_vol;
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(amp))) {
        Ok(h) => {
            info!("cinematic voice: {path}");
            voice.handle = Some(h);
        }
        Err(e) => warn!("cinematic voice: {path} — {e:#}"),
    }
}
