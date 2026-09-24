use mlua::{Lua, Value};

use crate::layout::{Anchor, Rect};
use crate::order::ZTarget;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Public value + extract types (engine-free seams)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A simple, engine-free value the host can inject into Lua — the seam the app's net/state→event
/// bridge uses to hand `fire_event` its `arg1..argN`. Deliberately not
/// `mlua::Value` so callers never touch mlua handles (the MAXCSTACK discipline reaches the API too).
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptValue {
    Nil,
    Bool(bool),
    /// An integer argument (GUIDs-as-strings aside, most event args are ints/strings).
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

/// A texture region's UV mapping — the two live `SetTexCoord` forms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TexCoords {
    /// The 4-edge crop `[left, right, top, bottom]` (`SetTexCoord(l,r,t,b)` / XML `<TexCoords>`):
    /// an axis-aligned UV sub-rect in 0..1 texture space, top-left origin. A mirrored
    /// (`left > right`) or flipped tuple keeps its orientation.
    Rect([f32; 4]),
    /// The 8-arg affine form (`SetTexCoord(ULx,ULy, LLx,LLy, URx,URy, LRx,LRy)`) — an arbitrary
    /// UV quad (rotation/shear; the reference's `DrawRouteLine` taxi route lines are the first
    /// consumer). Stored per corner in **screen order `[TL, TR, BR, BL]`** (the renderer's
    /// `push_quad` winding — the Lua arg order is converted at the binding).
    Corners([[f32; 2]; 4]),
}

