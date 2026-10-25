//! The movie frame's audio: each movie's MP3 stream, opened from the AVI's audio chunks as the
//! reference's `0x78ca30` opens a memory stream (`FSOUND_Stream_Open` with `0x8000`, then
//! `FSOUND_Stream_Play` on channel 0, its volume never set). The movie feed
//! (`ui_script::movie`) asks for each stream and its stop, and reads back each stream's play
//! time or end, which the movie's clock steps on.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::MovieAudio;

use super::{mixer, SoundConfig, SoundOutput};

/// The movie feed's asks, by movie number.
#[derive(Message, Clone, Debug)]
pub(crate) enum MovieAudioRequest {
    /// Open and play this movie's audio.
    Start { id: u64, mp3: Vec<u8> },
    /// The movie stopped: its stream stops with it (`0x78caa0`).
    Stop { id: u64 },
}

/// What the streams did since the feed last read: a failed open, then each stream's play time
/// or its end, by movie number.
#[derive(Resource, Default)]
pub(crate) struct MovieAudioReports(pub(crate) Vec<(u64, MovieAudio)>);

/// The streams the movies hold, by movie number.
#[derive(Default)]
struct MovieStreams(HashMap<u64, mixer::StreamingSoundHandle<kira::sound::FromFileError>>);

/// Carry out the feed's asks and report every stream. With sound off, or no device, no stream
/// opens and the movie runs on the wall clock, as an open fails with FMOD not started.
fn drive_movie_audio(
    mut requests: MessageReader<MovieAudioRequest>,
    mut streams: NonSendMut<MovieStreams>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    mut reports: ResMut<MovieAudioReports>,
) {
    for request in requests.read() {
        match request {
            MovieAudioRequest::Start { id, mp3 } => {
                match open_stream(&mut out, &config, mp3.clone()) {
                    Some(h) => {
                        // A number is never handed out twice; a stream under it would leak.
                        if let Some(mut old) = streams.0.insert(*id, h) {
                            old.stop(mixer::fade(0));
                        }
                    }
                    None => reports.0.push((*id, MovieAudio::Failed)),
                }
            }
            MovieAudioRequest::Stop { id } => {
                if let Some(mut h) = streams.0.remove(id) {
                    h.stop(mixer::fade(0));
                }
            }
        }
    }
    for (&id, h) in &streams.0 {
        let report = if h.state() == kira::sound::PlaybackState::Stopped {
            MovieAudio::Ended
        } else {
            MovieAudio::At((h.position() * 1000.0) as u32)
        };
        reports.0.push((id, report));
    }
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
    app.add_message::<MovieAudioRequest>()
        .init_resource::<MovieAudioReports>()
        .insert_non_send_resource(MovieStreams::default())
        .add_systems(
            Update,
            // After the feed's asks and the tick, its reports reaching the next frame's feed;
            // after the frame's mute toggle and the cinematic's music hold settle the config it
            // opens under, and after the streaming stage's main-thread work.
            drive_movie_audio
                .after(crate::ui_script::UiTick)
                .after(super::toggle_mute)
                .after(benilla_world::schedule::WorldStage::Stream),
        );
}
