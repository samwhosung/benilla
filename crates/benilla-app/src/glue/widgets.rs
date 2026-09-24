//! The glue screens' reusable widget builders — the art button shapes of `GlueButtons.xml` /
//! `CharacterCreate.xml` that each screen's layout places at its authored offsets — plus the
//! widget-vocabulary marker components they spawn. Builders are generic over the screen's action
//! component (`CreateAction`, `SelectAction`, …); each degrades to a plain-fill + text face
//! without client art.

use benilla_ui::markup::{tokens, TokenKind};
use benilla_ui::widget::EditBoxState;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui_render::ui_material::MaterialNode;

use super::art::{tc_rect, ArrowArt, GlueArt, BTN_BG, BUTTON_TC, FALLBACK_ALPHA, GOLD, NAME_EDGE};
use super::backdrop::{backdrop_border, tiled_bg_node};

// ── The shared widget vocabulary ─────────────────────────────────────────────────────────────────

/// A button's highlight overlay: the reference's `HighlightTexture`, lit while the cursor is on
/// the button — or held lit by [`LockHighlight`].
///
/// **Its visibility belongs to [`super::glue_hilights`] and to nothing else.** It used to be "the
/// owning screen's to drive", which is how four screens ended up with four hand-rolled hover
/// loops, two of them subtly different, and how the realm list ended up with none at all — no
/// sheen on any of its buttons, because nobody remembered to write the fifth.
#[derive(Component)]
pub(crate) struct Hilight;
/// **`Button:LockHighlight()`** — hold this button's [`Hilight`] lit whether or not the cursor is
/// on it, which is how every glue list marks its selected row.
///
/// The reference's own verb, and the reason the sheen can have one owner: "lit" is
/// `hovered || locked`, and a screen that knows which row is chosen says exactly that much and
/// nothing about visibility.
#[derive(Component, Default)]
pub(crate) struct LockHighlight(pub(crate) bool);
/// A button spawned with a plain-fill face because client art is missing — the only buttons whose
/// `BackgroundColor` a hover pass may shade (every `Node` carries one since Bevy 0.15's required
/// components, so presence alone can't distinguish the fallback).
#[derive(Component)]
pub(crate) struct FallbackFace;
/// An icon button's name label (the ref's `HighlightText`: visible on hover/selected — always,
/// without icon art).
#[derive(Component)]
pub(crate) struct HoverLabel;
/// A glue-panel button (swaps up/down/disabled art; its caption whitens on hover).
#[derive(Component)]
pub(crate) struct GlueBtn;
/// A glue-panel button's enabled state (the ref's `Enable()`/`Disable()`): the screen toggles it,
/// [`super::glue_button_visuals`] renders it (disabled art, gray caption, no hover).
#[derive(Component, Default)]
pub(crate) struct GlueDisabled(pub(crate) bool);
/// A glue button's own authored `TexCoords` crop, where a template overrides the shared
/// `GlueButtons.xml` region (`AddonListButtonTemplate` crops `0.025–0.535`). Read by
/// [`super::glue_button_visuals`] in place of [`BUTTON_TC`].
#[derive(Component, Clone, Copy)]
pub(crate) struct BtnTexCoords(pub(crate) [f32; 4]);
/// A glue button's caption (gold at rest, white on hover — the ref's `HighlightFont`).
#[derive(Component)]
pub(crate) struct GlueCaption;
/// A two-state button face (spinner arrows, rotate): pressed swaps `up` → `down`.
#[derive(Component)]
pub(crate) struct ArtSwap {
    pub(crate) up: Handle<Image>,
    pub(crate) down: Handle<Image>,
}
/// One of an outlined text's black copies ([`outlined_text`]) — content mirrored from its real
/// sibling by [`super::sync_outlines`]; `dir` is its unit offset direction, seated to exactly one
/// DEVICE pixel by [`super::seat_outline_copies`].
///
/// The reference's `outline="NORMAL"` is not string geometry at all: the ring is **baked into the
/// glyph atlas cell by one 8-neighbour dilation pass at rasterization** (`0x5ce440`/`0x5cea30`) —
/// and the glyph rasterizes at final
/// device-pixel size, so the ring is one device pixel hugging the glyph at every resolution
/// (vanilla's famously thin outlines at high res). Offsetting copies by an authored *unit*
/// instead put the ring 2–3 device px out at fullscreen scales, where it read as a separate
/// doubled stroke (director's report, 2026-07-19).
#[derive(Component)]
pub(crate) struct OutlineCopy {
    pub(crate) dir: Vec2,
}

