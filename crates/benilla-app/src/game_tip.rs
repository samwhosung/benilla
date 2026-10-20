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
//! Layout, drawn between the background and the progress bar: unlike them, which sit in the
//! fitted viewport's `[0,1]` ortho, the tip is laid out in window terms. The font engine's width
//! and height are the window rectangle's (`0x5c2b50` reads `0x58a240`), and its batch draws under
//! a pixel ortho over the viewport's own window pixels (`0x5c70b0`). So the wrap width
//! `s = 515 / (a·1024)` (the box width of the call at `0x40679c`; its box height 1.0 and spacing
//! 0.005 follow) is a fraction of the window's width, where `a = [0x832a4c]` is aspect × 0.75
//! (`0x41ad10`, read at `0x40668c`), the font is 0.018 of its height, and the position is
//! `(0.5 − s·0.5, 0.1 + yoff)` of the window, left-justified, `yoff` being the letterbox band
//! `(1 − a)·0.5` below 4:3 (`0x4066b9`) and zero otherwise; `0xd7c8c8c8` ARGB with a black
//! shadow at `(+0.001, −0.001)` of the window. `s·W` is `515·H/768` at every aspect:
//!
//! ```text
//! wrap = 515·W / (a·1024) = 515·H / (1024·0.75) = 0.6706·H
//! left = (W − wrap)/2                             = 0.5·W − 0.3353·H
//! ```
//!
//! [`placement`] is that, relative to the fitted content area it is a child of.
//!
//! The string goes through the client's font engine (`0x5c1c30`), so it is laid out and drawn by
//! [`crate::ui_text`], the engine every FontString uses: whole-pixel raster sizes, the step law
//! and the `|c…|r` escapes (`0x5c28af`, flags bit `0x800` clear) that colour the DBC's
//! `|cffffd100Tip:|r ` prefix. Its glyph cells are drawn as `bevy_ui` image nodes, because the
//! loading screen they sit inside is `bevy_ui`.
//!
//! The face is the locale's (`0x406663`-`0x406678`, table `0x82f018` by `[0xc0e080]`): koKR's
//! `2002.TTF`, zhCN's and zhTW's `FZLBJW.TTF`, `FRIZQT__.TTF` for the other five ([`tip_font_path`]).

use benilla_ui::script::{JustifyH, JustifyV, Outline};
use bevy::prelude::*;

use benilla_formats::GameTipsCatalog;

use crate::ui_pass::UiQuad;
use crate::ui_text::{glyph_texels, UiFontAtlas};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, TextSeat};

/// `s` at `a = 1`: the wrap width only, as a fraction of the window's width (the box width the
/// call at `0x40679c` passes; the font size is [`TIP_FONT_FRACTION`]). Not a FrameXML length:
/// `0x41ae40` divides `G44` back out, so the FrameXML-unit helper would be off by 1.25 at 4:3.
const TIP_SCALE: f32 = 515.0 / 1024.0;

/// The box's bottom as a fraction of the window's height above the window's bottom, under the
/// font engine's pixel ortho (`0x5c70b0`), before `yoff`. `0x4066ee` builds it as
/// `0.05·0.5 + 0.075`, the progress bar's top edge in the viewport at 4:3, so the block sits on
/// the bar there; the string is bottom-justified in it (`0x406778` pushes 2).
const TIP_BOTTOM: f32 = 0.1;

/// The font height as a fraction of the window's height. In `0x406659`'s `0.018 · [0x832a48]` the
/// slot is a unit conversion, not part of the size: `0x41ad10` writes it as height/diagonal and
/// `[0x832a44]` as width/diagonal, the diagonal-normalized units `0x44d040` works in (`0x41ae70`
/// and `0x41ae60` multiply by each). Applying its shipped `.data` value, 0.6, would shrink the
/// line. The string itself is created at a bare 0.018 (`0x40678e`).
const TIP_FONT_FRACTION: f32 = 0.018;

/// The line spacing as a fraction of the window's height, in the string's own units, as its
/// height is (`0x40677f` pushes 0.005).
const TIP_SPACING_FRACTION: f32 = 0.005;

