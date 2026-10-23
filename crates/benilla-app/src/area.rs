//! The player's area identity for the UI: the `ZONE_CHANGED` event family and the host globals
//! behind `GetZoneText`, `GetSubZoneText`, `GetRealZoneText`, `GetMinimapZoneText` and
//! `GetZonePVPInfo`, all fired from the one updater `0x494780`.
//!
//! - A zone id change fires `ZONE_CHANGED_NEW_AREA` alone; else a text change fires
//!   `ZONE_CHANGED_INDOORS` or `ZONE_CHANGED` by the indoor bit. The first world-enter is a zone
//!   id change from the zeroed cache.
//! - The leaf is the area query's answer (`0x6703a0`): on a map whose WDT holds one global WMO,
//!   `Map.dbc` field 19 for every entity; elsewhere the down-ray's WMO row, else the terrain. The
//!   indoor naming below still applies on such a map, as `0x67e510` takes the indoor arm by the
//!   indoor bit alone.
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
//! `GetZonePVPInfo`'s type and faction show only on a PvP realm or in one of the six capitals
//! (zone Flags `0x10`); elsewhere they are nil and the stock zone texts stay uncoloured. The
//! commit pushes the zone's row; the realm flag here and our faction template (`crate::ui_unit`)
//! are pushed as they change, and the getter reads all three at the call (`0x48d540`).

use bevy::prelude::*;

use benilla_formats::{AreaTableCatalog, AreaTableRow};

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
    /// The realm flag last pushed to this VM.
    realm_pvp: Option<bool>,
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

/// Resolves the texts and PvP info, pushes the zone caches, then fires the elected zone event and,
/// independently, `MINIMAP_ZONE_CHANGED`. The poll reports the active mover's position, else the
/// active player's, and nothing with neither held (`0x5db937`, `0x5db956`, `0x5db95d`): after a
/// worldport frees every object, the commit waits for our player's create. It runs on the tick
/// after the tick's packets (category 6's drain `0x403620`, then category 5's `0x401ec0`), so it
/// reads what the unit feed pushed of a player that arrived this frame.
fn feed_zone_events(
    mut hosted_vm: crate::script_calls::HostedVm,
    me: Query<(), (With<crate::net::SelfPlayer>, With<crate::net::ObjectStore>)>,
    world: benilla_world::world_point::WorldPoint,
    areas: Option<Res<AreaTableRes>>,
    factions: Option<Res<Factions>>,
    realm_pvp: Option<Res<RealmPvp>>,
    mut cache: Local<crate::ui_script::VmMemo<ZoneCache>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let cache = cache.get(script);
        let realm_pvp = realm_pvp.is_some_and(|r| r.0);
        if cache.realm_pvp != Some(realm_pvp) {
            cache.realm_pvp = Some(realm_pvp);
            script.set_realm_pvp(realm_pvp);
        }
        let Some(areas) = areas else {
            return;
        };
        if me.is_empty() {
            return;
        }
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
            // The whole-WMO -1 row. An unnamed one with `AreaTableID` 0 takes the leaf's name,
            // which is the terrain areaId the client's area-0 arm reads.
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

        script.set_zone_texts(benilla_ui::script::ZoneTexts {
            zone: signal.zone_text.clone(),
            real_zone: real_zone_text,
            subzone: signal.subzone_text.clone(),
            minimap: minimap_text.clone(),
            pvp_zone: zone_pvp(areas.0.get(zone), factions.as_deref()),
            is_arena: leaf_row.flags & 0x80 != 0,
        });

        if let Some(event) = event {
            script.fire_event(event, vec![]);
            debug!(
                "area: {event} → zone {:?} / sub {:?}",
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
    })
}

/// What `GetZonePVPInfo` (`0x48d540`) reads of the committed zone's row `zone`: the capital bit
/// (Flags `0x10`, `0x48d5cc`), the `FactionGroupMask` (`+0x50`) and the faction name,
/// `FactionGroup.dbc`'s for the mask, `""` when no row names one (`0x48d633`). `None` with no row
/// (`0x48d5b4`).
fn zone_pvp(
    zone: Option<&AreaTableRow>,
    factions: Option<&Factions>,
) -> Option<benilla_ui::script::ZonePvp> {
    let zone = zone?;
    let mask = zone.faction_group_mask;
    Some(benilla_ui::script::ZonePvp {
        capital: zone.flags & 0x10 != 0,
        faction_group_mask: mask,
        faction_name: factions
            .and_then(|f| f.catalog().faction_group_name(mask))
            .unwrap_or("")
            .to_string(),
    })
}

