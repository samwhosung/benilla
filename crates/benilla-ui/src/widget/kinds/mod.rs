//! Widget kinds: [`FrameKind`], [`RegionKind`] and the per-kind state, [`KindState`].

use std::collections::{HashSet, VecDeque};

use super::{FrameHandle, RegionHandle};

mod editbox;
mod messageframe;
pub use editbox::*;
pub use messageframe::*;

/// A [`Frame`]'s widget class; one with modeled behavior carries it in [`Frame::kind_state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameKind {
    /// Plain `CSimpleFrame`, the base container.
    Frame,
    /// `CGWorldFrame` (factory `0x4959d0`), the singleton the world renders behind: a `Frame` to
    /// Lua, in stratum `WORLD` below `BACKGROUND`; hovered, it is not over the UI but runs scripts.
    WorldFrame,
    Button,
    /// `CLootButton` (factory `0x495a30`): a `CSimpleButton` plus a loot slot and an `OnClick`
    /// (`0x4c1820`) that takes it; a registered `CreateFrame` type the stock `LootFrame.xml` needs.
    LootButton,
    CheckButton,
    EditBox,
    StatusBar,
    Slider,
    ScrollFrame,
    Model,
    /// `CGCharacterModelBase` (`0x505680`, factory `0x495bd0`): `Model` plus `SetUnit`,
    /// `RefreshUnit` and `SetRotation` (table `0x84f1fc`), which a plain `<Model>` lacks.
    PlayerModel,
    /// `CGDressUpModelFrame` (factory `0x495c00`): `PlayerModel` plus `Undress`, `Dress` and
    /// `TryOn` (table `0x84f190`); a try-on is composed app-side against live equipment.
    DressUpModel,
    /// `TabardModel` (`0x503bd0`): `CGCharacterModelBase` plus ten verbs (table `0x84ee40`).
    TabardModel,
    /// `CSimpleMessageFrame`, `UIErrorsFrame`'s class: a sibling of the scrolling one.
    MessageFrame,
    /// `CSimpleMessageScrollFrame`, the chat window's ring-buffered class; a sibling of
    /// [`FrameKind::MessageFrame`] whose offsets do not transfer (bases `0x7a3540`/`0x769090`).
    ScrollingMessageFrame,
    ColorSelect,
    SimpleHtml,
    MovieFrame,
    /// The `GameTooltip` family, a game-layer factory; its 38 bindings are `0x530c40`..`0x5364a0`.
    GameTooltip,
    /// The `<Minimap>` widget, a game-layer factory (`0x495940`) the app draws the map into.
    Minimap,
}

/// The kind of a [`Region`] leaf, the client's `CScriptRegion`-derived non-frame objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegionKind {
    /// A `Texture` (BLP quad).
    Texture,
    /// A `FontString` (text run).
    FontString,
    /// A frame's title region, the drag handle `Frame:CreateTitleRegion()` makes: a plain Region
    /// (`0x773910`) that answers `"Region"` and has only the 19 Region methods.
    Title,
}

/// The state a modeled kind adds over `CSimpleFrame`, carried on the [`Frame`] node.
#[derive(Clone, Debug, PartialEq)]
pub enum KindState {
    /// No modeled per-kind state.
    None,
    /// `CSimpleStatusBar` (factory `0x6eef20`, LoadXML `0x782ef0`): a value filling a bar texture.
    StatusBar(StatusBarState),
    /// `CSimpleButton`/`CSimpleCheckbox` (factories `0x6eeab0`/`0x6eeb30`, LoadXML
    /// `0x7788c0`/`0x785170`); the checkbox extends the button and uses the `checked` members.
    Button(ButtonState),
    /// `CSimpleEditBox` (factory `0x6eec70`): text, cursor, selection, flags and its FontString.
    EditBox(EditBoxState),
    /// `CSimpleMessageScrollFrame` (ctor `0x787670`): the line ring, fades and scrollback.
    ScrollingMessage(ScrollingMessageState),
    /// `CSimpleMessageFrame` (ctor `0x785640`): lines and `insertMode`, no ring or scrollback.
    Message(MessageFrameState),
    /// `CSimpleScrollFrame`: the scroll child and offsets. `0x786db0` stores an offset unclamped;
    /// `0x786e30` measures the range from the child's subtree.
    Scroll(ScrollFrameState),
    /// `CSimpleSlider` (factory `0x6eee40`, LoadXML `0x789580`), every scrollbar included.
    Slider(SliderState),
    /// `CSimpleColorSelect` (ctor `0x78b220`, factory `0x6eef90`, LoadXML `0x78b3f0`).
    ColorSelect(ColorSelectState),
    /// The scene state of every model kind; the pixels are the app's.
    Model(ModelState),
    /// The `<Minimap>` widget's zoom, mask and player arrow; the tiles and blips are the app's.
    Minimap(MinimapState),
    /// The `GameTooltip` line stack, owner and fade.
    Tooltip(TooltipState),
}

impl KindState {
    /// The display lines of either message-frame class, which share the [`MessageLine`] record.
    pub fn message_lines(&self) -> Option<&VecDeque<MessageLine>> {
        match self {
            KindState::ScrollingMessage(smf) => Some(&smf.lines),
            KindState::Message(mf) => Some(&mf.lines),
            _ => None,
        }
    }

    /// The message kinds' sweep-skip generation ([`ScrollingMessageState::lines_gen`]).
    pub fn lines_gen(&self) -> Option<u64> {
        match self {
            KindState::ScrollingMessage(smf) => Some(smf.lines_gen),
            KindState::Message(mf) => Some(mf.lines_gen),
            _ => None,
        }
    }

    /// [`Self::message_lines`], mutably, for the measure's write-back.
    pub fn message_lines_mut(&mut self) -> Option<&mut VecDeque<MessageLine>> {
        match self {
            // Any mutable borrow counts as a text change for the measure sweep's skip token.
            KindState::ScrollingMessage(smf) => {
                smf.lines_gen = smf.lines_gen.wrapping_add(1);
                Some(&mut smf.lines)
            }
            KindState::Message(mf) => {
                mf.lines_gen = mf.lines_gen.wrapping_add(1);
                Some(&mut mf.lines)
            }
            _ => None,
        }
    }
}

/// A tooltip's anchor mode (`+0x318`), set by `SetOwner` (`0x5310d0`), which reads a missing or
/// unknown mode as [`TooltipAnchor::Left`] (`0x53120d`). The ids follow `GetAnchorType`'s table
/// (`0x5313e0`, `0x531530`), not the setter's compare order. Placement (`0x52fe90`): modes 0..5
/// clear the anchors and point, 6 and 7 only clear, 8 does neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipAnchor {
    /// `ANCHOR_LEFT`: the plate's right edge on the owner's left.
    Left,
    /// `ANCHOR_RIGHT`: the plate's left edge on the owner's right.
    Right,
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
    /// `ANCHOR_CURSOR`: re-anchored to the live cursor every frame by the update `0x530b20`.
    Cursor,
    /// `ANCHOR_NONE`: cleared, and the caller points the plate. The default here: the ctor
    /// `0x529240` never writes `+0x318`, leaving the reference's start value to its allocator,
    /// and the getter answers this for any out-of-range id (`0x53146f`).
    #[default]
    None,
    /// `ANCHOR_PRESERVE`: the previous placement and its anchors are kept (`0x52fe90` returns at
    /// `0x52fead`, before the clear); `GetAnchorType` still answers `"ANCHOR_PRESERVE"`.
    Preserve,
}

