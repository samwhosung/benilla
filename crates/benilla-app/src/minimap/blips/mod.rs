//! The minimap blip layer: AreaPOI landmarks, quest and tracking dots, rim arrows and the hover
//! tooltip.
//!
//! Landmarks (selection `0x6d9a90`, candidates `0x6d8e10`, draw `0x4ed148`/`0x4ee170`):
//! - Candidates: `ContinentID` is the current map and `Flags & 1`, no faction or importance gate.
//! - In range (`d / viewRadius ≤ 0.8`): the `POIIcons.blp` cell of the `Icon` column, only for
//!   rows with `Flags & 2` (the world-PvP towers in 1.12 data); other rows draw nothing.
//! - Out of range within 694.444 yd: the first 3 by (`Importance` signed ascending, distance),
//!   on the 0.8 rim, each rotated to point at its POI in its source's art ([`RimArrow`]).
//!
//! Dots come from the ObjectIcons classifier `0x4eaa90`. A unit or player must first be alive, not
//! our own charm or summon, and trackable ([`dots`]'s `unit_dot_eligible`). Then quest status 7
//! draws the gold cell 3 (status 6 draws nothing; green cell 2 is never used in 1.12), and every
//! other object falls through to tracking: a GameObject passing `0x5ed2b0` draws gold cell 0, a
//! unit passing `0x5ed210` red cell 1, against the `PLAYER_TRACK_CREATURES`/`RESOURCES` masks.
//! Party members are blue cell 4. Dots cull at the view radius in 3-D distance and draw last.
//!
//! Blip sizes are frozen once by the CGMinimapFrame ctor (`0x4edbc0`) against a 140.8-px basis.
//!
//! Not built: the landmark `WorldStateID` gate (`0x6d9b27`); the per-source rim-arrow z-order
//! (bottom to top landmark, party, quest, gossip, corpse, from `+0x48` as a frame-level offset),
//! which conflicts with `0x4ed7b7` drawing the dots last and is unsettled; and static blip slot 0,
//! `SelectQuestLogEntry()`'s gold guide arrow at the quest's POI (`0x4def70`).
//!
//! Deviation: dots re-project every frame, where the reference draws coordinates about 1 s
//! stale, because that staleness is a throttle quirk.
//!
//! The cross-interior grey (`0xffb0b0b0`) comes from an indoor-containment mismatch; the
//! reference's compare (`0x670540`) is untraced. The tooltip sits at the cursor; its exact seat
//! is untraced, since `0x4eb0c0` gives only its content.

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

/// The landmark rank radius in yards (`0x8116cc`).
const LANDMARK_RANK_YD: f32 = 694.444;
/// The in-range split and the rim radius as a fraction of the half-disc (`0x811730`).
const BLIP_EDGE_RATIO: f32 = 0.8;
/// AreaPOI `Flags` bit 0, the candidacy gate (`0x6d8e10`).
const FLAG_CANDIDATE: u32 = 0x1;
/// AreaPOI `Flags` bit 1, draw the in-range icon (`0x6d9a90`).
const FLAG_IN_RANGE_ICON: u32 = 0x2;
/// The basis every blip size is frozen against: the stock 140-unit widget at 140.8 px on the
/// 1024×768 screen, computed once by the CGMinimapFrame ctor (`0x4edbc0`).
pub(super) const BLIP_BASIS_PX: f32 = 140.8;
/// The player arrow's art, on the minimap and the world map, whose stock `WorldMapFrame.lua`
/// `SetModel`s a pane to the same M2.
pub(crate) const PLAYER_ARROW_TEXTURE: &str = "Interface\\Minimap\\MinimapArrow";
/// `MinimapArrow.m2` is one 0.0262 × 0.0263-unit quad at 1280 px per model unit.
pub(crate) const PLAYER_ARROW_QUAD_PX: f32 = 33.6;
/// The quad's authored centre, (+0.0004, +0.00135) model units off the origin; it turns with
/// facing.
pub(crate) const PLAYER_ARROW_OFFSET_PX: bevy::math::Vec2 = bevy::math::Vec2::new(0.51, -1.73);
/// The rim arrow's quad: `Rotating-MinimapArrow.m2`'s 0.0500-unit full-texture quads at 768 px
/// per model unit (modelScale 0.6 × 1280), so the sprite needs no uv crop.
const ARROW_QUAD_PX: f32 = 38.4;

