//! The sound layer's packet handlers: the three server audio triggers, each a
//! [`ServerSoundMessage`] the zone mixer ([`super::zone`]) plays.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp, ServerSoundKind, ServerSoundMessage};

/// Register the handlers, from [`super::SoundPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PlaySound, on_play)
        .net_handler(K::PlayMusic, on_play)
        .net_handler(K::PlayObjectSound, on_play);
}

fn on_play(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut out: MessageWriter<ServerSoundMessage>,
) {
    match ev {
        SessionEvent::PlaySound { sound_id } => play_sound(sound_id, &mut out),
        SessionEvent::PlayMusic { music_id } => play_music(music_id, &mut out),
        SessionEvent::PlayObjectSound { sound_id, guid } => {
            play_object_sound(sound_id, guid, &index, &mut out)
        }
        _ => {}
    }
}

/// `SMSG_PLAY_SOUND`: a 2D one-shot.
fn play_sound(sound_id: u32, out: &mut MessageWriter<ServerSoundMessage>) {
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::Sound2d,
        sound_id,
        source: None,
    });
}

/// `SMSG_PLAY_MUSIC`: the zone or event music track.
fn play_music(music_id: u32, out: &mut MessageWriter<ServerSoundMessage>) {
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::Music,
        sound_id: music_id,
        source: None,
    });
}

/// `SMSG_PLAY_OBJECT_SOUND`: a one-shot at a streamed object, silent while its guid is not
/// streamed in.
fn play_object_sound(
    sound_id: u32,
    guid: u64,
    index: &GuidIndex,
    out: &mut MessageWriter<ServerSoundMessage>,
) {
    out.write(ServerSoundMessage {
        kind: ServerSoundKind::ObjectSound,
        sound_id,
        source: index.0.get(&guid).copied(),
    });
}
