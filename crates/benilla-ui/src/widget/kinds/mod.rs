//! Widget kinds: [`FrameKind`], [`RegionKind`] and the per-kind state, [`KindState`].

use std::collections::{HashSet, VecDeque};

use super::{FrameHandle, RegionHandle};

mod character;
mod editbox;
mod messageframe;
pub use character::*;
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

/// A kind's own per-frame update: the `vtable+0x38` override the update walk calls in the frame's
/// place (`0x765710`), which fires the frame's `OnUpdate` (`0x76b2c0`) first and then does the
/// kind's engine work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindUpdate {
    /// `CSimpleEditBox`'s `0x77a790`: the flush, then the caret blink.
    EditBox,
    /// `CGameTooltip`'s `0x530b20`: the fade.
    Tooltip,
    /// `CSimpleModel`'s `0x76d7f0`, in the slot of every model kind's vtable (`0x81c608`,
    /// `0x8084f8`, `0x8083a8`, `0x808260`): the scene clock and the completions it reports.
    Model,
    /// `CSimpleMessageFrame`'s `0x786200` (slot `0x81cca0`): the cap to the lines that fit, then
    /// the line fades.
    MessageFrame,
    /// `CSimpleMessageScrollFrame`'s `0x788460` (slot `0x81ce50`): the line fades.
    ScrollingMessageFrame,
    /// `CSimpleStatusBar`'s `0x7833c0`: the bar texture's coordinates, when the fill is marked.
    StatusBar,
    /// `CSimpleScrollFrame`'s `0x787140` (slot `0x81cd78`): a marked range's recompute.
    ScrollFrame,
}

impl FrameKind {
    /// A `CSimpleModel` kind, carrying a [`ModelState`].
    pub fn is_model(self) -> bool {
        matches!(
            self,
            FrameKind::Model
                | FrameKind::PlayerModel
                | FrameKind::DressUpModel
                | FrameKind::TabardModel
        )
    }

    /// The kind's own update, run in the walk after the frame's `OnUpdate`. The reference
    /// overrides the slot on more kinds; benilla models these.
    pub fn kind_update(self) -> Option<KindUpdate> {
        match self {
            FrameKind::EditBox => Some(KindUpdate::EditBox),
            FrameKind::GameTooltip => Some(KindUpdate::Tooltip),
            FrameKind::MessageFrame => Some(KindUpdate::MessageFrame),
            FrameKind::ScrollingMessageFrame => Some(KindUpdate::ScrollingMessageFrame),
            FrameKind::ScrollFrame => Some(KindUpdate::ScrollFrame),
            FrameKind::StatusBar => Some(KindUpdate::StatusBar),
            k if k.is_model() => Some(KindUpdate::Model),
            _ => None,
        }
    }
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
    /// The owner's region-removed hook, frame vtable `+0x2c`, which `0x77fd10` runs on the old
    /// parent whenever a region leaves it, freed, reparented or detached (`0x77fd55`): a kind
    /// that keeps a region in a slot drops it there, so no slot names a dead region. Button
    /// `0x778cb0`, CheckButton `0x785230`, StatusBar `0x783110`, Slider `0x789790` and ColorSelect
    /// `0x78b530` override it; every other kind keeps the base unlink `0x76a7f0` alone.
    pub fn region_removed(&mut self, rh: RegionHandle) {
        let clear = |slot: &mut Option<RegionHandle>| {
            if *slot == Some(rh) {
                *slot = None;
            }
        };
        match self {
            KindState::Button(bs) => bs.region_removed(rh),
            // The bar texture, `+0x324`.
            KindState::StatusBar(sb) => clear(&mut sb.bar),
            // The thumb, `+0x328`.
            KindState::Slider(sl) => clear(&mut sl.thumb),
            // The four art slots, `+0x318` to `+0x324`.
            KindState::ColorSelect(cs) => {
                clear(&mut cs.wheel);
                clear(&mut cs.wheel_thumb);
                clear(&mut cs.value_strip);
                clear(&mut cs.value_thumb);
            }
            _ => {}
        }
    }

    /// The display lines of either message-frame class, which share the [`MessageLine`] record.
    pub fn message_lines(&self) -> Option<&VecDeque<MessageLine>> {
        match self {
            KindState::ScrollingMessage(smf) => Some(&smf.lines),
            KindState::Message(mf) => Some(&mf.lines),
            _ => None,
        }
    }

    /// The message kinds' font holder ([`ScrollingMessageState::font_region`]).
    pub(crate) fn message_font_region_mut(
        &mut self,
    ) -> Option<&mut Option<crate::widget::RegionHandle>> {
        match self {
            KindState::ScrollingMessage(smf) => Some(&mut smf.font_region),
            KindState::Message(mf) => Some(&mut mf.font_region),
            _ => None,
        }
    }

    /// The message kinds' text insets ([`ScrollingMessageState::insets`]).
    pub fn message_insets(&self) -> Option<[f32; 4]> {
        match self {
            KindState::ScrollingMessage(smf) => Some(smf.insets),
            KindState::Message(mf) => Some(mf.insets),
            _ => None,
        }
    }

