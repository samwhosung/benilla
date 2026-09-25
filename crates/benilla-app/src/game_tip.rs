//! The loading screen's tip of the day. 1.12's FrameXML only has the `showGameTips` option, so the
//! feature is engine-side: `CGlueMgr::EnterWorld` picks a `GameTips.dbc` row for the screen and
//! writes the next index back to the `gameTip` CVar.
//!
//! Selection (`0x46b662`-`0x46b6e3`): `gameTip` holds the next index, not the shown one, so stored
//! values run 1..=count and never 0. The walk is sequential and wraps by clamping (`i < 0 || i >=
//! count` gives 0, `0x46b682`), not by modulo; `count <= 0` bails. `showGameTips` off skips the
//! `CVar::Set` as well as the draw (`0x46b671`/`0x46b678`), so it freezes the index; so does
//! `[selChar+0x10a]`, which `0x5b42a0` sets when `SMSG_CHAR_ENUM` reports level 0 (unreachable
//! against vmangos, honoured anyway).
//!
//! Only the glue-to-world load shows a tip: the setter `0x406630` has one caller, and neither
//! `SMSG_TRANSFER_PENDING` arm sets it. The tip is picked at the entry raise and cleared only at
//! the dismiss (`0x407f2b`, inside `0x407e80`), so a later raise such as the
//! `SMSG_LOGIN_VERIFY_WORLD` snap keeps it.
//!
//! Layout, laid out once per raise and drawn between the background and the progress bar, in the
//! screen's `[0,1]` ortho: scale and wrap width `s = 515 / (a·1024)`, where `a = [0x832a4c]` is
//! aspect × 0.75 (`0x41ad10`); position `(0.5 − s·0.5, 0.1 + yoff)`, left-justified; `0xd7c8c8c8`
//! ARGB with an opaque black shadow at `(+0.001, −0.001)`. In a height-fit 4:3 box the `a` cancels,
//! so the box gives the reference's numbers at every aspect:
//!
//! ```text
//! ref wrap  = s·W          = 515·W / (a·1024)          = 515·H / (1024·0.75) = 0.6706·H
//! our wrap  = s₁·(4H/3)    = (515/1024)·(4H/3)                               = 0.6706·H
//! ref left  = (0.5 − s/2)·W                                   = 0.5·W − 0.3353·H
//! our left  = (W − 4H/3)/2 + (0.5 − s₁/2)·(4H/3)              = 0.5·W − 0.3353·H
//! ```
//!
//! `yoff` is `(1 − a)·0.5` only for `a < 1` (`0x4066b9`), zero on any window at least 4:3 wide.
//! Narrower than 4:3 the loading screen's box overflows where the reference letterboxes top and
//! bottom, so the whole screen differs there, the tip with it.
//!
//! The `|cffffd100Tip:|r ` prefix and trailing `\r\n` are in the DBC data and the escapes are
//! interpreted (`0x5c28af`, flags bit `0x800` clear), so the line goes through
//! [`benilla_ui::markup`].

use benilla_ui::markup::{self, TokenKind};
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;

use benilla_formats::GameTipsCatalog;

/// `s` at `a = 1`, the 4:3 box this draws into: the text's scale and also its wrap width
/// (`0x406e18` passes `s` for both). Not a FrameXML length: `0x41ae40` divides `G44` back out, so
/// the FrameXML-unit helper would be off by 1.25 at 4:3.
const TIP_SCALE: f32 = 515.0 / 1024.0;

/// The baseline from the bottom of the `[0,1]` ortho. `0x4066ee` builds it as `0.05·0.5 + 0.075`,
/// the progress bar's top edge, so the block sits on the bar.
const TIP_BOTTOM: f32 = 0.1;

/// The font height as a fraction of the viewport height. In `0x406659`'s `0.018 · [0x832a48]` the
/// slot is a unit conversion, not part of the size: `0x41ad10` writes it as height/diagonal and
/// `[0x832a44]` as width/diagonal, the diagonal-normalized units `0x44d040` works in (`0x41ae70`
/// and `0x41ae60` multiply by each). Applying its shipped `.data` value, 0.6, would shrink the
/// line.
const TIP_FONT_FRACTION: f32 = 0.018;

/// The body colour `0xd7c8c8c8` (ARGB) and its shadow offset, in the same `[0,1]` space.
const TIP_COLOR: Color = Color::srgba(200.0 / 255.0, 200.0 / 255.0, 200.0 / 255.0, 215.0 / 255.0);
const TIP_SHADOW_OFFSET: f32 = 0.001;

