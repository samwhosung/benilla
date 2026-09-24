//! `WMOAreaTable.dbc` — the per-building (and per-group) audio identity: what a WMO interior
//! sounds like. This is where 1.12's interior soundscape actually lives: the
//! Northshire Abbey monk chant is this table's whole-WMO row → `ZoneIntroMusicTable` 221, and
//! ~4 000 group rows carry interior reverb/ambience (the terrain `AreaTable` path covers only
//! open ground).
//!
//! Layout — VERIFIED against build 5875 (header + row/string decodes, 2026-07-03):
//! **21115 × 20 × 80 B**: `ID(0), WMOID(1), NameSetID(2), WMOGroupID(3, −1 = whole-WMO default),
//! SoundProviderPref(4), SoundProviderPrefUnderwater(5), AmbienceID(6), ZoneMusic(7),
//! IntroSound(8), Flags(9), … AreaName enUS(11), … locale-mask(19)` (col 10 is another locale
//! slot, empty in this data; enUS at 11 pinned by clean NUL-preceded string starts —
//! "Lion's Pride Inn", "Cathedral of Light").
//!
//! Join keys, byte-verified against the real files: `WMOID` = the root's `MOHD.wmoID`
//! (NSabbey.wmo → 59), `WMOGroupID` = each group file's `MOGP.uniqueID` @ 0x38 (Chapel_000 →
//! 478), `NameSetID` = the ADT placement's `MODF.nameSet` (one abbey model serves Northshire /
//! Tyr's Hand by name set).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One WMO area row — the audio fields (all 0 = none/inherit).
#[derive(Clone)]
pub struct WmoArea {
    /// The DBC row ID (col 0) — the client's indoor **dedup key** (`[0x86860c]`): while the
    /// player stays in a WMO area with the same row id, the zone-text updater is skipped
    /// entirely.
    pub id: u32,
    /// `SoundProviderPreferences` FKs, `[dry, underwater]`.
    pub sound_provider: [u32; 2],
    /// `SoundAmbience.dbc` FK.
    pub ambience: u32,
    /// `ZoneMusic.dbc` FK.
    pub zone_music: u32,
    /// `ZoneIntroMusicTable.dbc` FK — the entry fanfare (the abbey's monk chant).
    pub intro_sound: u32,
    /// `AreaTable.dbc` FK — the WORLD area this WMO group counts as (0 = none). The client's
    /// GetAreaID resolver (`0x670250`) reads this when the down-ray keeps the WMO over the
    /// terrain chunk, so an indoor city (Ironforge/Undercity) reports its own area, not the
    /// enclosing zone's. Feeds the minimap/world-map zone and interior sound.
    pub area_table_id: u32,
    /// The interior's display name (enUS; empty for unnamed rows).
    pub name: String,
}

/// The table keyed for the runtime lookup: exact group rows + whole-WMO defaults.
pub struct WmoAreaCatalog {
    /// `(WMOID, NameSetID, WMOGroupID)` → row (group-specific).
    groups: HashMap<(u32, u32, u32), WmoArea>,
    /// `(WMOID, NameSetID)` → the `WMOGroupID == −1` whole-WMO row.
    defaults: HashMap<(u32, u32), WmoArea>,
}

impl WmoAreaCatalog {
    /// The hit group's OWN row, **exact key** — the zone-text chain's **query B** (`0x69d8f0` →
    /// bsearch `0x7ccd30`: a single exact
    /// `(WMOID, NameSetID, WMOGroupID)` search, **no NameSetID retry, no default fallback, no
    /// overlay**): its name re-populates the subzone slot (an unnamed/missing row leaves the
    /// slot as the override set it).
    pub fn group_row(&self, wmo_id: u32, name_set: u32, group_id: u32) -> Option<&WmoArea> {
        self.groups.get(&(wmo_id, name_set, group_id))
    }

    /// The whole-WMO `WMOGroupID == −1` row, **exact key** (same single-search law) — the
    /// zone-text chain's **query A** (`0x69d830` → `0x7ccde0`, `push 0xffffffff`): its name
    /// overrides the ZONE slot only when non-empty and different from the current subzone (the
    /// inn splash; skipped at the abbey, whose default name equals its yard subzone). An
    /// EXISTING-but-unnamed row falls back to an AreaTable name (the caller's job — it needs the
    /// area catalog); a MISSING row never overrides.
    pub fn default_row(&self, wmo_id: u32, name_set: u32) -> Option<&WmoArea> {
        self.defaults.get(&(wmo_id, name_set))
    }

    /// Resolve a WMO interior's audio identity as a **field-level overlay**: the group row's
    /// nonzero fields over the whole-WMO (−1) default's. Data demands this — the abbey's chant
    /// (`IntroSound` 221), music (204) and ambience (26) all live on its −1 row while the group
    /// rows the player actually stands in ("Main Hall" 1934…) are near-empty; a row-level pick
    /// returned the zeros and silenced the abbey (director-caught, 2026-07-03). Name-set
    /// fallback to 0 when the placement's set has no rows (INTERIM — the client's name-set miss
    /// behavior is unrecorded; set-0 rows exist for every audio-carrying WMO).
    pub fn resolve(&self, wmo_id: u32, name_set: u32, group_id: u32) -> Option<WmoArea> {
        let (group, default) = [name_set, 0]
            .iter()
            .map(|&ns| {
                (
                    self.groups.get(&(wmo_id, ns, group_id)),
                    self.defaults.get(&(wmo_id, ns)),
                )
            })
            .find(|(g, d)| g.is_some() || d.is_some())?;
        let base = default.cloned().unwrap_or_else(|| WmoArea {
            id: 0,
            sound_provider: [0, 0],
            ambience: 0,
            zone_music: 0,
            intro_sound: 0,
            area_table_id: 0,
            name: String::new(),
        });
        let Some(g) = group else { return Some(base) };
        let nz = |v: u32, b: u32| if v != 0 { v } else { b };
        Some(WmoArea {
            id: g.id,
            sound_provider: [
                nz(g.sound_provider[0], base.sound_provider[0]),
                nz(g.sound_provider[1], base.sound_provider[1]),
            ],
            ambience: nz(g.ambience, base.ambience),
            zone_music: nz(g.zone_music, base.zone_music),
            intro_sound: nz(g.intro_sound, base.intro_sound),
            area_table_id: nz(g.area_table_id, base.area_table_id),
            name: if g.name.is_empty() {
                base.name
            } else {
                g.name.clone()
            },
        })
    }

