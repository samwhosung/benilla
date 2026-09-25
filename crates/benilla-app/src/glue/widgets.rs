//! The glue screens' widget builders (the button shapes of `GlueButtons.xml` and
//! `CharacterCreate.xml`) and their marker components. Builders are generic over the screen's
//! action component; each falls back to a plain face with text without client art.

use benilla_ui::markup::{tokens, TokenKind};
use benilla_ui::widget::EditBoxState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui_render::ui_material::MaterialNode;

use super::art::{tc_rect, ArrowArt, GlueArt, BTN_BG, BUTTON_TC, FALLBACK_ALPHA, GOLD, NAME_EDGE};
use super::backdrop::{backdrop_border, tiled_bg_node};

// ── The shared widget vocabulary ─────────────────────────────────────────────────────────────────

/// A button's `HighlightTexture`, lit on hover or by [`LockHighlight`]. Its visibility belongs to
/// [`super::glue_hilights`] alone.
#[derive(Component)]
pub(crate) struct Hilight;
/// `Button:LockHighlight()`: hold this button's [`Hilight`] lit, as every glue list marks its
/// selected row.
#[derive(Component, Default)]
pub(crate) struct LockHighlight(pub(crate) bool);
/// A button with a plain-fill face for missing art, the only kind whose `BackgroundColor` a hover
/// pass may shade (every `Node` carries one).
#[derive(Component)]
pub(crate) struct FallbackFace;
/// An icon button's name label, the `HighlightText` shown on hover or selection.
#[derive(Component)]
pub(crate) struct HoverLabel;
/// A glue-panel button.
#[derive(Component)]
pub(crate) struct GlueBtn;
/// A glue-panel button's `Enable()`/`Disable()` state: the screen toggles it,
/// [`super::glue_button_visuals`] renders it.
#[derive(Component, Default)]
pub(crate) struct GlueDisabled(pub(crate) bool);
/// A template's own `TexCoords` crop, used in place of [`BUTTON_TC`].
#[derive(Component, Clone, Copy)]
pub(crate) struct BtnTexCoords(pub(crate) [f32; 4]);
/// A glue button's caption: gold at rest, white on hover (`HighlightFont`).
#[derive(Component)]
pub(crate) struct GlueCaption;
/// A two-state button face (spinner arrows, rotate): pressed shows `down`.
#[derive(Component)]
pub(crate) struct ArtSwap {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Handle<Image>,
}
/// One of an outlined text's eight black copies; `dir` is its offset direction.
///
/// The reference's `outline="NORMAL"` is one 8-neighbour dilation baked into the glyph at its
/// device-pixel size (`0x5ce440`, `0x5cea30`), so the ring is one device pixel at every resolution,
/// never one UI unit.
#[derive(Component)]
pub(crate) struct OutlineCopy {
    pub(crate) dir: Vec2,
}

// ── Layout helpers ───────────────────────────────────────────────────────────────────────────────

/// An absolutely-positioned node at the authored `(left, top, w, h)`, scaled by `s`.
pub(crate) fn abs(s: f32, left: f32, top: f32, w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(left * s),
        top: Val::Px(top * s),
        width: Val::Px(w * s),
        height: Val::Px(h * s),
        ..default()
    }
}

/// A full-parent absolute overlay (highlights, border sheets).
pub(crate) fn overlay() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}

/// A glue string's look before the outline. `text` is markup, decoded as every
/// `CSimpleFontString` decodes it, and `color` is the base its escapes override.
pub(crate) struct GlueText<'a> {
    pub text: &'a str,
    pub size: f32,
    pub color: Color,
    pub wrap: bool,
}

