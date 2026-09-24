use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};
use crate::Chain;

const CHAR_HAIR_GEOSETS: &str = "DBFilesClient\\CharHairGeosets.dbc";
const CHAR_FACIAL_HAIR: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const HELMET_GEOSET_VIS: &str = "DBFilesClient\\HelmetGeosetVisData.dbc";

/// The 16 default region bases of `cc+0x144` (ctors `0x476810`/`0x476960`); group 7's is 702.
/// Entries 0–3, hair and facial hair, are replaced by the customization; 4–15 are the equipment
/// groups' bare defaults.
const REGION_BASES: [u16; 16] = [
    1, 101, 201, 301, 401, 501, 601, 702, 801, 901, 1001, 1101, 1201, 1301, 1401, 1501,
];

/// The customization → geoset tables, resolving an appearance to the set of visible geosets.
pub struct CharacterGeosets {
    /// (race, sex, hairStyle) → hair `GeosetID` (group 0).
    hair: HashMap<(u8, u8, u8), u32>,
    /// (race, sex, facialHair) → three geoset variations in file order, for groups 1, 3, 2
    /// (`0x478660`).
    facial: HashMap<(u8, u8, u8), [u32; 3]>,
    /// HelmetGeosetVisData row id → five race bitmasks (`0x4799a0`): bit `1 << race` forces a hair,
    /// facial-hair or ear slot back to its group base under a helm.
    helmet_vis: HashMap<u32, [u32; 5]>,
}

/// What the equipment branches of `0x477520` read: each bodyslot 2–9's three `geosetGroup`s
/// (shirt, chest, belt, pants, boots, wrist, gloves, tabard), the cloak's first, and the helm's
/// `HelmetGeosetVisData` rows (`[male, female]`, ItemDisplayInfo cols 12/13). Default is naked.
#[derive(Default, Clone, Copy)]
pub struct EquipGeosets {
    pub bodyslots: [Option<[u32; 3]>; 8],
    pub cloak: Option<u32>,
    pub helm_vis: Option<[u32; 2]>,
    /// Branch B3's gate: something other than the shirt dresses the forearm (`cc+0x26c..cc+0x280`,
    /// ArmLower cells 1–6); fill it from [`forearm_dressed`](crate::forearm_dressed).
    pub forearm_dressed: bool,
    /// Branch B6's gate `[cc+0xc]`: the tabard designer is open (setter `0x5e07fb`), so the body
    /// wears a previewable tabard over an empty tabard slot.
    pub tabard_preview: bool,
}