/// Which rim-arrow art a blip source draws. The minimap's one arrow model,
/// `Rotating-MinimapArrow.m2`, picks among four arrow layers by sequence (`0x4ed349`): in each
/// looping sequence exactly one layer's stepped alpha is 1.0, untinted, so one flat sprite per
/// source is exact. `AnimationData.dbc` names them 165 `GroupArrow`, 166 `Arrow`, 167
/// `CorpseArrow`, 168 `GuideArrow`.
///
/// The selector is `0xa7` when the output record's `+0x48 == 2`, else `0xa6 + 2·(v != -2)`, so
/// everything but a DBC landmark and the corpse draws the guide arrow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RimArrow {
    /// An `AreaPOI.dbc` landmark: sequence `0xa6`, `+0x48 == -2`, the white one.
    Landmark,
    /// A party or raid member, or your pet: sequence `0xa5`, armed once at `0x4ee1e3` and never
    /// re-armed.
    Group,
    /// Your own corpse: sequence `0xa7`, `+0x48 == 2`.
    Corpse,
    /// The guard's directions ([`crate::poi_marker`]): sequence `0xa8`, the default arm, gold.
    Guide,
}

impl RimArrow {
    /// The `AnimationData.dbc` sequence the reference plays for this art; the test checks the
    /// model against it.
    #[cfg(test)]
    pub(crate) const fn anim_id(self) -> u16 {
        match self {
            Self::Group => 0xa5,
            Self::Landmark => 0xa6,
            Self::Corpse => 0xa7,
            Self::Guide => 0xa8,
        }
    }

    /// The `.blp` that sequence lights.
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

/// The four rim-arrow textures; a missing one draws no arrow for its source, never another's.
#[derive(Default)]
pub(crate) struct RimArrowArt([Option<Handle<Image>>; 4]);

impl RimArrowArt {
    pub(crate) fn set(&mut self, kind: RimArrow, tex: Option<Handle<Image>>) {
        self.0[kind as usize] = tex;
    }

    pub(super) fn get(&self, kind: RimArrow) -> Option<&Handle<Image>> {
        self.0[kind as usize].as_ref()
    }