impl TexCoords {
    /// The tightest axis-aligned `[left, right, top, bottom]` containing the mapping (exact for
    /// `Rect`, the bounding box for `Corners`).
    ///
    /// **Not what `GetTexCoord()` reports** — that answers eight per-corner values, and the 4-value
    /// rect it used to return is a shape the reference has nowhere. This is the
    /// renderer's and the app's convenience view, and it keeps its callers.
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
    /// The frame's own draw slot (backdrop/scissor seat) — no region visual of its own in v1.
    Frame,
    /// A `Minimap` widget's own draw slot: the circular HUD map's content hole. The engine core
    /// carries the resolved rect + the zoom index; the app renderer draws the world into it —
    /// the streamed tile window, the mask, and the player arrow. Emitted *at the
    /// frame's own z*, so the widget's children (border art, buttons) paint above the map exactly
    /// as the reference layers them.
    Minimap {
        /// The **outdoor** zoom index, `0..MINIMAP_ZOOM_LEVELS` (0 = widest) — see
        /// [`crate::widget::MinimapState`].
        zoom: u8,
        /// The **indoor** zoom index (persisted separately; the client's `0x86f69c`). Both travel
        /// so the app renderer picks by its own WMO-containment test rather than racing the
        /// `inside` flag it pushed down.
        inside_zoom: u8,
    },
    /// A `Model` / `PlayerModel` widget's own draw slot: the 3D pane's content hole. The engine
    /// core carries the resolved rect and the pane's identity; the app renderer puts pixels in it
    /// — the same division of labour [`QuadContent::Minimap`] uses, and for the same reason (the
    /// scene state is [`crate::widget::ModelState`]; the render is not this crate's).
    ///
    /// **The clock is deliberately NOT here.** A pane's play head moves every tick, and the
    /// host memoizes its whole quad conversion on the extracted list being unchanged — a
    /// cursor in this variant would re-convert the entire interface every frame a cooldown or a
    /// ping is showing. The host reads the play heads straight off the engine instead
    /// (`UiScript::visible_model_panes`); this variant carries only what the
    /// layout and the Lua setters decide.
    ///
    /// **Why the NAME travels rather than the scene.** benilla draws a body pane by sampling an
    /// off-screen bake the app already keeps per *window* (the paper doll's, the inspect window's,
    /// the pet page's), and which bake a pane samples is a fact about that window, not about the
    /// widget. So the seam carries what the app needs to make the join — the pane's global frame
    /// name, which since decision 1751 is the reference's own — and nothing it would have to
    /// invent a meaning for. A pane with no name, or one no window has claimed, draws nothing;
    /// that is also what a `SetModel` pane does today, and it is honest rather than a white slab.
    ModelPane {
        /// The pane's frame handle — the renderer's key for the tile it keeps per pane (a name
        /// is optional and shared by nothing; the handle is neither).
        handle: crate::widget::FrameHandle,
        /// The pane's global frame name (`$parent`-expanded), or `None` for an anonymous
        /// `CreateFrame("Model")` — pfUI's autocast shine is the corpus example of the latter.
        name: Option<String>,
        /// The `SetModel` path the pane holds (`None` for a unit pane or an empty one) — what the
        /// host's tile renderer draws, the map arrow's `crate::script::ARROW_MODEL`
        /// among them since 2015.
        model: Option<String>,
        /// `SetFacing`'s radians (0 default).
        facing: f32,
        /// `SetModelScale`'s factor (1 default).
        model_scale: f32,
        /// `SetPosition`'s offset, in the ortho leg's layout units (`0x76d1a0`: the root is
        /// `T(pos · layoutScale) · R(facing) · S(…)`).
        position: (f32, f32, f32),
        /// The frame's **own** alpha — what the model instance draws at. The reference re-pushes
        /// `[widget+0xc8]/255` into the instance on every `Frame:SetAlpha` (`0x76d120`), and
        /// nothing folds the parent chain in: a pane under a faded parent draws at its own
        /// alpha. [`ExtractedQuad::alpha`] carries the effective one.
        own_alpha: f32,
        /// `ReplaceIconTexture`'s path — the type-14 texture override, or `None` for the file's
        /// own textures.
        icon: Option<String>,
        /// The installed camera as a RAW index into the file's camera table, or `None` for the
        /// NULL camera. **This is the render leg**: `None` is the orthographic
        /// one — the model laid flat over the pane's rect — and `Some(n)` the perspective one,
        /// framed by the file's own record `n`.
        camera: Option<u32>,
        /// The pane's embedded `CGLight`. Disabled on every shipped pane, which is why an
        /// unlit-on-every-material UI M2 never notices; a LIT batch under a disabled light draws
        /// black, and that is the reference's answer, not a gap.
        light: crate::widget::ModelLight,
        /// The pane's fog, only when armed — colour, near and far as the fill callback stages
        /// them. Per-batch from there: a material with the UNFOGGED bit ignores it.
        fog: Option<crate::widget::ModelFog>,
    },
    /// A `Texture` region: a BLP path *or* a solid/vertex color (or both — a tinted texture).
    Texture {
        path: Option<String>,
        color: Option<[f32; 4]>,
        /// WoW `ADD` blend (highlights/glows) instead of straight alpha.
        additive: bool,
        /// The `<TexCoords>`/`SetTexCoord` UV mapping the region samples — the 4-edge crop or the
        /// 8-arg affine quad ([`TexCoords`]). `None` = the full texture (0,1,0,1). The renderer
        /// crops (or reprojects) the quad's UVs to this.
        tex_coords: Option<TexCoords>,
        /// Draw masked to the inscribed circle — a **portrait** (`SetPortraitToTexture`, and
        /// `SetPortraitTexture`'s round unit-portrait binding). The renderer bakes the circular
        /// alpha so the square icon/model doesn't poke past the frame ring. A `portrait_unit`
        /// binding with this **false** is the square booth pane (`BenillaSetBoothTexture` —
        /// the paper doll's model view).
        circular: bool,
        /// A **live unit portrait** (`SetPortraitTexture(region, unit)` round;
        /// `BenillaSetBoothTexture(region, token)` square — `circular` above distinguishes): the
        /// unit token whose model this region renders. When `Some`, the renderer ignores
        /// `path`/`color` and samples the app-side portrait render-target for this token (the
        /// off-screen model bake). `None` = an ordinary texture/color region. The engine-free core
        /// only carries the token; resolving it to a rendered image is the app's job (the modern
        /// 2D portrait is a high-res model bake).
        portrait_unit: Option<String>,
        /// On-screen rotation about the quad center, radians, **counterclockwise-positive**.
        ///
        /// **Always `0.0` now — nothing writes it any more.** It carried a later-era
        /// `Texture:SetRotation` benilla shipped early so the world-map player arrow could spin,
        /// standing in for the reference's engine arrow model (0203 flags the stand-in). That arrow
        /// became a real Model frame driven by `ModelState::facing` (`script::worldmap_arrow`), and
        /// the verb has since come off the Texture map — 1.12 registers `SetRotation` on PlayerModel
        /// alone (`0x84f1fc`). The plumbing is left standing rather than pruned in the same change,
        /// because it runs into the app's quad emit; see [`RegionData::rotation`].
        rotation: f32,
        /// `Texture:SetDesaturated(1)` — draw the sampled texel as its **luminance** instead of
        /// its colour. The renderer greys the texel and *then* modulates by
        /// `color`, because the reference's own consumers pass a dim tint alongside the flag
        /// (`SetItemButtonDesaturated(button, 1, 0.65, 0.65, 0.65)`) and expect both to land.
        desaturated: bool,
    },
    /// A `ColorSelect`'s **hue disc** — its `<ColorWheelTexture>` region. That element carries no
    /// `file=` in the reference either: the wheel is generated by the client, not loaded, and there
    /// is no BLP anywhere in the MPQ chain that is a colour wheel. So the engine core hands the
    /// renderer the rect and the widget's HSV and the app makes the pixels — the same division the
    /// [`QuadContent::Minimap`] slot uses, for the same reason.
    ///
    /// The disc it must draw is fixed by the *pick* law it has to invert (`0x78bd80`): at
    /// normalised offset `(nx, ny)` from the rect's centre the
    /// pixel is `HSV(atan2(ny, nx)·180/π + 180, min(|n|, 1), …)`, so clicking a pixel selects the
    /// colour that pixel shows.
    ///
    /// **It carries no colour**, and that is the byte law rather than an omission: the fill loop
    /// draws every texel at a literal `V = 1.0` (`0x78b68b`) and never reads the widget's own
    /// value, so the disc is the same image at every colour. Handing the renderer an HSV it must
    /// ignore would also churn this quad on every step of a drag, for no pixel.
    ColorWheel,
    /// A `ColorSelect`'s **brightness strip** — its `<ColorValueTexture>` region, the wheel's
    /// file-less twin. A vertical ramp of the currently selected hue and saturation, black at the
    /// bottom, which is the inverse of the strip's own pick law
    /// (`V = clamp((y − bottom)/(top − bottom), 0, 1)`).
    ColorValue {
        /// The widget's hue, degrees.
        hue: f32,
        /// The widget's saturation, `0..=1`. Together these are the whole strip: its top is
        /// `HSV(hue, sat, 1)` and its bottom is black, so the widget's **value** moves no pixel of
        /// it (only its marker).
        sat: f32,
    },
    /// One frame `Backdrop` piece (the tiled bg, or one of the 8 border pieces) — emitted at the
    /// owning frame's own draw slot, behind its regions ([`UiScript::extract`](super::UiScript::extract)). A textured quad
    /// whose UVs are four explicit per-corner pairs (`[TL, TR, BR, BL]`, screen order) because the
    /// TOP/BOTTOM edges are rotated 90°, and whose texture needs REPEAT addressing (edges tile).
    /// The [`ExtractedQuad::rect`] is the piece's screen bounding box.
    Backdrop {
        /// The bg or edge texture path.
        path: String,
        /// The bg (`SetBackdropColor`) or border (`SetBackdropBorderColor`) vertex tint, pre-alpha.
        color: [f32; 4],
        /// Four per-corner UVs, `[TL, TR, BR, BL]` (screen order).
        uvs: [[f32; 2]; 4],
        /// Sample the texture with REPEAT (wrap) addressing.
        tile: bool,
    },
    /// A `FontString` region: its text, color, horizontal justification, and the resolved font
    /// (path + height + shadow) from its font object / `SetFont` — rendering/metrics are the Bevy
    /// side's job (it honors [`QuadContent::Text::justify_h`], picks the face by `font`, bakes at
    /// `height`, and draws [`QuadContent::Text::shadow`] as an offset dark pass behind the glyphs).
    Text {
        text: Option<String>,
        color: Option<[f32; 4]>,
        justify_h: JustifyH,
        justify_v: JustifyV,
        /// The resolved font face path (`SetFontObject`/`SetFont`/XML `inherits=`), e.g.
        /// `"Fonts\\FRIZQT__.TTF"`. `None` ⇒ the renderer's default face (Friz Quadrata).
        font: Option<String>,
        /// The resolved font height in logical px. `None` ⇒ the renderer's default size.
        font_height: Option<f32>,
        /// The `SetTextHeight` override (see [`RegionData::text_height`]): `Some` = the
        /// scaled-string regime — draw at THIS height, uncapped (the one-to-one 32-px raster cap
        /// applies only when this is `None`).
        text_height: Option<f32>,
        /// The resolved drop shadow (font object `<Shadow>` — offset + color). The real 1.12
        /// `MasterFont` root carries `(1,-1)` black, so nearly every stock font is shadowed; the
        /// crispness of client text against parchment/world IS this shadow.
        shadow: Option<FontShadow>,
        /// The resolved glyph outline (`outline="NORMAL"/"THICK"`). The Number* fonts (action-bar
        /// hotkeys/counts) carry it; the renderer draws it as a black offset halo under the fill —
        /// `cosmic-text` has no native outline. `None` for the common un-outlined case.
        outline: Outline,
        /// The write-on reveal (`SetAlphaGradient` — see [`RegionData::alpha_gradient`]): the
        /// renderer multiplies each glyph's alpha by the ramp at its character position (before
        /// `start` opaque, the next `length` chars 1→0, beyond invisible). `None` = draw whole.
        alpha_gradient: Option<(f32, f32)>,
        /// **Seat this block exactly where `rect` puts it**, skipping the UI grid's vertical
        /// block-top snap — true for a FontString owned by a V-plate, false for every other one
        /// (`super::nameplate::is_world_seated`).
        ///
        /// Not a client concept: in the reference the whole interface *is* the pixel grid, so the
        /// snap and the rect agree by construction. benilla has one family of frames that lives
        /// outside it — the WorldFrame overlays that slide continuously over the 3-D scene, whose
        /// seat law is a DEVICE-pixel snap (`vplates::device_snap`, 0188/1398) rather than the
        /// UI's logical-pixel one. Snapping their text on the coarser UI grid makes it *beat*
        /// against the art it is supposed to be rigid to: two quantizers on one sliding object.
        world_seat: bool,
    },
}

