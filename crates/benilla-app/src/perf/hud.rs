//! The standing dev HUD: the cost pill, and nothing else.
//!
//! The pill is fps (dim) and CPU cost per frame, drawn from a 4 Hz snapshot of the meters, which
//! keep sampling every frame. It is laid onto the player-UI quad pass, never egui: drawing
//! through egui wakes its whole per-frame pipeline and a full-screen compositing camera.
//!
//! The HUD owns no settings: it is `#[cfg(feature = "dev")]`, so nothing a player must reach can
//! live here. `$WOW_NOVSYNC=1` is the measurement knob for vsync.

use benilla_ui::script::{JustifyH, JustifyV, Outline, UiScript};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{PrimaryWindow, Window};

use super::stats::FrameStats;
use crate::ui_pass::{UiQuad, UiQuads};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, UiFontAtlas};

// ---- The quad pill, drawn on the player-UI pass. ---------------------------------------------
// Client-space sRGB for [`UiQuad::color`]: black alpha 224 fill, gray 235 text, gray 180 dim.
const Q_FILL: [f32; 4] = [0.0, 0.0, 0.0, 224.0 / 255.0];
const Q_TEXT: [f32; 4] = [0.92, 0.92, 0.92, 1.0];
/// `|cAARRGGBB` markup for the dim fps run (gray 180).
const Q_DIM_MARKUP: &str = "|cffb4b4b4";
/// Paint order: the background under the glyphs, both above every packed WoW z_key, whose frame
/// tuples never fill the top bits.
const Z_PILL_BG: u64 = u64::MAX - 1;
const Z_PILL: u64 = u64::MAX;
/// The quad pill's font height (logical px) and box padding.
const PILL_QUAD_PX: f32 = 12.0;
const PILL_PAD: Vec2 = Vec2::new(9.0, 4.0);
/// Top-centre offset, where the pill sits when nothing else claims that band.
const PILL_TOP: f32 = 8.0;
/// The gap the pill leaves under whatever game UI it is stepping below.
const PILL_YIELD_GAP: f32 = 4.0;

/// How often the drawn snapshot advances; between refreshes [`pill_quads`] reuses its cache.
const HUD_REFRESH_SECS: f32 = 0.25;

/// HUD state. The dev chord + `P` (Ctrl+Shift+P) toggles `visible`; it starts hidden unless
/// `WOW_PERF_HUD=1`. The capture harness ([`crate::capture`]) forces it off for UI-free shots.
#[derive(Resource)]
pub(crate) struct PerfHud {
    pub(crate) visible: bool,
    /// The meters as of the last [`HUD_REFRESH_SECS`] tick, the view the pill draws.
    snap: FrameStats,
    /// The clock `snap` was taken at, which is also the refresh timer.
    snap_at: f32,
    /// How far down the top-centre band the game UI reaches ([`top_centre_claimed`]).
    top_claimed: f32,
}

impl Default for PerfHud {
    fn default() -> Self {
        Self {
            visible: std::env::var("WOW_PERF_HUD").as_deref() == Ok("1"),
            snap: FrameStats::default(),
            // So the first frame refreshes rather than drawing an empty snapshot.
            snap_at: f32::NEG_INFINITY,
            top_claimed: 0.0,
        }
    }
}

impl PerfHud {
    /// Advance the snapshot if it is older than [`HUD_REFRESH_SECS`]; `true` when it did.
    fn maybe_refresh(&mut self, stats: &FrameStats, now: f32) -> bool {
        if now - self.snap_at < HUD_REFRESH_SECS {
            return false;
        }
        self.snap = stats.clone();
        self.snap_at = now;
        true
    }

    /// The pill's top edge: its usual seat, pushed below anything claiming the band.
    fn pill_top(&self) -> f32 {
        PILL_TOP.max(self.top_claimed + PILL_YIELD_GAP)
    }
}

pub(super) fn toggle_hud(keys: Res<ButtonInput<KeyCode>>, mut hud: ResMut<PerfHud>) {
    // The dev chord, not a bare `P`, which is the reference's TOGGLESPELLBOOK; a chord cannot be
    // typed text, so it needs no EditBox gate.
    if benilla_world::modkeys::dev_chord(&keys, KeyCode::KeyP) {
        hud.visible = !hud.visible;
    }
}

/// Advance the HUD's 4 Hz snapshot, independent of whether anything drew, and re-ask
/// [`top_centre_claimed`] on the same cadence.
pub(super) fn refresh_hud_snapshot(
    mut hud: ResMut<PerfHud>,
    stats: Res<FrameStats>,
    time: Res<Time<Real>>,
    script: Option<NonSend<UiScript>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if !hud.visible {
        return;
    }
    let now = time.elapsed_secs();
    if !hud.maybe_refresh(&stats, now) {
        return;
    }
    hud.top_claimed = match (script, windows.single()) {
        (Some(script), Ok(win)) => top_centre_claimed(&script, win.height()),
        _ => 0.0,
    };
}

