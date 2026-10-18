//! The minimap blip layer: AreaPOI landmarks, quest and tracking dots, rim arrows and the hover
//! tooltip.
//!
//! Landmarks (selection `0x6d9a90`, candidates `0x6d8e10`, draw `0x4ed148`/`0x4ee170`):
//! - Candidates: `ContinentID` is the current map and `Flags & 1`, no faction or importance gate.
//! - In range (`d / viewRadius ≤ 0.8`): the `POIIcons.blp` cell of the `Icon` column, only for
//!   rows with `Flags & 2` (the world-PvP towers in 1.12 data); other rows draw nothing.
//! - Out of range within 694.444 yd: the first 3 by (`Importance` signed ascending, distance),
//!   each a rim arrow: one of the minimap's engine `Model` children, which the VM aims at its POI
//!   and the model pane renderer draws in its source's art ([`RimArrow`]), at a level above the
//!   minimap's own draw, so above every dot.
//! - After the DBC rows, the static records: the gossip marker (slot 1) and the corpse (slot 2,
//!   icon 7, flags 2, rank −1, `0x6dacd0`), which alone skips the 694.444-yd cut (`0x6d9cc2`).
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
//! Not built: the landmark `WorldStateID` gate (`0x6d9b27`) and static blip slot 0,
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
use benilla_ui::script::{MinimapRimArrow, UiScript};

use crate::go_templates::GameObjectTemplates;
use crate::names::NameCache;
use crate::net::{Embodied, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
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
/// The rim arrow's quad for the hover test: `Rotating-MinimapArrow.m2`'s 0.0500-unit quads at
/// 768 units per model unit (model scale 0.6 × 1280).
const ARROW_QUAD_PX: f32 = 38.4;

/// Which source a landmark rim arrow comes from: an `AreaPOI.dbc` row (`+0x48 == -2`, sequence
/// 166 `Arrow`), the guard's directions, the gossip marker (static slot 1, sequence 168
/// `GuideArrow`), or the corpse (static slot 2, sequence 167 `CorpseArrow`). The VM picks the
/// sequence from the slot (`0x4ed353`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RimArrow {
    /// An `AreaPOI.dbc` landmark.
    Landmark,
    /// The guard's directions ([`crate::poi_marker`]).
    Guide,
    /// The player's corpse ([`corpse_landmark`]).
    Corpse,
}

impl RimArrow {
    /// The arrow record's static slot (`+0x48`, `0x6dab26`–`0x6dab41`), −2 for a DBC row.
    fn slot(self) -> i32 {
        match self {
            RimArrow::Landmark => -2,
            RimArrow::Guide => 1,
            RimArrow::Corpse => 2,
        }
    }
}

/// The corpse's static blip (`0x6dacd0`): slot 2, icon 7, flags 2, rank −1, named by
/// `CORPSE_RED`, present only while the corpse is on the map the player is on (`0x492010`).
pub(crate) fn corpse_landmark(position: [f32; 3], map_id: u32, name: String) -> AreaPoi {
    AreaPoi {
        importance: -1_i32 as u32,
        icon: 7,
        faction_id: 0,
        pos: position,
        continent_id: map_id,
        flags: 2,
        area_id: 0,
        name,
        description: String::new(),
        world_state_id: 0,
    }
}

/// An in-range POI icon's quad, 16 px (`[0xbc7658]`, frozen by the ctor).
const POI_ICON_PX: f32 = 16.0;

/// The blip under the cursor this frame, for [`drive_blip_tooltip`].
#[derive(Resource, Default)]
pub(crate) enum MinimapBlipHover {
    #[default]
    None,
    /// A landmark's name.
    Landmark(String),
    /// A unit dot's guid, resolved through the name cache, and whether the dot drew grey; the
    /// tooltip line greys the same way (`|cffb0b0b0`).
    Npc(u64, bool),
    /// A tracked GameObject dot: its template name and the grey flag.
    TrackedGo(String, bool),
}