// ── Layout helpers ───────────────────────────────────────────────────────────────────────────────

/// An absolutely-positioned node at the authored `(left, top, w, h)`, scaled by `s` — the shape of
/// nearly every ref anchor once resolved to 1024×768 coordinates.
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

/// A glue string's look, before the outline treatment: content, authored size, color, and whether
/// it may wrap (the ref FontStrings never break a label; only the info paragraphs wrap).
///
/// `text` is **markup**, not a literal: its `|cAARRGGBB…|r` escapes decode to colour and `color`
/// is the base they override ([`markup_spans`]). Every glue string goes through that decode
/// because every `CSimpleFontString` in 5875 does.
pub(crate) struct GlueText<'a> {
    pub text: &'a str,
    pub size: f32,
    pub color: Color,
    pub wrap: bool,
}

/// A glue text with the fonts' black outline (`outline="NORMAL"`, GlueFonts.xml — every glue font
/// but the edit box carries it). Bevy text has no stroke, so the string draws nine times: eight
/// black copies one DEVICE pixel out in the 8-neighbour directions (the ref's baked dilation ring
/// — see [`OutlineCopy`]), then the real text — with the MasterFont (1,−1) drop shadow — painted
/// last on top. `node` is the layout shape (position/margins/centering); an auto-sized inner box
/// keeps the copies registered with the real string wherever `node` puts it. `wrapper_extra` rides
/// the wrapper (visibility markers hide all nine at once); `text_extra` rides the real text (the
/// markers the refresh writes — [`super::sync_outlines`] mirrors those writes into the copies).
/// Returns the real text entity.
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

/// [`outlined_text`], **centred** — for a wrapped paragraph the reference centres.
///
/// A separate door rather than a field on [`GlueText`] because centring is the exception here, and
/// naming it at the one call site that wants it is cheaper and clearer than a fifth field on
/// thirty-three literals. The justification has to reach all nine strings (the eight outline copies
/// share the layout), so it cannot be inserted onto the real text afterwards.
///
/// **Which way round is faithful is not obvious**, so: a `FontString` with no `justifyH` defaults
/// to **CENTER**, and `GlueDialogText` (`GlueDialog.xml:56`) omits it — so the dialog is centred.
/// Every *other* wrapped glue string in the shipped XML sets `justifyH="LEFT"` explicitly
/// (`CharacterCreate.xml`'s race/class/faction bodies, `AddonList.xml`'s title/notes/deps), which
/// is why [`outlined_text`] stays left and this is the exception rather than the default.
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

/// Split a glue string into its coloured spans — WoW's `|cAARRGGBB…|r` inline markup, decoded by
/// the reference's grammar ([`benilla_ui::markup`]) instead of drawn literally.
///
/// **This lives in the primitive on purpose.** It used to be one call site's private helper (the
/// AddOns row title), which is exactly how the same `|cff0055FF…|r` came out coloured in a list
/// row and literal in the tooltip an inch to its left (B273): a FontString property implemented
/// per-caller is a property no caller reliably has. In the real client the decode is
/// `CSimpleFontString`'s own and takes no opt-in — `0x5c2810` parses `|c`/`|r`/`|H`/`|h`/`||`
/// unconditionally for every one of them (the flags word that could disable them has no writer
/// anywhere in the image). So every `GlueText` decodes, and there is no way to spawn one that
/// doesn't.
///
/// `base` is the string's own colour; an escape overrides it until `|r`, and the escape's alpha
/// byte is discarded exactly as the client discards it (`0x5c2ab2`). Link escapes hide their
/// payload and keep their visible text — the glue has no clickable links, so a `|H…|h` reads as
/// its text. `||` draws one `|`.
///
/// A line break (`\n`, `|n`) follows the string's own `wrap`: a real break where the ref lets the
/// string wrap (a tooltip paragraph), a space where it authored one line (a row title, a button
/// caption) — the flag is our stand-in for the multi-line bit the client tests, and collapsing
/// beats a label growing a second row inside a 20-unit slot.
///
/// Never empty: an empty string yields one empty span, so [`outlined_spans`] always has a root.
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