/// A `FontString`'s horizontal text justification within its resolved rect — the XML `justifyH`
/// attr (`SetJustifyH`). The FrameXML default is **CENTER** (the client's `JustifyH` field default);
/// the app's text renderer positions each line's run against the rect per this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyH {
    Left,
    #[default]
    Center,
    Right,
}

/// A `FontString`'s vertical text justification within its resolved rect — the XML `justifyV`
/// attr (`SetJustifyV`). The FrameXML default is **MIDDLE** (the client's `JustifyV` field
/// default) — a sized FontString centers its line block vertically, which is why the real
/// client's 13px money numbers sit on their coin icons' centerline. The app's text renderer
/// offsets the line block against the rect per this (no-op for a rect sized to its text, i.e.
/// every host-measured height-less FontString).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyV {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// A Texture's blend mode — the client's shared `alphaMode` enum, the `{value, name}` table at
/// `0x811aa8`: `DISABLE=0 · ALPHAKEY=1 · BLEND=2 · ADD=3 · MOD=4`. It is one enum reached two
/// ways — the XML `alphaMode=` attribute and `Texture:SetBlendMode` — and read back by
/// `Texture:GetBlendMode`.
///
/// **`Blend` is the default because the CSimpleTexture ctor writes it**: `0x76fc64 mov
/// [esi+0xd0],2`, which `ActionButtonTemplate.xml`'s `<NormalTexture>` keeps by declaring no
/// `alphaMode` (`0x76fb82` gates on the value for a different question, and it is therefore
/// evidence rather than assertion — mode 1
/// would alpha-test a half-alpha vertex colour away entirely, mode 2 ghosts it). It is the answer
/// every ordinary texture gives `ShaguTweaks/mods/dark-ui-elements.lua:169`'s
/// `region:GetBlendMode() == "ADD"` guard, so getting it wrong is the same recolouring bug one
/// branch over.
///
/// **Only ADD changes what this engine draws.** The renderer takes the ADD/not-ADD bit
/// ([`QuadContent::Texture::additive`]); `DISABLE`, `ALPHAKEY` and `MOD` are carried faithfully
/// through the setter and the getter and drawn as straight alpha — the stated v1 gap, unchanged by
/// storing the whole mode instead of a bool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlendMode {
    /// `DISABLE` = 0 — `GL_ONE, GL_ZERO`, alpha reference 0. Drawn as straight alpha here.
    Disable,
    /// `ALPHAKEY` = 1 — `GL_ONE, GL_ZERO` with alpha reference 224/255. Drawn as straight alpha here.
    AlphaKey,
    /// `BLEND` = 2 — `GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA`. The ctor's value, hence the default.
    #[default]
    Blend,
    /// `ADD` = 3 — `GL_SRC_ALPHA, GL_ONE`. The one mode the renderer acts on.
    Add,
    /// `MOD` = 4 — `GL_DST_COLOR, GL_ZERO`. Drawn as straight alpha here.
    Mod,
}

impl BlendMode {
    /// Parse the enum's own spelling, as `SetBlendMode` and the XML `alphaMode=` attribute take it.
    /// Case-insensitive (the Lua binding coerces through the shared string-arg law); `None` for a
    /// name outside the table's five.
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

    /// The reference's own spelling, as `GetBlendMode()` answers it (the name column of `0x811aa8`).
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

/// A `FontString`'s glyph outline (the XML `outline`/`<Font outline=>` attr, OUTLINETYPE): `NONE`
/// (the common case), `NORMAL`, or `THICK`. Resolved through the font registry and readable via
/// `GetFont`; the v1 text renderer does **not** yet rasterize outlines (a documented gap — the atlas
/// bakes plain coverage), so this is carried for fidelity/round-tripping, not drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outline {
    #[default]
    None,
    Normal,
    Thick,
}

impl Outline {
    /// Parse the **XML** OUTLINETYPE attribute (`outline="NONE"/"NORMAL"/"THICK"`,
    /// case-insensitive); unknown ⇒ `None`. This is *not* the Lua spelling — see [`Outline::flags`].
    pub fn parse(s: &str) -> Outline {
        match s.to_ascii_uppercase().as_str() {
            "NORMAL" => Outline::Normal,
            "THICK" => Outline::Thick,
            _ => Outline::None,
        }
    }

    /// Parse a **Lua `SetFont` flags string** — a different vocabulary from the XML attribute
    /// above, and mixing the two is a silent no-op: `SetFont(path, h, "OUTLINE")` through
    /// [`Outline::parse`] matches nothing and lands on `NONE`.
    ///
    /// The reference reads the argument as a *set* of substrings against
    /// `{0x1 OUTLINE, 0x4 THICKOUTLINE, 0x2 MONOCHROME}` (`0x811b10`), which is why this is
    /// `contains` and not
    /// equality — `"OUTLINE, MONOCHROME"` is a real thing addons write. MONOCHROME has no field
    /// here (we model no glyph AA mode) and is ignored; `THICK` is checked first because
    /// `"THICKOUTLINE"` contains `"OUTLINE"`.
    ///
    /// One function, three callers: `Button:SetFont`, `FontString:SetFont` and `Font:SetFont` all
    /// delegate to the *same* impl in the reference (`0x79f210`), so they cannot be allowed to
    /// disagree here. They did: the Font-object binding parsed its Lua flags with the XML reader,
    /// so `GameFontNormal:SetFont(f, h, "OUTLINE")` silently cleared the outline instead of setting
    /// it, while the FontString binding carried its own inline copy of this match.
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