/// The body colour `0xd7c8c8c8` (ARGB), client-space sRGB.
const TIP_COLOR: [f32; 4] = [200.0 / 255.0, 200.0 / 255.0, 200.0 / 255.0, 215.0 / 255.0];

/// The shadow: `0xff000000` (`0x4067ae`), whose alpha `0x5cd650` lowers to the body's `0xd7`
/// (`0x5cd65b`-`0x5cd667`: `+0x33 = min(shadow alpha, [+0x2f])`).
const TIP_SHADOW_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 215.0 / 255.0];

/// The shadow offset, 0.001 of the window's width and of its height (`0x4067b5`, `0x4067bc`).
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

/// The tips table and the row the screen shows, picked once per raise.
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

/// The string the engine draws: `0x406758`-`0x406766` copy the tip with its trailing CR and LF cut.
fn tip_text(tip: &str) -> &str {
    tip.trim_end_matches(['\r', '\n'])
}

/// The tip's box and type, in logical pixels, relative to the loading screen's content area.
pub(crate) struct TipPlacement {
    pub(crate) left: f32,
    pub(crate) bottom: f32,
    /// The wrap width.
    pub(crate) width: f32,
    /// The box height, 1.0 of the window: the bottom-justified block grows up from `bottom`.
    pub(crate) height: f32,
    pub(crate) font_size: f32,
    pub(crate) spacing: f32,
    /// The reference's `(+x, −y)` in whole device px; Bevy's y grows down, so the sign already
    /// matches.
    pub(crate) shadow: Vec2,
}

/// Where the reference puts the tip for a window of this logical size at `dpi` device px per
/// logical px (see the module doc), relative to the content area
/// [`crate::loading_screen::fit_rect`] gives for it.
pub(crate) fn placement(window: Vec2, widescreen: bool, dpi: f32) -> TipPlacement {
    let a = crate::loading_screen::scaled_aspect(window, widescreen);
    let area = crate::loading_screen::fit_rect(window, widescreen);
    let s = TIP_SCALE / a;
    // `yoff` is the letterbox band; a NaN or a wider window keeps zero (`0x4066c6`-`0x4066c9`).
    let yoff = if a < 1.0 { (1.0 - a) * 0.5 } else { 0.0 };
    // The anchor floors to a whole device px (`0x5cdf70`), y up from the window's bottom.
    let from_window_left = ((0.5 - s * 0.5) * window.x * dpi).floor() / dpi;
    let from_window_bottom = ((TIP_BOTTOM + yoff) * window.y * dpi).floor() / dpi;
    // The offsets round half away from zero in device px at the draw (`0x5c7010`, `0x5c6fa0`).
    let shadow = |extent: f32| (extent * TIP_SHADOW_OFFSET * dpi).round() / dpi;
    let area_bottom = window.y - (area.y + area.height);
    TipPlacement {
        left: from_window_left - area.x,
        bottom: from_window_bottom - area_bottom,
        width: s * window.x,
        height: window.y,
        font_size: window.y * TIP_FONT_FRACTION,
        spacing: window.y * TIP_SPACING_FRACTION,
        shadow: Vec2::new(shadow(window.x), shadow(window.y)),
    }
}

/// The tip as the font engine lays it out, in its node's own px: the box's bottom-left corner at
/// the origin, y down.
pub(crate) struct TipGlyphs {
    pub(crate) shadow: Vec<UiQuad>,
    pub(crate) fill: Vec<UiQuad>,
}

