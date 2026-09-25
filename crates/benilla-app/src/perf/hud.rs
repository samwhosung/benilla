//! The standing dev HUD: the cost pill, and nothing else.
//!
//! **The pill is two small numbers** (1448 pared it back; 1454 made it the whole HUD; 1455
//! dropped the spike arrow). fps dim — the familiar anchor, and by construction the number that
//! cannot see cost — then process-CPU cost per frame, the meter vsync cannot rail; both
//! under-size, because the pill sits over the game all session. Anything deeper is an
//! instrument's job, not a panel's: the journal (`WOW_FPS_JOURNAL`), the probes, Tracy, the
//! `frame hitch` log line and the stall self-sampler. The expanded egui readout that used to
//! live behind a click is gone (1454) — it duplicated the instruments at a standing cost — and
//! the spike-latch arrow went the same way (1455, the director's call).
//!
//! **Drawn from a 4 Hz snapshot, not the live meters.** The pill renders from [`PerfHud::snap`];
//! the meters keep sampling every frame — only the *view* is quantized, and 250 ms is inside a
//! human read of a number.
//!
//! **The pill does not touch egui at all** (1453). Drawing anything through egui wakes the whole
//! lane — bevy_egui's per-frame pipeline plus a full-screen compositing camera — which the
//! director's live toggle priced at ~1 ms for a 20-glyph pill. So [`pill_quads`] lays the pill
//! onto the player-UI quad pass, whose append lane is rebuilt every frame anyway; since 1454 the
//! HUD has no egui surface left and [`crate::debug_panel`]'s 1445 gate no longer consults it.
//!
//! **The HUD does not own settings.** It briefly carried a VSync checkbox and an MSAA readout;
//! VSync is a player video option now ([`crate::video`], the Graphics page's `gxVSync` row) and
//! MSAA is a startup knob (`$WOW_MSAA`). An instrument reports — it does not double as the control
//! panel, and this one is `#[cfg(feature = "dev")]`, so anything a player must reach cannot live
//! here at all. The measurement knob the checkbox actually existed for is `$WOW_NOVSYNC=1`.

use benilla_ui::script::{JustifyH, JustifyV, Outline, UiScript};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{PrimaryWindow, Window};

use super::stats::FrameStats;
use crate::ui_pass::{UiQuad, UiQuads};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, UiFontAtlas};

// ---- The quad pill (1453): the standing readout, drawn on the player-UI pass. --------------
// The old egui overlay palette, restated as client-space sRGB floats for [`UiQuad::color`]
// (black alpha 224 fill; gray 235 text; gray 180 dim).
const Q_FILL: [f32; 4] = [0.0, 0.0, 0.0, 224.0 / 255.0];
const Q_TEXT: [f32; 4] = [0.92, 0.92, 0.92, 1.0];
/// `|cAARRGGBB` markup for the dim fps run (gray 180).
const Q_DIM_MARKUP: &str = "|cffb4b4b4";
/// Paint order: the pill background one under the glyphs, both above every packed WoW z_key
/// (frame tuples never fill the top bits).
const Z_PILL_BG: u64 = u64::MAX - 1;
const Z_PILL: u64 = u64::MAX;
/// The quad pill's font height (logical px) and box padding.
const PILL_QUAD_PX: f32 = 12.0;
const PILL_PAD: Vec2 = Vec2::new(9.0, 4.0);
/// Top-center offset — where the pill sits when nothing else claims that band.
const PILL_TOP: f32 = 8.0;
/// The gap the pill leaves under whatever game UI it is stepping below.
const PILL_YIELD_GAP: f32 = 4.0;

/// How often the drawn snapshot advances. Fast enough that the numbers read as live and a latched
/// badge appears within a perceptual beat; slow enough that between refreshes the pill's cached
/// quads are reused byte-identical (see [`pill_quads`]).
const HUD_REFRESH_SECS: f32 = 0.25;

