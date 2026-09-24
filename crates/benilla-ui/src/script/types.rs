use mlua::{Lua, Value};

use crate::layout::{Anchor, Rect};
use crate::order::ZTarget;

/// A value the host passes into Lua, such as a `fire_event` argument, without an mlua handle.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    Int(i64),
    Number(f64),
    Str(String),
}

impl ScriptValue {
    pub(crate) fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(match self {
            ScriptValue::Nil => Value::Nil,
            ScriptValue::Bool(b) => Value::Boolean(b),
            ScriptValue::Int(i) => Value::Integer(i),
            ScriptValue::Number(n) => Value::Number(n),
            ScriptValue::Str(s) => Value::String(lua.create_string(&s)?),
        })
    }
}

/// A texture region's UV mapping, one of the two `SetTexCoord` forms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TexCoords {
    /// `SetTexCoord(l,r,t,b)` or XML `<TexCoords>`, 0..1 from the top left; a mirrored or
    /// flipped tuple keeps its orientation.
    Rect([f32; 4]),
    /// `SetTexCoord(ULx,ULy, LLx,LLy, URx,URy, LRx,LRy)`, an arbitrary UV quad, stored in screen
    /// order `[TL, TR, BR, BL]`: the binding reorders the Lua arguments.
    Corners([[f32; 2]; 4]),
}

impl TexCoords {
    /// The mapping's axis-aligned `[left, right, top, bottom]` bounds; not what `GetTexCoord()`
    /// answers, which is the eight corner values.
    pub fn edges(&self) -> [f32; 4] {
        match *self {
            TexCoords::Rect(e) => e,
            TexCoords::Corners(c) => {
                let (us, vs): (Vec<f32>, Vec<f32>) = c.iter().map(|&[u, v]| (u, v)).unzip();
                let min = |xs: &[f32]| xs.iter().copied().fold(f32::INFINITY, f32::min);
                let max = |xs: &[f32]| xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                [min(&us), max(&us), min(&vs), max(&vs)]
            }
        }
    }
}

