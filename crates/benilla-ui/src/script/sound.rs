//! The sound bindings: `PlaySound`, `PlaySoundFile`, `PlayMusic` and `StopMusic` each queue a
//! plain request for the app to drain, since the engine does not touch the app's mixer.
//!
//! `PlaySound` takes a kit name, as the reference's binding does (`0x4586d0` reaches only
//! `PlaySoundByName`, `0x458030`), and also Era's numeric kit id, which 1.12's binding does not
//! take (the client plays a kit id internally through `0x458850`). Both verbs answer Era's
//! `willPlay, soundHandle`, true and nil, not 1.12's shapes: the reference's `PlaySound` answers
//! nothing and its `PlaySoundFile` one value (`reference/1.12-shapes.tsv`).
//!
//! The music pair drives its own slot: the reference gives Lua a stream of its own (`[0xb06ccc]`,
//! opened in `0x460450` at `0x4604a7`) beside the zone track's (`[0xb06cc4]`), one of the client's
//! two endlessly looping streams (`0x7a5592`). `StopMusic` (`0x458770`) is `0x460450` with a null
//! name, so a [`MusicRequest`] is the argument, not the verb. Neither answers anything.

use mlua::{Lua, Value};

use super::Model;

/// One queued sound for the app's kit player.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRequest {
    /// `PlaySound(id)`: a `SoundEntries` kit id, Era's form.
    KitId(u32),
    /// `PlaySound("name")`: a kit name, the 1.12 form.
    KitName(String),
    /// `PlaySoundFile("path")`: a file path, no kit.
    File(String),
}

/// One queued `PlayMusic` or `StopMusic`, for the app's Lua music slot.
#[derive(Clone, Debug, PartialEq)]
pub enum MusicRequest {
    /// `PlayMusic("path")`: start the caller's looping stream.
    Play(String),
    /// `StopMusic()`: `0x460450` with no name.
    Stop,
}

impl super::UiScript {
    /// Queue a kit play from the app side, for the UI sounds the client fires from C++
    /// (`QUESTCOMPLETED` on the turn-in) that no Lua handler or message row owns.
    pub fn queue_sound_kit(&mut self, name: &str) {
        self.model_mut()
            .sound_queue
            .push(SoundRequest::KitName(name.to_string()));
    }

    /// Drain the queued sounds; the app plays each in 2D, as UI sounds have no world position.
    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.model_mut().sound_queue)
    }

    /// Drain the music intents in call order: a play and a stop in one frame are a track or
    /// silence by their order.
    pub fn take_music(&mut self) -> Vec<MusicRequest> {
        std::mem::take(&mut self.model_mut().music_queue)
    }
}