/// HUD state. The **dev chord + `P`** (Ctrl+Shift+P) toggles `visible`, and it starts **hidden**
/// — the director's call, 2026-09-08: the pill sits over the game for the whole session and the
/// game is what a dev build is for looking at. It is an instrument you reach for, not furniture.
/// `visible` is `pub(crate)` so the capture harness ([`crate::capture`]) can force the overlay off
/// for pristine, UI-free screenshots.
///
/// **`WOW_PERF_HUD=1` starts it shown**, which is how the HUD gets priced — the knob kept its
/// meaning in both spellings when the default flipped, so `0` and unset are both hidden. 1370
/// records the open gap: every campaign anchor was measured on a binary that draws this overlay,
/// at a cost booked as "est 0.4–1.2 ms CPU + unquantified GPU" — an estimate, never a measurement,
/// because nothing could turn the fixture off without also changing the binary. One env var makes
/// it an interleaved A/B on *one* binary instead (a leg runner), so the constant baked into
/// every anchor becomes a number. The meters keep sampling either way: only the drawing stops,
/// which is the half being priced — and with the default flipped, the *unmeasured* leg is now the
/// one nobody is running.
#[derive(Resource)]
pub(crate) struct PerfHud {
    pub(crate) visible: bool,
    /// The snapshot the pill draws from — a copy of the meters taken every [`HUD_REFRESH_SECS`],
    /// so the strings it formats hold still between refreshes (the module doc's cost argument).
    /// The live [`FrameStats`] keeps sampling every frame; this is only the view.
    snap: FrameStats,
    /// The clock `snap` was taken at. Spike ages are computed against this, not the live clock,
    /// so they hold still with the rest of the view — and it doubles as the refresh timer.
    snap_at: f32,
    /// How far down the screen the top-centre band is already spoken for (window logical px from
    /// the top edge, `0.0` when it is clear) — see [`top_centre_claimed`]. The pill seats itself
    /// below this.
    top_claimed: f32,
}

impl Default for PerfHud {
    fn default() -> Self {
        Self {
            visible: std::env::var("WOW_PERF_HUD").as_deref() == Ok("1"),
            snap: FrameStats::default(),
            // −∞, so the very first frame refreshes rather than drawing an empty snapshot.
            snap_at: f32::NEG_INFINITY,
            top_claimed: 0.0,
        }
    }
}

impl PerfHud {
    /// Advance the snapshot if the current one is older than [`HUD_REFRESH_SECS`]; `true` when it
    /// did (the caller's gate for the rest of the refresh — see [`refresh_hud_snapshot`]).
    fn maybe_refresh(&mut self, stats: &FrameStats, now: f32) -> bool {
        if now - self.snap_at < HUD_REFRESH_SECS {
            return false;
        }
        self.snap = stats.clone();
        self.snap_at = now;
        true
    }

    /// Where the pill's top edge goes: its usual seat, pushed below anything already occupying the
    /// top-centre band.
    fn pill_top(&self) -> f32 {
        PILL_TOP.max(self.top_claimed + PILL_YIELD_GAP)
    }
}

pub(super) fn toggle_hud(keys: Res<ButtonInput<KeyCode>>, mut hud: ResMut<PerfHud>) {
    // The dev chord + `P`, not a bare `p` — `P` is the reference's TOGGLESPELLBOOK, and a dev
    // doesn't get to squat on a game binding. The chord can't be mistaken for typed
    // text, so unlike the old bare key it needs no chat-bar/EditBox gate.
    if benilla_world::modkeys::dev_chord(&keys, KeyCode::KeyP) {
        hud.visible = !hud.visible;
    }
}

/// Advance the HUD's 4 Hz snapshot — its own system (1453), kept separate so the sampling cadence
/// never depends on whether anything drew this frame. It also re-asks who else is using the pill's
/// band ([`top_centre_claimed`]), on the same cadence and for the same reason: once per view, not
/// once per frame.
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

