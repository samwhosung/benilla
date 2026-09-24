//! The minimap **blip layer** (decision 0203 phase 3; the byte law per the fold-back,
//! decision 0337): AreaPOI landmarks + quest dots + the hover tooltip.
//!
//! **Landmarks** (selection `0x6d9a90`, candidates `0x6d8e10`, draw `0x4ed148`/`0x4ee170`):
//! - **Candidates**: `ContinentID == current map` AND `Flags & 1` — nothing else (no faction,
//!   no importance gate; both factions' rows are candidates).
//! - **In-range** (`d/viewRadius ≤ 0.8`): draws the `POIIcons.blp` cell indexed by the DBC
//!   `Icon` column at the POI's true position — but **only** rows with `Flags & 2` (in 5875
//!   data that is exactly the 32 world-PvP tower rows, e.g. the EPL towers). An ordinary
//!   in-range landmark (town/city, `Flags & 2` clear) draws nothing.
//! - **Out-of-range**, `d ≤ 694.444`: the nearest **3**, ranked by (`Importance` **signed**
//!   asc, dist asc) — the rank key is Importance, NOT AreaID (the fold-back's C2 correction) —
//!   drawn ON the 0.8 rim, each **rotated to point at its POI** (the client bakes direction +
//!   `0.8·radius` into the arrow frame's matrix), in the art its **source** calls for: see
//!   [`RimArrow`], the four-way table 1519 pinned.
//!
//! **Quest dots** — the ObjectIcons classifier `0x4eaa90`: reads the SAME per-guid
//! DIALOG_STATUS cache the overhead `!`/`?` markers mirror (one `SMSG_QUESTGIVER_STATUS`, two
//! consumers), and draws a dot **only for status == 7** (REWARD2) → the gold **cell 3**.
//! Status 6 draws NO dot (the vmangos "6 = red dot" comment is server intent, not this
//! client; blue cell 4 is party members, green cell 2 is never populated in 1.12.1). Dots
//! hard-cull at the live zoom radius in **3-D** world distance (no rim ride), and draw
//! LAST — above every arrow.
//!
//! **Tracking dots** (decisions 0560/0564) — the classifier's fall-through for objects NOT
//! at quest status 7: a GameObject passing `0x5ed2b0` draws the gold **cell 0**
//! (Find Herbs/Minerals), a unit passing `0x5ed210` the red **cell 1**. The
//! masks are the `PLAYER_TRACK_CREATURES`/`PLAYER_TRACK_RESOURCES` descriptor mirror
//! (fields 1104/1105 — `values[UNIT_END] + 0x394/0x395`, byte-exact) with one bit per
//! active tracking aura's MiscValue. A GO tests its lockId → `Lock.dbc` skill-slot
//! `LockType`; a unit its creature type via the 3-way resolver `0x605570` (shapeshift-form
//! override → cached creature template → race/Humanoid), behind two always-show clauses:
//! `UNIT_DYNFLAG_TRACK_UNIT` (Hunter's Mark) and our track-stealthed bit vs the target's
//! CREEP vis-flag (the TRACK_STEALTHED consumer). No alive/dead or faction gate.
//!
//! **Sizes are byte-pinned** (`0x4edbc0`, the 0342 fold-back): every blip constant is
//! frozen once by the CGMinimapFrame ctor against the stock widget's 140.8-px screen basis —
//! dot 8 px, POI icon 16 px, orbit 0.8 × the 70.4-px half-disc; the arrows render their M2
//! models at 768 px/unit (rim, modelScale 0.6) and 1280 px/unit (player). Both arrow models
//! are plain textured quads (measured from the real M2s), so the flat-sprite stand-in is
//! geometry-exact at the model's own quad size — no uv crop, the padding is authored. The rim
//! model stacks six of them, but only ever shows one at a time ([`RimArrow`]).
//!
//! **The guard's directions marker** ([`crate::poi_marker`], decisions 1514/1516) is a landmark
//! candidate like any other, appended after the DBC filter the way the reference appends its
//! static blip slot — everything above applies to it unchanged, except the one thing that is
//! *about* being a different kind of thing: its rim arrow is the gold guide arrow ([`RimArrow`]).
//!
//! Remaining residue: one gate the landmark selection here does NOT implement, recorded in 1516
//! and belonging to the DBC rows rather than the marker — a `WorldStateID` gate at `0x6d9b27`.
//!
//! Three more from 1525, all recorded rather than built (each is a visible change to a
//! feature that is not the arrow art, and one is contested):
//! - **The corpse is in the wrong array and the pet blip is missing** — see [`emit_party_arrows`].
//! - **Rim arrows have a per-source z-order we flatten.** `+0x48` doubles as a frame-level offset
//!   (`[minimap+0xc4]+3+kind`, POI frames re-levelled every draw at `0x4ed344`; party frames once
//!   at creation), giving bottom→top: landmark, party/pet, quest, gossip, corpse. We paint every
//!   arrow at one `z_key` in emission order, which happens to agree on landmark-under-party and
//!   disagrees above that. **NOT built because two findings conflict**: `0x4ed7b7` has the
//!   object dots drawing LAST, above every arrow, while this table puts gossip and corpse above the
//!   quest dot. Frame level vs. the parent's own draw order is a compositing question neither
//!   finding settles, and guessing would move a look on a coin flip.
//! - **Static blip slot 0 is `SelectQuestLogEntry()`'s marker** (`0x4def70`, cleared by
//!   `0x4df0e0`): selecting a quest in the log drops a gold guide arrow at that quest's POI —
//!   `flags = 0`, so it is **arrow-only**, showing nothing once you are inside the 0.8 rim. Live
//!   code, not a dead hook, and a feature we do not have. (Its `PointX/PointY` source is INFERRED
//!   from use, not byte-derived.)
//!
//! Also: dots re-project every frame where the reference draws ~1 Hz-stale
//! snapshot coords verbatim (a throttle quirk, deliberately not aped — positions agree for
//! standing NPCs); the subzone grey tint (`0xffb0b0b0`) on dots and their tooltip is drawn
//! from the indoor-containment MISMATCH — the exact `0x670540` compare is INTERIM and still
//! open; the tooltip anchor beside the map is an eyeball (the engine handler `0x4eb0c0`
//! law gives content, not the exact seat).