/// A glue text with the glue fonts' `outline="NORMAL"` (GlueFonts.xml): eight black copies one
/// device pixel out, then the real text with the MasterFont (1,−1) shadow on top.
/// `wrapper_extra` rides the wrapper of all nine, `text_extra` the real text, which is returned.
pub(crate) fn outlined_text<W: Bundle, T: Bundle>(
    parent: &mut ChildSpawnerCommands,
    node: Node,
    wrapper_extra: W,
    text_extra: T,
    spec: GlueText,
    font: &Handle<Font>,
    s: f32,
) -> Entity {
    outlined_spans(
        parent,
        node,
        wrapper_extra,
        text_extra,
        &markup_spans(spec.text, spec.color, spec.wrap),
        spec.size,
        spec.wrap,
        Justify::Left,
        font,
        s,
    )
}

/// [`outlined_text`], centred. A `FontString` with no `justifyH` is CENTER, and
/// `GlueDialogText` omits it (`GlueDialog.xml:56`); every other wrapped glue string sets LEFT.
pub(crate) fn outlined_text_centered<W: Bundle, T: Bundle>(
    parent: &mut ChildSpawnerCommands,
    node: Node,
    wrapper_extra: W,
    text_extra: T,
    spec: GlueText,
    font: &Handle<Font>,
    s: f32,
) -> Entity {
    outlined_spans(
        parent,
        node,
        wrapper_extra,
        text_extra,
        &markup_spans(spec.text, spec.color, spec.wrap),
        spec.size,
        spec.wrap,
        Justify::Center,
        font,
        s,
    )
}

/// Split a glue string into coloured spans by the `|c…|r` markup grammar ([`benilla_ui::markup`]).
/// Every `CSimpleFontString` decodes it unconditionally (`0x5c2810`), so every glue string does.
///
/// An escape overrides `base` until `|r`, its alpha discarded (`0x5c2ab2`); a link keeps only its
/// text; `||` draws one `|`. A line break is real when `wrap` is set and a space otherwise, `wrap`
/// standing in for the multi-line flag the reference tests. An empty string yields one empty span.
pub(crate) fn markup_spans(text: &str, base: Color, wrap: bool) -> Vec<(String, Color)> {
    let mut spans: Vec<(String, Color)> = Vec::new();
    let mut cur = String::new();
    let mut colour = base;
    for (_, tok) in tokens(text) {
        let switch = match tok.kind {
            TokenKind::Color(rgba) => {
                let [r, g, b, _] = rgba.to_f32_at(1.0);
                Some(Color::srgb(r, g, b))
            }
            TokenKind::ColorReset => Some(base),
            TokenKind::EscapedPipe => {
                cur.push('|');
                None
            }
            TokenKind::LineBreak => {
                cur.push(if wrap { '\n' } else { ' ' });
                None
            }
            TokenKind::LinkOpen { .. } | TokenKind::LinkClose => None,
            TokenKind::Char(c) => {
                cur.push(c);
                None
            }
        };
        if let Some(next) = switch {
            if !cur.is_empty() {
                spans.push((std::mem::take(&mut cur), colour));
            }
            colour = next;
        }
    }
    if !cur.is_empty() {
        spans.push((cur, colour));
    }
    if spans.is_empty() {
        spans.push((String::new(), base));
    }
    spans
}