/// How far down the top-centre band the **game UI** already reaches, in window logical px from the
/// top edge (`0.0` = clear).
///
/// The pill has sat at the top centre since 1453 because that band was empty. It is not empty
/// everywhere: the always-up world-state readout (`WorldStateAlwaysUpFrame` — the tower counters
/// and battleground scores) is anchored to the top centre too, and in Eastern
/// Plaguelands or a battleground the two drew straight through each other. **The dev instrument is
/// the one that yields** — the readout is the game, the pill is scaffolding — and it yields by
/// stepping below it rather than by moving house, because a 1.12 UI has no corner a standing
/// overlay can claim outright (player/target frames own the top left, buffs and the minimap the top
/// right, the action bar the bottom, the chat dock the bottom left).
///
/// **Asked of the frame, not recomputed from its numbers.** Its row pitch and anchor live in
/// the stock `WorldStateFrame.xml` (1972); mirroring them here would be two copies to keep in step, so
/// this reads the resolved edge the layout actually produced (`GetBottom`, y-up). One tiny chunk
/// at 4 Hz, only while the HUD is drawing — the same shape as [`crate::hover_log`]'s tooltip
/// probe.
///
/// **Two coordinate spaces meet here, and mixing them is the whole trap.** The layout answers in
/// WoW UI units — a screen that is always `768/uiScale` units tall whatever the window is
/// (decision 0582's seam) — while the pill lays itself out in window px. Subtracting one from the
/// other is right only in the single case where the seam scale happens to be 1 (a 768 px-tall
/// window at uiScale 1) and wrong by `windowH − 768` everywhere else — which on the director's
/// window seated the pill back on top of the readout's third row while every test at 768 passed.
/// So the chunk answers a **fraction of the screen**, which belongs to neither space, and the
/// caller multiplies by the window height it actually draws in.
///
/// **Its own cost, stated (1370's rule for this overlay):** an edge read settles the layout, so on
/// a frame where something has written anchors since the last resolve this pays one graph solve
/// (~47 µs on a 200-frame tree — `layout_methods::settle`'s own measurement), four times a second.
/// Every other frame it is a chunk load and a table lookup.
pub(crate) fn top_centre_claimed(script: &UiScript, win_h: f32) -> f32 {
    // The stock `WorldStateAlwaysUpFrame` is a permanently shown container (1972); what the
    // readout actually occupies is its ROWS — `AlwaysUpFrame<n>`, built on demand and hidden when
    // the scope admits nothing — so the claim is the lowest shown row's bottom, or nothing.
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

/// The pill as ~20 quads on the player-UI pass. The append lane is rebuilt every
/// frame anyway, so the marginal cost is the clone of a cached Vec — where the old egui pill woke
/// bevy_egui's whole pipeline plus a full-screen compositing camera (~1 ms on the director's live
/// toggle). Glyphs are laid out only when the snapshot ticks or the window resizes.
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
        return; // headless: nothing draws, nothing to price
    };
    let win_w = win.width();
    let top = hud.pill_top();
    // Draws this frame that bound a blend state contradicting their material (the additive
    // check, `perf::blend_check`): shown red the frame it happens, so a wrong halo on screen
    // and a non-zero count here are seen together.
    let mismatch = mismatch.map_or(0, |m| m.0.load(std::sync::atomic::Ordering::Relaxed));
    let stale = !matches!(
        &*cache,
        Some(c) if c.snap_at == hud.snap_at && c.win_w == win_w && c.top == top && c.mismatch == mismatch
    );
    if stale {
        let cpu = hud.snap.cpu.mean();
        let main = hud.snap.main.mean();
        let fps = hud.snap.fps();
        // One string, the dim runs via markup: "59 fps  7.0 ms  17.3 cpu" — fps dim, the MAIN
        // thread's ms in full text (the part of the frame the player feels), and the process-wide
        // sum dim at the end (every thread, the number a CPU % agrees with — decision 1954: a
        // raid read 17 on it at a solid 60 with the main thread at 7, and the sum was taken for
        // a frame time by everyone who looked at it).
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

/// [`pill_quads`]' cache: the laid-out pill, valid for one snapshot tick at one window width and
/// one seat (the seat moves when the game UI takes the band — [`top_centre_claimed`]).
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

    /// The readout and the dev cost pill both want the top centre, and before this they drew straight
    /// through each other (the pill is anchored 8 px down; the readout's first row starts at 15). The
    /// pill asks the frame itself how far down the band is spoken for ([`top_centre_claimed`]) and
    /// seats itself below that — so this pins the probe against the shipped XML: nothing claimed while
    /// the readout is empty, and the readout's real resolved bottom once it is up.
    ///
    /// **Why it lives here and not beside the frame it drives.** It was written in
    /// `ui_script/world_state_tests.rs`, next to the readout — but `perf` is compiled out of a player
    /// build, so the `player-tests` gate (`--no-default-features --lib`) could not
    /// compile that file, and gating the one test with `#[cfg(feature = "dev")]` trips `run_mode`'s
    /// dev-plane enforcer (1179: seam knowledge has exactly three addresses, and a gameplay module is
    /// not one of them). Both laws point the same way — a test of a dev instrument belongs in a dev
    /// root. It borrows the readout's own `#[cfg(test)]` helpers rather than copying them, so the XML
    /// it drives stays the shipped one.
    #[test]
    fn the_readout_tells_the_dev_pill_how_much_of_the_top_it_uses() {
        benilla_formats::wow_data_or_skip!();
        // NOT 768. The layout answers in WoW UI units — a screen that is always 768 units tall
        // whatever the window is — and the pill draws in window px, so a probe that
        // subtracts one from the other is right only when the two happen to coincide. Feeding a
        // window height that is NOT the virtual one is the whole point of this test: it is what the
        // director's client does, and the first version of this probe put the pill back on top of the
        // readout's third row there while passing every test at 768.
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
        // The frame's own geometry, read back the way the probe reads it: the second (lowest)
        // row's resolved bottom. Asserted against the stock XML rather than restated as constants.
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

    /// The view the HUD draws only advances on the refresh interval — between ticks it holds
    /// still, which is the entire cost argument (identical snapshots are what the quad cache can
    /// reuse) — and a refresh adopts the live meters wholesale.
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

    /// The pill's seat: its usual place while the top-centre band is clear, and below the game UI
    /// the moment something claims it — the always-up world-state readout is the one frame that
    /// shares this band, and before this the two drew through each other.
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