/// What an [`ExtractedQuad`] carries to a renderer, beyond its rect.
#[derive(Clone, Debug, PartialEq)]
pub enum QuadContent {
    /// The frame's own draw slot, which draws nothing itself.
    Frame,
    /// A `Minimap` widget's content hole, which the app fills with the map; emitted at the
    /// frame's own z, so the widget's children (border art, buttons) paint above it.
    Minimap {
        /// The outdoor zoom index, `0..MINIMAP_ZOOM_LEVELS`, 0 the widest.
        zoom: u8,
        /// The indoor zoom index (`0x86f69c`), sent too so the renderer picks by its own test.
        inside_zoom: u8,
    },
    /// A `Model` or `PlayerModel` widget's 3D pane: a `model` file draws as a tile, a unit pane
    /// samples the bake of the window its name belongs to, or nothing. The host memoizes on this
    /// list, so it reads the moving play head from `UiScript::visible_model_panes` instead.
    ModelPane {
        /// The renderer's key for the tile it keeps per pane.
        handle: crate::widget::FrameHandle,
        /// The global frame name; `None` for an anonymous `CreateFrame("Model")`.
        name: Option<String>,
        /// The `SetModel` file; `None` for a unit pane or an empty one.
        model: Option<String>,
        /// `SetFacing`'s radians (0 default).
        facing: f32,
        /// `SetModelScale`'s factor (1 default).
        model_scale: f32,
        /// `SetPosition`'s offset, in the ortho leg's layout units (`0x76d1a0`: the root is
        /// `T(pos · layoutScale) · R(facing) · S(…)`).
        position: (f32, f32, f32),
        /// The frame's own alpha, which the model draws at: `Frame:SetAlpha` pushes it into the
        /// instance without the parent chain (`0x76d120`).
        own_alpha: f32,
        /// `ReplaceIconTexture`'s path, the type-14 texture override.
        icon: Option<String>,
        /// The camera, a raw index into the file's camera table: `None` renders orthographic, the
        /// model laid flat over the rect, and `Some(n)` in perspective through camera `n`.
        camera: Option<u32>,
        /// The embedded `CGLight`, off on every shipped pane; a lit batch under it draws black, as
        /// in the reference.
        light: crate::widget::ModelLight,
        /// The fog, when armed; a material with the UNFOGGED bit ignores it.
        fog: Option<crate::widget::ModelFog>,
    },
    /// A `Texture` region: a BLP path, a solid or vertex colour, or both (a tinted texture).
    Texture {
        path: Option<String>,
        color: Option<[f32; 4]>,
        /// The `ADD` blend instead of straight alpha.
        additive: bool,
        /// `None` is the full texture.
        tex_coords: Option<TexCoords>,
        /// A portrait, masked to the inscribed circle; false with a `portrait_unit` is the square
        /// booth pane (`BenillaSetBoothTexture`).
        circular: bool,
        /// A live unit portrait's unit: the renderer samples the app's bake of it instead of
        /// `path` and `color`.
        portrait_unit: Option<String>,
        /// Rotation about the centre, radians counterclockwise; always 0, since 1.12 registers
        /// `SetRotation` on PlayerModel alone (`0x84f1fc`).
        rotation: f32,
        /// `SetDesaturated`: the texel draws as its luminance, then is tinted by `color`, since
        /// `SetItemButtonDesaturated` passes a dim tint with the flag.
        desaturated: bool,
    },
    /// A `ColorSelect`'s `<ColorWheelTexture>` disc, generated: the pixel at normalised offset
    /// `(nx, ny)` from the centre is `HSV(atan2(ny, nx)·180/π + 180, min(|n|, 1), 1)`, inverting
    /// the pick law (`0x78bd80`); the fill draws every texel at `V = 1` (`0x78b68b`), so no colour
    /// travels.
    ColorWheel,
    /// A `ColorSelect`'s `<ColorValueTexture>` strip: the hue and saturation, black at the
    /// bottom, the inverse of its pick law `V = clamp((y − bottom)/(top − bottom), 0, 1)`.
    ColorValue {
        /// Degrees.
        hue: f32,
        /// `0..=1`; the widget's value moves only its marker, never the strip.
        sat: f32,
    },
    /// One `Backdrop` piece, the tiled background or one of the 8 border pieces, drawn in the
    /// frame's slot behind its regions; per-corner UVs, since the top and bottom edges turn 90°.
    Backdrop {
        path: String,
        /// The `SetBackdropColor` or `SetBackdropBorderColor` tint, pre-alpha.
        color: [f32; 4],
        /// `[TL, TR, BR, BL]`, screen order.
        uvs: [[f32; 2]; 4],
        /// Sample with repeat (wrap) addressing.
        tile: bool,
    },
    /// A `FontString` region with its resolved font; the app measures and draws it.
    Text {
        text: Option<String>,
        color: Option<[f32; 4]>,
        justify_h: JustifyH,
        justify_v: JustifyV,
        /// The face path, e.g. `"Fonts\\FRIZQT__.TTF"`; `None` is Friz Quadrata.
        font: Option<String>,
        /// Logical px; `None` is the renderer's default size.
        font_height: Option<f32>,
        /// The `SetTextHeight` override, drawn uncapped; `None` keeps the 32 px raster cap.
        text_height: Option<f32>,
        /// 1.12's `MasterFont` sets `(1,-1)` black, so nearly every stock font has one.
        shadow: Option<FontShadow>,
        outline: Outline,
        /// `SetAlphaGradient`: before `start` opaque, the next `length` characters fading from 1
        /// to 0, the rest invisible.
        alpha_gradient: Option<(f32, f32)>,
        /// Seat the text at `rect` without the UI grid's block-top snap: true on a nameplate
        /// (`super::nameplate::is_world_seated`), which slides on a device-pixel snap, so a second
        /// snap would jitter its text. Not a reference field.
        world_seat: bool,
    },
}