mod dots;

pub(super) use dots::{emit_party_dots, emit_quest_dots, emit_tracking_dots, SelfTracking};

use bevy::ecs::system::NonSendMut;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{AreaPoi, AreaPoiCatalog};
use benilla_ui::script::UiScript;

use crate::go_templates::GameObjectTemplates;
use crate::names::NameCache;
use crate::net::{Embodied, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::ui_pass::{UiQuad, UiQuads, UvRect};

/// The landmark rank radius in yards (`0x8116cc` = 694.444, VERIFIED).
const LANDMARK_RANK_YD: f32 = 694.444;
/// The in/out-of-range split ratio (`0x811730` = 0.8, VERIFIED): at or under it a POI is
/// in-range (icon path); over it, an edge-arrow candidate riding the rim — whose radius is
/// byte-pinned as 0.8 × the frozen 70.4-px half-disc = 56.32 px at stock (= 0.4 × side, the
/// same expression the placement below uses).
const BLIP_EDGE_RATIO: f32 = 0.8;
/// AreaPOI `Flags` bit 0 — the candidacy gate (`zone_rebuild 0x6d8e10`, VERIFIED).
const FLAG_CANDIDATE: u32 = 0x1;
/// AreaPOI `Flags` bit 1 — "draw the in-range icon" (`0x6d9a90`, VERIFIED); the world-PvP
/// tower class in 5875 data.
const FLAG_IN_RANGE_ICON: u32 = 0x2;
/// The frozen basis the client bakes every blip-size constant against: the CGMinimapFrame
/// ctor (`0x4edbc0`) computes them ONCE from the stock 140-XML-unit widget —
/// which lands at 140.8 screen px on the 1024×768 reference — and never recomputes. Sizes
/// below are that table's byte-derived pixel values over this basis, so at stock geometry we
/// render them exactly and any resized widget scales proportionally.
pub(super) const BLIP_BASIS_PX: f32 = 140.8;
/// The player arrow's quad: `MinimapArrow.m2` is a single full-texture 0.0262 × 0.0263-unit
/// quad rendered at 1280 px per model unit — 33.6 px at stock. Its authored centre sits
/// (+0.0004, +0.00135) model units off the frame origin: ≈(+0.5 right, 1.7 up) screen px,
/// rotating with the facing (model +y = screen-up at zero rotation).
/// The player arrow's art — the minimap ring's and, since decision 1980, the world map's: the stock
/// `WorldMapFrame.lua` `SetModel`s an anonymous pane to the same M2 and the app draws that pane as
/// this sprite (`ui_script::extract`).
pub(crate) const PLAYER_ARROW_TEXTURE: &str = "Interface\\Minimap\\MinimapArrow";
pub(crate) const PLAYER_ARROW_QUAD_PX: f32 = 33.6;
pub(crate) const PLAYER_ARROW_OFFSET_PX: bevy::math::Vec2 = bevy::math::Vec2::new(0.51, -1.73);
/// The rim arrow's on-screen quad: the `Rotating-MinimapArrow.m2` geometry is a stack of six
/// full-texture 0.0500-unit quads (measured from the real M2's 24 vertices, z 0/0.0145/0.0291),
/// and the frame renders at 768 px per model unit (modelScale 0.6 × 1280 px/unit):
/// 0.0500 · 768 = 38.4 px. The full 32² texture maps onto it (uv 0..1 on every layer), so the
/// flat-sprite stand-in needs NO uv crop — the art's transparent padding is authored into the
/// model's own quad.
///
/// The six quads are **not** an alpha-layering trick on one texture, as this note used to say
/// (1519): they carry six *different* textures, four of which are the four arrow arts, and the
/// sequence played picks which one is opaque. See [`RimArrow`] — that is why only one of the six
/// ever draws, and why the stand-in is a single sprite rather than a stack.
const ARROW_QUAD_PX: f32 = 38.4;

/// **Which rim-arrow art a blip source draws** — the reference's four, and why they look like one.
///
/// `Rotating-MinimapArrow.mdx` is the *only* arrow model the minimap owns (`0x4ee2b0` binds it to
/// `minimapArrowModel` for both the party frames and the POI frames), so a reader naturally
/// concludes there is one arrow art. There are four, and the model picks between them **by
/// animation**: `0x4ed349`–`0x4ed37b` hands `0x76cf50` — the SetSequence arm, `0x76cf50(this, seq)`
/// → `0x7121a0(model, -1, seq, -1, 0, 1.0, 0, 1)` — an `AnimationData.dbc` id chosen off the
/// output record's `+0x48`, and `0x4ee170` arms the 5 party frames with `0xa5`. That argument is
/// a sequence id, not a "model id" for one of four `.mdx` variants, and there is one model.
///
/// The model is what proves the mapping, and it is measured, not inferred: the install's own
/// `Rotating-MinimapArrow.m2` authors **six** textured quads — the four arrow arts below, plus a
/// glow and `Spells\Star4` — and exactly eight sequences, `0xa5`/`0xa6`/`0xa7`/`0xa8` (1 s, looping)
/// and `0xcc`–`0xcf` (2 s, one-shot arrival flashes). Every layer's M2Color **alpha** track is
/// `interp = 0` (step), and in each looping band exactly one arrow layer steps to 1.0 while the
/// other three sit at 0.0 — so "play sequence `0xa8`" *is* "draw the guide arrow". The four arrow
/// layers' RGB tracks are a single `[1,1,1]` key (untinted: the art's own colour), and the glow and
/// star layers hold alpha 0 through all four loops — they light only in the one-shot flashes, which
/// this call site never plays. So one flat sprite per source is colour- and geometry-exact.
/// [`tests::each_rim_arrow_sequence_lights_exactly_its_own_layer`] re-measures all of that against
/// the real file.
///
/// `AnimationData.dbc` names the four rows outright — 165 `GroupArrow`, 166 `Arrow`, 167
/// `CorpseArrow`, 168 `GuideArrow` (1525). Four arbitrary-looking ids landing on four names that
/// describe their callers exactly is why this is settled rather than merely likely.
///
/// **The selector is a default, not an enumeration** (1525): `0xa7` if `+0x48 == 2`, else
/// `0xa6 + 2·(v != -2)`. Only a DBC landmark and the corpse are special-cased; **everything else
/// draws the gold guide arrow**, including static slot 0. So [`Self::Guide`] is the right art for
/// any future non-DBC candidate we append, and the two named rows are the exceptions to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RimArrow {
    /// An `AreaPOI.dbc` landmark — sequence `0xa6`, output record `+0x48 == -2`. The white one.
    Landmark,
    /// A party/raid member **or your pet** — sequence `0xa5`, armed once at `0x4ee1e3` and never
    /// re-armed: neither draw loop touches it, and a dead/offline/zoned-out member is hidden
    /// upstream rather than re-skinned (1525). Permanently the group arrow.
    Group,
    /// Your own corpse — sequence `0xa7`, output record `+0x48 == 2`.
    Corpse,
    /// The guard's directions ([`crate::poi_marker`]) — sequence `0xa8`, the default arm. The gold
    /// one: the guard is your *guide*, and the art is named for it.
    Guide,
}