    /// The OUTLINETYPE token (for `GetFont`'s flags return).
    pub fn as_str(self) -> &'static str {
        match self {
            Outline::None => "",
            Outline::Normal => "OUTLINE",
            Outline::Thick => "THICKOUTLINE",
        }
    }
}

/// A font object's drop shadow (`<Shadow><Offset/><Color/></Shadow>`): pixel offset in WoW y-up
/// XML convention (`y="-1"` = 1px down) + color. Inherited down `<Font>` chains like every other
/// paint field (the 1.12 `MasterFont` root sets `(1,-1)` black — ref-Fonts.xml l.55-62).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontShadow {
    /// XML `<Offset><AbsDimension x= y=/>` — WoW y-up (negative y = down-screen).
    pub offset: [f32; 2],
    /// `<Color r= g= b= a=/>`; alpha defaults 1.
    pub color: [f32; 4],
}

/// A named virtual **Font object** (`<Font name=… font=… inherits=…>`): the resolved paint a
/// `FontString` picks up by `inherits=`/`SetFontObject`. Values are already flattened through the
/// `<Font>` inherits chain (rooted at a real TTF path); any field the chain never set is `None`
/// (the renderer falls back). This is the registry entry [`Model::font_objects`](super::Model::font_objects) keys by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontObject {
    /// The TTF path (`font="Fonts\\FRIZQT__.TTF"`), inherited down the chain.
    pub font: Option<String>,
    /// The font height in logical px (`<FontHeight><AbsValue val=/>`).
    pub height: Option<f32>,
    /// The drop shadow (`<Shadow>`), inherited down the chain like the rest of the paint.
    pub shadow: Option<FontShadow>,
    /// The text color (`<Color r= g= b= a=/>`).
    pub color: Option<[f32; 4]>,
    /// The glyph outline (`outline=` attr).
    pub outline: Outline,
    /// The default horizontal justification (`justifyH=` attr), if the object sets one.
    pub justify_h: Option<JustifyH>,
    /// The default vertical justification (`justifyV=` attr), if the object sets one.
    pub justify_v: Option<JustifyV>,
}

/// One entry of the render list [`UiScript::extract`](super::UiScript::extract) produces: a draw target in the client's exact
/// painter order (the [`crate::order::traversal`] `ZKey` order), its resolved screen rect (if the
/// layout pass resolved it), the effective alpha to draw at, and its content. This is the seam the
/// Bevy extractor consumes; it names only this crate's own types (handles, [`Rect`]) — no engine.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractedQuad {
    /// The draw target (a frame's own slot, or one of its regions).
    pub target: ZTarget,
    /// The packed draw-order key (`ZKey::raw`); entries are already sorted ascending by it.
    pub z: u64,
    /// The resolved rect (`[bottom, left, top, right]`, screen px, y-up). `None` if the owning frame
    /// is under-constrained (no resolvable anchors) — a renderer skips it. A region's rect is its
    /// own owner-relative resolution when it carries anchors, else centered-on / filling its owner
    /// (see [`UiScript::extract`](super::UiScript::extract)).
    pub rect: Option<Rect>,
    /// The effective alpha to draw at: for a frame slot its own `effective_alpha`; for a region the
    /// verified single-hop product `region.alpha × ownerFrame.effective_alpha` (see
    /// [`RegionData::alpha`]).
    pub alpha: f32,
    /// The renderable content.
    pub content: QuadContent,
    /// The ScrollFrame clip this quad draws within: `Some(rect)` when the owning
    /// frame is a ScrollFrame's scroll child, or any descendant of one — nested ScrollFrames
    /// intersect (see [`UiScript::extract`](super::UiScript::extract)'s `effective_clip`). `None` = unclipped, the common case.
    pub clip: Option<Rect>,
    /// The owning frame's `effective_scale` (`0x76ac90`: `parentScale · ownScale`). The
    /// `rect` above already carries it — the layout solver multiplies every placement by the
    /// owner's scale — but glyph METRICS don't live in the rect: the renderer must rasterize a
    /// `Text` quad's font at `font_height × scale` (and scale the shadow offset with it) or a
    /// `SetScale`'d window draws scaled boxes full of unscaled text, the exact divergence
    /// decision 0219 §2 recorded. 1.0 for the unscaled common case.
    pub scale: f32,
}

/// The visual state of a region leaf (`Texture`/`FontString`), stored beside the arena because
/// [`crate::widget::Region`] models only structure (kind/owner/layer), not paint. See [`object`]'s
/// `SetTexture`/`SetVertexColor`/`SetText`.
/// A two-stop linear gradient (`SetGradientAlpha`/`SetGradient`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gradient {
    /// `true` for `"VERTICAL"`, `false` for `"HORIZONTAL"` — the client matches the token
    /// case-insensitively and treats anything else as horizontal.
    pub vertical: bool,
    /// The gradient's two stops, RGBA. `SetGradient` (no alpha) sets both alphas to 1.
    pub start: [f32; 4],
    pub end: [f32; 4],
}