    /// Any art at all: the landmark pass runs without `AreaPOI.dbc`, for the guard marker.
    pub(super) fn any(&self) -> bool {
        self.0.iter().any(Option::is_some)
    }
}
/// An in-range POI icon's quad, 16 px (`[0xbc7658]`, frozen by the ctor).
const POI_ICON_PX: f32 = 16.0;

/// The blip under the cursor this frame, for [`drive_blip_tooltip`].
#[derive(Resource, Default)]
pub(crate) enum MinimapBlipHover {
    #[default]
    None,
    /// A landmark's name and the cursor's UI-space point, the tooltip's seat.
    Landmark(String, Vec2),
    /// A unit dot's guid, resolved through the name cache, the cursor point, and whether the dot
    /// drew grey; the tooltip line greys the same way (`|cffb0b0b0`).
    Npc(u64, Vec2, bool),
    /// A tracked GameObject dot: its template name, the cursor point and the grey flag.
    TrackedGo(String, Vec2, bool),
}

/// The blip layer's system inputs, tupled to keep `emit_minimap` under Bevy's 16-parameter cap.
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

/// Every streamed object the classifier considers; our own avatar is the arrow, never a dot.
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

/// The frame geometry the blip emitters draw in; `wx`, `wy`, `wz` are the player's position.
pub(super) struct BlipCtx {
    pub(super) center: Vec2,
    pub(super) side: f32,
    pub(super) px_per_yd: f32,
    /// The live view radius in yards: the in-range split's denominator and the dot cull.
    pub(super) radius_yd: f32,
    pub(super) z: u64,
    pub(super) alpha: f32,
    pub(super) wx: f32,
    pub(super) wy: f32,
    pub(super) wz: f32,
    /// The cursor in the quads' y-down logical px.
    pub(super) cursor: Option<Vec2>,
    /// The cursor in y-up UI space; the reference seats the blip tooltip at the cursor, its exact
    /// offset untraced.
    pub(super) cursor_ui: Option<Vec2>,
    /// Window px per UI unit: everything else here is window px, while Lua
    /// ([`Minimap:PingLocation`](super::ping)) speaks UI units.
    pub(super) seam: f32,
}

impl BlipCtx {
    /// A WoW world point's screen offset from the widget centre, north up (+X up, +Y left).
    pub(super) fn offset(&self, w: [f32; 3]) -> Vec2 {
        Vec2::new(
            (self.wy - w[1]) * self.px_per_yd,
            -(w[0] - self.wx) * self.px_per_yd,
        )
    }
}

/// The landmark selection's two draw lists (`0x6d9a90`).
pub(super) struct LandmarkSelection<'a> {
    /// In-range rows with [`FLAG_IN_RANGE_ICON`], drawn at their position.
    pub(super) icons: Vec<&'a AreaPoi>,
    /// The rim arrows `(dist, poi, art)` in rank order.
    pub(super) arrows: Vec<(f32, &'a AreaPoi, RimArrow)>,
}

/// The landmark selection (module doc). `marker`, the guard-directions POI, is appended after
/// the DBC scan as the reference appends its static blip slot: it skips the candidacy gate but
/// not the 694.444-yd cut, which `0x6d9cc2` spares only the corpse slot `0xcea848`.
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

/// The `POIIcons.blp` 8×8-grid cell for an `Icon`, gated `Icon < 0x40` (`0x4ed15e`).
fn poi_icon_cell(icon: u32) -> Option<[f32; 4]> {
    if icon >= 64 {
        return None;
    }
    let (c, r) = ((icon % 8) as f32, (icon / 8) as f32);
    Some([c / 8.0, (c + 1.0) / 8.0, r / 8.0, (r + 1.0) / 8.0])
}

/// Draw the in-range POI icons, then the rim arrows; the later-drawn hover hit wins.
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

/// One rim arrow on the 0.8 rim, pointing at its target, drawn in one pass as only one of the
/// model's layers is ever opaque. Returns its rect for the hover test.
fn push_rim_arrow(
    ctx: &BlipCtx,
    target: [f32; 3],
    tex: &Handle<Image>,
    quads: &mut UiQuads,
) -> Rect {
    let dir = ctx.offset(target).normalize_or_zero();
    let pos = ctx.center + dir * (ctx.side * 0.5 * BLIP_EDGE_RATIO);
    // Quad rotation is clockwise on a y-down screen and the art points up at zero.
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

/// A party member's position: the streamed transform, else the `PARTY_MEMBER_STATS` position
/// (whole yards on the wire).
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

/// The party and corpse rim arrows, the out-of-range half of the party placement `0x6dad10`,
/// drawn before the player arrow.
///
/// This differs from the reference, not built yet: its slot 4 is the pet (`UNIT_FIELD_CHARM`,
/// else `UNIT_FIELD_SUMMON`, at `0x6dad60`), and the corpse is static blip slot 2 on the POI path,
/// at `Importance` -1 and exempt from the 694.444-yd cut, taking one of the three rim slots.
/// So there is no pet blip here, and four rim arrows can draw where the reference draws three.
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
            continue; // in range: the dot pass draws it
        }
        let Some(tex) = arrows.get(art) else {
            continue;
        };
        push_rim_arrow(ctx, [x, y, 0.0], tex, quads);
    }
}

/// Show the hovered blip's name; a unit's resolves through the name cache. Hover loss fades the
/// tooltip, never hides it at once. Runs after `UnitFeed`, so a same-frame move from a world
/// hover to a blip ends on the blip's tooltip.
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
        // Same blip: the reference's blip tooltip follows the cursor.
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

    /// The call site gives only the sequence id; the model says what it draws.
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

    /// Real 1.12 AreaPOI rows near Northshire Abbey at the default zoom's 133-yd radius.
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

    /// The rank key is `Importance` (DBC column 1), not AreaID; distance only breaks ties.
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

    /// The marker carries `Importance` 0, so it outranks the cities though farthest.
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
        let arts: Vec<RimArrow> = sel.arrows.iter().map(|&(_, _, a)| a).collect();
        assert_eq!(
            arts,
            [RimArrow::Guide, RimArrow::Landmark, RimArrow::Landmark],
            "the guard's directions draw the guide arrow; DBC rows draw the landmark arrow"
        );
    }

    /// Every 1.12 `points_of_interest` row carries flags 99 (0x63), so `Flags & 2` is set.
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

    /// The caller ([`crate::poi_marker::PoiMarker`]'s `on_map`) decides the marker's map.
    #[test]
    fn the_guard_marker_skips_the_candidacy_filter() {
        let mut marker = poi(0, 0, 300.0, "The Inn"); // Flags bit 0 clear: a DBC row would drop
        marker.continent_id = 571; // and a continent that isn't the displayed one
        let sel = select_landmarks(std::iter::empty(), Some(&marker), 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 1, "appended unconditionally");
    }

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
