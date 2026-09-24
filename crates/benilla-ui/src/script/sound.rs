//! The `PlaySound`/`PlaySoundFile` bindings — the first **Lua→app intent queue** (decisions
//! 0068 §3 / 0070 §4).
//!
//! The engine must not touch the app's systems (the same seam that keeps [`super::unit`]'s
//! bindings reading a pushed snapshot), so an outbound *action* can't call the mixer either:
//! `PlaySound` pushes a plain [`SoundRequest`] onto a queue in [`super::Model`], and the app
//! drains it each frame via [`UiScript::take_sounds`](super::UiScript::take_sounds) into its kit
//! player. This queue-drain shape is the pattern every Lua→app intent follows (`CastSpell`,
//! `UseAction`, … ride the same seam as they land).
//!
//! Surface: the Era signature `PlaySound(soundKitID[, channel, forceNoDuplicates])` (addons pass
//! numeric `SOUNDKIT.*` ids), plus the 1.12-native by-name form `PlaySound("KitName")` — the
//! client resolved both (`PlaySoundById 0x458850` / `PlaySoundByName 0x458030`), and our kit
//! player keeps both entries. Returns `willPlay, soundHandle`: `willPlay` reports "queued" —
//! whether the kit actually plays is decided at drain time by the kit player's own gates
//! (audibility, duplicate suppression), which the client also treats as silent non-errors; the
//! handle is nil (no stop-by-handle surface yet). The `channel`/`forceNoDuplicates` extras are
//! accepted and ignored for now — the kit's own `SoundType`/flags decide category and dedup.
//!
//! `PlaySoundFile("Sound\\...\\file.wav")` is the sibling by-path form (the 1.12 client resolves
//! it through the same file layer as kit entries — MPQ chain, loose files shadowing): no kit, so
//! no gates and no variation; same queue, same returns.
//!
//! `PlayMusic("Sound\\Music\\...\\track.mp3")` / `StopMusic()` are the **music** pair, and what
//! makes them their own queue ([`MusicRequest`]) rather than two more [`SoundRequest`]s is that
//! they are a different *slot*, not a different sound: the reference hands the Lua caller a music
//! stream of its own (`[0xb06ccc]`, opened inside `0x460450` at `0x4604a7`) beside the zone
//! track's (`[0xb06cc4]`), and it is one of the client's only two infinitely-looping streams
//! (`SetLoopCount(-1)`, the single call site `0x7a5592`; the other is the glue theme). The app
//! drains this queue into that slot, never into the kit player. Both bindings answer **nothing**
//! — `reference/1.12-shapes.tsv` types the pair at arity 0, `exact` — so neither returns the
//! `willPlay, soundHandle` pair `PlaySound` does.
//!
//! **They are one engine verb with one branch, which is why this queue carries an `Option`.**
//! `StopMusic 0x458770` is four instructions — `xor ecx,ecx; call 0x460450; xor eax,eax; ret` —
//! so *stopping is playing a NULL name*, and `0x460450` (two callers image-wide, no
//! address-takes) is the whole of both verbs. What that shared head does before the branch, and
//! what only the play arm does after it, is the app side's business (`sound::zone`'s
//! `set_lua_music`) — the point here is that the two bindings are not independent, and a
//! `MusicRequest` is the argument, not the verb.

use mlua::{Lua, Value};

use super::Model;

/// One queued `PlaySound` intent, drained by the app (plain data — the engine-free seam).
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRequest {
    /// `PlaySound(id)` — a `SoundEntries` kit id (the Era `SOUNDKIT.*` form).
    KitId(u32),
    /// `PlaySound("name")` — a kit name (the 1.12 `PlaySoundByName` form).
    KitName(String),
    /// `PlaySoundFile("path")` — a raw file path, no kit.
    File(String),
}

/// One queued Lua **music** intent — the `PlayMusic`/`StopMusic` pair, drained by the app's music
/// slot (module docs: a different slot, not a different sound, and **one verb with a NULL arm**).
#[derive(Clone, Debug, PartialEq)]
pub enum MusicRequest {
    /// `PlayMusic("path")` — the name arm: start the caller's own looping stream on the slot.
    Play(String),
    /// `StopMusic()` — `0x460450`'s NULL arm, the same call with no name.
    Stop,
}

