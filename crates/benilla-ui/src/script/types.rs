use mlua::{Lua, Value};

use crate::layout::{Anchors, Rect};
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
        /// The outdoor zoom cell, 0 the widest; raw, so the reader caps it as the radius read does.
        zoom: u32,
        /// The indoor zoom index (`0x86f69c`), sent too so the renderer picks by its own test.
        inside_zoom: u32,
    },
    /// A `Model` or `PlayerModel` widget's 3D pane: a `model` file draws as a tile, a character
    /// pane its own copy of its subject, an empty pane nothing. The host memoizes on this
    /// list, so it reads the moving play head from `UiScript::visible_model_panes` instead.
    ModelPane {
        /// The renderer's key for the tile it keeps per pane.
        handle: crate::widget::FrameHandle,
        /// The global frame name; `None` for an anonymous `CreateFrame("Model")`.
        name: Option<String>,
        /// A `PlayerModel`, `DressUpModel` or `TabardModel`: with no file it draws its copy of
        /// the subject `SetUnit` stored ([`crate::script::UiScript::character_panes`]).
        character: bool,
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
        /// The fog the scene draws with ([`crate::widget::ModelState::scene_fog`]); a material
        /// with the UNFOGGED bit ignores it.
        fog: Option<crate::widget::ModelFog>,
        /// A `ModelFFX` (`0x46fa00`): the glue screens' scene, which the app's glue booth draws.
        ffx: bool,
    },
    /// A `Texture` region: a BLP path, a solid or vertex colour, or both (a tinted texture).
    Texture {
        path: Option<String>,
        /// The solid fill times the one vertex colour; with `gradient`, the fill alone.
        color: Option<[f32; 4]>,
        /// A gradient's four vertex colours, `[TL, TR, BR, BL]` ([`Gradient::vertex_colors`]),
        /// each multiplying the texel and `color`; `None` for a one-colour quad.
        gradient: Option<[[f32; 4]; 4]>,
        /// The blend the record draws with: the region's `SetBlendMode` mode, except that
        /// `DISABLE` and `ALPHAKEY` draw as `BLEND` under a vertex colour (`0x76fb93`).
        blend: BlendMode,
        /// `None` is the full texture.
        tex_coords: Option<TexCoords>,
        /// The wrap flag the file loaded with (`0x770200`, bits 3 and 4 of the texture's flags):
        /// the sampler repeats on both axes, else clamps to the edge, whatever the coordinates
        /// say now.
        wrap: bool,
        /// A portrait, masked to the inscribed circle.
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
        /// The `monochrome` flag (font flag bit 1): the glyphs draw from a 1-bit raster.
        monochrome: bool,
        /// The gap between lines in logical px, never negative: the line step is the font height
        /// plus this, rounded up to a whole device pixel (`LayoutLines` `0x5cdc20`).
        spacing: f32,
        /// `SetAlphaGradient`: before `start` opaque, the next `length` characters fading from 1
        /// to 0, the rest invisible.
        alpha_gradient: Option<(f32, f32)>,
        /// Seat the text at `rect` without the UI grid's block-top snap: true on a nameplate
        /// (`super::nameplate::is_world_seated`), which slides on a device-pixel snap, so a second
        /// snap would jitter its text. Not a reference field.
        world_seat: bool,
        /// A single-line EditBox's text, which holds the box's window (`0x77d858`): one line from
        /// the region's left, never wrapped or ellipsized, cut at the region's rect.
        edit_line: bool,
        /// `nonspacewrap` ([`RegionData::non_space_wrap`]); a multi-line EditBox's text is always
        /// on (`0x77a58f`).
        non_space_wrap: bool,
        /// `maxLines` ([`RegionData::max_lines`]).
        max_lines: u32,
        /// The text frame whose link buttons this text's `|H` spans take: a ScrollingMessageFrame
        /// line's (`0x788ce7`) or a SimpleHTML block's (`0x78b052`); `None` for other text, whose
        /// links are only text.
        link_owner: Option<crate::widget::FrameHandle>,
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
/// or `SetBlendMode`; BLEND is the constructor's (`0x76fc64 mov [esi+0xd0],2`). The discriminant
/// is the EGxBlend the submit sets (`0x76fba2`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    /// `DISABLE` (0): blending off, alpha reference 0 (no test).
    Disable = 0,
    /// `ALPHAKEY` (1): blending off, alpha reference 224/255.
    AlphaKey = 1,
    /// `BLEND` (2): `GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA`, alpha reference 1/255.
    #[default]
    Blend = 2,
    /// `ADD` (3): `GL_SRC_ALPHA, GL_ONE`, alpha reference 1/255.
    Add = 3,
    /// `MOD` (4): `GL_DST_COLOR, GL_ZERO`, alpha reference 1/255.
    Mod = 4,
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

    /// Parse a Lua `SetFont` flags string, shared by every `SetFont` (`0x79f210`): `0x6f1a90` ORs in
    /// each name of `{0x1 OUTLINE, 0x4 THICKOUTLINE, 0x2 MONOCHROME}` (`0x811b10`) found as a
    /// case-insensitive substring (`0x64b540`), so `THICKOUTLINE` sets 0x1 and 0x4 and `THICK`
    /// alone sets nothing. MONOCHROME is [`monochrome_flag`]'s.
    pub fn flags(s: &str) -> Outline {
        let s = s.to_ascii_uppercase();
        if s.contains("THICKOUTLINE") {
            Outline::Thick
        } else if s.contains("OUTLINE") {
            Outline::Normal
        } else {
            Outline::None
        }
    }

    /// The flags text `GetFont` answers (`0x6f1ad0`): the name of every set bit in `0x811b10`'s
    /// order, joined by `", "` (`0x84480c`). A thick outline holds both outline bits, as both
    /// readers set it (`0x6f1a90`, the XML `THICK`'s 5 at `0x7711e3`).
    pub fn flags_text(self, monochrome: bool) -> &'static str {
        match (self, monochrome) {
            (Outline::None, false) => "",
            (Outline::Normal, false) => "OUTLINE",
            (Outline::Thick, false) => "OUTLINE, THICKOUTLINE",
            (Outline::None, true) => "MONOCHROME",
            (Outline::Normal, true) => "OUTLINE, MONOCHROME",
            (Outline::Thick, true) => "OUTLINE, THICKOUTLINE, MONOCHROME",
        }
    }
}

