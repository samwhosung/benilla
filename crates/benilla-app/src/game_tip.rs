//! The loading screen's **tip of the day**.
//!
//! `showGameTips`' only mention in 1.12's FrameXML is its options row and the tooltip string
//! "Uncheck this to hide the tip of the day in the load screens", so the whole feature is
//! engine-side: `CGlueMgr::EnterWorld` picks a row from `GameTips.dbc`, hands it to the loading
//! screen's text slot, and writes the *next* index back into the `gameTip` CVar.
//!
//! ## The selection law (`0x46b662`–`0x46b6e3`)
//!
//! **`gameTip` holds the NEXT index, not the one on screen.** The client reads it, shows *that*
//! row, then stores `read + 1`, so on-disk values run 1..=74 and never 0 — a client that stores the
//! index it just showed is off by one, and the reference's own `Config.wtf` (`SET gameTip "34"`) is
//! what a naive reading mis-anchors on.
//!
//! **The wrap is a clamp, not a modulo**: `if (i < 0 || i >= count) i = 0` at `0x46b682`–`0x46b68f`.
//! `count <= 0` bails. The walk is **sequential**, never random.
//!
//! Two guards sit in front of it. `showGameTips` off jumps past the `CVar::Set` as well as the
//! draw (`0x46b671`/`0x46b678`), so **turning tips off freezes the index** rather than advancing it
//! invisibly. And `[selChar+0x10a]` — a *client-synthesised* flag, set by `0x5b42a0` iff the
//! `SMSG_CHAR_ENUM` level byte arrived as `0` — suppresses the tip for a character the roster
//! reported at level zero. vmangos always sends a real level, so that arm is unreachable against
//! our server; it is honoured anyway because it costs one comparison.
//!
//! ## Where it draws
//!
//! **Only on the glue→world transition.** The setter `0x406630` (a seven-byte
//! `mov [0x882e10],ecx; ret`) has exactly one caller, and neither `SMSG_TRANSFER_PENDING` arm sets
//! it — so an in-world portal or worldport screen carries **no tip**. That is why this module hangs
//! off the loading screen's `world entry` raise.
//!
//! **It comes down with the screen, never with a raise.** `[0x882e10]`'s only other writer
//! image-wide is `0x407f2b`, inside the dismiss `0x407e80` — so the two edges are *pick at the
//! entry raise* and *clear at the dismiss*, and nothing that happens mid-load can take a tip away.
//! This module used to clear on every non-entry raise, which read the same for an in-world screen
//! (the tip was already down) and was wrong for exactly one case: the login snap
//! `SMSG_LOGIN_VERIFY_WORLD`, which benilla was treating as a fresh load, wiped the tip a server
//! round-trip into **every** world entry.
//!
//! The reference lays the text out **once per raise** and draws it every frame, between the
//! background quad and the progress bar. Its placement, in the screen's own `[0,1]` ortho:
//! `s = 515 / (a · 1024)` where `a = [0x832a4c]` is **aspect × 0.75** (not the aspect — the writer
//! `0x41ad10` multiplies by `0.75` first, and reading it as W/H inflates the wrap width by a third
//! at 16:9); position `(0.5 − s·0.5, 0.1 + yoff, 0)`; `justifyH = 0` (left); wrap width `s`; body
//! `0xd7c8c8c8` ARGB with an opaque black shadow at `(+0.001, −0.001)`.
//!
//! **Placing it in the 4:3 box is not an approximation of that — it is the same numbers.** The
//! reference's `[0,1]` ortho spans the whole window, so its `s` and `left` both carry `a`; ours are
//! percentages of a box that is `H` tall and `4H/3` wide. Multiply them out and the `a` cancels:
//!
//! ```text
//! ref wrap  = s·W          = 515·W / (a·1024)          = 515·H / (1024·0.75) = 0.6706·H
//! our wrap  = s₁·(4H/3)    = (515/1024)·(4H/3)                               = 0.6706·H
//! ref left  = (0.5 − s/2)·W                                   = 0.5·W − 0.3353·H
//! our left  = (W − 4H/3)/2 + (0.5 − s₁/2)·(4H/3)              = 0.5·W − 0.3353·H
//! ```
//!
//! …for **every** aspect, not just 4:3 — the reference's own wrap column is a constant multiple of
//! the window HEIGHT, which is exactly what a height-fit letterboxed box makes it. And `yoff` is
//! settled rather than dodged: `0x4066b9`'s `fcom; fnstsw; test ah,5; jp` takes the `(1 − a)·0.5`
//! arm **only for `a < 1`**, so on any window at least 4:3 wide the term is zero and the two
//! layouts agree bit for bit. Narrower than 4:3 they part — but so does the whole loading screen
//! there (our box overflows where the reference letterboxes top and bottom), and that is the
//! screen's difference to answer, not the tip's.
//!
//! `TIP_BOTTOM`'s `0.1` is not a round number either: `0x4066ee` builds it as `0.05·0.5 + 0.075`,
//! which is the bar border's own `halfH·0.5 + cy` — **the tip's baseline IS the top edge of the
//! progress bar**, so the block always sits directly on it.
//!
//! The `|cffffd100Tip:|r ` prefix and the trailing `\r\n` are **in the DBC data**, and the escapes
//! *are* interpreted (`0x5c28af`, flags bit `0x800` clear), so the gold "Tip:" is markup rather
//! than literal text — which is why the line is built through [`benilla_ui::markup`] rather than
//! drawn as one string.