/// Lays the tip out as `0x5c1c30` creates it, at `p`'s size and spacing, bottom- and
/// left-justified (`0x406778`/`0x406776` push 2 and 0) in `p`'s box, with the shadow `0x5c27a0`
/// sets, in the locale's face.
///
/// Deviation: the reference rasterizes the tip's font at most 32 px (`0x5ca096`/`0x5ca09b` in
/// `0x5ca030`, reached through `0x44d040`) and magnifies it to the string's height; this
/// rasterizes at the true size, as the engine does all big text (`crate::ui_text`'s engine).
pub(crate) fn tip_glyphs(
    atlas: &UiFontAtlas,
    tip: &str,
    p: &TipPlacement,
    locale: usize,
) -> TipGlyphs {
    let text = tip_text(tip);
    let spec = FontSpec {
        path: Some(tip_font_path(locale)),
        height: Some(p.font_size),
        // The tip's font is created with flags 0 (`0x406774`): no outline, not monochrome.
        outline: Outline::None,
        monochrome: false,
        spacing: p.spacing,
        // A set shadow seeds each row's pen (`0x5cddb1`); the layout is built at the first draw
        // (`0x5cd6a0`), after `0x5c27a0` has set it.
        shadow_x: p.shadow.x,
        alpha_gradient: None,
        non_space_wrap: false,
        max_lines: 0,
    };
    let justify = Justify {
        h: JustifyH::Left,
        v: JustifyV::Bottom,
    };
    let rect = Rect::new(0.0, -p.height, p.width, 0.0);
    let (dx, dy) = (p.shadow.x, p.shadow.y);
    let shadow_rect = Rect::new(
        rect.min.x + dx,
        rect.min.y + dy,
        rect.max.x + dx,
        rect.max.y + dy,
    );
    let mut e = atlas.lock();
    let mut shadow = layout_text_quads(
        &mut e,
        text,
        shadow_rect,
        TIP_SHADOW_COLOR,
        justify,
        0,
        spec,
        TextSeat::Exact,
    );
    // Markup tints ride the fill only.
    for q in &mut shadow {
        q.color = TIP_SHADOW_COLOR;
    }
    let fill = layout_text_quads(
        &mut e,
        text,
        rect,
        TIP_COLOR,
        justify,
        0,
        spec,
        TextSeat::Exact,
    );
    TipGlyphs { shadow, fill }
}

/// One laid-out glyph as a `bevy_ui` node: its engine cell, drawn one to one at its rect.
fn glyph_node(q: &UiQuad) -> impl Bundle {
    let [r, g, b, a] = q.color;
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(q.rect.min.x),
            top: Val::Px(q.rect.min.y),
            width: Val::Px(q.rect.width()),
            height: Val::Px(q.rect.height()),
            ..default()
        },
        ImageNode {
            image: q.texture.clone().unwrap_or_default(),
            color: Color::srgba(r, g, b, a),
            rect: Some(glyph_texels(&q.uv)),
            ..default()
        },
    )
}

/// The window's logical size, 1024 x 768 when there is none (a headless run).
fn window_size(windows: &Query<&Window, With<bevy::window::PrimaryWindow>>) -> Vec2 {
    windows.iter().next().map_or(Vec2::new(1024.0, 768.0), |w| {
        Vec2::new(w.width(), w.height())
    })
}

/// Writes `p`'s box to the tip node, only where it differs.
fn apply(p: &TipPlacement, node: &mut Node) {
    let (left, bottom, width) = (Val::Px(p.left), Val::Px(p.bottom), Val::Px(p.width));
    if node.left != left || node.bottom != bottom || node.width != width {
        node.left = left;
        node.bottom = bottom;
        node.width = width;
    }
}

/// What the tip's glyphs were laid out for: the window, the `widescreen` fit, the engine's DPI and
/// sheet generation, whether it exists, and the string.
#[derive(Default)]
struct Laid {
    key: (Vec2, bool, f32, u64, bool),
    tip: Option<String>,
}

