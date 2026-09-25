//! The minimap ping, engine-owned and pinned to a world point.
//!
//! The world `(x, y)` is the only stored position: the stock `Minimap_OnUpdate` re-seats the
//! `MiniMapPing` frame every frame from `GetPingPosition()`, the normalized offset
//! [`drive_minimap_ping`] republishes against the live view radius, so the marker follows the pan
//! and the zoom with no second copy to fall out of step.
//!
//! - In: the stock `Minimap_OnClick` calls `Minimap:PingLocation(dx, dy)` in UI units from the
//!   centre; the renderer converts it with the geometry of the frame it draws: UI units × the seam
//!   scale = window px, ÷ `px_per_yd` = yards.
//! - Across: our ping sends `MSG_MINIMAP_PING` (raw world floats, relayed to the rest of the group
//!   only, `GroupHandler.cpp:384-391`); a member's seats the same way. A ping seats locally at
//!   click time, so a solo ping works.
//! - Out: `MINIMAP_PING (unitToken, nx, ny)` fires with the relay `0x4ee330`'s offsets
//!   `(−dy·k, dx·k)`, `k = 1/(2·radius)`; `Minimap:GetPingPosition()` reads them back.
//!
//! The reference keeps the world point in two statics nothing clears (`0xbc787c`/`0xbc7880`), and
//! the stock `Minimap.lua` owns everything visible: it shows the `MiniMapPing` `<Model>`
//! (`Interface\MiniMap\Ping\MinimapPing.mdx`, rendered by `crate::ui_models`), holds it 5 s and
//! fades it 0.5 s (`MINIMAPPING_TIMER`, `MINIMAPPING_FADE_TIMER`). `WorldMapPing` is the same model
//! on the world map.
//!
//! Reaching a ping does not clear it: the reference's 10-yd `d² < 100` auto-clear belongs to the
//! `SMSG_GOSSIP_POI` marker (`0x6d99aa`–`0x6d9a4c`).

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_ui::script::{ScriptValue, UiScript};

use super::blips::BlipCtx;
use crate::net::{ClientCommand, Guid, NetCommands, SelfPlayer};
use crate::player::Player;

/// The stored ping, the reference's two statics: one, never cleared, with no map tag (after a
/// worldport the old point re-projects and the stock Lua's disc test hides it).
struct LivePing {
    /// The WoW `(x, y)` this ping marks; the screen seat is re-derived from it every frame.
    world: (f32, f32),
    /// The pinger's guid, `0` for ourselves: the event's unit token, and whether it is sent.
    sender: u64,
}

/// The engine-owned ping, seated by a click or a group member's `MSG_MINIMAP_PING`, announced by
/// [`drive_minimap_ping`] and drawn by the stock `MiniMapPing` frame.
#[derive(Resource, Default)]
pub(crate) struct MinimapPing {
    live: Option<LivePing>,
    /// Seated since the last [`drive_minimap_ping`]: owes a `MINIMAP_PING` event, and the wire send
    /// if ours.
    fresh: bool,
}

impl MinimapPing {
    /// Seat a ping at a world point, replacing the live one, as the reference's re-ping does.
    pub(crate) fn seat(&mut self, world: (f32, f32), sender: u64) {
        self.live = Some(LivePing { world, sender });
        self.fresh = true;
    }
}

/// A `Minimap:PingLocation(x, y)` click to the world point it names: `ui` is centre-relative UI
/// units (x right, y up), `seam` window px per UI unit ([`crate::ui_script::seam_scale`]), `ctx`
/// the map as drawn this frame. The inverse of [`BlipCtx::offset`]: screen right is −WoW y, screen
/// up +WoW x. `None` outside the disc, the stock test (`Minimap.lua:135`) in yards.
fn click_to_world(ctx: &BlipCtx, ui: (f32, f32), seam: f32) -> Option<(f32, f32)> {
    if ctx.px_per_yd <= 0.0 || seam <= 0.0 {
        return None;
    }
    let right_yd = ui.0 * seam / ctx.px_per_yd;
    let up_yd = ui.1 * seam / ctx.px_per_yd;
    if right_yd.hypot(up_yd) >= ctx.radius_yd {
        return None;
    }
    Some((ctx.wx + up_yd, ctx.wy - right_yd))
}

