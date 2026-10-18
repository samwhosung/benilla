//! Chat bubbles, `CGChatBubbleFrame`: the speech bubble a chat line spawns over the speaker. This
//! file owns the world side (gate, lifetime, fade, anchor, projection, stacking order, sizes); the
//! widgets, real `WorldFrame` children an addon can skin, live in
//! [`benilla_ui::script::chat_bubble`].
//!
//! - Spawn (`0x608ac0`, from the `SMSG_MESSAGECHAT` display path): the sender resolves to a live
//!   unit, its CVar is on, the text is non-empty, the unit carries no V-plate (`0x608adc`; a live
//!   bubble in turn hides the overhead name, [`BubblesActive`]), and the local player is within
//!   20 yd (3-D). Self is not excluded. A new line replaces the old bubble, never queues.
//! - Lifetime and fade (`0x4b0c30`): a 250 ms fade in, the line's [`duration_ms`], a 250 ms fade
//!   out, then the frame goes back to the pool. Out of range, or with the speaker not drawn in the
//!   last render ([`speaker_drawn`]), it fades out and hides, and fades back in on return.
//! - Anchor (`0x4b0c30`): unit z + height × model scale + 0.7, the text's BOTTOM on the projected
//!   point (`0x4b157a`), growing upward. The height is the Stand sequence box's Z extent from the
//!   model header (`0x711a20`), not the posed attachment the overhead name reads (`0x608640`),
//!   latched once per chat line.
//! - Stacking (`0x4b1060`): every frame the bubbles sort farthest-first by camera distance and
//!   take frame levels 2, 3, 4…, so whole cards stack with the nearest speaker's on top.
//!
//! Only say, yell, party and monster say/yell bubble: the reference's gate is "sender resolves",
//! but the wire-type remap (`0x49a870`) that would admit other kinds is untraced.
//!
//! Deviation: sizes ride the plates' damped diagonal basis ([`plate_basis`]), not the unbounded
//! reference law, so bubble text and plate text keep one em at every window size.
//!
//! Not built: the reference also ends a bubble whose speaker's model data is not resident
//! (`0x7103d0`); here a speaker with no model yet only counts as not drawn.

use std::collections::HashMap;

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use benilla_ui::script::{BubbleGeometry, BubbleState};

use crate::entities::StandBoxHeight;
use crate::net::{Embodied, Guid, NetEntity, SelfGuid};
use crate::ui_chat::{default_color, ChatEventKind};
use crate::vplates::{gx_px, plate_basis, text_px, VPlateSet, VPlates};
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

/// The 0.7 yd lift over the anchor height (`[0x7ffd7c]`).
const LIFT: f32 = 0.7;
/// The range gate, 20 yd squared (`[0x806798]`, the plates' too): a spawn passes at or under it
/// (`0x608ba0`), a live bubble stays in range only under it (`0x4b0cd2`).
const MAX_DIST_SQ: f32 = 400.0;
/// The linear fade, in and out, in ms (`0x4b0d3e`, `0x4b0d78`).
const FADE_MS: u64 = 250;
/// `NAMEPLATE_FONT` at 0.01 gx (`0x4b0b21`), the plate name's em.
const TEXT_H: f32 = 0.01;
/// The frame's margin around the text, all four sides (`0x3c23d70a`).
const MARGIN: f32 = 0.01;
/// The text wrap hard cap, gx (`[0x80679c]` = 0.2).
const WRAP_W: f32 = 0.2;
/// The border unit (edge size, insets, tail side): 16/1024 of the screen width.
const BORDER_FRAC: f32 = 16.0 / 1024.0;

/// The CVar gating this kind's bubble (`0x608b0d`); `None` for a kind that does not bubble.
fn bubble_cvar(kind: ChatEventKind, cfg: &BubbleConfig) -> Option<bool> {
    use ChatEventKind as K;
    match kind {
        K::Party => Some(cfg.party),
        K::Say | K::Yell | K::MonsterSay | K::MonsterYell => Some(cfg.all),
        _ => None,
    }
}