impl TooltipAnchor {
    /// The name `GetAnchorType()` answers (table `0x531530`).
    pub const fn name(self) -> &'static str {
        match self {
            TooltipAnchor::Left => "ANCHOR_LEFT",
            TooltipAnchor::Right => "ANCHOR_RIGHT",
            TooltipAnchor::BottomLeft => "ANCHOR_BOTTOMLEFT",
            TooltipAnchor::BottomRight => "ANCHOR_BOTTOMRIGHT",
            TooltipAnchor::TopLeft => "ANCHOR_TOPLEFT",
            TooltipAnchor::TopRight => "ANCHOR_TOPRIGHT",
            TooltipAnchor::Cursor => "ANCHOR_CURSOR",
            TooltipAnchor::None => "ANCHOR_NONE",
            TooltipAnchor::Preserve => "ANCHOR_PRESERVE",
        }
    }
}

/// A `GameTooltip`'s state. Its lines are FontString regions named `<name>TextLeftN`/`TextRightN`,
/// made on demand and published as Lua globals, as the stock template's 30 pairs are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TooltipState {
    /// Lines currently filled (`NumLines`). Regions past this index exist but are hidden.
    pub num_lines: usize,
    /// The left-column FontString pool, index i = line i+1, grown on demand with no line cap.
    pub left_lines: Vec<RegionHandle>,
    /// The right-column pool (the `AddDoubleLine`/`TextRightN` half), parallel to `left_lines`.
    pub right_lines: Vec<RegionHandle>,
    /// `SetOwner`'s frame, dropped on hide.
    pub owner: Option<FrameHandle>,
    /// The last `SetOwner`'s mode, read back by `GetAnchorType()`.
    pub anchor: TooltipAnchor,
    /// `SetMinimumWidth`: a floor on the auto-sized width, cleared with the content.
    pub min_width: f32,
    /// `FadeOut()`'s start on the `GetTime` clock; any fresh content or `Show` cancels the fade.
    pub fade_start: Option<f64>,
    /// The unit this tooltip shows; a `set_unit` push for it re-drives the health bar.
    pub unit_token: Option<String>,
    /// Shows world-hover content, which fades when the hover is lost; a window hover never fades.
    pub world_owned: bool,
    /// Armed for a compare render: the next `SetInventoryItem` prepends "Currently Equipped"
    /// (`[arg+0x18]`, not the compact `[arg+0x14]`), surviving the stock caller's `SetOwner` in
    /// between (`MerchantFrame.xml:67-72`); the reference's route to `0x52b650` is untraced.
    pub equipped_header_armed: bool,
    /// `SetPadding`: extra width past the content (`ItemRef.xml:40` sets 16 for the close button).
    pub padding: f32,
    /// Line 1 came from XML (`ShoppingTooltipTemplate`), so later lines clone the previous fonts.
    pub xml_declared_lines: bool,
}

/// The plate's text inset: the stock template seats `TextLeft1` at TOPLEFT (10, −10).
pub const TOOLTIP_PAD: f32 = 10.0;
/// The line gap: each `TextLeftN` hangs at the previous line's BOTTOMLEFT (0, −2).
pub const TOOLTIP_LINE_GAP: f32 = 2.0;
/// The least double-line column gap, the stock `TextRightN` offset; the reference's is untraced.
pub const TOOLTIP_DOUBLE_GAP: f32 = 40.0;
/// `FadeOut`'s ramp length in seconds; the reference's constant is untraced.
pub const TOOLTIP_FADE_SECS: f64 = 0.5;
/// A wrap-flagged line's width, pinned when it is appended; the reference's column is untraced.
pub const TOOLTIP_WRAP_WIDTH: f32 = 260.0;

/// A model pane's scene: the `CSimpleModel` (`0x76c8e0`) members every model kind carries, read
/// and written by `Model`'s 23 methods (table `0x878948`); the render is the app's.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelState {
    /// `SetModel`'s path as written (`.mdx`); the app's loader maps it to the shipped `.m2`.
    pub path: Option<String>,
    /// `PlayerModel:SetUnit`'s unit (`0x505d70`); it and [`Self::path`] each clear the other.
    pub unit: Option<String>,
    /// The last `SetSequence` id (`[bone0 block + 0xf8]`); what plays is [`Self::armed`].
    pub sequence: i32,
    /// The pane's own scene clock in ms (`0x76cfc0`): its `OnUpdate` (`0x76d7f0`) adds
    /// `trunc(elapsed · 1000)` only while shown; `AdvanceTime` is inert, `SetModel` keeps it.
    pub clock_ms: u64,
    /// The sequence on bone slot 0. A `SetSequence` id the file does not own stops what played
    /// and arms nothing (`0x7121a0` interrupts before its bounds check).
    pub armed: Option<ArmedSequence>,
    /// `SetModel` ran and the loader's completion ([`ModelState::seed_from_facts`]) awaits facts.
    pub pending_seed: bool,
    /// `ReplaceIconTexture`'s override (`0x76cfe0`); `SetModel` and `ClearModel` drop it.
    pub icon: Option<String>,
    /// No size authored: the pane takes the file's box extent, `bboxExtent · 768·√(a²+1)` units
    /// (`0x6f1eb0`, `0x76d080`), re-derived on aspect change; an authored size clears it for good.
    pub implicit_size: bool,
    /// Yaw in radians (`+0x39c`), one slot written by both `Model:SetFacing` (`0x76dce0`) and
    /// `PlayerModel:SetRotation` (`0x505f00`), so `GetFacing` reads either back.
    pub facing: f32,
    /// `SetModelScale`: the model's scale within the pane, default 1.
    pub scale: f32,
    /// The camera index still to install (`+0x320`): the ctor's `Some(0)`, or a `SetCamera` made
    /// before the facts (`0x76cec0`). While it is `Some` the pane draws nothing (`0x76d5f0`).
    pub camera_pending: Option<i32>,
    /// The installed camera (`+0x31c`): a raw camera-table index, never via `cameraLookup`, drawn
    /// in perspective; `None`, the NULL camera an out-of-range index installs, draws orthographic.
    pub camera: Option<u32>,
    /// `SetPosition`: the model's offset within the pane's scene.
    pub position: (f32, f32, f32),
    pub light: ModelLight,
    /// `SetFogColor` as the reference stores it, one packed `0xAARRGGBB` dword, so a round trip
    /// keeps 8 bits a channel; the ctor's `0xffff_ffff` reads back as `1, 1, 1, 1`.
    pub fog_color: u32,
    /// Fog armed (`+0x3a4` bit 0): set by `SetFogColor` and `<FogColor>`, cleared by `ClearFog`,
    /// which leaves colour, near and far for the next `SetFogColor`.
    pub fog: bool,
    /// Fog near (`+0x3ac`): unclamped from `SetFogNear`, clamped `≥ 0` only from XML `fogNear`.
    pub fog_near: f32,
    /// Fog far (`+0x3b0`), shaped like [`Self::fog_near`]; the ctor's is `1.0`.
    pub fog_far: f32,
}