/// How far down the top-centre band the game UI reaches, in window logical px (`0.0` when
/// clear). The always-up world-state readout (`WorldStateAlwaysUpFrame`, stock
/// `WorldStateFrame.xml`) shares the band, and the dev pill is the one that yields.
///
/// The layout answers in UI units, a screen `768/uiScale` units tall whatever the window, while
/// the pill draws in window px, so the chunk returns a fraction of the screen and the caller
/// scales it by the window height. Reading an edge settles the layout, one graph solve at most.
pub(crate) fn top_centre_claimed(script: &UiScript, win_h: f32) -> f32 {
    // `WorldStateAlwaysUpFrame` is always shown; the readout occupies its `AlwaysUpFrame<n>` rows,
    // built on demand and hidden when empty, so the claim is the lowest shown row's bottom.
    const CHUNK: &str = r#"
        local f = WorldStateAlwaysUpFrame
        if not (f and f:IsVisible()) then return -1 end
        local lowest, i = nil, 1
        while true do
            local r = getglobal("AlwaysUpFrame" .. i)
            if not r then break end
            if r:IsShown() then
                local b = r:GetBottom()
                if b and (not lowest or b < lowest) then lowest = b end
            end
            i = i + 1
        end
        local screen = GetScreenHeight()
        if not lowest or not screen or screen <= 0 then return -1 end
        return (screen - lowest) / screen
    "#;
    let frac: f32 = script.eval::<f64>(CHUNK).unwrap_or(-1.0) as f32;
    if !(0.0..=1.0).contains(&frac) {
        return 0.0;
    }
    frac * win_h
}

/// The pill as ~20 quads on the player-UI pass, a cached Vec clone per frame; glyphs are laid
/// out again only when the snapshot ticks, the window resizes or the seat moves.
pub(super) fn pill_quads(
    hud: Res<PerfHud>,
    atlas: Option<Res<UiFontAtlas>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut quads: ResMut<UiQuads>,
    mismatch: Option<Res<super::BlendMismatchShared>>,
    mut cache: Local<Option<PillCache>>,
) {
    if !hud.visible {
        return;
    }
    let (Some(atlas), Ok(win)) = (atlas, windows.single()) else {
        return; // headless
    };
    let win_w = win.width();
    let top = hud.pill_top();
    // Draws that bound a blend state contradicting their material (`perf::blend_check`), shown
    // red the frame it happens.
    let mismatch = mismatch.map_or(0, |m| m.0.load(std::sync::atomic::Ordering::Relaxed));
    let stale = !matches!(
        &*cache,
        Some(c) if c.snap_at == hud.snap_at && c.win_w == win_w && c.top == top && c.mismatch == mismatch
    );
    if stale {
        let cpu = hud.snap.cpu.mean();
        let main = hud.snap.main.mean();
        let fps = hud.snap.fps();
        // "59 fps  7.0 ms  17.3 cpu": fps dim, the main thread's ms in full, and the all-thread
        // sum dim, so the sum is not read as a frame time.
        let mut text = match (main, cpu) {
            (Some(main), Some(cpu)) => {
                format!("{Q_DIM_MARKUP}{fps:.0} fps|r  {main:.1} ms  {Q_DIM_MARKUP}{cpu:.1} cpu|r")
            }
            (None, Some(cpu)) => format!("{Q_DIM_MARKUP}{fps:.0} fps|r  {cpu:.1} cpu"),
            _ => format!("{Q_DIM_MARKUP}-- ms"),
        };
        if mismatch > 0 {
            text.push_str(&format!("  |cffff5050blend x{mismatch}|r"));
        }
        let center = Vec2::new(win_w * 0.5, 0.0); // measured first, then shifted under PILL_TOP
        let mut e = atlas.lock();
        let glyphs = layout_text_quads(
            &mut e,
            &text,
            Rect::from_center_size(center, Vec2::ZERO),
            Q_TEXT,
            Justify {
                h: JustifyH::Center,
                v: JustifyV::Middle,
            },
            Z_PILL,
            FontSpec {
                path: None,
                height: Some(PILL_QUAD_PX),
                outline: Outline::None,
                alpha_gradient: None,
            },
            // Our own dev overlay: measured off a degenerate rect, then shifted into the pill.
            crate::ui_text::TextSeat::Exact,
        );
        drop(e);
        let bounds = glyphs
            .iter()
            .map(|q| q.rect)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::from_center_size(center, Vec2::ZERO));
        let strip = Rect::new(
            bounds.min.x - PILL_PAD.x,
            bounds.min.y - PILL_PAD.y,
            bounds.max.x + PILL_PAD.x,
            bounds.max.y + PILL_PAD.y,
        );
        // Measured about y=0; seat the whole strip at the pill's top edge.
        let dy = top - strip.min.y;
        let shift = |r: Rect| Rect::new(r.min.x, r.min.y + dy, r.max.x, r.max.y + dy);
        let strip = shift(strip);
        let mut out = vec![UiQuad {
            rect: strip,
            z_key: Z_PILL_BG,
            color: Q_FILL,
            ..Default::default()
        }];
        for mut q in glyphs {
            q.rect = shift(q.rect);
            out.push(q);
        }
        *cache = Some(PillCache {
            snap_at: hud.snap_at,
            win_w,
            top,
            mismatch,
            quads: out,
        });
    }
    let Some(c) = &*cache else { return };
    quads.overlays.extend(c.quads.iter().cloned());
}

