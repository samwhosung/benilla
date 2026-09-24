//! The `Minimap` method surface — the zoom API the FrameXML zoom buttons drive
//! (`MinimapZoomIn`/`MinimapZoomOut` → `Minimap:SetZoom(Minimap:GetZoom() ± 1)`).
//!
//! `get_zoom_levels` (`0x6da9a0`) returns the constant 6, `set_zoom` clamps at 5 and marks the tile
//! grid dirty, and the zoom index feeds the zoom-to-scale tables (`0x6da9b0`). The engine core
//! carries only the index
//! ([`MinimapState`]); the app renderer maps it to a world radius and draws the tiles
//! (decision 0203). The two model attrs (`minimapArrowModel=`/`minimapPlayerModel=`) are modeled
//! only as far as the Lua surface can see them: [`apply_model_attrs`] names the nine engine
//! children the ctor built, and the app still draws every arrow from its own art.
//!
//! **The ping** (decision 1596) is the same split, one rung further out: the two methods here are
//! pure seam — `PingLocation` parks a click and `GetPingPosition` reads back what the app
//! published — because the ping's only stored form is a WORLD point the app pins it to, and the
//! engine core has no world. Nothing here holds the ping's position, its lifetime, or its art;
//! putting any of that in Lua is what made the first attempt flaky.
//!
//! **The level persists** (decision 1131). The client keeps two halves: the live indices
//! (`0x86f698` outdoor / `0x86f69c` indoor — our [`MinimapState`]) and the two CVar objects
//! `minimapZoom`/`minimapInsideZoom` (both registered default `"3"`), which are what `Config.wtf`
//! actually stores. `set_zoom` writes *both*; the minimap reset path (`0x6d9008`–`0x6d901f`)
//! re-seeds the live index from the CVar's parsed int. benilla runs the same two halves at the same
//! seam: `SetZoom` below writes the index and the CVar, and the app seeds the widget from the CVar
//! table once, when the in-game UI materializes ([`super::UiScript::set_minimap_zoom`]).
//!
//! The methods live in their own registry table, consulted by the frame `__index` dispatcher only
//! for Minimap frames — the same duck-typing posture as StatusBar's.

use mlua::{Lua, Table};

use super::object::frame_handle_of;
use super::Model;
use crate::widget::{KindState, MinimapState, MINIMAP_ZOOM_LEVELS};

/// Registry key of the Minimap method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_MINIMAP_METHODS: &str = "__benilla_minimap_methods";

/// Run `f` over a frame's Minimap state under one short write borrow. Errors if `this` is not a
/// live Minimap (unreachable through the kind dispatcher, but the method table is a plain Lua
/// value — a caller can fish it out and misapply it).
fn with_minimap<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut MinimapState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Minimap(m) => Ok(f(m)),
        _ => Err(mlua::Error::runtime("not a Minimap")),
    }
}