impl Gradient {
    /// The single colour a one-tint quad can show for this gradient — the midpoint.
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
    /// Region-level visibility (`Show`/`Hide` on a Texture/FontString, XML `hidden=` — the real
    /// VisibleRegion bit): a hidden region emits no quad regardless of its owner frame's state.
    /// The ref's own kit relies on it (PanelTemplates_SelectTab hides tab slice textures;
    /// ContainerFrame_Update hides cooldown textures).
    pub(crate) hidden: bool,
    /// The region's own alpha (`SetAlpha`/`GetAlpha`, XML `alpha=`); `None` = never set = 1.0.
    ///
    /// A region draws at `ownAlpha × ownerFrame.alpha` — a **single hop** to its immediate owner,
    /// never a product up the tree (frame `SetAlpha 0x76a690`
    /// overwrite-cascades onto child *frames* and only *invalidates* child regions, which re-read
    /// the owner's `+0xc8` at draw). [`UiScript::extract`](super::UiScript::extract) folds this into
    /// [`ExtractedQuad::alpha`] alongside that owner alpha.
    ///
    /// **Settled for a Texture, kept distinct anyway:** `Texture:SetAlpha 0x79b580` is
    /// `SetVertexColor(r, g, b, a)` on the *same* `+0xa8`/`+0xb8` slot as [`Self::vertex_color`], so
    /// in the real client the two clobber each other. We keep them separate fields because the draw
    /// result is identical either way — both readings end in one product,
    /// `texel × ownColorAlpha × frameAlpha` — and no reference call site sets both on one region;
    /// separate storage additionally keeps `SetAlpha` on a FontString from wiping its font object's
    /// colour, which the CSimpleFontString half of that binding was never pinned for. The
    /// divergence is now *known* rather than open: a call site that sets both is where it bites.
    pub(crate) alpha: Option<f32>,
    /// A texture path (`SetTexture("Interface\\...")`).
    pub(crate) texture: Option<String>,
    /// The **solid-colour texture** (`SetTexture(r, g, b, a)`, and XML `<Texture><Color/></Texture>`
    /// with no `file=`) — the `+0xcc` slot [`Self::texture`] also occupies, so the two are mutually
    /// exclusive by construction: every writer of one clears the other. Not a tint. The real client
    /// really does *generate an 8×8 texture* here (`CSimpleTexture::SetTexture(const CImVector*)`
    /// `0x770360` → `0x44a9c0` → the `rep stos` generator `0x5c5350` fills 64 texels with the packed
    /// ARGB), which is why the XML alpha rides in the **texel** and multiplies with
    /// [`Self::vertex_color`] rather than being replaced by it.
    pub(crate) fill: Option<[f32; 4]>,
    /// `SetGradientAlpha(orientation, r1,g1,b1,a1, r2,g2,b2,a2)` / `SetGradient(...)` — the two-stop
    /// linear gradient the client generates into the same texture slot the colour form fills.
    ///
    /// **Stored in full, painted as its midpoint, and that gap is stated rather than implied.** The
    /// UI renderer has one tint per quad (a colour region is the shared 1x1 white image tinted), so
    /// there is nowhere yet to put a second stop; folding to the average is a *visible*
    /// approximation and belongs to the director to judge (method §7). It is kept whole HERE so the
    /// renderer can honour it later without another API change — the data is already right, only
    /// the paint is coarse.
    ///
    /// Measured cost of the approximation on the corpus's biggest consumer: `FuBar_Panel.lua:144`
    /// is `SetGradientAlpha("VERTICAL", 1,1,1,0, 1,1,1,0.5*t)` — **white at both stops, alpha
    /// only**, i.e. a soft fade, and both textures are `Hide()`n on the next two lines. A uniform
    /// half-alpha band instead of a fade is the whole difference there.
    pub(crate) gradient: Option<Gradient>,
    /// The **vertex colour** (`SetVertexColor 0x79abd0` → `0x77f750`, writing `+0xb8`; a StatusBar's
    /// `SetStatusBarColor`; a FontString's `SetTextColor` and its font object's colour) — storage
    /// distinct from [`Self::fill`]/[`Self::texture`]'s `+0xcc`.
    ///
    /// **Draw law** (`0x77f750`):
    /// `drawn = texel × vertexColour`, per channel, **alpha included**. `None` = never set = the
    /// untinted white every region draws at by default. This is why the reference `SkillFrame`'s
    /// row trough — declared `<Color 1,1,1,0.2>`, then `SetVertexColor(0, 0, 0.75, 0.5)`'d — draws
    /// at alpha `0.2 × 0.5 = 0.1`, not `0.5`. [`super::extract`] does that multiply.
    pub(crate) vertex_color: Option<[f32; 4]>,
    /// This region is a **portrait**: draw its texture masked to the inscribed circle (set by
    /// `SetPortraitToTexture`, and by `SetPortraitTexture`'s round unit binding). WoW portraits
    /// are circular — the frame ring is a thin band whose transparent corners would otherwise
    /// expose the square icon/model. The app bakes the circular alpha mask; the flag rides
    /// through [`QuadContent::Texture::circular`]. False with a live `portrait_unit` = the square
    /// booth pane (`BenillaSetBoothTexture`).
    pub(crate) circular: bool,
    /// A **live unit portrait** (`SetPortraitTexture(region, unit)` round;
    /// `BenillaSetBoothTexture(region, token)` square): the unit token whose model this region
    /// renders. `Some` ⇒ the region samples the app's off-screen portrait bake for this token
    /// (ignoring `texture`/`color`); rides through [`QuadContent::Texture::portrait_unit`]. Cleared by
    /// `SetTexture`/`SetPortraitToTexture` (the region becomes an ordinary texture again).
    pub(crate) portrait_unit: Option<String>,
    /// FontString text (`SetText`).
    pub(crate) text: Option<String>,
    /// The write-on reveal (`SetAlphaGradient(start, length)` — CSimpleFontString's per-character
    /// alpha gradient, the quest-description "writing" machinery): characters before `start` draw
    /// opaque, the next `length` ramp 1→0, the rest invisible. `None` = no gradient (every
    /// FontString but an armed quest description). Cleared by `SetText` — fresh text draws whole.
    pub(crate) alpha_gradient: Option<(f32, f32)>,
    /// The region's blend mode (`SetBlendMode` / XML `alphaMode` — the reference's `[texture+0xd0]`,
    /// the shared enum `0x811aa8`). Highlight state textures are created at
    /// [`BlendMode::Add`] (the client's `SetHighlightTexture` contract); everything else starts at
    /// the ctor's [`BlendMode::Blend`].
    ///
    /// **The whole mode is kept, not just "is it ADD"**, because `GetBlendMode` answers it back as
    /// a string and a getter that could only ever say `"BLEND"` or `"ADD"` would answer `"BLEND"`
    /// to a texture the caller set to `"MOD"`. The renderer still reads only the ADD/not-ADD
    /// distinction ([`QuadContent::Texture::additive`]) — that is unchanged, and the three modes it
    /// flattens are the stated gap they always were.
    pub(crate) blend: BlendMode,
    /// Explicit region size (`SetWidth`/`SetHeight`/XML `<Size>`); `None` = derive. Size fills the
    /// axis the anchors don't pin (the client's "0 = derive") — so under a texture's implicit
    /// SetAllPoints (two corners pin everything) an authored size is structurally unread, which is
    /// how the reference stack-split plate authors 256×32 and renders 172×96.
    pub(crate) size: Option<(f32, f32)>,
    /// Region anchors (`SetPoint`/XML `<Anchors>` on a Texture/FontString). An anchor's
    /// `relative_to` defaults to the owner frame and may name a frame or a **sibling region**
    /// (resolved via [`Model::region_names`]; the fixpoint in [`UiScript::resolve`] orders the
    /// sibling chain). Every drawable region carries at least one — authored, or the
    /// creation-path implicit anchor ([`super::region::implicit_creation_anchor`], decision
    /// 1310); empty means a templateless Lua region nobody anchored, which never resolves and
    /// never draws. Non-empty ⇒ resolved in [`UiScript::resolve`], any edge the anchors leave
    /// unset inherited from the owner frame's rect.
    pub(crate) anchors: Vec<Anchor>,
    /// `Texture:SetDesaturated(flag)` — the shader desaturation state (`0x79c1e0`).
    /// Rides the extract as [`QuadContent::Texture::desaturated`] and the
    /// renderer greys the texel by it, so the binding answers "shader supported"
    /// — see `region/paint.rs`'s `SetDesaturated`.
    pub(crate) desaturated: bool,
    /// `SetNonSpaceWrap` / `CanNonSpaceWrap` — FontString only (`0x79e9f0`/`0x79ead0`).
    /// **State only here.** The real client's gx flag `0x40` feeds a
    /// mid-word wrap carry and the fit-count terminator whose one consumer is the ellipsis
    /// truncate at `0x771ec0`; our text layout has no ellipsis path, so honouring the value in
    /// layout would mean inventing one. Stored and answered faithfully, unread by the renderer —
    /// which is the honest position and is stated rather than left to be discovered.
    ///
    /// Default is ON (a no-arg `SetNonSpaceWrap()` also enables), so `None` reads as enabled.
    pub(crate) non_space_wrap: Option<bool>,
    /// FontString justification, as the client's own **dword** rather than a pair of resolved
    /// enums — `CSimpleFontString+0x120`, bits 0–2 horizontal and 3–5 vertical, defaulting to the
    /// ctor's `0x212` (CENTER|MIDDLE). See [`crate::justify::Justify`]: an axis can be *cleared*,
    /// which no resolved enum can hold, and the Lua getter and the draw path then answer that
    /// state differently (`"UNKNOWN"` vs centred) — both faithfully.
    pub(crate) justify: crate::justify::Justify,
    /// The `<TexCoords>`/`SetTexCoord` UV mapping ([`TexCoords`]: the 4-edge crop, or the 8-arg
    /// affine quad). `None` = the full texture. Slices the quadrant/atlas art.
    pub(crate) tex_coords: Option<TexCoords>,
    /// `Texture:SetTexCoordModifiesRect(flag)` / `GetTexCoordModifiesRect()` — the reference's
    /// `[texture+0x124]`, whose sole writer is `SetTexCoordModifiesRect 0x79c080`
    /// (`0x79c113 mov [edi+0x124],eax`) and which is BSS-zero otherwise.
    ///
    /// **State only, and the geometry half is NOT wired.** In the reference the flag gates a
    /// rect-recompute leg (`0x770462`): with it set, a `SetTexCoord` re-derives the
    /// region's own rect from the UV quad rather than leaving the rect alone and merely resampling
    /// inside it. Honouring that here means `region::layout`'s resolve reading this flag and
    /// deriving the region's rect from [`Self::tex_coords`] instead of from its anchors and size —
    /// a resolve-order change, not a paint one, and nothing in either addon corpus or the stock UI
    /// calls the SETTER, so there is no measured case to build it against. Stored and answered
    /// truthfully, unread by layout; stated here rather than left to be discovered.
    pub(crate) tex_coord_modifies_rect: bool,
    /// On-screen rotation about the region center, radians, counterclockwise-positive.
    /// **Unwritable: the `Texture:SetRotation` that was its only setter is gone** (1.12 registers
    /// the name on PlayerModel alone) — see [`QuadContent::Texture::rotation`] for why it is still
    /// here.
    pub(crate) rotation: f32,
    /// The named font object this FontString last resolved (`inherits=`/`SetFontObject`) — our
    /// `FONTINSTANCE+0x028 parentFontObject`, and a **live** link: mutating that object
    /// (`GameFontNormal:SetFont(…)`) re-paints this region through
    /// [`script::font::propagate`](super::font::propagate). `GetFontObject` returns its handle.
    /// The object's paint is eagerly copied into the fields below so every reader stays a plain
    /// field read.
    pub(crate) font_object: Option<String>,
    /// Which of the font properties below this region set **for itself** since its last
    /// `SetFontObject` — our `FONTINSTANCE+0x038 explicitlySetMask`. See [`FontExplicit`].
    pub(crate) font_explicit: FontExplicit,
    /// Resolved font face path (`SetFont`/`SetFontObject`). `None` = the renderer's default face.
    pub(crate) font_path: Option<String>,
    /// Resolved font height in logical px (`SetFont`/`SetFontObject`/`<FontHeight>`). `None` = default.
    pub(crate) font_height: Option<f32>,
    /// A `SetTextHeight` scale override — the client's two text-size regimes:
    /// a FontString defaults to **one-to-one** (bit `0x200`,
    /// drawn at the raster px, subject to the 32-px raster cap); `SetTextHeight 0x771600` is the
    /// ONLY clearer — the literal size then flows through UNCAPPED, magnified from the raster.
    /// `Some` = that regime; the renderer draws at this height and skips the one-to-one cap.
    pub(crate) text_height: Option<f32>,
    /// Resolved glyph outline (`SetFont` flags / font object). Carried, not yet drawn (see [`Outline`]).
    pub(crate) outline: Outline,
    /// Resolved drop shadow (font object `<Shadow>`), drawn by the renderer behind the glyphs.
    pub(crate) font_shadow: Option<FontShadow>,
    /// The host-measured wrapped text size for a FontString with no explicit height — the engine's
    /// side of the measure round-trip ([`UiScript::fontstrings_needing_measure`] →
    /// [`UiScript::set_measured_text`]): the real client's layout asks its font engine for string
    /// metrics exactly like this. `key` invalidates on text/font/wrap changes.
    pub(crate) measured: Option<MeasuredText>,
}