use benilla_ui::markup::{self, TokenKind};
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;

use benilla_formats::GameTipsCatalog;

/// `s = 515 / (a · 1024)` at `a = 1` — the 4:3 case, which is the aspect of the content box this
/// draws into. It is the text's **scale**, and it doubles as the wrap width in the same `[0,1]`
/// space (`0x406e18` passes `s` for both).
///
/// **Not a FrameXML length.** The reference's idiom here is the FrameXML-unit conversion with one
/// extra instruction (`0x41ae40` divides `G44` back out), so reusing that helper is off by 1.25 at
/// 4:3.
const TIP_SCALE: f32 = 515.0 / 1024.0;

/// `pos.y` — the tip's baseline in the screen's `[0,1]` ortho, measured from the bottom, sitting
/// just above the progress bar's `cy = 0.075`.
const TIP_BOTTOM: f32 = 0.1;

/// **The font height is 1.8 % of the viewport HEIGHT**, and the `[0x832a48]` in `0x406659`'s
/// `0.018 · [0x832a48]` is the unit conversion, not part of the number. `0x41ad10` writes that
/// slot as `1/√(x²+1)` and its neighbour `[0x832a44]` as `x/√(x²+1)` — height/diagonal and
/// width/diagonal, i.e. the pair that turns a height- or width-fraction into the diagonal-
/// normalized units `0x44d040` and the placement round trip both work in (`0x41ae70 = x·[832a48]`
/// beside `0x41ae60 = x·[832a44]`). Reading the 0.6 the shipped `.data` holds as a factor to apply
/// would shrink the line to five-eighths of the reference's.
const TIP_FONT_FRACTION: f32 = 0.018;

/// The body colour `0xd7c8c8c8` (ARGB) and its shadow offset, in the same `[0,1]` space.
const TIP_COLOR: Color = Color::srgba(200.0 / 255.0, 200.0 / 255.0, 200.0 / 255.0, 215.0 / 255.0);
const TIP_SHADOW_OFFSET: f32 = 0.001;

/// The two CVars, mirrored: `showGameTips` and the `gameTip` cursor.
///
/// `next` is an `i64` rather than a `u32` because the CVar is a string a player (or a downgraded
/// build, or a bigger locale table) can leave anything in, and the reference's own tolerance is a
/// clamp at read time (`0x46b682`'s `i < 0 || i >= count`), not a validation at write time.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct GameTipSetting {
    pub(crate) show: bool,
    pub(crate) next: i64,
}

impl Default for GameTipSetting {
    fn default() -> Self {
        GameTipSetting {
            show: true,
            next: 0,
        }
    }
}