/// A `FontString`'s horizontal justification (`justifyH`, `SetJustifyH`); CENTER by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyH {
    Left,
    #[default]
    Center,
    Right,
}

/// A `FontString`'s vertical justification (`justifyV`, `SetJustifyV`); MIDDLE by default, so a
/// sized FontString centres its lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyV {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// A Texture's blend mode, the client's `alphaMode` enum (`0x811aa8`), set by XML `alphaMode=`
/// or `SetBlendMode`; BLEND is the constructor's (`0x76fc64 mov [esi+0xd0],2`). The renderer
/// acts only on ADD: the others draw as straight alpha, their reference blending not built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    /// `DISABLE` (0): `GL_ONE, GL_ZERO`, alpha reference 0.
    Disable,
    /// `ALPHAKEY` (1): `GL_ONE, GL_ZERO`, alpha reference 224/255.
    AlphaKey,
    /// `BLEND` (2): `GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA`.
    #[default]
    Blend,
    /// `ADD` (3): `GL_SRC_ALPHA, GL_ONE`.
    Add,
    /// `MOD` (4): `GL_DST_COLOR, GL_ZERO`.
    Mod,
}

impl BlendMode {
    /// Parse the name, case-insensitively, as `SetBlendMode` and XML `alphaMode=` take it.
    pub fn parse(s: &str) -> Option<BlendMode> {
        match s.to_ascii_uppercase().as_str() {
            "DISABLE" => Some(BlendMode::Disable),
            "ALPHAKEY" => Some(BlendMode::AlphaKey),
            "BLEND" => Some(BlendMode::Blend),
            "ADD" => Some(BlendMode::Add),
            "MOD" => Some(BlendMode::Mod),
            _ => None,
        }
    }

    /// The name `GetBlendMode()` answers (the name column of `0x811aa8`).
    pub const fn name(self) -> &'static str {
        match self {
            BlendMode::Disable => "DISABLE",
            BlendMode::AlphaKey => "ALPHAKEY",
            BlendMode::Blend => "BLEND",
            BlendMode::Add => "ADD",
            BlendMode::Mod => "MOD",
        }
    }
}

/// A `FontString`'s glyph outline (XML `outline=`, OUTLINETYPE), answered by `GetFont`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outline {
    #[default]
    None,
    Normal,
    Thick,
}

impl Outline {
    /// Parse the XML `outline=` attribute, case-insensitively; not the Lua spelling, which is
    /// [`Outline::flags`].
    pub fn parse(s: &str) -> Outline {
        match s.to_ascii_uppercase().as_str() {
            "NORMAL" => Outline::Normal,
            "THICK" => Outline::Thick,
            _ => Outline::None,
        }
    }

    /// Parse a Lua `SetFont` flags string, shared by every `SetFont` (`0x79f210`) and matched by
    /// substring against `{0x1 OUTLINE, 0x4 THICKOUTLINE, 0x2 MONOCHROME}` (`0x811b10`): THICK
    /// first, since `THICKOUTLINE` contains `OUTLINE`. MONOCHROME is ignored, its mode not built.
    pub fn flags(s: &str) -> Outline {
        let s = s.to_ascii_uppercase();
        if s.contains("THICK") {
            Outline::Thick
        } else if s.contains("OUTLINE") {
            Outline::Normal
        } else {
            Outline::None
        }
    }

    /// The flags token `GetFont` answers.
    pub fn as_str(self) -> &'static str {
        match self {
            Outline::None => "",
            Outline::Normal => "OUTLINE",
            Outline::Thick => "THICKOUTLINE",
        }
    }
}

/// A font object's `<Shadow>`, inherited down `<Font>` chains; 1.12's `MasterFont` sets `(1,-1)`
/// black (`Fonts.xml:55`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontShadow {
    /// Pixels, y up: `y="-1"` is 1 px down.
    pub offset: [f32; 2],
    /// Alpha defaults to 1.
    pub color: [f32; 4],
}

