//! **Chat bubbles** — `CGChatBubbleFrame` (decision 0288's phase-9 tail, landed by decision
//! 0598): the over-the-head speech bubble a chat line spawns, a **2-D overlay** like the V-plate
//! ([`crate::vplates`]) and unlike the world-pass overhead *names* ([`crate::nameplates`]).
//!
//! The pinned law, transcribed:
//! - **Spawn** (`0x608ac0`, one caller — the SMSG_MESSAGECHAT display path): the sender GUID
//!   typed-looks-up to a live unit or no bubble; CVar select `ChatBubbles` (party lines:
//!   `ChatBubblesParty`); non-empty text; **no active V-plate on the unit** (`0x608adc` —
//!   mutual exclusion both ways: a live bubble in turn suppresses the floating overhead name,
//!   [`BubblesActive`]); the local player resolves; 3-D dist² ≤ 400 (20 yd). Self is NOT
//!   excluded. **Replace, never queue**: a new line tears the old bubble down first.
//! - **Lifetime** (`0x4b1810`): word-count-scaled, self-vs-other asymmetric —
//!   `2750 + 750·(words−1)` ms for others, `1500 + 500·(words−1)` for the local player
//!   (your own bubbles are shorter-lived); words = space/tab-run count. 250 ms linear fade-in;
//!   at `create + duration + 250` a permanent 250 ms fade-out, then the frame recycles. The
//!   20 yd gate re-tests **every frame**: out of range fades out *recoverable*, back in range
//!   fades back in.
//! - **Geometry** (`0x4b0940`/`0x4b1600`): the classic `Backdrop` construct —
//!   `ChatBubble-Background` flat fill, `ChatBubble-Backdrop` 8-piece edge
//!   ([`benilla_ui::script::backdrop::pieces`], the same engine geometry FrameXML backdrops
//!   use), `ChatBubble-Tail` a separate square of side = the border-unit, TOPRIGHT on the
//!   frame's BOTTOM + (0, border/4). Border-unit = insets = edge-size = **16/1024 of the
//!   screen width** (`G44·16/(S·1024)`); text = `NAMEPLATE_FONT` at 0.01 gx, wrap hard-capped
//!   at **0.2 gx**, floor 2·border-unit; the frame hugs the text layout ± **0.01 gx** on all
//!   four sides; text colored by chatType (the 94-entry table ≡ [`default_color`]), rendered
//!   PLAIN (`||`→`|`, color/hyperlink escapes stripped — [`sanitize`]).
//! - **Anchor** (`0x4b0c30`): `worldZ = unit.z + attachmentHeight·modelScale + 0.7`, projected
//!   by the plate/name projector, seated **BOTTOM at the point, growing upward** (the mirror
//!   of the plate, which hangs down). `attachmentHeight` is `0x4b0e38 call 0x711a20` — a read of
//!   the **MD20 header image**, i.e. the **Stand sequence CAaBox's Z extent**, a file constant
//!   with no bone matrix anywhere in its call tree — and the scaled product is **latched** at
//!   `bubble+0x354` behind a parity guard, so it is queried exactly **once per chat line**
//!   ([`crate::entities::StandBoxHeight`], 1406).
//! - **Stacking** (`0x4b1060`, the per-frame pass over ALL live bubbles — one caller, `0x4817a3`):
//!   each bubble is its own frame and its frame LEVEL is restamped every frame from the CAMERA's
//!   distance to the speaker. The list re-sorts farthest-first (`0x4b1360`, comparing
//!   `[bubble+0x350]` = |cam − unit|², written at `0x4b12c0`), then a head→tail walk stamps
//!   `SetFrameLevel(2), (3), (4)…` (`0x4b1309`). Level outranks draw layer in the client's total
//!   order, so overlapping bubbles stack as WHOLE cards with the nearest speaker's on top —
//!   nothing interleaves. ([`level_z`]; benilla read the bytes for this one, 1504.)
//!
//! Named divergences (all deliberate):
//! - ~~**`ChatBubblesParty` defaults ON**~~ — **no longer a divergence (1804).** It shipped `"1"`
//!   against the binary's `"0"` from 0598, on the director's `/p` ask; both CVars now boot at the
//!   registrar's own values and the Chat page's row is the way back on. Same walk-back as
//!   [`crate::vplates::VPlateMode`], whose enemy plates were the same shape of pin.
//! - **The v1 kind set is SAY/YELL/PARTY + monster say/yell.** The byte gate is
//!   "sender resolves", not a type whitelist, which *implies* guild/officer/whisper/emote
//!   bubbles too — but that category claim is inferred, resting on an open wire-type remap
//!   (`0x49a870`), and contradicts the remembered reference look, so the uncontested
//!   set ships and a capture can widen it ([`bubble_cvar`]).
//! - ~~**The anchor height is the posed overhead attachment**~~ — **REFUTED and removed (1406).**
//!   This shipped as an INFERRED equivalence ("both are the head-region attachment height,
//!   model-scaled") between the overhead-name chain `0x608640` and the bubble's `0x711a20`. They
//!   are not equivalent, and they differ on exactly the axis that mattered: `0x608640` reads the
//!   live posed palette and tracks the pose, `0x711a20` reads file bytes. The overhead *name*
//!   keeps `0x608640`; the bubble now takes the Stand-box constant the bytes actually specify. It
//!   sits **0.199 model units lower** on a human male (2.0128 vs the attachment's 2.2120) — a
//!   deliberate, measured move toward the reference, not a regression.
//! - **Sizes ride the plates' damped diagonal basis** ([`plate_basis`])
//!   so bubble text and plate text stay the same em at every window — the same director-pinned
//!   deviation from the unbounded byte law.
//! - **No per-frame occlusion fade**: the client also fades a bubble whose speaker model isn't
//!   render-visible (`0x7103d0`/`0x6704c0`); v1 re-tests distance only. Residual named in 0598.
//!
//! (0598 briefly shipped the plate-blocks-bubble gate UN-transcribed — bubbles stacked over our
//! then-always-on plates. 0599 restored the faithful gate and booted friendly plates OFF
//! instead, so friendly/party bubbles have room the reference way.)

use std::collections::HashMap;

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use benilla_ui::layout::Rect as GxRect;
use benilla_ui::script::{inset_atlas_bleed, pieces, Backdrop, Insets};
use benilla_ui::script::{JustifyH, JustifyV, Outline};

