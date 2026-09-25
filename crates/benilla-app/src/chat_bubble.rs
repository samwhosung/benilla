//! Chat bubbles, `CGChatBubbleFrame`: the speech bubble a chat line spawns over the speaker, a
//! 2-D overlay like the V-plate ([`crate::vplates`]).
//!
//! - Spawn (`0x608ac0`, from the `SMSG_MESSAGECHAT` display path): the sender resolves to a live
//!   unit, its CVar is on, the text is non-empty, the unit carries no V-plate (`0x608adc`; a live
//!   bubble in turn hides the overhead name, [`BubblesActive`]), and the local player is within
//!   20 yd (3-D). Self is not excluded. A new line replaces the old bubble, never queues.
//! - Anchor (`0x4b0c30`): unit z + height × model scale + 0.7, bottom-seated, growing upward. The
//!   height is the Stand sequence box's Z extent from the model header (`0x711a20`), not the posed
//!   attachment the overhead name reads (`0x608640`), latched once per chat line.
//! - Stacking (`0x4b1060`): every frame the bubbles sort farthest-first by camera distance and
//!   take frame levels 2, 3, 4…, so whole cards stack with the nearest speaker's on top.
//!
//! Only say, yell, party and monster say/yell bubble: the reference's gate is "sender resolves",
//! but the wire-type remap (`0x49a870`) that would admit other kinds is untraced.
//!
//! Deviation: sizes ride the plates' damped diagonal basis ([`plate_basis`]), not the unbounded
//! reference law, so bubble text and plate text keep one em at every window size.
//!
//! Not built: the reference also fades a bubble whose speaker's model data is not resident or whose
//! scene-entity flag is clear (`0x7103d0`/`0x6704c0`); only distance is re-tested here.

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

/// The two bubble CVars (registrar `0x603280`), booting at the reference's defaults.
#[derive(Resource)]
pub(crate) struct BubbleConfig {
    /// `ChatBubbles`: say, yell and their monster variants.
    pub(crate) all: bool,
    /// `ChatBubblesParty`: party lines, which the reference gates separately.
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

/// The bubble art (`0x4b0940`).
const BG_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Background";
const EDGE_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Backdrop";
const TAIL_TEXTURE: &str = "Interface\\Tooltips\\ChatBubble-Tail";

/// The 0.7 yd lift over the anchor height (`[0x7ffd7c]`).
const LIFT: f32 = 0.7;
/// The spawn and per-frame range gate, 20 yd squared (`[0x806798]`, the plates' too).
const MAX_DIST_SQ: f32 = 400.0;
/// The 250 ms linear fade, in and out (`0x4b0ea0`/`0x4b0ee0`).
const FADE_SECS: f32 = 0.25;
/// `NAMEPLATE_FONT` at 0.01 gx, the plate name's em.
const TEXT_H: f32 = 0.01;
/// The body margin around the text layout, all four sides (`0x3c23d70a`).
const MARGIN: f32 = 0.01;
/// The text wrap hard cap, gx (`[0x80679c]` = 0.2).
const WRAP_W: f32 = 0.2;
/// The border unit (edge size, insets, tail side): 16/1024 of the screen width.
const BORDER_FRAC: f32 = 16.0 / 1024.0;

/// Paint order inside one bubble's level: the tail covers the bottom edge piece and the text draws
/// last, as the reference's batch drains textures before font strings (`0x76fb00`).
const Z_BG: u64 = 0;
const Z_EDGE: u64 = 1;
const Z_TAIL: u64 = 2;
const Z_TEXT: u64 = 3;

/// The z base for the bubble at `rank`, farthest first: the reference's per-frame
/// `SetFrameLevel(2 + i)` walk (`0x4b12d5`-`0x4b1312`), one level per bubble, under the V-plates.
fn level_z(rank: usize) -> u64 {
    let level = (rank as u64).min(overlay_z::BUBBLE_MAX_LEVEL);
    overlay_z::BUBBLE + level * overlay_z::BUBBLE_STRIDE
}

/// The CVar gating this kind's bubble (`0x608b0d`); `None` for a kind that does not bubble.
fn bubble_cvar(kind: ChatEventKind, cfg: &BubbleConfig) -> Option<bool> {
    use ChatEventKind as K;
    match kind {
        K::Party => Some(cfg.party),
        K::Say | K::Yell | K::MonsterSay | K::MonsterYell => Some(cfg.all),
        _ => None,
    }
}

/// The word count (`0x4b1810`): maximal runs of characters other than space and tab.
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

/// The lifetime (`0x4b1810`): `base + perWord·(words−1)` ms, 2750/750 for others and 1500/500
/// for the local player.
fn duration_secs(words: u32, is_self: bool) -> f32 {
    if words == 0 {
        return 0.0;
    }
    let (base, per) = if is_self { (1500, 500) } else { (2750, 750) };
    (base + per * (words - 1)) as f32 / 1000.0
}

/// Bubble text is plain, coloured only by chat type: colour escapes and hyperlink wrappers strip
/// (their display text stays), and `||` stays escaped for the glyph layout's markup parser.
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
                // Skip the opener through its `|h`; the closing `|h` drops in the arm below.
                i += 2;
                while i < b.len() && !(b[i] == b'|' && b.get(i + 1) == Some(&b'h')) {
                    i += 1;
                }
                i += 2;
            }
            Some(b'h') => i += 2,
            // A dangling or unknown escape renders as a literal pipe.
            _ => {
                out.push_str("||");
                i += 1;
            }
        }
    }
    out
}