impl RimArrow {
    /// The `AnimationData.dbc` sequence the reference plays to select this art.
    ///
    /// Nothing in our draw path needs it — we resolve to a flat sprite and never play a sequence —
    /// so it exists for the one job only the id can do: asking the real model what that sequence
    /// shows, which is what pins [`Self::texture`] to something better than a guess. Test-only for
    /// the same reason `poi_marker`'s `ICON_POI_REDFLAG` is.
    #[cfg(test)]
    pub(crate) const fn anim_id(self) -> u16 {
        match self {
            Self::Group => 0xa5,
            Self::Landmark => 0xa6,
            Self::Corpse => 0xa7,
            Self::Guide => 0xa8,
        }
    }

    /// The `.blp` that sequence lights — the layer whose M2Color alpha steps to 1.0 in its band.
    pub(crate) const fn texture(self) -> &'static str {
        match self {
            Self::Landmark => "Interface\\Minimap\\Rotating-MinimapArrow",
            Self::Group => "Interface\\Minimap\\Rotating-MinimapGroupArrow",
            Self::Corpse => "Interface\\Minimap\\Rotating-MinimapCorpseArrow",
            Self::Guide => "Interface\\Minimap\\Rotating-MinimapGuideArrow",
        }
    }

    /// The four, in [`RimArrowArt`]'s slot order.
    pub(crate) const ALL: [Self; 4] = [Self::Landmark, Self::Group, Self::Corpse, Self::Guide];
}

/// The four rim-arrow textures, loaded once — the flat-sprite stand-in for the one model the
/// reference re-animates. A missing slot draws no arrow for that source rather than the wrong one.
#[derive(Default)]
pub(crate) struct RimArrowArt([Option<Handle<Image>>; 4]);