use crate::entities::StandBoxHeight;
use crate::net::{Embodied, Guid, NetEntity, SelfGuid};
use crate::ui_chat::{default_color, ChatEventKind};
use crate::ui_pass::{overlay_z, UiQuad, UiQuadAppend, UiQuads, UvRect};
use crate::ui_text::{layout_text_quads, measure_text, FontSpec, Justify, TextSeat, UiFontAtlas};
use crate::vplates::{device_snap, gx_px, plate_basis, text_px, VPlateSet, VPlates};
use benilla_assets::{AssetSet, WorldAssets};
use benilla_world::view::WorldCamera;

/// The two CVars (registrar `0x603280`), host side. **Registered knobs since decision 1139** —
/// they were a pair of `const bool` from 0598 until the options window had a page to put
/// them on, which is exactly the shape 1134 calls a row over a frozen gate.
///
/// **Both defaults are the binary's own** (`ChatBubbles` `"1"`, `ChatBubblesParty` `"0"`).
/// `party` shipped ON from 0598 to 1804 on the director's `/p` ask; the row is on the Chat page,
/// one click from where it was.
#[derive(Resource)]
pub(crate) struct BubbleConfig {
    /// `ChatBubbles` — say/yell and their monster variants.
    pub(crate) all: bool,
    /// `ChatBubblesParty` — party lines, which the client gates separately.
    pub(crate) party: bool,
}

impl Default for BubbleConfig {
    fn default() -> Self {
        Self {
            all: true,
            party: false,
        }
    }
}

/// The bubble art (`0x4b0940` ctor) — the shared tooltip-family textures.
const BG_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Background";
const EDGE_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Backdrop";
const TAIL_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Tail";

/// The 0.7 yd lift over the attachment height (`[0x7ffd7c]` = 0.699999988).
const LIFT: f32 = 0.7;
/// The spawn + per-frame range gate: dist² ≤ 400 (20 yd, `[0x806798]` — same as the plates).
const MAX_DIST_SQ: f32 = 400.0;
/// The fade ramps: 250 ms linear, in and out (`0x4b0ea0`/`0x4b0ee0`).
const FADE_SECS: f32 = 0.25;
/// `NAMEPLATE_FONT` at 0.01 gx — the same em law as the plate name.
const TEXT_H: f32 = 0.01;
/// The auto-fit body margin: text layout ± 0.01 gx on all four sides (`0x3c23d70a`).
const MARGIN: f32 = 0.01;
/// The text wrap hard cap, gx (`[0x80679c]` = 0.2).
const WRAP_W: f32 = 0.2;
/// Border-unit = edge-size = insets = tail side: 16/1024 of the screen WIDTH
/// (`G44·16.0/(S·1024.0)` — G44/S nets the width in the diagonal gx basis).
const BORDER_FRAC: f32 = 16.0 / 1024.0;

/// Paint order **inside one bubble's frame level** — the four pieces, back→front. The tail draws
/// over the frame's bottom edge piece: its art carries the border lines that make the seam read
/// continuous, and the reference gets the same result from its own layer key (the tail is an
/// ARTWORK texture, the text an ARTWORK font string, and a batch drains every texture before
/// every font string — `0x76fb00`).
const Z_BG: u64 = 0;
const Z_EDGE: u64 = 1;
const Z_TAIL: u64 = 2;
const Z_TEXT: u64 = 3;

/// The z base for the bubble at `rank` in this frame's farthest→nearest order — the port of the
/// reference's per-frame `SetFrameLevel(2 + i)` walk (`0x4b12d5`–`0x4b1312`). One
/// LEVEL per bubble, [`overlay_z::BUBBLE_STRIDE`] keys wide, so a whole bubble stacks over a whole
/// bubble; the whole band stays under the V-plates ([`crate::ui_pass::overlay_z`]).
fn level_z(rank: usize) -> u64 {
    let level = (rank as u64).min(overlay_z::BUBBLE_MAX_LEVEL);
    overlay_z::BUBBLE + level * overlay_z::BUBBLE_STRIDE
}

/// Which CVar gates this chat kind's bubble — `None` = the kind never bubbles in v1.
/// PARTY selects `ChatBubblesParty`, every other bubbling kind `ChatBubbles` (`0x608b0d`).
/// Guild/officer/whisper/emote are structurally implied by the byte gate but INFERRED on an
/// OPEN remap (module doc) — out until a capture confirms.
fn bubble_cvar(kind: ChatEventKind, cfg: &BubbleConfig) -> Option<bool> {
    use ChatEventKind as K;
    match kind {
        K::Party => Some(cfg.party),
        K::Say | K::Yell | K::MonsterSay | K::MonsterYell => Some(cfg.all),
        _ => None,
    }
}

/// The space/tab-run word counter (`0x4b1810`): maximal runs of non-space/tab characters.
fn word_count(text: &str) -> u32 {
    let mut words = 0u32;
    let mut in_word = false;
    for b in text.bytes() {
        let sep = b == b' ' || b == b'\t';
        if !sep && !in_word {
            words += 1;
        }
        in_word = !sep;
    }
    words
}

/// The duration law (`0x4b1810`): `base + perWord·(words−1)` ms — 2750/750 for someone else,
/// 1500/500 for the local player (your own bubbles are shorter-lived). Empty → 0.
fn duration_secs(words: u32, is_self: bool) -> f32 {
    if words == 0 {
        return 0.0;
    }
    let (base, per) = if is_self { (1500, 500) } else { (2750, 750) };
    (base + per * (words - 1)) as f32 / 1000.0
}

/// The bubble-text sanitize: `||` stays an escaped literal pipe, `|cAARRGGBB`/`|r` color
/// escapes and the `|H…|h`/`|h` hyperlink wrappers strip (their display text stays) — bubble
/// text is always plain, colored solely by chatType. Output remains in ESCAPED form (literal
/// pipes as `||`) because the glyph layout's own markup parser consumes it downstream.
fn sanitize(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'|' {
            // Copy the whole non-escape run at once (UTF-8 safe: '|' is single-byte).
            let start = i;
            while i < b.len() && b[i] != b'|' {
                i += 1;
            }
            out.push_str(&text[start..i]);
            continue;
        }
        match b.get(i + 1) {
            Some(b'|') => {
                out.push_str("||");
                i += 2;
            }
            Some(b'c') | Some(b'C') if i + 10 <= b.len() => i += 10,
            Some(b'r') | Some(b'R') => i += 2,
            Some(b'H') => {
                // Skip the opener through its `|h`; the display text then flows until the
                // closing `|h`, which the arm below drops.
                i += 2;
                while i < b.len() && !(b[i] == b'|' && b.get(i + 1) == Some(&b'h')) {
                    i += 1;
                }
                i += 2;
            }
            Some(b'h') => i += 2,
            // A dangling or unknown escape renders as a literal pipe + the rest.
            _ => {
                out.push_str("||");
                i += 1;
            }
        }
    }
    out
}