/// The `showGameTips` switch and the `gameTip` cursor. `next` is an `i64` because the CVar can hold
/// anything; the reference clamps at read time (`0x46b682`), not at write time.
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

/// The tips table and the row the screen shows, laid out once per raise.
#[derive(Resource, Default)]
pub(crate) struct GameTips {
    /// `GameTips.dbc` in file order, the array at `[0xc0dcd0]`/`[0xc0dcd4]`.
    catalog: GameTipsCatalog,
    /// `[0x882e10]`: the row on screen; `None` with tips off, in-world, or with an empty table.
    shown: Option<String>,
}

impl GameTips {
    /// `EnterWorld`'s tip block: clamps the stored index and returns that row with the index to
    /// store (`shown + 1`); `None` for an empty table (`count <= 0` bails at `0x46b684`).
    fn take(&self, stored: i64) -> Option<(&str, u32)> {
        let count = self.catalog.len();
        if count == 0 {
            return None;
        }
        // The clamp is the wrap; the second range test at `0x46b695`-`0x46b69b` is dead.
        let index = if stored < 0 || stored >= count as i64 {
            0
        } else {
            stored as usize
        };
        let tip = self.catalog.get(index)?;
        Some((tip, index as u32 + 1))
    }

    pub(crate) fn shown(&self) -> Option<&str> {
        self.shown.as_deref()
    }
}

/// The tip as coloured runs: `|cAARRGGBB…|r` honoured against `base`, which `|r` restores
/// (`0x5cce99` restores `FontString+0x2c`), and the trailing `\r\n` dropped.
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
            // No shipped tip has a hyperlink; one would draw its text and lose only the click.
            TokenKind::LinkOpen { .. } | TokenKind::LinkClose => {}
        }
    }
    flush(&mut run, colour, &mut out);
    // The data's trailing `\r\n` (two on one tip) would draw as blank lines and lift the block.
    if let Some((last, _)) = out.last_mut() {
        while last.ends_with('\n') {
            last.pop();
        }
    }
    out.retain(|(t, _)| !t.is_empty());
    out
}

/// `(left, bottom, width)` as percentages of the 4:3 content box, so a mid-load resize cannot
/// strand a node laid out once per raise.
pub(crate) const fn geometry() -> (f32, f32, f32) {
    (
        (0.5 - TIP_SCALE * 0.5) * 100.0,
        TIP_BOTTOM * 100.0,
        TIP_SCALE * 100.0,
    )
}

/// The font height and shadow offset in pixels, for a `width × height` content box.
pub(crate) fn layout(width: f32, height: f32) -> TipLayout {
    TipLayout {
        font_size: height * TIP_FONT_FRACTION,
        shadow: Vec2::new(width * TIP_SHADOW_OFFSET, height * TIP_SHADOW_OFFSET),
    }
}

/// [`layout`]'s result, in logical pixels inside the 4:3 content box.
pub(crate) struct TipLayout {
    pub(crate) font_size: f32,
    /// The reference's `(+x, −y)`; Bevy's y grows down, so the sign already matches.
    pub(crate) shadow: Vec2,
}

/// The body colour the tip's `|r` restores.
pub(crate) const fn base_color() -> Color {
    TIP_COLOR
}

/// The tip node's components: the loading screen spawns them and [`drive_game_tip`] queries them,
/// so one definition keeps the two in step.
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

/// The tip CVars' change callback. A hand-edited cursor lands verbatim; [`raise`] clamps it, as the
/// reference does (`0x46b682`).
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
            // Initialised here, not by the CVar host: [`on_cvar`] takes it as a plain `ResMut` and
            // panics without it.
            .init_resource::<GameTipSetting>()
            .add_systems(
                Startup,
                load_game_tips.after(benilla_assets::AssetSet::Open),
            )
            // After the loading screen's drive, which sets the edge this reads.
            .add_systems(
                Update,
                drive_game_tip.after(benilla_world::schedule::WorldStage::Present),
            );
    }
}

/// Hides the tip node, drops its spans and re-inserts an empty `Text`. The re-insert matters:
/// despawning a `Text` root's last `TextSpan` removes `Children` rather than changing it, bevy
/// 0.18's `detect_text_needs_rerender` misses a removal, and the next resize re-lays-out a stale
/// buffer and panics in `bevy_text`. [`crate::text_reshape`] guards the same hole app-wide.
fn empty_tip(e: &mut EntityCommands, vis: &mut Visibility) {
    *vis = Visibility::Hidden;
    e.despawn_related::<Children>();
    e.insert(Text::new(String::new()));
}