/// The tips table plus the one piece of state the reference keeps beside it: which row the screen
/// currently shows, laid out once per raise.
#[derive(Resource, Default)]
pub(crate) struct GameTips {
    /// `GameTips.dbc` in file order — the array `[0xc0dcd0]`/`[0xc0dcd4]`.
    catalog: GameTipsCatalog,
    /// `[0x882e10]` — the row the current screen shows, or `None` for a screen with no tip (tips
    /// off, an in-world transfer, an empty table).
    shown: Option<String>,
}

impl GameTips {
    /// `EnterWorld`'s tip block: read the stored index, clamp it, take that row, and return the
    /// index to store back (`shown + 1`).
    ///
    /// `None` when the table is empty (`count <= 0` bails at `0x46b684`).
    fn take(&self, stored: i64) -> Option<(&str, u32)> {
        let count = self.catalog.len();
        if count == 0 {
            return None;
        }
        // The clamp IS the wrap — there is no modulo, and the second range test at
        // `0x46b695`–`0x46b69b` is dead.
        let index = if stored < 0 || stored >= count as i64 {
            0
        } else {
            stored as usize
        };
        let tip = self.catalog.get(index)?;
        Some((tip, index as u32 + 1))
    }

    /// The line the screen is showing, if any.
    pub(crate) fn shown(&self) -> Option<&str> {
        self.shown.as_deref()
    }
}

/// The tip split into coloured runs, ready for a Bevy text tree — `|cAARRGGBB…|r` honoured, the
/// trailing `\r\n` the data carries dropped rather than drawn as blank lines.
///
/// Returns `(text, colour)` pairs against `base`, which is the string's own colour that `|r`
/// restores (`0x5cce99` restores `FontString+0x2c`).
pub(crate) fn spans(tip: &str, base: Color) -> Vec<(String, Color)> {
    let mut out: Vec<(String, Color)> = Vec::new();
    let mut colour = base;
    let mut at = 0;
    let mut run = String::new();
    let flush = |run: &mut String, colour: Color, out: &mut Vec<(String, Color)>| {
        if !run.is_empty() {
            out.push((std::mem::take(run), colour));
        }
    };
    while let Some(token) = markup::token_at(tip, at) {
        at += token.byte_len;
        match token.kind {
            TokenKind::Color(rgba) => {
                flush(&mut run, colour, &mut out);
                colour = Color::srgb_u8(rgba.r(), rgba.g(), rgba.b());
            }
            TokenKind::ColorReset => {
                flush(&mut run, colour, &mut out);
                colour = base;
            }
            TokenKind::LineBreak => run.push('\n'),
            TokenKind::EscapedPipe => run.push('|'),
            TokenKind::Char(c) => run.push(c),
            // No shipped tip carries a hyperlink; a locale archive that adds one draws its visible
            // text and loses only the click, which nothing on a loading screen could use anyway.
            TokenKind::LinkOpen { .. } | TokenKind::LinkClose => {}
        }
    }
    flush(&mut run, colour, &mut out);
    // The data's trailing `\r\n` (one tip carries two) would otherwise draw as empty lines under
    // the sentence and push the block off its anchor.
    if let Some((last, _)) = out.last_mut() {
        while last.ends_with('\n') {
            last.pop();
        }
    }
    out.retain(|(t, _)| !t.is_empty());
    out
}

/// The block's geometry as **percentages of the 4:3 content box** — `(left, bottom, width)`.
/// Percent rather than pixels because the node is laid out once per raise, and a window resize
/// mid-load must not strand it.
pub(crate) const fn geometry() -> (f32, f32, f32) {
    (
        (0.5 - TIP_SCALE * 0.5) * 100.0,
        TIP_BOTTOM * 100.0,
        TIP_SCALE * 100.0,
    )
}

/// The two numbers that can only be pixels — the font height and the shadow offset — for a content
/// box of `width × height` logical pixels.
pub(crate) fn layout(width: f32, height: f32) -> TipLayout {
    TipLayout {
        font_size: height * TIP_FONT_FRACTION,
        shadow: Vec2::new(width * TIP_SHADOW_OFFSET, height * TIP_SHADOW_OFFSET),
    }
}