/// The bleed inset for one border piece, `Vec2::ZERO` texels meaning "art size unknown, leave the
/// UVs alone" (a bare test app with no patch chain).
fn pieces_inset(uvs: [[f32; 2]; 4], texels: Vec2) -> [[f32; 2]; 4] {
    if texels.x <= 0.0 || texels.y <= 0.0 {
        return uvs;
    }
    inset_atlas_bleed(uvs, texels.x, texels.y)
}

/// The frame's bottom-left origin for a projected seat: BOTTOM-CENTER on the seat, then both
/// coordinates snapped onto the **device** pixel grid ([`device_snap`], the plate's law).
///
/// Split out so the snap law is nameable and pinned. It shipped as a plain logical `round()`,
/// which is 1 device pixel only at scale 1: on the 2× display we play on it stepped the bubble
/// two physical pixels per axis over a continuously-sliding world, and at a fractional scale
/// (1.25/1.5) it never landed on a texel boundary at all — so it paid the stepping without even
/// buying the crispness it exists for. Same bug, same fix as the plate (0188's snap → the plate's
/// device grid); the bubble was the site that never got it (1398).
fn seat_origin(seat: Vec2, w: f32, scale: f32) -> Vec2 {
    Vec2::new(
        device_snap(seat.x - w * 0.5, scale),
        device_snap(seat.y, scale),
    )
}

/// The border-unit in logical px for this viewport: 16/1024 of the screen width, carried
/// through the damped size basis (so it shrinks in step with plate/text sizes past the knee).
fn border_px(viewport: Vec2, basis: f32) -> f32 {
    let width_gx = viewport.x / viewport.length();
    gx_px(width_gx * BORDER_FRAC, basis).max(1.0)
}

/// The bubble-spawn requests this frame — fed by the chat feed's wire arm (the reference
/// spawns in the SMSG display path, the same moment the line routes) and drained by
/// [`drive_bubbles`]. Push-side filtered so guild/system spam never queues strings.
#[derive(Resource, Default)]
pub(crate) struct BubbleQueue(Vec<(u64, ChatEventKind, String)>);

impl BubbleQueue {
    /// Queue a routed wire line for a bubble. Drops non-bubbling kinds, disabled CVars, and
    /// senderless lines here; the live-unit/range/plate gates run in the driver. The switch is
    /// read at PUSH time, so a Chat-page click takes the very next line either way.
    pub(crate) fn push(
        &mut self,
        cfg: &BubbleConfig,
        sender_guid: u64,
        kind: ChatEventKind,
        text: &str,
    ) {
        if sender_guid == 0 || !bubble_cvar(kind, cfg).unwrap_or(false) {
            if benilla_assets::trace::enabled_for("bub") {
                let why = match (sender_guid, bubble_cvar(kind, cfg)) {
                    (0, _) => "senderless".to_string(),
                    (_, None) => format!("{kind:?}-never-bubbles"),
                    (_, Some(false)) => format!("{kind:?}-cvar-off"),
                    _ => unreachable!("the guard above admits nothing else"),
                };
                benilla_assets::trace::line(
                    "bub",
                    &format!("refuse guid={sender_guid:#x} push:{why}"),
                );
            }
            return;
        }
        self.0.push((sender_guid, kind, text.to_string()));
    }
}

/// One live bubble (the `CGChatBubbleFrame` + its `CGUnit+0xe64` handle, folded together).
struct Bubble {
    /// Sanitized display text ([`sanitize`] — plain, escapes stripped).
    text: String,
    /// The chatType color (the 94-entry table), client-space sRGB 0..1.
    color: [f32; 3],
    /// `Time::elapsed_secs` at spawn.
    born: f32,
    /// The steady window past the fade-in ([`duration_secs`]).
    duration: f32,
    /// The **latched** anchor height above the speaker's feet: the Stand box's Z extent
    /// ([`crate::entities::StandBoxHeight`]) already multiplied by the unit's model scale, queried
    /// ONCE here and never re-read — the reference caches exactly this product at `bubble+0x354`
    /// behind a parity guard, so it is one query per chat line (1406).
    lift: f32,
    /// Current fade alpha 0..1 — ramped 250 ms linear toward the eligibility verdict.
    alpha: f32,
}

/// The live bubbles, keyed by speaker guid (a unit carries at most one — replace, never queue).
#[derive(Resource, Default)]
struct Bubbles(HashMap<u64, Bubble>);

/// The units carrying a live bubble this frame — the `ShouldShowName` exclusivity verdict the
/// overhead-name driver reads (`+0xe64` ≠ 0 suppresses the floating name), the mirror of
/// [`VPlates`].
#[derive(Resource, Default)]
pub(crate) struct BubblesActive(pub(crate) EntityHashSet);

/// The bubble art, warmed at boot like the plate art (a first bubble must not draw texture-less).
/// The edge strip needs REPEAT addressing (its runs tile UVs past 1); bg and tail clamp.
#[derive(Resource)]
struct BubbleArt {
    bg: Handle<Image>,
    edge: Handle<Image>,
    tail: Handle<Image>,
    /// The edge atlas's size in texels, for the half-texel bleed inset
    /// ([`benilla_ui::script::backdrop::inset_atlas_bleed`] — 1402). Stamped at load because the
    /// draw has no `Assets<Image>` and this never changes.
    edge_texels: Vec2,
}

