//! The minimap **ping** (decision 1596; the feature 0471 paused and this brings back).
//!
//! ## The pin
//!
//! A ping marks a **place in the world**, so a world point `(x, y)` is the only thing this module
//! stores. Where it lands on screen is *derived* — by the stock `Minimap_OnUpdate`, which
//! re-seats the `MiniMapPing` frame every frame from `GetPingPosition()`, the normalized offset
//! [`drive_minimap_ping`] republishes from the pin against the live view radius. It therefore
//! cannot drift from the map, cannot lag the pan by a frame, and cannot survive a zoom change at
//! the old scale: there is no second copy of the position to fall out of step with.
//!
//! That is the whole difference from the first attempt (decision 0453 / 0471), which stored the
//! world point in the engine but drew the marker from **Lua** off a stale push; 1596 §2 has the
//! autopsy.
//!
//! ## The three legs
//!
//! - **In** — a click reaches Lua's `Minimap_OnClick` (the stock one, hookable: the corpus's
//!   `CleanMinimap` replaces that global outright), which calls `Minimap:PingLocation(dx, dy)`
//!   with centre-relative offsets in **UI units**. [`seat_click`]'s caller drains it in the *same
//!   frame it draws the map*, converting through that frame's own geometry: UI units × the 0582
//!   seam scale = window px, ÷ `px_per_yd` = yards. (Skipping that seam multiply is what put the
//!   first version's ping ~27 % too far from the player at 1080p.)
//! - **Across** — our own ping sends `MSG_MINIMAP_PING` (raw world floats; the server relays them
//!   verbatim to the rest of the group and nowhere else). A group member's arrives through the
//!   session event and seats the same way. A ping is seated **locally at click time**, never
//!   waited for off the wire: vanilla pings work solo.
//! - **Out** — `MINIMAP_PING (unitToken, nx, ny)` fires for addons, with the same normalized
//!   offsets the relay `0x4ee330` hands Lua (`(−dy·k, dx·k)`, `k = 1/(2·radius)`).
//!   `Minimap:GetPingPosition()` reads the live value back.
//!
//! ## Lifetime and pixels — the stock `Minimap.lua`'s and its `<Model>`'s, not ours
//!
//! The reference splits the ping in two: the engine stores the world point in a pair of statics
//! **nothing ever clears** (`0xbc787c`/`0xbc7880` — six instructions touch those cells
//! and the only zeroing is a CRT initializer), and FrameXML owns everything visible — the
//! `MiniMapPing` `<Model>` it shows on `MINIMAP_PING`, re-seats every frame from
//! `GetPingPosition()`, holds 5 s (`MINIMAPPING_TIMER`), "fades" 0.5 s through a `SetAlpha(255·t)`
//! that clamps to full until the last ~2 ms, and hides. Since 1751's swap that file runs here
//! verbatim (1974), and since decision 2013 the `<Model>` it shows **renders its own file**
//! (`Interface\MiniMap\Ping\MinimapPing.mdx`, through `crate::ui_models`): the spinner on its
//! global-sequence clock, the static centre, the ring on the looping Stand — the model's own
//! bones, weight tracks and additive quads, on the pane's private clock that runs only while the
//! frame is shown (decision 2007). The sprite this module used to draw in their place (1596/1599's
//! byte-measured re-expression of those quads) is gone with it, and so is the world-map ping gap
//! 1980 named: `WorldMapPing` is the same file on the map sheet.
//!
//! What is *not* a lifetime: proximity. The first version applied the client's 10-yd
//! `d² < 100` auto-clear to the party ping, and that clear belongs to the **`SMSG_GOSSIP_POI`
//! marker** — a different feature in a different slot (`0x6d99aa`–`0x6d9a4c`). Walking to your
//! own ping used to delete it mid-hold.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_ui::script::{ScriptValue, UiScript};

use super::blips::BlipCtx;
use crate::net::{ClientCommand, Guid, NetCommands, SelfPlayer};
use crate::player::Player;

/// The stored ping — the reference's two statics. One, never cleared: the reference keeps no
/// list, and no map tag either (a worldport mid-ping re-projects the old point against the new
/// map's player position, which the stock Lua's disc test then hides — reproduced as is).
struct LivePing {
    /// **The pin**: the WoW `(x, y)` this ping marks. The only stored position; the screen seat is
    /// re-derived from it every frame.
    world: (f32, f32),
    /// The pinger's guid, `0` = ourselves — resolved to the `MINIMAP_PING` event's unit token, and
    /// the test for "this is ours, put it on the wire".
    sender: u64,
}

/// The engine-owned ping state (decision 1596). Seated by a click (drained in the renderer, with
/// that frame's geometry) or by a group member's `MSG_MINIMAP_PING`; announced by
/// [`drive_minimap_ping`]; drawn by the stock `MiniMapPing` frame for exactly as long as that
/// frame shows itself.
#[derive(Resource, Default)]
pub(crate) struct MinimapPing {
    live: Option<LivePing>,
    /// A ping seated since the last [`drive_minimap_ping`] — it still owes the world an outbound
    /// `MSG_MINIMAP_PING` (if it is ours) and a `MINIMAP_PING` event (either way).
    fresh: bool,
}

impl MinimapPing {
    /// Seat a ping at a world point. Re-pinging replaces: the reference tolerates the same, and a
    /// group echo of our own click lands on the spot we already drew (`Minimap_SetPing` twice on
    /// one spot just restarts the timer).
    pub(crate) fn seat(&mut self, world: (f32, f32), sender: u64) {
        self.live = Some(LivePing { world, sender });
        self.fresh = true;
    }
}

