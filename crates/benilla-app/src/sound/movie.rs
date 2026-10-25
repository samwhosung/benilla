//! The movie frame's audio: each movie's MP3 stream, opened from the AVI's audio chunks as the
//! reference's `0x78ca30` opens a memory stream (`FSOUND_Stream_Open` with `0x8000`, then
//! `FSOUND_Stream_Play` on channel 0, its volume never set), and its play time and end reported
//! back to the movie's clock ([`benilla_ui::script::UiScript::set_movie_audio`]).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::MovieAudio;

use super::{mixer, SoundConfig, SoundOutput};

/// The streams the movies hold, by movie number.
#[derive(Default)]
struct MovieStreams(HashMap<u64, mixer::StreamingSoundHandle<kira::sound::FromFileError>>);

/// Open each new movie's stream, report each stream's play time or end, and stop the stream of a
/// movie that stopped (`0x78caa0`). With sound off, or no device, no stream opens and the movie
/// runs on the wall clock, as FMOD unopened fails the stream open.
fn drive_movie_audio(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut streams: NonSendMut<MovieStreams>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            for (_, mut h) in streams.0.drain() {
                h.stop(mixer::fade(0));
            }
            return;
        };
        for start in script.take_movie_audio_starts() {
            match open_stream(&mut out, &config, start.mp3) {
                Some(h) => {
                    streams.0.insert(start.id, h);
                }
                None => script.set_movie_audio(start.id, MovieAudio::Failed),
            }
        }
        let live = script.movie_audio_ids();
        streams.0.retain(|id, h| {
            if live.contains(id) {
                return true;
            }
            h.stop(mixer::fade(0));
            false
        });
        for (&id, h) in &streams.0 {
            let report = if h.state() == kira::sound::PlaybackState::Stopped {
                MovieAudio::Ended
            } else {
                MovieAudio::At((h.position() * 1000.0) as u32)
            };
            script.set_movie_audio(id, report);
        }
    });
    out.movie_streams = streams
        .0
        .values()
        .filter(|h| h.state() != kira::sound::PlaybackState::Stopped)
        .count();
}

/// The stream at full volume: the reference sets none on the channel it plays.
fn open_stream(
    out: &mut SoundOutput,
    config: &SoundConfig,
    mp3: Vec<u8>,
) -> Option<mixer::StreamingSoundHandle<kira::sound::FromFileError>> {
    if !config.enabled {
        return None;
    }
    let mixer = out.mixer.as_mut()?;
    let data = mixer::stream_from_bytes(mp3)
        .inspect_err(|e| warn!("movie audio: {e:#}"))
        .ok()?;
    mixer
        .play_stream(data)
        .inspect_err(|e| warn!("movie audio: {e:#}"))
        .ok()
}

pub(super) fn plugin(app: &mut App) {
    app.insert_non_send_resource(MovieStreams::default())
        .add_systems(
            Update,
            // A feed of the tick: the clock steps on the play time read here.
            drive_movie_audio.in_set(crate::ui_script::UiFeed),
        );
}