fn load_bubble_art(
    mut commands: Commands,
    assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut assets) = assets else {
        return; // no game data (bare test app) — drive_bubbles tolerates the missing resource
    };
    let bg = assets.sprite_texture(BG_TEXTURE, &mut images);
    let edge = assets.sprite_texture_tiled(EDGE_TEXTURE, &mut images);
    let tail = assets.sprite_texture(TAIL_TEXTURE, &mut images);
    let (Some(bg), Some(edge), Some(tail)) = (bg, edge, tail) else {
        warn!("chat_bubble: bubble art missing from the patch chain — bubbles will not draw");
        return;
    };
    let edge_texels = images.get(&edge).map_or(Vec2::ZERO, |i| {
        let s = i.texture_descriptor.size;
        Vec2::new(s.width as f32, s.height as f32)
    });
    commands.insert_resource(BubbleArt {
        bg,
        edge,
        tail,
        edge_texels,
    });
}

/// Spawn, tick, and draw, every frame: drain the queue through the spawn gate (`0x608ac0`),
/// ramp each live bubble's fade against its lifetime + the per-frame 20 yd re-test
/// (`0x4b0c30`), publish the name-suppression verdict, and append the draw list — backdrop
/// pieces, tail, wrapped chatType-colored text — bottom-seated on the projected anchor.
/// Runs after [`VPlateSet`] (the spawn gate reads this frame's plate verdict), inside the
/// [`UiQuadAppend`] window.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn drive_bubbles(
    mut queue: ResMut<BubbleQueue>,
    mut bubbles: ResMut<Bubbles>,
    mut active: ResMut<BubblesActive>,
    vplates: Res<VPlates>,
    self_guid: Res<SelfGuid>,
    units: Query<(Entity, &Guid, &Transform), With<NetEntity>>,
    self_q: Query<&Transform, With<Embodied>>,
    camera: Query<(&Camera, &Transform), With<WorldCamera>>,
    mut atlas: Option<ResMut<UiFontAtlas>>,
    mut quads: ResMut<UiQuads>,
    art: Option<Res<BubbleArt>>,
    // The latched anchor height, read once per bubble at spawn ([`StandBoxHeight`]).
    heights: Query<&StandBoxHeight>,
    time: Res<Time>,
    // The device scale, for the seat's pixel snap ([`device_snap`]).
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    active.0.clear();
    let now = time.elapsed_secs();
    let step = time.delta_secs() / FADE_SECS;
    let self_tf = self_q.single().ok();
    // The typed sender lookup (`0x468460`): guid → live unit. Linear over the streamed set,
    // like the plate walk — both populations are 20 yd-bounded and small.
    let find = |guid: u64| units.iter().find(|(_, g, _)| g.0 == guid);

    // ── The spawn/replace gate (`0x608ac0`) ─────────────────────────────────────────────────
    // Every arm below reports its REFUSAL under the `bub` tag. A bubble that never appears used to
    // be a silent five-way question — no text, no unit, a plate in the way, no local player, out of
    // range — with nothing in any log to separate them, and the draw trace beside it says nothing
    // because the draw never runs. One line per refusal is the difference between reading the
    // answer and bisecting for it.
    let refuse = |why: &str, guid: u64| {
        if benilla_assets::trace::enabled_for("bub") {
            benilla_assets::trace::line("bub", &format!("refuse guid={guid:#x} {why}"));
        }
    };
    for (guid, kind, raw) in queue.0.drain(..) {
        let text = sanitize(&raw);
        let words = word_count(&text);
        if words == 0 {
            refuse("empty-text", guid);
            continue; // empty text → duration 0 → no bubble
        }
        let Some((entity, _, tf)) = find(guid) else {
            refuse("sender-not-a-live-unit", guid);
            continue; // sender not resolvable to a live unit → NO bubble
        };
        // An active V-plate blocks bubble creation (`0x608adc` — the mutual exclusion,
        // faithful; restored by 0599 after 0598 briefly stacked them). Friendly plates boot
        // OFF (0599's other half), so friendly/party bubbles have room; a plated hostile
        // yelling shows no bubble, exactly like the reference with plates toggled on.
        if vplates.0.contains(&entity) {
            refuse("v-plate-on-speaker", guid);
            continue;
        }
        let Some(self_tf) = self_tf else {
            refuse("no-local-player", guid);
            continue; // the local player must resolve
        };
        let d2 = (tf.translation - self_tf.translation).length_squared();
        if d2 > MAX_DIST_SQ {
            refuse(&format!("out-of-range dist={:.1}yd", d2.sqrt()), guid);
            continue; // 20 yd at spawn
        }
        let is_self = self_guid.0 == Some(guid);
        let c = default_color(kind);
        // The one-time attachment query (`0x4b0e38 call 0x711a20`, scaled by `[unit+0x90]`): a file
        // constant × this unit's model scale. A speaker with no bounds reads 0 and the bubble sits
        // at the feet + 0.7, which is the reference's own degenerate for a bounds-less model.
        let lift = heights.get(entity).map_or(0.0, |h| h.0) * tf.scale.y;
        // Replace, never queue: the insert tears the old bubble down (`0x608c00`) and the
        // fresh one fades in from 0.
        bubbles.0.insert(
            guid,
            Bubble {
                text,
                color: [
                    f32::from(c[0]) / 255.0,
                    f32::from(c[1]) / 255.0,
                    f32::from(c[2]) / 255.0,
                ],
                born: now,
                duration: duration_secs(words, is_self),
                lift,
                alpha: 0.0,
            },
        );
    }
    if bubbles.0.is_empty() {
        return;
    }

    // ── Tick + draw ─────────────────────────────────────────────────────────────────────────
    let cam = camera.single().ok();
    let art = art.as_deref();
    let scale = window.single().map_or(1.0, Window::scale_factor);
    let trace = std::env::var("WOW_BUBBLE_TRACE").as_deref() == Ok("1");
    let mut dead = Vec::new();
    let mut pending: Vec<Pending> = Vec::new();
    for (guid, b) in bubbles.0.iter_mut() {
        let Some((entity, _, tf)) = find(*guid) else {
            dead.push(*guid); // the speaker despawned — the unit teardown takes its bubble
            continue;
        };
        if now >= b.born + b.duration + FADE_SECS {
            // The permanent fade-out (`0x4b0ee0(1)`) — at 0 the frame recycles.
            b.alpha -= step;
            if b.alpha <= 0.0 {
                dead.push(*guid);
                continue;
            }
        } else {
            // Per-frame re-eligibility: the same 20 yd gate, recoverable — out of range fades
            // out, back in range fades back in. (The client also fades on the speaker model's
            // render visibility — the v1 residual, module doc.)
            let eligible = self_tf
                .is_some_and(|s| (tf.translation - s.translation).length_squared() <= MAX_DIST_SQ);
            b.alpha = if eligible {
                (b.alpha + step).min(1.0)
            } else {
                (b.alpha - step).max(0.0)
            };
        }
        // The live handle (`+0xe64` ≠ 0): the name suppression holds while the bubble EXISTS,
        // faded or not.
        active.0.insert(entity);
        if b.alpha <= 0.0 {
            continue;
        }
        // No camera/atlas/art — lifetimes still tick, nothing draws.
        let (Some((cam, cam_pose)), true) = (cam, atlas.is_some() && art.is_some()) else {
            continue;
        };
        // World anchor (`0x4b0c30`): the unit's position, Z lifted by the LATCHED Stand-box height
        // + 0.7 yd — seated BOTTOM at the projected point, growing upward. Every term is this
        // frame's `Transform` or a constant, so there is no pose to read and no clock to get wrong.
        // A point behind the camera draws nothing (state kept — it fades back the moment it
        // projects again).
        let seat_world = tf.translation + Vec3::Y * (b.lift + LIFT);
        let cam_tf = GlobalTransform::from(*cam_pose);
        // The SEVENTH silent cause, and the one that cost the most to find: a seat behind the
        // camera fails to project and the bubble draws nothing — alive, ticking, invisible, and
        // (before this) invisible to the trace too, because every `bub` line is written by the
        // draw. A probe whose camera had been restored pitched into the ground therefore read as
        // "bubbles are broken" for six runs (1402). Reported like the spawn gate's refusals.
        let Ok(seat) = cam.world_to_viewport(&cam_tf, seat_world) else {
            refuse("not-on-screen (behind the camera)", *guid);
            continue;
        };
        let Some(viewport) = cam.logical_viewport_size() else {
            refuse("no-viewport", *guid);
            continue;
        };
        // Seated and projected here, DRAWN below: the level a bubble draws at is a property of
        // the whole live set, not of this bubble ([`level_z`]), so the draw waits for the sort.
        pending.push(Pending {
            guid: *guid,
            entity,
            seat,
            viewport,
            // The sort key (`0x4b1251`–`0x4b12c0`): |camera − speaker|², the CAMERA's distance,
            // not the local player's (which is the eligibility gate's, above).
            cam_dist_sq: cam_pose.translation.distance_squared(tf.translation),
            anchor: seat_world,
            unit_pos: tf.translation,
        });
    }
    for g in dead {
        bubbles.0.remove(&g);
    }

    // ── The frame-level pass (`0x4b12d5`–`0x4b1312`) ─────────────────────────────────────────
    // The reference re-sorts its live-bubble list by camera distance, FARTHEST at the head, and
    // then walks head→tail stamping `SetFrameLevel(2), (3), (4)…` — so every bubble owns a frame
    // level of its own and the NEAREST speaker's bubble ends up on top. (It only re-sorts when
    // the camera moved or a bubble is dirty; the stamping walk runs unconditionally. Sorting
    // every frame is the same list with no state to keep.) Ties break on guid so an exact
    // distance tie can't flip the stack frame to frame.
    stack_sort(&mut pending);
    let (Some((_, cam_pose)), Some(atlas), Some(art)) = (cam, atlas.as_deref_mut(), art) else {
        return; // nothing reached `pending` either — the same gate refused it in the tick
    };
    for (rank, p) in pending.iter().enumerate() {
        let Some(b) = bubbles.0.get(&p.guid) else {
            continue; // despawned between the tick and the draw
        };
        draw_bubble(
            atlas,
            &mut quads,
            art,
            b,
            p.seat,
            p.viewport,
            scale,
            trace,
            p.entity,
            p.anchor,
            p.unit_pos,
            cam_pose,
            level_z(rank),
        );
    }
}