impl CharacterGeosets {
    /// The geosets (`skinSectionId`s) this appearance draws, sorted and deduplicated: the naked
    /// set, then the eight equipment branches B1–B8 of `0x477520`.
    pub fn visible_geosets(
        &self,
        race: u8,
        sex: u8,
        hair_style: u8,
        facial_hair: u8,
        equip: &EquipGeosets,
    ) -> Vec<u16> {
        // The naked set: the region bases and geoset 0, the body.
        let mut set = REGION_BASES.to_vec();
        set.push(0);
        // Hair: `0x478540` returns `max(1, geosetId)`, so a bald style shows geoset 1, the scalp.
        if let Some(&g) = self.hair.get(&(race, sex, hair_style)) {
            set[0] = g.max(1) as u16;
        }
        // Facial hair: `0x478660` adds 100/300/200 to the row's fields; no row keeps the bases.
        if let Some(&[a, b, c]) = self.facial.get(&(race, sex, facial_hair)) {
            set[1] = (a + 100) as u16;
            set[3] = (b + 300) as u16;
            set[2] = (c + 200) as u16;
        }
        // A helm's vis row (`0x4799a0`) forces slots back to their bases, the ears to 701, after
        // the customization; its slots {0,1,2,3,7} are disjoint from every equipment branch.
        if let Some(rows) = equip.helm_vis {
            let row = rows[usize::from(sex == 1)];
            if let Some(masks) = self.helmet_vis.get(&row) {
                const FORCED: [(usize, u16); 5] = [(0, 1), (1, 101), (2, 201), (3, 301), (7, 701)];
                let bit = 1u32 << (race & 0x1f);
                for (mask, (slot, forced)) in masks.iter().zip(FORCED) {
                    if mask & bit != 0 {
                        set[slot] = forced;
                    }
                }
            }
        }
        // The equipment branches; `g(slot, sub)` is a non-zero geosetGroup, slots 0 shirt, 1 chest,
        // 2 belt, 3 pants, 4 boots, 5 wrist, 6 gloves, 7 tabard.
        let g = |slot: usize, sub: usize| {
            equip.bodyslots[slot]
                .map(|groups| groups[sub])
                .filter(|v| *v != 0)
        };
        let disable = |set: &mut Vec<u16>, lo: u16, hi: u16| set.retain(|id| *id < lo || *id > hi);
        // The robe bit (chest or pants geosetGroup[2]) gates B4, B5 and B6; B7 reads the chest's.
        let robe = g(1, 2).or_else(|| g(3, 2));
        // B1/B2 (`0x477564`): gloves replace the glove group, else the chest's sleeves show.
        if let Some(v) = g(6, 0) {
            disable(&mut set, 401, 499);
            set.push(401 + v as u16);
        } else if let Some(v) = g(1, 0) {
            set.push(801 + v as u16);
        }
        // B3 (`0x4775bd`): the shirt's cuff, unless something else dresses the forearm.
        if !equip.forearm_dressed {
            if let Some(v) = g(0, 0) {
                set.push(801 + v as u16);
            }
        }
        // B4 (`0x4775fb`): a robe hides the leg groups and shows its skirt, else boots replace the
        // boot group, else the pants' kneepads. The bare 901 enables are the reference's own
        // (`0x477648`, `0x477685`), redundant with the bases.
        if let Some(v) = robe {
            disable(&mut set, 501, 599);
            disable(&mut set, 902, 999);
            disable(&mut set, 1100, 1199);
            disable(&mut set, 1300, 1399);
            set.push(1301 + v as u16);
        } else if let Some(v) = g(4, 0) {
            // `0x477639` disables the whole boot group first: the bare foot, 501, is a real submesh
            // on 11 of the 18 character models.
            disable(&mut set, 501, 599);
            set.push(901);
            set.push(501 + v as u16);
        } else if let Some(v) = g(3, 1) {
            set.push(901 + v as u16);
        } else {
            set.push(901);
        }
        // B5 (`0x477706`): the tabard flap, hidden by a robe.
        if robe.is_none() {
            if let Some(v) = g(7, 0) {
                set.push(1201 + v as u16);
            }
        }
        // B6 (`0x477752`): the designer preview wears 1201, the group's empty base, and without a
        // robe 1202, the flap.
        if equip.tabard_preview {
            set.push(1201); // redundant with the bases; the reference enables it too
            if robe.is_none() {
                set.push(1202);
            }
        }
        // B7 (`0x477799`): the shirt's doublet and the pants' legs (base 1102, not 1101); skipped
        // under a chest robe (`0x4777a4`, not a legs robe) or a tabard (`0x4777b8`).
        if g(1, 2).is_none() && g(7, 0).is_none() {
            if let Some(v) = g(0, 1) {
                set.push(1001 + v as u16);
            }
            if let Some(v) = g(3, 0) {
                set.push(1102 + v as u16);
            }
        }
        // B8 (`0x47780a`): a cloak replaces the cloak group.
        if let Some(v) = equip.cloak.filter(|v| *v != 0) {
            disable(&mut set, 1500, 1599);
            set.push(1501 + v as u16);
        }
        // The client sets a flag per submesh (`0x7110d0`), so a repeated enable is a no-op there.
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Load the customization DBCs from the patch chain. A repeated `(race, sex, variation)` key
    /// takes its first row, as the client's linear scans (`0x478540`, `0x478740`) do: the shipped
    /// CharHairGeosets lists goblin male variation 0 four times (geosets 1, 2, 1, 2).
    pub fn load(chain: &mut Chain) -> Result<Self> {
        let hair = {
            let bytes = chain
                .read_file(CHAR_HAIR_GEOSETS)
                .with_context(|| format!("reading {CHAR_HAIR_GEOSETS}"))?;
            let rs = parse(&bytes, char_hair_geosets_schema(), "CharHairGeosets")?;
            let mut m = HashMap::with_capacity(rs.records().len());
            for r in rs.records() {
                // fields: ID, RaceID, SexID, VariationID(hairStyle), GeosetID, ShowScalp.
                if let (Some(race), Some(sex), Some(var), Some(geoset)) =
                    (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3), u32_at(r, 4))
                {
                    // The first row wins.
                    m.entry((race as u8, sex as u8, var as u8))
                        .or_insert(geoset);
                }
            }
            m
        };
        let facial = {
            let bytes = chain
                .read_file(CHAR_FACIAL_HAIR)
                .with_context(|| format!("reading {CHAR_FACIAL_HAIR}"))?;
            let rs = parse(
                &bytes,
                char_facial_hair_schema(),
                "CharacterFacialHairStyles",
            )?;
            let mut m = HashMap::with_capacity(rs.records().len());
            for r in rs.records() {
                // fields: RaceID, SexID, VariationID(facialHair), 3×unused, Geoset100/300/200.
                if let (Some(race), Some(sex), Some(var)) =
                    (u32_at(r, 0), u32_at(r, 1), u32_at(r, 2))
                {
                    let g = [
                        u32_at(r, 6).unwrap_or(0),
                        u32_at(r, 7).unwrap_or(0),
                        u32_at(r, 8).unwrap_or(0),
                    ];
                    // The first row wins.
                    m.entry((race as u8, sex as u8, var as u8)).or_insert(g);
                }
            }
            m
        };
        // HelmetGeosetVisData, the helm hide-masks: without the file, helms hide nothing.
        let helmet_vis = match chain.read_file(HELMET_GEOSET_VIS) {
            Ok(bytes) => {
                let rs = parse(&bytes, helmet_vis_schema(), "HelmetGeosetVisData")?;
                let mut m = HashMap::with_capacity(rs.records().len());
                for r in rs.records() {
                    if let Some(id) = u32_at(r, 0) {
                        m.insert(id, std::array::from_fn(|i| u32_at(r, 1 + i).unwrap_or(0)));
                    }
                }
                m
            }
            Err(_) => HashMap::new(),
        };
        Ok(Self {
            hair,
            facial,
            helmet_vis,
        })
    }
}