/// Seat this frame's `Minimap:PingLocation` click inside the renderer, against the geometry the
/// map drew this frame, so a click is never converted at a stale scale.
pub(super) fn seat_click(ctx: &BlipCtx, ping: &mut MinimapPing, click: Option<(f32, f32)>) {
    if let Some(world) = click.and_then(|c| click_to_world(ctx, c, ctx.seam)) {
        ping.seat(world, 0);
    }
}

/// Republish the ping's normalized position and announce a fresh ping. Runs before the script
/// tick, so the `MINIMAP_PING` event and `Minimap:GetPingPosition()` land in the same tick.
pub(super) fn drive_minimap_ping(
    script: Option<bevy::ecs::system::NonSendMut<UiScript>>,
    mut ping: ResMut<MinimapPing>,
    player: Res<Player>,
    widget: Res<super::MinimapWidget>,
    inside: Res<super::MinimapInside>,
    group: Res<crate::ui_party::GroupState>,
    self_q: Query<&Guid, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else { return };
    let Some(live) = ping.live.as_ref() else {
        return;
    };

    // The relay's `(−dy·k, dx·k)`, `k = 1/(2·radius)`, recomputed every tick. With the map hidden
    // there is no live zoom, so the registered default stands in.
    let wow = bevy_to_wow(player.pos);
    let radius = super::view_radius_yd(
        widget
            .0
            .as_ref()
            .map_or(super::MINIMAP_DEFAULT_ZOOM, |s| s.zoom),
        widget
            .0
            .as_ref()
            .map_or(super::MINIMAP_DEFAULT_ZOOM, |s| s.inside_zoom),
        inside.0,
    );
    let k = 1.0 / (2.0 * radius);
    let norm = ((wow[1] - live.world.1) * k, (live.world.0 - wow[0]) * k);
    script.set_minimap_ping(norm);

    if !std::mem::take(&mut ping.fresh) {
        return;
    }
    let Some(live) = ping.live.as_ref() else {
        return;
    };
    // Ours goes on the wire only when grouped, as the reference's `PingLocation` (`0x4eeca0`)
    // sends; the local ping stands either way.
    if live.sender == 0 && group.in_group {
        let _ = commands.0.send(ClientCommand::MinimapPing {
            x: live.world.0,
            y: live.world.1,
        });
    }
    // The event's unit token: ourselves or the sender's party slot. An unresolved sender still
    // pings; the stock handler ignores arg1.
    let self_guid = self_q.iter().next().map(|g| g.0);
    let token = if live.sender == 0 || Some(live.sender) == self_guid {
        "player".to_string()
    } else {
        group
            .party_slots()
            .position(|m| m.guid == live.sender)
            .map_or_else(|| "party1".to_string(), |i| format!("party{}", i + 1))
    };
    script.fire_event(
        "MINIMAP_PING",
        vec![
            ScriptValue::Str(token),
            ScriptValue::Number(f64::from(norm.0)),
            ScriptValue::Number(f64::from(norm.1)),
        ],
    );
}

/// Register the ping's packet handler.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::MinimapPing,
        on_minimap_ping,
    );
}