/// `[0x88272c]`, the realm's PvP flag the display gate reads: the selected realm's
/// `PlayerKillingAllowed` in `Cfg_Configs.dbc`, stored once per world entry from the character
/// list (`0x4015de` in `0x401570`), so a `/reload` or a far teleport keeps it.
#[derive(Resource, Default)]
pub(crate) struct RealmPvp(bool);

/// Latch [`RealmPvp`] from the realm this session entered the world on.
fn latch_realm_pvp(
    mut latch: ResMut<RealmPvp>,
    roster: Option<Res<crate::glue_session::roster::Roster>>,
    tables: Option<Res<crate::glue_session::realm::RealmTables>>,
) {
    let realm_type = roster.and_then(|r| r.realm.as_ref().map(|realm| realm.realm_type));
    let row = realm_type
        .zip(tables)
        .and_then(|(t, tables)| tables.row_pvp(t));
    latch.0 = latched_realm_pvp(latch.0, row);
}

/// A realm type with no `Cfg_Configs.dbc` row skips the store (`0x4015d2`) and keeps the last
/// world entry's value, which the process starts at 0 (`.data`).
fn latched_realm_pvp(previous: bool, row_pvp: Option<bool>) -> bool {
    row_pvp.unwrap_or(previous)
}

/// The shared area catalog and the zone-event feed.
pub(crate) struct AreaPlugin;