/// Whether a Lua `SetFont` flags string holds `MONOCHROME` (`0x2` in `0x811b10`), matched as
/// [`Outline::flags`] matches its names.
pub(crate) fn monochrome_flag(s: &str) -> bool {
    s.to_ascii_uppercase().contains("MONOCHROME")
}

/// A font object's `<Shadow>`, inherited down `<Font>` chains; 1.12's `MasterFont` sets `(1,-1)`
/// black (`Fonts.xml:55`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontShadow {
    /// Pixels, y up: `y="-1"` is 1 px down.
    pub offset: [f32; 2],
    /// The packed colour; a `<Shadow>` with no `<Color>` is opaque black (`0x78405f`).
    pub color: crate::color::Rgba8,
}

/// A named `<Font>` object's paint, its own over what its `inherits=` parent had; a field the chain
/// never sets is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontObject {
    /// The TTF path (`font=`).
    pub font: Option<String>,
    /// Logical px (`<FontHeight>`).
    pub height: Option<f32>,
    pub shadow: Option<FontShadow>,
    /// The packed text colour (`+0x58`), alpha in its top byte; unset reads white (`0x783a8e`).
    pub color: Option<crate::color::Rgba8>,
    pub outline: Outline,
    /// `monochrome=` (font flag bit 1).
    pub monochrome: bool,
    /// The justify word (`+0x54`), both axes one group: a font that sets either axis holds the
    /// whole word, the other axis at the constructor's (`0x212`, `0x783a87`).
    pub justify: Option<crate::justify::Justify>,
    /// The gap between lines (`spacing=`, `SetSpacing`), stored as given (`+0x50`).
    pub spacing: Option<f32>,
}