    pub fn len(&self) -> usize {
        self.groups.len() + self.defaults.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("WMOAreaTable");
    for name in [
        "ID",
        "WMOID",
        "NameSetID",
        "WMOGroupID",
        "SoundProviderPref",
        "SoundProviderPrefUnderwater",
        "AmbienceID",
        "ZoneMusic",
        "IntroSound",
        "Flags",
        "AreaTableID",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.add_field(SchemaField::new("AreaName", FieldType::String));
    for i in 12..20 {
        s.add_field(SchemaField::new(format!("_pad{i}"), FieldType::UInt32));
    }
    s
}

/// Read `WMOAreaTable.dbc` off the patch chain.
pub fn load_wmo_area_catalog(chain: &mut Chain) -> Result<WmoAreaCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\WMOAreaTable.dbc")
        .context("reading WMOAreaTable.dbc")?;
    let rs = parse(&bytes, schema(), "WMOAreaTable")?;
    let mut groups = HashMap::new();
    let mut defaults = HashMap::new();
    for r in rs.records() {
        let wmo_id = u32_at(r, 1).unwrap_or(0);
        let name_set = u32_at(r, 2).unwrap_or(0);
        let group = u32_at(r, 3).unwrap_or(0);
        let area = WmoArea {
            id: u32_at(r, 0).unwrap_or(0),
            sound_provider: [u32_at(r, 4).unwrap_or(0), u32_at(r, 5).unwrap_or(0)],
            ambience: u32_at(r, 6).unwrap_or(0),
            zone_music: u32_at(r, 7).unwrap_or(0),
            intro_sound: u32_at(r, 8).unwrap_or(0),
            area_table_id: u32_at(r, 10).unwrap_or(0),
            name: str_at(&rs, r, 11).unwrap_or_default(),
        };
        if group == u32::MAX {
            defaults.insert((wmo_id, name_set), area);
        } else {
            groups.insert((wmo_id, name_set, group), area);
        }
    }
    Ok(WmoAreaCatalog { groups, defaults })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 table resolves the two byte-verified anchors: the abbey's whole-WMO monk
    /// chant (WMO 59 → intro 221 "Sacred01") and the chapel's interior group row (WMO 656 group
    /// 478 → CAVE reverb 75 + ambience 50). Skips without client data.
    #[test]
    fn real_wmo_area_resolves_abbey_and_chapel() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_wmo_area_catalog(&mut chain).expect("load WMOAreaTable");
        // 21115 rows, 21105 distinct (wmo, nameset, group) keys — 10 exact-duplicate keys are
        // editor debris in the 5875 data (e.g. WMO 556 nameset 1 rows appear twice).
        assert_eq!(cat.len(), 21105, "all distinct rows load");

        // NSabbey (59): any interior group falls back to the whole-WMO row = the monk chant.
        let abbey = cat.resolve(59, 0, 9999).expect("abbey default row");
        assert_eq!(abbey.intro_sound, 221, "Sacred01 intro");

        // AreaTableID (field 10) — the WORLD area a WMO group counts as. The abbey model is reused;
        // its nameset-0 "Main Hall" (group 1934) sits at Alexston Farmstead (area 219), proving the
        // column parses real AreaTable ids (not the all-zero Flags at field 9). This is what lets an
        // indoor city report its own area — a raw scan finds 186 group rows → Ironforge (1537) and
        // 219 → Undercity (1497).
        assert_eq!(
            cat.resolve(59, 0, 1934).unwrap().area_table_id,
            219,
            "the group's AreaTableID resolves (field 10, not the zeroed field 9)"
        );

        // The overlay (the director-caught silence): standing in the abbey's "Main Hall"
        // (group 1934, a near-empty row) must still hear the whole-WMO identity.
        let hall = cat.resolve(59, 0, 1934).expect("Main Hall");
        assert_eq!(hall.intro_sound, 221, "the chant reaches the hall");
        assert_eq!(hall.zone_music, 204);
        assert_eq!(hall.ambience, 26);
        assert_eq!(hall.name, "Main Hall", "the group's own name wins");

        // Chapel (656) interior group 478: CAVE reverb + interior ambience overlaying the
        // whole-WMO row's underwater pref; no music anywhere on this WMO.
        let chapel = cat.resolve(656, 0, 478).expect("chapel group row");
        assert_eq!(chapel.sound_provider, [75, 11]);
        assert_eq!(chapel.ambience, 50);
        assert_eq!(chapel.zone_music, 0);

        // A named row reads through the enUS column.
        let inn = cat
            .resolve(53, 2, 9999)
            .or_else(|| cat.resolve(53, 0, 9999));
        assert!(inn.is_some(), "the Goldshire inn WMO has rows");
    }
}