/// Convert a `Minimap:PingLocation(x, y)` click into the world point it names, drain-side.
///
/// `ui` is centre-relative in **UI units** (x right, y up — `GetCursorPosition()`'s space);
/// `seam` is window px per UI unit ([`crate::ui_script::seam_scale`]), and `ctx` is the geometry
/// of the map **as drawn this frame**. The mapping is [`BlipCtx::offset`]'s inverse: screen right
/// = −WoW y (west), screen up = +WoW x (north).
///
/// `None` when the click is outside the disc — the reference's `Minimap_OnClick` makes the same
/// test in Lua (`sqrt(x² + y²) < width/2`), stated here in yards because that is the space the
/// answer lives in.
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

/// Seat this frame's `Minimap:PingLocation` click — inside the renderer, against the geometry
/// the player actually clicked on and the map actually drew at.
///
/// The seat happens here rather than in a system of its own precisely so there is no window in
/// which a click is held against a *stale* view scale: the first version parked the click for a
/// separate system that read the scale the renderer had left behind on the previous frame, and
/// dropped the click outright whenever that leftover was still zero. (The *drain* is the caller's,
/// one step earlier, so the click is spent even on a frame that draws no map — see there.)
pub(super) fn seat_click(ctx: &BlipCtx, ping: &mut MinimapPing, click: Option<(f32, f32)>) {
    if let Some(world) = click.and_then(|c| click_to_world(ctx, c, ctx.seam)) {
        ping.seat(world, 0);
    }
}

/// Announce a fresh ping and republish the position — everything that is *not* geometry.
///
/// Runs before the script tick so the `MINIMAP_PING` event and the position behind
/// `Minimap:GetPingPosition()` land in the same tick, and so an addon's handler sees a ping that
/// is already seated (the renderer seated it at the end of the previous frame).
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

    // The normalized offsets, recomputed from the pin every tick against the live view radius —
    // the byte-verified relay's own `(−dy·k, dx·k)`, `k = 1/(2·radius)`. With the map hidden there
    // is no live index to read (the extract publishes no slot), so the event's numbers fall back
    // to the registered default zoom: an addon still hears the ping, at the scale the map would
    // have if it were up.
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
    // Ours goes on the wire — raw world floats — but only when there is a group to relay them to.
    // The reference gates its send the same way (`PingLocation` `0x4eeca0` sends only when
    // grouped, VERIFIED) while still pinging locally, which is why a solo ping works at all and
    // why the marker is drawn at click time rather than awaited off the wire.
    if live.sender == 0 && group.in_group {
        let _ = commands.0.send(ClientCommand::MinimapPing {
            x: live.world.0,
            y: live.world.1,
        });
    }
    // The event's unit token: ourselves, or the sender's party slot. A sender we cannot resolve
    // (they left the group mid-flight) still pings — the reference's own Lua ignores arg1.
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

/// Register the ping's packet handler — called from [`super::MinimapPlugin`] (in the net handler
/// table since 2313).
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::MinimapPing,
        on_minimap_ping,
    );
}

/// A group member pinged (decision 1596). The wire carries raw world floats and the relay is
/// stateless in the reference too — we seat them as the pin and the minimap derives the rest.
/// The server only relays a ping between people who are grouped, and a ping from another map
/// would be dropped by the renderer's own map test anyway.
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

    /// **The first version's ping landed in the wrong place** (decision 1596 §2.1): the click
    /// arrives in UI units and the map's `px_per_yd` is in *window* px, and it divided one by the
    /// other. At the shipped default (0.9 uiScale on a 1080p window) the seam is ≈1.27, so every
    /// ping seated ≈27 % further from the player than the player clicked — worse the further out
    /// you clicked, which is exactly what "it pings somewhere else" looks like.
    #[test]
    fn a_click_converts_through_the_seam_scale() {
        let c = ctx();
        let seam = 1080.0 / 768.0 * 0.9; // the shipped default at 1080p
                                         // 20 UI units right of centre → 20·seam window px → ÷ px_per_yd yards WEST (−y).
        let (x, y) = click_to_world(&c, (20.0, 0.0), seam).expect("inside the disc");
        let expect_yd = 20.0 * seam / c.px_per_yd;
        assert!((x - 0.0).abs() < 1e-3, "no northing from a due-east click");
        assert!(
            (y + expect_yd).abs() < 1e-3,
            "screen right is WoW −y (west): {y} vs {}",
            -expect_yd
        );
        // The bug: dropping the seam multiply shortens every click by the same factor.
        let naive = 20.0 / c.px_per_yd;
        assert!(
            (expect_yd - naive).abs() > 5.0,
            "the seam is load-bearing, not a rounding difference"
        );
    }

    /// Screen up is WoW +x (north) — [`BlipCtx::offset`]'s inverse, so a ping seated from a click
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

    /// **The pin.** The stored form is a world point, so walking moves the marker across the map
    /// by exactly the player's displacement — no re-seating, no second copy to drift.
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

    /// **No proximity clear** (decision 1596 §2.2). The first version applied the client's 10-yd
    /// `d² < 100` auto-clear to the party ping; that clear belongs to the `SMSG_GOSSIP_POI` marker
    /// (`0x6d99aa`–`0x6d9a4c`), and `MSG_MINIMAP_PING` has no C-side storage to clear at all.
    /// Standing on your own ping must not delete it — and a frame with no click seats nothing
    /// over it.
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

    /// A degenerate frame (the widget has not drawn yet) drops the click rather than seating a
    /// ping at a garbage point — and, unlike the first version, that is the *only* case in which
    /// a click is dropped for want of a scale.
    #[test]
    fn a_click_before_the_map_has_drawn_is_dropped() {
        let mut c = ctx();
        c.px_per_yd = 0.0;
        assert!(click_to_world(&c, (10.0, 10.0), 1.0).is_none());
    }
}