/// CharHairGeosets.dbc: 6 fields.
pub(crate) fn char_hair_geosets_schema() -> Schema {
    let mut s = Schema::new("CharHairGeosets");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("RaceID", FieldType::UInt32),
        ("SexID", FieldType::UInt32),
        ("VariationID", FieldType::UInt32),
        ("GeosetID", FieldType::UInt32),
        ("ShowScalp", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// HelmetGeosetVisData.dbc: 6 fields, 0x18-byte records (loader `0x546f00`); the five masks are
/// race bitmasks (`0x4799a0`).
pub(crate) fn helmet_vis_schema() -> Schema {
    let mut s = Schema::new("HelmetGeosetVisData");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("HideHair", FieldType::UInt32),
        ("HideFacial1", FieldType::UInt32),
        ("HideFacial2", FieldType::UInt32),
        ("HideFacial3", FieldType::UInt32),
        ("HideEars", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// CharacterFacialHairStyles.dbc: 9 fields, no ID column.
pub(crate) fn char_facial_hair_schema() -> Schema {
    let mut s = Schema::new("CharacterFacialHairStyles");
    for (name, ty) in [
        ("RaceID", FieldType::UInt32),
        ("SexID", FieldType::UInt32),
        ("VariationID", FieldType::UInt32),
        ("Unused3", FieldType::UInt32),
        ("Unused4", FieldType::UInt32),
        ("Unused5", FieldType::UInt32),
        ("Geoset100", FieldType::UInt32),
        ("Geoset300", FieldType::UInt32),
        ("Geoset200", FieldType::UInt32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gloves, boots, a robe and a cloak replace their groups; a robe hides the tabard flap.
    #[test]
    fn equipment_geoset_branches() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        let naked = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
        assert!(naked.contains(&401) && naked.contains(&1101) && naked.contains(&1501));

        let mut eq = EquipGeosets::default();
        eq.bodyslots[6] = Some([1, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&402) && !set.contains(&401));

        // Boots take the bare foot, 501, off (`0x477639`).
        let mut eq = EquipGeosets::default();
        eq.bodyslots[4] = Some([2, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&503) && !set.contains(&501));

        // A robe shows its skirt and hides the leg groups and the tabard flap.
        let mut eq = EquipGeosets::default();
        eq.bodyslots[1] = Some([1, 0, 1]);
        eq.bodyslots[3] = Some([2, 1, 0]);
        eq.bodyslots[4] = Some([2, 0, 0]);
        eq.bodyslots[7] = Some([1, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1302), "robe skirt on");
        assert!(
            !set.contains(&1101) && !set.contains(&1301),
            "pant legs off"
        );
        assert!(!set.contains(&501) && !set.contains(&503), "boot group off");
        assert!(!set.contains(&1202), "tabard suppressed under a robe");
        assert!(set.contains(&802), "the chest's sleeves still show");

        let eq = EquipGeosets {
            cloak: Some(4),
            ..Default::default()
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1505) && !set.contains(&1501));
    }

    /// Branch B3 (`0x4775bd`) keys the shirt's cuff on the forearm tile, not the chest slot.
    #[test]
    fn b3_shirt_sleeve_follows_the_forearm_tile_not_the_chest_slot() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        let shirt = |dressed: bool| EquipGeosets {
            bodyslots: [Some([1, 0, 0]), None, None, None, None, None, None, None],
            forearm_dressed: dressed,
            ..Default::default()
        };
        assert!(
            cg.visible_geosets(1, 0, 0, 0, &shirt(false)).contains(&802),
            "bare forearm: the shirt's cuff shows"
        );
        assert!(
            !cg.visible_geosets(1, 0, 0, 0, &shirt(true)).contains(&802),
            "something else paints the forearm: the cuff goes"
        );
        // A chest with no sleeve geoset (B2 would add one to group 8 too) leaves the cuff.
        let mut eq = shirt(false);
        eq.bodyslots[1] = Some([0, 0, 0]);
        assert!(
            cg.visible_geosets(1, 0, 0, 0, &eq).contains(&802),
            "an equipped chest alone does not hide the shirt's cuff"
        );
    }

    /// Branch B6 (`0x477752`): the open designer wears the flap over an empty slot, unless robed.
    #[test]
    fn b6_tabard_preview_wears_the_flap_with_an_empty_slot() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
        assert!(!set.contains(&1202), "no tabard, no designer: no flap");
        let preview = EquipGeosets {
            tabard_preview: true,
            ..Default::default()
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &preview);
        assert!(set.contains(&1202), "the designer forces the flap on");
        assert!(set.contains(&1201), "and the group base with it");
        for slot in [1usize, 3] {
            let mut eq = preview;
            eq.bodyslots[slot] = Some([0, 0, 1]);
            assert!(
                !cg.visible_geosets(1, 0, 0, 0, &eq).contains(&1202),
                "a robe (slot {slot}) hides the previewed flap"
            );
        }
    }

    /// Branch B7 is off under a chest robe or a tabard (`0x4777a4`, `0x4777b8`), not a legs robe.
    #[test]
    fn b7_is_gated_by_the_chest_robe_and_the_tabard() {
        let cg = CharacterGeosets {
            hair: HashMap::new(),
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        // Shirt doublet g1=1 → 1002; pants g0=1 → 1103.
        let base = EquipGeosets {
            bodyslots: [
                Some([0, 1, 0]),
                None,
                None,
                Some([1, 0, 0]),
                None,
                None,
                None,
                None,
            ],
            ..Default::default()
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &base);
        assert!(
            set.contains(&1002) && set.contains(&1103),
            "ungated: both on"
        );

        let mut eq = base;
        eq.bodyslots[7] = Some([1, 0, 0]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1202), "the tabard's own flap is on");
        assert!(
            !set.contains(&1002) && !set.contains(&1103),
            "a tabard skips B7 whole"
        );

        let mut eq = base;
        eq.bodyslots[1] = Some([0, 0, 1]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(
            !set.contains(&1002) && !set.contains(&1103),
            "a chest robe skips B7 whole"
        );

        // A legs robe: B4 hides the pant-leg group, then B7 enables its geosets anyway.
        let mut eq = base;
        eq.bodyslots[3] = Some([1, 0, 1]);
        let set = cg.visible_geosets(1, 0, 0, 0, &eq);
        assert!(set.contains(&1002), "leg robes do not gate B7");
        assert!(
            set.contains(&1103),
            "and B7 re-enables the pant leg after B4-long"
        );
    }

    /// The helm vis row (`0x4799a0`) is picked by sex, and a race bit forces the group bases.
    #[test]
    fn helm_vis_forces_group_bases_by_race() {
        let mut hair = HashMap::new();
        hair.insert((1, 0, 1), 5u32); // styled hair → geoset 5
        let mut facial = HashMap::new();
        facial.insert((1, 0, 1), [2u32, 3, 4]); // styled facial → 102/303/204
        let mut helmet_vis = HashMap::new();
        helmet_vis.insert(368, [u32::MAX; 5]); // a shipped row id, hiding everything
        helmet_vis.insert(245, [0u32; 5]); // a shipped row id, hiding nothing
        let cg = CharacterGeosets {
            hair,
            facial,
            helmet_vis,
        };
        let helm = EquipGeosets {
            helm_vis: Some([368, 245]),
            ..Default::default()
        };
        // Male picks row 368.
        let set = cg.visible_geosets(1, 0, 1, 1, &helm);
        assert!(set.contains(&1) && !set.contains(&5), "hair → bare scalp");
        assert!(set.contains(&101) && !set.contains(&102));
        assert!(set.contains(&201) && !set.contains(&204));
        assert!(set.contains(&301) && !set.contains(&303));
        assert!(set.contains(&701) && !set.contains(&702), "ears → 701");
        // Female picks row 245.
        let mut hair = HashMap::new();
        hair.insert((1, 1, 1), 5u32);
        let cg2 = CharacterGeosets {
            hair,
            facial: HashMap::new(),
            helmet_vis: cg.helmet_vis.clone(),
        };
        let set = cg2.visible_geosets(1, 1, 1, 1, &helm);
        assert!(
            set.contains(&5) && set.contains(&702),
            "row 245 hides nothing"
        );
        // A mask covering only race 2 leaves a race-1 wearer styled.
        let mut helmet_vis = HashMap::new();
        helmet_vis.insert(300, [1u32 << 2; 5]);
        let cg3 = CharacterGeosets {
            hair: cg2.hair.clone(),
            facial: HashMap::new(),
            helmet_vis,
        };
        let masked = EquipGeosets {
            helm_vis: Some([300, 300]),
            ..Default::default()
        };
        let set = cg3.visible_geosets(1, 1, 1, 1, &masked);
        assert!(set.contains(&5), "race bit miss → style kept");
    }

    /// A Human male's naked set, shipped rows: hairStyle 1 → geoset 2, facialHair 1 → 1/2/1.
    #[test]
    fn naked_human_male_selection() {
        let mut hair = HashMap::new();
        hair.insert((1, 0, 1), 2u32);
        let mut facial = HashMap::new();
        facial.insert((1, 0, 1), [1u32, 2, 1]);
        let cg = CharacterGeosets {
            hair,
            facial,
            helmet_vis: HashMap::new(),
        };
        let set = cg.visible_geosets(1, 0, 1, 1, &EquipGeosets::default());

        assert!(set.contains(&0), "body geoset 0 always on");
        assert!(set.contains(&2), "hair: max(1, GeosetID 2)");
        assert!(set.contains(&101), "facial group 1: gA 1 + 100");
        assert!(set.contains(&302), "facial group 3: gB 2 + 300");
        assert!(set.contains(&201), "facial group 2: gC 1 + 200");
        assert!(set.contains(&401) && set.contains(&702) && set.contains(&1501));
        // Geoset 5 is hairStyle 4, not chosen.
        assert!(!set.contains(&5), "unselected hair variant hidden");
    }

    /// A bald style (GeosetID 0) resolves to geoset 1, the scalp, not 0, the body.
    #[test]
    fn bald_resolves_to_scalp_geoset_one() {
        let mut hair = HashMap::new();
        hair.insert((1, 0, 0), 0u32); // variation 0 → GeosetID 0
        let cg = CharacterGeosets {
            hair,
            facial: HashMap::new(),
            helmet_vis: HashMap::new(),
        };
        let set = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
        assert!(set.contains(&1), "max(1, 0) = scalp geoset 1");
    }

    /// The shipped CharHairGeosets repeats only the goblin male's one style, and the first row,
    /// geoset 1, the bare scalp, wins; geoset 2 adds an untextured topknot.
    #[test]
    fn goblin_male_hair_takes_the_first_of_four_duplicate_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // The file's rows, read without the loader.
        let bytes = chain.read_file(CHAR_HAIR_GEOSETS).expect("CharHairGeosets");
        let rs = parse(&bytes, char_hair_geosets_schema(), "CharHairGeosets").expect("parse");
        let mut keys: HashMap<(u32, u32, u32), Vec<u32>> = HashMap::new();
        for r in rs.records() {
            if let (Some(race), Some(sex), Some(var), Some(geoset)) =
                (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3), u32_at(r, 4))
            {
                keys.entry((race, sex, var)).or_default().push(geoset);
            }
        }
        let dups: Vec<_> = keys.iter().filter(|(_, v)| v.len() > 1).collect();
        assert_eq!(
            dups.len(),
            1,
            "exactly one duplicated (race,sex,variation) key ships; got {dups:?}"
        );
        assert_eq!(
            keys.get(&(9, 0, 0)).map(Vec::as_slice),
            Some([1u32, 2, 1, 2].as_slice()),
            "the goblin-male block is the duplicate, in file order"
        );

        let cg = CharacterGeosets::load(&mut chain).expect("load customization tables");
        let set = cg.visible_geosets(9, 0, 0, 0, &EquipGeosets::default());
        assert!(
            set.contains(&1),
            "goblin male shows the bare scalp (geoset 1)"
        );
        assert!(
            !set.contains(&2),
            "goblin male does NOT show geoset 2 — the white topknot of B11"
        );
        // Race 9 ships only sex-0 rows, so the goblin female keeps the region base, the same scalp.
        let f = cg.visible_geosets(9, 1, 0, 0, &EquipGeosets::default());
        assert!(f.contains(&1) && !f.contains(&2), "goblin female unchanged");
    }
}