/// `CMinimap::LoadXML 0x4ee2b0`'s model half: assign the two `<Minimap>` model attributes to the
/// nine engine children the ctor already built. `minimapArrowModel` (engine default
/// [`crate::widget::MINIMAP_DEFAULT_ARROW_MODEL`]) goes to children **1–8** via `0x4ee170`'s two loops
/// (`0x4ee1b7` over the five at `+0x320`, `0x4ee204` over the three at `+0x314`);
/// `minimapPlayerModel` (default [`crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL`]) goes to child **9 alone**, via
/// `0x4ee260`.
///
/// The split is what makes the nine *distinguishable* from Lua: without it `GetModel()` is `""` on
/// all nine and nothing in the tuple says which one is the player arrow.
///
/// Runs before the `<Frames>` descent, because `0x4ee2b0` does its own work and only then chains to
/// `CSimpleFrame::LoadXML 0x76a2f0`, which is what recurses into the children.
pub(crate) fn apply_model_attrs(
    lua: &Lua,
    this: &Table,
    arrow: &str,
    player: &str,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(children) = model.arena.frame(h).map(|f| f.children.clone()) else {
        return Ok(());
    };
    for (i, child) in children
        .into_iter()
        .take(crate::widget::MINIMAP_ENGINE_CHILDREN)
        .enumerate()
    {
        let path = if i + 1 == crate::widget::MINIMAP_ENGINE_CHILDREN {
            player
        } else {
            arrow
        };
        if let Some(frame) = model.arena.frame_mut(child) {
            if let KindState::Model(state) = &mut frame.kind_state {
                state.path = Some(path.to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "GetZoom",
        // Reads whichever index is live — the indoor one (`0x86f69c`) while inside a WMO, else the
        // outdoor one (`0x86f698`). The client's `get_zoom_index` routes on the same flag.
        lua.create_function(|lua, this: Table| with_minimap(lua, &this, |m| m.active_zoom()))?,
    )?;
    m.set(
        "SetZoom",
        lua.create_function(|lua, (this, zoom): (Table, f64)| {
            // The client's set_zoom clamps into [0, levels-1] (`0x6daa10`: clamp at 5). A negative
            // or fractional Lua number truncates like lua_tonumber → int. It writes the index the
            // inside flag selects, so the +/- buttons zoom the map you're actually looking at and
            // each mode keeps its own level.
            let clamped = (zoom.max(0.0) as u8).min(MINIMAP_ZOOM_LEVELS - 1);
            let inside = with_minimap(lua, &this, |m| {
                m.set_active_zoom(clamped);
                m.inside
            })?;
            // …and it persists the level in the same breath: `set_zoom` writes the live index AND
            // `CVar::Set`s the matching CVar (`minimapInsideZoom` indoors, `minimapZoom` out), which
            // is the whole reason a zoom level survives a restart. The borrow above is released
            // before this one — both reach the same `Model` app_data.
            super::cvars::set_from_engine(
                &mut lua.app_data_mut::<Model>().expect("model app_data"),
                if inside {
                    "minimapInsideZoom"
                } else {
                    "minimapZoom"
                },
                clamped.to_string(),
            );
            Ok(())
        })?,
    )?;
    m.set(
        // SetMaskTexture(path) — the disc's mask art. A real 1.12 method (the name is in the 5875
        // image) with no getter beside it, so this is write-only from Lua, exactly as there.
        //
        // An absent or empty path restores the engine default rather than leaving the map
        // unmasked: `SetMaskTexture` is how a UI replaces the circle, never how it removes one,
        // and a nil-means-square reading would hand every mistyped path a square minimap with no
        // error. (pfUI passes a real path; this is about the failure mode, not about pfUI.)
        "SetMaskTexture",
        lua.create_function(|lua, (this, path): (Table, Option<String>)| {
            let path = path.filter(|p| !p.is_empty());
            with_minimap(lua, &this, |m| m.mask_texture = path)
        })?,
    )?;
    m.set(
        "GetZoomLevels",
        lua.create_function(|lua, this: Table| {
            // Validate the receiver like every kind method (the constant is per the client's
            // `get_zoom_levels`, but a non-Minimap receiver is still a caller bug).
            with_minimap(lua, &this, |_| MINIMAP_ZOOM_LEVELS)
        })?,
    )?;
    m.set(
        "PingLocation",
        // The minimap click (our own `Minimap_OnClick`, and any addon that replaced it): centre-
        // relative offsets in **UI units**, x right / y up — exactly `GetCursorPosition()` minus
        // `Minimap:GetCenter()`, both of which are UI-space. Parked, not converted: the app owns
        // the view scale, and it drains this in the SAME frame it draws the map so the click
        // resolves against the geometry the player actually clicked on (decision 1596).
        //
        // The receiver is validated but the value lives on the model — one pending click, not one
        // per Minimap widget.
        lua.create_function(|lua, (this, x, y): (Table, f32, f32)| {
            with_minimap(lua, &this, |_| ())?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .minimap_ping_request = Some((x, y));
            Ok(())
        })?,
    )?;
    m.set(
        "GetPingPosition",
        // The ping's normalized offsets from the widget centre (fractions of the widget side,
        // x right / y up — the `MINIMAP_PING` event's own arg2/arg3 space), recomputed by the app
        // from the stored world point every frame. So a caller polling this while walking sees
        // the value MOVE, which is the whole point: the ping is pinned to the world, not to the
        // map.
        //
        // **Two numbers, always** (`0x4eefd0`): the reference
        // recomputes from statics nothing ever clears, and the stock `Minimap_OnUpdate` feeds the
        // answer straight into `x * Minimap:GetWidth()` for the whole of its 5 s timer — a nil
        // here is a Lua error on every frame of that window (1974).
        lua.create_function(|lua, this: Table| {
            with_minimap(lua, &this, |_| ())?;
            let (x, y) = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap_ping;
            Ok((x, y))
        })?,
    )?;
    lua.set_named_registry_value(REG_MINIMAP_METHODS, m)?;
    Ok(())
}