/// The per-property "this region set it itself, so it does not inherit" record — the real client's
/// `FONTINSTANCE+0x038 explicitlySetMask`.
///
/// It exists for exactly one job: when a **font object is mutated**
/// (`GameFontNormal:SetTextColor(…)`), every region inheriting it re-reads the object, and a
/// property the region had overridden must survive that re-read. A dropdown row that does
/// `SetFontObject(GameFontNormalSmall)` and then `SetTextColor(1, 0, 0)` keeps its red.
///
/// A fresh `SetFontObject` **clears the whole mask** — re-pointing at an object is a deliberate
/// "take this object's paint", and `Dewdrop-2.0` re-runs exactly that pair on every row refresh
/// (`SetFontObject` at l.2160-2166, then `SetTextColor` at l.2180), which only stays correct if the
/// re-point wins. (Whether the real client's mask outlives a re-point is the one part of this the
/// bytes have not yet been asked; the choice above is the one that leaves current behaviour
/// untouched, and it is the *propagation* half — new behaviour either way — that the mask governs.)
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
    /// Reserved for a region-level shadow setter; XML `<Shadow>` on the FontString itself.
    pub(crate) shadow: bool,
    /// `SetJustifyH`, or XML `justifyH=`.
    pub(crate) justify_h: bool,
    /// `SetJustifyV`, or XML `justifyV=`.
    pub(crate) justify_v: bool,
}