/// What [`layout`] resolves to, in logical pixels inside the loading screen's 4:3 content box.
pub(crate) struct TipLayout {
    pub(crate) font_size: f32,
    /// `(+x, −y)` in the reference; Bevy's shadow offset is `(+x, +y)` with y growing DOWN, so the
    /// sign is already right.
    pub(crate) shadow: Vec2,
}

/// The body colour the tip's own `|r` restores to.
pub(crate) const fn base_color() -> Color {
    TIP_COLOR
}

/// **The tip node's components, in one place.** The loading screen spawns this under its 4:3
/// content box; [`drive_game_tip`] queries it back. They were written twice — the tuple in
/// `loading_screen.rs`, the query here — with nothing tying the two together.
pub(crate) fn tip_bundle() -> impl Bundle {
    (
        Text::new(String::new()),
        TextLayout {
            linebreak: LineBreak::WordBoundary,
            justify: Justify::Left,
        },
        TextFont::default(),
        TextColor(TIP_COLOR),
        TextShadow::default(),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(0.0),
            bottom: Val::Percent(0.0),
            width: Val::Percent(0.0),
            ..default()
        },
        Visibility::Hidden,
    )
}

pub(crate) struct GameTipPlugin;

/// The tip rows' change callback: the switch, and the cursor — which is not a
/// preference: a hand-edited or downgraded value lands here verbatim and [`raise`] clamps it,
/// which is the reference's own tolerance (`0x46b682`).
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut setting: ResMut<GameTipSetting>) {
    match ev.key().as_str() {
        "showgametips" => setting.show = ev.flag(),
        "gametip" => setting.next = ev.num() as i64,
        _ => {}
    }
}

impl Plugin for GameTipPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<GameTips>()
            // The CVar knob, init'd HERE and not by the CVar host: [`on_cvar`] takes it as a
            // plain `ResMut`, so a build that registers the row without the resource panics on
            // the first write — which is what a live run caught and no unit test could, the
            // test harness having its own `init_resource` chain.
            .init_resource::<GameTipSetting>()
            .add_systems(
                Startup,
                load_game_tips.after(benilla_assets::AssetSet::Open),
            )
            // After the loading screen's own drive, which is what sets the edge this reads.
            .add_systems(
                Update,
                drive_game_tip.after(benilla_world::schedule::WorldStage::Present),
            );
    }
}

/// **Putting the tip line away — one path, because there were two and they had drifted.**
///
/// Two callers empty this node and both want the same end state: hidden, childless, and holding no
/// text. Only the second of them used to clear the ROOT's own run, and that difference crashed the
/// client. Despawning a `Text` root's last `TextSpan` child does not *change*
/// `Children` — it REMOVES the component — and bevy 0.18's `detect_text_needs_rerender` watches
/// `Changed<Children>`, which a removal cannot satisfy. So the dismiss left a two-run shaped buffer
/// behind a one-run span list, and the first window resize after world entry re-laid-out that
/// buffer and indexed a run that no longer existed: an out-of-bounds panic inside `bevy_text`, on
/// a node nobody could see. Re-inserting `Text` is what tells the pipeline the block moved.
///
/// [`crate::text_reshape`] nets the same hole app-wide; this is the site being coherent on its own
/// rather than leaning on the net, and it is the half that also keeps a hidden tip from holding a
/// stale sentence.
fn empty_tip(e: &mut EntityCommands, vis: &mut Visibility) {
    *vis = Visibility::Hidden;
    e.despawn_related::<Children>();
    e.insert(Text::new(String::new()));
}

