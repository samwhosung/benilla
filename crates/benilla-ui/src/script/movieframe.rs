//! The `MovieFrame` methods (`CSimpleMovieFrame`, factory `0x6ef000`; method table `0x87ab4c`):
//! `StartMovie` (`0x78d770`), `StopMovie` (`0x78d870`) and `EnableSubtitles` (`0x78d940`).
//!
//! Deviation: benilla plays no pre-rendered movie (#968), so `StartMovie` answers nil, the
//! reference's "could not start" (`0x78d861`), which the stock `MovieFrame_PlayMovie` answers by
//! hiding the frame and so by returning to the login screen. With no movie ever running,
//! `StopMovie` has nothing to stop: `0x78ced0` returns at once when the playing slot `+0x314` is
//! clear, and only past that test fires `OnMovieFinished` (`0x78cf0b`-`0x78cf1e`). The three
//! `OnMovie*` scripts (script map `0x78c2e0`, slots `+0x320`/`+0x328`/`+0x330`) are accepted by
//! `SetScript` and never run.

use mlua::{Lua, MultiValue, Table, Value};

use super::binding_abi::bool_or_default;
use super::font_block::usage_name;
use super::object::frame_handle_of;
use super::Model;
use crate::widget::FrameKind;

/// Registry key of the MovieFrame method table (the MAXCSTACK discipline: a Lua-side root).
pub(super) const REG_MOVIEFRAME_METHODS: &str = "__benilla_movieframe_methods";

/// Raises unless `this` is a live MovieFrame: the method table is a plain Lua value a caller can
/// lift onto another frame (`0x847e98`, "Wrong object type for member function").
fn check_movie_frame(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    match model.arena.frame(h) {
        Some(f) if f.kind == FrameKind::MovieFrame => Ok(()),
        _ => Err(mlua::Error::runtime(
            "Wrong object type for member function",
        )),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `lua_isstring` on the path (`0x6f3510`: a string or a number), else
    // `Usage: %s:StartMovie("filename")` (`0x87ab8c`); one value back (`0x78d84d`/`0x78d861`),
    // 1 for a movie that started and nil for one that did not.
    m.set(
        "StartMovie",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            check_movie_frame(lua, &this)?;
            if !matches!(
                path,
                Value::String(_) | Value::Integer(_) | Value::Number(_)
            ) {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:StartMovie(\"filename\")",
                    usage_name(&this)
                )));
            }
            Ok(Value::Nil)
        })?,
    )?;
    // No arguments, no returns (`0x78d8b8`).
    m.set(
        "StopMovie",
        lua.create_function(|lua, this: Table| check_movie_frame(lua, &this))?,
    )?;
    // The flag defaults to true when absent (`0x6f1c10` with the default `1`, as
    // `EnableKeyboard`'s `0x776f4e`), stored at `+0x31c` for the player's subtitle events, which
    // nothing here fires; no returns.
    m.set(
        "EnableSubtitles",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            check_movie_frame(lua, &this)?;
            let _enabled = bool_or_default(args.front(), true);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_MOVIEFRAME_METHODS, m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// `StartMovie` answers nil (no player), takes a string or a number, and raises its usage on
    /// anything else; `StopMovie` and `EnableSubtitles` answer nothing; and the three player
    /// scripts are taken by `SetScript` and read back.
    #[test]
    fn the_movie_frame_takes_its_methods_and_scripts_and_plays_nothing() {
        let s = UiScript::new_glue().unwrap();
        s.run(r#"M = CreateFrame("MovieFrame", "MovieProbe")"#)
            .unwrap();
        assert!(s
            .eval::<bool>(r#"return M:StartMovie("a\\b") == nil"#)
            .unwrap());
        assert!(s.eval::<bool>("return M:StartMovie(7) == nil").unwrap());
        assert!(s
            .eval::<bool>("return table.getn({M:StopMovie()}) == 0")
            .unwrap());
        assert!(s
            .eval::<bool>("return table.getn({M:EnableSubtitles(1)}) + table.getn({M:EnableSubtitles()}) == 0")
            .unwrap());
        for bad in ["M:StartMovie()", "M:StartMovie({})", "M:StartMovie(true)"] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains(r#"Usage: MovieProbe:StartMovie("filename")"#),
                "{bad}: {err}"
            );
        }
        for name in [
            "OnMovieFinished",
            "OnMovieShowSubtitle",
            "OnMovieHideSubtitle",
        ] {
            s.run(&format!("M:SetScript('{name}', function() end)"))
                .unwrap();
            assert!(
                s.eval::<bool>(&format!("return M:GetScript('{name}') ~= nil"))
                    .unwrap(),
                "{name}"
            );
        }
        // A plain frame has none of the methods.
        s.run(r#"P = CreateFrame("Frame", "NotAMovie")"#).unwrap();
        assert!(s.eval::<bool>("return P.StartMovie == nil").unwrap());
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