/// A cached host measurement of a FontString's laid-out text (see [`RegionData::measured`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MeasuredText {
    /// The extent of the text **as laid out** — wrapped inside the region's declared width, if it
    /// has one. The auto-size input, and what `GetWidth`/`GetHeight` echo.
    pub(crate) w: f32,
    pub(crate) h: f32,
    /// The extent the text would take with **no wrap constraint** — what `GetStringWidth` reports,
    /// and a different number from [`Self::w`] for exactly those regions that carry a declared width.
    ///
    /// The reference keeps only this one (`GetStringWidth 0x79e510` → `0x772890`, cached at
    /// `+0xfc`): it measures the raw text with **no wrap constraint**, so "Lua sees the natural,
    /// unwrapped, un-truncated width at the DRAWN size". benilla needs both, because its auto-size
    /// path reads the laid-out extent off this same cache.
    ///
    /// Serving the laid-out width to `GetStringWidth` instead is a **feedback loop**, not a rounding
    /// difference: any kit that sizes a box from `GetStringWidth` and then sets a width on the string
    /// — which is what the reference's own `PanelTemplates_TabResize` does — reads its own output
    /// back as its next input. The macro window's character tab changed width every single frame for
    /// exactly this reason.
    pub(crate) natural_w: f32,
    /// Hash of (text, font path, font height bits, wrap-width bits) — mismatch ⇒ re-measure.
    pub(crate) key: u64,
}

impl MeasuredText {
    /// Did the part of this measurement the **layout** reads move?
    ///
    /// Only `w`/`h` are layout inputs — they are the auto-size axes, and the only fields
    /// `InputFingerprint` feeds (`script::layout`'s region walk hashes `m.w`/`m.h`, nothing else).
    /// `key` is the measure CACHE's business and `natural_w` is `GetStringWidth`'s; neither can
    /// move a rect.
    ///
    /// The distinction is load-bearing, not tidiness. A measure request is only ever *issued* when
    /// the stored key mismatches, and `MeasuredText`'s derived `PartialEq` includes `key` — so
    /// `d.measured != Some(new)` is **structurally always true** at both write sites, and touching
    /// the epoch on it meant every answered measure opened tier 1. A FontString whose text changes
    /// to something the same size does that on a cadence: a buff countdown ticking `"58s" → "57s"`,
    /// a colour-code swap, any same-width digit. Tier 1 then went dirty and tier 2 hashed all
    /// ~10k anchored regions to conclude nothing had moved — ~1 ms, `solves=0`, per tick, times
    /// the number of unsynchronised timers on screen. The same shape 0740's own doc records the
    /// bag-hover loop paying, and a sibling of the castbar's (1385).
    pub(crate) fn layout_moved(before: Option<Self>, after: Self) -> bool {
        before.is_none_or(|b| {
            b.w.to_bits() != after.w.to_bits() || b.h.to_bits() != after.h.to_bits()
        })
    }
}

