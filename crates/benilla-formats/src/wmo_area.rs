//! `WMOAreaTable.dbc`, a WMO interior's audio identity per building and per group; the Northshire
//! Abbey chant is its whole-WMO row's `ZoneIntroMusicTable` 221. Rows key on the root's
//! `MOHD.wmoID`, the placement's `MODF.nameSet` and each group's `MOGP.uniqueID` (`+0x38`), with
//! `WMOGroupID` `-1` for the whole-WMO row; the enUS `AreaName` is column 11 of 20.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One `WMOAreaTable` row's audio fields, `0` for none or inherit.
#[derive(Clone)]
pub struct WmoArea {
    /// The row id, the client's indoor dedup key (`[0x86860c]`): while it stays the same, the
    /// zone-text update is skipped.
    pub id: u32,
    /// `SoundProviderPreferences` ids, `[dry, underwater]`.
    pub sound_provider: [u32; 2],
    /// `SoundAmbience.dbc` id.
    pub ambience: u32,
    /// `ZoneMusic.dbc` id.
    pub zone_music: u32,
    /// `ZoneIntroMusicTable.dbc` id, the entry fanfare.
    pub intro_sound: u32,
    /// The `AreaTable.dbc` area this group counts as (0 = none), read by the area resolver
    /// `0x670250` when the down-ray keeps the WMO over the terrain: Ironforge reports its own area.
    pub area_table_id: u32,
    /// The interior's enUS display name, empty for an unnamed row.
    pub name: String,
}

/// The table keyed for the runtime lookup: exact group rows and whole-WMO defaults.
pub struct WmoAreaCatalog {
    /// `(WMOID, NameSetID, WMOGroupID)` to its group row.
    groups: HashMap<(u32, u32, u32), WmoArea>,
    /// `(WMOID, NameSetID)` to the `WMOGroupID == −1` whole-WMO row.
    defaults: HashMap<(u32, u32), WmoArea>,
}

impl WmoAreaCatalog {
    /// The hit group's own row by exact key, the zone-text chain's second query (`0x69d8f0`, one
    /// bsearch at `0x7ccd30` with no name-set retry, default fallback or overlay): its name, if
    /// any, refills the subzone slot.
    pub fn group_row(&self, wmo_id: u32, name_set: u32, group_id: u32) -> Option<&WmoArea> {
        self.groups.get(&(wmo_id, name_set, group_id))
    }

    /// The whole-WMO row by exact key, the zone-text chain's first query (`0x69d830` to
    /// `0x7ccde0`, `push 0xffffffff`): a name unlike the subzone overrides the zone slot, an
    /// unnamed row falls back to an `AreaTable` name, and a missing row never overrides.
    pub fn default_row(&self, wmo_id: u32, name_set: u32) -> Option<&WmoArea> {
        self.defaults.get(&(wmo_id, name_set))
    }

    /// A WMO interior's audio, the group row's non-zero fields over the whole-WMO row's: the
    /// abbey's chant, music and ambience sit on its whole-WMO row. A name set without rows falls
    /// back to set 0; the reference's handling of that miss is untraced.
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

    #[test]
    fn real_wmo_area_resolves_abbey_and_chapel() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_wmo_area_catalog(&mut chain).expect("load WMOAreaTable");
        // 21115 rows but 21105 distinct (wmo, nameset, group) keys: the shipped data repeats 10.
        assert_eq!(cat.len(), 21105, "all distinct rows load");

        // NSabbey (59): an unknown group falls back to the whole-WMO row, the monk chant.
        let abbey = cat.resolve(59, 0, 9999).expect("abbey default row");
        assert_eq!(abbey.intro_sound, 221, "Sacred01 intro");

        // The reused abbey model's "Main Hall" (group 1934) sits at Alexston Farmstead (area 219).
        assert_eq!(
            cat.resolve(59, 0, 1934).unwrap().area_table_id,
            219,
            "the group's AreaTableID resolves (field 10, not the zeroed field 9)"
        );

        // The overlay: the near-empty "Main Hall" row still hears the whole-WMO identity.
        let hall = cat.resolve(59, 0, 1934).expect("Main Hall");
        assert_eq!(hall.intro_sound, 221, "the chant reaches the hall");
        assert_eq!(hall.zone_music, 204);
        assert_eq!(hall.ambience, 26);
        assert_eq!(hall.name, "Main Hall", "the group's own name wins");

        // Chapel (656) group 478: cave reverb and ambience over the whole-WMO underwater preset.
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