impl Plugin for AreaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RealmPvp>()
            .add_systems(Startup, load_area_table.after(AssetSet::Open))
            .add_systems(
                OnEnter(crate::client_state::ClientState::InWorld),
                latch_realm_pvp,
            )
            .add_systems(
                Update,
                // After the leaf authority: leaf, indoor bit and names must come from one frame,
                // as in the client's single-pass resolve (`0x67e510`), or a stale leaf fires a
                // spurious splash. After the unit feed too, by the sets: the commit's handlers read
                // our template as the frame's packets left it.
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

    fn row(flags: u32, faction_group_mask: u32) -> AreaTableRow {
        AreaTableRow {
            map_id: 0,
            zone_id: 0,
            explore_flag: 0,
            flags,
            faction_group_mask,
            exploration_level: 0,
            name: String::new(),
        }
    }

    /// The zone's row as the getter reads it: the capital bit, the mask and its faction name,
    /// `""` for a mask no `FactionGroup` row names; no row, nothing to read.
    #[test]
    fn the_commit_pushes_the_zones_capital_bit_mask_and_faction_name() {
        let orgrimmar = row(0x138, 4);
        let stranglethorn = row(0x40, 0);
        assert_eq!(
            zone_pvp(Some(&orgrimmar), None),
            Some(benilla_ui::script::ZonePvp {
                capital: true,
                faction_group_mask: 4,
                faction_name: String::new(),
            })
        );
        assert!(!zone_pvp(Some(&stranglethorn), None).unwrap().capital);
        assert_eq!(zone_pvp(None, None), None);
    }

    /// A realm type with no row keeps what the last world entry stored.
    #[test]
    fn a_realm_type_with_no_row_keeps_the_last_latch() {
        assert!(!latched_realm_pvp(false, None));
        assert!(latched_realm_pvp(true, None));
        assert!(!latched_realm_pvp(true, Some(false)));
        assert!(latched_realm_pvp(false, Some(true)));
    }

    /// On the install's AreaTable the gate opens in exactly the six capitals for a PvE realm.
    #[test]
    fn the_six_capitals_are_the_zones_a_pve_realm_colours() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let cat = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let open: Vec<u32> = (0..=u32::from(u16::MAX))
            .filter(|&id| {
                cat.get(id)
                    .is_some_and(|r| r.zone_id == 0 && zone_pvp(Some(r), None).unwrap().capital)
            })
            .collect();
        assert_eq!(open, [1497, 1519, 1537, 1637, 1638, 1657]);
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

    /// `GetZonePVPInfo` (`0x48d540`) reads, at the call, the zone the last commit stored
    /// (`0xb4e314`) and the active player's faction template (`0x48d5b4`-`0x48d5fe`): a capital
    /// entered before our template arrives answers once it is there, with no zone change between,
    /// and the type follows a template that changes.
    #[test]
    fn the_zone_pvp_type_reads_our_template_at_the_call() {
        use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
        use crate::target::relations::fixture::{factions, HUMAN, ORC};
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        use benilla_ui::script::UiScript;
        const ME: u64 = 0x7;
        const ORGRIMMAR: u32 = 1637;
        /// `UNIT_FIELD_FACTIONTEMPLATE`.
        const TEMPLATE: u16 = 35;

        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        benilla_world::world_point::init_world_point_resources(world);
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world.insert_resource(factions());
        world.insert_resource(AreaTableRes(AreaTableCatalog::from_rows(vec![(
            ORGRIMMAR,
            AreaTableRow {
                name: "Orgrimmar".into(),
                ..row(0x138, 4)
            },
        )])));
        world.insert_resource(benilla_world::terrain_stream::CurrentArea(Some(ORGRIMMAR)));
        world.insert_non_send_resource(UiScript::new().unwrap());
        let store = |template: Option<u32>| {
            let pairs: Vec<(u16, u32)> = template.map(|t| (TEMPLATE, t)).into_iter().collect();
            ObjectStore(ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player))
        };
        let me = world.spawn((SelfPlayer, Guid(ME), store(None))).id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        let pvp = |app: &mut App| {
            let world = app.world_mut();
            crate::ui_unit::run_feed_units_once(world);
            world.run_system_cached(feed_zone_events).unwrap();
            world
                .non_send_resource::<UiScript>()
                .eval::<(Option<String>, Option<String>)>(
                    "local t, f = GetZonePVPInfo() return t, f",
                )
                .unwrap()
        };
        assert_eq!(pvp(&mut app), (None, None), "no template: nil");
        app.world_mut().entity_mut(me).insert(store(Some(ORC)));
        assert_eq!(pvp(&mut app).0.as_deref(), Some("friendly"));
        app.world_mut().entity_mut(me).insert(store(Some(HUMAN)));
        assert_eq!(pvp(&mut app).0.as_deref(), Some("hostile"));
    }

    /// A worldport frees our player with every object (`0x467700`) and its create comes after the
    /// load. The zone poll reports only the active mover's, else the active player's, position
    /// (`0x5db937`, `0x5db956`), and runs on the tick's category 5 after the category 6 packet
    /// drain (`0x401ec0`, `0x403620`), so the commit `0x494780` waits for our player and follows
    /// its arrival. A capital entered on a PvE realm answers its type inside the new area's event.
    #[test]
    fn a_worldport_commits_the_zone_once_our_player_is_back() {
        use crate::net::{Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
        use crate::target::relations::fixture::{factions, ORC};
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        use benilla_ui::script::UiScript;
        const ME: u64 = 0x7;
        const ORGRIMMAR: u32 = 1637;
        /// `UNIT_FIELD_FACTIONTEMPLATE`.
        const TEMPLATE: u16 = 35;

        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        benilla_world::world_point::init_world_point_resources(world);
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world.insert_resource(factions());
        world.insert_resource(AreaTableRes(AreaTableCatalog::from_rows(vec![(
            ORGRIMMAR,
            AreaTableRow {
                name: "Orgrimmar".into(),
                ..row(0x138, 4)
            },
        )])));
        // The body is at the port's destination before our player's create arrives.
        world.insert_resource(benilla_world::terrain_stream::CurrentArea(Some(ORGRIMMAR)));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                Seen = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("ZONE_CHANGED_NEW_AREA")
                f:SetScript("OnEvent", function()
                    table.insert(Seen, tostring((GetZonePVPInfo())))
                end)
                "#,
            )
            .unwrap();
        world.insert_non_send_resource(script);
        let frame = |app: &mut App| {
            let world = app.world_mut();
            crate::ui_unit::run_feed_units_once(world);
            world.run_system_cached(feed_zone_events).unwrap();
            let s = world.non_send_resource::<UiScript>();
            let seen = s.eval::<Vec<String>>("return Seen").unwrap();
            s.run("Seen = {}").unwrap();
            seen
        };
        assert_eq!(
            frame(&mut app),
            Vec::<String>::new(),
            "no player, no commit"
        );
        let store = ObjectStore(
            ObjectFields::from_pairs(&[(TEMPLATE, ORC)]).into_created(ObjectType::Player),
        );
        let me = app.world_mut().spawn((SelfPlayer, Guid(ME), store)).id();
        app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
        assert_eq!(frame(&mut app), ["friendly"]);
    }

    /// The commit reads what the unit feed pushed of a player that arrived this frame.
    #[test]
    fn the_zone_commit_runs_after_the_unit_feed() {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        assert!(crate::ui_unit::feed_units_runs_before(
            &mut app,
            feed_zone_events
        ));
    }
}