/// Per frame: the tip follows the window, as the reference's font engine does when the viewport
/// changes (`0x5c2b50` re-lays every font), and a sheet reset, which moves its cells.
fn paint_game_tip(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    tips: Res<GameTips>,
    atlas: Option<Res<UiFontAtlas>>,
    mut tip: Query<(Entity, &mut Node), With<crate::loading_screen::LoadingTip>>,
    mut laid: Local<Laid>,
    mut commands: Commands,
) {
    let Ok((entity, mut node)) = tip.single_mut() else {
        return;
    };
    let window = window_size(&windows);
    let widescreen = crate::loading_screen::widescreen(cvars.as_deref());
    // The engine's DPI, which the glyphs are laid out at; the window's without an engine.
    let dpi = atlas.as_ref().map_or_else(
        || windows.iter().next().map_or(1.0, Window::scale_factor),
        |a| a.dpi(),
    );
    let key = (
        window,
        widescreen,
        dpi,
        atlas.as_ref().map_or(0, |a| a.generation),
        atlas.is_some(),
    );
    if laid.key == key && laid.tip.as_deref() == tips.shown() {
        return;
    }
    let p = placement(window, widescreen, dpi);
    apply(&p, &mut node);
    let mut e = commands.entity(entity);
    e.despawn_related::<Children>();
    if let (Some(shown), Some(atlas)) = (tips.shown(), atlas.as_deref()) {
        let glyphs = tip_glyphs(atlas, shown, &p, crate::ui_script::client_locale().index());
        // The shadow first, so the fill paints over it.
        e.with_children(|c| {
            for q in glyphs.shadow.iter().chain(&glyphs.fill) {
                c.spawn(glyph_node(q));
            }
        });
    }
    *laid = Laid {
        key,
        tip: tips.shown().map(str::to_owned),
    };
}

/// The tip node: the box [`paint_game_tip`] places and fills with glyph nodes. The loading screen
/// spawns it and [`drive_game_tip`] queries it, so one definition keeps the two in step.
pub(crate) fn tip_bundle() -> impl Bundle {
    (
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
                (
                    drive_game_tip.after(benilla_world::schedule::WorldStage::Present),
                    // After the font engine's other users fill this frame's cells: the UI pass,
                    // the nameplates and the append lane's combat text.
                    paint_game_tip
                        .after(drive_game_tip)
                        .after(crate::loading_screen::fit_loading_area)
                        .after(crate::ui_script::UiPaint)
                        .after(crate::nameplates::drive_nameplates)
                        .after(crate::ui_pass::UiQuadAppend),
                ),
            );
    }
}

