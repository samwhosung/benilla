//! The player's area identity for the UI: the `ZONE_CHANGED` event family and the host globals
//! behind `GetZoneText`, `GetSubZoneText`, `GetRealZoneText`, `GetMinimapZoneText` and
//! `GetZonePVPInfo`, all fired from the one updater `0x494780`.
//!
//! - A zone id change fires `ZONE_CHANGED_NEW_AREA` alone; else a text change fires
//!   `ZONE_CHANGED_INDOORS` or `ZONE_CHANGED` by the indoor bit. The first world-enter is a zone
//!   id change from the zeroed cache.
//! - The zone is the leaf's single-hop parent (AreaTable field 2, itself when 0).
//! - The indoor bit is the faces-only down-ray (`0x6a87f0`): the surface below is a WMO face whose
//!   group lacks MOGP `0x8` EXTERIOR.
//! - Indoor naming (`0x67e670`): while indoors in the same group the whole updater is skipped
//!   (`[0x868608]`, `[0x86860c]`). On a change, the whole-WMO row (`0x69d830`) overrides the zone
//!   slot and nulls the subzone when its name is non-empty and differs from the subzone; an
//!   unnamed row resolves through its `AreaTableID`, a missing row never overrides. Then the
//!   group's own row (`0x69d8f0`) refills the subzone. Both are exact-key lookups.
//! - `GetRealZoneText` reads the zone name before the indoor override.
//! - `GetMinimapZoneText` is subzone-else-zone (`0xb4da28`), with its own `MINIMAP_ZONE_CHANGED`
//!   (`0x494970`).
//!
//! On the first world-enter `0x494780` also sets the world map to the player's zone
//! (`0x4a6650`); that half lives in `crate::ui_world_map::world_enter_selection`.
//!
//! Host globals are written before the event fires, so a handler's `GetZoneText()` sees the new
//! state.
//!
//! Deviation: the indoor dedup keys `(WMOID, MOGP uniqueID)`, not the client's raw per-WMO group
//! index, because the raw index can alias across adjacent buildings.
//!
//! The reference's PvP display gate `[0x88272c]` (a cached boolean, not the realm type) is
//! untraced; ours is always open.

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_formats::AreaTableCatalog;
use benilla_ui::script::UiScript;

use crate::net::{ObjectStore, SelfPlayer};
use crate::target::Factions;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// The shared `AreaTable.dbc` catalog, read by the world map and this module; absent if the DBC
/// failed to read.
#[derive(Resource)]
pub(crate) struct AreaTableRes(pub(crate) AreaTableCatalog);

fn load_area_table(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_area_table_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("area: {} rows in the shared AreaTable catalog", cat.len());
            commands.insert_resource(AreaTableRes(cat));
        }
        Err(e) => warn!("area: AreaTable catalog failed to load: {e:#}"),
    }
}

/// The client's zone-text cache: the compare targets of `0x494780`, the minimap line's cache
/// (`0x494970`) and the indoor dedup pair (`[0x868608]`, `[0x86860c]`). It lives per VM, so every
/// login starts zeroed and its first resolve is a NEW_AREA.
#[derive(Default)]
struct ZoneCache {
    /// `None` until the first resolve.
    zone_id: Option<u32>,
    zone_text: String,
    subzone_text: String,
    minimap_text: String,
    /// `[0x868608]`.
    indoor: bool,
    /// The previous hit group (`[0x86860c]`, keyed as the module doc's deviation says).
    wmo_id: u32,
    wmo_group: u32,
}

/// One resolve's compare inputs.
struct ZoneSignal {
    zone_id: u32,
    zone_text: String,
    subzone_text: String,
    indoor: bool,
}

/// The event election of `0x494780`.
fn elect_event(cache: &ZoneCache, next: &ZoneSignal) -> Option<&'static str> {
    if cache.zone_id != Some(next.zone_id) {
        return Some("ZONE_CHANGED_NEW_AREA");
    }
    if cache.zone_text != next.zone_text || cache.subzone_text != next.subzone_text {
        return Some(if next.indoor {
            "ZONE_CHANGED_INDOORS"
        } else {
            "ZONE_CHANGED"
        });
    }
    None
}