/// A model pane's armed fog, all the renderer needs ([`ModelState::armed_fog`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelFog {
    /// The packed `0xAARRGGBB` colour (`+0x3a8`); the fill callback `0x76d680` never reads alpha.
    pub color: u32,
    /// Raw near and far (`+0x3ac`/`+0x3b0`); a batch fogs only when `1/(far − near) > 0`, so
    /// `far <= near` draws unfogged.
    pub near: f32,
    pub far: f32,
}

impl ModelFog {
    /// The colour as `[r, g, b]` in `0..=1`, the fill callback's unpack (`0x7bbf20`).
    pub fn rgb(&self) -> [f32; 3] {
        [16, 8, 0].map(|shift| ((self.color >> shift) & 0xff) as f32 / 255.0)
    }
}

/// A model pane's embedded `CGLight` (`+0x324`), its only light (`0x76d680`): off on a plain
/// `<Model>` until Lua enables it, so a lit batch draws black (`0x71bf90`), and on for a
/// `PlayerModel` (ctor `0x505680`). Colours are stored times their intensity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelLight {
    /// `+0x60`: set by a nonzero `SetLight` (`0x76e1e0`) first argument; `SetLight(0, …)` is inert.
    pub enabled: bool,
    /// `+0x08`: point (`1`, both ctors) or directional; picks where `SetLight`'s `(x, y, z)` goes.
    pub omni: bool,
    /// Position (`+0x0c`), or a directional light's from-light direction (`+0x24`), normalised.
    pub vector: [f32; 3],
    /// `+0x30` ambient, intensity already folded in.
    pub ambient: [f32; 3],
    /// `+0x3c` diffuse, intensity already folded in.
    pub diffuse: [f32; 3],
}

/// The `<Model>` ctor's light (`0x76c8e0`, `0x71b4a0`): white, and switched off.
impl Default for ModelLight {
    fn default() -> Self {
        Self {
            enabled: false,
            omni: true,
            vector: [0.0; 3],
            ambient: [1.0; 3],
            diffuse: [1.0; 3],
        }
    }
}

/// A fresh pane as the `CSimpleModel` ctor leaves it.
impl Default for ModelState {
    fn default() -> Self {
        Self {
            path: None,
            unit: None,
            sequence: 0,
            clock_ms: 0,
            armed: None,
            pending_seed: false,
            icon: None,
            implicit_size: false,
            facing: 0.0,
            scale: 1.0,
            camera_pending: Some(0),
            camera: None,
            position: (0.0, 0.0, 0.0),
            light: ModelLight::default(),
            fog_color: 0xffff_ffff,
            fog: false,
            fog_near: 0.0,
            fog_far: 1.0,
        }
    }
}

/// What bone slot 0 plays, as the `0x7121a0` arm leaves it: the id and the anchor the cursor
/// counts from, with no counter of its own, so a caller can scrub the pane every paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmedSequence {
    /// The `AnimationData.dbc` id: `SetSequence`'s argument, or the loader's Stand seed.
    pub anim_id: i32,
    /// The clock value the cursor counts from: `SetSequenceTime(id, ms)` at `c` stores `c − ms`.
    pub anchor_ms: i64,
    /// The clock at the arm, so an arm queued behind the load replays with its offset.
    pub armed_at_ms: u64,
    /// `OnAnimFinished` has fired for this arm; it fires once per arm.
    pub finished: bool,
}

/// The playing sequence's cursor, as the renderer samples it ([`ModelState::play_head`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPlayHead {
    /// The armed sequence's `AnimationData.dbc` id, one the file owns.
    pub anim_id: u16,
    /// Milliseconds in: wrapped when looping, held at the end when clamped.
    pub cursor_ms: u32,
}

/// One sequence of a model file, as the pane clock needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceFacts {
    /// The `AnimationData.dbc` id (`M2Sequence+0x00`).
    pub anim_id: u16,
    /// The sequence's length, `end − start` on the file's timeline (`+0x08 − +0x04`).
    pub duration_ms: u32,
    /// Loops; a clamped sequence holds its last frame.
    pub looping: bool,
}

/// What a pane needs from its model file, which the reference reads off the resident `MD20`; the
/// host hands it over (`UiScript::set_model_facts`), keyed by [`model_key`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModelFileFacts {
    /// The file's sequences in file order; the first is the seed when id 0 is missing.
    pub sequences: Vec<SequenceFacts>,
    /// The header bounding box `(min, max)` in model space.
    pub bbox: ([f32; 3], [f32; 3]),
    /// The camera table's size (`MD20+0x124`), `SetCamera`'s bound (`0x76cec0`); 0 for most models.
    pub cameras: u32,
}

impl ModelFileFacts {
    /// Whether the file owns `anim_id`: `0x711960`'s `animationLookup` test.
    pub fn owns(&self, anim_id: u16) -> bool {
        self.sequences.iter().any(|s| s.anim_id == anim_id)
    }

    /// The sequence `SetSequence(anim_id)` plays: the id's first slot, variation 0 (`0x7121a0`).
    pub fn sequence(&self, anim_id: u16) -> Option<&SequenceFacts> {
        self.sequences.iter().find(|s| s.anim_id == anim_id)
    }

    /// The box's `(x, y)` extent, a size-less pane's implicit rect (`0x76d080`/`0x76d0d0`).
    pub fn extent(&self) -> (f32, f32) {
        let (min, max) = self.bbox;
        ((max[0] - min[0]).max(0.0), (max[1] - min[1]).max(0.0))
    }

    /// The camera raw index `idx` installs; `None`, the NULL camera, out of range or negative.
    pub fn camera_at(&self, idx: i32) -> Option<u32> {
        u32::try_from(idx).ok().filter(|&i| i < self.cameras)
    }

    /// The loader's idle seed (`0x70ebd0`): Stand (id 0) if owned, else the first sequence's id.
    pub fn stand_id(&self) -> Option<u16> {
        if self.owns(0) {
            Some(0)
        } else {
            self.sequences.first().map(|s| s.anim_id)
        }
    }
}

/// A model path's key: case-folded, forward slashes, no extension, so `.mdx` and `.m2` match.
pub fn model_key(path: &str) -> String {
    let p = path.to_ascii_lowercase().replace('\\', "/");
    let stem = p
        .strip_suffix(".mdx")
        .or_else(|| p.strip_suffix(".mdl"))
        .or_else(|| p.strip_suffix(".m2"))
        .unwrap_or(&p);
    stem.to_string()
}