/// The bleed inset for one border piece; zero texels (no patch chain) leaves the UVs alone.
fn pieces_inset(uvs: [[f32; 2]; 4], texels: Vec2) -> [[f32; 2]; 4] {
    if texels.x <= 0.0 || texels.y <= 0.0 {
        return uvs;
    }
    inset_atlas_bleed(uvs, texels.x, texels.y)
}

/// The frame's bottom-left origin, bottom-center on the seat. Deviation: snapped onto the device
/// pixel grid ([`device_snap`]) so the border art blits 1:1; the reference seats it fractionally.
fn seat_origin(seat: Vec2, w: f32, scale: f32) -> Vec2 {
    Vec2::new(
        device_snap(seat.x - w * 0.5, scale),
        device_snap(seat.y, scale),
    )
}

/// The border unit in logical px: 16/1024 of the screen width, in the damped size basis.
fn border_px(viewport: Vec2, basis: f32) -> f32 {
    let width_gx = viewport.x / viewport.length();
    gx_px(width_gx * BORDER_FRAC, basis).max(1.0)
}

/// This frame's bubble-spawn requests, pushed as the chat line routes.
#[derive(Resource, Default)]
pub(crate) struct BubbleQueue(Vec<(u64, ChatEventKind, String)>);

impl BubbleQueue {
    /// Queue a routed line; kind, CVar and sender filter here, unit, plate and range in the driver.
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

/// One live bubble: the `CGChatBubbleFrame` and its `CGUnit+0xe64` handle.
struct Bubble {
    /// The [`sanitize`]d display text.
    text: String,
    /// The chat-type colour (the 94-entry table), sRGB 0..1.
    color: [f32; 3],
    /// `Time::elapsed_secs` at spawn.
    born: f32,
    /// The steady window after the fade-in ([`duration_secs`]).
    duration: f32,
    /// Stand-box height × model scale, latched at spawn as the reference does (`bubble+0x354`).
    lift: f32,
    /// Fade alpha 0..1, ramped toward the eligibility verdict.
    alpha: f32,
}

/// The live bubbles by speaker guid, at most one per unit.
#[derive(Resource, Default)]
struct Bubbles(HashMap<u64, Bubble>);

/// Units with a live bubble this frame, whose overhead name is suppressed (`+0xe64` ≠ 0).
#[derive(Resource, Default)]
pub(crate) struct BubblesActive(pub(crate) EntityHashSet);

/// The bubble art, loaded at boot. The edge strip tiles UVs past 1 (repeat); bg and tail clamp.
#[derive(Resource)]
struct BubbleArt {
    bg: Handle<Image>,
    edge: Handle<Image>,
    tail: Handle<Image>,
    /// The edge atlas's size in texels, for the half-texel bleed inset.
    edge_texels: Vec2,
}

fn load_bubble_art(
    mut commands: Commands,
    assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut assets) = assets else {
        return; // no game data (a bare test app); `drive_bubbles` tolerates the missing art
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

/// Every frame: drain the queue through the spawn gate (`0x608ac0`), ramp each bubble's fade
/// against its lifetime and the 20 yd re-test (`0x4b0c30`), publish [`BubblesActive`], and draw.
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
    heights: Query<&StandBoxHeight>,
    time: Res<Time>,
    // The device scale, for the seat's pixel snap.
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    active.0.clear();
    let now = time.elapsed_secs();
    let step = time.delta_secs() / FADE_SECS;
    let self_tf = self_q.single().ok();
    // The typed sender lookup (`0x468460`), a linear scan of the small streamed set.
    let find = |guid: u64| units.iter().find(|(_, g, _)| g.0 == guid);

    // ── The spawn/replace gate (`0x608ac0`) ─────────────────────────────────────────────────
    // Every refusal is traced under the `bub` tag.
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
            continue; // no words, no lifetime, no bubble
        }
        let Some((entity, _, tf)) = find(guid) else {
            refuse("sender-not-a-live-unit", guid);
            continue;
        };
        // An active V-plate blocks the bubble (`0x608adc`): a plated hostile's yell shows none.
        if vplates.0.contains(&entity) {
            refuse("v-plate-on-speaker", guid);
            continue;
        }
        let Some(self_tf) = self_tf else {
            refuse("no-local-player", guid);
            continue;
        };
        let d2 = (tf.translation - self_tf.translation).length_squared();
        if d2 > MAX_DIST_SQ {
            refuse(&format!("out-of-range dist={:.1}yd", d2.sqrt()), guid);
            continue;
        }
        let is_self = self_guid.0 == Some(guid);
        let c = default_color(kind);
        // The one-time height query (`0x4b0e38` calling `0x711a20`, scaled by `[unit+0x90]`). A
        // model with no bounds reads 0 and the bubble sits 0.7 above the feet, as in the reference.
        let lift = heights.get(entity).map_or(0.0, |h| h.0) * tf.scale.y;
        // Replace, never queue (`0x608c00`): the fresh bubble fades in from 0.
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
            dead.push(*guid); // the speaker despawned, and its bubble with it
            continue;
        };
        if now >= b.born + b.duration + FADE_SECS {
            // The permanent fade-out (`0x4b0ee0(1)`); at 0 the frame recycles.
            b.alpha -= step;
            if b.alpha <= 0.0 {
                dead.push(*guid);
                continue;
            }
        } else {
            // The 20 yd gate, re-tested every frame: out of range fades out, back in fades in.
            let eligible = self_tf
                .is_some_and(|s| (tf.translation - s.translation).length_squared() <= MAX_DIST_SQ);
            b.alpha = if eligible {
                (b.alpha + step).min(1.0)
            } else {
                (b.alpha - step).max(0.0)
            };
        }
        // The name stays suppressed while the bubble exists, faded or not (`+0xe64` ≠ 0).
        active.0.insert(entity);
        if b.alpha <= 0.0 {
            continue;
        }
        // No camera, atlas or art: lifetimes tick, nothing draws.
        let (Some((cam, cam_pose)), true) = (cam, atlas.is_some() && art.is_some()) else {
            continue;
        };
        // The anchor (`0x4b0c30`), from this frame's `Transform` and constants only. A seat behind
        // the camera draws nothing and keeps its state.
        let seat_world = tf.translation + Vec3::Y * (b.lift + LIFT);
        let cam_tf = GlobalTransform::from(*cam_pose);
        let Ok(seat) = cam.world_to_viewport(&cam_tf, seat_world) else {
            refuse("not-on-screen (behind the camera)", *guid);
            continue;
        };
        let Some(viewport) = cam.logical_viewport_size() else {
            refuse("no-viewport", *guid);
            continue;
        };
        // Drawn after the sort: a bubble's level depends on the whole live set ([`level_z`]).
        pending.push(Pending {
            guid: *guid,
            entity,
            seat,
            viewport,
            // The sort key (`0x4b1251`-`0x4b12c0`): the camera's distance, not the player's.
            cam_dist_sq: cam_pose.translation.distance_squared(tf.translation),
            anchor: seat_world,
            unit_pos: tf.translation,
        });
    }
    for g in dead {
        bubbles.0.remove(&g);
    }

    // ── The frame-level pass (`0x4b12d5`–`0x4b1312`) ─────────────────────────────────────────
    // Sorted farthest first (`0x4b1360`), levels stamped head to tail (`0x4b1309`). The reference
    // re-sorts only on a change; sorting every frame gives the same list.
    stack_sort(&mut pending);
    let (Some((_, cam_pose)), Some(atlas), Some(art)) = (cam, atlas.as_deref_mut(), art) else {
        return; // the tick's same gate kept `pending` empty
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

/// Farthest camera distance first, so the index is the frame level. Ties break on guid, as the
/// reference keeps its order across an exact tie and a `HashMap` walk cannot.
fn stack_sort(pending: &mut [Pending]) {
    pending.sort_by(|a, b| {
        b.cam_dist_sq
            .total_cmp(&a.cam_dist_sq)
            .then(a.guid.cmp(&b.guid))
    });
}

/// One projected bubble awaiting the frame-level sort.
struct Pending {
    guid: u64,
    entity: Entity,
    seat: Vec2,
    viewport: Vec2,
    /// |camera − speaker|², the reference's sort key (`[bubble+0x350]`).
    cam_dist_sq: f32,
    /// The world seat and the speaker's position, for the `bub` trace.
    anchor: Vec3,
    unit_pos: Vec3,
}

/// Append one bubble's draw list: the backdrop pieces, the tail and the wrapped, centered text.
fn draw_bubble(
    atlas: &mut UiFontAtlas,
    quads: &mut UiQuads,
    art: &BubbleArt,
    b: &Bubble,
    seat: Vec2,
    viewport: Vec2,
    scale: f32,
    trace: bool,
    // The `bub` trace's inputs.
    entity: Entity,
    anchor: Vec3,
    unit_pos: Vec3,
    cam_pose: &Transform,
    // This bubble's frame level as a z base.
    z: u64,
) {
    let basis = plate_basis(viewport);
    let border = border_px(viewport, basis);
    let margin = gx_px(MARGIN, basis);

    // Shaped at the exact window-derived em, so the glyphs are never a rescaled bitmap.
    let px = text_px(TEXT_H, basis);
    if px <= 0.0 {
        return;
    }
    let mut e = atlas.lock();
    let spec = FontSpec {
        path: None, // `NAMEPLATE_FONT`: Friz Quadrata, the engine's default face
        height: Some(px),
        outline: Outline::None,
        alpha_gradient: None,
    };
    // The wrap law (`0x4b1600`): wider than 0.2 gx wraps at the cap, else max(width, 2 borders).
    let cap = gx_px(WRAP_W, basis);
    let floor = 2.0 * border;
    let (line_w, _) = measure_text(&mut e, &b.text, None, spec);
    let box_w = if line_w > cap { cap } else { line_w.max(floor) };
    let (_, box_h) = measure_text(&mut e, &b.text, Some(box_w), spec);
    let (text_w, text_h) = (box_w.ceil(), box_h.ceil());

    // The frame hugs the text layout plus the margin, bottom-center on the seat, growing upward.
    let w = text_w + 2.0 * margin;
    let h = text_h + 2.0 * margin;
    let origin = seat_origin(seat, w, scale);
    let (left, bottom) = (origin.x, origin.y);
    let frame = Rect::new(left, bottom - h, left + w, bottom);
    let alpha = b.alpha;

    // The backdrop (`0x4b0a35` into `0x76a5d0`): one value feeds the four insets and both
    // edge-size fields. `pieces` is y-up, so y negates across the seam.
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
        // Equal insets keep every piece axis-aligned: a y-down rect from its TL and BR corners.
        let rect = Rect::new(
            p.corners[0][0],
            -p.corners[0][1],
            p.corners[2][0],
            -p.corners[2][1],
        );
        // The border pieces share one 256×32 atlas: without the half-texel inset, bilinear blends
        // in the neighbour's column, white at alpha 0 beside the top slice, a pale line.
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
    // The tail (`0x4b0af1`): a border-unit square, TOPRIGHT on the bottom lifted border/4.
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
    // The text, centered in the margin box and wrapped at the cap.
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
        // Exact, so the text stays rigid against the device-snapped frame.
        TextSeat::Exact,
    );
    drop(e);
    if trace {
        eprintln!(
            "bubble-trace: viewport={viewport:?} frame=({:.1},{:.1})..({:.1},{:.1}) border={border:.1} alpha={alpha:.2} text={:?}",
            frame.min.x, frame.min.y, frame.max.x, frame.max.y, b.text
        );
    }
    // The jitter decomposition (`WOW_MOVE_TRACE` tag `bub`): `anchor=` and `pos=` differ by a
    // latched constant on Y only, so a wobble between them is a defect. Not the eprintln above,
    // whose unbuffered writes would distort frame pacing.
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

/// The bubble stage; [`crate::nameplates`] orders after it to read [`BubblesActive`].
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BubbleSet;

pub(crate) struct ChatBubblePlugin;

/// The two bubble CVars' change callback.
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
            // After the V-plate drive, whose verdict the spawn gate reads.
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

    /// Space and tab runs only, not Unicode whitespace.
    #[test]
    fn word_count_is_the_space_tab_run_law() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   \t "), 0);
        assert_eq!(word_count("hi"), 1);
        assert_eq!(word_count("  hi   there\tfriend  "), 3);
        assert_eq!(word_count("a\u{a0}b"), 1, "NBSP is not a separator");
    }

    #[test]
    fn duration_matches_the_byte_law() {
        assert_eq!(duration_secs(0, false), 0.0);
        assert_eq!(duration_secs(1, false), 2.75);
        assert_eq!(duration_secs(4, false), 5.0);
        assert_eq!(duration_secs(1, true), 1.5);
        assert_eq!(duration_secs(3, true), 2.5);
    }

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

    /// Both switches on, since a kind whose switch is off cannot show whether the gate admits it.
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

        // Party off with say on is the default pair, so this also pins the boot state.
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

    /// A logical `round()` would step two physical pixels per axis at 2×.
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
        // The x half snaps the centered left edge (seat − w/2), where the border blit starts.
        let o = seat_origin(Vec2::new(100.4, 0.0), 41.0, 2.0);
        assert_eq!(o.x, 80.0);
        assert_eq!(((100.4_f32 - 20.5) * 2.0).round() / 2.0, o.x);
        // 1×: identical to a logical round().
        assert_eq!(seat_origin(Vec2::new(0.0, 10.4), 0.0, 1.0).y, 10.0);
        assert_eq!(seat_origin(Vec2::new(0.0, 10.6), 0.0, 1.0).y, 11.0);
        // 1.5×, where a logical round() is never texel-aligned.
        for v in [10.4, 10.9, 11.2] {
            let o = seat_origin(Vec2::new(0.0, v), 0.0, 1.5);
            assert_eq!(
                (o.y * 1.5).fract(),
                0.0,
                "{v} at 1.5× is a whole physical px"
            );
        }
    }

    /// Over a continuous glide the snap displaces any one frame by at most half a device pixel.
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

    /// One contiguous band per bubble, ascending with rank, so no two bubbles interleave.
    #[test]
    fn each_bubble_gets_its_own_level_band() {
        // The four pieces of one bubble, in paint order, all inside one level.
        const { assert!(Z_BG < Z_EDGE && Z_EDGE < Z_TAIL && Z_TAIL < Z_TEXT) };
        const { assert!(Z_TEXT < overlay_z::BUBBLE_STRIDE) };
        for rank in 0..8usize {
            // ...and every piece strictly below the next bubble's background.
            assert!(level_z(rank) + Z_TEXT < level_z(rank + 1) + Z_BG);
        }
        assert!(level_z(1) > level_z(0), "later rank draws later");
    }

    /// However many bubbles are live, none reaches the V-plates above or the combat text below.
    #[test]
    fn the_bubble_band_stays_between_its_neighbours() {
        for rank in [0, 1, 1_000, usize::MAX] {
            let z = level_z(rank);
            assert!(z >= overlay_z::BUBBLE, "over the floating numbers");
            assert!(z + Z_TEXT < overlay_z::VPLATE, "under the V-plates");
        }
    }

    /// Farthest camera distance first, guid breaking an exact tie.
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
        // The rank the draw uses is the frame level: the nearest speaker draws last.
        assert!(level_z(0) < level_z(p.len() - 1));
    }

    /// 16 px at 1024-wide 4:3 (width/64), damped past the plate knee.
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