/// Take the raise's tip edge, then paint whatever the screen is showing.
///
/// Two jobs in one system because they are one mechanism seen at two moments: the reference picks
/// the row inside `EnterWorld` and lays it out once per raise, then draws that layout every frame.
fn drive_game_tip(
    mut screen: ResMut<crate::loading_screen::LoadingScreen>,
    mut tips: ResMut<GameTips>,
    mut setting: ResMut<GameTipSetting>,
    roster: Option<Res<crate::char_select::Roster>>,
    mut node: Query<
        (
            Entity,
            &mut Node,
            &mut TextFont,
            &mut TextShadow,
            &mut Visibility,
        ),
        With<crate::loading_screen::LoadingTip>,
    >,
    windows: Query<&Window>,
    assets: Res<AssetServer>,
    mut commands: Commands,
    // The cursor persists through the registry, not the knob alone: a host write there is what
    // the file is composed from and what the VM's mirror learns.
    mut cvars: ResMut<crate::cvars::Cvars>,
) {
    let Some(edge) = screen.take_tip_edge() else {
        return;
    };
    let next = match edge {
        crate::loading_screen::TipEdge::Pick => raise(
            &mut tips,
            setting.show,
            roster
                .as_ref()
                .is_some_and(|r| r.pending_level() == Some(0)),
            setting.next,
        ),
        crate::loading_screen::TipEdge::Clear => {
            clear(&mut tips);
            None
        }
    };
    if let Some(next) = next {
        // The cursor moves only when a tip was actually shown — a suppressed tip freezes it. This
        // is `EnterWorld`'s own bookkeeping and belongs to the PICK, not to the draw: it stands
        // even on a frame the paint below cannot complete.
        setting.next = i64::from(next);
        cvars.set("gameTip", &next.to_string());
    }

    // The text is laid out once per raise, not per frame, exactly as the reference does.
    let Ok((entity, mut n, mut font, mut shadow, mut vis)) = node.single_mut() else {
        // Our own `setup_loading_screen` spawns this node, so a miss is a STRUCTURAL bug — the
        // bundle no longer answering the shape of this query — and never a state a run can
        // legitimately be in. It was a silent `return` for one release, and it cost the whole
        // feature while every other instrument read green.
        warn!("loading screen: the tip node is missing — no tip can draw");
        return;
    };
    let Some(tip) = tips.shown() else {
        empty_tip(&mut commands.entity(entity), &mut vis);
        return;
    };
    // The 4:3 content box is `100vh` tall and `100vh · 4/3` wide, so the window's height is the
    // box's height and the box's width follows from it.
    let height = windows.iter().next().map_or(768.0, |w| w.height());
    let l = layout(height * 4.0 / 3.0, height);
    let (left, bottom, width) = geometry();
    n.left = Val::Percent(left);
    n.bottom = Val::Percent(bottom);
    n.width = Val::Percent(width);
    font.font = crate::char_select::wow_font(&assets);
    font.font_size = l.font_size;
    shadow.offset = l.shadow;
    shadow.color = Color::BLACK;
    *vis = Visibility::Inherited;

    // The coloured runs: the first is the `Text` root's own, the rest are `TextSpan` children.
    let runs = spans(tip, base_color());
    let painted = !runs.is_empty();
    let mut e = commands.entity(entity);
    match runs.split_first() {
        Some(((head, head_color), rest)) => {
            e.despawn_related::<Children>();
            e.insert((Text::new(head.clone()), TextColor(*head_color)));
            let (rest, tf) = (rest.to_vec(), font.clone());
            e.with_children(|c| {
                for (text, color) in rest {
                    c.spawn((TextSpan::new(text), tf.clone(), TextColor(color)));
                }
            });
        }
        None => empty_tip(&mut e, &mut vis),
    }

    // The run's own evidence — and it sits HERE, past the query, the layout and the runs, because
    // the line it replaces sat in front of all three: it reported a row picked over a screen that
    // drew nothing, and a live smoke run printed it twice while the feature was entirely dead.
    if let (Some(next), true) = (next, painted) {
        info!(
            "loading screen: tip {} of {} on screen",
            next - 1,
            tips.catalog.len()
        );
    }
}