/// Resolves the texts and PvP info, writes the host globals, then fires the elected zone event and,
/// independently, `MINIMAP_ZONE_CHANGED`.
fn feed_zone_events(
    script: Option<NonSendMut<UiScript>>,
    world: benilla_world::world_point::WorldPoint,
    areas: Option<Res<AreaTableRes>>,
    factions: Option<Res<Factions>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    mut cache: Local<crate::ui_script::VmMemo<ZoneCache>>,
) {
    let (Some(mut script), Some(areas)) = (script, areas) else {
        return;
    };
    let cache = cache.get(&script);
    let Some(leaf) = world.area() else { return };
    let Some(leaf_row) = areas.0.get(leaf) else {
        // An unknown id keeps the last state, as the resolver does for areaId 0.
        return;
    };
    let zone = if leaf_row.zone_id == 0 {
        leaf
    } else {
        leaf_row.zone_id
    };
    let real_zone_text = areas.0.name(zone).unwrap_or_default().to_string();
    let indoor = world.area_interior().is_some();

    // The outdoor texts `0x67e670` starts from.
    let mut zone_text = real_zone_text.clone();
    let mut subzone_text = if leaf == zone {
        String::new()
    } else {
        leaf_row.name.clone()
    };

    let mut wmo_group = 0u32;
    if let Some((k, group, default)) = world.area_interior_rows() {
        // The client keys `[groupRec+0x7c]`, nonzero-gated; see the module doc's deviation.
        wmo_group = k.group_area_id;
        if cache.indoor
            && k.group_area_id != 0
            && (cache.wmo_id, cache.wmo_group) == (k.wmo_id, wmo_group)
        {
            return;
        }
        cache.wmo_id = k.wmo_id;
        // The whole-WMO -1 row. An unnamed one with `AreaTableID` 0 takes the leaf's name, which
        // is the terrain areaId the client's area-0 arm reads.
        let a_name = default.as_ref().map(|d| {
            if !d.name.is_empty() {
                d.name.clone()
            } else if d.area_table_id != 0 {
                areas
                    .0
                    .name(d.area_table_id)
                    .unwrap_or_default()
                    .to_string()
            } else {
                leaf_row.name.clone()
            }
        });
        if let Some(a) = a_name.filter(|n| !n.is_empty()) {
            if a != subzone_text {
                zone_text = a;
                subzone_text = String::new();
            }
        }
        // The hit group's own row refills the subzone.
        if let Some(b) = group
            .as_ref()
            .map(|r| r.name.as_str())
            .filter(|n| !n.is_empty())
        {
            subzone_text = b.to_string();
        }
    }
    let signal = ZoneSignal {
        zone_id: zone,
        zone_text,
        subzone_text,
        indoor,
    };
    let minimap_text = if signal.subzone_text.is_empty() {
        signal.zone_text.clone()
    } else {
        signal.subzone_text.clone()
    };

    let event = elect_event(cache, &signal);
    let minimap_changed = cache.minimap_text != minimap_text;
    if event.is_none() && !minimap_changed {
        return;
    }

    // GetZonePVPInfo (`0x48d540`): isArena is the leaf's Flags `0x80`; the type is the zone's
    // FactionGroupMask against the player template's friend then enemy masks, "contested" when
    // neither; never "arena"; nil only on structural failure. factionName is FactionGroup.dbc's
    // Name for the zone's mask bit; the realm type never enters.
    let is_arena = leaf_row.flags & 0x80 != 0;
    let zone_mask = areas.0.get(zone).map_or(0, |r| r.faction_group_mask);
    let pvp = factions
        .as_ref()
        .zip(self_store.single().ok())
        .and_then(|(f, store)| {
            let tpl = f.catalog().template(store.0.unit_faction_template()?)?;
            let ty = if zone_mask & tpl.friend_group_mask != 0 {
                "friendly"
            } else if zone_mask & tpl.enemy_group_mask != 0 {
                "hostile"
            } else {
                "contested"
            };
            Some((ty, f.catalog().faction_group_name(zone_mask).unwrap_or("")))
        });
    let (pvp_type, pvp_faction) = pvp.unwrap_or(("", ""));

    let globals = script.lua().globals();
    let pushed = globals
        .set("__benilla_zone_name", signal.zone_text.clone())
        .and_then(|()| globals.set("__benilla_real_zone_name", real_zone_text))
        .and_then(|()| globals.set("__benilla_subzone_name", signal.subzone_text.clone()))
        .and_then(|()| globals.set("__benilla_zone_text", minimap_text.clone()))
        .and_then(|()| globals.set("__benilla_pvp_type", pvp_type))
        .and_then(|()| globals.set("__benilla_pvp_faction", pvp_faction))
        .and_then(|()| globals.set("__benilla_pvp_arena", is_arena));
    if let Err(e) = pushed {
        warn!("area: zone host globals: {e}");
        return;
    }

    if let Some(event) = event {
        script.fire_event(event, vec![]);
        debug!(
            "area: {event} → zone {:?} / sub {:?} ({pvp_type})",
            signal.zone_text, signal.subzone_text
        );
    }
    if minimap_changed {
        script.fire_event("MINIMAP_ZONE_CHANGED", vec![]);
        debug!("area: minimap zone text → {minimap_text:?}");
    }
    cache.zone_id = Some(signal.zone_id);
    cache.zone_text = signal.zone_text;
    cache.subzone_text = signal.subzone_text;
    cache.minimap_text = minimap_text;
    cache.indoor = signal.indoor;
    cache.wmo_group = wmo_group;
}