/// [`outlined_text`]'s body: the real text as a `Text` root with `TextSpan` children, the outline
/// copies as the flattened string. Private, so no caller skips the markup decode.
fn outlined_spans<W: Bundle, T: Bundle>(
    parent: &mut ChildSpawnerCommands,
    node: Node,
    wrapper_extra: W,
    text_extra: T,
    spans: &[(String, Color)],
    size: f32,
    wrap: bool,
    justify: Justify,
    font: &Handle<Font>,
    s: f32,
) -> Entity {
    // Both fields set, so no `..default()` (clippy's `needless_update`).
    let layout = TextLayout {
        linebreak: if wrap {
            LineBreak::WordBoundary
        } else {
            LineBreak::NoWrap
        },
        justify,
    };
    let tf = TextFont {
        font: font.clone(),
        font_size: size * s,
        ..default()
    };
    let flat: String = spans.iter().map(|(t, _)| t.as_str()).collect();
    let mut real = Entity::PLACEHOLDER;
    parent
        .spawn((node, wrapper_extra))
        .with_children(|wrapper| {
            // The −1 px trim: a centred layout box sits the glyphs about 1 px below the
            // reference's baseline placement.
            let trim = Node {
                top: Val::Px(-s),
                ..default()
            };
            wrapper.spawn(trim).with_children(|inner| {
                // The 8-neighbour ring; `seat_outline_copies` sets it to one device pixel, and
                // 0.5 logical covers the frame before it runs.
                for dy in [-1.0, 0.0, 1.0] {
                    for dx in [-1.0, 0.0, 1.0] {
                        if dx == 0.0 && dy == 0.0 {
                            continue;
                        }
                        inner.spawn((
                            OutlineCopy {
                                dir: Vec2::new(dx, dy),
                            },
                            Text::new(flat.clone()),
                            tf.clone(),
                            layout,
                            TextColor(Color::BLACK),
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(dx * 0.5),
                                top: Val::Px(dy * 0.5),
                                width: Val::Percent(100.0),
                                ..default()
                            },
                        ));
                    }
                }
                let (first, rest) = spans.split_first().expect("outlined_spans: empty spans");
                let mut e = inner.spawn((
                    Text::new(first.0.clone()),
                    tf.clone(),
                    layout,
                    TextColor(first.1),
                    TextShadow {
                        offset: Vec2::splat(s),
                        color: Color::BLACK,
                    },
                    text_extra,
                ));
                e.with_children(|spans| {
                    for (text, color) in rest {
                        spans.spawn((TextSpan::new(text.clone()), tf.clone(), TextColor(*color)));
                    }
                });
                real = e.id();
            });
        });
    real
}

