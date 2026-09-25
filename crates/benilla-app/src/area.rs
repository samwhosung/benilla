//! The player's area identity, published to the UI — the real 1.12 `ZONE_CHANGED` event family
//! and the zone-text host globals behind `GetZoneText`/`GetSubZoneText`/`GetRealZoneText`/
//! `GetMinimapZoneText`/`GetZonePVPInfo` (corrected to the bytes by its fold-back
//! record).
//!
//! The model (`0x494780` is the one updater all three zone events fire from, on the per-update
//! area resolve):
//!
//! - The client caches `(zoneId, zoneText, subzoneText)` and compares per update: **zone id
//!   changed → `ZONE_CHANGED_NEW_AREA` fires alone**; else **either text changed →
//!   `ZONE_CHANGED_INDOORS`** when the resolver's indoor bit is set, else **`ZONE_CHANGED`**.
//!   Mutually exclusive; the first world-enter is a zone-id change from the zeroed cache.
//! - **Indoors, the zone-text slot takes the WMO interior's name and the subzone slot nulls**
//!   (`0x67e670`) — entering a named inn splashes the inn's name. `GetRealZoneText` reads the
//!   pre-override (WMO-immune) zone name; `GetMinimapZoneText` reads subzone-else-zone
//!   (`0xb4da28`, its own change event `MINIMAP_ZONE_CHANGED` from `0x494970`).
//! - The "zone" is the leaf's **single-hop parent** (AreaTable field 2; itself when 0) — equal
//!   to the defensive `top_zone` walk on 5875 data (chains are 1–2 deep), which the world map
//!   keeps using.
//! - `GetZonePVPInfo` (`0x48d540`): `isArena` = the **leaf** row's Flags bit `0x80` (the FFA-pit
//!   flag); ownership = the **zone** row's FactionGroupMask vs the player template's
//!   friend-then-enemy group masks → "friendly"/"hostile", else **"contested" — never nil for an
//!   ownerless zone** (nil is structural failure only); `factionName` = FactionGroup.dbc's
//!   localized Name for the zone's mask bit; pvpType is never "arena"; realm type never enters.
//! - **The indoor bit** is the player's faces-only down-ray verdict
//!   ([`benilla_world::wmo_portal::CurrentAreaInterior`] — the CGLight node's `+0x90` bit 0, set
//!   by `0x6a87f0`): indoors ⇔ the nearest surface below is a WMO face whose group lacks MOGP
//!   `0x8` EXTERIOR. The abbey yard is terrain-below ⇒ outdoors; the flip is the doorway.
//! - **The indoor naming** (`0x67e670`): while indoors and the hit
//!   GROUP is unchanged, the WHOLE updater is skipped (the dedup `[0x868608]`/`[0x86860c]`). On
//!   an indoor change: **query A** (the whole-WMO −1 row, exact key, `0x69d830`) overrides the
//!   ZONE slot — only when its resolved name is non-empty AND differs from the current subzone
//!   (the inn splash; skipped at the abbey, whose default name equals its yard subzone) —
//!   nulling the subzone; an existing-but-unnamed −1 row resolves through its `AreaTableID`'s
//!   AreaTable name (Ironforge's unnamed city WMO), a missing row never overrides. Then
//!   **query B** (the hit group's own row, exact key, `0x69d8f0`) re-populates the subzone
//!   ("Main Hall"). No name-set retry, no cross-row fallback anywhere.
//!
//! **`0x494780` has one more effect than the election, and it does NOT live here** (2192): when
//! the cached zone id was 0 — the first world-enter of a session — it also calls `0x4a6650`, the
//! player→(continent, zone) resolver, and hands the answer to the world map's `SetMap` setter, so
//! a freshly-logged-in client is already on the player's own zone map before any Lua asks. That
//! half is modelled where the map data lives, in
//! [`crate::ui_world_map::world_enter_selection`] — the gate there is the same precondition
//! (`0x67e510` never reaches this updater with a zero zone id), read off our own area feed rather
//! than off the cache below.
//!
//! The area *authority* stays `terrain_stream::CurrentArea` (the MCNK `areaId`
//! with the WMO-interior override, now off the same faces-only claim). Host globals are written
//! before the event fires, so a handler's `GetZoneText()` already sees the new state.
//!
//! Named divergence: the dedup keys `(WMOID, MOGP uniqueID)` where the client stores the raw
//! per-WMO group INDEX — ours changes exactly when the hit group does but never aliases across
//! adjacent buildings the way the raw index can. INTERIM residue: the `[0x88272c]` PvP display
//! gate (an unnamed cached boolean, verified NOT realm type) ships open.
//!
//! [`AreaTableRes`] is also the one shared `AreaTable.dbc` projection (0287's consolidation):
//! the world map and this module consume it. (`area_sound.rs` keeps its own audio-FK fold —
//! different columns, different concern.)

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use benilla_formats::AreaTableCatalog;
use benilla_ui::script::UiScript;