/// The stack order: **farthest camera distance first**, so the enumeration index that follows is
/// the reference's frame level and the nearest speaker's bubble draws last (on top). Ties break on
/// guid — the reference's list keeps its previous order across an exact tie because it re-inserts
/// only ahead of a *strictly* closer node, and a `HashMap` walk has no such memory.
fn stack_sort(pending: &mut [Pending]) {
    pending.sort_by(|a, b| {
        b.cam_dist_sq
            .total_cmp(&a.cam_dist_sq)
            .then(a.guid.cmp(&b.guid))
    });
}

/// One bubble that passed the tick and projected on screen, waiting for the frame-level sort.
struct Pending {
    guid: u64,
    entity: Entity,
    seat: Vec2,
    viewport: Vec2,
    /// |camera − speaker|², the reference's own sort key (`[bubble+0x350]`).
    cam_dist_sq: f32,
    /// The world seat and the speaker's position — the `bub` jitter trace's decomposition.
    anchor: Vec3,
    unit_pos: Vec3,
}

/// Append one bubble's draw list: the Backdrop pieces (bg fill inset by the border-unit +
/// the 8-piece edge), the tail square, and the wrapped, centered, chatType-colored text.
fn draw_bubble(
    atlas: &mut UiFontAtlas,
    quads: &mut UiQuads,
    art: &BubbleArt,
    b: &Bubble,
    seat: Vec2,
    viewport: Vec2,
    scale: f32,
    trace: bool,
    // The `bub` jitter-decomposition trace's inputs — see the tail of this function.
    entity: Entity,
    anchor: Vec3,
    unit_pos: Vec3,
    cam_pose: &Transform,
    // This bubble's frame level, flattened to a z base ([`level_z`]).
    z: u64,
) {
    let basis = plate_basis(viewport);
    let border = border_px(viewport, basis);
    let margin = gx_px(MARGIN, basis);

    // The exact window-derived em, laid out AT that em. This used to shape at the nearest baked
    // ladder size and rescale the finished quads by `px/shaped` around the box centre — a bubble's
    // size is derived from the viewport and so lands on nothing round, which meant every bubble on
    // screen was a resampled bitmap. Since decision 1342 the raster follows the request.
    let px = text_px(TEXT_H, basis);
    if px <= 0.0 {
        return;
    }
    let mut e = atlas.lock();
    let spec = FontSpec {
        path: None, // NAMEPLATE_FONT — Friz Quadrata, the engine's default face
        height: Some(px),
        outline: Outline::None,
        alpha_gradient: None,
    };
    // The wrap law (`0x4b1600` tail): single-line width > 0.2 gx → hard-cap (forces wrap);
    // else max(measured, 2·border-unit).
    let cap = gx_px(WRAP_W, basis);
    let floor = 2.0 * border;
    let (line_w, _) = measure_text(&mut e, &b.text, None, spec);
    let box_w = if line_w > cap { cap } else { line_w.max(floor) };
    let (_, box_h) = measure_text(&mut e, &b.text, Some(box_w), spec);
    let (text_w, text_h) = (box_w.ceil(), box_h.ceil());

    // The auto-fit body: the frame hugs the text layout ± the flat margin; BOTTOM-center on
    // the seat, growing upward. Snapped onto the **device** pixel grid (the plate divergence —
    // a fractional corner bilinear-smears the border art), which is [`device_snap`], the plate's
    // own law, not the plain logical `round()` this shipped with: the quad lane is logical px, so
    // rounding there quantized the bubble to `scale_factor` PHYSICAL pixels — two pixels of
    // stepping per axis on the 2× display we play on, against a world sliding continuously
    // underneath, and no texel alignment at all at a fractional scale. It is the same bug the
    // plate was carrying and the same fix; the bubble is simply the site that never got it (1398).
    let w = text_w + 2.0 * margin;
    let h = text_h + 2.0 * margin;
    let origin = seat_origin(seat, w, scale);
    let (left, bottom) = (origin.x, origin.y);
    let frame = Rect::new(left, bottom - h, left + w, bottom);
    let alpha = b.alpha;

    // The Backdrop construct (`0x4b0a35 → 0x76a5d0`): one value feeds the 4 insets and both
    // edge-size fields. The engine's `pieces` speaks y-up — negate y across the seam.
    let bd = Backdrop {
        bg_file: Some(BG_TEXTURE.to_string()),
        edge_file: Some(EDGE_TEXTURE.to_string()),
        tile: false,
        tile_size: 0.0,
        edge_size: border,
        insets: Insets {
            left: border,
            right: border,
            top: border,
            bottom: border,
        },
        bg_color: [1.0; 4],
        border_color: [1.0; 4],
    };
    let up = GxRect::new(-frame.max.y, frame.min.x, -frame.min.y, frame.max.x);
    for p in pieces(up, &bd) {
        // Every bubble piece is axis-aligned (equal insets make the BR-inset quirk vacuous):
        // corners [TL,TR,BR,BL] y-up → a y-down rect from TL/BR, UVs riding their corners.
        let rect = Rect::new(
            p.corners[0][0],
            -p.corners[0][1],
            p.corners[2][0],
            -p.corners[2][1],
        );
        // The border pieces share one 256×32 atlas; without the half-texel inset, bilinear at each
        // piece's own edge blends in the neighbour's first column — and the column beside the TOP
        // slice is WHITE at alpha 0, which is the pale line that ran through the bubble (1402). The
        // bg is its own texture and keeps its UVs.
        let uvs = if p.is_bg {
            p.uvs
        } else {
            pieces_inset(p.uvs, art.edge_texels)
        };
        quads.overlays.push(UiQuad {
            rect,
            z_key: z + if p.is_bg { Z_BG } else { Z_EDGE },
            texture: Some(if p.is_bg {
                art.bg.clone()
            } else {
                art.edge.clone()
            }),
            uv: UvRect::from_corners(uvs),
            color: [1.0, 1.0, 1.0, alpha],
            ..default()
        });
    }
    // The tail (`0x4b0af1`): a border-unit square, TOPRIGHT on the frame's BOTTOM lifted
    // border/4 INTO the body — the overlap that makes the seam read continuous.
    let tail_top = frame.max.y - border * 0.25;
    let cx = (frame.min.x + frame.max.x) * 0.5;
    quads.overlays.push(UiQuad {
        rect: Rect::new(cx - border, tail_top, cx, tail_top + border),
        z_key: z + Z_TAIL,
        texture: Some(art.tail.clone()),
        uv: UvRect::FULL,
        color: [1.0, 1.0, 1.0, alpha],
        ..default()
    });
    // The text: centered in the margin box, wrapped at the cap. No rescale pass — the glyphs came
    // out of the cache at this em.
    let center = Vec2::new(cx, (frame.min.y + frame.max.y) * 0.5);
    let mut text_quads = layout_text_quads(
        &mut e,
        &b.text,
        Rect::from_center_size(center, Vec2::new(box_w, box_h)),
        [b.color[0], b.color[1], b.color[2], alpha],
        Justify {
            h: JustifyH::Center,
            v: JustifyV::Middle,
        },
        z + Z_TEXT,
        spec,
        // The bubble frame seats on the DEVICE pixel grid ([`device_snap`], 1398) and slides with
        // the speaker; its text must be rigid against it. Snapping the block top on the UI's
        // logical grid instead made the text pop a whole px every second step the frame took —
        // the V-plate's defect at the sibling site, exactly as 1398 found the snap itself missing
        // here. Unlike the plate this rect is real (a margin box), so the degenerate-rect
        // carve-out never covered it.
        TextSeat::Exact,
    );
    drop(e);
    if trace {
        eprintln!(
            "bubble-trace: viewport={viewport:?} frame=({:.1},{:.1})..({:.1},{:.1}) border={border:.1} alpha={alpha:.2} text={:?}",
            frame.min.x, frame.min.y, frame.max.x, frame.max.y, b.text
        );
    }
    // **The jitter decomposition** (`WOW_MOVE_TRACE` tag `bub`, one line per bubble per frame) —
    // the plate's `vpl` line ([`crate::vplates`]) for the bubble: the seat's world point, the
    // camera pose that projected it, the raw projected seat, and the snapped frame origin. "The
    // bubble is jittery when running" is then attributed from numbers rather than from the eye —
    // a moving anchor, a noisy camera, or the pixel snap. Since 1406 `anchor=` and `pos=` differ by
    // a LATCHED constant on Y and by nothing at all on X/Z, so any wobble between them is a defect
    // by construction. (Through 1398 they were read on different clocks — the anchor computed
    // through a `GlobalTransform` Bevy propagates in `PostUpdate`, i.e. last frame's — and their
    // difference measured exactly that lag: the term 1341 cleared for the plate by measuring a unit
    // that was STANDING STILL, where it is identically zero. Dropping the pose read removes the
    // seam rather than correcting it.) It rides the shared tag-filtered trace and NOT the `WOW_BUBBLE_TRACE`
    // eprintln beside it, whose unbuffered writes would distort the frame pacing the question is
    // about (the 0880 lesson, the same reason `vpl` lives there).
    if benilla_assets::trace::enabled_for("bub") {
        let (cp, cf) = (cam_pose.translation, cam_pose.forward());
        benilla_assets::trace::line(
            "bub",
            &format!(
                "e={} vp=({:.0},{:.0}) anchor=[{:.4},{:.4},{:.4}] cam=[{:.4},{:.4},{:.4}] \
                 fwd=[{:.4},{:.4},{:.4}] pos=[{:.4},{:.4},{:.4}] scr=({:.3},{:.3}) \
                 frame=({:.2},{:.2}) scale={scale:.2}",
                entity.index(),
                viewport.x,
                viewport.y,
                anchor.x,
                anchor.y,
                anchor.z,
                cp.x,
                cp.y,
                cp.z,
                cf.x,
                cf.y,
                cf.z,
                unit_pos.x,
                unit_pos.y,
                unit_pos.z,
                seat.x,
                seat.y,
                frame.min.x,
                frame.max.y,
            ),
        );
    }
    quads.overlays.append(&mut text_quads);
}