/// A 48² race/class/gender check-button: the `IconShadow`, the face, a `ButtonHilight-Square`
/// lit on hover and locked while selected (the template's `CheckedTexture` is commented out), and
/// the `HighlightText` name label at BOTTOM +1. `dyn_icon` and `label_dyn` are refresh markers.
///
/// It carries its own [`LockHighlight`]: `SetCharacterRace`/`Class`/`Gender` lock these by hand
/// (`CharacterCreate.lua` l.171/254/326), and the screen's query needs the component present.
pub(crate) fn icon_button<A: Component, I: Bundle, L: Bundle>(
    parent: &mut ChildSpawnerCommands,
    font: &Handle<Font>,
    action: A,
    dyn_icon: Option<I>,
    fixed: Option<(Handle<Image>, Rect)>,
    label_dyn: Option<L>,
    label: &str,
    art: &GlueArt,
    s: f32,
) {
    let px = |v: f32| Val::Px(v * s);
    let mut b = parent.spawn((
        action,
        Button,
        LockHighlight::default(),
        Node {
            width: px(48.0),
            height: px(48.0),
            ..default()
        },
    ));
    if art.races.is_none() {
        b.insert((FallbackFace, BackgroundColor(BTN_BG))); // no art: the label is the button face
    }
    b.with_children(|b| {
        // The shadow: 64² centred, offset (2,−2), behind everything.
        if let Some(shadow) = &art.icon_shadow {
            b.spawn((
                ImageNode::new(shadow.clone()),
                abs(s, -6.0, -6.0, 64.0, 64.0),
            ));
        }
        // A dynamic face is transparent until the refresh assigns it, since a default
        // `ImageNode` is a white square; the gender halves are fixed.
        let mut face = b.spawn((
            match &fixed {
                Some((sheet, rect)) => ImageNode {
                    image: sheet.clone(),
                    rect: Some(*rect),
                    ..default()
                },
                None => ImageNode {
                    color: Color::NONE,
                    ..default()
                },
            },
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ));
        if let Some(icon) = dyn_icon {
            face.insert(icon);
        }
        if let Some(hilight) = &art.hilight {
            b.spawn((
                Hilight,
                Visibility::Hidden,
                MaterialNode(hilight.clone()),
                overlay(),
            ));
        }
        // The wrapper carries the hover visibility, so all nine strings toggle together.
        let real = outlined_text(
            b,
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(1.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            (HoverLabel, Visibility::Hidden),
            (),
            GlueText {
                text: label,
                size: 12.0, // GlueFontNormalSmall
                color: GOLD,
                wrap: false,
            },
            font,
            s,
        );
        if let Some(dyn_text) = label_dyn {
            b.commands().entity(real).insert(dyn_text);
        }
    });
}

/// A 32² spinner arrow at its authored x in the dial row, or a plain `<`/`>` without art.
pub(crate) fn dial_arrow<A: Component>(
    row: &mut ChildSpawnerCommands,
    arrow: &Option<ArrowArt>,
    font: &Handle<Font>,
    action: A,
    left: f32,
    fallback: &str,
    s: f32,
) {
    let mut b = row.spawn((
        action,
        Button,
        Node {
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..abs(s, left, 0.0, 32.0, 32.0)
        },
    ));
    match arrow {
        Some(a) => {
            b.insert(ImageNode::new(a.up.clone()));
            if let Some(down) = &a.down {
                b.insert(ArtSwap {
                    up: a.up.clone(),
                    down: down.clone(),
                });
            }
            if let Some(hi) = &a.hi {
                b.with_children(|inner| {
                    inner.spawn((
                        Hilight,
                        Visibility::Hidden,
                        MaterialNode(hi.clone()),
                        overlay(),
                    ));
                });
            }
        }
        None => {
            b.insert((FallbackFace, BackgroundColor(BTN_BG)));
            b.with_children(|inner| {
                inner.spawn((
                    Text::new(fallback.to_string()),
                    TextFont {
                        font: font.clone(),
                        font_size: 14.0 * s,
                        ..default()
                    },
                    TextColor(GOLD),
                ));
            });
        }
    }
}

/// A glue button template (`GlueButtons.xml`, `GlueDialog.xml`): its caption font and its
/// per-template `<ButtonText>` CENTER offset.
#[derive(Clone, Copy)]
pub(crate) enum GlueBtnKind {
    /// `GlueButtonTemplate` (170×45): GlueFontNormal, ButtonText CENTER (−3, 3).
    Normal,
    /// `GlueButtonSmallTemplate` (150×38): GlueFontNormalSmall, ButtonText CENTER (0, 3).
    Small,
    /// `GlueDialogButtonTemplate` (200×40): GlueFontNormal, ButtonText CENTER (0, 2).
    Dialog,
    /// `AddonListButtonTemplate` (160×35): GlueFontNormal, ButtonText CENTER (0, 2), and its own
    /// art crop, TexCoords 0.025–0.535.
    List,
}

impl GlueBtnKind {
    /// `(font size, caption offset)`, the offset in the reference's anchor space, y up.
    fn caption(self) -> (f32, Vec2) {
        match self {
            Self::Normal => (15.0, Vec2::new(-3.0, 3.0)),
            Self::Small => (12.0, Vec2::new(0.0, 3.0)),
            Self::Dialog | Self::List => (15.0, Vec2::new(0.0, 2.0)),
        }
    }

    fn tex_coords(self) -> Option<BtnTexCoords> {
        match self {
            Self::List => Some(BtnTexCoords([0.025, 0.535, 0.0, 0.75])),
            _ => None,
        }
    }
}

/// `GlueEditBoxFont`'s size (GlueFonts.xml: ARIALN 18).
pub(crate) const EDIT_FONT_SIZE: f32 = 18.0;
/// The typed line's line-height multiple; the caret's height must use the same number.
const EDIT_LINE_HEIGHT: f32 = 1.2;
/// The typed text and caret colour: the reference's caret takes `FONTINSTANCE.textColor` whenever
/// the font changes (`0x77e2a0`, mask bit 2).
const EDIT_TEXT_COLOR: Color = Color::WHITE;
/// The edit caret is a drawn bar, a `CSimpleTexture` at `E+0x368` (`0x779c86`), not a glyph. It is
/// 4.0 UI units wide at every aspect (`0x77ba2d–0x77ba67`) and one line tall, centred on the line.
const CARET_W: f32 = 4.0;

/// The edit caret, spawned hidden for the owner to blink, as the flex sibling after the typed
/// text so it lands at the cursor with no text measuring. Every glue edit box spawns it here.
///
/// It takes no width: a zero-wide seam with the bar hung off it, so the text never moves. The
/// reference's caret is a quad at `drawLayer 3` over the FontString's `2`, anchored at the advance
/// of the text before the cursor (`0x779c86`, `0x77da80`).
pub(crate) fn caret_bar<C: Bundle>(
    parent: &mut ChildSpawnerCommands,
    caret: C,
    font_size: f32,
    s: f32,
) {
    parent
        .spawn((
            caret,
            Visibility::Hidden,
            // The seam: zero-wide, one line tall (the row centres it), above its siblings.
            ZIndex(1),
            Node {
                width: Val::Px(0.0),
                height: Val::Px(font_size * EDIT_LINE_HEIGHT * s),
                ..default()
            },
        ))
        .with_children(|c| {
            // Out of flow; inherits the seam's visibility, so blinking the seam blinks the bar.
            c.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(CARET_W * s),
                    height: Val::Px(font_size * EDIT_LINE_HEIGHT * s),
                    ..default()
                },
                BackgroundColor(EDIT_TEXT_COLOR),
            ));
        });
}