use crate::net::{ObjectStore, SelfPlayer};
use crate::target::Factions;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// The shared `AreaTable.dbc` catalog: id → (name, parent zone, flags, faction mask). Loaded once
/// at Startup; absent if the DBC failed to read (consumers take `Option` and go quiet).
#[derive(Resource)]
pub(crate) struct AreaTableRes(pub(crate) AreaTableCatalog);

/// Startup: load the shared area catalog off the patch chain.
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

/// What the client's zone-text cache holds — the compare targets of `0x494780` (zone id + the
/// two display strings), the minimap line's own cache (`0x494970`), and the indoor dedup pair
/// (`[0x868608]` prev-indoor / `[0x86860c]` prev WMO-area row id).
///
/// Held as a [`crate::ui_script::VmMemo`], which is not a deviation from that BSS model but the
/// completion of it: the cache's whole purpose is deduping what was last *pushed into the VM*, and
/// the VM is built at world entry and destroyed at the character screen. A fresh VM
/// is the client's own fresh BSS — the zeroed cache below is exactly the state the real client is in
/// at that moment, so the first resolve of every login is a NEW_AREA with the globals written, which
/// is what the zone splash and the minimap line need.
#[derive(Default)]
struct ZoneCache {
    /// `None` = never resolved (the zeroed BSS cache: the first resolve is a NEW_AREA).
    zone_id: Option<u32>,
    zone_text: String,
    subzone_text: String,
    minimap_text: String,
    /// The previous update's indoor bit (`[0x868608]`).
    indoor: bool,
    /// The previous hit group's identity (`[0x86860c]` — the client stores the group index; we
    /// scope the MOGP uniqueID by WMO): same group while indoors ⇒ the whole updater is skipped.
    wmo_id: u32,
    wmo_group: u32,
}

/// The texts + indoor bit one resolve produces (the updater's compare inputs).
struct ZoneSignal {
    zone_id: u32,
    zone_text: String,
    subzone_text: String,
    indoor: bool,
}

/// The event election, verbatim from `0x494780`: zone id changed → NEW_AREA **alone**; else any
/// text changed → INDOORS/CHANGED by the indoor bit; else nothing.
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