/// A named `<Font>` object's paint, flattened through its `inherits=` chain; a field the chain
/// never sets is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontObject {
    /// The TTF path (`font=`).
    pub font: Option<String>,
    /// Logical px (`<FontHeight>`).
    pub height: Option<f32>,
    pub shadow: Option<FontShadow>,
    pub color: Option<[f32; 4]>,
    pub outline: Outline,
    pub justify_h: Option<JustifyH>,
    pub justify_v: Option<JustifyV>,
}

/// One entry of [`UiScript::extract`](super::UiScript::extract)'s render list, in the client's
/// painter order ([`crate::order::traversal`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedQuad {
    /// A frame's own slot, or one of its regions.
    pub target: ZTarget,
    /// The packed draw-order key (`ZKey::raw`); entries are sorted ascending by it.
    pub z: u64,
    /// `None` when the owning frame's anchors do not resolve; a renderer skips it.
    pub rect: Option<Rect>,
    /// A frame's `effective_alpha`, or a region's alpha times its owner's, one hop only.
    pub alpha: f32,
    pub content: QuadContent,
    /// The ScrollFrame clip of a scroll child and its descendants, nested ones intersecting.
    pub clip: Option<Rect>,
    /// The owner's `effective_scale` (`0x76ac90`), already in `rect`; a `Text` quad's font and
    /// shadow offset must be scaled by it too.
    pub scale: f32,
}

/// A two-stop linear gradient (`SetGradientAlpha`/`SetGradient`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    /// `"VERTICAL"`, matched case-insensitively; anything else is horizontal, as in the client.
    pub vertical: bool,
    /// RGBA; `SetGradient`, which takes no alpha, sets both alphas to 1.
    pub start: [f32; 4],
    pub end: [f32; 4],
}