/// The shared area catalog and the zone-event feed.
pub(crate) struct AreaPlugin;

impl Plugin for AreaPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_area_table.after(AssetSet::Open))
            .add_systems(
                Update,
                // After the leaf authority: leaf, indoor bit and names must come from one frame,
                // as in the client's single-pass resolve (`0x67e510`), or a stale leaf fires a
                // spurious splash.
                feed_zone_events
                    .after(crate::ui_script::UiInput)
                    .after(benilla_world::terrain_stream::AreaAuthoritySet),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(zone_id: u32, zone: &str, sub: &str, indoor: bool) -> ZoneSignal {
        ZoneSignal {
            zone_id,
            zone_text: zone.into(),
            subzone_text: sub.into(),
            indoor,
        }
    }

    fn cache(zone_id: Option<u32>, zone: &str, sub: &str) -> ZoneCache {
        ZoneCache {
            zone_id,
            zone_text: zone.into(),
            subzone_text: sub.into(),
            ..ZoneCache::default()
        }
    }

    /// The `0x494780` election.
    #[test]
    fn election_matches_the_byte_law() {
        let c = ZoneCache::default();
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Northshire Valley", false)),
            Some("ZONE_CHANGED_NEW_AREA")
        );

        // Zone hop: NEW_AREA alone, though the texts changed too.
        let c = cache(Some(12), "Elwynn Forest", "");
        assert_eq!(
            elect_event(&c, &sig(40, "Westfall", "The Jansen Stead", false)),
            Some("ZONE_CHANGED_NEW_AREA")
        );

        let c = cache(Some(12), "Elwynn Forest", "");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            Some("ZONE_CHANGED")
        );

        // Inn entry: the indoor override changed the zone text.
        let c = cache(Some(12), "Elwynn Forest", "Goldshire");
        assert_eq!(
            elect_event(&c, &sig(12, "Lion's Pride Inn", "", true)),
            Some("ZONE_CHANGED_INDOORS")
        );

        let c = cache(Some(12), "Lion's Pride Inn", "");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            Some("ZONE_CHANGED")
        );

        let c = cache(Some(12), "Elwynn Forest", "Goldshire");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            None
        );
    }

    /// The `0x67e670` override skip (abbey) and fire (inn) branches on the install's data.
    #[test]
    fn indoor_naming_matches_the_byte_law_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let cat = benilla_formats::load_wmo_area_catalog(&mut chain).expect("WMOAreaTable");

        // Abbey (WMO 59, name-set 1): the default name equals the yard subzone, so no override.
        let default = cat.default_row(59, 1).expect("abbey default row");
        assert_eq!(default.name, "Northshire Abbey");
        let group = cat.group_row(59, 1, 1934).expect("abbey main hall row");
        assert_eq!(group.name, "Main Hall");
        assert_eq!(
            group.area_table_id, 24,
            "the leaf stays Northshire Abbey (24)"
        );
        // Rooms have distinct rows, so a room hop re-fires the dedup.
        let library = cat.group_row(59, 1, 1943).expect("library row");
        assert_ne!(group.id, library.id, "distinct rows exist per room");

        let unnamed = cat.group_row(59, 1, 1935).expect("unnamed room row");
        assert!(unnamed.name.is_empty());

        // Goldshire inn (WMO 53, name-set 2): only a whole-WMO row, named unlike the street, so
        // the override fires and the subzone stays nulled.
        let inn = cat.default_row(53, 2).expect("the inn's whole-WMO row");
        assert_eq!(inn.name, "Lion's Pride Inn");
        assert_eq!(inn.area_table_id, 0);
        assert!(
            cat.group_row(53, 2, 0).is_none_or(|g| g.name.is_empty()),
            "no named group rows — query B leaves the nulled subzone alone"
        );
    }
}