/// The zone-splash data plane (module doc). Per resolve: compute the display texts (with the
/// indoor override), the PvP tuple, write the host globals, then fire the elected zone event
/// and — independently, like the client's second site — `MINIMAP_ZONE_CHANGED` when the
/// subzone-else-zone line changed.
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
        // An id the catalog doesn't know stays unpublished (keep the last real state, like the
        // resolver itself does for areaId 0).
        return;
    };
    // The zone: the leaf's single-hop parent (field 2), itself when 0 (`0x494780`'s stored row;
    // == `top_zone` on 5875's 1–2-deep chains).
    let zone = if leaf_row.zone_id == 0 {
        leaf
    } else {
        leaf_row.zone_id
    };
    let real_zone_text = areas.0.name(zone).unwrap_or_default().to_string();
    let indoor = world.area_interior().is_some();

    // The outdoor texts — the locals `0x67e670` starts from.
    let mut zone_text = real_zone_text.clone();
    let mut subzone_text = if leaf == zone {
        String::new()
    } else {
        leaf_row.name.clone()
    };

    // The indoor naming (`0x67e670`, module doc): dedup-skip while the hit group
    // holds; else the default-row name (query A) may override the zone slot and the group-row
    // name (query B) re-populates the subzone — both EXACT-key lookups (no name-set retry).
    let mut wmo_group = 0u32;
    if let Some((k, group, default)) = world.area_interior_rows() {
        // The dedup key: the hit GROUP's identity (the client's `[groupRec+0x7c]` group index,
        // nonzero-gated). We key the MOGP uniqueID scoped by WMO id: it changes exactly
        // when the hit group does, and — a named divergence — never aliases across two adjacent
        // buildings the way the client's raw per-WMO index can.
        wmo_group = k.group_area_id;
        if cache.indoor
            && k.group_area_id != 0
            && (cache.wmo_id, cache.wmo_group) == (k.wmo_id, wmo_group)
        {
            return; // the dedup: same WMO group ⇒ the whole updater is skipped
        }
        cache.wmo_id = k.wmo_id;
        // Query A (the whole-WMO −1 row): override the zone name + null the subzone — only when
        // the resolved name is non-empty and differs from the current subzone (the abbey skip).
        // An EXISTING-but-unnamed row falls back to an AreaTable name — the row's own
        // `AreaTableID`, else the current leaf (how Ironforge's unnamed city WMO splashes
        // "Ironforge"; the client's area-0 arm reads the raw terrain areaId at the node — the
        // current leaf equals it in that arm, since an area-0 row also never overrode the leaf).
        // A MISSING row never overrides (the client's `""` sentinel bypass).
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
        // Query B (the hit group's own row): re-populate the subzone (empty/missing = leave it).
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
    // GetMinimapZoneText's own line: subzone-else-zone (`0xb4da28`).
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

    // GetZonePVPInfo (`0x48d540`): leaf row's 0x80 for isArena; zone row's mask vs the player
    // template's friend-then-enemy masks; factionName from FactionGroup.dbc. Never "arena" as a
    // type; "contested" (not nil) for an ownerless zone; nil only on structural failure. The
    // `[0x88272c]` display gate ships open (module doc).
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

/// The shared area catalog + the zone-event feed (decision 0287 + fold-back).
pub(crate) struct AreaPlugin;

impl Plugin for AreaPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_area_table.after(AssetSet::Open))
            .add_systems(
                Update,
                // After the script tick, like the other feeds: the event dispatches on the next
                // tick — a frame of latency the splash can't perceive. MINIMAP_ZONE_CHANGED's
                // handler reads the PvP globals written above in the same call. ALSO after the
                // leaf authority (which orders after the interior claim): leaf, indoor bit, and
                // names must come from ONE coherent frame — the client's single-pass resolve
                // (`0x67e510`); a fresh claim against a stale leaf fired the abbey login's
                // spurious big splash.
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

    /// The `0x494780` election domain: first-enter and zone hops take NEW_AREA alone; same-zone
    /// text changes split INDOORS/CHANGED on the indoor bit; no change fires nothing.
    #[test]
    fn election_matches_the_byte_law() {
        // First world-enter: the zeroed cache is a zone-id change.
        let c = ZoneCache::default();
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Northshire Valley", false)),
            Some("ZONE_CHANGED_NEW_AREA")
        );

        // Zone hop: NEW_AREA alone — even though the texts changed too.
        let c = cache(Some(12), "Elwynn Forest", "");
        assert_eq!(
            elect_event(&c, &sig(40, "Westfall", "The Jansen Stead", false)),
            Some("ZONE_CHANGED_NEW_AREA")
        );

        // Outdoor subzone hop: same zone, subzone text changed → ZONE_CHANGED.
        let c = cache(Some(12), "Elwynn Forest", "");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            Some("ZONE_CHANGED")
        );

        // Inn entry: same zone, the ZONE text changed by the indoor override, indoor set →
        // INDOORS (the named-interior splash).
        let c = cache(Some(12), "Elwynn Forest", "Goldshire");
        assert_eq!(
            elect_event(&c, &sig(12, "Lion's Pride Inn", "", true)),
            Some("ZONE_CHANGED_INDOORS")
        );

        // Inn exit: texts revert, outdoors again → plain ZONE_CHANGED (the frames re-cache
        // silently and only the subzone line splashes — the consumer law).
        let c = cache(Some(12), "Lion's Pride Inn", "");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            Some("ZONE_CHANGED")
        );

        // No change → nothing.
        let c = cache(Some(12), "Elwynn Forest", "Goldshire");
        assert_eq!(
            elect_event(&c, &sig(12, "Elwynn Forest", "Goldshire", false)),
            None
        );
    }

    /// The indoor naming law (`0x67e670`) against the REAL abbey/inn data shapes — the
    /// override-skip (whole-WMO name == yard subzone) and the override-fire (inn) branches,
    /// run through the same WmoAreaCatalog the live feed reads. Skips without client data.
    #[test]
    fn indoor_naming_matches_the_byte_law_on_real_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("chain");
        let cat = benilla_formats::load_wmo_area_catalog(&mut chain).expect("WMOAreaTable");

        // The abbey (WMO 59, Northshire placement name-set 1): the default row's name equals the
        // yard subzone ⇒ the zone override SKIPS; group 1934's own name re-fills the subzone.
        let default = cat.default_row(59, 1).expect("abbey default row");
        assert_eq!(default.name, "Northshire Abbey");
        let group = cat.group_row(59, 1, 1934).expect("abbey main hall row");
        assert_eq!(group.name, "Main Hall");
        assert_eq!(
            group.area_table_id, 24,
            "the leaf stays Northshire Abbey (24)"
        );
        // The dedup rides the hit-group identity (the client's group index; our MOGP
        // uniqueID) — distinct per room, so a room hop re-fires and standing still skips.
        let library = cat.group_row(59, 1, 1943).expect("library row");
        assert_ne!(group.id, library.id, "distinct rows exist per room");

        // An unnamed room (group 1935): query B empty ⇒ the subzone keeps what the override
        // left — with the override skipped, that's the terrain leaf name.
        let unnamed = cat.group_row(59, 1, 1935).expect("unnamed room row");
        assert!(unnamed.name.is_empty());

        // The Goldshire inn (WMO 53, name-set 2 — its ONLY row is the whole-WMO one): the
        // default name differs from the street subzone ("Goldshire") ⇒ the override FIRES (the
        // big inn splash), and with no named group row the subzone stays nulled — the un-doubled
        // inn entry. Its AreaTableID is 0, so the leaf stays the terrain chunk's (Goldshire).
        let inn = cat.default_row(53, 2).expect("the inn's whole-WMO row");
        assert_eq!(inn.name, "Lion's Pride Inn");
        assert_eq!(inn.area_table_id, 0);
        assert!(
            cat.group_row(53, 2, 0).is_none_or(|g| g.name.is_empty()),
            "no named group rows — query B leaves the nulled subzone alone"
        );
    }
}