/// Which item of a glue edit box's row `[before][caret][selected][caret][after]` an entity is.
/// Flex places the caret and selection with no text measuring; an empty `Selected` has zero
/// width, and the caret slots are zero-width seams, so the text is one unbroken line.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueFieldPart {
    /// `display[..sel_start]`
    Before,
    /// The caret at the selection's start, and whenever nothing is selected.
    CaretAtStart,
    /// `display[sel_start..sel_end]`, the highlighted run.
    Selected,
    CaretAtEnd,
    /// `display[sel_end..]`
    After,
}

/// Paint one glue edit box's segments, selection and caret from its [`EditBoxState`].
pub(crate) fn paint_glue_field<'a>(
    field: &EditBoxState,
    focused: bool,
    parts: impl Iterator<
        Item = (
            &'a GlueFieldPart,
            Option<Mut<'a, Text>>,
            Mut<'a, Visibility>,
        ),
    >,
) {
    // `focused` gates the caret, not the highlight: the reference's caret flush hides when
    // `E != [0xcf4dc8]` (`0x77da80`), but the selection flush (`0x77d950`, `0x77de70`) never reads
    // focus, so an unfocused box paints the selection it holds.
    let display = field.display();
    let lo = field.sel_start.min(field.sel_end);
    let hi = field.sel_start.max(field.sel_end);
    let (d_lo, d_hi) = (field.text_to_display(lo), field.text_to_display(hi));
    let d_cursor = field.text_to_display(field.cursor);
    // `caret_shown` is ticked by `textinput::tick_caret`, the chat caret's clock too.
    let caret_on = focused && field.caret_shown;
    for (part, text, mut vis) in parts {
        let (want_text, want_vis) = match part {
            GlueFieldPart::Before => (Some(&display[..d_lo]), true),
            GlueFieldPart::Selected => (Some(&display[d_lo..d_hi]), true),
            GlueFieldPart::After => (Some(&display[d_hi..]), true),
            GlueFieldPart::CaretAtStart => (None, caret_on && d_cursor <= d_lo),
            GlueFieldPart::CaretAtEnd => (None, caret_on && d_cursor > d_lo),
        };
        if let (Some(want), Some(mut t)) = (want_text, text) {
            if t.0 != want {
                t.0 = want.to_string();
            }
        }
        let want = if want_vis {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

/// A glue edit box's chrome, as every glue EditBox authors it: `UI-Tooltip-Background` tiled at
/// 16 inside the (10,5,4,9) insets under the `Glue-Tooltip-Border` 16-edge, both tinted by the
/// caller, and the text row at `text_insets`. `marker` goes on all five row items.
pub(crate) fn glue_edit_box<E: Bundle, T: Bundle + Clone>(
    parent: &mut ChildSpawnerCommands,
    art: &GlueArt,
    edit_font: &Handle<Font>,
    extras: E,
    marker: T,
    (w, h): (f32, f32),
    (border, fill): (Color, Color),
    text_insets: (f32, f32, f32, f32),
    s: f32,
) {
    let px = |v: f32| Val::Px(v * s);
    let framed = art.name_border.is_some() && art.tooltip_bg.is_some();
    let mut boxed = parent.spawn((
        extras,
        Node {
            width: px(w),
            height: px(h),
            ..default()
        },
    ));
    if !framed {
        boxed.insert(BackgroundColor(fill.with_alpha(FALLBACK_ALPHA)));
    }
    boxed.with_children(|b| {
        if framed {
            b.spawn((
                tiled_bg_node(art.tooltip_bg.clone().unwrap(), NAME_EDGE, s, fill),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(10.0),
                    right: px(5.0),
                    top: px(4.0),
                    bottom: px(9.0),
                    ..default()
                },
            ));
            backdrop_border(b, art.name_border.as_ref().unwrap(), NAME_EDGE, border);
        }
        // `<TextInsets>` (left, right, top, bottom): the typed line is centred in the inset rect,
        // not the whole box.
        let (ti_l, ti_r, ti_t, ti_b) = text_insets;
        b.spawn((Node {
            position_type: PositionType::Absolute,
            left: px(ti_l),
            right: px(ti_r),
            top: px(ti_t),
            bottom: px(ti_b),
            align_items: AlignItems::Center,
            ..default()
        },))
            .with_children(|f| {
                let segment = |f: &mut ChildSpawnerCommands, part: GlueFieldPart| {
                    let mut e = f.spawn((
                        marker.clone(),
                        part,
                        Text::new(""),
                        TextFont {
                            font: edit_font.clone(),
                            font_size: EDIT_FONT_SIZE * s, // GlueEditBoxFont
                            ..default()
                        },
                        // Its own component in Bevy 0.18, not a `TextFont` field.
                        LineHeight::RelativeToFont(EDIT_LINE_HEIGHT),
                        TextColor(EDIT_TEXT_COLOR),
                        TextLayout {
                            linebreak: LineBreak::NoWrap,
                            ..default()
                        },
                    ));
                    if part == GlueFieldPart::Selected {
                        // `SetHighlightColor`'s default, opaque medium grey, as the chat box.
                        e.insert(BackgroundColor(Color::srgb(
                            96.0 / 255.0,
                            96.0 / 255.0,
                            96.0 / 255.0,
                        )));
                    }
                };
                segment(f, GlueFieldPart::Before);
                caret_bar(
                    f,
                    (marker.clone(), GlueFieldPart::CaretAtStart),
                    EDIT_FONT_SIZE,
                    s,
                );
                segment(f, GlueFieldPart::Selected);
                caret_bar(
                    f,
                    (marker.clone(), GlueFieldPart::CaretAtEnd),
                    EDIT_FONT_SIZE,
                    s,
                );
                segment(f, GlueFieldPart::After);
            });
    });
}

/// A glue-panel button on `Glue-Panel-Button` art with its hover sheen, or a plain fill. Returns
/// the button's entity.
pub(crate) fn glue_button<A: Component>(
    parent: &mut ChildSpawnerCommands,
    art: &GlueArt,
    font: &Handle<Font>,
    action: A,
    caption: &str,
    w: f32,
    h: f32,
    kind: GlueBtnKind,
    s: f32,
) -> Entity {
    let px = |v: f32| Val::Px(v * s);
    let mut b = parent.spawn((
        action,
        GlueBtn,
        GlueDisabled(false),
        Button,
        Node {
            width: px(w),
            height: px(h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
    ));
    let tc = kind.tex_coords();
    if let Some(tc) = tc {
        b.insert(tc);
    }
    match &art.button_up {
        Some((up, size)) => {
            b.insert(ImageNode {
                image: up.clone(),
                rect: Some(tc_rect(*size, tc.map(|t| t.0).unwrap_or(BUTTON_TC))),
                ..default()
            });
        }
        None => {
            b.insert((FallbackFace, BackgroundColor(BTN_BG)));
        }
    }
    b.with_children(|inner| {
        if let Some(hi) = &art.button_hi {
            inner.spawn((
                Hilight,
                Visibility::Hidden,
                MaterialNode(hi.clone()),
                overlay(),
            ));
        }
        let (font_size, offset) = kind.caption();
        outlined_text(
            inner,
            // The `<ButtonText>` offset off CENTER; the reference's y is up, so it negates into
            // `top`.
            Node {
                left: Val::Px(offset.x * s),
                // +1 cancels `outlined_text`'s −1 trim for captions: the `<ButtonText>` offset
                // already corrects that unit.
                top: Val::Px((1.0 - offset.y) * s),
                ..default()
            },
            (),
            GlueCaption,
            GlueText {
                text: caption,
                size: font_size,
                color: GOLD,
                wrap: false,
            },
            font,
            s,
        );
    });
    b.id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_escapes_become_spans_not_text() {
        let base = GOLD;
        let green = Color::srgb(0.0, 1.0, 0.0);
        let blue = Color::srgb(0.0, 0x55 as f32 / 255.0, 1.0);

        assert_eq!(
            markup_spans("|cff00ff00MapCoords 0.32", base, false),
            vec![("MapCoords 0.32".to_string(), green)],
            "the whole-title escape colours everything and prints nothing"
        );
        assert_eq!(
            markup_spans("|cff0055FFDeadly Boss Mod API|r", base, true),
            vec![("Deadly Boss Mod API".to_string(), blue)],
            "the tooltip's title wraps, and wrapping does not exempt it from the decode"
        );
        assert_eq!(
            markup_spans("A |cff00ff00B|r C", base, false),
            vec![
                ("A ".to_string(), base),
                ("B".to_string(), green),
                (" C".to_string(), base)
            ],
            "|r restores the string's base colour"
        );
        assert_eq!(
            markup_spans("pipe || pipe", base, false),
            vec![("pipe | pipe".to_string(), base)],
            "|| draws one literal pipe"
        );
        assert_eq!(
            markup_spans("|xnot an escape", base, false),
            vec![("|xnot an escape".to_string(), base)],
            "a | that opens nothing well-formed is an ordinary character (the grammar's \
             fall-through arm)"
        );
        assert_eq!(
            markup_spans("", base, false),
            vec![(String::new(), base)],
            "an empty string still yields one span, so the text entity exists"
        );
    }

    #[test]
    fn a_link_escape_keeps_its_text_and_hides_its_payload() {
        assert_eq!(
            markup_spans("see |Hitem:1234:0:0:0|h[Thunderfury]|h now", GOLD, true),
            vec![("see [Thunderfury] now".to_string(), GOLD)],
        );
    }

    #[test]
    fn a_line_break_follows_the_strings_own_wrap_flag() {
        assert_eq!(
            markup_spans("one|ntwo", GOLD, true),
            vec![("one\ntwo".to_string(), GOLD)],
            "a wrapping string breaks where the author asked"
        );
        assert_eq!(
            markup_spans("one|ntwo", GOLD, false),
            vec![("one two".to_string(), GOLD)],
            "a one-line label collapses the break to a space"
        );
    }
}