impl RimArrowArt {
    pub(crate) fn set(&mut self, kind: RimArrow, tex: Option<Handle<Image>>) {
        self.0[kind as usize] = tex;
    }

    pub(super) fn get(&self, kind: RimArrow) -> Option<&Handle<Image>> {
        self.0[kind as usize].as_ref()
    }

    /// Any art at all — the landmark pass runs on the arrow art alone (it draws the guard marker
    /// even with no `AreaPOI.dbc`), so it needs to know whether the load found anything.
    pub(super) fn any(&self) -> bool {
        self.0.iter().any(Option::is_some)
    }
}
/// An in-range POI icon's quad: 16 × 16 px (`bc7658 = base·0.0125` × 1280, ctor-frozen).
const POI_ICON_PX: f32 = 16.0;
// The dot-layer constants (cell coords, dot sizes, the tracking-predicate constants) live
// with the dot emitters in [`dots`].

/// The blip under the cursor this frame — written by `emit_minimap`'s blip passes, consumed
/// by [`drive_blip_tooltip`]. A quest dot carries the guid (the name resolves at the drive,
/// where the ask-once `NameCache` lives).
#[derive(Resource, Default)]
pub(crate) enum MinimapBlipHover {
    #[default]
    None,
    /// A landmark's name + the cursor's UI-space point at detection (the tooltip's seat).
    Landmark(String, Vec2),
    /// A unit dot's guid (quest gold or tracked red — both resolve through the name cache,
    /// the classifier's own `GetName()` route) + cursor point + whether the dot rendered
    /// GREY (cross-interior — the tooltip line grey-wraps the same way, `|cffb0b0b0`).
    Npc(u64, Vec2, bool),
    /// A tracked GameObject dot: its template name (known synchronously — the same string
    /// `GetName()` returns for a GO) + cursor point + the grey flag.
    TrackedGo(String, Vec2, bool),
}

/// The blip layer's system inputs, tupled so `emit_minimap` stays under Bevy's 16-param
/// ceiling: the quest-status store, the guid→entity index + unit positions, the cursor's
/// window, the hover-out slot, the party state, and the tracking-dot inputs (candidates,
/// our own descriptor's masks, the creature/GO template caches, `Lock.dbc`), plus the
/// `uiScale` dial the tooltip's cursor seat converts through, and the guard-directions marker
/// the landmark pass draws as a candidate.
pub(super) type BlipInputs<'w, 's> = (
    Res<'w, crate::ui_quest::QuestGiver>,
    Res<'w, GuidIndex>,
    Query<'w, 's, &'static GlobalTransform, With<NetEntity>>,
    Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    ResMut<'w, MinimapBlipHover>,
    Res<'w, crate::ui_script::UiScaleCvar>,
    Res<'w, crate::ui_party::GroupState>,
    TrackedCandidates<'w, 's>,
    Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    Res<'w, NameCache>,
    Res<'w, GameObjectTemplates>,
    Option<Res<'w, crate::go_templates::Locks>>,
    Res<'w, crate::poi_marker::PoiMarker>,
    Option<Res<'w, crate::area_poi::AreaPoiRes>>,
    ResMut<'w, super::MinimapPing>,
    Option<NonSendMut<'w, UiScript>>,
);

/// Every streamed object the tracking classifier considers (our own avatar excluded — the
/// player is the arrow, never a dot).
pub(super) type TrackedCandidates<'w, 's> = Query<
    'w,
    's,
    (
        &'static Guid,
        &'static NetEntity,
        &'static GlobalTransform,
        Option<&'static ObjectStore>,
    ),
    Without<Embodied>,
>;

/// The shared frame geometry the blip emitters draw in: the widget centre/side, the active
/// branch's world→px scale and view radius, the paint key, and the player's WoW position.
pub(super) struct BlipCtx {
    pub(super) center: Vec2,
    pub(super) side: f32,
    pub(super) px_per_yd: f32,
    /// The live view radius in yards (the active zoom's half-extent / interior radius) —
    /// the in/out-of-range split's denominator and the dot cull's range.
    pub(super) radius_yd: f32,
    pub(super) z: u64,
    pub(super) alpha: f32,
    pub(super) wx: f32,
    pub(super) wy: f32,
    pub(super) wz: f32,
    /// Window cursor in the same y-down logical px as the quad rects; `None` = off-window.
    pub(super) cursor: Option<Vec2>,
    /// The same cursor in UI space (y-up, from the window bottom-left) — the tooltip's
    /// anchor point (the reference seats the blip tooltip at the cursor; director-verified,
    /// exact engine offset pending the anchor-law pin).
    pub(super) cursor_ui: Option<Vec2>,
    /// THE seam scale (decision 0582): **window px per UI unit**. Everything else in this struct
    /// is window px; anything arriving from Lua ([`Minimap:PingLocation`](super::ping)) is in UI
    /// units, and this is the one number that crosses them. Mixing the two silently is decision
    /// 1596's first root cause.
    pub(super) seam: f32,
}