/// The blip layer's system inputs, tupled to keep `emit_minimap` under Bevy's 16-parameter cap.
pub(super) type BlipInputs<'w, 's> = (
    Res<'w, crate::ui_quest::QuestGiver>,
    Res<'w, GuidIndex>,
    Query<'w, 's, &'static GlobalTransform, With<NetEntity>>,
    Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    ResMut<'w, MinimapBlipHover>,
    Res<'w, crate::ui_party::GroupState>,
    TrackedCandidates<'w, 's>,
    Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    Res<'w, NameCache>,
    Res<'w, GameObjectTemplates>,
    Option<Res<'w, crate::go_templates::Locks>>,
    Res<'w, crate::poi_marker::PoiMarker>,
    Option<Res<'w, crate::area_poi::AreaPoiRes>>,
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
    pub(super) z: u128,
    pub(super) alpha: f32,
    pub(super) wx: f32,
    pub(super) wy: f32,
    pub(super) wz: f32,
    /// The cursor in the quads' y-down logical px.
    pub(super) cursor: Option<Vec2>,
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

/// The landmark selection (module doc). `marker`, the guard-directions POI, and `corpse` are
/// appended after the DBC scan as the reference appends its static blip slots 1 and 2: both skip
/// the candidacy gate, and only the corpse (`0xcea848`) skips the 694.444-yd cut (`0x6d9cc2`).
pub(super) fn select_landmarks<'a>(
    pois: impl Iterator<Item = &'a AreaPoi>,
    marker: Option<&'a AreaPoi>,
    corpse: Option<&'a AreaPoi>,
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
        .chain(marker.map(|m| (m, RimArrow::Guide)))
        .chain(corpse.map(|c| (c, RimArrow::Corpse)));
    for (p, art) in candidates {
        let d = ((p.pos[0] - wx).powi(2) + (p.pos[1] - wy).powi(2)).sqrt();
        if d / radius_yd <= BLIP_EDGE_RATIO {
            if p.flags & FLAG_IN_RANGE_ICON != 0 {
                icons.push(p);
            }
        } else if d <= LANDMARK_RANK_YD || art == RimArrow::Corpse {
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

/// Draw the in-range POI icons, then hover-test the rim arrows, which the engine children draw;
/// the later hit wins.
pub(super) fn emit_landmarks(
    ctx: &BlipCtx,
    cat: Option<&AreaPoiCatalog>,
    marker: Option<&AreaPoi>,
    corpse: Option<&AreaPoi>,
    map_id: u32,
    poi_icons: Option<&Handle<Image>>,
    quads: &mut UiQuads,
    hover: &mut MinimapBlipHover,
) {
    let sel = select_landmarks(
        cat.into_iter().flat_map(|c| c.rows().map(|(_, p)| p)),
        marker,
        corpse,
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
            if let Some(c) = ctx.cursor {
                if rect.contains(c) {
                    *hover = MinimapBlipHover::Landmark(poi.name.clone());
                }
            }
        }
    }
    for (_d, poi, _art) in &sel.arrows {
        if let Some(c) = ctx.cursor {
            if rim_arrow_rect(ctx, poi.pos).contains(c) {
                *hover = MinimapBlipHover::Landmark(poi.name.clone());
            }
        }
    }
}

/// A rim arrow's square on the 0.8 rim toward its target, for the hover test.
fn rim_arrow_rect(ctx: &BlipCtx, target: [f32; 3]) -> Rect {
    let dir = ctx.offset(target).normalize_or_zero();
    let pos = ctx.center + dir * (ctx.side * 0.5 * BLIP_EDGE_RATIO);
    Rect::from_center_size(pos, Vec2::splat(ctx.side * (ARROW_QUAD_PX / BLIP_BASIS_PX)))
}

/// The bearing from the player to `(x, y)` in radians from north toward west, the reference's
/// `atan2` of the offset (`0x6d9dd1`, `0x6daf7f`), which is also the arrow's facing.
pub(super) fn bearing(wx: f32, wy: f32, x: f32, y: f32) -> f32 {
    (y - wy).atan2(x - wx)
}

/// A kept landmark's identity, its name and position bits.
pub(super) type ArrowKey = (String, [u32; 2]);

/// The landmark selection as the render's arrow records (`0x6daa20`), with each record's identity
/// for the rebuild test: the list is rebuilt when its count or any kept record changes
/// (`0x6d9d82`–`0x6d9dfb`).
pub(super) fn rim_arrow_records(
    sel: &LandmarkSelection<'_>,
    wx: f32,
    wy: f32,
) -> (Vec<MinimapRimArrow>, Vec<ArrowKey>) {
    sel.arrows
        .iter()
        .map(|(_, poi, art)| {
            (
                MinimapRimArrow {
                    bearing: bearing(wx, wy, poi.pos[0], poi.pos[1]),
                    slot: art.slot(),
                    // The gossip marker and the corpse show indoors too (`+0x44`, `0x6daaf4`,
                    // `0x6daafb`).
                    indoors: *art != RimArrow::Landmark,
                },
                (
                    poi.name.clone(),
                    [poi.pos[0].to_bits(), poi.pos[1].to_bits()],
                ),
            )
        })
        .unzip()
}

/// A party member's position: the streamed transform, else the `PARTY_MEMBER_STATS` position
/// (whole yards on the wire).
pub(crate) fn party_member_pos<F: bevy::ecs::query::QueryFilter>(
    m: &benilla_protocol::messages::GroupMemberEntry,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, F>,
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

/// The party and pet arrow frames' bearings, the out-of-range half of the party placement
/// `0x6dad10`: slots 0 to 3 the party members, slot 4 the pet, our charm else our summon
/// (`0x6dad60`–`0x6dad9c`), each fetched as a unit (`0x6dadc8`); a member that is not streamed
/// falls back to its party-stats position, the pet to nothing (`0x6dae25`). Beyond 0.8 of the
/// radius it has an arrow at its bearing (`0x6daf5c`–`0x6daf7f`).
pub(super) fn group_arrow_records<F: bevy::ecs::query::QueryFilter>(
    wx: f32,
    wy: f32,
    radius_yd: f32,
    group: &crate::ui_party::GroupState,
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, F>,
    pet: Option<u64>,
) -> [Option<f32>; benilla_ui::widget::MINIMAP_GROUP_ARROWS] {
    let arrow = |(x, y): (f32, f32)| {
        (((x - wx).powi(2) + (y - wy).powi(2)).sqrt() / radius_yd > BLIP_EDGE_RATIO)
            .then(|| bearing(wx, wy, x, y))
    };
    let mut records = [None; benilla_ui::widget::MINIMAP_GROUP_ARROWS];
    for (i, m) in group.party_slots().enumerate().take(4) {
        records[i] = party_member_pos(m, group, guids, unit_pos).and_then(arrow);
    }
    records[4] = pet
        .and_then(|g| guids.0.get(&g))
        .and_then(|e| unit_pos.get(*e).ok())
        .map(|tf| {
            let w = bevy_to_wow(tf.translation());
            (w[0], w[1])
        })
        .and_then(arrow);
    records
}

/// Show the hovered blip's name; a unit's resolves through the name cache. This is the in-rect
/// half of the Minimap's probe `0x4eb0c0`, which the hover walk runs on the Minimap while it holds
/// the hover or the capture ([`UiScript::minimap_probed`]). A new blip shows; no blip where one
/// was held hides `GameTooltip`, whoever holds it (`0x4eb55f`). The cursor off the Minimap is the
/// walk's: the probe's miss and the Minimap's leave.
pub(super) fn drive_blip_tooltip(
    script: Option<NonSendMut<UiScript>>,
    hover: Res<MinimapBlipHover>,
    names: Res<NameCache>,
    objects: crate::net::Objects,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    if !script.minimap_probed() {
        return;
    }
    let show = match &*hover {
        MinimapBlipHover::None => None,
        MinimapBlipHover::Landmark(name) => Some((name.clone(), false)),
        // A live object names itself through its name method (`0x4eb31a`, vtable slot 28,
        // `GetUnitName` with a callback), which every probe runs, so a refused name is asked
        // again about once per answer; a guid with no object only reads (`0x4eb33c`), and so does
        // a creature's record (`0x60934b`).
        MinimapBlipHover::Npc(npc, grey) => {
            let ask = if objects.store(*npc).is_some() {
                Ask::Event(&commands)
            } else {
                Ask::Read
            };
            names
                .resolve(&objects, *npc, ask)
                .map(|n| (n.to_string(), *grey))
        }
        MinimapBlipHover::TrackedGo(name, grey) => Some((name.clone(), *grey)),
    };
    match (show, script.minimap_blip()) {
        // Same blip: the plate, owned in mode 6, follows the cursor in its own update.
        (Some((t, _)), Some(held)) if t == held => {}
        (Some((t, grey)), _) => {
            script.minimap_tooltip(&t, grey);
        }
        (None, Some(_)) => script.minimap_blip_lost(),
        (None, None) => {}
    }
}

#[cfg(test)]
mod tooltip_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The VM arms only a sequence id (`0x4ed353`, `0x4ee1de`); the model says what it draws: in
    /// each looping sequence exactly one arrow layer's stepped alpha is 1.0, untinted.
    #[test]
    fn each_rim_arrow_sequence_lights_exactly_its_own_layer() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::Chain::open(&data).expect("the patch chain");
        let bytes = chain
            .read_file("Interface\\Minimap\\Rotating-MinimapArrow.m2")
            .expect("the one rim-arrow model");

        for (anim_id, texture) in [
            (
                benilla_ui::script::MINIMAP_GROUP_ARROW,
                "Interface\\Minimap\\Rotating-MinimapGroupArrow",
            ),
            (
                benilla_ui::script::MINIMAP_LANDMARK_ARROW,
                "Interface\\Minimap\\Rotating-MinimapArrow",
            ),
            (
                benilla_ui::script::MINIMAP_CORPSE_ARROW,
                "Interface\\Minimap\\Rotating-MinimapCorpseArrow",
            ),
            (
                benilla_ui::script::MINIMAP_GUIDE_ARROW,
                "Interface\\Minimap\\Rotating-MinimapGuideArrow",
            ),
        ] {
            let shown = benilla_formats::m2_sequence_visible_textures(&bytes, anim_id)
                .unwrap_or_else(|| {
                    panic!(
                        "the model authors no sequence {anim_id:#x}: the id is wrong, or this is \
                         not the model the reference animates"
                    )
                });
            let want = format!("{}.BLP", texture.to_uppercase());
            assert_eq!(
                shown,
                vec![want],
                "sequence {anim_id:#x} must light its layer and ONLY its layer"
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
        let sel = select_landmarks(rows.iter(), None, None, 0, 0.0, 0.0, 133.0);
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
        let sel = select_landmarks(rows.iter(), None, None, 0, 0.0, 0.0, 100.0);
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
        let sel = select_landmarks(cities.iter(), Some(&marker), None, 0, 0.0, 0.0, 133.0);
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
        let sel = select_landmarks(std::iter::empty(), Some(&marker), None, 0, 0.0, 0.0, 133.0);
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
        let sel = select_landmarks(std::iter::empty(), Some(&marker), None, 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 1, "appended unconditionally");
    }

    #[test]
    fn in_range_icons_gate_on_flag_bit1_and_the_atlas_bound() {
        let mut tower = poi(0, 0x87, 50.0, "Crown Guard Tower");
        tower.icon = 9; // col 1, row 1
        let plain = poi(3, 0x5, 50.0, "Northshire Abbey");
        let rows = [tower, plain];
        let sel = select_landmarks(rows.iter(), None, None, 0, 0.0, 0.0, 133.0);
        assert_eq!(sel.arrows.len(), 0);
        assert_eq!(sel.icons.len(), 1, "only the Flags&2 tower draws in range");
        assert_eq!(
            poi_icon_cell(9),
            Some([0.125, 0.25, 0.125, 0.25]),
            "icon 9 = col 1 row 1 of the 8x8 atlas"
        );
        assert_eq!(poi_icon_cell(64), None, "the client gates Icon < 0x40");
    }

    /// The corpse (`0x6dacd0`, rank −1) ranks ahead of every row and the gossip marker, keeps its
    /// rim slot past 694.444 yd where the marker is cut (`0x6d9cc2`), and so leaves two of the three
    /// slots; its record is static slot 2, shown indoors. In range it is an icon (flags 2), cell 7.
    #[test]
    fn the_corpse_takes_a_rim_slot_first_and_alone_skips_the_rank_cut() {
        let cities = [
            poi(0, 0x5, 200.0, "Goldshire"),
            poi(0, 0x5, 300.0, "Northshire Abbey"),
            poi(0, 0x5, 400.0, "Stormwind"),
        ];
        let far = corpse_landmark([2000.0, 0.0, 0.0], 0, "Corpse".into());
        let marker = poi(0, 0x63, 700.0, "The Bank");
        let sel = select_landmarks(cities.iter(), Some(&marker), Some(&far), 0, 0.0, 0.0, 133.0);
        let names: Vec<&str> = sel.arrows.iter().map(|(_, p, _)| p.name.as_str()).collect();
        assert_eq!(names, ["Corpse", "Goldshire", "Northshire Abbey"]);
        let (rim, _) = rim_arrow_records(&sel, 0.0, 0.0);
        assert_eq!((rim[0].slot, rim[0].indoors), (2, true));
        assert_eq!((rim[1].slot, rim[1].indoors), (-2, false));

        let near = corpse_landmark([50.0, 0.0, 0.0], 0, "Corpse".into());
        let sel = select_landmarks(std::iter::empty(), None, Some(&near), 0, 0.0, 0.0, 133.0);
        assert!(sel.arrows.is_empty());
        assert_eq!(sel.icons.len(), 1);
        assert_eq!(
            poi_icon_cell(sel.icons[0].icon),
            Some([0.875, 1.0, 0.0, 0.125])
        );
    }

    /// Slot 4 is the pet, our charm else our summon, found only as a streamed unit (`0x6dad60`,
    /// `0x6dae25`); out of range it has an arrow at its bearing, and no corpse ever lands there.
    #[test]
    fn the_fifth_group_arrow_is_the_pet_and_only_a_streamed_one() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let far = world
            .spawn(GlobalTransform::from_translation(
                benilla_assets::coords::wow_to_bevy([500.0, 0.0, 0.0]),
            ))
            .id();
        let mut guids = GuidIndex::default();
        guids.0.insert(0xF140, far);
        world.insert_resource(guids);
        world.init_resource::<crate::ui_party::GroupState>();
        let records = |world: &mut World, pet: Option<u64>| {
            world
                .run_system_once(
                    move |q: Query<&GlobalTransform>,
                          guids: Res<GuidIndex>,
                          group: Res<crate::ui_party::GroupState>| {
                        group_arrow_records(0.0, 0.0, 133.0, &group, &guids, &q, pet)
                    },
                )
                .unwrap()
        };
        let got = records(&mut world, Some(0xF140));
        assert!(got[..4].iter().all(Option::is_none));
        assert!(got[4].is_some_and(|b| b.abs() < 1e-6), "due north: {got:?}");
        assert_eq!(
            records(&mut world, Some(0xF141))[4],
            None,
            "not streamed: no arrow"
        );
        assert_eq!(records(&mut world, None)[4], None);
    }
}