/// The lifetime in ms (`0x4b1810`) over the displayed text: the base, plus the per-word step for
/// each space or tab that ends a word, 2750/750 for others and 1500/500 for the local player. An
/// empty text lasts 0; separators alone, the base.
fn duration_ms(text: &str, is_self: bool) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let (base, per) = if is_self { (1500, 500) } else { (2750, 750) };
    let mut ms = base;
    // Starts as if after a separator, so a leading run adds nothing.
    let mut after_sep = true;
    for b in text.bytes() {
        let sep = b == b' ' || b == b'\t';
        if sep && !after_sep {
            ms += per;
        }
        after_sep = sep;
    }
    ms
}

/// The bubble's copy of the line (`0x4b1600`, into a 0x800-byte buffer): `||` stays escaped for the
/// text's markup parser; any other `|` jumps to the next `[`, or ends the text if there is none;
/// from there the brackets drop, and a `|h` then a `|r` right after the `]` skip. So a link shows
/// its bare name and a colour code with no link after it cuts the line short.
fn sanitize(text: &str) -> String {
    const BUF: usize = 0x800 - 1;
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() && out.len() < BUF {
        if b[i] != b'|' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        if b.get(i + 1) == Some(&b'|') {
            out.extend_from_slice(b"||");
            i += 2;
            continue;
        }
        let Some(open) = b[i..].iter().position(|&c| c == b'[') else {
            break;
        };
        i += open;
        while i < b.len() {
            match b[i] {
                b'[' => i += 1,
                b']' => {
                    i += 1;
                    for skip in [b"|h", b"|r"] {
                        if b[i..].starts_with(skip) {
                            i += 2;
                        }
                    }
                    break;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }
    out.truncate(BUF);
    // Cut only at ASCII bytes, so whole characters, but for the buffer cap.
    String::from_utf8_lossy(&out).into_owned()
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

/// `[bubble+0x320]` bit 1: fading in.
const FADING_IN: u8 = 0x1;
/// Bit 2: fading out.
const FADING_OUT: u8 = 0x2;
/// Bit 4: the fade out is the last, at the end of the line's life.
const FINAL: u8 = 0x4;

/// A bubble's fade (`0x4b0ea0`, `0x4b0ee0`, the middle of `0x4b0c30`): the flags, the fade's start
/// time, the frame's alpha byte and its shown bit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    flags: u8,
    /// `[bubble+0x33c]`, the current fade's start.
    since: u64,
    /// The frame's alpha byte (`+0xc8`); a new frame starts opaque.
    alpha: u8,
    /// The own-shown bit (`+0xd0`).
    shown: bool,
}

impl Fade {
    /// A line's start (`0x4b17da`, then `0x4b17f7`): the flags reset, then the fade in. The alpha
    /// is a new frame's (`0x7690d2`); a reused frame keeps what its last fade left, which this
    /// does not track, and which matters only to a line whose first frame fades out.
    fn spawn(now: u64) -> Self {
        let mut f = Fade {
            flags: 0,
            since: now,
            alpha: 255,
            shown: true,
        };
        f.fade_in(now);
        f
    }

    /// `0x4b0ea0`: unless fading in or on the last fade out, start fading in and show.
    fn fade_in(&mut self, now: u64) {
        if self.flags & (FADING_IN | FINAL) != 0 {
            return;
        }
        self.flags = (self.flags & !FADING_OUT) | FADING_IN;
        self.since = now;
        self.shown = true;
    }

    /// `0x4b0ee0`: unless on the last fade out, mark it final if asked, and start fading out
    /// unless already.
    fn fade_out(&mut self, now: u64, last: bool) {
        if self.flags & FINAL != 0 {
            return;
        }
        if last {
            self.flags |= FINAL;
        }
        if self.flags & FADING_OUT == 0 {
            self.flags = (self.flags & !FADING_IN) | FADING_OUT;
            self.since = now;
        }
    }

    /// One frame of `0x4b0c30` past its residency gates; `false` when the last fade out ends and
    /// the bubble goes back to the pool.
    fn tick(&mut self, now: u64, in_range: bool, born: u64, duration: u64) -> bool {
        if !in_range {
            if self.alpha != 0 {
                self.fade_out(now, false);
            }
        } else if self.alpha != 255 {
            self.fade_in(now);
        }
        if self.flags & FADING_OUT == 0 && now.saturating_sub(born) > duration + FADE_MS {
            self.fade_out(now, true);
        }
        // `elapsed · 255 / 250`, truncated (`0x4b0d5b`-`0x4b0d68`).
        let ramp = |e: u64| (e * 255 / FADE_MS) as u8;
        let e = now.saturating_sub(self.since);
        if self.flags & FADING_IN != 0 {
            if e > FADE_MS {
                self.alpha = 255;
                self.flags &= !FADING_IN;
            } else {
                self.alpha = ramp(e);
            }
        } else if self.flags & FADING_OUT != 0 {
            if e > FADE_MS {
                if self.flags & FINAL != 0 {
                    return false;
                }
                // An out-of-range fade ends hidden (`0x4b0d95`).
                self.flags &= !(FADING_OUT | FINAL);
                self.shown = false;
            } else {
                self.alpha = 255 - ramp(e);
            }
        }
        true
    }
}

/// One live bubble: the `CGChatBubbleFrame` and its `CGUnit+0xe64` handle.
struct Bubble {
    /// This line's number, which tells the widget layer a replacement from a repeat.
    line: u64,
    /// The [`sanitize`]d display text.
    text: String,
    /// The chat-type colour (the 94-entry table), sRGB 0..1.
    color: [f32; 3],
    /// The spawn time in ms (`[bubble+0x338]`).
    born: u64,
    /// The steady window after the fade-in ([`duration_ms`], `[bubble+0x340]`).
    duration: u64,
    /// Stand-box height × model scale, latched at spawn as the reference does (`bubble+0x354`).
    lift: f32,
    fade: Fade,
    /// The squared camera distance the bubble was last stacked by (`[bubble+0x350]`); `None` for
    /// a new line, which the next pass stacks (the spawn flag, `[bubble+0x320] & 0x8`).
    depth: Option<f32>,
}

/// Whether the speaker was drawn in the last render: the reference's scene-node test `0x6704c0`
/// (`node && [node+0x94] == 0`, read at `0x4b0cab`). Every frame sets `[node+0x94]` on every node
/// in the scene list `[0xca7d70]` (`0x698a07`) and clears it only for a node that passes the
/// frustum test `0x682ef0` and the horizon test `0x686000` (`0x6833dd`-`0x6833ec`, and
/// `0x68359b`-`0x6835aa` in WMO groups). Here it is any of the unit's meshes passing last frame's
/// visibility check; benilla has no horizon cull yet, so that is the frustum verdict alone.
fn speaker_drawn(
    unit: Entity,
    children: &Query<&Children>,
    meshes: &Query<&ViewVisibility, With<Mesh3d>>,
) -> bool {
    children
        .get(unit)
        .is_ok_and(|kids| kids.iter().any(|c| meshes.get(c).is_ok_and(|v| v.get())))
}

/// The live bubbles by speaker guid, at most one per unit, their stacking, and the next line's
/// number.
#[derive(Resource, Default)]
struct Bubbles {
    live: HashMap<u64, Bubble>,
    /// The stacking, farthest first (the list at `0xb6f038`); the index is the frame level.
    stack: Vec<u64>,
    /// The camera position the stack was last sorted from (`0xb6eff0`, held once `0xb6effc` is
    /// set).
    eye: Option<Vec3>,
    next_line: u64,
}

impl Bubbles {
    /// A new line from `guid`: it replaces the speaker's old bubble, which goes back to the pool
    /// (`0x608bb4` → `0x608c00`), and the new one is linked at the stack's tail (`0x608bbf` →
    /// `0x4b1870`, `0x4b18f0`), unstacked until the next pass.
    fn spawn(&mut self, guid: u64, bubble: Bubble) {
        self.stack.retain(|g| *g != guid);
        self.stack.push(guid);
        self.live.insert(guid, bubble);
    }

    /// The stacking step of the bubble pass (`0x4b1060`). The camera moved when any coordinate
    /// differs from the held one, compared exactly (`0x4b10a5`–`0x4b10d3`); then every bubble is
    /// re-stacked, else only a new line (`0x4b11c7`), each from the held position to its speaker
    /// (`pos`), into the stack before the first one nearer (`0x4b1360`). The rest keep their
    /// place, so a speaker walking under a still camera keeps its stacking.
    fn restack(&mut self, camera: Option<Vec3>, pos: impl Fn(u64) -> Option<Vec3>) {
        let moved = camera.is_some() && camera != self.eye;
        if moved {
            self.eye = camera;
        }
        let Self {
            live, stack, eye, ..
        } = self;
        stack.retain(|g| live.contains_key(g));
        // Pulled out in stack order, then each re-inserted (`0x4b11d7`, `0x4b1220`).
        let dirty: Vec<u64> = stack
            .iter()
            .copied()
            .filter(|g| moved || live[g].depth.is_none())
            .collect();
        stack.retain(|g| !dirty.contains(g));
        for g in dirty {
            let Some(at) = pos(g) else {
                continue;
            };
            let d = eye.map_or(0.0, |e| e.distance_squared(at));
            if let Some(b) = live.get_mut(&g) {
                b.depth = Some(d);
            }
            let slot = stack
                .iter()
                .position(|o| live[o].depth.is_some_and(|od| od < d))
                .unwrap_or(stack.len());
            stack.insert(slot, g);
        }
    }
}

/// This frame's live bubbles for the widget layer, farthest from the camera first, and the sizes
/// the window gives them; `None` with no viewport to lay out against. [`crate::ui_chat::bubbles`]
/// hands them to the VM.
#[derive(Resource, Default)]
pub(crate) struct BubbleFeed {
    pub(crate) geometry: Option<BubbleGeometry>,
    pub(crate) states: Vec<BubbleState>,
}

/// Units with a live bubble this frame, whose overhead name is suppressed (`+0xe64` ≠ 0).
#[derive(Resource, Default)]
pub(crate) struct BubblesActive(pub(crate) EntityHashSet);

/// Every frame: drain the queue through the spawn gate (`0x608ac0`), run each bubble's fade against
/// its lifetime and the 20 yd re-test (`0x4b0c30`), publish [`BubblesActive`], and lay out the
/// live set for the widget layer ([`BubbleFeed`]), farthest from the camera first.
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
    heights: Query<&StandBoxHeight>,
    time: Res<Time>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut feed: ResMut<BubbleFeed>,
    // The speakers' meshes and their last-frame visibility, for [`speaker_drawn`].
    drawn: (Query<&Children>, Query<&ViewVisibility, With<Mesh3d>>),
) {
    active.0.clear();
    feed.states.clear();
    feed.geometry = None;
    let now = time.elapsed().as_millis() as u64;
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
        // The raw line, not its display copy (`0x608af6`): a line of separators or of a bare
        // colour code still bubbles.
        if raw.is_empty() {
            refuse("empty-text", guid);
            continue;
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
        let text = sanitize(&raw);
        let is_self = self_guid.0 == Some(guid);
        let c = default_color(kind);
        // The one-time height query (`0x4b0e38` calling `0x711a20`, scaled by `[unit+0x90]`). A
        // model with no bounds reads 0 and the bubble sits 0.7 above the feet, as in the reference.
        let lift = heights.get(entity).map_or(0.0, |h| h.0) * tf.scale.y;
        let line = bubbles.next_line;
        bubbles.next_line += 1;
        // Replace, never queue (`0x608bb4`): the old line's frame goes back to the pool.
        bubbles.spawn(
            guid,
            Bubble {
                line,
                duration: duration_ms(&text, is_self),
                text,
                color: [
                    f32::from(c[0]) / 255.0,
                    f32::from(c[1]) / 255.0,
                    f32::from(c[2]) / 255.0,
                ],
                born: now,
                lift,
                fade: Fade::spawn(now),
                depth: None,
            },
        );
    }

    // ── The per-frame pass (`0x4b1060` → `0x4b0c30`) ────────────────────────────────────────
    // A speaker gone (`0x4b117e`), or no local player (`0x4b0c40`), ends the bubble at once.
    // A speaker not drawn in the last render takes the out-of-range leg (`0x4b0cb2` →
    // `0x4b0cd7`): its bubble fades out and hides, and comes back when the speaker is drawn again.
    bubbles.live.retain(|guid, b| {
        let (Some((entity, _, tf)), Some(s)) = (find(*guid), self_tf) else {
            return false;
        };
        let in_range = speaker_drawn(entity, &drawn.0, &drawn.1)
            && (tf.translation - s.translation).length_squared() < MAX_DIST_SQ;
        b.fade.tick(now, in_range, b.born, b.duration)
    });

    let cam = camera.single().ok();
    let window = window.single().ok();
    // The order is the frame levels (`0x4b1360`, stamped `0x4b12d5`-`0x4b1312`), farthest from
    // the camera first.
    bubbles.restack(cam.map(|(_, pose)| pose.translation), |guid| {
        find(guid).map(|(_, _, tf)| tf.translation)
    });
    for guid in &bubbles.stack {
        if let Some((entity, _, _)) = find(*guid) {
            active.0.insert(entity);
        }
    }

    // Sizes from the camera's viewport, else the window's; with neither there is nothing to lay
    // out against.
    let viewport = cam
        .and_then(|(c, _)| c.logical_viewport_size())
        .or_else(|| window.map(|w| Vec2::new(w.width(), w.height())));
    let Some(viewport) = viewport.filter(|v| v.x > 0.0 && v.y > 0.0) else {
        return;
    };
    // Everything below is in window px, divided by the seam once for the `WorldFrame`'s units.
    let seam = window.map_or(1.0, |w| crate::ui_script::seam_scale(w.height()));
    let scale = window.map_or(1.0, Window::scale_factor);
    let basis = plate_basis(viewport);
    feed.geometry = Some(BubbleGeometry {
        border: border_px(viewport, basis) / seam,
        margin: gx_px(MARGIN, basis) / seam,
        wrap_cap: gx_px(WRAP_W, basis) / seam,
        text_height: text_px(TEXT_H, basis) / seam,
        device_px: seam * scale,
    });
    feed.states = bubbles
        .stack
        .iter()
        .filter_map(|guid| {
            let b = bubbles.live.get(guid)?;
            let (entity, _, tf) = find(*guid)?;
            // The anchor (`0x4b0c30`), from this frame's `Transform` and the latched lift.
            let anchor = tf.translation + Vec3::Y * (b.lift + LIFT);
            let seat = cam.and_then(|(c, pose)| {
                c.world_to_viewport(&GlobalTransform::from(*pose), anchor)
                    .ok()
                    .map(|p| (pose, p))
            });
            if benilla_assets::trace::enabled_for("bub") {
                trace_seat(entity, anchor, tf.translation, seat, viewport, scale);
            }
            Some(BubbleState {
                key: *guid,
                line: b.line,
                text: b.text.clone(),
                colour: b.color,
                shown: b.fade.shown,
                alpha: b.fade.alpha,
                seat: match seat {
                    Some((_, p)) => (p.x / seam, (viewport.y - p.y) / seam),
                    // Short of the near plane (behind the camera) the projection writes no point
                    // (`0x483f7a`) and the placement ignores its verdict, seating the zeroed one
                    // (`0x4b14e4`): the text's BOTTOM on the screen's bottom-left corner. Bevy's
                    // reverse-Z projection reports such a point as past either plane. Such a
                    // speaker is not drawn, so the bubble is shown here only while its fade out
                    // runs, at most 250 ms from the alpha it had, which a fresh line has at 255.
                    None => (0.0, 0.0),
                },
            })
        })
        .collect();
}

/// The jitter decomposition (`WOW_MOVE_TRACE` tag `bub`): `anchor=` and `pos=` differ by a latched
/// constant on Y only, so a wobble between them is a defect.
fn trace_seat(
    entity: Entity,
    anchor: Vec3,
    pos: Vec3,
    seat: Option<(&Transform, Vec2)>,
    viewport: Vec2,
    scale: f32,
) {
    let (cp, cf, scr) = match seat {
        Some((pose, p)) => (pose.translation, *pose.forward(), p),
        None => (Vec3::NAN, Vec3::NAN, Vec2::NAN),
    };
    benilla_assets::trace::line(
        "bub",
        &format!(
            "e={} vp=({:.0},{:.0}) anchor=[{:.4},{:.4},{:.4}] cam=[{:.4},{:.4},{:.4}] \
             fwd=[{:.4},{:.4},{:.4}] pos=[{:.4},{:.4},{:.4}] scr=({:.3},{:.3}) scale={scale:.2}",
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
            pos.x,
            pos.y,
            pos.z,
            scr.x,
            scr.y,
        ),
    );
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
            .init_resource::<BubbleFeed>()
            // After the V-plate drive, whose verdict the spawn gate reads, and before the script
            // paint, so a bubble draws the frame it moves.
            .add_systems(
                Update,
                (
                    drive_bubbles.in_set(BubbleSet),
                    crate::ui_chat::bubbles::feed_bubbles,
                )
                    .chain()
                    .after(VPlateSet)
                    .before(crate::ui_script::UiPaint),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x4b1810`: the base, plus the per-word step for each space or tab that ends a word; an
    /// empty buffer lasts 0.
    #[test]
    fn duration_is_the_separator_count_law() {
        assert_eq!(duration_ms("", false), 0);
        assert_eq!(duration_ms("hi", false), 2750);
        assert_eq!(duration_ms("hi there", false), 3500);
        assert_eq!(
            duration_ms("hi ", false),
            3500,
            "a trailing space ends a word"
        );
        assert_eq!(
            duration_ms("hi  \t there", false),
            3500,
            "a run counts once"
        );
        assert_eq!(
            duration_ms("   ", false),
            2750,
            "separators alone: the base"
        );
        assert_eq!(
            duration_ms("a\u{a0}b", false),
            2750,
            "NBSP is not a separator"
        );
        assert_eq!(duration_ms("hi", true), 1500);
        assert_eq!(duration_ms("a b c", true), 2500);
    }

    /// The copy loop in `0x4b1600`: `||` stays escaped; any other `|` jumps to the next `[` (or
    /// ends the text), the bracket pair drops and a closing `|h` then `|r` after it skip.
    #[test]
    fn sanitize_is_the_bubble_copy_loop() {
        assert_eq!(sanitize("hello"), "hello");
        assert_eq!(sanitize("a||b"), "a||b");
        assert_eq!(
            sanitize("look |cffa335ee|Hitem:19019:0:0:0|h[Thunderfury]|h|r!"),
            "look Thunderfury!"
        );
        assert_eq!(
            sanitize("look |Hitem:19019|h[Thunderfury]|h now"),
            "look Thunderfury now"
        );
        assert_eq!(
            sanitize("|cffff0000red|r plain"),
            "",
            "no bracket: the text ends"
        );
        assert_eq!(sanitize("dangling |"), "dangling ");
        assert_eq!(sanitize("[x] |y [z] w"), "[x] z w");
    }

    /// A line in range: 250 ms in, its lifetime, 250 ms out, then back to the pool; the ramp is
    /// the truncated `elapsed · 255 / 250`.
    #[test]
    fn a_line_fades_in_holds_fades_out_and_ends() {
        let (born, life) = (1000, 2750);
        let mut f = Fade::spawn(born);
        let at = |f: &mut Fade, t: u64| -> Option<(u8, bool)> {
            f.tick(t, true, born, life).then_some((f.alpha, f.shown))
        };
        assert_eq!(at(&mut f, born), Some((0, true)));
        assert_eq!(at(&mut f, born + 100), Some((102, true)));
        assert_eq!(
            at(&mut f, born + 250),
            Some((255, true)),
            "250 is still the ramp"
        );
        assert_eq!(at(&mut f, born + 251), Some((255, true)));
        // The last fade starts once `now − born` passes the lifetime plus one fade.
        assert_eq!(at(&mut f, born + life + 250), Some((255, true)));
        assert_eq!(
            at(&mut f, born + life + 251),
            Some((255, true)),
            "starts at 0 elapsed"
        );
        assert_eq!(at(&mut f, born + life + 351), Some((153, true)));
        assert_eq!(at(&mut f, born + life + 501), Some((0, true)));
        assert_eq!(
            at(&mut f, born + life + 502),
            None,
            "the frame goes back to the pool"
        );
    }

    /// Out of range a bubble fades out and hides but lives on; back in range it shows and fades
    /// in, and turning back mid fade-out restarts the ramp from 0 (`0x4b0ea0`). Ticked every
    /// 10 ms, as frames would.
    #[test]
    fn out_of_range_fades_to_hidden_and_back() {
        let life = 60_000;
        let mut f = Fade::spawn(0);
        let run = |f: &mut Fade, from: u64, to: u64, in_range: bool| {
            for t in (from..=to).step_by(10) {
                assert!(f.tick(t, in_range, 0, life), "alive at {t}");
            }
        };
        run(&mut f, 0, 300, true);
        assert_eq!((f.alpha, f.shown), (255, true));
        run(&mut f, 310, 600, false);
        assert_eq!(
            (f.alpha, f.shown, f.flags),
            (0, false, 0),
            "hidden, alive, no fade running"
        );
        run(&mut f, 610, 790, false);
        assert!(!f.shown, "out of range it waits hidden");
        run(&mut f, 800, 900, true);
        assert_eq!((f.shown, f.flags), (true, FADING_IN));
        assert_eq!(f.alpha, 102, "100 ms into the fade in");
        run(&mut f, 910, 1200, true);
        run(&mut f, 1210, 1310, false);
        assert_eq!(f.alpha, 153, "100 ms into the fade out");
        run(&mut f, 1320, 1320, true);
        assert_eq!(f.alpha, 0, "the fade in starts over");
    }

    /// A speaker culled from the last render takes the out-of-range leg: the bubble fades out and
    /// hides while the speaker is undrawn, and shows again once it is drawn (`0x4b0cab`).
    #[test]
    fn a_culled_speaker_fades_its_bubble_out_and_back() {
        use benilla_protocol::EntityKind;
        use bevy::camera::visibility::SetViewVisibility;
        use std::time::Duration;

        let mut app = App::new();
        app.init_resource::<BubbleQueue>()
            .init_resource::<BubbleConfig>()
            .init_resource::<Bubbles>()
            .init_resource::<BubblesActive>()
            .init_resource::<BubbleFeed>()
            .init_resource::<VPlates>()
            .init_resource::<SelfGuid>()
            .init_resource::<Time>()
            .add_systems(Update, drive_bubbles);
        let speaker = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Guid(7),
                Transform::from_xyz(5.0, 0.0, 0.0),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((Mesh3d::default(), ViewVisibility::HIDDEN, ChildOf(speaker)))
            .id();
        app.world_mut().spawn((Embodied, Transform::default()));
        let set_drawn = |app: &mut App, drawn: bool| {
            let w = app.world_mut();
            if drawn {
                w.get_mut::<ViewVisibility>(body).unwrap().set_visible();
            } else {
                w.entity_mut(body).insert(ViewVisibility::HIDDEN);
            }
        };
        // Run frames 10 ms apart up to `to` ms, and read the bubble's fade.
        let mut now = 0u64;
        let mut run_to = |app: &mut App, to: u64| -> Option<(u8, bool)> {
            while now < to {
                now += 10;
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(Duration::from_millis(10));
                app.update();
            }
            let b = app.world().resource::<Bubbles>();
            b.live.get(&7).map(|b| (b.fade.alpha, b.fade.shown))
        };

        set_drawn(&mut app, true);
        let cfg = BubbleConfig::default();
        app.world_mut().resource_mut::<BubbleQueue>().push(
            &cfg,
            7,
            ChatEventKind::Say,
            "one two three four five six seven eight",
        );
        assert_eq!(run_to(&mut app, 400), Some((255, true)), "drawn: faded in");
        set_drawn(&mut app, false);
        assert_eq!(
            run_to(&mut app, 800),
            Some((0, false)),
            "culled in range: faded out and hidden, still alive"
        );
        set_drawn(&mut app, true);
        assert_eq!(
            run_to(&mut app, 1200),
            Some((255, true)),
            "drawn again: shown and faded back in"
        );
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

    /// A line spoken by `guid` into `b`, as the spawn gate leaves it.
    fn speak(b: &mut Bubbles, guid: u64) {
        let line = b.next_line;
        b.next_line += 1;
        b.spawn(
            guid,
            Bubble {
                line,
                text: String::new(),
                color: [1.0; 3],
                born: 0,
                duration: 0,
                lift: 0.0,
                fade: Fade::spawn(0),
                depth: None,
            },
        );
    }

    /// Farthest from the camera first, an exact tie keeping the earlier line in front of the
    /// later; the stack stands while the camera does, whoever walks, and a camera move re-sorts.
    #[test]
    fn the_stack_resorts_on_a_camera_move_or_a_new_line_only() {
        let mut at: HashMap<u64, Vec3> = HashMap::from([
            (1, Vec3::new(5.0, 0.0, 0.0)),
            (2, Vec3::new(20.0, 0.0, 0.0)),
            (3, Vec3::new(20.0, 0.0, 0.0)),
        ]);
        let mut b = Bubbles::default();
        for g in [1, 2, 3] {
            speak(&mut b, g);
        }
        let mut camera = Vec3::ZERO;
        b.restack(Some(camera), |g| at.get(&g).copied());
        assert_eq!(b.stack, [2, 3, 1], "farthest first; the tie in line order");

        // Speaker 1 walks behind the others under a still camera.
        at.insert(1, Vec3::new(40.0, 0.0, 0.0));
        b.restack(Some(camera), |g| at.get(&g).copied());
        assert_eq!(b.stack, [2, 3, 1], "a still camera keeps the stacking");

        // A new line stacks at its own distance; the others keep their stale ones.
        speak(&mut b, 4);
        at.insert(4, Vec3::new(10.0, 0.0, 0.0));
        b.restack(Some(camera), |g| at.get(&g).copied());
        assert_eq!(b.stack, [2, 3, 4, 1]);

        // The camera moves, by any amount: every bubble re-sorts.
        camera.y = 1e-3;
        b.restack(Some(camera), |g| at.get(&g).copied());
        assert_eq!(b.stack, [1, 2, 3, 4]);

        // A bubble that ends leaves the stack.
        b.live.remove(&2);
        b.restack(Some(camera), |g| at.get(&g).copied());
        assert_eq!(b.stack, [1, 3, 4]);
    }

    /// A speaker who speaks again goes to the tail before the pass re-stacks the new lines in list
    /// order (`0x4b1870`, `0x4b11d7`), so in a tie the later line lands in front.
    #[test]
    fn a_repeat_line_moves_its_speaker_to_the_tail() {
        let at = |_: u64| Some(Vec3::new(10.0, 0.0, 0.0));
        let mut b = Bubbles::default();
        for g in [1, 2] {
            speak(&mut b, g);
        }
        b.restack(Some(Vec3::ZERO), at);
        assert_eq!(b.stack, [1, 2]);
        // In one frame, 3 speaks, then 1 again, all at one distance.
        speak(&mut b, 3);
        speak(&mut b, 1);
        b.restack(Some(Vec3::ZERO), at);
        assert_eq!(b.stack, [2, 3, 1], "1's new line came after 3's");
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