impl BlipCtx {
    /// A WoW world point → its screen offset from the widget centre (north-up: up = +X north,
    /// left = +Y west — the same mapping the tiles and the corpse blip use).
    pub(super) fn offset(&self, w: [f32; 3]) -> Vec2 {
        Vec2::new(
            (self.wy - w[1]) * self.px_per_yd,
            -(w[0] - self.wx) * self.px_per_yd,
        )
    }
}

/// The landmark selection's two draw lists (`0x6d9a90`'s split).
pub(super) struct LandmarkSelection<'a> {
    /// In-range rows with [`FLAG_IN_RANGE_ICON`]: the `Icon` POIIcons cell at true position.
    pub(super) icons: Vec<&'a AreaPoi>,
    /// The out-of-range nearest-3 `(dist, poi, art)` in rank order: rim arrows. The art is the
    /// reference's `+0x48` read ([`RimArrow`]) — a DBC row draws the white landmark arrow, the
    /// appended guard marker the gold guide arrow.
    pub(super) arrows: Vec<(f32, &'a AreaPoi, RimArrow)>,
}

/// The byte-verified landmark selection: candidacy (`ContinentID` + `Flags&1`), the 0.8
/// in/out split against the live view radius, and the out-of-range nearest-3 ranked by
/// (`Importance` signed asc, dist asc) within 694.444 yd.
///
/// `marker` is the guard-directions POI ([`crate::poi_marker`]) — the reference appends its
/// static blip slot to the candidate list *after* the DBC scan, so it bypasses the candidacy
/// gate and then competes as an equal for the rim slots. Equal, and no more: it gets **no**
/// exemption from the 694.444-yd rank cut (that belongs to the corpse slot `0xcea848`, the only
/// candidate `0x6d9cc2` spares).
pub(super) fn select_landmarks<'a>(
    pois: impl Iterator<Item = &'a AreaPoi>,
    marker: Option<&'a AreaPoi>,
    map_id: u32,
    wx: f32,
    wy: f32,
    radius_yd: f32,
) -> LandmarkSelection<'a> {
    let mut icons = Vec::new();
    let mut arrows: Vec<(f32, &AreaPoi, RimArrow)> = Vec::new();
    let candidates = pois
        .filter(|p| p.continent_id == map_id && p.flags & FLAG_CANDIDATE != 0)
        .map(|p| (p, RimArrow::Landmark))
        .chain(marker.map(|m| (m, RimArrow::Guide)));
    for (p, art) in candidates {
        let d = ((p.pos[0] - wx).powi(2) + (p.pos[1] - wy).powi(2)).sqrt();
        if d / radius_yd <= BLIP_EDGE_RATIO {
            if p.flags & FLAG_IN_RANGE_ICON != 0 {
                icons.push(p);
            }
        } else if d <= LANDMARK_RANK_YD {
            arrows.push((d, p, art));
        }
    }
    arrows.sort_by(|a, b| {
        (a.1.importance as i32)
            .cmp(&(b.1.importance as i32))
            .then(a.0.total_cmp(&b.0))
    });
    arrows.truncate(3);
    LandmarkSelection { icons, arrows }
}

/// `POIIcons.blp` cell for a DBC `Icon` index as `[l, r, t, b]` tex coords — the 8×8 grid of
/// 16-px cells, gated `Icon < 0x40` exactly like the client (`0x4ed15e`).
fn poi_icon_cell(icon: u32) -> Option<[f32; 4]> {
    if icon >= 64 {
        return None;
    }
    let (c, r) = ((icon % 8) as f32, (icon / 8) as f32);
    Some([c / 8.0, (c + 1.0) / 8.0, r / 8.0, (r + 1.0) / 8.0])
}

/// Draw the landmark layer: in-range POI icons at position, then the nearest-3 rim arrows,
/// each rotated to point at its POI. Records a hover hit (later-drawn wins, matching draw
/// order).
pub(super) fn emit_landmarks(
    ctx: &BlipCtx,
    cat: Option<&AreaPoiCatalog>,
    marker: Option<&AreaPoi>,
    map_id: u32,
    arrows: &RimArrowArt,
    poi_icons: Option<&Handle<Image>>,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    let sel = select_landmarks(
        cat.into_iter().flat_map(|c| c.rows().map(|(_, p)| p)),
        marker,
        map_id,
        ctx.wx,
        ctx.wy,
        ctx.radius_yd,
    );
    if let Some(icons_tex) = poi_icons {
        for poi in &sel.icons {
            let Some(cell) = poi_icon_cell(poi.icon) else {
                continue;
            };
            let rect = Rect::from_center_size(
                ctx.center + ctx.offset(poi.pos),
                Vec2::splat(ctx.side * (POI_ICON_PX / BLIP_BASIS_PX)),
            );
            quads.overlays.push(UiQuad {
                rect,
                z_key: ctx.z,
                texture: Some(icons_tex.clone()),
                uv: UvRect::from_tex_coords(cell),
                color: [1.0, 1.0, 1.0, ctx.alpha],
                ..default()
            });
            if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
                if rect.contains(c) {
                    *hover = MinimapBlipHover::Landmark(poi.name.clone(), ui);
                }
            }
        }
    }
    for (_d, poi, art) in &sel.arrows {
        // No art for this source ⇒ no arrow. Drawing another source's would be a lie about
        // which kind of thing is out there, which is the whole job of the four arts.
        let Some(tex) = arrows.get(*art) else {
            continue;
        };
        let rect = push_rim_arrow(ctx, poi.pos, tex, quads);
        if let (Some(c), Some(ui)) = (ctx.cursor, ctx.cursor_ui) {
            if rect.contains(c) {
                *hover = MinimapBlipHover::Landmark(poi.name.clone(), ui);
            }
        }
    }
}