impl RegionData {
    /// The measure-cache key for the region's CURRENT text/font/wrap/outline — the one recipe both
    /// sides of the measure round-trip share. [`super::UiScript::fontstrings_needing_measure`]
    /// stamps it into each request (and skips regions whose stored [`MeasuredText::key`] still
    /// matches); the Lua-visible metric reads (`GetStringWidth`/`GetWidth`, region.rs) treat a
    /// stored measure whose key mismatches as ABSENT — a measure of text this region no longer
    /// holds is not a metric (the whisper-header cursor bug: `SetText("Tell …: ")` then a
    /// same-frame `GetWidth()` served the OLD header's width, and the edit box latched its text
    /// insets on it). The layout solver's own read (layout.rs) deliberately does NOT key-check:
    /// it keeps the last-known box until the fresh measure lands, so lines never collapse for the
    /// round-trip frame. **With one carve-out, and it is the carve-out this cache needs**: EMPTY
    /// text is never measured at all (both measure asks filter it out), so "until the fresh
    /// measure lands" is a promise that can never be kept for a cell that goes from text to `""`
    /// — the solver drops the stored measure there rather than hold a dead box forever (the
    /// item tooltip's blank SET spacers drew rows the plate counted as zero).
    /// `scale` is the owner frame's `effective_scale`: it's in the key because the host measures
    /// at the drawn raster size ([`MeasureRequest::scale`]), so a `SetScale` under a cached
    /// measure must invalidate it exactly like a font-size change.
    pub(crate) fn measure_key(&self, scale: f32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // By reference: `String` hashes exactly as `str` does, and this runs for every
        // FontString on every frame's staleness sweep — a clone here was ~1.5k heap allocs a
        // frame at a city pin, all discarded at the key compare.
        self.text.as_deref().unwrap_or("").hash(&mut hasher);
        self.font_path.hash(&mut hasher);
        self.font_height.map(f32::to_bits).hash(&mut hasher);
        self.text_height.map(f32::to_bits).hash(&mut hasher);
        let wrap_width = self.size.map(|s| s.0).filter(|w| *w > 0.0);
        wrap_width.map(f32::to_bits).hash(&mut hasher);
        // Outline is in the key because it changes measured WIDTH under the client's step law
        // (an outlined font steps +1px more per glyph — GlyphStepBase 0x5ca2b0).
        (self.outline as u8).hash(&mut hasher);
        scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}

/// The focused EditBox's advance-table request — returned by
/// [`UiScript::editbox_advances_request`](super::UiScript::editbox_advances_request), answered
/// via [`UiScript::set_editbox_advances`](super::UiScript::set_editbox_advances) with the
/// per-byte cumulative laid-out widths of `text` (len+1 entries, `[0] = 0`, a continuation byte
/// repeating its lead's value). The metrics seam that makes click→char-index, drag-select, and
/// the caret scroll window engine-local (the click handler `0x77b800` and its caret leaves).
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxAdvanceRequest {
    /// Opaque frame id of the box — pass back verbatim.
    pub id: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// The resolved glyph outline — the per-glyph step bias (+1px, GlyphStepBase 0x5ca2b0)
    /// changes every advance.
    pub outline: Outline,
    /// The box frame's `effective_scale`. The host measures at the drawn raster size
    /// (`height × scale × seam`) but divides the answer by the SEAM ALONE, so the advances come
    /// back in screen UI units — the space the engine's ÷seam mouse feed and the box's resolved
    /// (scale-multiplied) rect both live in. Contrast [`MeasureRequest::scale`], whose answer is
    /// frame-local.
    pub scale: f32,
    /// The DISPLAY string (mask-aware) the table indexes.
    pub text: String,
    /// `Some(width)` for a multiline box: also answer the wrapped-row starts + row pitch at this
    /// wrap width (the text region's resolved width — the same width the draw wraps at), via
    /// [`UiScript::set_editbox_advances`](super::UiScript::set_editbox_advances)'s `rows`/`cell_h`.
    /// `None` = single-line (answer `rows = [0]`).
    pub wrap_width: Option<f32>,
    /// Cache key — pass back verbatim.
    pub key: u64,
}

// `EditAction`/`EditUnit` are the *editing law's* vocabulary, so they live beside the state that
// law mutates ([`crate::widget::EditBoxState`]) rather than here — `script` depends on `widget`,
// never the other way round, and the glue screens drive the law with no Lua VM in sight.
// Re-exported here because this is the path the host has always used.
pub use crate::widget::{EditAction, EditOutcome, EditUnit};

/// The focused EditBox's per-frame text-UI geometry — returned by
/// [`UiScript::focused_editbox_text_ui`](super::UiScript::focused_editbox_text_ui): everything
/// the host needs to draw the box's window substring, selection highlight, and caret.
#[derive(Clone, Debug, PartialEq)]
pub struct EditBoxTextUi {
    /// The text region's draw target — match the extracted Text quad by it.
    pub target: crate::order::ZTarget,
    /// Draw `display[display_from..]` (the scroll window's start, a DISPLAY byte offset).
    /// Always 0 for a multiline box (it wraps in place instead of windowing).
    pub display_from: usize,
    /// The box is multiline: the host draws the full wrapped block (no window, no unbounded
    /// rect) and seats the caret/selection by `(row, x)` at the answered row pitch.
    pub multi_line: bool,
    /// Caret x in px from the drawn window's text origin (multiline: from the row's origin).
    pub caret_x: f32,
    /// Caret wrapped-row index (always 0 single-line) — the vertical half of the 2-D seat.
    pub caret_row: usize,
    /// The answered row pitch in px (0 until the advance answer lands); the host may prefer its
    /// own draw-side line cell — the two are the same law (the snapped font em).
    pub cell_h: f32,
    /// The blink phase — draw the caret this frame (hosts may pin it on for captures).
    pub caret_on: bool,
    /// The selection as per-row `(row, x0, x1)` px spans from each row's origin, when non-empty
    /// (single-line: at most one span, row 0 — the old scalar span).
    pub selection: Vec<(usize, f32, f32)>,
    /// The selection highlight tint (`SetHighlightColor`; ctor default opaque `0x606060` gray).
    pub highlight_color: [f32; 4],
}

/// One FontString the layout needs host-measured this frame (no explicit height; text present).
/// Returned by [`UiScript::fontstrings_needing_measure`](super::UiScript::fontstrings_needing_measure); answer via [`UiScript::set_measured_text`](super::UiScript::set_measured_text).
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureRequest {
    /// Opaque region id — pass back verbatim.
    pub id: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// The `SetTextHeight` override (see [`RegionData::text_height`]): measured at this height,
    /// uncapped, when present — the drawn size is the measured size (`0x772890`).
    pub text_height: Option<f32>,
    /// Wrap width when the region's width is pinned (explicit `<Size x>`), else single-line.
    pub wrap_width: Option<f32>,
    /// The resolved glyph outline — it biases the client's per-glyph step (+1px, GlyphStepBase
    /// 0x5ca2b0), so the measured width depends on it.
    pub outline: Outline,
    /// The owner frame's `effective_scale`. The host must measure at the DRAWN raster size —
    /// `font_height × scale × its own screen seam` — and divide the px answer back by the full
    /// product: glyph advances step to whole pixels (the step law), so a measure taken at the
    /// unscaled size is not `scale ×` the drawn one, and an auto-sized FontString in a scaled
    /// frame would mis-fit its own text (spurious wrap → spurious "..." ellipsis).
    pub scale: f32,
    pub text: String,
    /// Cache key — pass back verbatim.
    pub key: u64,
}

/// One ScrollingMessageFrame ring line whose wrapped **row count** needs a host measurement (its
/// cache key went stale — new line, width change, font change). The message-frame half of the
/// measure round-trip: returned by
/// [`UiScript::message_lines_needing_measure`](super::UiScript::message_lines_needing_measure),
/// answered via [`UiScript::set_message_line_rows`](super::UiScript::set_message_line_rows) with
/// `(frame, index, rows, key)` — same-frame, before extract, no re-resolve needed (rows shift only
/// the emitted bands, never the anchor graph).
#[derive(Clone, Debug, PartialEq)]
pub struct LineMeasureRequest {
    /// Opaque frame id — pass back verbatim.
    pub frame: u32,
    /// The line's ring index at collect time — pass back verbatim (the ring cannot shift between
    /// collect and answer within one host drive; the key still guards a stale store).
    pub index: u32,
    pub font: Option<String>,
    pub height: Option<f32>,
    /// The frame's resolved inner width — the wrap constraint. Already scale-multiplied (it is
    /// the resolved rect's width), unlike `height`, which is the font's local size.
    pub wrap_width: f32,
    /// The resolved glyph outline (step-law width bias, as on [`MeasureRequest`]).
    pub outline: Outline,
    /// The frame's `effective_scale` — the host wraps at the drawn raster size
    /// (`height × scale × seam`), as on [`MeasureRequest::scale`].
    pub scale: f32,
    pub text: String,
    /// Cache key — pass back verbatim.
    pub key: u64,
}