impl super::UiScript {
    /// Drain the sounds queued by `PlaySound` since the last call. The app plays each through its
    /// kit player (2D — UI sounds have no world position).
    /// Queue a kit play from the APP side (the client's C++-fired UI sounds — QUESTADDED on a
    /// log add, QUESTCOMPLETED on the turn-in packet — which no Lua handler owns). Drained by the
    /// same [`UiScript::take_sounds`] as every Lua `PlaySound`.
    pub fn queue_sound_kit(&mut self, name: &str) {
        self.model_mut()
            .sound_queue
            .push(SoundRequest::KitName(name.to_string()));
    }

    pub fn take_sounds(&mut self) -> Vec<SoundRequest> {
        std::mem::take(&mut self.model_mut().sound_queue)
    }

    /// Drain the `PlayMusic`/`StopMusic` intents queued since the last call, **in call order** —
    /// which this queue has to preserve in a way the kit one does not: two calls in a frame are a
    /// start and a stop of the same slot, and the order is the difference between a track and
    /// silence.
    pub fn take_music(&mut self) -> Vec<MusicRequest> {
        std::mem::take(&mut self.model_mut().music_queue)
    }
}

/// Register the `PlaySound` and `PlaySoundFile` globals.
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
                // The client raises a usage error on a bad argument; in a handler this lands in
                // the host's collected script errors, never a panic.
                return Err(mlua::Error::runtime(
                    "Usage: PlaySound(soundKitID or \"KitName\")",
                ));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // `PlaySoundByName 0x458030`'s first act: read the UI-load suppression depth
            // (`0x458046`) and bail at `0x45804d` if it is non-zero — BEFORE the
            // `MasterSoundEffects` CVar and before the kit-hash lookup. So the call is dropped
            // rather than muted or queued, and the binding still answers as if it had played.
            // NAME-keyed only: the id-keyed entry (`0x457fb0`) has no such gate, which is why the
            // arm above pushes unconditionally.
            let suppressed = matches!(req, SoundRequest::KitName(_)) && model.sound_suppression > 0;
            if !suppressed {
                model.sound_queue.push(req);
            }
            drop(model);
            // willPlay (queued), soundHandle (nil — module docs).
            Ok((true, Value::Nil))
        })?,
    )?;
    lua.globals().set(
        "PlayMusic",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `0x458720`'s own argument shape, byte for byte: `lua_isstring 0x6f3510` then
            // `lua_tostring 0x6f3690` — so a **number is accepted and stringified**
            // (`PlayMusic(42)` asks for the file `"42"`), and everything else, *absent included*,
            // raises this exact literal (`0x835f7c`) through `lua_error 0x458758`. No
            // normalisation, no extension appended: the pointer goes verbatim to the file layer.
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
        // Takes no argument at all (`0x458770`, arity and argc both 0), so an extra one is
        // ignored rather than refused — `()` reads a bare call and any call alike.
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
        // Era extras accepted and ignored; the by-name 1.12 form works too.
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
        // Drained: the queue is empty until the next PlaySound.
        assert!(s.take_sounds().is_empty());
    }

    #[test]
    fn the_music_pair_queues_in_call_order_and_answers_nothing() {
        let mut s = UiScript::new().unwrap();
        // Arity 0 for both (`reference/1.12-shapes.tsv`, `exact`) — not `PlaySound`'s pair.
        // Counted with `table.getn` over a table constructor: this VM is 1.12's Lua **5.0**
        // surface, which has no `select`.
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
        // Drained: the queue is empty until the next call.
        assert!(s.take_music().is_empty());
        // And it is its own queue — the kit player never sees a music intent.
        assert!(s.take_sounds().is_empty());
    }

    /// `PlayMusic`'s argument is `lua_isstring` → `lua_tostring` (`0x6f3510`/`0x6f3690`), so a
    /// **number is a filename** and everything else — absent included — raises the binding's own
    /// literal. `StopMusic` reads no argument at all, so an extra one cannot be a usage error.
    #[test]
    fn playmusic_takes_a_string_or_a_number_and_stopmusic_takes_anything() {
        let mut s = UiScript::new().unwrap();
        assert!(s.run("PlayMusic()").is_err());
        assert!(s.run("PlayMusic(nil)").is_err());
        assert!(s.run("PlayMusic(true)").is_err());
        assert!(s.run("PlayMusic({})").is_err());
        assert!(s.take_music().is_empty(), "a raise queues nothing");

        // Coerced, not refused: the reference asks the file layer for `"42"` and gets silence.
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