/// The bubble stage — [`crate::nameplates`] orders after it (the bubble/name exclusion reads
/// [`BubblesActive`]), exactly as it orders after [`VPlateSet`] for the plate verdict.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BubbleSet;

pub(crate) struct ChatBubblePlugin;

/// The two bubble switches' change callback (1139, 2303): flags, like every other pair.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut bubbles: ResMut<BubbleConfig>) {
    match ev.key().as_str() {
        "chatbubbles" => bubbles.all = ev.flag(),
        "chatbubblesparty" => bubbles.party = ev.flag(),
        _ => {}
    }
}

impl Plugin for ChatBubblePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<BubbleQueue>()
            .init_resource::<BubbleConfig>()
            .init_resource::<Bubbles>()
            .init_resource::<BubblesActive>()
            .add_systems(Startup, load_bubble_art.after(AssetSet::Open))
            // After the V-plate drive (the spawn gate reads this frame's plate verdict),
            // inside the UI-quad append window; the name driver orders after [`BubbleSet`]
            // (the bubble/name exclusion reads this frame's verdict).
            .add_systems(
                Update,
                drive_bubbles
                    .after(VPlateSet)
                    .in_set(UiQuadAppend)
                    .in_set(BubbleSet),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word counter is a space/tab-run law — not Unicode whitespace, not collapse-free.
    #[test]
    fn word_count_is_the_space_tab_run_law() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   \t "), 0);
        assert_eq!(word_count("hi"), 1);
        assert_eq!(word_count("  hi   there\tfriend  "), 3);
        assert_eq!(word_count("a\u{a0}b"), 1, "NBSP is not a separator");
    }

    /// The duration bytes: others 2750 + 750·(n−1), self 1500 + 500·(n−1), empty 0.
    #[test]
    fn duration_matches_the_byte_law() {
        assert_eq!(duration_secs(0, false), 0.0);
        assert_eq!(duration_secs(1, false), 2.75);
        assert_eq!(duration_secs(4, false), 5.0);
        assert_eq!(duration_secs(1, true), 1.5);
        assert_eq!(duration_secs(3, true), 2.5);
    }

    /// Bubble text is plain: color/hyperlink escapes strip (display text stays), `||` stays
    /// escaped for the downstream markup parser, a dangling pipe renders literally.
    #[test]
    fn sanitize_strips_escapes_keeps_display_text() {
        assert_eq!(sanitize("hello"), "hello");
        assert_eq!(sanitize("a||b"), "a||b");
        assert_eq!(sanitize("|cffff0000red|r plain"), "red plain");
        assert_eq!(
            sanitize("look |Hitem:19019|h[Thunderfury]|h!"),
            "look [Thunderfury]!"
        );
        assert_eq!(sanitize("dangling |"), "dangling ||");
        assert_eq!(sanitize("|x odd"), "||x odd");
    }

    /// The v1 kind set: say/yell + monster say/yell on `ChatBubbles`, party on
    /// `ChatBubblesParty`; everything else out pending the OPEN remap capture. And the two
    /// switches are genuinely separate: the client gates party lines on their own CVar, so
    /// turning one off leaves the other bubbling.
    ///
    /// The sweep runs on a config with **both switches planted on**, which is not what a fresh
    /// client boots at: `ChatBubblesParty` shipped `"1"` from 0598's `/p` ask until 1804 put both
    /// defaults back on the binary's (`ChatBubbles` `"1"`, `ChatBubblesParty` `"0"`). Planted,
    /// because the question here is which *kinds* the gate admits, and a kind whose switch is off
    /// cannot answer it. The shipped pair is asserted below, where it is exactly the
    /// party-off half of the independence check.
    #[test]
    fn the_kind_set_is_the_uncontested_v1() {
        use ChatEventKind as K;
        let on = BubbleConfig {
            all: true,
            party: true,
        };
        for k in [K::Say, K::Yell, K::MonsterSay, K::MonsterYell] {
            assert_eq!(bubble_cvar(k, &on), Some(true));
        }
        assert_eq!(
            bubble_cvar(K::Party, &on),
            Some(true),
            "party on its own CVar"
        );
        for k in [
            K::Guild,
            K::Officer,
            K::Whisper,
            K::Emote,
            K::TextEmote,
            K::System,
            K::Channel,
            K::MonsterEmote,
        ] {
            assert_eq!(bubble_cvar(k, &on), None, "{k:?} must not bubble in v1");
        }

        // Party off with say on — which is the SHIPPED pair since 1804, so this half of the
        // independence check is [`BubbleConfig::default`] itself and pins the boot state too.
        let no_party = BubbleConfig::default();
        assert!(no_party.all && !no_party.party, "the shipped pair");
        assert_eq!(
            bubble_cvar(K::Party, &no_party),
            Some(false),
            "/p does not bubble out of the box"
        );
        assert_eq!(
            bubble_cvar(K::Say, &no_party),
            Some(true),
            "say is untouched"
        );
        let no_say = BubbleConfig {
            all: false,
            party: true,
        };
        assert_eq!(bubble_cvar(K::Say, &no_say), Some(false));
        assert_eq!(
            bubble_cvar(K::Party, &no_say),
            Some(true),
            "party has its own switch"
        );
    }

    /// The seat snaps on the DEVICE grid, not the logical one — the plate's law
    /// ([`device_snap`]), which the bubble shipped without. At the 2× display we play on, a
    /// logical `round()` moved the bubble two physical pixels per axis against a world that
    /// slides continuously; this moves it one, the smallest step that still lands the border
    /// blit on a texel boundary. Pinned so `scale` can't be "simplified" back out into `round()`.
    #[test]
    fn the_bubble_seat_snaps_on_the_device_grid() {
        // 2×: the grid is every half logical pixel, and every snapped edge is a whole physical px.
        for (seat_y, want) in [(10.0, 10.0), (10.2, 10.0), (10.3, 10.5), (10.6, 10.5)] {
            let o = seat_origin(Vec2::new(100.0, seat_y), 40.0, 2.0);
            assert_eq!(o.y, want, "seat y {seat_y} at 2×");
            assert_eq!(
                (o.y * 2.0).fract(),
                0.0,
                "{} is a whole physical pixel",
                o.y
            );
        }
        // The x half is the same law applied to the CENTERED left edge (seat − w/2), so an odd
        // width still lands the left edge — the one the border blit starts from — on the grid.
        let o = seat_origin(Vec2::new(100.4, 0.0), 41.0, 2.0);
        assert_eq!(o.x, 80.0);
        assert_eq!(((100.4_f32 - 20.5) * 2.0).round() / 2.0, o.x);
        // 1×: identical to the logical round() it replaces — the fix costs nothing at scale 1.
        assert_eq!(seat_origin(Vec2::new(0.0, 10.4), 0.0, 1.0).y, 10.0);
        assert_eq!(seat_origin(Vec2::new(0.0, 10.6), 0.0, 1.0).y, 11.0);
        // 1.5× (the Windows norm), where a logical round() was never texel-aligned at all.
        for v in [10.4, 10.9, 11.2] {
            let o = seat_origin(Vec2::new(0.0, v), 0.0, 1.5);
            assert_eq!(
                (o.y * 1.5).fract(),
                0.0,
                "{v} at 1.5× is a whole physical px"
            );
        }
    }

    /// The snap must not be a *quantizer with a large step*: over a continuous glide, the extra
    /// displacement it adds to any one frame is bounded by half a device pixel. This is the
    /// property the jitter report is about — at 2× the old logical round() allowed a whole
    /// logical pixel (two physical) of extra step, which is what read as judder.
    #[test]
    fn the_snap_adds_at_most_half_a_device_pixel_of_step() {
        let scale = 2.0;
        let mut worst: f32 = 0.0;
        // A continuous glide across ~40 px at a fractional per-frame speed, like a run.
        for i in 0..400 {
            let seat = 137.317 + 0.1013 * i as f32;
            let snapped = seat_origin(Vec2::new(0.0, seat), 0.0, scale).y;
            worst = worst.max((snapped - seat).abs());
        }
        assert!(
            worst <= 0.5 / scale + f32::EPSILON,
            "snap displacement {worst} exceeds half a device pixel"
        );
    }

    /// Every bubble owns a frame LEVEL: its four pieces are one contiguous band, bands ascend
    /// with rank, and no two bubbles can interleave. This is the whole 1504 fix — with a flat
    /// per-piece key, every bubble's background painted under every bubble's text, so a speaker's
    /// line bled across a neighbour's card.
    #[test]
    fn each_bubble_gets_its_own_level_band() {
        // The four pieces of one bubble, in paint order, all inside one level.
        const { assert!(Z_BG < Z_EDGE && Z_EDGE < Z_TAIL && Z_TAIL < Z_TEXT) };
        const { assert!(Z_TEXT < overlay_z::BUBBLE_STRIDE) };
        for rank in 0..8usize {
            // ...and every piece strictly below the NEXT bubble's background.
            assert!(level_z(rank) + Z_TEXT < level_z(rank + 1) + Z_BG);
        }
        assert!(level_z(1) > level_z(0), "later rank draws later");
    }

    /// The band is bounded: however many bubbles are live, none of them can reach the V-plates'
    /// band above ([`crate::ui_pass::overlay_z`]) or the floating combat text's below.
    #[test]
    fn the_bubble_band_stays_between_its_neighbours() {
        for rank in [0, 1, 1_000, usize::MAX] {
            let z = level_z(rank);
            assert!(z >= overlay_z::BUBBLE, "over the floating numbers");
            assert!(z + Z_TEXT < overlay_z::VPLATE, "under the V-plates");
        }
    }

    /// The stack order is the reference's: farthest camera distance first (so the nearest
    /// speaker's bubble ends up on top), guid breaking an exact tie deterministically.
    #[test]
    fn the_stack_is_sorted_farthest_first() {
        let at = |guid: u64, d2: f32| Pending {
            guid,
            entity: Entity::PLACEHOLDER,
            seat: Vec2::ZERO,
            viewport: Vec2::ONE,
            cam_dist_sq: d2,
            anchor: Vec3::ZERO,
            unit_pos: Vec3::ZERO,
        };
        let mut p = vec![at(7, 25.0), at(3, 400.0), at(9, 1.0), at(2, 400.0)];
        stack_sort(&mut p);
        assert_eq!(
            p.iter().map(|q| q.guid).collect::<Vec<_>>(),
            vec![2, 3, 7, 9],
            "farthest first; the 400-yd² pair ordered by guid"
        );
        // The rank the draw uses IS the frame level: the nearest speaker draws last.
        assert!(level_z(0) < level_z(p.len() - 1));
    }

    /// The border-unit lands the byte constants: 16 px at 1024-wide 4:3 (G44·16/(S·1024)
    /// nets width/64), damped past the plate knee like every other size.
    #[test]
    fn border_unit_is_a_64th_of_the_width() {
        let vp = Vec2::new(1024.0, 768.0);
        assert_eq!(border_px(vp, plate_basis(vp)), 16.0);
        let wide = Vec2::new(2560.0, 1440.0);
        let b = border_px(wide, plate_basis(wide));
        assert!(
            b > 16.0 && b < 40.0,
            "damped growth between the native pin and the faithful 40 px, got {b}"
        );
    }
}