impl Gradient {
    /// The midpoint, the one colour a one-tint quad can show.
    pub fn midpoint(&self) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (i, chan) in out.iter_mut().enumerate() {
            *chan = (self.start[i] + self.end[i]) / 2.0;
        }
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RegionData {
    /// `Show`/`Hide` and XML `hidden=`: a hidden region emits no quad, whatever its owner's state.
    pub(crate) hidden: bool,
    /// `SetAlpha`, XML `alpha=`; `None` is 1. Drawn times the owner frame's alpha, one hop only:
    /// frame `SetAlpha` (`0x76a690`) cascades to child frames and only invalidates regions.
    ///
    /// Deviation: kept apart from [`Self::vertex_color`], which the reference's `Texture:SetAlpha`
    /// (`0x79b580`) rewrites, because the drawn product differs only on a region that sets both,
    /// which no stock code does, and a FontString's `SetAlpha` then keeps its font colour.
    pub(crate) alpha: Option<f32>,
    /// A texture path (`SetTexture("Interface\\...")`).
    pub(crate) texture: Option<String>,
    /// `SetTexture(r, g, b, a)` or an XML `<Color>` with no `file=`, sharing `+0xcc` with
    /// [`Self::texture`]: setting one clears the other. The reference makes an 8×8 texture of it
    /// (`0x770360` → `0x44a9c0` → `0x5c5350`), so its alpha multiplies with the vertex colour's.
    pub(crate) fill: Option<[f32; 4]>,
    /// `SetGradientAlpha`/`SetGradient`, stored whole and drawn as its midpoint: the renderer has
    /// one tint per quad, so the reference's gradient is not built.
    pub(crate) gradient: Option<Gradient>,
    /// `SetVertexColor` (`0x79abd0` → `0x77f750`, `+0xb8`), `SetStatusBarColor` or a text colour,
    /// drawn as `texel × colour` per channel, alpha included: a `<Color 1,1,1,0.2>` tinted
    /// `(0, 0, 0.75, 0.5)` draws at alpha 0.1, as `SkillFrame`'s row trough does.
    pub(crate) vertex_color: Option<[f32; 4]>,
    /// A portrait (`SetPortraitToTexture`, `SetPortraitTexture`), masked to the inscribed circle;
    /// false with a `portrait_unit` is the square booth pane (`BenillaSetBoothTexture`).
    pub(crate) circular: bool,
    /// A live unit portrait's unit, drawn from the app's bake instead of the texture or colour;
    /// cleared by `SetTexture` and `SetPortraitToTexture`.
    pub(crate) portrait_unit: Option<String>,
    /// FontString text (`SetText`).
    pub(crate) text: Option<String>,
    /// `SetAlphaGradient(start, length)`, the quest text's write-on reveal; cleared by `SetText`.
    pub(crate) alpha_gradient: Option<(f32, f32)>,
    /// `SetBlendMode`, XML `alphaMode` (the reference's `+0xd0`), starting at [`BlendMode::Add`]
    /// for a highlight texture, as `SetHighlightTexture` makes it, else at the constructor's BLEND.
    pub(crate) blend: BlendMode,
    /// `SetWidth`/`SetHeight`, XML `<Size>`; `None` derives it. It fills only an axis the anchors
    /// leave free, so under an implicit `SetAllPoints` the stack-split plate's 256×32 is unread.
    pub(crate) size: Option<(f32, f32)>,
    /// `SetPoint`, XML `<Anchors>`, relative to the owner frame unless naming a frame or sibling
    /// region. An edge they leave unset comes from the region's own size, measured text or texel
    /// extent, never the owner's rect; one still unset leaves the region unresolved (`0x767a20`).
    /// Empty is a Lua region nobody anchored, which never draws: every other has an authored or
    /// creation-time anchor ([`super::region::implicit_creation_anchor`]).
    pub(crate) anchors: Vec<Anchor>,
    /// `SetDesaturated` (`0x79c1e0`): the renderer greys the texel, so the binding reports support.
    pub(crate) desaturated: bool,
    /// `SetNonSpaceWrap`/`CanNonSpaceWrap` (`0x79e9f0`/`0x79ead0`), `None` on. Stored, not applied:
    /// the reference's flag `0x40` drives a mid-word wrap and the ellipsis fit count (`0x771ec0`).
    pub(crate) non_space_wrap: Option<bool>,
    /// The client's dword at `CSimpleFontString+0x120`: bits 0-2 horizontal, 3-5 vertical, `0x212`
    /// from the constructor. A cleared axis answers `"UNKNOWN"` to the getter and draws centred.
    pub(crate) justify: crate::justify::Justify,
    /// The `<TexCoords>`/`SetTexCoord` mapping; `None` is the full texture.
    pub(crate) tex_coords: Option<TexCoords>,
    /// `SetTexCoordModifiesRect`, the reference's `+0x124` (written only at `0x79c113`, in
    /// `0x79c080`). Stored, not applied: there a set flag makes `SetTexCoord` re-derive the rect
    /// from the UV quad (`0x770462`); no stock or corpus code sets it.
    pub(crate) tex_coord_modifies_rect: bool,
    /// Rotation about the centre, radians counterclockwise; always 0: 1.12 has no Texture setter.
    pub(crate) rotation: f32,
    /// The font object this FontString inherits (`FONTINSTANCE+0x028`, a FontString's `+0xd0`), a
    /// live link: its paint is copied below, and
    /// [`script::font::propagate`](super::font::propagate) repaints on a change.
    pub(crate) font_object: Option<String>,
    /// What this region set for itself, which its font object no longer overwrites.
    pub(crate) font_explicit: FontExplicit,
    /// Resolved font face path (`SetFont`/`SetFontObject`); `None` is the renderer's default.
    pub(crate) font_path: Option<String>,
    /// Resolved font height in logical px; `None` is the renderer's default.
    pub(crate) font_height: Option<f32>,
    /// A `SetTextHeight` override: a FontString is one-to-one by default (bit `0x200`, capped at
    /// 32 px), and only `SetTextHeight 0x771600` clears that, drawing the size uncapped.
    pub(crate) text_height: Option<f32>,
    /// Resolved glyph outline (`SetFont` flags or the font object).
    pub(crate) outline: Outline,
    /// Resolved drop shadow (font object `<Shadow>`), drawn behind the glyphs.
    pub(crate) font_shadow: Option<FontShadow>,
    /// The host-measured size of a FontString with no explicit height.
    pub(crate) measured: Option<MeasuredText>,
}

/// The font properties a region set for itself, which changes to its font object no longer
/// reach: the reference's inherit mask (`FONTINSTANCE+0x2c`, a FontString's `+0xd4`; justify
/// per axis at `+0x124`), whose bit each local setter clears. Nothing restores one, a later
/// `SetFontObject` included, so a row that calls `SetTextColor(1, 0, 0)` stays red.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FontExplicit {
    /// `SetFont`'s path argument, or XML `font=`.
    pub(crate) face: bool,
    /// `SetFont`'s height argument, or XML `<FontHeight>`.
    pub(crate) height: bool,
    /// `SetFont`'s flags argument, or XML `outline=`.
    pub(crate) outline: bool,
    /// `SetTextColor`/`SetVertexColor`, or a `<FontString><Color>`.
    pub(crate) color: bool,
    /// `SetShadowColor`/`SetShadowOffset`, or XML `<Shadow>` on the FontString itself.
    pub(crate) shadow: bool,
    /// `SetJustifyH`, or XML `justifyH=`.
    pub(crate) justify_h: bool,
    /// `SetJustifyV`, or XML `justifyV=`.
    pub(crate) justify_v: bool,
}