/// `GameTips.dbc`, once, off the patch chain — the reference's own single linear load, with no
/// reload path. `.after(AssetSet::Open)` for the reason every DBC load in this tree carries it.
fn load_game_tips(mut tips: ResMut<GameTips>, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else { return };
    use benilla_assets::LockRecover;
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_game_tips(&mut chain) {
        Ok(cat) => {
            info!("loading screen: {} game tips", cat.len());
            tips.catalog = cat;
        }
        // A missing table is a loading screen with no tip, not a boot failure.
        Err(e) => warn!("GameTips.dbc unavailable — loading screens carry no tip: {e:#}"),
    }
}

/// Pick the row for a screen that is about to rise, and hand back the index to store.
///
/// `show` is `showGameTips`; `level_is_zero` is `[selChar+0x10a]`. Both suppress the tip AND the
/// advance — the reference jumps past its own `CVar::Set`, so an off switch freezes the index.
pub(crate) fn raise(
    tips: &mut GameTips,
    show: bool,
    level_is_zero: bool,
    stored: i64,
) -> Option<u32> {
    if !show || level_is_zero {
        tips.shown = None;
        return None;
    }
    match tips.take(stored) {
        Some((tip, next)) => {
            tips.shown = Some(tip.to_string());
            Some(next)
        }
        None => {
            tips.shown = None;
            None
        }
    }
}