/// Takes the raise's tip edge, then paints what the screen shows: the reference picks the row in
/// `EnterWorld`, lays it out once per raise and draws it every frame.
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
    // The cursor persists through the registry, which composes the file and feeds the VM's mirror.
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
        // Only a shown tip moves the cursor, at the pick, even if the paint below fails.
        setting.next = i64::from(next);
        cvars.set("gameTip", &next.to_string());
    }

    // Laid out once per raise, not per frame, as the reference does.
    let Ok((entity, mut n, mut font, mut shadow, mut vis)) = node.single_mut() else {
        // `setup_loading_screen` spawns this node, so a miss is a structural bug: the bundle no
        // longer matches this query.
        warn!("loading screen: the tip node is missing — no tip can draw");
        return;
    };
    let Some(tip) = tips.shown() else {
        empty_tip(&mut commands.entity(entity), &mut vis);
        return;
    };
    // The 4:3 content box is `100vh` tall, so the window height is the box height.
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

    // Logged only once the line is painted, not merely picked.
    if let (Some(next), true) = (next, painted) {
        info!(
            "loading screen: tip {} of {} on screen",
            next - 1,
            tips.catalog.len()
        );
    }
}

/// `GameTips.dbc`, loaded once off the patch chain with no reload path, as the reference does.
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

/// Picks the row for a rising screen and returns the index to store. `show` is `showGameTips` and
/// `level_is_zero` is `[selChar+0x10a]`; either suppresses the tip and the advance.
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

/// The dismiss (`0x407f2b`, inside `0x407e80`), the only thing that takes a tip down.
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

    /// A fresh `"0"` shows row 0 and stores 1.
    #[test]
    fn the_stored_index_is_the_next_one_not_the_shown_one() {
        let mut t = tips(&["a", "b", "c"]);
        assert_eq!(raise(&mut t, true, false, 0), Some(1));
        assert_eq!(t.shown(), Some("a"));
        assert_eq!(raise(&mut t, true, false, 1), Some(2));
        assert_eq!(t.shown(), Some("b"));
    }

    /// Past the count or negative restarts at row 0.
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

    /// Both guards suppress the tip and the advance.
    #[test]
    fn a_suppressed_tip_freezes_the_index() {
        let mut t = tips(&["a", "b", "c"]);
        assert_eq!(raise(&mut t, false, false, 1), None, "showGameTips off");
        assert_eq!(t.shown(), None);
        assert_eq!(raise(&mut t, true, true, 1), None, "the level-zero guard");
        assert_eq!(t.shown(), None);
        // The next armed raise still reads 1.
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

    /// The gold "Tip:" is its own run, and the trailing `\r\n` goes.
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

    /// At 4:3, where `yoff` is zero: `s = 515/1024`, left at `0.5 − s/2`, baseline at `0.1`.
    #[test]
    fn the_layout_is_the_reference_numbers_at_four_three() {
        let (left, bottom, width) = geometry();
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

    /// The real node and system with one tip, plus the text stack and `detect_text_needs_rerender`,
    /// so what the paint and the dismiss do to the `ComputedTextBlock` is observable.
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

    /// A pick reaches the node the loading screen spawns, not only the cursor: the bundle must
    /// match [`drive_game_tip`]'s query.
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
        // The paint's geometry puts the line above the bar, off the bundle's corner.
        let node = w.get::<Node>(tip).expect("Node");
        assert_eq!(node.bottom, Val::Percent(geometry().1));
        assert_eq!(node.width, Val::Percent(geometry().2));
        assert!(
            w.get::<TextFont>(tip).is_some_and(|f| f.font_size > 0.0),
            "the font height is resolved from the window"
        );
        // The cursor moved with it: `gameTip` holds the next row.
        assert_eq!(w.resource::<GameTipSetting>().next, 1);
    }

    /// A raise with no edge on a live screen keeps the line and the cursor; only the dismiss takes
    /// it down. [`crate::loading_screen`]'s test covers which edges are emitted.
    #[test]
    fn the_tip_outlives_a_second_raise_and_dies_with_the_screen() {
        let (mut app, tip) = tip_app();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(tip),
            Some(&Visibility::Inherited)
        );

        // The destination snap: `drive_loading_screen` raises again and sets no edge.
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

    /// After the dismiss the root holds no text and the block is marked for a re-shape, so a later
    /// resize cannot index a despawned run (the loading screen's root is hidden, never despawned).
    /// [`crate::text_reshape`] shows the re-shape is what stops the panic.
    #[test]
    fn the_dismiss_leaves_the_node_reshapeable() {
        let (mut app, tip) = tip_app();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();
        // A painted frame's state: a two-run block, shaped, nothing pending.
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