/// A cached host measurement of a FontString's laid-out text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MeasuredText {
    /// The laid-out extent, wrapped in any declared width, which auto-size and `GetWidth` read.
    pub(crate) w: f32,
    pub(crate) h: f32,
    /// The unwrapped width `GetStringWidth` answers, the only one the reference keeps
    /// (`0x79e510` → `0x772890`, cached at `+0xfc`); answering `w` would feed back through
    /// `PanelTemplates_TabResize`, which sets the string's width from it.
    pub(crate) natural_w: f32,
    /// The [`RegionData::measure_key`] it was measured under; a mismatch re-measures.
    pub(crate) key: u64,
}

impl MeasuredText {
    /// Whether `w` or `h`, the layout's only inputs, moved: a fresh measure always differs in
    /// `key`, so a whole-value compare would dirty the layout on every same-size text change.
    pub(crate) fn layout_moved(before: Option<Self>, after: Self) -> bool {
        before.is_none_or(|b| {
            b.w.to_bits() != after.w.to_bits() || b.h.to_bits() != after.h.to_bits()
        })
    }
}

impl RegionData {
    /// The measure-cache key of the text, font, wrap width, outline and owner scale. A Lua metric
    /// read treats a measure under another key as absent; the layout keeps the old box until the
    /// new one lands, except for empty text, which is never measured.
    pub(crate) fn measure_key(&self, scale: f32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // By reference: `String` hashes as `str` does, and this runs for every FontString every
        // frame, so a clone would allocate for nothing.
        self.text.as_deref().unwrap_or("").hash(&mut hasher);
        self.font_path.hash(&mut hasher);
        self.font_height.map(f32::to_bits).hash(&mut hasher);
        self.text_height.map(f32::to_bits).hash(&mut hasher);
        let wrap_width = self.size.map(|s| s.0).filter(|w| *w > 0.0);
        wrap_width.map(f32::to_bits).hash(&mut hasher);
        // The outline changes the measure: a THICK one steps each glyph 1 px further
        // (`GlyphStepBase 0x5ca2b0`).
        (self.outline as u8).hash(&mut hasher);
        scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}

/// The focused EditBox's advance-table request, answered via
/// [`UiScript::set_editbox_advances`](super::UiScript::set_editbox_advances) with the width at
/// each byte of `text` (len+1 entries, `[0] = 0`, a continuation byte repeating its lead's), for
/// the engine's click-to-caret and selection (the reference's click handler is `0x77b800`).
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxAdvanceRequest {
    /// Opaque frame id; pass it back verbatim.
    pub id: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// A THICK outline adds 1 px to every glyph step (`GlyphStepBase 0x5ca2b0`).
    pub outline: Outline,
    /// The box's `effective_scale`. The host measures at `height × scale × seam` but divides by
    /// the seam alone, so advances come back in screen UI units, like the mouse feed and the
    /// box's rect; a [`MeasureRequest`] answer is frame-local instead.
    pub scale: f32,
    /// The displayed string (mask-aware) the table indexes.
    pub text: String,
    /// For a multiline box, the width the draw wraps at: the answer then also carries the row
    /// starts and pitch (`rows`, `cell_h`); `None` is single-line, answered `rows = [0]`.
    pub wrap_width: Option<f32>,
    /// Cache key; pass it back verbatim.
    pub key: u64,
}

// Re-exported from `widget`, which `script` depends on and never the reverse.
pub use crate::widget::{EditAction, EditOutcome, EditUnit};

/// The focused EditBox's per-frame text geometry: its text window, selection and caret.
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxTextUi {
    /// The text region's draw target, to find its extracted Text quad by.
    pub target: crate::order::ZTarget,
    /// The scroll window's start, a display byte offset; always 0 for a multiline box.
    pub display_from: usize,
    /// Multiline: the whole wrapped block draws, caret and selection seated by `(row, x)`.
    pub multi_line: bool,
    /// Caret x in px from the drawn window's text origin (multiline: from the row's origin).
    pub caret_x: f32,
    /// Always 0 single-line.
    pub caret_row: usize,
    /// The answered row pitch in px, 0 until the advances arrive.
    pub cell_h: f32,
    /// The blink phase: draw the caret this frame.
    pub caret_on: bool,
    /// Per-row `(row, x0, x1)` px spans from each row's origin; single-line has at most one.
    pub selection: Vec<(usize, f32, f32)>,
    /// `SetHighlightColor`; the constructor's default is opaque `0x606060` grey.
    pub highlight_color: [f32; 4],
}

/// One FontString the layout needs host-measured this frame (text, no explicit height), answered
/// via [`UiScript::set_measured_text`](super::UiScript::set_measured_text).
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureRequest {
    /// Opaque region id; pass it back verbatim.
    pub id: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// The `SetTextHeight` override, measured uncapped at the drawn size (`0x772890`).
    pub text_height: Option<f32>,
    /// Set when the region's width is pinned (explicit `<Size x>`), else single-line.
    pub wrap_width: Option<f32>,
    /// A THICK outline adds 1 px to every glyph step (`GlyphStepBase 0x5ca2b0`).
    pub outline: Outline,
    /// The owner's `effective_scale`. The host measures at `font_height × scale × seam` and
    /// divides by that whole product: advances step to whole pixels, so a measure at the
    /// unscaled size is not proportional to the drawn one.
    pub scale: f32,
    pub text: String,
    /// Cache key; pass it back verbatim.
    pub key: u64,
}

/// One ScrollingMessageFrame line whose wrapped row count needs measuring, answered via
/// [`UiScript::set_message_line_rows`](super::UiScript::set_message_line_rows) in the same frame,
/// before extract: rows move only the drawn bands, never the anchors.
#[derive(Clone, Debug, PartialEq)]
pub struct LineMeasureRequest {
    /// Opaque frame id; pass it back verbatim.
    pub frame: u32,
    /// The line's ring index at collect time; pass it back verbatim.
    pub index: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// The frame's inner width: already scale-multiplied, unlike `height`, the font's own size.
    pub wrap_width: f32,
    pub outline: Outline,
    /// As for [`MeasureRequest::scale`].
    pub scale: f32,
    pub text: String,
    /// Cache key; pass it back verbatim.
    pub key: u64,
}