impl ModelState {
    /// `SetModel` (`0x76cce0`): a fresh instance, dropping unit, icon override and arm, seeded now
    /// if the file is resident, else when it lands (`0x70ebd0`); the scene clock runs on.
    pub fn set_file(&mut self, path: String, facts: Option<&ModelFileFacts>) {
        self.path = Some(path);
        self.unit = None;
        self.icon = None;
        self.armed = None;
        self.pending_seed = true;
        if let Some(facts) = facts {
            self.seed_from_facts(facts);
        }
    }

    /// `ClearModel`: releases the instance.
    pub fn clear_file(&mut self) {
        self.path = None;
        self.unit = None;
        self.icon = None;
        self.armed = None;
        self.pending_seed = false;
    }

    /// `SetSequence`/`SetSequenceTime` (`0x7121a0`): interrupts what plays, with no
    /// `OnAnimFinished` (`0x76cdc6`), then arms `id` at `ms` unless the known file lacks it.
    pub fn arm(&mut self, id: i32, ms: i64, facts: Option<&ModelFileFacts>) {
        self.sequence = id;
        let owned = u16::try_from(id)
            .ok()
            .is_some_and(|id| facts.is_none_or(|f| f.owns(id)));
        self.armed = owned.then(|| ArmedSequence {
            anim_id: id,
            anchor_ms: self.clock_ms as i64 - ms,
            armed_at_ms: self.clock_ms,
            finished: false,
        });
    }

    /// The loader's completion (`0x70ebd0`): arms Stand, then replays an arm queued during the
    /// load at its offset, which decides the result. A later call only re-checks ownership.
    pub fn seed_from_facts(&mut self, facts: &ModelFileFacts) {
        let queued = self.armed.filter(|_| self.pending_seed);
        if self.pending_seed {
            self.pending_seed = false;
            self.armed = facts.stand_id().map(|id| ArmedSequence {
                anim_id: i32::from(id),
                anchor_ms: self.clock_ms as i64,
                armed_at_ms: self.clock_ms,
                finished: false,
            });
        }
        if let Some(q) = queued {
            let offset = q.armed_at_ms as i64 - q.anchor_ms;
            self.arm(q.anim_id, offset, Some(facts));
        } else if let Some(armed) = self.armed {
            if !u16::try_from(armed.anim_id).is_ok_and(|id| facts.owns(id)) {
                self.armed = None;
            }
        }
        // The model-ready hook's other half (`0x76ce00`): the pending camera, 0 from the ctor.
        if let Some(idx) = self.camera_pending {
            self.install_camera(idx, Some(facts));
        }
    }

    /// The pane's fog when armed (`+0x3a4` bit 0), the fill callback's own gate.
    pub fn armed_fog(&self) -> Option<ModelFog> {
        self.fog.then_some(ModelFog {
            color: self.fog_color,
            near: self.fog_near,
            far: self.fog_far,
        })
    }

    /// `SetCamera` by raw index (`0x76cec0`): deferred until the facts are known, then installed
    /// or answered with the NULL camera, either clearing the pending index (`0x76ce80`).
    pub fn install_camera(&mut self, idx: i32, facts: Option<&ModelFileFacts>) {
        match facts {
            Some(f) => {
                self.camera = f.camera_at(idx);
                self.camera_pending = None;
            }
            None => self.camera_pending = Some(idx),
        }
    }

    /// Where the armed sequence stands on the scene clock under `facts`.
    pub fn play_head(&self, facts: &ModelFileFacts) -> Option<ModelPlayHead> {
        let armed = self.armed?;
        let anim_id = u16::try_from(armed.anim_id).ok()?;
        let seq = facts.sequence(anim_id)?;
        let raw = (self.clock_ms as i64 - armed.anchor_ms).max(0) as u64;
        let dur = u64::from(seq.duration_ms);
        let cursor = if dur == 0 {
            0
        } else if seq.looping {
            raw % dur
        } else {
            raw.min(dur)
        };
        Some(ModelPlayHead {
            anim_id,
            cursor_ms: cursor as u32,
        })
    }

    /// The armed sequence ran its length and `OnAnimFinished` (`0x76cdc0`) is owed, once per arm
    /// and for a loop too: `0x719370` enqueues it before testing the loop flag.
    pub fn completion_due(&self, facts: &ModelFileFacts) -> bool {
        let Some(armed) = self.armed else {
            return false;
        };
        if armed.finished {
            return false;
        }
        let Some(seq) = u16::try_from(armed.anim_id)
            .ok()
            .and_then(|id| facts.sequence(id))
        else {
            return false;
        };
        (self.clock_ms as i64 - armed.anchor_ms) >= i64::from(seq.duration_ms)
    }
}

/// The Minimap widget's state. The client keeps two zoom indices, outdoor (`0x86f698`, CVar
/// `minimapZoom`) and indoor (`0x86f69c`, CVar `minimapInsideZoom`, radii `{150,120,90,60,40,25}`
/// yd); the WMO-interior flag `0xceaa60` picks which one `GetZoom`/`SetZoom` use.
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapState {
    /// The outdoor zoom index, 0 widest to 5 tightest; `SetZoom` clamps at 5 (`0x6daa10`).
    pub zoom: u8,
    pub inside_zoom: u8,
    /// Inside a WMO interior, which picks the index; pushed by the app, which owns that test.
    pub inside: bool,
    /// `Minimap:SetMaskTexture`'s mask, write-only from Lua; `None` is [`MINIMAP_DEFAULT_MASK`].
    pub mask_texture: Option<String>,
    /// The player-arrow `Model` (`[Minimap+0x338]`), reached by `SetPlayerFacing` (`0x4eb8e0`).
    pub player_arrow: Option<FrameHandle>,
}

/// Both zoom CVars register with `"3"` (`0x48fc5a`/`0x48fc76`) and the reset copies the CVar in;
/// a higher index zooms in, 3 being a 60 yd indoor radius and a 133.3 yd outdoor half-extent.
pub const MINIMAP_DEFAULT_ZOOM: u8 = 3;

impl Default for MinimapState {
    fn default() -> Self {
        Self {
            zoom: MINIMAP_DEFAULT_ZOOM,
            inside_zoom: MINIMAP_DEFAULT_ZOOM,
            inside: false,
            mask_texture: None,
            // Set by `WidgetArena::create` on insert, as the ctor builds its children.
            player_arrow: None,
        }
    }
}

impl MinimapState {
    /// The index `GetZoom`/`SetZoom` act on now.
    pub fn active_zoom(&self) -> u8 {
        if self.inside {
            self.inside_zoom
        } else {
            self.zoom
        }
    }

    /// Writes the index the inside flag selects.
    pub fn set_active_zoom(&mut self, zoom: u8) {
        if self.inside {
            self.inside_zoom = zoom;
        } else {
            self.zoom = zoom;
        }
    }
}

/// The zoom-level count (`GetZoomLevels`, `0x6da9a0`).
pub const MINIMAP_ZOOM_LEVELS: u8 = 6;