/// A group member's ping: raw world floats, seated as the pin.
fn on_minimap_ping(In(ev): In<benilla_protocol::SessionEvent>, mut ping: ResMut<MinimapPing>) {
    if let benilla_protocol::SessionEvent::MinimapPing { guid, x, y } = ev {
        ping.seat((x, y), guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `BlipCtx` for a 140-px-side map at a 100-yd view radius, player at the origin.
    fn ctx() -> BlipCtx {
        let side = 140.0;
        let radius = 100.0;
        BlipCtx {
            center: Vec2::new(500.0, 200.0),
            side,
            px_per_yd: (side * 0.5) / radius,
            radius_yd: radius,
            z: 0,
            alpha: 1.0,
            wx: 0.0,
            wy: 0.0,
            wz: 0.0,
            cursor: None,
            cursor_ui: None,
            seam: 1.0,
        }
    }

    /// The click is in UI units and `px_per_yd` in window px: at 0.9 uiScale on a 1080p window the
    /// seam is ≈1.27, and a conversion that skips it seats every ping ≈27 % too far out.
    #[test]
    fn a_click_converts_through_the_seam_scale() {
        let c = ctx();
        let seam = 1080.0 / 768.0 * 0.9; // the shipped default at 1080p
                                         // 20 UI units right → 20·seam px → yards west (−y).
        let (x, y) = click_to_world(&c, (20.0, 0.0), seam).expect("inside the disc");
        let expect_yd = 20.0 * seam / c.px_per_yd;
        assert!((x - 0.0).abs() < 1e-3, "no northing from a due-east click");
        assert!(
            (y + expect_yd).abs() < 1e-3,
            "screen right is WoW −y (west): {y} vs {}",
            -expect_yd
        );
        // Without the seam multiply every click lands off by the same factor.
        let naive = 20.0 / c.px_per_yd;
        assert!(
            (expect_yd - naive).abs() > 5.0,
            "the seam is load-bearing, not a rounding difference"
        );
    }

    /// Screen up is WoW +x (north), [`BlipCtx::offset`]'s inverse, so a ping seated from a click
    /// draws back under the cursor.
    #[test]
    fn a_click_round_trips_through_the_blip_mapping() {
        let c = ctx();
        let ui = (18.0, -25.0);
        let world = click_to_world(&c, ui, 1.0).expect("inside the disc");
        let back = c.offset([world.0, world.1, 0.0]);
        // `offset` is y-DOWN screen space; the click was y-up.
        assert!((back.x - ui.0).abs() < 1e-3, "{back:?} vs {ui:?}");
        assert!((back.y + ui.1).abs() < 1e-3, "{back:?} vs {ui:?}");
    }

    /// The reference's own disc test, in yards: a click outside the map's radius is not a ping.
    #[test]
    fn a_click_outside_the_disc_is_no_ping() {
        let c = ctx();
        // The disc is 70 px of the 140-px side; 69 px in is a ping, 71 px out is not.
        assert!(click_to_world(&c, (69.0, 0.0), 1.0).is_some());
        assert!(click_to_world(&c, (71.0, 0.0), 1.0).is_none());
        assert!(
            click_to_world(&c, (50.0, 50.0), 1.0).is_none(),
            "the corner"
        );
    }

    /// The stored form is a world point, so walking moves the marker by exactly the player's
    /// displacement.
    #[test]
    fn the_marker_tracks_the_world_as_the_player_walks() {
        let mut c = ctx();
        let mut ping = MinimapPing::default();
        ping.seat((30.0, 0.0), 0); // 30 yd north of the player
        let live = ping.live.as_ref().unwrap();
        let before = c.offset([live.world.0, live.world.1, 0.0]);
        assert!(before.y < 0.0, "north draws UP the screen: {before:?}");
        // Walk 10 yd north. The ping is now 20 yd away, so it draws 10 yd closer to the centre.
        c.wx += 10.0;
        let after = c.offset([live.world.0, live.world.1, 0.0]);
        assert!(
            (after.y - (before.y + 10.0 * c.px_per_yd)).abs() < 1e-3,
            "{before:?} → {after:?}"
        );
    }

    /// No proximity clear: the 10-yd `d² < 100` auto-clear is the `SMSG_GOSSIP_POI` marker's
    /// (`0x6d99aa`–`0x6d9a4c`), and the ping's statics are never cleared. A frame with no click
    /// seats nothing over it.
    #[test]
    fn reaching_the_ping_does_not_clear_it() {
        let mut c = ctx();
        let mut ping = MinimapPing::default();
        ping.seat((1.0, 1.0), 0);
        // Walk onto the point: the pin stands.
        c.wx = 1.0;
        c.wy = 1.0;
        seat_click(&c, &mut ping, None);
        let live = ping.live.as_ref().expect("a reached ping is still a ping");
        assert_eq!(live.world, (1.0, 1.0));
    }

    /// A click seats a fresh pin at the clicked world point, replacing the last one; the
    /// announcement flag rides the seat.
    #[test]
    fn a_click_seats_a_fresh_pin() {
        let c = ctx();
        let mut ping = MinimapPing::default();
        ping.seat((5.0, 5.0), 0);
        ping.fresh = false;
        seat_click(&c, &mut ping, Some((0.0, 20.0))); // 20 UI units up = 20 px = 28.57 yd north
        let live = ping.live.as_ref().unwrap();
        assert!((live.world.0 - 20.0 / c.px_per_yd).abs() < 1e-3 && live.world.1.abs() < 1e-3);
        assert!(ping.fresh, "a seat owes the world its event");
    }

    /// A click before the map has drawn (no scale) is dropped, not seated at a garbage point.
    #[test]
    fn a_click_before_the_map_has_drawn_is_dropped() {
        let mut c = ctx();
        c.px_per_yd = 0.0;
        assert!(click_to_world(&c, (10.0, 10.0), 1.0).is_none());
    }
}