/// The dismiss (`0x407f2b`, inside `0x407e80`) — the only thing that takes a tip down.
pub(crate) fn clear(tips: &mut GameTips) {
    tips.shown = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tips(rows: &[&str]) -> GameTips {
        GameTips {
            catalog: GameTipsCatalog::from_tips(rows.iter().map(|s| (*s).to_string()).collect()),
            shown: None,
        }
    }

    /// **`gameTip` holds the NEXT index.** A fresh client's registered `"0"` shows row 0 and stores
    /// 1, so on-disk values run 1..=count and never 0 — the off-by-one a client that stores what it
    /// just showed would ship.
    #[test]
    fn the_stored_index_is_the_next_one_not_the_shown_one() {
        let mut t = tips(&["a", "b", "c"]);
        assert_eq!(raise(&mut t, true, false, 0), Some(1));
        assert_eq!(t.shown(), Some("a"));
        assert_eq!(raise(&mut t, true, false, 1), Some(2));
        assert_eq!(t.shown(), Some("b"));
    }

    /// The wrap is the **clamp**, not a modulo: anything at or past the count restarts at 0, and so
    /// does a negative. A stored index from a bigger table (a locale archive, a patch) lands on
    /// row 0 rather than nothing.
    #[test]
    fn the_clamp_is_the_wrap() {
        let mut t = tips(&["a", "b", "c"]);
        assert_eq!(raise(&mut t, true, false, 3), Some(1), "count wraps to 0");
        assert_eq!(t.shown(), Some("a"));
        assert_eq!(
            raise(&mut t, true, false, 900),
            Some(1),
            "far past, same clamp"
        );
        assert_eq!(raise(&mut t, true, false, -4), Some(1), "and negative");
    }

    /// Both guards suppress the tip **and the advance** — the reference jumps past its own
    /// `CVar::Set`, so turning tips off freezes the index rather than burning through the table
    /// invisibly.
    #[test]
    fn a_suppressed_tip_freezes_the_index() {
        let mut t = tips(&["a", "b", "c"]);
        assert_eq!(raise(&mut t, false, false, 1), None, "showGameTips off");
        assert_eq!(t.shown(), None);
        assert_eq!(raise(&mut t, true, true, 1), None, "the level-zero guard");
        assert_eq!(t.shown(), None);
        // …and the index it would have advanced is untouched: the next armed raise still reads 1.
        assert_eq!(raise(&mut t, true, false, 1), Some(2));
        assert_eq!(t.shown(), Some("b"));
    }

    /// An empty table bails (`count <= 0` at `0x46b684`) rather than showing a blank line.
    #[test]
    fn an_empty_table_shows_nothing() {
        let mut t = tips(&[]);
        assert_eq!(raise(&mut t, true, false, 0), None);
        assert_eq!(t.shown(), None);
    }

    /// The `|c…|r` prefix is **markup in the data**, so the gold "Tip:" is its own run and the rest
    /// falls back to the body colour — and the trailing `\r\n` the DBC carries is dropped rather
    /// than drawn as blank lines under the sentence.
    #[test]
    fn the_gold_tip_prefix_is_markup_and_the_trailing_newlines_go() {
        let base = base_color();
        let runs = spans("|cffffd100Tip:|r Nearby questgivers.\r\n", base);
        assert_eq!(runs.len(), 2, "two runs: {runs:?}");
        assert_eq!(runs[0].0, "Tip:");
        assert_eq!(runs[0].1, Color::srgb_u8(0xff, 0xd1, 0x00));
        assert_eq!(runs[1].0, " Nearby questgivers.");
        assert_eq!(runs[1].1, base);
    }

    /// The placement, at the one aspect the reference's own `yoff` is zero — a 4:3 content box,
    /// which is what benilla's loading screen always is. `s = 515/1024`, left-anchored at
    /// `0.5 − s/2`, wrap width `s`, baseline at `0.1` from the bottom.
    #[test]
    fn the_layout_is_the_reference_numbers_at_four_three() {
        let (left, bottom, width) = geometry();
        // `s = 515/1024` — a 515 px column in a 1024-wide box, centred, and the baseline a tenth of
        // the box's height up, just clear of the progress bar's `cy = 0.075`.
        assert!(
            (width * 0.01 * 1024.0 - 515.0).abs() < 0.01,
            "s·width = 515 px"
        );
        assert!(
            (left * 0.01 * 1024.0 - (1024.0 - 515.0) / 2.0).abs() < 0.01,
            "centred column"
        );
        assert!((bottom - 10.0).abs() < 0.001, "0.1 of the box height");
        let l = layout(1024.0, 768.0);
        assert!(
            (l.font_size - 13.824).abs() < 0.01,
            "0.018 of the box height"
        );
    }

    /// An app holding the real node and the real system, one tip in the table — plus the text
    /// stack and the real `detect_text_needs_rerender`, so what the paint and the dismiss do to
    /// the node's `ComputedTextBlock` is observable.
    fn tip_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        crate::text_reshape::harness::add_text_plugins(&mut app);
        app.add_systems(PostUpdate, bevy::text::detect_text_needs_rerender::<Text>);
        app.init_resource::<crate::cvars::Cvars>();
        app.init_resource::<GameTipSetting>();
        app.insert_resource(GameTips {
            catalog: GameTipsCatalog::from_tips(vec![
                "|cffffd100Tip:|r Talk to the innkeeper.\r\n".to_string(),
            ]),
            shown: None,
        });
        app.init_resource::<crate::loading_screen::LoadingScreen>();
        let tip = app
            .world_mut()
            .spawn((crate::loading_screen::LoadingTip, tip_bundle()))
            .id();
        app.add_systems(Update, drive_game_tip);
        (app, tip)
    }

    fn set_edge(app: &mut App, edge: crate::loading_screen::TipEdge) {
        app.world_mut()
            .resource_mut::<crate::loading_screen::LoadingScreen>()
            .tip_edge = Some(edge);
    }

    /// **The tip has to reach the node, and only a run of the real system can say that.** Every
    /// pure-function test here covers the *pick* — and the pick is exactly the half that worked: it
    /// advanced the cursor, wrote `gameTip`, and logged "tip N of 74" over a screen that drew
    /// nothing, because the spawned bundle was one component short of what [`drive_game_tip`]'s
    /// query asks for and the missed `single_mut()` returned in silence. So this drives the
    /// system over the bundle the loading screen actually spawns and looks at the glass end.
    #[test]
    fn a_pick_paints_the_node_the_loading_screen_spawns() {
        let (mut app, tip) = tip_app();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();

        let w = app.world();
        assert_eq!(
            w.get::<Visibility>(tip),
            Some(&Visibility::Inherited),
            "the tip node never came out of hiding"
        );
        assert_eq!(
            w.get::<Text>(tip).map(|t| t.0.as_str()),
            Some("Tip:"),
            "the gold prefix is the root run"
        );
        let kids = w
            .get::<Children>(tip)
            .expect("the body sentence is a TextSpan child");
        assert_eq!(kids.len(), 1, "one run after the prefix");
        assert_eq!(
            w.get::<TextSpan>(kids[0]).map(|t| t.0.as_str()),
            Some(" Talk to the innkeeper.")
        );
        // …and the geometry the paint writes, which is what puts it above the bar rather than at
        // the box's corner where the bundle leaves it.
        let node = w.get::<Node>(tip).expect("Node");
        assert_eq!(node.bottom, Val::Percent(geometry().1));
        assert_eq!(node.width, Val::Percent(geometry().2));
        assert!(
            w.get::<TextFont>(tip).is_some_and(|f| f.font_size > 0.0),
            "the font height is resolved from the window"
        );
        // The cursor moved with it: `gameTip` holds the NEXT row.
        assert_eq!(w.resource::<GameTipSetting>().next, 1);
    }

    /// **A tip outlives every raise and dies with the screen** — [`crate::loading_screen`]'s law,
    /// seen from the painting end. That module's own test asserts which *edges*
    /// the state machine emits; this one asserts what the painter does with them, and the case that
    /// matters is the one with no edge at all: a raise landing on a live screen must leave the line
    /// exactly where it is, same text and same cursor. Between them the two cover the login snap
    /// that used to wipe the tip a server round-trip into every world entry.
    #[test]
    fn the_tip_outlives_a_second_raise_and_dies_with_the_screen() {
        let (mut app, tip) = tip_app();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(tip),
            Some(&Visibility::Inherited)
        );

        // The destination snap: `drive_loading_screen` raises again and sets NO edge.
        app.update();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(tip),
            Some(&Visibility::Inherited),
            "a raise with no edge must leave the line where it is"
        );
        assert_eq!(
            app.world().get::<Text>(tip).map(|t| t.0.as_str()),
            Some("Tip:")
        );
        assert_eq!(
            app.world().resource::<GameTipSetting>().next,
            1,
            "and it must not re-pick: one screen, one row"
        );

        // The dismiss.
        set_edge(&mut app, crate::loading_screen::TipEdge::Clear);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(tip),
            Some(&Visibility::Hidden),
            "the pending tip dies with the screen"
        );
        assert!(app
            .world()
            .get::<Children>(tip)
            .is_none_or(|c| c.is_empty()));
    }

    /// **B383 — the dismiss must leave the tip node re-shapeable.** The loading screen's root is
    /// never despawned, only hidden, so the tip node outlives every load and every later window
    /// resize re-lays-out its text block. Emptying it by despawning the spans alone left the
    /// block's shaped buffer pointing at a run that no longer existed, and the first maximize
    /// after world entry panicked inside `bevy_text`. The observable: after the dismiss the root
    /// holds no text and the block is marked for a re-shape.
    ///
    /// The mechanism's own end — that the re-shape is what stops the panic — is
    /// [`crate::text_reshape`]'s, which drives the real pipeline over the same shape.
    #[test]
    fn the_dismiss_leaves_the_node_reshapeable() {
        let (mut app, tip) = tip_app();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();
        // The state a painted frame leaves behind: a two-run block, shaped, nothing pending.
        crate::text_reshape::harness::shape(&mut app, tip);
        assert_eq!(
            app.world()
                .get::<bevy::text::ComputedTextBlock>(tip)
                .map(|b| b.entities().len()),
            Some(2),
            "the gold prefix and the sentence are two runs"
        );

        set_edge(&mut app, crate::loading_screen::TipEdge::Clear);
        app.update();

        assert_eq!(
            app.world().get::<Text>(tip).map(|t| t.0.as_str()),
            Some(""),
            "the dismissed tip holds no sentence — a hidden node with stale text is the bug"
        );
        assert!(
            app.world()
                .get::<bevy::text::ComputedTextBlock>(tip)
                .expect("the block")
                .needs_rerender(),
            "and the block is marked for the re-shape a later resize would otherwise skip"
        );
    }
}
