//! The glue screens' FrameScript state, which [`UiScript::new_glue`] builds: what the glue builder
//! (`0x46a7b0`) gives Lua where `UI_Init` (`0x48fbf0`) gives it the world's surface.
//!
//! The reference keeps one Lua state (`0xceef74`). The glue builder starts from the bare base set
//! (`0x7039e0`): on a rebuild it resets the state (`0x703b80` at `0x46a87b`, re-running the base
//! init), while the first build (`0x46a505`, passing 0) skips the reset (`je` at `0x46a879`) and
//! takes the state the startup init left (`0x402aee`). Either way it then runs the glue registrar
//! `0x46abb0` and loads the 26-event table below in place of the in-game one
//! (`0x703d90` at `0x46a88f`). The frame types are a process-wide registry instead
//! ([`super::object::registered_frame_kind`]).
//!
//! [`UiScript::new_glue`]: super::UiScript::new_glue

use mlua::Lua;

use super::Model;

/// Which of the reference's two FrameScript states a VM is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmKind {
    /// `UI_Init`'s: the in-game surface over FrameXML.
    InGame,
    /// The glue builder's: the base set, the three shared registrars and the glue verbs, over
    /// GlueXML.
    Glue,
}

/// The glue event table by id (`0xb41e70`, written by `0x46cc60`), `reference/1.12-glue-events.tsv`.
/// While the glue state runs these are the only events: `RegisterEvent` matches a name against
/// them case-insensitively (`0x702140` with `0x64a4c0`) and drops one they lack without an error.
pub const GLUE_EVENTS: [&str; 26] = [
    "SET_GLUE_SCREEN",
    "START_GLUE_MUSIC",
    "DISCONNECTED_FROM_SERVER",
    "OPEN_STATUS_DIALOG",
    "UPDATE_STATUS_DIALOG",
    "CLOSE_STATUS_DIALOG",
    "ADDON_LIST_UPDATE",
    "CHARACTER_LIST_UPDATE",
    "UPDATE_SELECTED_CHARACTER",
    "OPEN_REALM_LIST",
    "GET_PREFERRED_REALM_INFO",
    "UPDATE_SELECTED_RACE",
    "SELECT_LAST_CHARACTER",
    "SELECT_FIRST_CHARACTER",
    "GLUE_SCREENSHOT_SUCCEEDED",
    "GLUE_SCREENSHOT_FAILED",
    "PATCH_UPDATE_PROGRESS",
    "PATCH_DOWNLOADED",
    "SUGGEST_REALM",
    "SUGGEST_REALM_WRONG_PVP",
    "SUGGEST_REALM_WRONG_CATEGORY",
    "SHOW_SERVER_ALERT",
    "FRAMES_LOADED",
    "FORCE_RENAME_CHARACTER",
    "SHOW_SURVEY_NOTIFICATION",
    "PLAYER_ENTER_PIN",
];

impl VmKind {
    /// The event table's own spelling of `name` for `RegisterEvent`/`UnregisterEvent` (`0x702140`,
    /// `0x702280`), or `None` for a name the table lacks, which registers nothing.
    ///
    /// The in-game VM answers every name as given: benilla's in-game events register by name and
    /// carry no table, so a name off the reference's table registers and never fires, and only the
    /// exact spelling matches.
    pub(crate) fn event_key(self, name: &str) -> Option<&str> {
        match self {
            VmKind::InGame => Some(name),
            VmKind::Glue => GLUE_EVENTS
                .iter()
                .copied()
                .find(|e| e.eq_ignore_ascii_case(name)),
        }
    }

    /// Whether the host may fire `event` into this VM. The reference fires by id
    /// (`0x703e50`), so the glue state can only ever see one of its 26; a host's in-game event sent
    /// to the glue VM reaches no frame, `RegisterAllEvents` ones included.
    pub(crate) fn fires(self, event: &str) -> bool {
        match self {
            VmKind::InGame => true,
            VmKind::Glue => GLUE_EVENTS.contains(&event),
        }
    }
}

/// The glue core's screen verbs (registrar `0x46ddf0`, table `0x8373b8`): the screen root's size
/// and the cursor in it. The glue and in-game states share the root, `0x41ad10`'s, 768 units tall
/// and `768·a` wide for the aspect `a` (the glue builder sets it at `0x46a828`, from `0x46ab40`:
/// `gxResolution`'s width over height with `widescreen` on, its default, else 4:3). The glue has
/// no `UIParent` and so no UI scale: `GetScreenWidth` (`0x46dd30`) and `GetScreenHeight`
/// (`0x46dd80`) are the in-game pair (`0x48b480`, `0x48b4d0`) without its `0x494590` division,
/// and `GetCursorPosition` (`0x46dad0`) is byte for byte the in-game `0x48b820`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "GetScreenWidth",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.width())
        })?,
    )?;
    g.set(
        "GetScreenHeight",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.screen.height())
        })?,
    )?;
    g.set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.cursor_pos)
        })?,
    )?;
    super::glue_core::install(lua)?;
    super::glue_realm::install(lua)
}