/// One entry of [`UiScript::extract`](super::UiScript::extract)'s render list, in the client's
/// painter order ([`crate::order::traversal`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedQuad {
    /// A frame's own slot, or one of its regions.
    pub target: ZTarget,
    /// The packed draw-order key (`ZKey::raw`); entries are sorted ascending by it.
    pub z: u128,
    /// `None` when the owning frame's anchors do not resolve; a renderer skips it.
    pub rect: Option<Rect>,
    /// A frame's alpha, or a region's alpha times its owner's, one hop only.
    pub alpha: f32,
    pub content: QuadContent,
    /// The ScrollFrame clip of a scroll child and its descendants, nested ones intersecting.
    pub clip: Option<Rect>,
    /// The owner's `effective_scale` (`0x76ac90`), already in `rect`; a `Text` quad's font and
    /// shadow offset must be scaled by it too.
    pub scale: f32,
}

/// A two-stop gradient (`SetGradientAlpha`/`SetGradient`, XML `<Gradient>`): the four-colour form
/// of the vertex-colour slot (`+0xb0`, count 4), which `0x77f910` writes and `SetVertexColor`
/// collapses back to one colour (`0x77f87f`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    /// `VERTICAL` (1) rather than `HORIZONTAL` (0), the `0x811b00` enum.
    pub vertical: bool,
    /// `MinColor`, packed; `SetGradient`, which takes no alpha, sets both alphas to 255.
    pub start: crate::color::Rgba8,
    /// `MaxColor`.
    pub end: crate::color::Rgba8,
}