/// The engine's circular minimap mask, which `Minimap:SetMaskTexture` replaces.
pub const MINIMAP_DEFAULT_MASK: &str = "Textures\\MinimapMask";

/// The `Model` children the `CMinimap` ctor (`0x4edbc0`) makes before its XML ones, even for an
/// unloadable file (`0x4ee286`): five rim and party arrows, three POI arrows, the player arrow;
/// addons (Questie, pfQuest) read `({Minimap:GetChildren()})[9]` as the player arrow.
pub const MINIMAP_ENGINE_CHILDREN: usize = 9;

/// The default `minimapArrowModel` (`0x84c768`), applied to engine children 1 to 8 (`0x4ee170`).
pub const MINIMAP_DEFAULT_ARROW_MODEL: &str = "Interface\\Minimap\\Rotating-MinimapArrow.mdx";

/// The default `minimapPlayerModel` (`0x8453c0`), applied to engine child 9 alone (`0x4ee260`).
pub const MINIMAP_DEFAULT_PLAYER_MODEL: &str = "Interface\\Minimap\\MinimapArrow.mdx";

/// A `CSimpleScrollFrame`'s members (`+0x318`/`+0x324`/`+0x328`); the ranges are measured live.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollFrameState {
    /// `SetScrollChild`'s frame, which pans within this frame's rect.
    pub child: Option<FrameHandle>,
    /// `SetHorizontalScroll`'s offset in px, unclamped; `0x787100` anchors the child at
    /// `(+horizontal, +vertical)`, so scrolling right takes a negative offset, down a positive.
    pub horizontal: f32,
    /// `SetVerticalScroll`'s offset in px (`0x786db0`), unclamped: a positive one lifts the child
    /// to show what is below; the FrameXML scroll bar's range keeps it in bounds.
    pub vertical: f32,
}

/// `Button:SetFont`'s record, one for the client's three embedded fonts (`+0x33c`, `+0x434`,
/// `+0x3b8`), since `SetFont` (`0x780880`) sets all three alike and no Lua call parts them.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonFont {
    pub path: String,
    /// The font height in logical px.
    pub height: f32,
    /// The normalized outline, `""`, `"OUTLINE"` or `"THICKOUTLINE"`, as `GetFont` answers it; an
    /// omitted `flags` clears it, where the reference's handling is untraced.
    pub flags: String,
}

/// A button's state, `[CSimpleButton+0x328]`: 0 DISABLED, 1 NORMAL, 2 PUSHED, also the index into
/// its texture array `+0x4b8`. Past the ctor, `SetState` (`0x779790`) is its only writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVisualState {
    /// `Disable()`: the button fires no clicks and draws from the Disabled slot.
    Disabled,
    /// The resting state a button is born in.
    #[default]
    Normal,
    /// A mouse press captured over the button, or `SetButtonState("PUSHED")`.
    Pushed,
}

/// A button's state. The drawn state texture is latched, not derived: the shown pointer
/// (`+0x4c4`) moves only on a transition into a state that has a texture ([`Self::set_state`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonState {
    /// The lock (`+0x32c`) of `SetButtonState(state, locked)`: while set, the hide, press, release
    /// and drag edges skip the button, and an `Enable`/`Disable` that runs (`0x779160`) clears it.
    pub locked: bool,
    /// [`FrameKind::LootButton`]'s 0-based slot (`+0x4dc`) from `SetSlot`. Deviation: `None`
    /// until then, not the ctor's 0, so that an unset row takes nothing rather than slot 0.
    pub loot_slot: Option<u32>,
    /// A CheckButton's checked flag (`+0x4dc`, XML `checked`).
    pub checked: bool,
    /// `<NormalTexture>`/`SetNormalTexture` (`+0x4bc`).
    pub normal: Option<RegionHandle>,
    /// `<PushedTexture>` (`+0x4c0`); a button with none keeps what it showed ([`Self::set_state`]).
    pub pushed: Option<RegionHandle>,
    /// `<DisabledTexture>` (`+0x4b8`).
    pub disabled: Option<RegionHandle>,
    /// `<HighlightTexture>` (`+0x4c8`), drawn over the state texture while hovered.
    pub highlight: Option<RegionHandle>,
    /// `<CheckedTexture>` (`+0x4e0`), drawn while checked; `0x7854c0` shows
    /// [`Self::disabled_checked`] instead when disabled and it exists.
    pub checked_tex: Option<RegionHandle>,
    /// `<DisabledCheckedTexture>` (`+0x4e4`), which replaces the checked texture when disabled.
    pub disabled_checked: Option<RegionHandle>,
    /// The `<ButtonText>` fontstring (`+0x338`, `SetText`), always drawn.
    pub text: Option<RegionHandle>,
    /// `RegisterForClicks`' names, the transitions that reach `OnClick`; default `"LeftButtonUp"`.
    pub registered_clicks: HashSet<String>,
    /// The normal state's label font object (`<NormalFont>`); `None` keeps the label's own paint.
    pub normal_font: Option<String>,
    /// [`ButtonState::normal_font`] when highlighted; `None` keeps the normal one, colour and all.
    pub highlight_font: Option<String>,
    /// [`ButtonState::normal_font`] for the disabled state.
    pub disabled_font: Option<String>,
    /// `<NormalFont justifyH=>`, local to the normal font (`+0x33c`) and kept over
    /// `SetTextFontObject`; `SetFontString` (`0x778d20`) anchors an unanchored label by it.
    pub normal_justify_h: Option<crate::script::JustifyH>,
    /// `<HighlightFont justifyH=>` (`+0x3b8`); paint only, the label anchor reads the normal one.
    pub highlight_justify_h: Option<crate::script::JustifyH>,
    /// `<DisabledFont justifyH=>` (`+0x434`); paint only.
    pub disabled_justify_h: Option<crate::script::JustifyH>,
    /// `Button:SetFont`'s face, kept on the button since `SetFont` never touches the label
    /// (`+0x338`) and so creates none; extract applies it to whatever label there is.
    pub font: Option<ButtonFont>,
    /// `SetTextColor`'s label colour for the normal state, painted over the state font's own.
    pub normal_color: Option<[f32; 4]>,
    /// [`ButtonState::normal_color`] for the highlighted state, which never falls back to it.
    pub highlight_color: Option<[f32; 4]>,
    /// [`ButtonState::normal_color`] for the disabled state.
    pub disabled_color: Option<[f32; 4]>,
    /// `LockHighlight()`: highlighted regardless of hover, both the texture and the label font.
    pub locked_highlight: bool,
    /// The shown state texture (`+0x4c4`), written only by `set_state` and `set_state_slot`.
    shown: Option<RegionHandle>,
    /// The state (`+0x328`), a latch moved only by edges: the enter and leave notifies
    /// (`0x779490`/`0x7794e0`) never write it, so leaving a held button does not un-press it.
    state: ButtonVisualState,
}