/// [`pill_quads`]' cache: the laid-out pill, valid for one snapshot tick, window width and seat.
pub(super) struct PillCache {
    snap_at: f32,
    win_w: f32,
    top: f32,
    /// The blend-mismatch count the text was laid out with (`perf::blend_check`).
    mismatch: u64,
    quads: Vec<UiQuad>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::ScriptValue;

    use crate::ui_script::world_state_tests::{harness, push, row};

    /// Pins [`top_centre_claimed`] against the stock XML. It lives here, not beside the readout,
    /// because a test of a dev instrument belongs in a dev root.
    #[test]
    fn the_readout_tells_the_dev_pill_how_much_of_the_top_it_uses() {
        benilla_formats::wow_data_or_skip!();
        // Not 768: a window height other than the UI's virtual one catches mixed units.
        const SCREEN_H: f32 = 900.0;
        let mut s = harness();
        s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Str("".into())]);
        s.resolve();
        assert_eq!(
            top_centre_claimed(&s, SCREEN_H),
            0.0,
            "hidden readout claims nothing — the pill keeps its usual seat"
        );

        push(
            &mut s,
            vec![
                row(
                    "Interface\\WorldStateFrame\\AllianceTower",
                    "Towers Controlled: 3",
                    "Alliance Towers Controlled",
                ),
                row(
                    "Interface\\WorldStateFrame\\HordeTower",
                    "Towers Controlled: 1",
                    "Horde Towers Controlled",
                ),
            ],
        );
        s.resolve();
        let claimed = top_centre_claimed(&s, SCREEN_H);
        assert!(
            claimed > 8.0,
            "two rows reach past the pill's own seat, so the pill must move: {claimed}"
        );
        // The second (lowest) row's resolved bottom, read from the stock layout.
        let expected: f32 = s
            .eval::<f64>(
                "return (GetScreenHeight() - AlwaysUpFrame2:GetBottom()) / GetScreenHeight()",
            )
            .expect("the frame's own numbers") as f32
            * SCREEN_H;
        assert!(
            (claimed - expected).abs() < 0.5,
            "the claim is the readout's resolved bottom: {claimed} vs {expected}"
        );

        push(&mut s, Vec::new());
        s.resolve();
        assert_eq!(
            top_centre_claimed(&s, SCREEN_H),
            0.0,
            "and it hands the band back when the scope empties"
        );
    }

    #[test]
    fn the_snapshot_advances_on_the_interval_not_per_frame() {
        let mut live = FrameStats::default();
        let frames: Vec<_> = (0..60).map(|_| (16.6, 20.0)).collect();
        let t = live.feed_frames(&frames, 0.0, 1.0 / 60.0);

        let mut hud = PerfHud::default();
        assert!(hud.maybe_refresh(&live, t));
        assert_eq!(
            hud.snap.wall.len(),
            60,
            "the first refresh adopts the meters"
        );
        assert_eq!(hud.snap_at, t);

        let t2 = live.feed_frames(&[(40.0, 20.0)], t, 1.0 / 60.0);
        assert!(
            !hud.maybe_refresh(&live, t2),
            "inside the interval, no refresh"
        );
        assert_eq!(
            hud.snap.wall.len(),
            60,
            "one frame later — inside the interval — the view holds still"
        );

        assert!(hud.maybe_refresh(&live, t + HUD_REFRESH_SECS));
        assert_eq!(
            hud.snap.wall.len(),
            61,
            "past the interval the view catches up"
        );
    }

    #[test]
    fn the_pill_steps_below_whatever_claims_the_top_centre() {
        let mut hud = PerfHud::default();
        assert_eq!(hud.pill_top(), PILL_TOP, "clear band — the usual seat");

        // The readout is up and reaches 81 px down (its anchor plus three rows).
        hud.top_claimed = 81.0;
        assert_eq!(hud.pill_top(), 81.0 + PILL_YIELD_GAP);

        // A claim shallower than the pill's own seat moves nothing.
        hud.top_claimed = 1.0;
        assert_eq!(hud.pill_top(), PILL_TOP);
    }
}