impl Gradient {
    /// The quad's vertex colours in screen order `[TL, TR, BR, BL]`. `0x77f910` writes the
    /// vertices in `0x7705b0`'s order `TL BL TR BR`: HORIZONTAL the min on the left pair and the
    /// max on the right (`0x77f9c3`), VERTICAL the min on the bottom pair and the max on the top
    /// (`0x77fa2c`).
    pub fn vertex_colors(&self) -> [[f32; 4]; 4] {
        let (min, max) = (self.start.to_f32(), self.end.to_f32());
        if self.vertical {
            [max, max, min, min]
        } else {
            [min, max, max, min]
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RegionData {
    /// `Show`/`Hide` and XML `hidden=`: a hidden region emits no quad, whatever its owner's state.
    pub(crate) hidden: bool,
    /// `SetAlpha`'s byte, XML `alpha=`; `None` is 255. Drawn times the owner frame's alpha, one
    /// hop only: frame `SetAlpha` (`0x76a690`) cascades to child frames and only invalidates
    /// regions.
    ///
    /// Deviation: kept apart from [`Self::vertex_color`], which the reference's `Texture:SetAlpha`
    /// (`0x79b580`) rewrites, because the drawn product differs only on a region that sets both,
    /// or a gradient and an alpha, which no stock code does, and a FontString's `SetAlpha` then
    /// keeps its font colour.
    pub(crate) alpha: Option<u8>,
    /// The texture slot (`+0xcc`): a path (`SetTexture("Interface\\...")`) or a solid fill
    /// (`SetTexture(r, g, b, a)`, an XML `<Color>` with no `file=`), each clearing the other. The
    /// reference makes an 8×8 texture of a fill (`0x770360` → `0x44a9c0` → `0x5c5350`), so its alpha
    /// multiplies with the vertex colour's. Written only through [`super::Model::set_texel`].
    pub(crate) tex: super::texel::TexSlot,
    /// `SetGradientAlpha`/`SetGradient` or `<Gradient>`: four vertex colours, drawn as
    /// `texel × vertexColour` like [`Self::vertex_color`], whose slot it shares, so setting either
    /// clears the other. It is no texture: a gradient alone draws nothing (`0x7706e0`).
    pub(crate) gradient: Option<Gradient>,
    /// `SetVertexColor` (`0x79abd0` → `0x77f750`, `+0xb8`), `SetStatusBarColor` or a text colour,
    /// drawn as `texel × colour` per channel, alpha included: a `<Color 1,1,1,0.2>` tinted
    /// `(0, 0, 0.75, 0.5)` draws at alpha 0.1, as `SkillFrame`'s row trough does.
    pub(crate) vertex_color: Option<crate::color::Rgba8>,
    /// A portrait (`SetPortraitToTexture`, `SetPortraitTexture`), masked to the inscribed circle.
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
    pub(crate) anchors: Anchors,
    /// `SetDesaturated` (`0x79c1e0`): the renderer greys the texel, so the binding reports support.
    pub(crate) desaturated: bool,
    /// `nonspacewrap` / `SetNonSpaceWrap` / `CanNonSpaceWrap` (`0x77103f`, `0x79e9f0`,
    /// `0x79ead0`): bit `0x1000` of the justify word, off from the constructor (`0x770dd3`). The
    /// text layer takes it as flag `0x40` (`0x44d4c7`, `0x44d9c5`): it decides what a force-broken
    /// line leaves to the next and where the ellipsis fit stops.
    pub(crate) non_space_wrap: bool,
    /// XML `maxLines` (`0x7710b2`-`0x7710cd`, `+0x128`), 0 for none: the ellipsis truncate fits the
    /// text into that many lines, with or without a height (`0x771ec0`).
    pub(crate) max_lines: u32,
    /// The client's dword at `CSimpleFontString+0x120`: bits 0-2 horizontal, 3-5 vertical, `0x212`
    /// from the constructor. A cleared axis answers `"UNKNOWN"` to the getter and draws centred.
    pub(crate) justify: crate::justify::Justify,
    /// The `<TexCoords>`/`SetTexCoord` mapping; `None` is the full texture.
    pub(crate) tex_coords: Option<TexCoords>,
    /// The reference's `+0x124`: `SetTexCoordModifiesRect` writes it (`0x79c113`, in `0x79c080`)
    /// and installing the texture as a StatusBar's bar sets it (`0x783220`, in `0x7831f0`). Stored,
    /// not applied: there a set flag makes `SetTexCoord` re-derive the rect from the UV quad
    /// (`0x770462`); no stock or corpus code sets it.
    pub(crate) tex_coord_modifies_rect: bool,
    /// Rotation about the centre, radians counterclockwise; always 0: 1.12 has no Texture setter.
    pub(crate) rotation: f32,
    /// The font object this FontString follows (a FontString's `+0xd0`), a live link written only
    /// by [`script::font::link_region`](super::font::link_region): the font's groups land below
    /// and [`script::font::push`](super::font::push) brings each change.
    pub(crate) font_object: Option<super::font::FontId>,
    /// The groups this region still takes from its font object, `+0xd4`; each local setter clears
    /// its own.
    pub(crate) font_mask: FontMask,
    /// The justify bits the justify group writes, `+0x124` ([`JustifyMask`]).
    pub(crate) justify_mask: JustifyMask,
    /// Resolved font face path (`SetFont`/`SetFontObject`); `None` is no font, the reference's
    /// null `+0xe0` (`font_block::has_font`).
    pub(crate) font_path: Option<String>,
    /// The text height in logical px (`+0xe4`), which a font load (`0x7714f0`, `0x771589`) and
    /// `SetTextHeight` (`0x77163b`) both write and `GetFont` reads (`0x7727bc`); `None` is the
    /// renderer's default.
    pub(crate) font_height: Option<f32>,
    /// `SetTextHeight` cleared the one-to-one bit (`0x200` of `+0x120`, `0x771627`): the text then
    /// draws at [`Self::font_height`] uncapped, magnified from the raster. Nothing sets it back,
    /// so a later font load keeps the string scaled.
    pub(crate) text_scaled: bool,
    /// Resolved glyph outline (`SetFont` flags or the font object).
    pub(crate) outline: Outline,
    /// Resolved monochrome flag, from the same flags word as [`Self::outline`].
    pub(crate) monochrome: bool,
    /// Resolved drop shadow (font object `<Shadow>`), drawn behind the glyphs.
    pub(crate) font_shadow: Option<FontShadow>,
    /// The gap between lines, as set (`SetSpacing`, XML `spacing=` or the font object): a
    /// FontString's own setter clamps it (`0x772240`), the message frames' and the EditBox's font
    /// stores it raw (`0x79fb40`). A line string reads it through [`Self::line_gap`]; the plain
    /// message frame's pitch reads it raw (`0x785f06`).
    pub(crate) spacing: f32,
    /// The host-measured size of a FontString with no explicit height.
    pub(crate) measured: Option<MeasuredText>,
}

/// The font groups a follower still takes from the font it follows: the inherit mask (a Font's
/// `+0x2c`, a FontString's `+0xd4`), `0x1f` from each constructor (`0x783a98`, `0x770d49`). A push
/// or a link writes a group only through its bit (`0x770800`, `0x770910`); each local setter
/// clears its own bit and nothing sets one back, so a row that calls `SetTextColor(1, 0, 0)` stays
/// red through any later `SetFontObject`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FontMask(u8);

impl FontMask {
    /// The face, height and flags, one group (`0x770848` loads all three).
    pub(crate) const FONT: FontMask = FontMask(0x1);
    /// The justify word, both axes together (`0x770859`).
    pub(crate) const JUSTIFY: FontMask = FontMask(0x2);
    /// The text colour, alpha included (`0x7708a2`).
    pub(crate) const COLOR: FontMask = FontMask(0x4);
    /// The shadow colour and offset together (`0x7708a9`).
    pub(crate) const SHADOW: FontMask = FontMask(0x8);
    /// The line spacing (`0x7708fd`).
    pub(crate) const SPACING: FontMask = FontMask(0x10);
    pub(crate) const ALL: FontMask = FontMask(0x1f);
    pub(crate) const NONE: FontMask = FontMask(0);

    /// Whether every bit of `g` is set.
    pub(crate) fn has(self, g: FontMask) -> bool {
        self.0 & g.0 == g.0
    }

    pub(crate) fn clear(&mut self, g: FontMask) {
        self.0 &= !g.0;
    }

    pub(crate) fn set(&mut self, g: FontMask) {
        self.0 |= g.0;
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Default for FontMask {
    fn default() -> Self {
        FontMask::ALL
    }
}

impl std::ops::BitAnd for FontMask {
    type Output = FontMask;
    fn bitand(self, o: FontMask) -> FontMask {
        FontMask(self.0 & o.0)
    }
}

impl std::ops::BitOr for FontMask {
    type Output = FontMask;
    fn bitor(self, o: FontMask) -> FontMask {
        FontMask(self.0 | o.0)
    }
}

/// A FontString's per-bit justify mask (`+0x124`, `0x3f` from the constructor, `0x770ddd`): the
/// justify group writes `(word & !mask) | (font & mask)` (`0x770859`-`0x770876`). `SetJustifyH`
/// clears its three bits (`0x79e779`) and `SetJustifyV` its three (`0x79e979`), each with the
/// group bit; the koKR chat bubble clears the horizontal three alone (`0x4b1707`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JustifyMask(pub(crate) u32);

impl Default for JustifyMask {
    fn default() -> Self {
        JustifyMask(0x3f)
    }
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
    /// Write the one vertex colour (`0x77f750`), which collapses a gradient's four (`0x77f87f`).
    /// With no colour yet, white at alpha 254 or more stores nothing (`0x77f766`-`0x77f77c`), so
    /// the slot keeps reading opaque white.
    pub(crate) fn set_vertex_color(&mut self, color: Option<crate::color::Rgba8>) {
        if let Some(c) = color {
            let unset = self.vertex_color.is_none() && self.gradient.is_none();
            if unset && c.a >= 0xfe && (c.r, c.g, c.b) == (255, 255, 255) {
                return;
            }
        }
        self.vertex_color = color;
        self.gradient = None;
    }

    /// The one vertex colour as `0x77f8c0` answers it: white unless the slot holds exactly one.
    pub(crate) fn vertex_color_or_white(&self) -> crate::color::Rgba8 {
        self.vertex_color.unwrap_or(crate::color::Rgba8::WHITE)
    }

    /// The gap a line string lays its lines out by: [`Self::spacing`] clamped at 0, as a line
    /// string stores it through `0x772240`.
    pub(crate) fn line_gap(&self) -> f32 {
        self.spacing.max(0.0)
    }

    /// The size a `SetTextHeight` string draws at, uncapped ([`Self::text_scaled`]); `None` for a
    /// one-to-one string.
    pub(crate) fn text_height(&self) -> Option<f32> {
        self.text_scaled.then(|| self.font_height.unwrap_or(0.0))
    }

    /// The shadow's x offset in UI units, 0 for none ([`MeasureRequest::shadow_x`]).
    pub(crate) fn shadow_x(&self) -> f32 {
        self.font_shadow.map_or(0.0, |s| s.offset[0])
    }

    /// The measure-cache key of the text, font, wrap width, outline, line gap, shadow and owner
    /// scale. A Lua metric read treats a measure under another key as absent; the layout keeps the
    /// old box until the new one lands, except for empty text, which is never measured.
    pub(crate) fn measure_key(&self, scale: f32) -> u64 {
        // By reference: `String` hashes as `str` does, and this runs for every FontString every
        // frame, so a clone would allocate for nothing.
        self.measure_key_of(self.text.as_deref().unwrap_or(""), scale)
    }

    /// [`Self::measure_key`] with `text` in place of the region's own.
    pub(crate) fn measure_key_of(&self, text: &str, scale: f32) -> u64 {
        self.measure_key_at(text, self.wrap_width(), scale)
    }

    /// The width the text wraps in: the authored width when positive, else none.
    pub(crate) fn wrap_width(&self) -> Option<f32> {
        self.size.map(|s| s.0).filter(|w| *w > 0.0)
    }

    /// [`Self::measure_key_of`] with `wrap_width` in place of the region's own.
    pub(crate) fn measure_key_at(&self, text: &str, wrap_width: Option<f32>, scale: f32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        self.font_path.hash(&mut hasher);
        self.font_height.map(f32::to_bits).hash(&mut hasher);
        self.text_scaled.hash(&mut hasher);
        wrap_width.map(f32::to_bits).hash(&mut hasher);
        // The outline changes the measure: a THICK one steps each glyph 1 px further
        // (`GlyphStepBase 0x5ca2b0`); so does the shadow's x offset, which seeds the line.
        (self.outline as u8).hash(&mut hasher);
        self.line_gap().to_bits().hash(&mut hasher);
        self.shadow_x().to_bits().hash(&mut hasher);
        // The flag moves the wrap and `maxLines` the cut text the height is measured from.
        self.non_space_wrap.hash(&mut hasher);
        self.max_lines.hash(&mut hasher);
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
    /// The gap between a multiline box's rows, in frame-local units: the row pitch is the height
    /// plus it (`0x77d133`-`0x77d161`).
    pub spacing: f32,
    /// The shadow's x offset in UI units, 0 for none: it seeds each drawn row's pen.
    pub shadow_x: f32,
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
    /// Multiline: the whole wrapped block draws, caret and selection seated by `(row, x)`.
    pub multi_line: bool,
    /// Caret x in px from the text region's origin, the window's start (multiline: from the
    /// row's origin).
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
    /// The gap between wrapped lines in frame-local units ([`RegionData::line_gap`]), scaled like
    /// the height: the measure is `N·S + (N−1)·gap` (`0x5c2070`).
    pub spacing: f32,
    /// The shadow's x offset in UI units, 0 for none: `GetStringWidth` seeds its line with it,
    /// unscaled by the frame (`0x772890` hands `[fs+0x108]` to the kernel as it is).
    pub shadow_x: f32,
    /// `nonspacewrap`: what a force-broken line leaves to the next one ([`RegionData::non_space_wrap`]).
    pub non_space_wrap: bool,
    /// `maxLines`: the text is cut to that many lines before it is measured, 0 for no cut.
    pub max_lines: u32,
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
    /// The font's shadow x offset in UI units, 0 for none: it seeds each drawn row's pen.
    pub shadow_x: f32,
    /// As for [`MeasureRequest::scale`].
    pub scale: f32,
    pub text: String,
    /// Cache key; pass it back verbatim.
    pub key: u64,
}