/// Takes the raise's tip edge and shows or hides the line; [`paint_game_tip`] lays it out. The
/// reference picks the row in `EnterWorld` and draws it every frame.
fn drive_game_tip(
    mut screen: ResMut<crate::loading_screen::LoadingScreen>,
    mut tips: ResMut<GameTips>,
    mut setting: ResMut<GameTipSetting>,
    roster: Option<Res<crate::glue_session::roster::Roster>>,
    mut node: Query<&mut Visibility, With<crate::loading_screen::LoadingTip>>,
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
            roster.as_ref().is_some_and(|r| r.pending_new_character()),
            setting.next,
        ),
        crate::loading_screen::TipEdge::Clear => {
            clear(&mut tips);
            None
        }
    };
    if let Some(next) = next {
        // Only a shown tip moves the cursor, at the pick, even if nothing draws.
        setting.next = i64::from(next);
        cvars.set("gameTip", &next.to_string());
    }

    let Ok(mut vis) = node.single_mut() else {
        // `setup_loading_screen` spawns this node, so a miss is a structural bug: the bundle no
        // longer matches this query.
        warn!("loading screen: the tip node is missing — no tip can draw");
        return;
    };
    *vis = if tips.shown().is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    // Logged only for a line with something to draw, not merely picked.
    if let (Some(next), Some(tip)) = (next, tips.shown()) {
        if !tip_text(tip).is_empty() {
            info!(
                "loading screen: tip {} of {} on screen",
                next - 1,
                tips.catalog.len()
            );
        }
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
/// `new_character` is `[selChar+0x10a]`; either suppresses the tip and the advance.
pub(crate) fn raise(
    tips: &mut GameTips,
    show: bool,
    new_character: bool,
    stored: i64,
) -> Option<u32> {
    if !show || new_character {
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

/// The tip's face by locale index, the font engine's path for it: the table `0x82f018`
/// (`s_loadingFontPaths`), koKR (1) `2002.TTF`, zhCN (4) and zhTW (5) `FZLBJW.TTF`, every other
/// `FRIZQT__.TTF`.
fn tip_font_path(locale: usize) -> &'static str {
    match locale {
        1 => "Fonts\\2002.TTF",
        4 | 5 => "Fonts\\FZLBJW.TTF",
        _ => "Fonts\\FRIZQT__.TTF",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x82f018` by locale index (enUS, koKR, frFR, deDE, zhCN, zhTW, esES, xxYY): koKR and the
    /// two Chinese locales have faces of their own, every other index the Latin face.
    #[test]
    fn the_tip_face_follows_the_locale() {
        let want = [
            "FRIZQT__", "2002", "FRIZQT__", "FRIZQT__", "FZLBJW", "FZLBJW", "FRIZQT__", "FRIZQT__",
        ];
        for (locale, face) in want.into_iter().enumerate() {
            assert_eq!(
                tip_font_path(locale),
                format!("Fonts\\{face}.TTF"),
                "locale {locale}"
            );
        }
        // The index is clamped to the table by `Locale`, so none past it reaches here; the
        // default arm is the Latin face.
        assert_eq!(tip_font_path(99), "Fonts\\FRIZQT__.TTF");
    }

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
        assert_eq!(
            raise(&mut t, true, true, 1),
            None,
            "the new-character guard"
        );
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

    fn near(got: f32, want: f32) -> bool {
        (got - want).abs() < 0.01
    }

    /// At 4:3, where `yoff` is zero: `s = 515/1024`, left at `0.5 − s/2`, the box's bottom at
    /// `0.1`, each floored to a device px.
    #[test]
    fn the_layout_is_the_reference_numbers_at_four_three() {
        let p = placement(Vec2::new(1024.0, 768.0), true, 1.0);
        assert!(near(p.width, 515.0), "s·width = 515 px");
        assert_eq!(p.left, 254.0, "the centred column's 254.5, floored");
        assert_eq!(p.bottom, 76.0, "0.1 of the height, 76.8, floored");
        assert!(near(p.font_size, 13.824), "0.018 of the height");
        assert!(near(p.spacing, 3.84), "0.005 of the height");
        assert!(near(p.height, 768.0), "a box the window tall");
        assert_eq!(p.shadow, Vec2::ONE, "1.024 and 0.768 device px, rounded");
    }

    /// At DPI 2 the anchor floors and the shadow rounds in device px: a 1512×982 window is 3024×1964
    /// device px, so the shadow is 3 and 2 of them, not the logical 1.512 rounded to 2.
    #[test]
    fn the_snaps_are_in_device_pixels() {
        let p = placement(Vec2::new(1512.0, 982.0), true, 2.0);
        assert_eq!(p.shadow, Vec2::new(1.5, 1.0));
        // The column's left edge is 756 − 658.5/2 = 426.75 logical, 853.5 device px, floored.
        let area = crate::loading_screen::fit_rect(Vec2::new(1512.0, 982.0), true);
        assert!(near(p.left + area.x, 426.5), "{}", p.left + area.x);
        // 196.4 device px above the window's bottom, floored; the window is the area's height.
        assert_eq!(p.bottom, 98.0);
    }

    /// Wider than 4:3 the area is pillarboxed and the tip is the same size and height as in a 4:3
    /// window of that height (`s·W = 515·H/768`), centred on the window.
    #[test]
    fn a_wide_window_keeps_the_tip_of_its_height() {
        let p = placement(Vec2::new(1920.0, 1080.0), true, 1.0);
        assert!(near(p.width, 515.0 * 1080.0 / 768.0));
        // Area x = 240; the column is centred on the window, 960, its 597.89 floored.
        assert!(near(p.left + 240.0, (960.0 - p.width / 2.0).floor()));
        assert!(near(p.bottom, 108.0));
        assert!(near(p.font_size, 19.44));
    }

    /// `widescreen` off is a 4/3 aspect: `a` is 1.0, so the area is the window and the wrap is
    /// `515·W/1024`, centred, at any window shape.
    #[test]
    fn widescreen_off_lays_the_tip_out_on_the_whole_window() {
        let p = placement(Vec2::new(1280.0, 1024.0), false, 1.0);
        assert!(near(p.width, 515.0 * 1280.0 / 1024.0), "{}", p.width);
        assert!(near(p.left, ((1280.0 - p.width) / 2.0).floor()));
        assert!(near(p.bottom, 102.0), "0.1 of the height, floored");
    }

    /// Narrower than 4:3 the area is letterboxed and the tip is laid out in window terms: its
    /// width and type are fractions of the window height, and its baseline sits `yoff` plus 0.1 of
    /// the window height above the window's bottom, which is 0.1 of the height above the area's.
    #[test]
    fn a_tall_window_lays_the_tip_out_in_window_terms() {
        // 1280x1024: a = 0.9375, the area 1280x960 at y = 32.
        let p = placement(Vec2::new(1280.0, 1024.0), true, 1.0);
        assert!(near(p.width, 515.0 * 1024.0 / 768.0), "{}", p.width);
        assert!(near(p.left, (640.0 - p.width / 2.0).floor()), "area x is 0");
        // `0.13125·1024 = 134.4` above the window's bottom, floored, less the 32 px band.
        assert!(near(p.bottom, 102.0), "{}", p.bottom);
        assert!(near(p.font_size, 18.432));
        assert_eq!(p.shadow, Vec2::ONE);
    }

    const TIP: &str = "|cffffd100Tip:|r Talk to the innkeeper.\r\n";

    /// The glyphs the tip's characters draw: every one but the spaces.
    const TIP_GLYPHS: usize = "Tip:Talktotheinnkeeper.".len();

    /// The font engine at DPI 1 over the install's faces.
    fn engine_atlas() -> UiFontAtlas {
        UiFontAtlas::for_test(1.0).expect("the client faces open")
    }

    /// The engine lays the line out: the gold `Tip:` from the DBC's escape, the body in
    /// `0xd7c8c8c8`, the trailing CR LF cut, the block bottom-justified on the box's bottom, and
    /// the shadow the same glyphs a whole pixel right and down at the body's alpha.
    #[test]
    fn the_tip_is_the_font_engines_layout_of_the_dbc_line() {
        benilla_formats::wow_data_or_skip!();
        let atlas = engine_atlas();
        let p = placement(Vec2::new(1024.0, 768.0), true, 1.0);
        let g = tip_glyphs(&atlas, TIP, &p, 0);
        assert_eq!(g.fill.len(), TIP_GLYPHS, "one quad per inked character");
        assert_eq!(g.shadow.len(), g.fill.len());
        let gold = [1.0, f32::from(0xd1u8) / 255.0, 0.0];
        for (i, q) in g.fill.iter().enumerate() {
            let want = if i < 4 { gold } else { [TIP_COLOR[0]; 3] };
            assert_eq!(q.color[..3], want, "glyph {i}'s colour");
            assert_eq!(
                q.texture,
                Some(atlas.image()),
                "glyph {i} is an engine cell"
            );
        }
        assert!(g.fill[4..].iter().all(|q| q.color == TIP_COLOR));
        for (s, f) in g.shadow.iter().zip(&g.fill) {
            assert_eq!(s.color, TIP_SHADOW_COLOR);
            assert_eq!(
                s.rect.min,
                f.rect.min + Vec2::ONE,
                "a whole pixel right and down"
            );
        }
        // One 14 px line on the box's bottom, with one CR LF or the two one shipped tip ends in:
        // no blank line lifts it. The unhinted `p` hangs 4 rows under its baseline, one past the
        // line's 3, and the reference's seat keeps its ink there (`0x5d1360` lowers the quad).
        for tip in [TIP, "|cffffd100Tip:|r Talk to the innkeeper.\r\n\r\n"] {
            let g = tip_glyphs(&atlas, tip, &p, 0);
            let bottom = g.fill.iter().map(|q| q.rect.max.y).fold(f32::MIN, f32::max);
            let top = g.fill.iter().map(|q| q.rect.min.y).fold(f32::MAX, f32::min);
            assert!(
                (-2.0..=1.0).contains(&bottom),
                "{tip:?} sits on the box: {bottom}"
            );
            assert!(top > -16.0, "{tip:?} is one line tall: {top}");
        }
    }

    /// The reported "Tlp": the tip's `i` keeps an inkless row between its dot and its stem at every
    /// common window height, a 1080-tall window's 19 px among them.
    #[test]
    fn the_tips_i_keeps_its_dot() {
        benilla_formats::wow_data_or_skip!();
        for h in [768.0f32, 900.0, 1024.0, 1080.0, 1200.0, 1440.0] {
            let atlas = engine_atlas();
            let p = placement(Vec2::new(h * 16.0 / 9.0, h), true, 1.0);
            let g = tip_glyphs(&atlas, TIP, &p, 0);
            // `T`, `i`.
            let cell = crate::ui_text::glyph_texels(&g.fill[1].uv);
            let rows: Vec<u8> = (cell.min.y as u32..cell.max.y as u32)
                .map(|y| {
                    (cell.min.x as u32..cell.max.x as u32)
                        .filter_map(|x| atlas.pending_alpha_for_test(x, y))
                        .max()
                        .unwrap_or(0)
                })
                .collect();
            let dot = rows.iter().position(|&a| a >= 128).expect("the i has ink");
            let gap = rows[dot..].iter().position(|&a| a < 128).map(|i| i + dot);
            let stem = gap.and_then(|g| rows[g..].iter().position(|&a| a >= 128));
            assert!(
                stem.is_some(),
                "window {h}: the i's dot runs into its stem, row maxima {rows:?}"
            );
        }
    }

    /// The real nodes and systems with one tip, with the font engine or without it.
    fn tip_app(atlas: Option<UiFontAtlas>) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<crate::cvars::Cvars>();
        app.init_resource::<GameTipSetting>();
        app.insert_resource(GameTips {
            catalog: GameTipsCatalog::from_tips(vec![TIP.to_string()]),
            shown: None,
        });
        if let Some(atlas) = atlas {
            app.insert_resource(atlas);
        }
        app.init_resource::<crate::loading_screen::LoadingScreen>();
        let tip = app
            .world_mut()
            .spawn((crate::loading_screen::LoadingTip, tip_bundle()))
            .id();
        app.add_systems(
            Update,
            (drive_game_tip, paint_game_tip.after(drive_game_tip)),
        );
        (app, tip)
    }

    fn set_edge(app: &mut App, edge: crate::loading_screen::TipEdge) {
        app.world_mut()
            .resource_mut::<crate::loading_screen::LoadingScreen>()
            .tip_edge = Some(edge);
    }

    fn glyph_nodes(app: &App, tip: Entity) -> Vec<Entity> {
        app.world()
            .get::<Children>(tip)
            .map_or_else(Vec::new, |c| c.to_vec())
    }

    /// A pick reaches the node the loading screen spawns, not only the cursor: the bundle must
    /// match [`drive_game_tip`]'s query, and the box lands on the reference's numbers.
    #[test]
    fn a_pick_shows_the_node_the_loading_screen_spawns() {
        let (mut app, tip) = tip_app(None);
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();

        let w = app.world();
        assert_eq!(
            w.get::<Visibility>(tip),
            Some(&Visibility::Inherited),
            "the tip node never came out of hiding"
        );
        let node = w.get::<Node>(tip).expect("Node");
        let p = placement(Vec2::new(1024.0, 768.0), true, 1.0);
        assert_eq!(node.left, Val::Px(p.left));
        assert_eq!(node.bottom, Val::Px(p.bottom));
        assert_eq!(node.width, Val::Px(p.width));
        // The cursor moved with it: `gameTip` holds the next row.
        assert_eq!(w.resource::<GameTipSetting>().next, 1);
    }

    /// The tip is skipped, and the cursor with it, for a record whose new-character byte is set
    /// (`0x46b671`): the list parse sets it for a wire level of 0 and stores level 1
    /// (`0x5b44e8`-`0x5b44f9`), so the record's level says nothing of it.
    #[test]
    fn a_new_characters_entry_shows_no_tip_and_freezes_the_index() {
        let (mut app, tip) = tip_app(None);
        let mut new = crate::glue_session::roster::test_character(7, "New");
        new.new_character = true;
        app.insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            vec![new],
            7,
        ));
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();

        let w = app.world();
        assert_eq!(w.get::<Visibility>(tip), Some(&Visibility::Hidden));
        assert_eq!(w.resource::<GameTipSetting>().next, 0, "the index stood");
    }

    /// The pick draws the line as the font engine's cells, one node per glyph, each the engine's
    /// own cell one to one; nothing is re-laid while nothing changes, and the dismiss takes the
    /// glyphs down.
    #[test]
    fn a_pick_draws_the_tip_in_the_font_engines_glyphs() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, tip) = tip_app(Some(engine_atlas()));
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();

        let sheet = app.world().resource::<UiFontAtlas>().image();
        let nodes = glyph_nodes(&app, tip);
        assert_eq!(nodes.len(), 2 * TIP_GLYPHS, "a shadow and a fill per glyph");
        for &n in &nodes {
            let img = app.world().get::<ImageNode>(n).expect("an image node");
            assert_eq!(img.image, sheet, "drawn from the engine's glyph sheet");
            let texels = img.rect.expect("a cell of the sheet");
            let node = app.world().get::<Node>(n).expect("Node");
            assert_eq!(node.width, Val::Px(texels.width()), "one to one at DPI 1");
            assert_eq!(node.height, Val::Px(texels.height()));
        }

        app.update();
        assert_eq!(
            glyph_nodes(&app, tip),
            nodes,
            "an unchanged frame lays nothing out"
        );

        set_edge(&mut app, crate::loading_screen::TipEdge::Clear);
        app.update();
        assert!(
            glyph_nodes(&app, tip).is_empty(),
            "the glyphs die with the tip"
        );
    }

    /// The tip follows the window while the screen is up: a resize refits the box and lays the
    /// glyphs out again at the new size, which a once-per-raise layout left at the raise's.
    #[test]
    fn a_resize_relays_the_box_and_the_glyphs() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, tip) = tip_app(Some(engine_atlas()));
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: (1280_u32, 1024_u32).into(),
                    ..default()
                },
                bevy::window::PrimaryWindow,
            ))
            .id();
        set_edge(&mut app, crate::loading_screen::TipEdge::Pick);
        app.update();
        let tallest = |app: &App| {
            glyph_nodes(app, tip)
                .iter()
                .filter_map(|&n| match app.world().get::<Node>(n)?.height {
                    Val::Px(h) => Some(h),
                    _ => None,
                })
                .fold(0.0f32, f32::max)
        };
        let before = tallest(&app);

        // Resized, with no new raise.
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1920.0, 1440.0);
        app.update();
        let want = placement(Vec2::new(1920.0, 1440.0), true, 1.0);
        let node = app.world().get::<Node>(tip).unwrap();
        assert_eq!(node.width, Val::Px(want.width));
        assert_eq!(node.bottom, Val::Px(want.bottom));
        assert_eq!(glyph_nodes(&app, tip).len(), 2 * TIP_GLYPHS);
        assert!(
            tallest(&app) > before,
            "26 px glyphs, not the 18 px ones: {} vs {before}",
            tallest(&app)
        );
    }

    /// A raise with no edge on a live screen keeps the line and the cursor; only the dismiss takes
    /// it down. [`crate::loading_screen`]'s test covers which edges are emitted.
    #[test]
    fn the_tip_outlives_a_second_raise_and_dies_with_the_screen() {
        let (mut app, tip) = tip_app(None);
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
        assert_eq!(app.world().resource::<GameTips>().shown(), Some(TIP));
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
        assert_eq!(app.world().resource::<GameTips>().shown(), None);
    }
}