    /// [`Self::message_insets`], mutably, for the XML `<TextInsets>`.
    pub fn message_insets_mut(&mut self) -> Option<&mut [f32; 4]> {
        match self {
            KindState::ScrollingMessage(smf) => Some(&mut smf.insets),
            KindState::Message(mf) => Some(&mut mf.insets),
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
/// clear the anchors and point, 6 and 7 only clear (7 not when the sizer places, `0x530a45`), 8
/// does neither.
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

/// A `GameTooltip`'s state. Its lines are the FontString pairs named `<name>TextLeftN`/`TextRightN`
/// that its template declares, Lua globals as the stock template's 30 pairs are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TooltipState {
    /// Lines currently filled (`NumLines`). Regions past this index exist but are hidden.
    pub num_lines: usize,
    /// The left-column FontString array (`+0x32c`), index i = line i+1: the `<name>TextLeft<n>`
    /// pairs the post-load pass found (`0x529650`), then one per `AddFontStrings` (`0x529940`). Its
    /// length is the line limit (`+0x320`), `0x530270` adding no line past it (`0x530291`).
    pub left_lines: Vec<RegionHandle>,
    /// The right-column pool (the `AddDoubleLine`/`TextRightN` half), parallel to `left_lines`.
    pub right_lines: Vec<RegionHandle>,
    /// `SetOwner`'s frame, dropped on hide.
    pub owner: Option<FrameHandle>,
    /// `SetOwner`'s x and y offsets (`+0x3c4`, `+0x3c8`, stored by the core at `0x53001c`), which
    /// every anchor apply reads.
    pub owner_offset: (f32, f32),
    /// The anchor mode (`+0x318`), read back by `GetAnchorType()`: the last `SetOwner`'s while
    /// owned, `None` (mode 7) since a hide.
    pub anchor: TooltipAnchor,
    /// Each line's wrap flag (`+0x344`), parallel to `left_lines`: the line core stores it
    /// (`0x53036f`) and the clear zeroes it (`0x530236`).
    pub wrapped: Vec<bool>,
    /// `SetMinimumWidth` (`+0x3c0`): the sizer's starting widest (`0x530665`), zeroed by the clear
    /// (`0x530258`).
    pub min_width: f32,
    /// `FadeOut()`'s countdown in seconds (`[+0x3b4]` armed, `[+0x3b8]` left): the tooltip's own
    /// update counts it down by its elapsed, so it stands still while the tooltip is not walked.
    /// `FadeOut` arms it; the show, the SetOwner core (so every hide) and a cursor-mode `FadeOut`
    /// disarm it, and nothing else.
    pub fade_left: Option<f32>,
    /// The guid of the unit this tooltip shows (`+0x368`/`+0x36c`, stored at `0x52a070`), whose
    /// health watcher (`0x467e70`, registered at `0x52a0a8`) drives the bar; a `set_unit` push of
    /// that unit, under any token, re-drives it.
    pub unit_guid: Option<u64>,
    /// The unit's health as the bar was last driven with it: the watcher's copy of the field
    /// (`0x4667a0`), which a values update compares before calling it (`0x4655bb`).
    pub unit_health: u32,
    /// Armed for a compare render: the next `SetInventoryItem` prepends "Currently Equipped"
    /// (`[arg+0x18]`, not the compact `[arg+0x14]`), surviving the stock caller's `SetOwner` in
    /// between (`MerchantFrame.xml:67-72`); the reference's route to `0x52b650` is untraced.
    pub equipped_header_armed: bool,
    /// `SetPadding` (`+0x3bc`): extra plate width past the content (`0x5309f4`), which the clear
    /// keeps (`ItemRef.xml:40` sets 16 for the close button).
    pub padding: f32,
    /// Benilla-only: a sizing ran in a VM with no font engine before its lines were measured, so
    /// the next resolve runs it again ([`crate::script::tooltip`]'s headless fallback).
    pub relayout_owed: bool,
}

/// The plate's pad on each side of its content, `0x808e74` and `0x808e78` = 10.24 (`0x4123d70a`).
pub const TOOLTIP_PLATE_PAD: f32 = 10.24;
/// The gap the sizer counts between shown lines, `0x808e6c` = 2.0.
pub const TOOLTIP_LINE_GAP: f32 = 2.0;
/// The gap the sizer counts between a double line's texts, `0x808e70` = 38.4 (`0x4219999a`).
pub const TOOLTIP_COLUMN_GAP: f32 = 38.4;
/// `FadeOut`'s duration in seconds, `[0x808e7c]` = 2.0: the plate holds full alpha for the first
/// half and ramps over the second.
pub const TOOLTIP_FADE_SECS: f32 = 2.0;
/// The width a wrapped line is split at when its text is wider, `0x808e80` = 230.4 (`0x43666666`).
pub const TOOLTIP_WRAP_WIDTH: f32 = 230.4;

/// One frame's advance of a model pane's scene clock in whole milliseconds: `elapsed · 1000`
/// truncated toward zero with no `+0.5` (`0x76d846`, then `__ftol` `0x40a2b0`), where the world's
/// clock rounds (`0x483659`), so each frame's sub-millisecond remainder is dropped with no carry.
/// A negative or NaN `elapsed` advances nothing.
pub fn pane_clock_step_ms(elapsed: f32) -> u64 {
    (elapsed * 1000.0).trunc().max(0.0) as u64
}

/// A model pane's scene: the `CSimpleModel` (`0x76c8e0`) members every model kind carries, read
/// and written by `Model`'s 23 methods (table `0x878948`); the render is the app's.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelState {
    /// `SetModel`'s path as written (`.mdx`); the app's loader maps it to the shipped `.m2`.
    pub path: Option<String>,
    /// The `CGCharacterModelBase` half (`0x505680`) of a `PlayerModel`, `DressUpModel` or
    /// `TabardModel`; `None` on a plain `Model`, which has no `SetUnit`.
    pub character: Option<CharacterPane>,
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
    /// The installed camera (`+0x31c`), drawn in perspective; `None`, the NULL camera an
    /// out-of-range index installs, draws orthographic.
    pub camera: Option<InstalledCamera>,
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
    /// A `ModelFFX`, the glue's `CSimpleModelFFX` (`0x46fa00`). Its vtable differs from
    /// `CSimpleModel`'s in two slots past the destructor: its OnShow (`0x46fa60`) installs the glue
    /// glow pass (`0x6cde60` with `[0xb414c4]`) and pins `LightParams.glow` to `*0x8380b4` (0.30),
    /// and its layer-2 paint callback (`0x46fad0`, registered by `0x46fa90`) brackets the model
    /// paint `0x76d240` with the full-screen-effect pass pair `0x6cd890`/`0x6cda70`.
    pub ffx: bool,
    /// The scene's fill callback is the select screen's (`0x472150`, installed by
    /// `SetCharSelectBackground` at `0x472144`) in place of the fog callback `SetModel` installs
    /// (`0x76cd96` → `0x76d680`): the scene draws unfogged whatever the fog fields hold. The
    /// callback rides the model instance, so the next `SetModel` or `ClearModel` drops it.
    pub select_fill: bool,
    /// The number of the instance the last `SetModel` made: `0x76cce0` builds a new one on every
    /// call (`0x76ccf5`-`0x76cd0d`), its scene starting over, even for the file it held. 0 for
    /// none.
    pub instance: u64,
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

/// A model pane's embedded `CGLight` (`+0x324`), the light `0x76d680` adds to its file's own
/// (`0x718960`): off on a plain `<Model>` until Lua enables it, so a file with no lights draws
/// black (`0x71bf90`), and on for a `PlayerModel` (ctor `0x505680`). Colours are stored times
/// their intensity.
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

impl ModelLight {
    /// The `CGCharacterModelBase` ctor's light (`0x505680`), over the `<Model>` one it chains to:
    /// directional (`0x71b620(0)` at `0x5056c9`), ambient 0.7 (`+0x30`, `0x50572e`), diffuse
    /// (0.8, 0.8, 0.64) (`+0x3c`, `0x505702`) and enabled (`0x71b780(1)` at `0x50576a`). The
    /// direction is the second `0x71b6a0` write, `(0, 1, 0)` at `0x505761`, which overwrites the
    /// `(0, −√½, −√½)` of the first at `0x5056e9`. `PlayerModel`'s factory `0x495bd0` runs this
    /// ctor, and `DressUpModel` and `TabardModel` subclass it; no 1.12 FrameXML calls `SetLight`,
    /// so every stock window draws in this light.
    pub fn character() -> Self {
        Self {
            enabled: true,
            omni: false,
            vector: [0.0, 1.0, 0.0],
            ambient: [0.7; 3],
            diffuse: [0.8, 0.8, 0.64],
        }
    }
}

/// A fresh pane as the `CSimpleModel` ctor leaves it.
impl Default for ModelState {
    fn default() -> Self {
        Self {
            path: None,
            character: None,
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
            ffx: false,
            select_fill: false,
            instance: 0,
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

/// A pane's installed camera (`+0x31c`): raw record `index` of the camera table (never via
/// `cameraLookup`) of `file`, the file the pane held when the camera was resolved (`0x76ceef` →
/// `0x713590`). The pane holds the camera itself, so a later `SetModel` keeps aiming through the
/// old file's camera (`0x76cd30` leaves `+0x31c`, and the new file's ready hook installs nothing
/// while none is pending, `0x76ce44`-`0x76ce47`) until the next `SetCamera`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledCamera {
    pub file: String,
    pub index: u32,
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
    /// A fresh `PlayerModel`, `DressUpModel` or `TabardModel`: the `Model` ctor's state, then the
    /// `CGCharacterModelBase` ctor's (`0x505680`): its members zeroed and its light switched on.
    pub fn character() -> Self {
        Self {
            character: Some(CharacterPane::default()),
            light: ModelLight::character(),
            ..Self::default()
        }
    }

    /// `SetModel` (`0x76cce0`): a fresh instance, dropping icon override and arm, seeded now if the
    /// file is resident, else when it lands (`0x70ebd0`); the scene clock runs on. On a character
    /// pane the file replaces the copy at `+0x318` (slot 37), while the stored subject stays for
    /// the next build.
    pub fn set_file(&mut self, path: String, facts: Option<&ModelFileFacts>) {
        self.path = Some(path);
        if let Some(c) = &mut self.character {
            c.overtake();
        }
        self.icon = None;
        self.armed = None;
        self.pending_seed = true;
        self.select_fill = false;
        if let Some(facts) = facts {
            self.seed_from_facts(facts);
        }
    }

    /// `ClearModel`: releases the instance, a character pane's copy included (slot 36 with null),
    /// which leaves its stored subject (`+0x3e0`, `+0x3f0`) as it was.
    pub fn clear_file(&mut self) {
        self.path = None;
        if let Some(c) = &mut self.character {
            c.overtake();
        }
        self.icon = None;
        self.armed = None;
        self.pending_seed = false;
        self.select_fill = false;
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

    /// The fog the scene draws with: the armed fog under the fog callback `SetModel` installs
    /// (`0x76d680`), none under the select screen's ([`Self::select_fill`]).
    pub fn scene_fog(&self) -> Option<ModelFog> {
        if self.select_fill {
            None
        } else {
            self.armed_fog()
        }
    }

    /// `SetCamera` by raw index (`0x76cec0`): deferred until the facts are known, then installed
    /// from the file held now (`0x713590`) or answered with the NULL camera, either clearing the
    /// pending index (`0x76ce80`). The facts are always those of [`Self::path`].
    pub fn install_camera(&mut self, idx: i32, facts: Option<&ModelFileFacts>) {
        match facts {
            Some(f) => {
                self.camera = f
                    .camera_at(idx)
                    .zip(self.path.clone())
                    .map(|(index, file)| InstalledCamera { file, index });
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

/// A Minimap frame's own state: its engine children and the sizes its ctor froze. Everything else
/// the minimap shows is one process-wide set of statics ([`MinimapStatics`]), as the reference
/// keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MinimapState {
    /// The party and pet arrows (`[Minimap+0x320]`), engine children 1 to 5.
    pub group_arrows: Vec<FrameHandle>,
    /// The landmark and static-blip rim arrows (`[Minimap+0x314]`), engine children 6 to 8.
    pub rim_arrows: Vec<FrameHandle>,
    /// The player-arrow `Model` (`[Minimap+0x338]`), reached by `SetPlayerFacing` (`0x4eb8e0`).
    pub player_arrow: Option<FrameHandle>,
    /// The minimap's level plus 3 (`+0x334`, `0x4ede48`, again at `0x4ee1a3`), the rim arrows'
    /// level base.
    pub level_base: i32,
    /// The ctor's `G48 · 5/3` (`0x4edc42`, `0x4edc47`), about 1 at 4:3, every blip size's factor.
    pub base: f32,
    /// The disc, `[0xbc7970]` (`0x4edcaf`), in layout units: the arrow frames' size and the rim
    /// arrows' orbit.
    pub disc: f32,
}

/// `[0x807a44]`, the disc's factor on the unstored `G48 · 5/3` (`0x4edca9`); the disc itself is
/// [`MinimapState::disc`].
pub const MINIMAP_DISC: f32 = 0.11;

/// The arrow frames' model scale, `[0x807a28]` (`0x4edeb2`).
pub const MINIMAP_ARROW_SCALE: f32 = 0.6;

/// The minimap's statics, one set for every Minimap frame: the zoom cells, outdoor `0x86f698`
/// (CVar `minimapZoom`) and indoor `0x86f69c` (CVar `minimapInsideZoom`, radii
/// `{150,120,90,60,40,25}` yd), the indoor byte `0xceaa60` that picks between them, and the mask
/// `0xbc7968` with the icon atlases. `GetZoomLevels`, `GetZoom`, `SetZoom` and `GetPingPosition` read and write them
/// with no `this` lookup (`0x4eebe0`, `0x4eec20`, `0x4eec60`, `0x4eefd0`).
#[derive(Clone, Debug, PartialEq)]
pub struct MinimapStatics {
    /// The outdoor zoom index, 0 widest to 5 tightest; the world entry copies the CVar's integer
    /// in raw (`0x6d9013`), so it can hold any `u32`, and `GetZoom` answers it unsigned (`0x4eec31`).
    pub zoom: u32,
    pub inside_zoom: u32,
    /// Inside a WMO interior, which picks the index; pushed by the app, which owns that test.
    pub inside: bool,
    /// What each texture setter last left in its handle, with the call's stamp, so the host
    /// applies a call once; `None` until this VM sets that slot.
    pub art: [Option<(u64, MinimapArt)>; 3],
    /// The stamp the next setter call takes.
    pub art_stamp: u64,
    /// The one ping, a WoW world `(x, y)` (`0xbc787c`, `0xbc7880`): zero from the static
    /// initializer (`0x4ea1f0`), never cleared.
    pub ping: (f32, f32),
    /// The active player's WoW world `(x, y)`, pushed by the app every frame; `None` with no player
    /// object, when the ping methods do nothing (`0x4eecdf`, `0x4eeff6`).
    pub player: Option<(f32, f32)>,
}

/// Both zoom CVars register with `"3"` (`0x48fc5a`/`0x48fc76`) and the reset copies the CVar in;
/// a higher index zooms in, 3 being a 60 yd indoor radius and a 133.3 yd outdoor half-extent.
pub const MINIMAP_DEFAULT_ZOOM: u32 = 3;

impl Default for MinimapStatics {
    fn default() -> Self {
        Self {
            zoom: MINIMAP_DEFAULT_ZOOM,
            inside_zoom: MINIMAP_DEFAULT_ZOOM,
            inside: false,
            art: [None, None, None],
            art_stamp: 0,
            ping: (0.0, 0.0),
            player: None,
        }
    }
}

impl MinimapStatics {
    /// The index `GetZoom`/`SetZoom` act on now (`0x6da980`).
    pub fn active_zoom(&self) -> u32 {
        if self.inside {
            self.inside_zoom
        } else {
            self.zoom
        }
    }

    /// The view radius in yards at the live zoom (`0x6da9b0`).
    pub fn radius_yd(&self) -> f32 {
        minimap_radius_yd(self.zoom, self.inside_zoom, self.inside)
    }

    /// The ping's offset from the player over the zoom diameter `D`, `(−dy/D, dx/D)` with `dx`,
    /// `dy` the point less the player's position; `None` with no player.
    fn ping_fraction(&self) -> Option<(f64, f64)> {
        let player = self.player?;
        let k = 1.0 / (2.0 * f64::from(self.radius_yd()));
        let dx = f64::from(self.ping.0) - f64::from(player.0);
        let dy = f64::from(self.ping.1) - f64::from(player.1);
        Some((-dy * k, dx * k))
    }

    /// `GetPingPosition` (`0x4eefd0`), anew from the stored point every call: `(0, 0)` with no
    /// player (`0x4eeff6`), the second value rounded to a float (`0x4ef05b`), the first not.
    pub fn ping_position(&self) -> (f64, f64) {
        self.ping_fraction()
            .map_or((0.0, 0.0), |(a, b)| (a, f64::from(b as f32)))
    }

    /// `MinimapApplyPing` (`0x4ee330`): store the point (`0x4ee366`, `0x4ee373`) and give
    /// `MINIMAP_PING`'s two numbers, the first rounded to a float before it is negated
    /// (`0x4ee473`), the second a double (`0x4ee476`); `None`, nothing stored, with no player.
    pub fn apply_ping(&mut self, world: (f32, f32)) -> Option<(f64, f64)> {
        self.player?;
        self.ping = world;
        self.ping_fraction()
            .map(|(a, b)| (-f64::from((-a) as f32), b))
    }

    /// `SetZoomIndex` (`0x6da8e0`) on the `__ftol` of a level: the cell the indoor byte picks takes
    /// it capped by an unsigned compare against 5 (`0x6da8fd`, `0x6da902`), so a negative level is
    /// 5. Only a change writes, and names the CVar the reference then sets with `%d`
    /// (`minimapInsideZoom` or `minimapZoom`, `0x6da909`).
    pub fn set_zoom_index(&mut self, level: i32) -> Option<&'static str> {
        let index = (level as u32).min(u32::from(MINIMAP_ZOOM_LEVELS - 1));
        let (cell, cvar) = if self.inside {
            (&mut self.inside_zoom, "minimapInsideZoom")
        } else {
            (&mut self.zoom, "minimapZoom")
        };
        if *cell == index {
            return None;
        }
        *cell = index;
        Some(cvar)
    }
}

/// One of the minimap's three process-wide textures, each loaded at world entry (`0x4ed810`) and
/// replaced by its setter: the disc mask (`0xbc7968`), the landmark icon atlas (`0xbc827c`) and the
/// dot atlas (`0xbc822c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimapArtSlot {
    Mask = 0,
    Icons = 1,
    Blips = 2,
}

/// What a minimap texture setter left in its handle: `TextureCreate` (`0x449d90`) with a status
/// object and file flag 1, so a loose file loads too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MinimapArt {
    /// The file the setter named, which the host loads.
    File(String),
    /// A name that did not resolve: `TextureCreate`'s 8×8 solid `0xff00ff00` (`[0x90582c]`),
    /// stored before the setter raises.
    Missing,
    /// An empty name: `TextureCreate` returns no texture and logs nothing (`0x449db1`).
    Null,
}

impl MinimapStatics {
    /// Store a setter's result in its slot under a fresh stamp.
    pub fn set_art(&mut self, slot: MinimapArtSlot, art: MinimapArt) {
        self.art_stamp += 1;
        self.art[slot as usize] = Some((self.art_stamp, art));
    }
}

/// The outdoor zoom table (`0x8116d0`): the view diameter in map chunks per zoom index.
pub const MINIMAP_ZOOM_CHUNKS: [f32; 6] = [14.0, 12.0, 10.0, 8.0, 6.0, 4.0];

/// The indoor view radius in yards per indoor zoom index, table `0x8116e8`.
pub const MINIMAP_INDOOR_RADII: [f32; 6] = [150.0, 120.0, 90.0, 60.0, 40.0, 25.0];

/// The minimap's radius in yards (`0x6da9b0`): with the indoor byte set the indoor table at the
/// indoor index (`0x6da9bf`), else the outdoor chunk count times 0.5 and 33.333332 (`0x6da9e9`,
/// `0x6da9ef`). The ping, the blips and the dots all use it. Deviation: an index past 5 reads the
/// last entry, where 1.12.1 reads past both six-entry tables (`0x6da9d5`, `0x6da9c4`), a client
/// fault a hand-edited CVar reaches.
pub fn minimap_radius_yd(zoom: u32, inside_zoom: u32, inside: bool) -> f32 {
    let at = |z: u32| z.min(u32::from(MINIMAP_ZOOM_LEVELS - 1)) as usize;
    if inside {
        MINIMAP_INDOOR_RADII[at(inside_zoom)]
    } else {
        MINIMAP_ZOOM_CHUNKS[at(zoom)] * 0.5 * 33.333_332
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

/// The `[Minimap+0x320]` group's size, the party and pet arrows.
pub const MINIMAP_GROUP_ARROWS: usize = 5;

/// The `[Minimap+0x314]` group's size, the landmark rim arrows.
pub const MINIMAP_RIM_ARROWS: usize = 3;

/// The default `minimapArrowModel` (`0x84c768`), applied to engine children 1 to 8 (`0x4ee170`).
pub const MINIMAP_DEFAULT_ARROW_MODEL: &str = "Interface\\Minimap\\Rotating-MinimapArrow.mdx";

/// The default `minimapPlayerModel` (`0x8453c0`), applied to engine child 9 alone (`0x4ee260`).
pub const MINIMAP_DEFAULT_PLAYER_MODEL: &str = "Interface\\Minimap\\MinimapArrow.mdx";

/// A `CSimpleScrollFrame`'s members (`+0x314`..`+0x328`), all zero from the ctor (`0x786a51`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollFrameState {
    /// `SetScrollChild`'s frame, which pans within this frame's rect.
    pub child: Option<FrameHandle>,
    /// `[+0x31c]`, `[+0x320]`: the horizontal and vertical ranges in layout units over the frame's
    /// scale, as the last recompute stored them (`0x786eaa`, `0x786ed1`); the getters convert them.
    pub range: [f32; 2],
    /// `[+0x314]`: the ranges are out of date. `UpdateScrollChildRect`, `SetScrollChild` and a
    /// change in the frame's size set it; the frame's update recomputes and clears it.
    pub range_dirty: bool,
    /// The frame's width and height in layout units when its update last ran, from which that
    /// update sees a resize (`0x7871f0`'s mark); the ctor's empty rect until then.
    pub seen_size: [f32; 2],
    /// `SetHorizontalScroll`'s offset in px, unclamped; `0x787100` anchors the child at
    /// `(+horizontal, +vertical)`, so scrolling right takes a negative offset, down a positive.
    pub horizontal: f32,
    /// `SetVerticalScroll`'s offset in px (`0x786db0`), unclamped: a positive one lifts the child
    /// to show what is below; the FrameXML scroll bar's range keeps it in bounds.
    pub vertical: f32,
}

/// A Button's three embedded fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ButtonFonts {
    pub(crate) normal: crate::script::font::FontId,
    pub(crate) highlight: crate::script::font::FontId,
    pub(crate) disabled: crate::script::font::FontId,
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

/// The writes to regions' shown bit (`+0xc4`) a Button's state machine makes, in order. The bit
/// is the region's own, the one `Show`/`Hide` write and `IsShown` and the draw read, so the
/// machine hands its writes back for the caller to apply to the regions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "the caller writes these bits into the regions"]
pub struct ShownWrites(pub Vec<(RegionHandle, bool)>);

impl ShownWrites {
    fn hide(&mut self, rh: Option<RegionHandle>) {
        self.0.extend(rh.map(|rh| (rh, false)));
    }

    fn show(&mut self, rh: Option<RegionHandle>) {
        self.0.extend(rh.map(|rh| (rh, true)));
    }
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
    /// A `CSimpleHyperlinkButton`'s link (`+0x4dc`) and text (`+0x4e0`), which its enter, leave
    /// and click hand to its text frame (`script::hyperlink`); `None` on any other Button.
    pub hyperlink: Option<(String, String)>,
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
    /// The three embedded fonts the label follows by state (`0x779810`), normal `+0x33c`,
    /// highlight `+0x3b8` and disabled `+0x434`, made on first use (`script::button::fonts`).
    /// Each is a real font: `<NormalFont>` and kin load into it, `Set*FontObject` link it,
    /// `SetFont` and `Set*TextColor` write it and push to the label.
    pub(crate) fonts: Option<ButtonFonts>,
    /// `LockHighlight()`: highlighted regardless of hover, both the texture and the label font.
    pub locked_highlight: bool,
    /// `<PushedTextOffset>`/`SetPushedTextOffset` (`+0x4b0`/`+0x4b4`), x and y in layout units;
    /// the ctor's (0.001, -0.001) (`0x778740`, `0x77874a`), about 1.28 UI units right and down at
    /// 4:3. Only [`Self::refresh_label`] puts it on the label.
    pub pushed_text_offset: [f32; 2],
    /// The label's text offset (`CSimpleFontString::SetTextOffset` `0x772100`, `+0x110`/`+0x114`)
    /// in layout units: it moves the drawn text, never the label's rect.
    label_offset: [f32; 2],
    /// A real state change happened since the label last relinked ([`Self::take_relink`]).
    relink_due: bool,
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
            hyperlink: None,
            normal: None,
            pushed: None,
            disabled: None,
            highlight: None,
            checked_tex: None,
            disabled_checked: None,
            text: None,
            registered_clicks: HashSet::from(["LeftButtonUp".to_string()]),
            fonts: None,
            locked_highlight: false,
            pushed_text_offset: [f32::from_bits(0x3a83_126f), f32::from_bits(0xba83_126f)],
            label_offset: [0.0, 0.0],
            relink_due: false,
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
    /// `0x7786a0`), so a press or `Disable()` without its own texture keeps the normal art. The
    /// hide of the old (`0x7797c0`) runs only when the new state has a texture too, and the show of
    /// the new (`0x7797ea`) writes its bit.
    fn set_state(&mut self, new: ButtonVisualState) -> ShownWrites {
        if new == self.state {
            return ShownWrites::default();
        }
        self.state = new;
        let mut w = ShownWrites::default();
        if let Some(slot) = self.state_slot(new) {
            w.hide(self.shown);
            self.shown = Some(slot);
            w.show(Some(slot));
        }
        // The label refresh closes every real change (`0x7797fc`); its font half is the model's.
        self.refresh_label();
        self.relink_due = true;
        w
    }

    /// Whether a real state change since the last call owes the label its font relink, the half
    /// of the refresh (`0x779878`) that needs the model; an unchanged state returns before it
    /// (`0x7797a9`).
    pub(crate) fn take_relink(&mut self) -> bool {
        std::mem::take(&mut self.relink_due)
    }

    /// The label refresh's offset half (`0x779810`): PUSHED puts the pushed text offset on the
    /// label (`0x779829`-`0x779837`), any other state zero (`0x77984f`). It runs on a state change,
    /// an enabled enter or leave (`0x7794ab`, `0x779502`), `Lock`/`UnlockHighlight` (`0x495a98`)
    /// and a label install (`0x778daa`), so a `SetPushedTextOffset` on a held button shows at the
    /// next of these.
    pub fn refresh_label(&mut self) {
        self.label_offset = if self.state == ButtonVisualState::Pushed {
            self.pushed_text_offset
        } else {
            [0.0, 0.0]
        };
    }

    /// The offset the label's text draws at, in layout units ([`Self::refresh_label`]).
    pub fn label_offset(&self) -> [f32; 2] {
        self.label_offset
    }

    /// `SetButtonState(state, locked)` (`0x779790`), the one writer: the lock is stored before the
    /// equal-state early-out (`0x77979d`), so it is set or cleared even when the state holds.
    pub fn set_button_state(&mut self, new: ButtonVisualState, locked: bool) -> ShownWrites {
        self.locked = locked;
        self.set_state(new)
    }

    /// `CSimpleCheckbox::Refresh` (`0x7854c0`): clears both checked textures' bits, then shows
    /// the one the checked flag and the state pick (`0x785538`), the disabled pair's when DISABLED
    /// and it exists, else the checked texture. `force` is its second argument: without it a flag
    /// that holds does nothing (`0x7854d0`). Only `SetChecked`, a click, `SetEnabled`, the two
    /// checked setters and the XML `checked=` attribute (`0x7851a6`) reach it; a state switch alone
    /// leaves the pair. The reference's `checked=` runs before the checked textures exist, so it
    /// shows nothing and each setter's refresh shows its texture; benilla's loader calls
    /// `SetChecked` after the textures, which ends in the same state.
    pub fn refresh_checked(&mut self, checked: bool, force: bool) -> ShownWrites {
        if !force && checked == self.checked {
            return ShownWrites::default();
        }
        self.checked = checked;
        let mut w = ShownWrites::default();
        w.hide(self.checked_tex);
        w.hide(self.disabled_checked);
        if checked {
            w.show(match self.disabled_checked {
                Some(dc) if !self.enabled() => Some(dc),
                _ => self.checked_tex,
            });
        }
        w
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
    /// pushed button neither un-pushes nor unlocks it. A CheckButton's override (`0x785490`) then
    /// refreshes the checked pair whether or not it changed, which a Button, with no checked
    /// textures, does not notice.
    pub fn set_enabled(&mut self, on: bool) -> ShownWrites {
        let mut w = if on == self.enabled() {
            ShownWrites::default()
        } else {
            self.set_button_state(
                if on {
                    ButtonVisualState::Normal
                } else {
                    ButtonVisualState::Disabled
                },
                false,
            )
        };
        w.0.extend(self.refresh_checked(self.checked, true).0);
        w
    }

    /// The hide edge (`0x7791e0`): an unlocked, enabled button goes NORMAL; `<OnHide>` still fires.
    pub fn on_hide(&mut self) -> ShownWrites {
        if self.state != ButtonVisualState::Disabled && !self.locked {
            self.set_button_state(ButtonVisualState::Normal, false)
        } else {
            ShownWrites::default()
        }
    }

    /// The press edge (`0x779210`), past the registration and hit gates, so a registered click
    /// pushes the button even when the click itself does nothing.
    pub fn on_mouse_down(&mut self) -> ShownWrites {
        if !self.locked && self.state != ButtonVisualState::Disabled {
            self.set_button_state(ButtonVisualState::Pushed, false)
        } else {
            ShownWrites::default()
        }
    }

    /// The release edge (`0x7792d0`): a pushed, unlocked button goes NORMAL wherever the cursor
    /// is, even one Lua pushed. The caller skips it when the release ends a drag (`0x7792df`).
    pub fn on_mouse_up(&mut self) -> ShownWrites {
        if !self.locked && self.state == ButtonVisualState::Pushed {
            self.set_button_state(ButtonVisualState::Normal, false)
        } else {
            ShownWrites::default()
        }
    }

    /// The drag-start edge (`0x7793f0`), before `<OnDragStart>`: it fires at the drag threshold
    /// (0.01 frame units, `0x81c468`) on a `RegisterForDrag` frame, not at the rect edge.
    pub fn on_drag_start(&mut self) -> ShownWrites {
        if !self.locked && self.state != ButtonVisualState::Disabled {
            self.set_button_state(ButtonVisualState::Normal, false)
        } else {
            ShownWrites::default()
        }
    }

    /// `SetNormalTexture` and kin (`0x778fd0`): stores the slot; the state it is for being the
    /// current one, the texture becomes the shown one and its bit is written (`0x779027`..
    /// `0x779037`). A shown pointer at the displaced texture is dropped first (`0x778fe9`). The
    /// reference destroys a displaced texture (`0x77900a`); the caller frees it.
    pub fn set_state_slot(
        &mut self,
        state: ButtonVisualState,
        rh: Option<RegionHandle>,
    ) -> ShownWrites {
        let slot = match state {
            ButtonVisualState::Disabled => &mut self.disabled,
            ButtonVisualState::Normal => &mut self.normal,
            ButtonVisualState::Pushed => &mut self.pushed,
        };
        let old = std::mem::replace(slot, rh);
        if self.shown == old {
            self.shown = None;
        }
        let mut w = ShownWrites::default();
        if state == self.state && rh.is_some() {
            self.shown = rh;
            w.show(rh);
        }
        w
    }

    /// The region-removed hook: a CheckButton drops its checked pair (`0x785230`, `+0x4e0` and
    /// `+0x4e4`), then every button its label (`+0x338`), its three state slots (`+0x4b8` to
    /// `+0x4c0`), the shown one (`+0x4c4`) and its highlight (`+0x4c8`) (`0x778cb0`).
    fn region_removed(&mut self, rh: RegionHandle) {
        for slot in [
            &mut self.checked_tex,
            &mut self.disabled_checked,
            &mut self.text,
            &mut self.disabled,
            &mut self.normal,
            &mut self.pushed,
            &mut self.shown,
            &mut self.highlight,
        ] {
            if *slot == Some(rh) {
                *slot = None;
            }
        }
    }

    /// Whether the Highlight region draws: while hovered or locked. The reference gates it by its
    /// layer's enable (`0x76b6a0`/`0x76b6f0`), not by its shown bit, which it keeps from the
    /// link (`0x779139`); every other region of a Button draws by its own bit alone.
    pub fn highlight_draws(&self, rh: RegionHandle, hovered: bool) -> bool {
        // No `enabled` term: `Disable()` turns the whole HIGHLIGHT layer off before this.
        Some(rh) != self.highlight || hovered || self.locked_highlight
    }
}

/// A StatusBar's value and fill texture (`CSimpleStatusBar`, ctor `0x782d80`). The value fields
/// are private: every write goes through [`Self::set_min_max`] or [`Self::set_value`], which keep
/// the flag bits the client keeps at `+0x314`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusBarState {
    /// The low bound (`+0x318`).
    min: f32,
    /// The high bound (`+0x31c`).
    max: f32,
    /// The held value (`+0x320`), clamped into `[min, max]`.
    value: f32,
    /// `+0x314 & 2`, set by `SetMinMaxValues` (`0x78329a`); until then `SetValue` is a no-op.
    range_valid: bool,
    /// `+0x314 & 4`, set by the first stored value (`0x783347`).
    has_value: bool,
    /// `+0x314 & 1`, the fill to redo: set by the range (`0x78329a`), a stored value (`0x783347`),
    /// the orientation (`0x78325c`) and a bar texture installed (`0x783230`); cleared by the
    /// update that writes the coordinates ([`Self::take_fill`], `0x78344e`).
    fill_dirty: bool,
    /// `+0x328`: VERTICAL fills bottom-up; HORIZONTAL, the default, left to right (enum
    /// `0x811b00`).
    vertical: bool,
    /// The fill texture (`<BarTexture>`, `+0x324`), whose coordinates the update writes.
    pub bar: Option<RegionHandle>,
}

impl StatusBarState {
    /// The range, `(min, max)`, as `GetMinMaxValues` answers it.
    pub fn min_max(&self) -> (f32, f32) {
        (self.min, self.max)
    }

    /// The held value, as `GetValue` answers it.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// `CSimpleStatusBar::SetMinMaxValues` (`0x783270`), the one setter the Lua binding, the XML
    /// loader and the engine's health watchers all call: the low bound is `max` when `min > max`
    /// (`fcom; test ah,0x41`, so NaN keeps `min`), never a swap. With a value held it clears `& 4`
    /// and re-sets the value (`0x7832af`, `0x7832bc`), so `OnValueChanged` fires on every call
    /// once the bar has a value; the returned value is that fire.
    pub fn set_min_max(&mut self, min: f32, max: f32) -> Option<f32> {
        self.min = if min > max { max } else { min };
        self.max = max;
        self.range_valid = true;
        self.fill_dirty = true;
        if !self.has_value {
            return None;
        }
        self.has_value = false;
        self.set_value(self.value)
    }

    /// `CSimpleStatusBar::SetValue` (`0x7832d0`), op for op: nothing without a range
    /// (`0x7832dc`); the value raised to `min` (`0x7832e8`), then lowered to `max` when `max` is
    /// below that (`0x783303`); with a value held, an equal one returns (`0x783342`; NaN is never
    /// equal). Otherwise it is stored and returned, the caller firing `OnValueChanged`.
    pub fn set_value(&mut self, v: f32) -> Option<f32> {
        if !self.range_valid {
            return None;
        }
        let lo = if self.min > v { self.min } else { v };
        let v = if self.max < lo { self.max } else { lo };
        if self.has_value && v == self.value {
            return None;
        }
        self.value = v;
        self.has_value = true;
        self.fill_dirty = true;
        Some(v)
    }

    /// VERTICAL, as `GetOrientation` answers it.
    pub fn vertical(&self) -> bool {
        self.vertical
    }

    /// `CSimpleStatusBar::SetOrientation` (`0x783250`): stores it and marks the fill.
    pub fn set_orientation(&mut self, vertical: bool) {
        self.vertical = vertical;
        self.fill_dirty = true;
    }

    /// `InstallBarTexture` (`0x7831f0`) past its same-texture return (`0x783202`): the slot takes
    /// `bar` and the fill is marked (`0x783230`).
    pub fn install_bar(&mut self, bar: Option<RegionHandle>) {
        self.bar = bar;
        self.fill_dirty = true;
    }

    /// The fill fraction `(value − min) / (max − min)` (`0x783380`), `0.0` when `max − min` is not
    /// above 0.
    pub fn fraction(&self) -> f32 {
        if self.max - self.min > 0.0 {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The update `CSimpleStatusBar::OnFrameUpdate` (`0x7833c0`) runs after the frame's
    /// `OnUpdate`: with the fill marked, a bar texture, a range and a value (`0x7833d8`-`0x7833ea`)
    /// it clears the mark (`0x78344e`) and answers the coordinates it builds from scratch for
    /// `SetTexCoordRect` (`0x770410`), `[left, right, top, bottom]`: 0 to the fill across and 0 to
    /// 1 down when HORIZONTAL (`0x78340e`-`0x78341e`), 0 to 1 across and 0 to the fill down when
    /// VERTICAL (`0x78342f`-`0x783436`), so a vertical bar shows the top of its art. Otherwise it
    /// answers `None` and the texture keeps whatever coordinates it holds.
    pub fn take_fill(&mut self) -> Option<[f32; 4]> {
        if !(self.fill_dirty && self.bar.is_some() && self.range_valid && self.has_value) {
            return None;
        }
        self.fill_dirty = false;
        let f = self.fraction();
        Some(if self.vertical {
            [0.0, 1.0, 0.0, f]
        } else {
            [0.0, f, 0.0, 1.0]
        })
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