/// [`outlined_text`]'s span-tree body — the decoded spans of [`markup_spans`] drawn as one string
/// with the colour switching mid-run. The real text is Bevy's `Text` root + `TextSpan` children;
/// the eight outline copies carry the flattened plain string, which is identical geometry because
/// the copies only exist to be a black ring.
///
/// **Private**: [`outlined_text`] is the one door in, so no caller can hand-build spans and skip
/// the markup decode (B273's shape — see [`markup_spans`]).
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
    // Both fields set, so no `..default()` — `TextLayout` has exactly these two, and clippy's
    // `needless_update` is right that spelling a rest-pattern here only hides the next field
    // Bevy adds.
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
            // The −1px trim: our centered layout box sat the glyphs ~1px lower than the ref's
            // baseline placement everywhere (director-measured); one optical correction here
            // covers every glue string.
            let trim = Node {
                top: Val::Px(-s),
                ..default()
            };
            wrapper.spawn(trim).with_children(|inner| {
                // The 8-neighbour ring (see [`OutlineCopy`]) — seated to one device pixel by
                // `seat_outline_copies`; the 0.5-logical spawn value is the retina guess for the
                // one frame before that system runs.
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

/// A 48² icon check-button (race/class/gender): the `IconShadow` behind, the icon face (dynamic —
/// the owning screen assigns the sheet + rect — or fixed), a `ButtonHilight-Square` overlay lit on
/// hover *and held while selected* (the ref's `LockHighlight`; the template's `CheckedTexture` is
/// commented out in the shipped 1.12 GlueXML, so the locked square *is* the whole selected
/// visual), and the name label along the bottom (the ref's `HighlightText`, `GlueFontNormalSmall`,
/// anchored BOTTOM +1 — over the icon's bottom edge). `dyn_icon`/`label_dyn` are the screen's
/// refresh markers, spawned onto the face / real label text.
///
/// **It carries its own [`LockHighlight`]**, because the reference does: these are the
/// `CheckButton`s that `SetCharacterRace`/`SetCharacterClass`/`SetCharacterGender` lock and
/// unlock by hand (`CharacterCreate.lua` l.171/254/326). Leaving the flag to the screen is how
/// the create screen lost every selected sheen *and* every icon name for ten days — 2072 added a
/// `&mut LockHighlight` term to the screen's own visuals query and this spawn site had none, so
/// the query matched nothing at all.
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
        // The shadow (64² centered, offset (2,−2)) — behind everything.
        if let Some(shadow) = &art.icon_shadow {
            b.spawn((
                ImageNode::new(shadow.clone()),
                abs(s, -6.0, -6.0, 64.0, 64.0),
            ));
        }
        // The icon face: dynamic (transparent until the refresh assigns the sheet + rect — a
        // default `ImageNode` is a white square) or fixed at spawn (gender halves).
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
        // The ADD-mode overlays draw through the true-additive UI material (`add_material`).
        if let Some(hilight) = &art.hilight {
            b.spawn((
                Hilight,
                Visibility::Hidden,
                MaterialNode(hilight.clone()),
                overlay(),
            ));
        }
        // The name label: the wrapper carries the hover visibility (all five strings toggle
        // together); the dynamic marker rides the real text for the refresh + outline sync.
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

/// A 32² spinner arrow at its authored x in the dial row — up/down art states + the additive
/// hover highlight, or a plain `<`/`>` without art.
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

/// A glue button template (`GlueButtons.xml` / `GlueDialog.xml`): the caption font it authors, and
/// its `<ButtonText>` CENTER-anchor **offset**. The offset is real and per-template — the big
/// `GlueButtonTemplate` pulls its caption 3 units LEFT and 3 UP of dead centre — and drawing every
/// caption dead centre instead sat them all low, and the big buttons' right (decision 0543).
#[derive(Clone, Copy)]
pub(crate) enum GlueBtnKind {
    /// `GlueButtonTemplate` (170×45) — GlueFontNormal, ButtonText CENTER (−3, 3).
    Normal,
    /// `GlueButtonSmallTemplate` (150×38) — GlueFontNormalSmall, ButtonText CENTER (0, 3).
    Small,
    /// `GlueDialogButtonTemplate` (200×40) — GlueFontNormal, ButtonText CENTER (0, 2).
    Dialog,
    /// `AddonListButtonTemplate` (160×35) — GlueFontNormal, ButtonText CENTER (0, 2), and its own
    /// narrower art crop (TexCoords 0.025–0.535 of the `Glue-Panel-Button` sheets).
    List,
}

impl GlueBtnKind {
    /// `(font size, caption offset)` — the offset in the reference's anchor space, y UP-positive.
    fn caption(self) -> (f32, Vec2) {
        match self {
            Self::Normal => (15.0, Vec2::new(-3.0, 3.0)),
            Self::Small => (12.0, Vec2::new(0.0, 3.0)),
            Self::Dialog | Self::List => (15.0, Vec2::new(0.0, 2.0)),
        }
    }

    /// The template's art crop where it overrides the shared `GlueButtons.xml` region.
    fn tex_coords(self) -> Option<BtnTexCoords> {
        match self {
            Self::List => Some(BtnTexCoords([0.025, 0.535, 0.0, 0.75])),
            _ => None,
        }
    }
}

/// `GlueEditBoxFont`'s size (GlueFonts.xml: ARIALN 18) — the typed line and the caret beside it.
pub(crate) const EDIT_FONT_SIZE: f32 = 18.0;
/// The typed line's line-height multiple, pinned here rather than left to Bevy's default so the
/// caret's height (below) is provably the same number the text lays out with.
const EDIT_LINE_HEIGHT: f32 = 1.2;
/// The edit caret's colour: the reference's caret re-applies **`FONTINSTANCE.textColor`** whenever
/// the font changes (`0x77e2a0`, mask bit 2) — the ctor's `0xFFFFFFFF` is only the pre-font default.
/// So it is the box's text colour by law, not white by coincidence; both the line and the bar take
/// it from here.
const EDIT_TEXT_COLOR: Color = Color::WHITE;
/// The edit caret is a drawn **bar**, not a character — a `CSimpleTexture` at `E+0x368`, allocated
/// with a different allocator/tag/ctor than the `CSimpleFontString` beside it (`0x779c86` vs
/// `0x779bee`; the cursor flush `0x77da80` fires `OnCursorChanged` with
/// four float caret-*position* args). benilla used to append a `"|"` glyph (login) or a static `"_"`
/// (create), which put a font's shape on a font's baseline and re-laid the text out every blink.
///
/// **Width is 4.0 UI units** — byte-verified, not the 1 px this shipped with first (decision 0542).
/// `0x77ba2d–0x77ba67` stores `G1·4/(G3·1024)`, which the client's own internal→Lua converter maps
/// to exactly 4.0 at every aspect and resolution. Same units as every other authored glue number,
/// so it scales with `s` like the rest. Height is the FontString's line height; the bar is
/// vertically centred on the line (the ref anchors caret-LEFT to the FontString's own rect, so
/// TextInsets never enter the caret's own geometry — it inherits them by anchoring).
const CARET_W: f32 = 4.0;

/// The standard edit caret — the drawn [`CARET_W`]-unit bar, one line tall at the box's font size,
/// in the box's text colour, spawned hidden (the owning screen blinks its `Visibility` on the ref's
/// 0.5 s clock, reset solid on every keystroke). Seat it as the flex sibling right after the typed
/// line so it lands at the cursor without anyone measuring text. The ONE caret; every edit box —
/// [`glue_edit_box`]'s chrome and the delete dialog's ChatInputBorder box — spawns it here, so the
/// mechanism can never fork back into per-screen `"|"`/`"_"` glyphs.
///
/// **The caret is an OVERLAY: it takes no width in the row.** The flex item is zero-wide — a bare
/// *seam* the row's layout collapses to nothing — and the drawn bar is an absolutely-positioned
/// child hanging off it, left edge on the seam, so the typed line is one continuous run whatever
/// the cursor or the selection is doing. It used to be the bar itself, [`CARET_W`] units wide and
/// in flow, which pushed the text apart at the cursor and — because a select-all moves the whole
/// string from the `Before` slot to the `Selected` slot, across one caret seam — **shifted the
/// text bodily to the right the moment a box was focused** (the director's report, 2026-08-29).
///
/// This is also what the client does, and the two facts are the same fact: the caret is a
/// `CSimpleTexture` quad at `drawLayer 3`, anchored LEFT-to-LEFT on the FontString with
/// `x = W(lineStart → cursor)` — the measured advance of the text before the cursor (`0x779c86`,
/// `0x77da80`). A quad anchored *over* the line cannot
/// displace it, and its left edge sits exactly on the seam flex puts us on. `drawLayer 3` is above
/// the FontString's `2`, which is the [`ZIndex`] here.
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
            // The seam: zero-wide, one line tall (the row centres it), and above its siblings.
            ZIndex(1),
            Node {
                width: Val::Px(0.0),
                height: Val::Px(font_size * EDIT_LINE_HEIGHT * s),
                ..default()
            },
        ))
        .with_children(|c| {
            // The drawn bar, out of flow so it costs the row nothing. `Visibility::Inherited` by
            // default, so blinking the seam blinks the bar.
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

/// Which flex item of a glue edit box's text row an entity is.
///
/// The row is `[before][caret][selected][caret][after]` — five items, so the caret lands **at the
/// cursor** and a selection shows its highlight with **no text measuring anywhere**: flex layout
/// does the positioning, exactly as the single caret sibling used to. `Selected` carries the
/// highlight background permanently; with nothing selected its text is empty, so it has zero width
/// and paints nothing. Which of the two caret slots is visible follows the cursor, which the box
/// law keeps at one end of the selection or the other.
///
/// **Both caret slots are zero-width seams** ([`caret_bar`]), present or not, so the three text
/// slots concatenate to one unbroken line: which slot a run of text sits in never moves it.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueFieldPart {
    /// `display[..sel_start]`
    Before,
    /// The caret when it sits at the selection's start (and whenever nothing is selected).
    CaretAtStart,
    /// `display[sel_start..sel_end]` — the highlighted run.
    Selected,
    /// The caret when it sits at the selection's end.
    CaretAtEnd,
    /// `display[sel_end..]`
    After,
}

/// Paint one glue edit box from its [`EditBoxState`] — the segments, the selection, and the caret.
/// Shared so the login boxes, the create-name box and the delete dialog can never fork (decision
/// 0704); everything it draws comes off the same state the shared law edits.
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
    // **The highlight is deliberately NOT gated on `focused`** — `focused` drives the caret alone.
    // That asymmetry looks like an oversight and is the reference's own: the caret's flush
    // (`0x77da80`) explicitly hides when `E != [0xcf4dc8]`, while the selection flush (`0x77d950`)
    // and its geometry worker (`0x77de70`) contain no read of the focus global anywhere, and the
    // per-frame update calls the flush BEFORE its own focus test. The quads show iff
    // `start < end` — focus never enters it. An unfocused box that still holds a
    // selection paints it, and that is correct; the login screen simply never leaves one behind
    // (`LoginForm::focus` collapses the box it leaves).
    let display = field.display();
    let lo = field.sel_start.min(field.sel_end);
    let hi = field.sel_start.max(field.sel_end);
    let (d_lo, d_hi) = (field.text_to_display(lo), field.text_to_display(hi));
    let d_cursor = field.text_to_display(field.cursor);
    // Only a focused box blinks; `caret_shown` is the box's own blink phase, ticked by
    // `textinput::tick_caret`, so a glue caret and the chat caret share one clock.
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

/// A glue edit box's CHROME — the shape every glue EditBox authors (`AccountLoginAccountEdit`/
/// `PasswordEdit`, `CharacterCreateNameEdit`): the `UI-Tooltip-Background` fill tiled at 16 inside
/// the (10,5,4,9) insets, the `Glue-Tooltip-Border` 16-edge over it (both caller-tinted — the
/// login boxes take `DEFAULT_TOOLTIP_COLOR`, the create name box the Alliance row), and the text
/// row at the ref's TextInsets (left 15, vertically centered) in `GlueEditBoxFont` (ARIALN 18,
/// white).
///
/// `extras` rides the box node (the screen's click-to-focus action + `Button`); `marker` is cloned
/// onto all five row items ([`GlueFieldPart`]) so the screen's refresh can query them as a set and
/// hand them to [`paint_glue_field`]. Plain-fill fallback without art. Focus and typing are the
/// owning screen's systems — this is chrome only, so the screens' boxes can never fork.
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
        // The ref's `<TextInsets>` (left, right, top, bottom): the EditBox's FontString rect is
        // the box inset by them, and the typed line is centred in THAT — not in the whole box. The
        // login boxes inset the bottom by 5, which lifts the line, and the caret beside it, off the
        // box's own centre; the create name box insets only the left, so it stays centred.
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
                        // The box law's own highlight tint (`SetHighlightColor`'s ctor default,
                        // opaque medium grey) — the same colour the chat box selects with.
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

/// A glue-panel button (Accept/Back/Enter World/…): the real `Glue-Panel-Button` art with its
/// additive hover sheen and a caption that whitens on hover (the ref's `HighlightFont`);
/// plain-fill fallback. Art states + caption color are driven by [`super::glue_button_visuals`].
///
/// Returns the button's entity, so a screen can reach back into what it just built — the login
/// screen marks its realmlist button [`GlueDisabled`] when `$WOW_HOST` owns the session (1667).
/// Ignoring the return is the norm; nothing is `#[must_use]`.
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
            // The template's authored `<ButtonText>` offset off CENTER. The ref's anchor space is
            // y-UP, so it negates into Bevy's `top`; on a flex item both shift it visually without
            // disturbing the layout, which is exactly an anchor offset.
            Node {
                left: Val::Px(offset.x * s),
                // +1 cancels `outlined_text`'s shared −1 trim for captions: that trim was fitted
                // (director-measured) back when captions drew at dead centre, so once the authored
                // `<ButtonText>` offset above is applied it corrects the same unit twice and the
                // caption reads a notch high. Director's eye, 2026-07-19.
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

    /// `|cAARRGGBB…|r` markup renders as colour, not as text — for **every** glue string, which
    /// is the whole point of the decode living in the primitive.
    ///
    /// Two regressions are pinned here. The MapCoords one (a `## Title` of
    /// `|cff00ff00MapCoords 0.32`, printed literally in the AddOns list), and B273: the same
    /// screen's tooltip printed `|cff0055FFDeadly Boss Mod API|r` raw while the row an inch to
    /// its left drew it blue, because only the row had opted into a decode. Nothing opts in now
    /// — [`outlined_text`] is the one door and it always decodes — so the case worth testing is
    /// the function, once, for both.
    ///
    /// The grammar is the byte-verified [`benilla_ui::markup`]; the escape's alpha byte is
    /// discarded there, so only rgb reaches the span colour.
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
            "B273: the tooltip's title wraps, and wrapping does not exempt it from the decode"
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

    /// A `|H…|h` link keeps its visible text and drops its payload — the glue has no clickable
    /// links, so the alternative is an addon note printing `|Hitem:1234|h`.
    #[test]
    fn a_link_escape_keeps_its_text_and_hides_its_payload() {
        assert_eq!(
            markup_spans("see |Hitem:1234:0:0:0|h[Thunderfury]|h now", GOLD, true),
            vec![("see [Thunderfury] now".to_string(), GOLD)],
        );
    }

    /// A line break follows the string's own `wrap`: a real break where the ref lets the string
    /// wrap, a space where it authored one line. A 20-unit row title growing a second row is the
    /// failure this avoids; a tooltip paragraph losing its author's break is the other.
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