impl Default for ButtonState {
    fn default() -> Self {
        ButtonState {
            loot_slot: None,
            locked: false,
            checked: false,
            normal: None,
            pushed: None,
            disabled: None,
            highlight: None,
            checked_tex: None,
            disabled_checked: None,
            text: None,
            registered_clicks: HashSet::from(["LeftButtonUp".to_string()]),
            normal_font: None,
            highlight_font: None,
            disabled_font: None,
            normal_justify_h: None,
            highlight_justify_h: None,
            disabled_justify_h: None,
            font: None,
            normal_color: None,
            highlight_color: None,
            disabled_color: None,
            locked_highlight: false,
            shown: None,
            state: ButtonVisualState::Normal,
        }
    }
}

impl ButtonState {
    /// The slot a state draws from, the client's `[this + state*4 + 0x4b8]`.
    fn state_slot(&self, state: ButtonVisualState) -> Option<RegionHandle> {
        match state {
            ButtonVisualState::Disabled => self.disabled,
            ButtonVisualState::Normal => self.normal,
            ButtonVisualState::Pushed => self.pushed,
        }
    }

    /// `SetState` (`0x779790`): entering a state with no texture keeps the shown one
    /// (`0x7797be`/`0x7797e2`). A button is NORMAL before `LoadXML` adds its art (ctor
    /// `0x7786a0`), so a press or `Disable()` without its own texture keeps the normal art.
    fn set_state(&mut self, new: ButtonVisualState) {
        if new == self.state {
            return;
        }
        self.state = new;
        if let Some(slot) = self.state_slot(new) {
            self.shown = Some(slot);
        }
    }

    /// `SetButtonState(state, locked)` (`0x779790`), the one writer: the lock is stored before the
    /// equal-state early-out (`0x77979d`), so it is set or cleared even when the state holds.
    pub fn set_button_state(&mut self, new: ButtonVisualState, locked: bool) {
        self.locked = locked;
        self.set_state(new);
    }

    /// `IsEnabled` (`0x7800b0`): the state alone; the class has no enabled flag.
    pub fn enabled(&self) -> bool {
        self.state != ButtonVisualState::Disabled
    }

    /// `GetButtonState` (`0x780180`): the latched state, never re-derived from the cursor.
    pub fn button_state(&self) -> ButtonVisualState {
        self.state
    }

    // ── The engine's edges ──
    // Six of `0x779790`'s seven call sites, all passing `locked = 0`; the seventh is Lua's.

    /// `SetEnabled` (`0x779160`): a no-op if nothing changes (`0x779175`/`0x77919c`), so enabling a
    /// pushed button neither un-pushes nor unlocks it; returns whether it ran.
    pub fn set_enabled(&mut self, on: bool) -> bool {
        if on == self.enabled() {
            return false;
        }
        self.set_button_state(
            if on {
                ButtonVisualState::Normal
            } else {
                ButtonVisualState::Disabled
            },
            false,
        );
        true
    }

    /// The hide edge (`0x7791e0`): an unlocked, enabled button goes NORMAL; `<OnHide>` still fires.
    pub fn on_hide(&mut self) {
        if self.state != ButtonVisualState::Disabled && !self.locked {
            self.set_button_state(ButtonVisualState::Normal, false);
        }
    }

    /// The press edge (`0x779210`), past the registration and hit gates, so a registered click
    /// pushes the button even when the click itself does nothing.
    pub fn on_mouse_down(&mut self) {
        if !self.locked && self.state != ButtonVisualState::Disabled {
            self.set_button_state(ButtonVisualState::Pushed, false);
        }
    }

    /// The release edge (`0x7792d0`): a pushed, unlocked button goes NORMAL wherever the cursor
    /// is, even one Lua pushed. The caller skips it when the release ends a drag (`0x7792df`).
    pub fn on_mouse_up(&mut self) {
        if !self.locked && self.state == ButtonVisualState::Pushed {
            self.set_button_state(ButtonVisualState::Normal, false);
        }
    }

    /// The drag-start edge (`0x7793f0`), before `<OnDragStart>`: it fires at the drag threshold
    /// (0.01 frame units, `0x81c468`) on a `RegisterForDrag` frame, not at the rect edge.
    pub fn on_drag_start(&mut self) {
        if !self.locked && self.state != ButtonVisualState::Disabled {
            self.set_button_state(ButtonVisualState::Normal, false);
        }
    }

    /// `SetNormalTexture` and kin (`0x778fd0`): stores the slot, shown only if it is the current
    /// state's (`0x779027`). The reference destroys a displaced texture (`0x77900a`); here the
    /// region is made once and repainted, so a held `GetNormalTexture()` stays live.
    pub fn set_state_slot(&mut self, state: ButtonVisualState, rh: Option<RegionHandle>) {
        match state {
            ButtonVisualState::Disabled => self.disabled = rh,
            ButtonVisualState::Normal => self.normal = rh,
            ButtonVisualState::Pushed => self.pushed = rh,
        }
        if state == self.state {
            self.shown = rh;
        }
    }

    /// Whether a region of this button draws: a state texture only when shown, the highlight
    /// while hovered or locked, the checked pair by their rule, anything else always.
    pub fn region_visible(&self, rh: RegionHandle, hovered: bool) -> bool {
        let some = Some(rh);
        if some == self.normal || some == self.pushed || some == self.disabled {
            return some == self.shown;
        }
        if some == self.highlight {
            // No `enabled` term: `Disable()` turns the whole HIGHLIGHT layer off before this.
            return hovered || self.locked_highlight;
        }
        if some == self.checked_tex {
            return self.checked && (self.enabled() || self.disabled_checked.is_none());
        }
        if some == self.disabled_checked {
            return self.checked && !self.enabled();
        }
        true
    }
}

/// A StatusBar's value and fill texture, zero-initialized like the client's members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusBarState {
    /// `SetMinMaxValues` low bound (XML `minValue`); the loader swaps a reversed pair (`0x782ef0`).
    pub min: f32,
    /// `SetMinMaxValues` high bound (XML `maxValue`).
    pub max: f32,
    /// The current value (`SetValue`/XML `defaultValue`), clamped into `[min, max]`.
    pub value: f32,
    /// VERTICAL fills bottom-up; HORIZONTAL, the default, left to right (enum `0x811b00`).
    pub vertical: bool,
    /// The fill texture (`<BarTexture>`), cropped to [`Self::fraction`] along the orientation axis.
    pub bar: Option<RegionHandle>,
}