/// Register the sound and music globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "PlaySoundFile",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let Some(Value::String(s)) = args.front() else {
                return Err(mlua::Error::runtime("Usage: PlaySoundFile(\"filePath\")"));
            };
            let path = s.to_str()?.to_owned();
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.sound_queue.push(SoundRequest::File(path));
            drop(model);
            Ok((true, Value::Nil))
        })?,
    )?;
    lua.globals().set(
        "PlaySound",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let req = match args.front() {
                Some(Value::Integer(i)) if *i >= 0 => Some(SoundRequest::KitId(*i as u32)),
                Some(Value::Number(n)) if *n >= 0.0 => Some(SoundRequest::KitId(*n as u32)),
                Some(Value::String(s)) => Some(SoundRequest::KitName(s.to_str()?.to_owned())),
                _ => None,
            };
            let Some(req) = req else {
                // A bad argument raises a usage error, as the reference's does.
                return Err(mlua::Error::runtime(
                    "Usage: PlaySound(soundKitID or \"KitName\")",
                ));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // During the UI load a by-name play is dropped before any other gate (`0x458046`,
            // `0x45804d`), while the binding still answers; the id entry (`0x457fb0`) has no
            // such gate.
            let suppressed = matches!(req, SoundRequest::KitName(_)) && model.sound_suppression > 0;
            if !suppressed {
                model.sound_queue.push(req);
            }
            drop(model);
            Ok((true, Value::Nil))
        })?,
    )?;
    lua.globals().set(
        "PlayMusic",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `0x458720`: `lua_isstring` (`0x6f3510`), then `lua_tostring` (`0x6f3690`), so a
            // number becomes a file name and anything else raises this literal (`0x835f7c`,
            // `0x458758`); the path goes to the file layer verbatim.
            let path = super::binding_abi::string_arg(
                lua,
                args.front().cloned().unwrap_or(Value::Nil),
                "Usage: PlayMusic(\"music\")",
            )?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .music_queue
                .push(MusicRequest::Play(path));
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "StopMusic",
        // Reads no argument (`0x458770`), so an extra one is ignored.
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .music_queue
                .push(MusicRequest::Stop);
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::{MusicRequest, SoundRequest};
    use crate::script::UiScript;

    #[test]
    fn playsound_queues_by_id_and_name_and_drains() {
        let mut s = UiScript::new().unwrap();
        let (will_play, handle_is_nil): (bool, bool) = s
            .eval("local w, h = PlaySound(1234) return w, h == nil")
            .unwrap();
        assert!(will_play && handle_is_nil);
        // Era's extra arguments are ignored.
        s.run(r#"PlaySound(841, "SFX", true)"#).unwrap();
        s.run(r#"PlaySound("GAMEOBJECT_DOOROPEN")"#).unwrap();
        s.run(r#"PlaySoundFile("Sound\\Doodad\\BellTollHorde.wav")"#)
            .unwrap();

        assert_eq!(
            s.take_sounds(),
            vec![
                SoundRequest::KitId(1234),
                SoundRequest::KitId(841),
                SoundRequest::KitName("GAMEOBJECT_DOOROPEN".into()),
                SoundRequest::File("Sound\\Doodad\\BellTollHorde.wav".into()),
            ]
        );
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn the_music_pair_queues_in_call_order_and_answers_nothing() {
        let mut s = UiScript::new().unwrap();
        // Counted with `table.getn`: Lua 5.0 has no `select`.
        let (played, stopped): (usize, usize) = s
            .eval(
                r#"return table.getn({PlayMusic("Sound\\Music\\x.mp3")}),
                          table.getn({StopMusic()})"#,
            )
            .unwrap();
        assert_eq!((played, stopped), (0, 0));
        s.run(r#"PlayMusic("Sound\\Music\\ZoneMusic\\Elwynn\\DayElwynn01.mp3")"#)
            .unwrap();
        s.run("StopMusic()").unwrap();
        assert_eq!(
            s.take_music(),
            vec![
                MusicRequest::Play("Sound\\Music\\x.mp3".into()),
                MusicRequest::Stop,
                MusicRequest::Play("Sound\\Music\\ZoneMusic\\Elwynn\\DayElwynn01.mp3".into()),
                MusicRequest::Stop,
            ]
        );
        assert!(s.take_music().is_empty());
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn playmusic_takes_a_string_or_a_number_and_stopmusic_takes_anything() {
        let mut s = UiScript::new().unwrap();
        assert!(s.run("PlayMusic()").is_err());
        assert!(s.run("PlayMusic(nil)").is_err());
        assert!(s.run("PlayMusic(true)").is_err());
        assert!(s.run("PlayMusic({})").is_err());
        assert!(s.take_music().is_empty(), "a raise queues nothing");

        // The reference asks the file layer for `"42"` and plays nothing.
        s.run("PlayMusic(42)").unwrap();
        s.run("StopMusic(1, 2)").unwrap();
        assert_eq!(
            s.take_music(),
            vec![MusicRequest::Play("42".into()), MusicRequest::Stop]
        );
    }

    #[test]
    fn playsound_with_a_bad_argument_is_a_usage_error() {
        let mut s = UiScript::new().unwrap();
        assert!(s.run("PlaySound(nil)").is_err());
        assert!(s.run("PlaySound()").is_err());
        assert!(s.run("PlaySoundFile()").is_err());
        assert!(s.run("PlaySoundFile(42)").is_err());
        assert!(s.take_sounds().is_empty());
    }
}