/// One rim arrow: parked on the 0.8 rim in the target's direction and spun to point at it.
/// Returns its screen rect, for the hover hit-test.
///
/// ONE pass of the texture. The model authors six quads, but replaying a stack as flat UI quads
/// read as solid blobs (director, 2026-07-13 — the reference runs it through the model material
/// pipeline, whose blend mode is unread); the single soft pass is the approved look, and 1519's
/// measurement says it is also the exact one — only a single layer is ever opaque.
fn push_rim_arrow(
    ctx: &BlipCtx,
    target: [f32; 3],
    tex: &Handle<Image>,
    quads: &mut UiQuads,
) -> Rect {
    let dir = ctx.offset(target).normalize_or_zero();
    let pos = ctx.center + dir * (ctx.side * 0.5 * BLIP_EDGE_RATIO);
    // Point the arrow at its target: quad rotation is clockwise on a y-down screen, and the
    // authored art reads screen-up at zero, so up→dir is atan2(x, -y).
    let rotation = dir.x.atan2(-dir.y);
    let rect = Rect::from_center_size(pos, Vec2::splat(ctx.side * (ARROW_QUAD_PX / BLIP_BASIS_PX)));
    quads.overlays.push(UiQuad {
        rect,
        z_key: ctx.z,
        texture: Some(tex.clone()),
        color: [1.0, 1.0, 1.0, ctx.alpha],
        rotation,
        ..default()
    });
    rect
}

/// A party member's blip position: the live streamed transform wins; out of visibility range the
/// `PARTY_MEMBER_STATS` snapshot position covers (the wire truncates to i16 — yard precision,
/// invisible at minimap scale). No source → no blip (offline members carry no position).
pub(crate) fn party_member_pos(
    m: &benilla_protocol::messages::GroupMemberEntry,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
) -> Option<(f32, f32)> {
    if let Some(tf) = guids.0.get(&m.guid).and_then(|e| unit_pos.get(*e).ok()) {
        let w = bevy_to_wow(tf.translation());
        return Some((w[0], w[1]));
    }
    group
        .stats
        .get(&m.guid)
        .and_then(|s| s.position)
        .map(|(x, y)| (f32::from(x), f32::from(y)))
}

/// The party/corpse **rim arrows** — the out-of-range half of `place_party_raid_blips`
/// (`0x6dad10`, VERIFIED): `d = √(dx²+dy²)`; over `0.8·radius` the member rides the 0.8 rim,
/// rotated to the atan2 bearing (the 5-slot sibling array `this+0x320`). Drawn with the POI arrows
/// — before the player arrow; the in-range members become dots in [`emit_party_dots`], drawn last
/// with the object dots.
///
/// **The two sources draw different art** (1519), which this pass used to get wrong twice over: it
/// drew both with `MinimapArrow` — the *player* arrow, not a rim arrow at all. A member is the
/// party frames' `0xa5` ⇒ [`RimArrow::Group`]; your corpse is the corpse blip's `0xa7` ⇒
/// [`RimArrow::Corpse`], the art named for it. Both confirmed at the bytes (1525).
///
/// **But the corpse does not belong in this array, and the slot it occupies belongs to your PET**
/// (1525, VERIFIED — recorded here, not yet built, because it is a visible change to the corpse
/// blip 0308 rather than to this session's arrow art). `0x6dad10`'s loop splits at
/// `0x6dad60 cmp edi,4`: slots 0–3 are the party GUIDs via `0x4e81a0`, and slot **4** reads
/// `UNIT_FIELD_CHARM` else `UNIT_FIELD_SUMMON` off the descriptor base — the pet. Both legs fetch
/// with typemask UNIT (`mov ecx,8`), and a `CGCorpse` is typemask `0x80`, so a corpse could never
/// resolve there. The corpse reaches the minimap **only** as static blip slot 2 on the
/// POI path, which is a materially different producer: `Importance = -1` (so it outranks every
/// landmark and *takes* one of the three rim slots rather than adding a fourth), and the sole
/// exemption from the 694.444-yd cut at `0x6d9cc2`. Two consequences we currently get wrong — we
/// can draw four rim arrows where the reference draws three — and one feature we are missing
/// outright: the **pet blip**.
pub(super) fn emit_party_arrows(
    ctx: &BlipCtx,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
    corpse: Option<[f32; 3]>,
    arrows: &RimArrowArt,
    quads: &mut UiQuads,
) {
    let members = group
        .party_slots()
        .filter_map(|m| party_member_pos(m, group, guids, unit_pos))
        .map(|p| (p, RimArrow::Group));
    let corpse = corpse.map(|c| ((c[0], c[1]), RimArrow::Corpse));
    for ((x, y), art) in members.chain(corpse) {
        let d = ((x - ctx.wx).powi(2) + (y - ctx.wy).powi(2)).sqrt();
        if d / ctx.radius_yd <= BLIP_EDGE_RATIO {
            continue; // in range — the dot pass draws it
        }
        let Some(tex) = arrows.get(art) else {
            continue;
        };
        push_rim_arrow(ctx, [x, y, 0.0], tex, quads);
    }
}