impl StatusBarState {
    /// The fill fraction `(value − min) / (max − min)` in `[0, 1]`; `0.0` when `max <= min`.
    pub fn fraction(&self) -> f32 {
        if self.max > self.min {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A `CSimpleSlider`'s state. The default orientation is VERTICAL, unlike a StatusBar's: the
/// stock `UIPanelScrollBarTemplate` declares none and is vertical.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderState {
    /// `SetMinMaxValues` low bound (XML `minValue`); a reversed pair is not swapped (`0x789580`).
    pub min: f32,
    /// `SetMinMaxValues` high bound (XML `maxValue`).
    pub max: f32,
    /// The stored value (`+0x320`), zero-initialised and readable before any `SetValue`.
    pub value: f32,
    /// `+0x314` bit 1, set by `SetMinMaxValues` (`0x7898f0`); until then `SetValue` is a no-op.
    pub range_valid: bool,
    /// `+0x314` bit 2, set by the first `SetValue`, which always fires `OnValueChanged`;
    /// `SetMinMaxValues` re-clamps the value only once this is set.
    pub has_value: bool,
    /// `SetValueStep` (`+0x324`): every stored value is quantised onto `min + n·step`
    /// ([`slider_set_value`]); 0, the default the scrollbars keep, skips the quantiser.
    pub step: f32,
    /// VERTICAL, the default, puts `min` at the top of the track (enum `0x811b00`).
    pub vertical: bool,
    /// The thumb texture (`<ThumbTexture>`), placed at [`Self::fraction`] along the track.
    pub thumb: Option<RegionHandle>,
}

impl Default for SliderState {
    fn default() -> SliderState {
        SliderState {
            min: 0.0,
            max: 0.0,
            value: 0.0,
            range_valid: false,
            has_value: false,
            step: 0.0,
            vertical: true,
            thumb: None,
        }
    }
}

/// Where a press grabs the thumb: the offset from its leading edge that stays under the cursor,
/// with distances from the track's leading edge so every UI surface shares it. Off the thumb it
/// is the center, warping the value under the cursor. Deviation: on the thumb it is the grabbed
/// point, where `CSimpleSlider` (`0x789ba0`/`0x789ca0`) always centers it, because that feels
/// less surprising and a stock-sized thumb hides the difference.
pub fn slider_grab(cursor: f32, thumb_lead: f32, thumb_extent: f32) -> f32 {
    if cursor >= thumb_lead && cursor <= thumb_lead + thumb_extent {
        cursor - thumb_lead
    } else {
        thumb_extent * 0.5
    }
}

/// The fraction of travel that puts the thumb's leading edge at `cursor − grab`, clamped to
/// `[0, 1]` as `SetValue` clamps; `None` when the thumb fills the track.
pub fn slider_fraction(
    cursor: f32,
    grab: f32,
    track_extent: f32,
    thumb_extent: f32,
) -> Option<f32> {
    let travel = track_extent - thumb_extent;
    (travel > 0.0).then(|| ((cursor - grab) / travel).clamp(0.0, 1.0))
}

/// `CSimpleSlider::SetValue`'s arithmetic (`0x789930`), op for op. The client holds the range as
/// `min` and `span` (`0x7898f0` stores `end − start`). Clamp into `[min, span + min]`, a
/// degenerate range pinning to `span + min`; then, unless `step` is 0, round half away from zero
/// onto the `min + n·step` lattice. The result is not re-clamped, so an uneven range can overshoot
/// `max` by up to half a step. The f64 intermediates are the x87 registers, each `as f32` a spill.
pub fn slider_set_value(v: f32, min: f32, span: f32, step: f32) -> f32 {
    let minv = f64::from(min);
    let vv = f64::from(v);

    // Clamp low: `fcomp(min, v)` takes `min` only on `min > v` ordered, so a NaN `v` passes.
    let clamp_low = |v: f64| {
        if matches!(minv.partial_cmp(&v), Some(core::cmp::Ordering::Greater)) {
            minv
        } else {
            v
        }
    };
    let lo = clamp_low(vv);

    // `bound = span + min`, spilled to f32 at `[ebp-4]`.
    let bound = f64::from(span) + minv;
    let mut hi = if matches!(bound.partial_cmp(&lo), Some(core::cmp::Ordering::Less)) {
        f64::from(bound as f32) // `fld [ebp-4]`: the f32 reload, not the live value
    } else {
        clamp_low(vv)
    };

    // The step test is `fcomp(step, 0.0)`: exactly zero (either sign) skips the quantiser.
    if f64::from(step) != 0.0 {
        let stepv = f64::from(step);
        let diff = hi - minv;
        let halfstep = stepv * 0.5;
        let scaled = if diff > 0.0 {
            (halfstep + diff) / stepv
        } else {
            (diff - halfstep) / stepv
        };
        let n = scaled.trunc() as i32; // `__ftol`, truncate toward zero
        hi = (f64::from(n) * stepv) + minv;
    }
    hi as f32
}

impl SliderState {
    /// The value fraction in `[0, 1]`; `0.0` when `max <= min`, the thumb at the track's start.
    pub fn fraction(&self) -> f32 {
        if self.max > self.min {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// `SetValue` (`0x789930`): stores the settled value, returning it when `OnValueChanged` fires:
    /// never without a range, always the first time, else only on a change, which is what ends the
    /// scrollbar's `SetVerticalScroll` loop. The span is derived as `0x7898f0` computes it.
    pub fn store_value(&mut self, v: f32) -> Option<f32> {
        if !self.range_valid {
            return None;
        }
        let span = (f64::from(self.max) - f64::from(self.min)) as f32;
        let settled = slider_set_value(v, self.min, span, self.step);
        let first = !self.has_value;
        self.has_value = true;
        (first || settled != self.value).then(|| {
            self.value = settled;
            settled
        })
    }

    /// `SetValueStep` (`0x789a60`): stores the step, then with a range re-runs `SetMinMaxValues`
    /// with the max re-derived as `f32(span + min)` (`0x789a91`), which can re-quantise the value,
    /// fire `OnValueChanged` and even change the range, since that sum rounds.
    pub fn set_value_step(&mut self, step: f32) -> Option<f32> {
        self.step = step;
        if !self.range_valid {
            return None;
        }
        let span = (f64::from(self.max) - f64::from(self.min)) as f32;
        let max = (f64::from(span) + f64::from(self.min)) as f32;
        self.set_min_max(self.min, max)
    }

    /// `SetMinMaxValues` (`0x7898f0`): sets the range; re-settles the value only once one exists.
    pub fn set_min_max(&mut self, min: f32, max: f32) -> Option<f32> {
        (self.min, self.max) = (min, max);
        self.range_valid = true;
        if self.has_value {
            self.store_value(self.value)
        } else {
            None
        }
    }
}

/// A `ColorSelect`'s colour (ctor `0x78b220`): HSV `f32`s, hue in degrees, `-1` for a grey
/// (`0x7bbccd`). RGB enters rounded half up (quantizer A) and leaves floored (B), so a
/// `GetColorRGB`/`SetColorRGB` round trip drops a channel by 1 for about 9.75 % of colours, and
/// repeating it ratchets; that drift is the reference's. `SetColorRGB` discards a fifth argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorSelectState {
    /// `[hue°, saturation, value]` (`+0x328`/`+0x32c`/`+0x330`); an RGB grey has hue `-1.0`.
    pub hsv: [f32; 3],
    /// The hue disc (`<ColorWheelTexture>`, `0x78de90`, `+0x318`), its rect the wheel's hit box.
    pub wheel: Option<RegionHandle>,
    /// The disc's marker (`<ColorWheelThumbTexture>`, `0x78e160`), placed from `hsv` at extract;
    /// an XML anchor on it is discarded (`0x78b850` re-points it on every colour change).
    pub wheel_thumb: Option<RegionHandle>,
    /// The brightness strip (`<ColorValueTexture>`, `0x78e450`, `+0x320`), the second hit box.
    pub value_strip: Option<RegionHandle>,
    /// The strip's marker (`<ColorValueThumbTexture>`, `0x78e720`), placed from `hsv[2]`.
    pub value_thumb: Option<RegionHandle>,
}

impl Default for ColorSelectState {
    /// The ctor's white: H=0, S=0, V=1 (`0x78b27e`/`0x78b298`/`0x78b28e`).
    fn default() -> ColorSelectState {
        ColorSelectState {
            hsv: [0.0, 0.0, 1.0],
            wheel: None,
            wheel_thumb: None,
            value_strip: None,
            value_thumb: None,
        }
    }
}

impl ColorSelectState {
    /// Quantizer A, `SetColorRGB`'s inbound leg (`0x78ec7d`): clamp to `[0, 1]`, `·255 + 0.5`,
    /// truncate (`__ftol`, `0x40a2b0`), so round half up, in `f64` as the x87 registers run it.
    pub fn quantize_a(v: f64) -> u8 {
        let clamped = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        // In-range the product is `[0.5, 255.5]`, so the truncation is always a valid byte.
        (clamped * 255.0 + 0.5) as u8
    }

    /// Quantizer B, every outbound path (`0x7bbec0`): `v·255 + 512` as `f32` has a `2^-14` ulp, so
    /// `>> 14` reads the floor of `v·255`. Unclamped: out of range wraps mod 256.
    pub fn quantize_b(v: f32) -> u8 {
        let c = (f64::from(v) * 255.0 + 512.0) as f32;
        ((c.to_bits() >> 14) & 0xff) as u8
    }

    /// `0x7bbf20`'s unpack: the byte times the `f32` `0x3b808081`, 1/255 rounded up.
    fn unpack_byte(b: u8) -> f32 {
        let k = f32::from_bits(0x3b80_8081);
        (f64::from(i32::from(b)) * f64::from(k)) as f32
    }

    /// The Lua-facing normalize: the byte times the `f64` `1/255` at `0x804578`, never a divide.
    pub fn normalize(byte: u8) -> f64 {
        f64::from(byte) * (1.0_f64 / 255.0)
    }

    /// `0x7bf680`: the index of the largest `|component|`, a tie going to the later index.
    fn dominant_axis(v: &[f32; 3]) -> usize {
        let (a0, a1, a2) = (v[0].abs(), v[1].abs(), v[2].abs());
        if a0 > a1 {
            if a0 > a2 {
                0
            } else {
                2
            }
        } else if a1 > a2 {
            1
        } else {
            2
        }
    }

    /// `0x7bf700`: the index of the smallest `|component|`.
    fn minor_axis(v: &[f32; 3]) -> usize {
        let (a0, a1, a2) = (v[0].abs(), v[1].abs(), v[2].abs());
        if a0 >= a1 {
            if a2 < a1 {
                2
            } else {
                1
            }
        } else if a0 >= a2 {
            2
        } else {
            0
        }
    }

    /// `0x7bbc80`, RGB to HSV: each store rounds to `f32`; the chroma divide runs in `f64`.
    fn rgb_to_hsv(rgb: &[f32; 3]) -> [f32; 3] {
        let f = f64::from;
        let dom = Self::dominant_axis(rgb);
        let minor = Self::minor_axis(rgb);
        let value = rgb[dom];
        let sat = if value == 0.0 {
            0.0
        } else {
            ((f(value) - f(rgb[minor])) / f(value)) as f32
        };
        let hue = if sat == 0.0 {
            -1.0
        } else {
            let chroma = f(value) - f(rgb[minor]);
            let sector = match dom {
                0 => ((f(rgb[1]) - f(rgb[2])) / chroma) as f32,
                1 => ((f(rgb[2]) - f(rgb[0])) / chroma + 2.0) as f32,
                _ => ((f(rgb[0]) - f(rgb[1])) / chroma + 4.0) as f32,
            };
            let hue_deg = (f(sector) * 60.0) as f32;
            if hue_deg < 0.0 {
                (f(hue_deg) + 360.0) as f32
            } else {
                hue_deg
            }
        };
        [hue, sat, value]
    }

    /// `0x7bbd60`, HSV to RGB: a grey never reads hue; the sector is floored by quantizer B's
    /// trick, and the `f32` 1/60 (`0x3c888889`) is where the round trip's drift comes from.
    pub fn hsv_to_rgb(hsv: &[f32; 3]) -> [f32; 3] {
        let f = f64::from;
        let (h, s, v) = (hsv[0], hsv[1], hsv[2]);
        if s == 0.0 {
            return [v, v, v];
        }
        let hue = if h == 360.0 { 0.0 } else { h };
        let inv60 = f32::from_bits(0x3c88_8889);
        let sector_float = f(hue) * f(inv60); // an un-rounded f64 register
        let magic = (sector_float + 512.0) as f32;
        let raw = (magic.to_bits() >> 14) & 0xff;
        let sector = if raw <= 5 { raw } else { 5 };
        let frac = (sector_float - f64::from(sector as i32)) as f32;
        let p = ((1.0 - f(s)) * f(v)) as f32;
        let q = ((1.0 - f(frac) * f(s)) * f(v)) as f32;
        let t = ((1.0 - (1.0 - f(frac)) * f(s)) * f(v)) as f32;
        match sector {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            _ if sector == 4 => [t, p, v],
            _ => [v, p, q],
        }
    }

    /// Stores HSV raw, as the drag handler (`0x78bd80`) and `SetColorHSV` (`0x78e920`) do.
    pub fn set_hsv(&mut self, h: f32, s: f32, v: f32) {
        self.hsv = [h, s, v];
    }

    /// `SetColorRGB`'s store (`0x78eb7e`): quantizer A, the `0x7bbf20` unpack, RGB to HSV. It
    /// always stores, with no change gate: `0x78bafd` only tests whether a handler is bound.
    pub fn set_rgb(&mut self, r: f64, g: f64, b: f64) {
        let bytes = [
            Self::quantize_a(r),
            Self::quantize_a(g),
            Self::quantize_a(b),
        ];
        let rgb = [
            Self::unpack_byte(bytes[0]),
            Self::unpack_byte(bytes[1]),
            Self::unpack_byte(bytes[2]),
        ];
        self.hsv = Self::rgb_to_hsv(&rgb);
    }

    /// The read-back `GetColorRGB` (`0x78edda`) and the `OnColorSelect` payload (`0x78bb36`) both
    /// run, so they match bit for bit: HSV to RGB, quantizer B, then the `f64` `1/255`.
    pub fn rgb_f64(&self) -> (f64, f64, f64) {
        let rgb = Self::hsv_to_rgb(&self.hsv);
        (
            Self::normalize(Self::quantize_b(rgb[0])),
            Self::normalize(Self::quantize_b(rgb[1])),
            Self::normalize(Self::quantize_b(rgb[2])),
        )
    }
}