/// Push the hovered blip's name tooltip: a landmark's AreaPOI name or a tracked GO's template
/// name outright, a unit dot's (quest or tracked) name through the ask-once `NameCache` (the
/// first hover may show one frame late while the `SMSG_NAME_QUERY` answer lands — the
/// reference resolves the guid through its own name cache the same way). Hover loss arms the
/// world tooltip fade, never an instant hide. Runs after
/// the mouseover drive (`UnitFeed`) so a same-frame world-hover→blip transition ends with the
/// blip tooltip shown.
pub(super) fn drive_blip_tooltip(
    script: Option<NonSendMut<UiScript>>,
    hover: Res<MinimapBlipHover>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<(String, Vec2)>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let show = match &*hover {
        MinimapBlipHover::None => None,
        MinimapBlipHover::Landmark(name, at) => Some((name.clone(), *at, false)),
        MinimapBlipHover::Npc(npc, at, grey) => names
            .resolve(*npc, &commands)
            .map(|n| (n.to_string(), *at, *grey)),
        MinimapBlipHover::TrackedGo(name, at, grey) => Some((name.clone(), *at, *grey)),
    };
    match (&show, &*last) {
        // Same blip, cursor drifted: the plate FOLLOWS the pointer (anchor-only re-seat —
        // the reference's blip tooltip tracks the cursor; director-verified).
        (Some((t, at, _)), Some((lt, lat))) if t == lt => {
            if at != lat {
                script.world_tooltip_move(at.x, at.y);
                *last = Some((t.clone(), *at));
            }
        }
        (Some((t, at, grey)), _) => {
            script.minimap_tooltip(t, at.x, at.y, *grey);
            *last = Some((t.clone(), *at));
        }
        (None, Some(_)) => {
            script.world_tooltip_fade();
            *last = None;
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The whole [`RimArrow`] table, re-measured against the install's own model.**
    ///
    /// This is the fact the feature rests on, and it is not one you can read off the reference's
    /// call site: `0x4ed349`–`0x4ed37b` only proves *which id* each blip source plays. What that id
    /// draws lives in `Rotating-MinimapArrow.m2`, so the model is the authority, and asserting
    /// against it means a wrong row here fails rather than shipping the wrong arrow.
    ///
    /// It checks both directions on purpose: each sequence lights **exactly one** layer (a table
    /// that merely named a texture the sequence happens to show would pass a one-sided check while
    /// three other arrows drew on top of it), and that layer is the one this table claims.
    #[test]
    fn each_rim_arrow_sequence_lights_exactly_its_own_layer() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::Chain::open(&data).expect("the patch chain");
        let bytes = chain
            .read_file("Interface\\Minimap\\Rotating-MinimapArrow.m2")
            .expect("the one rim-arrow model");

        for kind in RimArrow::ALL {
            let shown = benilla_formats::m2_sequence_visible_textures(&bytes, kind.anim_id())
                .unwrap_or_else(|| {
                    panic!(
                        "{kind:?}: the model authors no sequence {:#x} — the id \
                    is wrong, or this is not the model the reference animates",
                        kind.anim_id()
                    )
                });
            let want = format!("{}.BLP", kind.texture().to_uppercase());
            assert_eq!(
                shown,
                vec![want],
                "{kind:?} (sequence {:#x}) must light its layer and ONLY its layer",
                kind.anim_id()
            );
        }
    }

    fn poi(importance: u32, flags: u32, x: f32, name: &str) -> AreaPoi {
        AreaPoi {
            importance,
            icon: 6,
            faction_id: 0,
            pos: [x, 0.0, 0.0],
            continent_id: 0,
            flags,
            area_id: 0,
            name: name.into(),
            description: String::new(),
            world_state_id: 0,
        }
    }

    /// The real Northshire shape (5875 AreaPOI.dbc rows, distances from a player near the
    /// abbey, the default zoom's 133-yd radius): Echo Ridge Mine (Flags 4, no bit 0) is not a
    /// candidate at all; Northshire Abbey (Flags 5, in-range, bit 1 clear) draws neither icon
    /// nor arrow; Stormwind and Goldshire (out-of-range, both Importance 3) become the rim
    /// arrows in distance order.
    #[test]
    fn northshire_shape_under_the_byte_law() {
        let rows = [
            poi(3, 0x5, 46.0, "Northshire Abbey"),
            poi(0, 0x4, 237.0, "Echo Ridge Mine"),
            poi(3, 0x1d, 556.0, "Stormwind"),
            poi(3, 0x5, 601.0, "Goldshire"),
        ];
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 133.0);
        assert!(sel.icons.is_empty(), "no Flags&2 row is in range");
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
        assert_eq!(names, ["Stormwind", "Goldshire"]);
    }

    /// The rank key is Importance (signed asc), distance only as the tiebreak — a nearer row
    /// of higher Importance ranks BELOW a farther low-Importance one (the fold-back's C2
    /// correction: the key is DBC column 1, not AreaID).
    #[test]
    fn arrow_rank_is_importance_then_distance_capped_at_three() {
        let rows = [
            poi(3, 1, 200.0, "city-near"),
            poi(0, 1, 650.0, "minor-far"),
            poi(0, 1, 300.0, "minor-near"),
            poi(3, 1, 400.0, "city-mid"),
            poi(0, 1, 695.0, "beyond-rank"),
        ];
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 100.0);
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
        assert_eq!(names, ["minor-near", "minor-far", "city-near"]);
    }

    /// The guard's directions marker ([`crate::poi_marker`]) is a landmark candidate like any
    /// other — and, carrying the reference's `Importance 0`, it takes a rim slot ahead of the
    /// town/city rows a capital is thick with. Out of the four here only three arrows draw, and
    /// the marker is one of them despite being the farthest.
    #[test]
    fn the_guard_marker_competes_for_a_rim_slot_and_outranks_the_cities() {
        let cities = [
            poi(3, 0x1d, 200.0, "Stormwind"),
            poi(3, 0x5, 300.0, "Goldshire"),
            poi(3, 0x5, 400.0, "Northshire Abbey"),
        ];
        let marker = poi(0, 0x63, 500.0, "Stormwind Warrior Trainer");
        let sel = select_landmarks(cities.iter(), Some(&marker), 0, 0.0, 0.0, 133.0);
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Stormwind Warrior Trainer", "Stormwind", "Goldshire"],
            "Importance 0 ranks the marker first; the third city is crowded out"
        );
        // …and it is drawn in gold, not in the cities' white: the reference's `+0x48` read
        // (1519). A rim slot won with the wrong art would look like just another town.
        let arts: Vec<RimArrow> = sel.arrows.iter().map(|&(_, _, a)| a).collect();
        assert_eq!(
            arts,
            [RimArrow::Guide, RimArrow::Landmark, RimArrow::Landmark],
            "the guard's directions draw the guide arrow; DBC rows draw the landmark arrow"
        );
    }

    /// In range, the marker draws its own `POIIcons` cell at its true position — `Flags & 2` is
    /// set on every 5875-era `points_of_interest` row (99 = 0x63), so unlike an ordinary town it
    /// keeps drawing once you are close.
    #[test]
    fn the_guard_marker_draws_its_icon_in_range() {
        let marker = poi(0, 0x63, 50.0, "The Bank");
        let sel = select_landmarks(std::iter::empty(), Some(&marker), 0, 0.0, 0.0, 133.0);
        assert!(sel.arrows.is_empty(), "in range — no rim arrow");
        assert_eq!(sel.icons.len(), 1);
        assert_eq!(
            poi_icon_cell(sel.icons[0].icon),
            Some([0.75, 0.875, 0.0, 0.125]),
            "icon 6 = ICON_POI_REDFLAG, col 6 row 0 of the 8x8 atlas"
        );
    }

    /// The marker bypasses the candidacy gate the DBC rows pass through — the reference appends
    /// its static blip slot *after* the scan. It is the caller ([`crate::poi_marker::PoiMarker`]'s
    /// `on_map`) that decides the marker belongs to this map, not this filter.
    #[test]
    fn the_guard_marker_skips_the_candidacy_filter() {
        let mut marker = poi(0, 0, 300.0, "The Inn"); // Flags bit 0 CLEAR — a DBC row would drop
        marker.continent_id = 571; // and a continent that isn't the displayed one
        let sel = select_landmarks(std::iter::empty(), Some(&marker), 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 1, "appended unconditionally");
    }

    /// An in-range row draws the icon only with Flags bit 1; the `Icon < 64` gate and the
    /// 8×8 cell math match the client's table build.
    #[test]
    fn in_range_icons_gate_on_flag_bit1_and_the_atlas_bound() {
        let mut tower = poi(0, 0x87, 50.0, "Crown Guard Tower");
        tower.icon = 9; // col 1, row 1
        let plain = poi(3, 0x5, 50.0, "Northshire Abbey");
        let rows = [tower, plain];
        let sel = select_landmarks(rows.iter(), None, 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 0);
        assert_eq!(sel.icons.len(), 1, "only the Flags&2 tower draws in range");
        assert_eq!(
            poi_icon_cell(9),
            Some([0.125, 0.25, 0.125, 0.25]),
            "icon 9 = col 1 row 1 of the 8x8 atlas"
        );
        assert_eq!(poi_icon_cell(64), None, "the client gates Icon < 0x40");
    }
}
